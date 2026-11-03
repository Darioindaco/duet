//! Plugins (VST3, AU) in a process of their own: if one crashes, that process goes, Duet and your
//! session don't. The plugin goes quiet (an effect lets its sound through), and the UI offers to load
//! it again.
//!
//! One plugin process holds every sandboxed plugin (and their windows). Per block, for each plugin:
//! Duet writes the block (audio, notes, controllers, parameter moves) into the plugin's slot of a
//! shared memory file, wakes the plugin's thread over there with a named semaphore, and waits (a
//! short spin) for it to say done. Nothing more is buffered, so it adds no latency. Everything that
//! isn't per block (making a plugin, its parameters, its state, its window) goes as a line of JSON
//! over the plugin process's stdin / stdout.
//!
//! The plugin process is Duet itself started with `--plugin-host <shm file> <semaphore prefix>`
//! (duet.py hands the command line over with set_command), running host_main below.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use memmap2::MmapMut;
use serde_json::{json, Value};

use crate::vst::{ParamInfo, PluginInfo, VstProc, MAX_BLOCK, NEXT_ID};

pub const SLOTS: usize = 128;
pub const EVENTS: usize = 256;

/// Plugins in their own process (the setting; off: in Duet's, as before).
pub static ENABLED: AtomicBool = AtomicBool::new(true);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Ev {
    /// 1 note on, 2 note off, 3 controller, 4 pitch wheel, 5 parameter
    kind: u32,
    a: u32,
    b: u32,
    offset: u32,
    val: f32,
}

#[repr(C)]
struct SlotShm {
    req: AtomicU64,
    done: AtomicU64,
    n: u32,
    nev: u32,
    mix: f32,
    _pad: u32,
    tempo: f64,
    ev: [Ev; EVENTS],
    l: [f32; MAX_BLOCK],
    r: [f32; MAX_BLOCK],
}

fn shm_len() -> usize {
    SLOTS * std::mem::size_of::<SlotShm>()
}

// --- named semaphores (one per slot: Duet posts, the plugin's thread waits) -------------------------

#[cfg(unix)]
struct Sem(*mut libc::sem_t, std::ffi::CString, bool);
#[cfg(unix)]
impl Sem {
    fn create(name: &str) -> Option<Sem> {
        let c = std::ffi::CString::new(name).ok()?;
        unsafe {
            libc::sem_unlink(c.as_ptr());
            let s = libc::sem_open(c.as_ptr(), libc::O_CREAT | libc::O_EXCL, 0o600 as libc::c_uint, 0 as libc::c_uint);
            (s != libc::SEM_FAILED).then(|| Sem(s, c, true))
        }
    }
    fn open(name: &str) -> Option<Sem> {
        let c = std::ffi::CString::new(name).ok()?;
        unsafe {
            let s = libc::sem_open(c.as_ptr(), 0);
            (s != libc::SEM_FAILED).then(|| Sem(s, c, false))
        }
    }
    fn post(&self) {
        unsafe { libc::sem_post(self.0) };
    }
    fn wait(&self) {
        unsafe {
            while libc::sem_wait(self.0) != 0 && *errno() == libc::EINTR {}
        }
    }
}
#[cfg(target_os = "macos")]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__error() }
}
#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}
#[cfg(unix)]
impl Drop for Sem {
    fn drop(&mut self) {
        unsafe {
            libc::sem_close(self.0);
            if self.2 {
                libc::sem_unlink(self.1.as_ptr());
            }
        }
    }
}

