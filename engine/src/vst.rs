//! VST3 plugin hosting (VST3 SDK interfaces via the `vst3` bindings; the SDK
//! itself is MIT-licensed since 3.8).
//!
//! Threads:
//! - The *plugin thread* does everything VST3 wants on a UI thread: loading
//!   modules, creating and initialising components/controllers, state, and the
//!   editor windows. On Windows it's a dedicated thread with its own message
//!   loop; on macOS it's the main thread when Duet runs its window there
//!   (DUET_VST_MAIN_THREAD=1, set by duet.py), else a plain worker thread.
//! - The *audio thread* only ever calls `IAudioProcessor::process`, with
//!   buffers, parameter queues and the event list all preallocated, and edits
//!   from the plugin's editor arriving through a lock-free queue.
//!
//! Plugins run in-process (no extra latency); a crashing plugin takes Duet
//! down with it, as in most hosts.

#![allow(non_upper_case_globals, non_snake_case)]

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};

use base64::Engine as _;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

pub const MAX_BLOCK: usize = 1024;
const MAX_PARAM_EDITS: usize = 64;

/// A ComPtr that we promise to use correctly across threads (VST3 objects are
/// plain COM; the threading rules are the plugin API's, which we follow).
pub struct Shared<T>(pub T);
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

// --- the plugin thread ---------------------------------------------------------

type Job = Box<dyn FnOnce() + Send>;

struct UiThread {
    tx: Mutex<mpsc::Sender<Job>>,
    #[cfg(windows)]
    thread_id: u32,
}

static UI: OnceLock<UiThread> = OnceLock::new();

fn ui() -> &'static UiThread {
    UI.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel();
        #[cfg(target_os = "macos")]
        if std::env::var_os("DUET_VST_MAIN_THREAD").is_some() {
            // jobs are dispatched to the main queue instead (see on_ui); the
            // channel is unused
            drop(rx);
            return UiThread { tx: Mutex::new(tx) };
        }
        std::thread::Builder::new()
            .name("duet-plugins".into())
            .spawn(move || {
                ON_PLUGIN_THREAD.with(|t| t.set(true));
                #[cfg(windows)]
                {
                    win::thread_init();
                    let _ = ready_tx.send(win::current_thread_id());
                    win::message_loop(rx);
                }
                #[cfg(not(windows))]
                {
                    let _ = ready_tx.send(0u32);
                    for job in rx {
                        job();
                    }
                }
            })
            .expect("plugin thread");
        let _tid = ready_rx.recv().unwrap_or(0);
        UiThread {
            tx: Mutex::new(tx),
            #[cfg(windows)]
            thread_id: _tid,
        }
    })
}