#[cfg(windows)]
struct Sem(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Sem {
    fn wide(name: &str) -> Vec<u16> {
        format!("Local\\{}", name.trim_start_matches('/')).encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn create(name: &str) -> Option<Sem> {
        let w = Self::wide(name);
        let h = unsafe { windows_sys::Win32::System::Threading::CreateSemaphoreW(std::ptr::null(), 0, i32::MAX, w.as_ptr()) };
        (!h.is_null()).then_some(Sem(h))
    }
    fn open(name: &str) -> Option<Sem> {
        let w = Self::wide(name);
        // SEMAPHORE_MODIFY_STATE | SYNCHRONIZE
        let h = unsafe { windows_sys::Win32::System::Threading::OpenSemaphoreW(0x0002 | 0x0010_0000, 0, w.as_ptr()) };
        (!h.is_null()).then_some(Sem(h))
    }
    fn post(&self) {
        unsafe { windows_sys::Win32::System::Threading::ReleaseSemaphore(self.0, 1, std::ptr::null_mut()) };
    }
    fn wait(&self) {
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, u32::MAX) };
    }
}
#[cfg(windows)]
impl Drop for Sem {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

unsafe impl Send for Sem {}
unsafe impl Sync for Sem {}

fn sem_name(prefix: &str, slot: usize) -> String {
    format!("/{prefix}-{slot}")
}

// --- Duet's side ----------------------------------------------------------------------------------

static COMMAND: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// How to start the plugin process (Duet's own command line, from duet.py).
pub fn set_command(argv: Vec<String>) {
    *COMMAND.lock().unwrap() = Some(argv);
}

pub fn available() -> bool {
    ENABLED.load(Relaxed) && COMMAND.lock().unwrap().is_some()
}

struct Io {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct Sandbox {
    io: Mutex<Io>,
    map: Mutex<MmapMut>,
    base: *mut SlotShm,
    sems: Vec<Sem>,
    free: Mutex<Vec<usize>>,
    /// the plugin process is gone (crashed, or killed): its plugins are quiet until loaded again
    pub dead: AtomicBool,
    pub pid: u32,
    _file: PathBuf,
}
unsafe impl Send for Sandbox {}
unsafe impl Sync for Sandbox {}

static CURRENT: Mutex<Option<Arc<Sandbox>>> = Mutex::new(None);
/// How many times the plugin process went down (the UI says so once each time).
pub static CRASHES: AtomicU64 = AtomicU64::new(0);

/// The running plugin process (started the first time; again after a crash).
pub fn get() -> Result<Arc<Sandbox>, String> {
    let mut cur = CURRENT.lock().unwrap();
    if let Some(sb) = cur.as_ref() {
        if !sb.dead.load(Relaxed) {
            return Ok(sb.clone());
        }
    }
    let sb = Arc::new(Sandbox::start()?);
    *cur = Some(sb.clone());
    Ok(sb)
}

impl Sandbox {
    fn start() -> Result<Sandbox, String> {
        let argv = COMMAND.lock().unwrap().clone().ok_or("no plugin process command")?;
        let tag = format!("dpl{}x{}", std::process::id(), NEXT_ID.fetch_add(1, Relaxed));
        let file = std::env::temp_dir().join(format!("{tag}.shm"));
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&file)
            .map_err(|e| format!("shared memory: {e}"))?;
        f.set_len(shm_len() as u64).map_err(|e| format!("shared memory: {e}"))?;
        let mut map = unsafe { MmapMut::map_mut(&f) }.map_err(|e| format!("shared memory: {e}"))?;
        let base = map.as_mut_ptr() as *mut SlotShm;
        let sems: Vec<Sem> = (0..SLOTS)
            .map(|k| Sem::create(&sem_name(&tag, k)).ok_or_else(|| "can't make a semaphore".to_string()))
            .collect::<Result<_, _>>()?;
        let mut child: Child = Command::new(&argv[0])
            .args(&argv[1..])
            .arg("--plugin-host")
            .arg(&file)
            .arg(&tag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("can't start the plugin process: {e}"))?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        // the plugin process says hello once it has the shared memory and the semaphores
        let mut line = String::new();
        stdout.read_line(&mut line).map_err(|e| format!("plugin process: {e}"))?;
        if !line.contains("\"ready\"") {
            let _ = child.kill();
            return Err(format!("the plugin process didn't start: {}", line.trim()));
        }
        let sb = Sandbox {
            io: Mutex::new(Io { stdin, stdout }),
            map: Mutex::new(map),
            base,
            sems,
            free: Mutex::new((0..SLOTS).rev().collect()),
            dead: AtomicBool::new(false),
            pid,
            _file: file,
        };
        Ok(sb).inspect(|_| {
            // watch it: when it goes, its plugins go quiet (and the next one starts a new process)
            std::thread::Builder::new()
                .name("duet-plugin-watch".into())
                .spawn(move || {
                    let _ = child.wait();
                    if let Some(sb) = CURRENT.lock().unwrap().as_ref() {
                        if sb.pid == pid && !sb.dead.swap(true, SeqCst) {
                            CRASHES.fetch_add(1, SeqCst);
                        }
                    }
                })
                .ok();
        })
    }

    fn request(&self, msg: Value) -> Result<Value, String> {
        if self.dead.load(Relaxed) {
            return Err("the plugin process is gone".into());
        }
        let mut io = self.io.lock().unwrap();
        writeln!(io.stdin, "{msg}").and_then(|_| io.stdin.flush()).map_err(|e| format!("plugin process: {e}"))?;
        let mut line = String::new();
        let n = io.stdout.read_line(&mut line).map_err(|e| format!("plugin process: {e}"))?;
        if n == 0 {
            self.dead.store(true, SeqCst);
            return Err("the plugin process is gone".into());
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("plugin process: {e}"))?;
        match v.get("error").and_then(Value::as_str) {
            Some(e) => Err(e.to_string()),
            None => Ok(v),
        }
    }

    fn slot(&self, k: usize) -> &mut SlotShm {
        unsafe { &mut *self.base.add(k) }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self._file);
        let _ = self.map.lock().map(|m| m.len());
    }
}

/// Which plugins run in the plugin process: Duet's id for them -> (the process, its id over there).
static REMOTES: OnceLock<Mutex<std::collections::HashMap<u64, (Arc<Sandbox>, u64)>>> = OnceLock::new();
fn remotes() -> &'static Mutex<std::collections::HashMap<u64, (Arc<Sandbox>, u64)>> {
    REMOTES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// A plugin running in the plugin process, as the mixer and the instruments use it (see VstProc).
pub struct RemoteProc {
    sb: Arc<Sandbox>,
    slot: usize,
    remote: u64,
    pub id: u64,
    pub info: PluginInfo,
    instrument: bool,
    events: Vec<Ev>,
    seq: u64,
    sr: f64,
    /// blocks the plugin didn't finish in time (it was too slow, or it hung)
    pub late: u64,
}

impl RemoteProc {
    pub fn create(path: &str, cid: &str, state: Option<&str>, sr: f64, instrument: bool) -> Result<RemoteProc, String> {
        let sb = get()?;
        let slot = sb.free.lock().unwrap().pop().ok_or("too many plugins in the plugin process")?;
        let r = sb.request(json!({"op": "create", "slot": slot, "path": path, "cid": cid, "state": state, "sr": sr, "inst": instrument}));
        let v = match r {
            Ok(v) => v,
            Err(e) => {
                sb.free.lock().unwrap().push(slot);
                return Err(e);
            }
        };
        let id = NEXT_ID.fetch_add(1, Relaxed);
        let remote = v["id"].as_u64().unwrap_or(0);
        remotes().lock().unwrap().insert(id, (sb.clone(), remote));
        let info = PluginInfo {
            name: v["name"].as_str().unwrap_or("Plugin").to_string(),
            latency: v["latency"].as_u64().unwrap_or(0) as u32,
            has_editor: v["editor"].as_bool().unwrap_or(false),
        };
        let s = sb.slot(slot);
        s.req.store(0, SeqCst);
        s.done.store(0, SeqCst);
        Ok(RemoteProc { sb, slot, remote, id, info, instrument, events: Vec::with_capacity(EVENTS), seq: 0, sr, late: 0 })
    }

    pub fn crashed(&self) -> bool {
        self.sb.dead.load(Relaxed)
    }