thread_local! {
    static ON_PLUGIN_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` on the plugin thread and wait for its result. Already there (e.g. an
/// instance released inside a plugin-thread job): just run it, don't deadlock.
pub fn on_ui<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    if ON_PLUGIN_THREAD.with(|t| t.get()) {
        return f();
    }
    #[cfg(target_os = "macos")]
    if std::env::var_os("DUET_VST_MAIN_THREAD").is_some() {
        return mac::on_main(f);
    }
    let t = ui();
    let (rtx, rrx) = mpsc::channel();
    let job: Job = Box::new(move || {
        let _ = rtx.send(f());
    });
    t.tx.lock().unwrap().send(job).expect("plugin thread gone");
    #[cfg(windows)]
    win::wake(t.thread_id);
    rrx.recv().expect("plugin thread dropped a job")
}

// --- modules -------------------------------------------------------------------

struct Module {
    _lib: libloading::Library,
    factory: ComPtr<IPluginFactory>,
}

/// Modules stay loaded for the life of the process: unloading plugin DLLs is a
/// classic source of crashes, and they're shared between instances anyway.
fn modules() -> &'static Mutex<HashMap<PathBuf, Arc<Shared<Module>>>> {
    static M: OnceLock<Mutex<HashMap<PathBuf, Arc<Shared<Module>>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// The binary inside a .vst3 (a bundle folder, or on Windows also a bare file).
fn binary_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let stem = path.file_stem().ok_or("bad plugin path")?;
    #[cfg(windows)]
    let candidates = {
        let arch = if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" };
        vec![path.join("Contents").join(arch).join(path.file_name().unwrap())]
    };
    #[cfg(target_os = "macos")]
    let candidates = vec![path.join("Contents").join("MacOS").join(stem)];
    #[cfg(not(any(windows, target_os = "macos")))]
    let candidates = vec![path.join("Contents").join("x86_64-linux").join(format!("{}.so", stem.to_string_lossy()))];
    let _ = stem;
    candidates.into_iter().find(|p| p.exists()).ok_or_else(|| format!("no binary inside {}", path.display()))
}

/// Load (or reuse) a module. Must run on the plugin thread.
fn load_module(path: &Path) -> Result<Arc<Shared<Module>>, String> {
    if let Some(m) = modules().lock().unwrap().get(path) {
        return Ok(m.clone());
    }
    let bin = binary_path(path)?;
    unsafe {
        let lib = libloading::Library::new(&bin).map_err(|e| format!("couldn't load {}: {e}", bin.display()))?;
        #[cfg(windows)]
        if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
            init();
        }
        #[cfg(target_os = "macos")]
        mac::bundle_entry(&lib, path)?;
        let get: libloading::Symbol<unsafe extern "system" fn() -> *mut IPluginFactory> =
            lib.get(b"GetPluginFactory\0").map_err(|_| "not a VST3 plugin (no GetPluginFactory)".to_string())?;
        let factory = ComPtr::from_raw(get()).ok_or("plugin returned no factory")?;
        let m = Arc::new(Shared(Module { _lib: lib, factory }));
        modules().lock().unwrap().insert(path.to_path_buf(), m.clone());
        Ok(m)
    }
}

// --- scanning ------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub cid: String, // 32 hex chars
    pub name: String,
    pub vendor: String,
    pub sub: String, // e.g. "Fx|Reverb"
}

impl ClassInfo {
    pub fn is_instrument(&self) -> bool {
        self.sub.split('|').any(|s| s == "Instrument")
    }
}

fn cstr(bytes: &[c_char]) -> String {
    let b: Vec<u8> = bytes.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&b).into_owned()
}

fn tuid_hex(t: &TUID) -> String {
    t.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

fn hex_tuid(s: &str) -> Result<TUID, String> {
    let s: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if s.len() != 32 {
        return Err(format!("bad class id {s}"));
    }
    let mut t: TUID = [0; 16];
    for i in 0..16 {
        t[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())? as i8;
    }
    Ok(t)
}

/// The audio-effect classes in a plugin. Reads Contents/Resources/moduleinfo.json
/// when the plugin has one (no code runs), else loads it and asks the factory.
pub fn scan(path: &Path) -> Result<Vec<ClassInfo>, String> {
    if let Some(v) = scan_moduleinfo(path) {
        return Ok(v);
    }
    let path = path.to_path_buf();
    on_ui(move || unsafe {
        let m = load_module(&path)?;
        let f = &m.0.factory;
        let f2 = f.cast::<IPluginFactory2>();
        let mut out = Vec::new();
        for i in 0..f.countClasses() {
            if let Some(f2) = &f2 {
                let mut info: PClassInfo2 = std::mem::zeroed();
                if f2.getClassInfo2(i, &mut info) == kResultOk && cstr(&info.category) == "Audio Module Class" {
                    out.push(ClassInfo { cid: tuid_hex(&info.cid), name: cstr(&info.name), vendor: cstr(&info.vendor), sub: cstr(&info.subCategories) });
                    continue;
                }
            }
            let mut info: PClassInfo = std::mem::zeroed();
            if f.getClassInfo(i, &mut info) == kResultOk && cstr(&info.category) == "Audio Module Class" {
                out.push(ClassInfo { cid: tuid_hex(&info.cid), name: cstr(&info.name), vendor: String::new(), sub: String::new() });
            }
        }
        Ok(out)
    })
}

fn scan_moduleinfo(path: &Path) -> Option<Vec<ClassInfo>> {
    let text = std::fs::read_to_string(path.join("Contents").join("Resources").join("moduleinfo.json")).ok()?;
    // moduleinfo.json is JSON5-ish: tolerate trailing commas
    let cleaned = text.replace(",\n}", "\n}").replace(",\n]", "\n]").replace(",\r\n}", "\r\n}").replace(",\r\n]", "\r\n]");
    let v: serde_json::Value = serde_json::from_str(&cleaned).ok()?;
    let classes = v.get("Classes")?.as_array()?;
    let out: Vec<ClassInfo> = classes
        .iter()
        .filter(|c| c.get("Category").and_then(|x| x.as_str()) == Some("Audio Module Class"))
        .filter_map(|c| {
            Some(ClassInfo {
                cid: c.get("CID")?.as_str()?.to_string(),
                name: c.get("Name")?.as_str()?.to_string(),
                vendor: c.get("Vendor").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                sub: c.get("Sub Categories").and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join("|"))
                    .unwrap_or_default(),
            })
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// Where VST3 plugins live by convention.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("CommonProgramFiles") {
            dirs.push(PathBuf::from(p).join("VST3"));
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(p).join("Programs").join("Common").join("VST3"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(h).join("Library/Audio/Plug-Ins/VST3"));
        }
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

/// Every .vst3 under the given folders (not descending into bundles).
pub fn find_plugins(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let is_vst = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vst3"));
            if is_vst {
                out.push(p);
            } else if p.is_dir() && depth < 4 {
                walk(&p, out, depth + 1);
            }
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(d, &mut out, 0);
    }
    out.sort();
    out
}

// --- host-side COM objects -----------------------------------------------------------

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let n = &mut *name;
        for (i, c) in "Duet".encode_utf16().chain(std::iter::once(0)).enumerate() {
            n[i] = c as TChar;
        }
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        let want = &*(cid as *const [u8; 16]);
        if *want == IMessage::IID {
            let m = ComWrapper::new(HostMessage { id: Mutex::new(CString::default()), attrs: ComWrapper::new(Attributes::default()) });
            *obj = m.to_com_ptr::<IMessage>().unwrap().into_raw() as *mut c_void;
            return kResultOk;
        }
        if *want == IAttributeList::IID {
            let a = ComWrapper::new(Attributes::default());
            *obj = a.to_com_ptr::<IAttributeList>().unwrap().into_raw() as *mut c_void;
            return kResultOk;
        }
        *obj = ptr::null_mut();
        kNoInterface
    }
}

fn host_app() -> &'static Shared<ComWrapper<HostApp>> {
    static H: OnceLock<Shared<ComWrapper<HostApp>>> = OnceLock::new();
    H.get_or_init(|| Shared(ComWrapper::new(HostApp)))
}

fn host_unknown() -> *mut FUnknown {
    host_app().0.to_com_ptr::<IHostApplication>().unwrap().into_raw() as *mut FUnknown
}

struct HostMessage {
    id: Mutex<CString>,
    attrs: ComWrapper<Attributes>,
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.lock().unwrap().as_ptr()
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        if !id.is_null() {
            *self.id.lock().unwrap() = CStr::from_ptr(id).to_owned();
        }
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // owned by the message; per the API no extra reference for the caller
        self.attrs.as_com_ref::<IAttributeList>().unwrap().as_ptr()
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

#[derive(Default)]
struct Attributes {
    map: Mutex<HashMap<CString, Attr>>,
}

impl Class for Attributes {
    type Interfaces = (IAttributeList,);
}

unsafe fn key(id: IAttributeList_::AttrID) -> CString {
    if id.is_null() { CString::default() } else { CStr::from_ptr(id).to_owned() }
}

impl IAttributeListTrait for Attributes {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        self.map.lock().unwrap().insert(key(id), Attr::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Int(v)) => { *value = *v; kResultOk }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        self.map.lock().unwrap().insert(key(id), Attr::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Float(v)) => { *value = *v; kResultOk }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        let mut v = Vec::new();
        if !string.is_null() {
            let mut i = 0;
            while *string.add(i) != 0 {
                v.push(*string.add(i));
                i += 1;
            }
        }
        v.push(0);
        self.map.lock().unwrap().insert(key(id), Attr::Str(v));
        kResultOk
    }
    unsafe fn getString(&self, id: IAttributeList_::AttrID, string: *mut TChar, sizeInBytes: uint32) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Str(v)) => {
                let n = (sizeInBytes as usize / 2).min(v.len());
                if n == 0 { return kResultFalse; }
                ptr::copy_nonoverlapping(v.as_ptr(), string, n);
                *string.add(n - 1) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setBinary(&self, id: IAttributeList_::AttrID, data: *const c_void, sizeInBytes: uint32) -> tresult {
        let v = if data.is_null() { Vec::new() } else { std::slice::from_raw_parts(data as *const u8, sizeInBytes as usize).to_vec() };
        self.map.lock().unwrap().insert(key(id), Attr::Bin(v));
        kResultOk
    }
    unsafe fn getBinary(&self, id: IAttributeList_::AttrID, data: *mut *const c_void, sizeInBytes: *mut uint32) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            // the bytes live as long as the list, as the API expects
            Some(Attr::Bin(v)) => { *data = v.as_ptr() as *const c_void; *sizeInBytes = v.len() as uint32; kResultOk }
            _ => kResultFalse,
        }
    }
}

/// In-memory IBStream for plugin state.
#[derive(Default)]
struct MemStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, numBytes: int32, numBytesRead: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = (numBytes.max(0) as usize).min(data.len().saturating_sub(*pos));
        ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer as *mut u8, n);
        *pos += n;
        if !numBytesRead.is_null() { *numBytesRead = n as int32; }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, numBytes: int32, numBytesWritten: *mut int32) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = numBytes.max(0) as usize;
        if data.len() < *pos + n { data.resize(*pos + n, 0); }
        ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(*pos), n);
        *pos += n;
        if !numBytesWritten.is_null() { *numBytesWritten = n as int32; }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, p) = &mut *g;
        let base = match mode as u32 {
            x if x == IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0i64,
            x if x == IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => *p as i64,
            _ => data.len() as i64,
        };
        *p = (base + pos).clamp(0, data.len() as i64) as usize;
        if !result.is_null() { *result = *p as int64; }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() { *pos = self.inner.lock().unwrap().1 as int64; }
        kResultOk
    }
}

fn stream(bytes: &[u8]) -> ComWrapper<MemStream> {
    ComWrapper::new(MemStream { inner: Mutex::new((bytes.to_vec(), 0)) })
}

/// Receives parameter edits from the plugin's own editor (plugin thread) and
/// queues them for the audio thread.
struct Handler {
    edits: Mutex<rtrb::Producer<(ParamID, f64)>>,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, valueNormalized: ParamValue) -> tresult {
        let _ = self.edits.lock().unwrap().push((id, valueNormalized));
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// A parameter's value changes for one block. Fixed capacity: no allocation.
struct Queue {
    id: UnsafeCell<ParamID>,
    points: UnsafeCell<[(int32, ParamValue); 16]>,
    len: UnsafeCell<usize>,
}
unsafe impl Sync for Queue {}
unsafe impl Send for Queue {}

impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        *self.id.get()
    }
    unsafe fn getPointCount(&self) -> int32 {
        *self.len.get() as int32
    }
    unsafe fn getPoint(&self, index: int32, sampleOffset: *mut int32, value: *mut ParamValue) -> tresult {
        if index < 0 || index as usize >= *self.len.get() { return kInvalidArgument; }
        let (o, v) = (*self.points.get())[index as usize];
        *sampleOffset = o;
        *value = v;
        kResultOk
    }
    unsafe fn addPoint(&self, sampleOffset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let len = &mut *self.len.get();
        if *len >= 16 { return kResultFalse; }
        (*self.points.get())[*len] = (sampleOffset, value);
        if !index.is_null() { *index = *len as int32; }
        *len += 1;
        kResultOk
    }
}

/// The set of parameter queues handed to process(). Fixed capacity.
struct Changes {
    queues: Vec<ComWrapper<Queue>>,
    used: UnsafeCell<usize>,
}
unsafe impl Sync for Changes {}
unsafe impl Send for Changes {}

impl Changes {
    fn new() -> Self {
        Changes {
            queues: (0..MAX_PARAM_EDITS)
                .map(|_| ComWrapper::new(Queue { id: UnsafeCell::new(0), points: UnsafeCell::new([(0, 0.0); 16]), len: UnsafeCell::new(0) }))
                .collect(),
            used: UnsafeCell::new(0),
        }
    }
    unsafe fn clear(&self) {
        *self.used.get() = 0;
    }
    /// Latest value for a parameter at sample 0 (used on the audio thread).
    unsafe fn set(&self, id: ParamID, v: f64) {
        let used = &mut *self.used.get();
        for q in &self.queues[..*used] {
            if *q.id.get() == id {
                (*q.points.get())[0] = (0, v);
                *q.len.get() = 1;
                return;
            }
        }
        if *used < self.queues.len() {
            let q = &self.queues[*used];
            *q.id.get() = id;
            (*q.points.get())[0] = (0, v);
            *q.len.get() = 1;
            *used += 1;
        }
    }
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        *self.used.get() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index < 0 || index as usize >= *self.used.get() { return ptr::null_mut(); }
        self.queues[index as usize].as_com_ref::<IParamValueQueue>().unwrap().as_ptr()
    }
    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        // for the plugin's output changes: take them so it has somewhere to write
        let used = &mut *self.used.get();
        for (i, q) in self.queues[..*used].iter().enumerate() {
            if *q.id.get() == *id {
                if !index.is_null() { *index = i as int32; }
                return q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr();
            }
        }
        if *used >= self.queues.len() { return ptr::null_mut(); }
        let q = &self.queues[*used];
        *q.id.get() = *id;
        *q.len.get() = 0;
        if !index.is_null() { *index = *used as int32; }
        *used += 1;
        q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()
    }
}

/// Notes for one process() call: preallocated, filled on the audio thread
/// (instruments), empty for effects.
const MAX_EVENTS: usize = 256;
struct Events {
    list: UnsafeCell<Vec<Event>>,
    len: UnsafeCell<usize>,
}
unsafe impl Sync for Events {}
unsafe impl Send for Events {}
impl Events {
    fn new() -> Self {
        // plain C structs: all-zero is a valid empty event
        Events { list: UnsafeCell::new(vec![unsafe { std::mem::zeroed() }; MAX_EVENTS]), len: UnsafeCell::new(0) }
    }
    unsafe fn push(&self, e: Event) {
        let len = &mut *self.len.get();
        if *len < MAX_EVENTS {
            (&mut *self.list.get())[*len] = e;
            *len += 1;
        }
    }
    unsafe fn clear(&self) {
        *self.len.get() = 0;
    }
}
impl Class for Events {
    type Interfaces = (IEventList,);
}
impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> int32 { *self.len.get() as int32 }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        if index < 0 || index as usize >= *self.len.get() || e.is_null() { return kInvalidArgument; }
        *e = (&*self.list.get())[index as usize];
        kResultOk
    }
    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() { return kInvalidArgument; }
        self.push(*e);
        kResultOk
    }
}

// --- an instance ---------------------------------------------------------------

/// The parts of a plugin instance that belong to the plugin thread.
pub struct Instance {
    pub name: String,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    _handler: ComWrapper<Handler>,
    pub latency: u32,
    editor: Mutex<Option<editor::Editor>>,
    pub has_editor: bool,
}

unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        // tear down where it was built
        let parts = Shared((self.component.clone(), self.processor.clone(), self.controller.clone(), self.editor.lock().unwrap().take()));
        on_ui(move || unsafe {
            // bind the whole wrapper: a 2021-edition closure would otherwise
            // capture just the (non-Send) fields inside it
            let parts = parts;
            let (component, processor, controller, editor) = parts.0;
            drop(editor);
            processor.setProcessing(0);
            component.setActive(0);
            if let (Some(c), Some(a), Some(b)) = (&controller, component.cast::<IConnectionPoint>(), controller.as_ref().and_then(|c| c.cast::<IConnectionPoint>())) {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
                let _ = c;
            }
            if let Some(c) = &controller {
                c.setComponentHandler(ptr::null_mut());
                c.terminate();
            }
            component.terminate();
        });
    }
}

/// Registry so the control side can reach an instance living inside the audio
/// thread's rack (to open its editor or read its state).
fn registry() -> &'static Mutex<HashMap<u64, Weak<Instance>>> {
    static R: OnceLock<Mutex<HashMap<u64, Weak<Instance>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn instance(id: u64) -> Option<Arc<Instance>> {
    registry().lock().unwrap().get(&id).and_then(|w| w.upgrade())
}

unsafe fn speaker_channels(arr: SpeakerArrangement) -> usize {
    (arr as u64).count_ones() as usize
}

/// Audio side of a plugin: what process() needs, all preallocated.
pub struct VstProc {
    inst: Arc<Instance>,
    processor: Shared<ComPtr<IAudioProcessor>>,
    edits: rtrb::Consumer<(ParamID, f64)>,
    changes: ComWrapper<Changes>,
    out_changes: ComWrapper<Changes>,
    events: ComWrapper<Events>,
    in_bufs: Vec<Vec<Vec<f32>>>, // [bus][channel][sample]
    out_bufs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_bus: Vec<AudioBusBuffers>,
    out_bus: Vec<AudioBusBuffers>,
    dry: [Vec<f32>; 2],
    context: ProcessContext,
    pub id: u64,
    sr: f64,
}

unsafe impl Send for VstProc {}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub component: Vec<u8>,
    pub controller: Vec<u8>,
}

impl State {
    /// "c:<base64>;e:<base64>" (component / edit controller)
    pub fn encode(&self) -> String {
        let b = base64::engine::general_purpose::STANDARD;
        format!("c:{};e:{}", b.encode(&self.component), b.encode(&self.controller))
    }
    pub fn decode(s: &str) -> Option<State> {
        let b = base64::engine::general_purpose::STANDARD;
        let mut st = State::default();
        for part in s.split(';') {
            if let Some(x) = part.strip_prefix("c:") { st.component = b.decode(x).ok()?; }
            if let Some(x) = part.strip_prefix("e:") { st.controller = b.decode(x).ok()?; }
        }
        Some(st)
    }
}

impl VstProc {
    /// Create and start an instance. Blocks while the plugin thread builds it.
    pub fn create(path: &str, cid: &str, state: Option<State>, sr: f64) -> Result<VstProc, String> {
        Self::create_as(path, cid, state, sr, false)
    }

    /// The same for an instrument: its note input is switched on.
    pub fn create_as(path: &str, cid: &str, state: Option<State>, sr: f64, instrument: bool) -> Result<VstProc, String> {
        let (edit_tx, edit_rx) = rtrb::RingBuffer::new(1024);
        let path = PathBuf::from(path);
        let cid = hex_tuid(cid)?;
        let built = on_ui(move || unsafe { build(&path, cid, state, sr, edit_tx, instrument) })?;
        let (inst, in_ch, out_ch) = built;
        let id = NEXT_ID.fetch_add(1, Relaxed);
        registry().lock().unwrap().insert(id, Arc::downgrade(&inst));
        let processor = Shared(inst.processor.clone());
        let mk = |chs: &Vec<usize>| -> Vec<Vec<Vec<f32>>> { chs.iter().map(|&c| (0..c).map(|_| vec![0.0; MAX_BLOCK]).collect()).collect() };
        let mut p = VstProc {
            inst,
            processor,
            edits: edit_rx,
            changes: ComWrapper::new(Changes::new()),
            out_changes: ComWrapper::new(Changes::new()),
            events: ComWrapper::new(Events::new()),
            in_bufs: mk(&in_ch),
            out_bufs: mk(&out_ch),
            in_ptrs: vec![],
            out_ptrs: vec![],
            in_bus: vec![],
            out_bus: vec![],
            dry: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
            // plain C struct; all-zero is a valid "nothing known yet"
            context: unsafe { std::mem::zeroed() },
            id,
            sr,
        };
        p.in_ptrs = p.in_bufs.iter_mut().map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        p.out_ptrs = p.out_bufs.iter_mut().map(|b| b.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        let bus = |ptrs: &mut Vec<*mut f32>| AudioBusBuffers {
            numChannels: ptrs.len() as int32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 { channelBuffers32: if ptrs.is_empty() { ptr::null_mut() } else { ptrs.as_mut_ptr() } },
        };
        p.in_bus = p.in_ptrs.iter_mut().map(bus).collect();
        p.out_bus = p.out_ptrs.iter_mut().map(bus).collect();
        p.context.sampleRate = sr;
        Ok(p)
    }

    pub fn instance(&self) -> &Arc<Instance> {
        &self.inst
    }

    /// Re-setup for a new sample rate (stream stopped; blocks on the plugin thread).
    pub fn set_sample_rate(&mut self, sr: f64) {
        self.sr = sr;
        self.context.sampleRate = sr;
        let p = Shared(self.processor.0.clone());
        let c = Shared(self.inst.component.clone());
        on_ui(move || unsafe {
            p.0.setProcessing(0);
            c.0.setActive(0);
            let mut setup = ProcessSetup { processMode: ProcessModes_::kRealtime as int32, symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32, maxSamplesPerBlock: MAX_BLOCK as int32, sampleRate: sr };
            p.0.setupProcessing(&mut setup);
            c.0.setActive(1);
            p.0.setProcessing(1);
        });
    }

    /// A note for the next process() call (instruments). `offset` is the sample
    /// in the block where it happens.
    pub fn note(&mut self, on: bool, key: u8, vel: u8, offset: usize) {
        let mut e: Event = unsafe { std::mem::zeroed() };
        e.sampleOffset = offset.min(MAX_BLOCK - 1) as int32;
        if on {
            e.r#type = Event_::EventTypes_::kNoteOnEvent as uint16;
            e.__field0.noteOn = NoteOnEvent { channel: 0, pitch: key as int16, tuning: 0.0, velocity: vel as f32 / 127.0, length: 0, noteId: -1 };
        } else {
            e.r#type = Event_::EventTypes_::kNoteOffEvent as uint16;
            e.__field0.noteOff = NoteOffEvent { channel: 0, pitch: key as int16, velocity: vel as f32 / 127.0, noteId: -1, tuning: 0.0 };
        }
        unsafe { self.events.push(e) };
    }

    /// One stereo block, in place. `mix` blends dry/wet. `tempo` from the
    /// metronome, for tempo-synced plugins. A mono plugin gets the sum and
    /// plays on both sides.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], mix: f32, tempo: f64) {
        let n = l.len().min(r.len()).min(MAX_BLOCK);
        unsafe {
            self.changes.clear();
            self.out_changes.clear();
            while let Ok((id, v)) = self.edits.pop() {
                self.changes.set(id, v);
            }
            for (b, chans) in self.in_bufs.iter_mut().enumerate() {
                let count = chans.len();
                for (c, ch) in chans.iter_mut().enumerate() {
                    match (b, count, c) {
                        (0, 1, _) => {
                            for k in 0..n {
                                ch[k] = 0.5 * (l[k] + r[k]);
                            }
                        }
                        (0, _, 0) => ch[..n].copy_from_slice(&l[..n]),
                        (0, _, 1) => ch[..n].copy_from_slice(&r[..n]),
                        _ => ch[..n].fill(0.0),
                    }
                }
            }
            for bus in self.in_bus.iter_mut().chain(self.out_bus.iter_mut()) {
                bus.silenceFlags = 0;
            }
            self.dry[0][..n].copy_from_slice(&l[..n]);
            self.dry[1][..n].copy_from_slice(&r[..n]);
            self.context.tempo = tempo;
            self.context.state = (ProcessContext_::StatesAndFlags_::kTempoValid) as uint32;
            let mut data = ProcessData {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                numSamples: n as int32,
                numInputs: self.in_bus.len() as int32,
                numOutputs: self.out_bus.len() as int32,
                inputs: if self.in_bus.is_empty() { ptr::null_mut() } else { self.in_bus.as_mut_ptr() },
                outputs: if self.out_bus.is_empty() { ptr::null_mut() } else { self.out_bus.as_mut_ptr() },
                inputParameterChanges: self.changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
                outputParameterChanges: self.out_changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
                inputEvents: self.events.as_com_ref::<IEventList>().unwrap().as_ptr(),
                outputEvents: ptr::null_mut(),
                processContext: &mut self.context,
            };
            self.processor.0.process(&mut data);
            self.events.clear();
            self.context.projectTimeSamples += n as i64;
            let Some(main) = self.out_bufs.first() else { return };
            match main.len() {
                0 => {}
                1 => {
                    l[..n].copy_from_slice(&main[0][..n]);
                    r[..n].copy_from_slice(&main[0][..n]);
                }
                _ => {
                    l[..n].copy_from_slice(&main[0][..n]);
                    r[..n].copy_from_slice(&main[1][..n]);
                }
            }
            if mix < 0.999 {
                for k in 0..n {
                    l[k] = self.dry[0][k] * (1.0 - mix) + l[k] * mix;
                    r[k] = self.dry[1][k] * (1.0 - mix) + r[k] * mix;
                }
            }
        }
    }
}

/// Build + start an instance. Runs on the plugin thread.
unsafe fn build(
    path: &Path,
    cid: TUID,
    state: Option<State>,
    sr: f64,
    edits: rtrb::Producer<(ParamID, f64)>,
    instrument: bool,
) -> Result<(Arc<Instance>, Vec<usize>, Vec<usize>), String> {
    let m = load_module(path)?;
    let f = &m.0.factory;
    let mut obj: *mut c_void = ptr::null_mut();
    if f.createInstance(cid.as_ptr(), IComponent::IID.as_ptr() as FIDString, &mut obj) != kResultOk || obj.is_null() {
        return Err("the plugin wouldn't create an instance".into());
    }
    let component = ComPtr::<IComponent>::from_raw(obj as *mut IComponent).ok_or("no component")?;
    if component.initialize(host_unknown()) != kResultOk {
        return Err("the plugin failed to initialise".into());
    }
    let mut name = String::new();
    {
        let mut info: PClassInfo = std::mem::zeroed();
        for i in 0..f.countClasses() {
            if f.getClassInfo(i, &mut info) == kResultOk && info.cid == cid {
                name = cstr(&info.name);
            }
        }
    }

    // the edit controller: the same object, or a separate class
    let controller: Option<ComPtr<IEditController>> = match component.cast::<IEditController>() {
        Some(c) => Some(c),
        None => {
            let mut ccid: TUID = [0; 16];
            if component.getControllerClassId(&mut ccid) == kResultOk {
                let mut obj: *mut c_void = ptr::null_mut();
                if f.createInstance(ccid.as_ptr(), IEditController::IID.as_ptr() as FIDString, &mut obj) == kResultOk && !obj.is_null() {
                    let c = ComPtr::<IEditController>::from_raw(obj as *mut IEditController);
                    if let Some(c) = &c {
                        c.initialize(host_unknown());
                    }
                    c
                } else {
                    None
                }
            } else {
                None
            }
        }
    };
    let separate = controller.is_some() && component.cast::<IEditController>().is_none();
    if separate {
        if let (Some(a), Some(b)) = (component.cast::<IConnectionPoint>(), controller.as_ref().and_then(|c| c.cast::<IConnectionPoint>())) {
            a.connect(b.as_ptr());
            b.connect(a.as_ptr());
        }
    }
    let handler = ComWrapper::new(Handler { edits: Mutex::new(edits) });
    if let Some(c) = &controller {
        c.setComponentHandler(handler.as_com_ref::<IComponentHandler>().unwrap().as_ptr());
    }

    // state: restore a saved one, or at least let the controller see the defaults
    match &state {
        Some(st) if !st.component.is_empty() => {
            component.setState(stream(&st.component).as_com_ref::<IBStream>().unwrap().as_ptr());
            if let Some(c) = &controller {
                c.setComponentState(stream(&st.component).as_com_ref::<IBStream>().unwrap().as_ptr());
                if !st.controller.is_empty() {
                    c.setState(stream(&st.controller).as_com_ref::<IBStream>().unwrap().as_ptr());
                }
            }
        }
        _ => {
            if let Some(c) = &controller {
                let s = stream(&[]);
                if component.getState(s.as_com_ref::<IBStream>().unwrap().as_ptr()) == kResultOk {
                    s.inner.lock().unwrap().1 = 0;
                    c.setComponentState(s.as_com_ref::<IBStream>().unwrap().as_ptr());
                }
            }
        }
    }

    let processor = component.cast::<IAudioProcessor>().ok_or("not an audio processor")?;
    let audio = MediaTypes_::kAudio as MediaType;
    let (dir_in, dir_out) = (BusDirections_::kInput as BusDirection, BusDirections_::kOutput as BusDirection);
    let n_in = component.getBusCount(audio, dir_in).max(0) as usize;
    let n_out = component.getBusCount(audio, dir_out).max(0) as usize;
    if n_out == 0 {
        return Err("the plugin has no audio output".into());
    }
    // current arrangements, then ask for stereo main buses (our strips are
    // stereo), falling back to mono
    let arr = |dir, i| {
        let mut a: SpeakerArrangement = 0;
        processor.getBusArrangement(dir, i as int32, &mut a);
        a
    };
    let mut ins: Vec<SpeakerArrangement> = (0..n_in).map(|i| arr(dir_in, i)).collect();
    let mut outs: Vec<SpeakerArrangement> = (0..n_out).map(|i| arr(dir_out, i)).collect();
    let try_set = |ins: &mut Vec<SpeakerArrangement>, outs: &mut Vec<SpeakerArrangement>, a: SpeakerArrangement| {
        if !ins.is_empty() { ins[0] = a; }
        outs[0] = a;
        processor.setBusArrangements(
            if ins.is_empty() { ptr::null_mut() } else { ins.as_mut_ptr() }, ins.len() as int32,
            outs.as_mut_ptr(), outs.len() as int32,
        ) == kResultTrue
    };
    if !try_set(&mut ins, &mut outs, SpeakerArr::kStereo) {
        try_set(&mut ins, &mut outs, SpeakerArr::kMono);
    }
    let in_ch: Vec<usize> = (0..n_in).map(|i| speaker_channels(arr(dir_in, i))).collect();
    let out_ch: Vec<usize> = (0..n_out).map(|i| speaker_channels(arr(dir_out, i))).collect();
    for i in 0..n_in {
        component.activateBus(audio, dir_in, i as int32, (i == 0) as TBool);
    }
    for i in 0..n_out {
        component.activateBus(audio, dir_out, i as int32, (i == 0) as TBool);
    }
    let events = MediaTypes_::kEvent as MediaType;
    for d in [dir_in, dir_out] {
        for i in 0..component.getBusCount(events, d).max(0) {
            // an instrument's first note input on; nothing else uses events
            component.activateBus(events, d, i, (instrument && d == dir_in && i == 0) as TBool);
        }
    }
    let mut setup = ProcessSetup {
        processMode: ProcessModes_::kRealtime as int32,
        symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
        maxSamplesPerBlock: MAX_BLOCK as int32,
        sampleRate: sr,
    };
    if processor.setupProcessing(&mut setup) != kResultOk {
        return Err("the plugin refused the audio setup".into());
    }
    component.setActive(1);
    processor.setProcessing(1);
    let latency = processor.getLatencySamples();
    let has_editor = controller.as_ref().is_some_and(|c| {
        let v = c.createView(b"editor\0".as_ptr() as FIDString);
        match ComPtr::<IPlugView>::from_raw(v) {
            Some(v) => v.isPlatformTypeSupported(editor::PLATFORM) == kResultTrue,
            None => false,
        }
    });
    let inst = Arc::new(Instance { name, component, processor, controller, _handler: handler, latency, editor: Mutex::new(None), has_editor });
    Ok((inst, in_ch, out_ch))
}

impl Instance {
    /// Current state (component + controller), for saving. Plugin thread.
    pub fn state(self: &Arc<Self>) -> Option<State> {
        let me = Shared(self.clone());
        on_ui(move || unsafe {
            let me = me.0;
            let s = stream(&[]);
            if me.component.getState(s.as_com_ref::<IBStream>().unwrap().as_ptr()) != kResultOk {
                return None;
            }
            let component = s.inner.lock().unwrap().0.clone();
            let mut controller = Vec::new();
            if let Some(c) = &me.controller {
                let s2 = stream(&[]);
                if c.getState(s2.as_com_ref::<IBStream>().unwrap().as_ptr()) == kResultOk {
                    controller = s2.inner.lock().unwrap().0.clone();
                }
            }
            Some(State { component, controller })
        })
    }

    /// Show the plugin's own window (create it the first time).
    pub fn show_editor(self: &Arc<Self>, title: String) -> Result<(), String> {
        let me = Shared(self.clone());
        on_ui(move || unsafe {
            let me = me.0;
            let mut ed = me.editor.lock().unwrap();
            if let Some(e) = ed.as_ref() {
                e.show();
                return Ok(());
            }
            let c = me.controller.as_ref().ok_or("this plugin has no editor")?;
            let view = ComPtr::<IPlugView>::from_raw(c.createView(b"editor\0".as_ptr() as FIDString)).ok_or("this plugin has no editor")?;
            *ed = Some(editor::Editor::open(view, &title)?);
            Ok(())
        })
    }
}

// --- editor windows ------------------------------------------------------------------

/// Resizes our window when the plugin asks to.
struct Frame {
    window: UnsafeCell<usize>, // HWND / NSWindow*
}
unsafe impl Sync for Frame {}
unsafe impl Send for Frame {}
impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}
impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, newSize: *mut ViewRect) -> tresult {
        let r = &*newSize;
        editor::resize_window(*self.window.get(), r.right - r.left, r.bottom - r.top);
        if let Some(v) = vst3::ComRef::<IPlugView>::from_raw(view) {
            v.onSize(newSize);
        }
        kResultOk
    }
}

#[cfg(windows)]
mod editor {
    use super::*;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub const PLATFORM: FIDString = b"HWND\0".as_ptr() as FIDString;

    pub struct Editor {
        view: ComPtr<IPlugView>,
        _frame: ComWrapper<Frame>,
        hwnd: HWND,
    }

    fn class_name() -> Vec<u16> {
        "DuetPluginWindow\0".encode_utf16().collect()
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            // closing just hides it: the plugin's view stays attached until the
            // slot goes away, and reopening is instant
            WM_CLOSE => {
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            WM_SIZE => {
                let view = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut IPlugView;
                if let Some(v) = vst3::ComRef::<IPlugView>::from_raw(view) {
                    let mut r = ViewRect { left: 0, top: 0, right: (lp & 0xFFFF) as i32, bottom: ((lp >> 16) & 0xFFFF) as i32 };
                    if v.canResize() == kResultTrue {
                        v.onSize(&mut r);
                    }
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub unsafe fn resize_window(hwnd: usize, w: i32, h: i32) {
        let hwnd = hwnd as HWND;
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let mut rc = RECT { left: 0, top: 0, right: w, bottom: h };
        AdjustWindowRectEx(&mut rc, style, 0, 0);
        SetWindowPos(hwnd, ptr::null_mut(), 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }

    impl Editor {
        pub unsafe fn open(view: ComPtr<IPlugView>, title: &str) -> Result<Editor, String> {
            if view.isPlatformTypeSupported(PLATFORM) != kResultTrue {
                return Err("the plugin's editor doesn't support Windows".into());
            }
            static REGISTERED: std::sync::Once = std::sync::Once::new();
            let cls = class_name();
            REGISTERED.call_once(|| {
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(ptr::null()),
                    hIcon: ptr::null_mut(),
                    hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                    hbrBackground: ptr::null_mut(),
                    lpszMenuName: ptr::null(),
                    lpszClassName: cls.as_ptr(),
                };
                RegisterClassW(&wc);
            });
            let mut size = ViewRect { left: 0, top: 0, right: 600, bottom: 400 };
            view.getSize(&mut size);
            let resizable = view.canResize() == kResultTrue;
            let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | if resizable { WS_THICKFRAME | WS_MAXIMIZEBOX } else { 0 };
            let mut rc = RECT { left: 0, top: 0, right: size.right - size.left, bottom: size.bottom - size.top };
            AdjustWindowRectEx(&mut rc, style, 0, 0);
            let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let hwnd = CreateWindowExW(
                0, cls.as_ptr(), title.as_ptr(), style, CW_USEDEFAULT, CW_USEDEFAULT,
                rc.right - rc.left, rc.bottom - rc.top, ptr::null_mut(), ptr::null_mut(),
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(ptr::null()), ptr::null(),
            );
            if hwnd.is_null() {
                return Err("couldn't create a window for the plugin".into());
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, view.as_ptr() as isize);
            let frame = ComWrapper::new(Frame { window: UnsafeCell::new(hwnd as usize) });
            view.setFrame(frame.as_com_ref::<IPlugFrame>().unwrap().as_ptr());
            if view.attached(hwnd as *mut c_void, PLATFORM) != kResultOk {
                DestroyWindow(hwnd);
                return Err("the plugin couldn't open its editor".into());
            }
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            Ok(Editor { view, _frame: frame, hwnd })
        }

        pub unsafe fn show(&self) {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }

        pub fn is_visible(&self) -> bool {
            unsafe { IsWindowVisible(self.hwnd) != 0 }
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            unsafe {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                self.view.removed();
                self.view.setFrame(ptr::null_mut());
                DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod editor {
    use super::*;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    pub const PLATFORM: FIDString = b"NSView\0".as_ptr() as FIDString;

    pub struct Editor {
        view: ComPtr<IPlugView>,
        _frame: ComWrapper<Frame>,
        window: Retained<NSWindow>,
    }

    pub unsafe fn resize_window(window: usize, w: i32, h: i32) {
        let window = &*(window as *const NSWindow);
        window.setContentSize(NSSize::new(w as f64, h as f64));
    }

    impl Editor {
        pub unsafe fn open(view: ComPtr<IPlugView>, title: &str) -> Result<Editor, String> {
            let mtm = MainThreadMarker::new().ok_or("plugin windows need the main thread")?;
            if view.isPlatformTypeSupported(PLATFORM) != kResultTrue {
                return Err("the plugin's editor doesn't support macOS".into());
            }
            let mut size = ViewRect { left: 0, top: 0, right: 600, bottom: 400 };
            view.getSize(&mut size);
            let resizable = view.canResize() == kResultTrue;
            let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
            if resizable {
                style |= NSWindowStyleMask::Resizable;
            }
            let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new((size.right - size.left) as f64, (size.bottom - size.top) as f64));
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(mtm.alloc(), rect, style, NSBackingStoreType::Buffered, false);
            // closing hides it; the plugin view stays attached until the slot goes
            window.setReleasedWhenClosed(false);
            window.setTitle(&NSString::from_str(title));
            let content = window.contentView().ok_or("no content view")?;
            let frame = ComWrapper::new(Frame { window: UnsafeCell::new(Retained::as_ptr(&window) as usize) });
            view.setFrame(frame.as_com_ref::<IPlugFrame>().unwrap().as_ptr());
            if view.attached(Retained::as_ptr(&content) as *mut c_void, PLATFORM) != kResultOk {
                return Err("the plugin couldn't open its editor".into());
            }
            window.center();
            window.makeKeyAndOrderFront(None);
            Ok(Editor { view, _frame: frame, window })
        }

        pub unsafe fn show(&self) {
            self.window.makeKeyAndOrderFront(None);
        }

        pub fn is_visible(&self) -> bool {
            self.window.isVisible()
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            unsafe {
                self.view.removed();
                self.view.setFrame(ptr::null_mut());
                self.window.close();
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod editor {
    use super::*;
    pub const PLATFORM: FIDString = b"X11EmbedWindowID\0".as_ptr() as FIDString;
    pub struct Editor;
    pub unsafe fn resize_window(_w: usize, _x: i32, _y: i32) {}
    impl Editor {
        pub unsafe fn open(_v: ComPtr<IPlugView>, _t: &str) -> Result<Editor, String> {
            Err("plugin windows aren't supported on this system".into())
        }
        pub unsafe fn show(&self) {}
        pub fn is_visible(&self) -> bool { false }
    }
}

// --- platform glue -----------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::Job;
    use std::sync::mpsc;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_JOBS: u32 = WM_APP + 1;

    pub fn current_thread_id() -> u32 {
        unsafe { GetCurrentThreadId() }
    }

    pub fn thread_init() {
        #[link(name = "ole32")]
        extern "system" {
            fn OleInitialize(reserved: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            OleInitialize(std::ptr::null_mut()); // STA + drag&drop/clipboard for plugin UIs
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE); // create the queue
        }
    }

    pub fn wake(thread_id: u32) {
        unsafe {
            PostThreadMessageW(thread_id, WM_JOBS, 0, 0);
        }
    }

    pub fn message_loop(jobs: mpsc::Receiver<Job>) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.hwnd.is_null() && msg.message == WM_JOBS {
                    while let Ok(job) = jobs.try_recv() {
                        job();
                    }
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::bundle::CFBundle;
    use core_foundation::url::CFURL;

    pub fn on_main<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        if objc2::MainThreadMarker::new().is_some() {
            return f();
        }
        let (tx, rx) = mpsc::channel();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let _ = tx.send(f());
        });
        rx.recv().expect("main thread dropped a plugin job")
    }

    pub unsafe fn bundle_entry(lib: &libloading::Library, path: &Path) -> Result<(), String> {
        let url = CFURL::from_path(path, true).ok_or("bad bundle path")?;
        let bundle = CFBundle::new(url).ok_or("not a bundle")?;
        if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"bundleEntry\0") {
            if !entry(bundle.as_concrete_TypeRef() as *mut c_void) {
                return Err("the plugin's bundleEntry failed".into());
            }
        }
        std::mem::forget(bundle); // stays alive with the module
        Ok(())
    }
}