    fn push(&mut self, e: Ev) {
        if self.events.len() < EVENTS {
            self.events.push(e);
        }
    }
    pub fn note(&mut self, on: bool, key: u8, vel: u8, offset: usize) {
        self.push(Ev { kind: if on { 1 } else { 2 }, a: key as u32, b: vel as u32, offset: offset as u32, val: 0.0 });
    }
    pub fn midi_cc(&mut self, n: u8, v: u8) {
        self.push(Ev { kind: 3, a: n as u32, b: v as u32, offset: 0, val: 0.0 });
    }
    pub fn midi_bend(&mut self, b: f32) {
        self.push(Ev { kind: 4, a: 0, b: 0, offset: 0, val: b });
    }
    pub fn set_param(&mut self, id: u32, v: f32) {
        // (the last move of a parameter in a block is enough)
        if let Some(e) = self.events.iter_mut().find(|e| e.kind == 5 && e.a == id) {
            e.val = v;
            return;
        }
        self.push(Ev { kind: 5, a: id, b: 0, offset: 0, val: v });
    }

    pub fn set_sample_rate(&mut self, sr: f64) {
        self.sr = sr;
        let _ = self.sb.request(json!({"op": "sr", "slot": self.slot, "sr": sr}));
    }

    /// One block: over to the plugin process and back. Gone or too slow: an effect lets its sound
    /// through, an instrument is quiet.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], mix: f32, tempo: f64) {
        let n = l.len().min(r.len()).min(MAX_BLOCK);
        let quiet = |l: &mut [f32], r: &mut [f32], inst: bool| {
            if inst {
                l.fill(0.0);
                r.fill(0.0);
            }
        };
        if self.crashed() {
            self.events.clear();
            quiet(l, r, self.instrument);
            return;
        }
        let s = self.sb.slot(self.slot);
        s.n = n as u32;
        s.mix = mix;
        s.tempo = tempo;
        s.nev = self.events.len() as u32;
        s.ev[..self.events.len()].copy_from_slice(&self.events);
        self.events.clear();
        s.l[..n].copy_from_slice(&l[..n]);
        s.r[..n].copy_from_slice(&r[..n]);
        self.seq += 1;
        s.req.store(self.seq, Release);
        self.sb.sems[self.slot].post();
        // a whole block's time at most (then it's a dropout either way)
        let deadline = Instant::now() + Duration::from_secs_f64((n as f64 / self.sr.max(8000.0)).max(0.0005));
        let mut spins = 0u32;
        loop {
            if s.done.load(Acquire) >= self.seq {
                l[..n].copy_from_slice(&s.l[..n]);
                r[..n].copy_from_slice(&s.r[..n]);
                return;
            }
            spins += 1;
            if spins & 63 == 0 && (Instant::now() > deadline || self.crashed()) {
                self.late += 1;
                quiet(l, r, self.instrument);
                return;
            }
            std::hint::spin_loop();
        }
    }
}

impl Drop for RemoteProc {
    fn drop(&mut self) {
        remotes().lock().unwrap().remove(&self.id);
        if !self.crashed() {
            let _ = self.sb.request(json!({"op": "destroy", "slot": self.slot}));
        }
        self.sb.free.lock().unwrap().push(self.slot);
    }
}

fn remote(id: u64) -> Option<(Arc<Sandbox>, u64)> {
    remotes().lock().unwrap().get(&id).cloned()
}

pub fn is_remote(id: u64) -> bool {
    remote(id).is_some()
}

/// A plugin in the plugin process that went down with it.
pub fn is_crashed(id: u64) -> bool {
    remote(id).is_some_and(|(sb, _)| sb.dead.load(Relaxed))
}

pub fn plugin_params(id: u64) -> Option<Vec<ParamInfo>> {
    let (sb, rid) = remote(id)?;
    let v = sb.request(json!({"op": "params", "id": rid})).ok()?;
    Some(v["params"].as_array()?.iter().map(|p| ParamInfo {
        id: p["id"].as_u64().unwrap_or(0) as u32,
        name: p["name"].as_str().unwrap_or("").to_string(),
        default: p["default"].as_f64().unwrap_or(0.0) as f32,
        value: p["value"].as_f64().unwrap_or(0.0) as f32,
        steps: p["steps"].as_i64().unwrap_or(0) as i32,
    }).collect())
}

pub fn plugin_touched(id: u64) -> Option<Option<u32>> {
    let (sb, rid) = remote(id)?;
    let v = sb.request(json!({"op": "touched", "id": rid})).ok()?;
    Some(v["touched"].as_u64().map(|x| x as u32))
}

pub fn plugin_state(id: u64) -> Option<Option<String>> {
    let (sb, rid) = remote(id)?;
    let v = sb.request(json!({"op": "state", "id": rid})).ok()?;
    Some(v["state"].as_str().map(str::to_string))
}

pub fn plugin_editor(id: u64, title: String) -> Option<Result<(), String>> {
    let (sb, rid) = remote(id)?;
    Some(sb.request(json!({"op": "editor", "id": rid, "title": title})).map(|_| ()))
}

/// For tests: end the plugin process as a crash would.
pub fn kill_for_test() -> bool {
    let cur = CURRENT.lock().unwrap().clone();
    match cur {
        Some(sb) => {
            #[cfg(unix)]
            unsafe {
                libc::kill(sb.pid as i32, libc::SIGKILL);
            }
            #[cfg(windows)]
            let _ = Command::new("taskkill").args(["/F", "/PID", &sb.pid.to_string()]).status();
            true
        }
        None => false,
    }
}

// --- the plugin process -----------------------------------------------------------------------------

struct Hosted {
    proc: Arc<Mutex<VstProc>>,
    quit: Arc<AtomicBool>,
}

/// The plugin process's main: `--plugin-host <shm file> <semaphore prefix>`. Answers Duet's requests
/// on stdin / stdout until stdin closes (Duet quit or crashed: then this goes too).
pub fn host_main(file: String, tag: String) -> Result<(), String> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(&file).map_err(|e| e.to_string())?;
    let mut map = unsafe { MmapMut::map_mut(&f) }.map_err(|e| e.to_string())?;
    if map.len() < shm_len() {
        return Err("the shared memory is too small".into());
    }
    let base = map.as_mut_ptr() as usize;
    let sems: Arc<Vec<Sem>> = Arc::new((0..SLOTS).map(|k| Sem::open(&sem_name(&tag, k)).ok_or("no semaphore")).collect::<Result<_, _>>()?);
    // plugin windows: on a Mac they live on the main thread, which runs the app's event loop; the
    // requests are answered on a thread of their own
    #[cfg(target_os = "macos")]
    {
        std::env::set_var("DUET_VST_MAIN_THREAD", "1");
        let sems2 = sems.clone();
        std::thread::spawn(move || {
            serve(base, sems2);
            std::process::exit(0);
        });
        mac_run();
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        serve(base, sems);
        drop(map);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn mac_run() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let mtm = MainThreadMarker::new().expect("the plugin process starts on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.run();
}

fn serve(base: usize, sems: Arc<Vec<Sem>>) {
    let stdout = std::io::stdout();
    let say = |v: Value| {
        let mut o = stdout.lock();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    };
    say(json!({"ready": true}));
    let mut hosted: std::collections::HashMap<usize, Hosted> = std::collections::HashMap::new();
    let mut by_id: std::collections::HashMap<u64, Arc<Mutex<VstProc>>> = std::collections::HashMap::new();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let m: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                say(json!({"error": e.to_string()}));
                continue;
            }
        };
        let op = m["op"].as_str().unwrap_or("");
        let slot = m["slot"].as_u64().unwrap_or(u64::MAX) as usize;
        let reply = match op {
            "create" if slot < SLOTS => {
                let made = VstProc::create_as(
                    m["path"].as_str().unwrap_or(""), m["cid"].as_str().unwrap_or(""), m["state"].as_str(),
                    m["sr"].as_f64().unwrap_or(48000.0), m["inst"].as_bool().unwrap_or(false));
                match made {
                    Ok(p) => {
                        let info = p.info();
                        let id = p.id();
                        let proc = Arc::new(Mutex::new(p));
                        let quit = Arc::new(AtomicBool::new(false));
                        let (pr, q, ss) = (proc.clone(), quit.clone(), sems.clone());
                        std::thread::Builder::new()
                            .name(format!("duet-plugin-{slot}"))
                            .spawn(move || run_slot(base, slot, &ss[slot], pr, q))
                            .ok();
                        by_id.insert(id, proc.clone());
                        hosted.insert(slot, Hosted { proc, quit });
                        json!({"id": id, "name": info.name, "latency": info.latency, "editor": info.has_editor})
                    }
                    Err(e) => json!({"error": e}),
                }
            }
            "destroy" => {
                if let Some(h) = hosted.remove(&slot) {
                    h.quit.store(true, SeqCst);
                    sems[slot].post();
                    let id = h.proc.lock().unwrap().id();
                    by_id.remove(&id);
                }
                json!({"ok": true})
            }
            "sr" => {
                if let Some(h) = hosted.get(&slot) {
                    h.proc.lock().unwrap().set_sample_rate(m["sr"].as_f64().unwrap_or(48000.0));
                }
                json!({"ok": true})
            }
            "params" => {
                let id = m["id"].as_u64().unwrap_or(0);
                match crate::vst::plugin_params(id) {
                    Some(ps) => json!({"params": ps.iter().map(|p| json!({"id": p.id, "name": p.name, "default": p.default,
                                                                          "value": p.value, "steps": p.steps})).collect::<Vec<_>>()}),
                    None => json!({"error": "that plugin isn't running"}),
                }
            }
            "touched" => json!({"touched": crate::vst::plugin_touched(m["id"].as_u64().unwrap_or(0))}),
            "state" => json!({"state": crate::vst::plugin_state(m["id"].as_u64().unwrap_or(0))}),
            "editor" => match crate::vst::plugin_editor(m["id"].as_u64().unwrap_or(0), m["title"].as_str().unwrap_or("Plugin").to_string()) {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"error": e}),
            },
            _ => json!({"error": format!("unknown request {op}")}),
        };
        say(reply);
    }
    // Duet is gone: so are we
    for h in hosted.values() {
        h.quit.store(true, SeqCst);
    }
}

fn run_slot(base: usize, slot: usize, sem: &Sem, proc: Arc<Mutex<VstProc>>, quit: Arc<AtomicBool>) {
    let s = unsafe { &mut *(base as *mut SlotShm).add(slot) };
    let (mut l, mut r) = (vec![0f32; MAX_BLOCK], vec![0f32; MAX_BLOCK]);
    loop {
        sem.wait();
        if quit.load(SeqCst) {
            return;
        }
        let req = s.req.load(Acquire);
        if req <= s.done.load(Acquire) {
            continue;
        }
        let n = (s.n as usize).min(MAX_BLOCK);
        let nev = (s.nev as usize).min(EVENTS);
        l[..n].copy_from_slice(&s.l[..n]);
        r[..n].copy_from_slice(&s.r[..n]);
        {
            let mut p = proc.lock().unwrap();
            for e in &s.ev[..nev] {
                match e.kind {
                    1 => p.note(true, e.a as u8, e.b as u8, e.offset as usize),
                    2 => p.note(false, e.a as u8, e.b as u8, e.offset as usize),
                    3 => p.midi_cc(e.a as u8, e.b as u8),
                    4 => p.midi_bend(e.val),
                    5 => p.set_param(e.a, e.val),
                    _ => {}
                }
            }
            p.process(&mut l[..n], &mut r[..n], s.mix, s.tempo);
        }
        s.l[..n].copy_from_slice(&l[..n]);
        s.r[..n].copy_from_slice(&r[..n]);
        s.done.store(req, Release);
    }
}
