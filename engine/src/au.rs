//! Audio Units (macOS): Apple's plugin format, hosted through AudioToolbox's C API. To the rest
//! of the engine an AU looks like a VST3 plugin (see vst::VstProc): a rack effect or an
//! instrument, with a state to save and (for AUs that have one) a window of its own.
//!
//! An AU is named by its component description: "au:" + type, subtype and manufacturer as
//! eight hex digits each (four-character codes can hold spaces), e.g. Apple's delay is
//! "au:61756678:64656c79:6170706c".

#![allow(non_upper_case_globals, clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use base64::Engine as _;
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

use crate::vst::{on_ui, Shared, MAX_BLOCK, NEXT_ID};

type OSStatus = i32;
type AudioComponent = *mut c_void;
type AudioUnit = *mut c_void;
type CFDataRef = *const c_void;
type CFPropertyListRef = *const c_void;
type CFAllocatorRef = *const c_void;

pub const TYPE_EFFECT: u32 = u32::from_be_bytes(*b"aufx");
pub const TYPE_MUSIC_EFFECT: u32 = u32::from_be_bytes(*b"aumf");
pub const TYPE_INSTRUMENT: u32 = u32::from_be_bytes(*b"aumu");
const FORMAT_LPCM: u32 = u32::from_be_bytes(*b"lpcm");
// float | packed | non-interleaved
const FORMAT_FLAGS: u32 = 1 | 8 | (1 << 5);

const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;
const SCOPE_OUTPUT: u32 = 2;

const P_CLASS_INFO: u32 = 0;
const P_SAMPLE_RATE: u32 = 2;
const P_STREAM_FORMAT: u32 = 8;
const P_LATENCY: u32 = 12;
const P_MAX_FRAMES: u32 = 14;
const P_RENDER_CALLBACK: u32 = 23;
const P_HOST_CALLBACKS: u32 = 27;
const P_COCOA_UI: u32 = 31;
const P_PARAMETER_LIST: u32 = 3;
const P_PARAMETER_INFO: u32 = 4;

const TS_SAMPLE_TIME_VALID: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Desc {
    kind: u32,
    sub: u32,
    maker: u32,
    flags: u32,
    mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Format {
    rate: f64,
    format: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Smpte {
    subframes: i16,
    divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: Smpte,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
struct Buffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

/// An AudioBufferList with room for two buffers (stereo, non-interleaved).
#[repr(C)]
struct BufferList {
    count: u32,
    bufs: [Buffer; 2],
}

type RenderFn = unsafe extern "C" fn(*mut c_void, *mut u32, *const TimeStamp, u32, u32, *mut BufferList) -> OSStatus;

#[repr(C)]
struct RenderCallback {
    proc_: RenderFn,
    refcon: *mut c_void,
}

type BeatTempoFn = unsafe extern "C" fn(*mut c_void, *mut f64, *mut f64) -> OSStatus;

#[repr(C)]
struct HostCallbacks {
    user: *mut c_void,
    beat_and_tempo: Option<BeatTempoFn>,
    musical_time: *const c_void,
    transport: *const c_void,
    transport2: *const c_void,
}

#[repr(C)]
struct CocoaViewInfo {
    bundle: *const c_void, // CFURLRef
    class: [CFStringRef; 1],
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(c: AudioComponent, desc: *const Desc) -> AudioComponent;
    fn AudioComponentGetDescription(c: AudioComponent, desc: *mut Desc) -> OSStatus;
    fn AudioComponentCopyName(c: AudioComponent, name: *mut CFStringRef) -> OSStatus;
    fn AudioComponentInstanceNew(c: AudioComponent, unit: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitSetProperty(unit: AudioUnit, id: u32, scope: u32, elem: u32, data: *const c_void, size: u32) -> OSStatus;
    fn AudioUnitGetProperty(unit: AudioUnit, id: u32, scope: u32, elem: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    fn AudioUnitGetPropertyInfo(unit: AudioUnit, id: u32, scope: u32, elem: u32, size: *mut u32, writable: *mut u8) -> OSStatus;
    fn AudioUnitRender(unit: AudioUnit, flags: *mut u32, ts: *const TimeStamp, bus: u32, frames: u32, data: *mut BufferList) -> OSStatus;
    fn MusicDeviceMIDIEvent(unit: AudioUnit, status: u32, d1: u32, d2: u32, offset: u32) -> OSStatus;
    fn AudioUnitGetParameter(unit: AudioUnit, id: u32, scope: u32, elem: u32, value: *mut f32) -> OSStatus;
    fn AudioUnitSetParameter(unit: AudioUnit, id: u32, scope: u32, elem: u32, value: f32, offset: u32) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFPropertyListCreateData(alloc: CFAllocatorRef, plist: CFPropertyListRef, format: isize, options: usize, err: *mut c_void) -> CFDataRef;
    fn CFPropertyListCreateWithData(alloc: CFAllocatorRef, data: CFDataRef, options: usize, format: *mut isize, err: *mut c_void) -> CFPropertyListRef;
    fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, len: isize) -> CFDataRef;
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
    fn CFDataGetLength(data: CFDataRef) -> isize;
}

const BINARY_PLIST: isize = 200;

/// "au:61756678:64656c79:6170706c" <-> its description.
pub fn path_of(kind: u32, sub: u32, maker: u32) -> String {
    format!("au:{kind:08x}:{sub:08x}:{maker:08x}")
}
fn desc_of(path: &str) -> Option<Desc> {
    let mut it = path.strip_prefix("au:").unwrap_or(path).split(':').map(|x| u32::from_str_radix(x, 16));
    let (kind, sub, maker) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    Some(Desc { kind, sub, maker, ..Default::default() })
}

/// AudioUnitParameterInfo
#[repr(C)]
struct ParamInfoC {
    name: [c_char; 52],
    unit_name: CFStringRef,
    clump: u32,
    cf_name: CFStringRef,
    unit: u32,
    min: f32,
    max: f32,
    default: f32,
    flags: u32,
}

const PF_CF_NAME_RELEASE: u32 = 1 << 4;
const PF_LOG: u32 = 1 << 22;
const PF_HAS_CF_NAME: u32 = 1 << 27;
const PF_READABLE: u32 = 1 << 30;
const PF_WRITABLE: u32 = 1 << 31;
const UNIT_INDEXED: u32 = 1;
const UNIT_BOOLEAN: u32 = 2;

/// One AU parameter: where its 0..1 maps to.
#[derive(Clone, Copy, Debug)]
struct Range {
    id: u32,
    min: f32,
    max: f32,
    log: bool,
}

impl Range {
    fn to_real(&self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        if self.log && self.min > 0.0 && self.max > self.min {
            self.min * (self.max / self.min).powf(v)
        } else {
            self.min + (self.max - self.min) * v
        }
    }
    fn to_norm(&self, x: f32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        if self.log && self.min > 0.0 {
            ((x.max(self.min) / self.min).ln() / (self.max / self.min).ln()).clamp(0.0, 1.0)
        } else {
            ((x - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }
}

/// The unit's writable global parameters: their ranges, names and step counts.
unsafe fn read_params(unit: AudioUnit) -> Vec<(Range, String, f32, i32)> {
    let mut size = 0u32;
    if AudioUnitGetPropertyInfo(unit, P_PARAMETER_LIST, SCOPE_GLOBAL, 0, &mut size, ptr::null_mut()) != 0 || size == 0 {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / 4];
    if AudioUnitGetProperty(unit, P_PARAMETER_LIST, SCOPE_GLOBAL, 0, ids.as_mut_ptr() as *mut c_void, &mut size) != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / 4);
    let mut out = Vec::new();
    for id in ids {
        let mut info: ParamInfoC = std::mem::zeroed();
        let mut sz = std::mem::size_of::<ParamInfoC>() as u32;
        if AudioUnitGetProperty(unit, P_PARAMETER_INFO, SCOPE_GLOBAL, id, &mut info as *mut _ as *mut c_void, &mut sz) != 0 {
            continue;
        }
        let name = if info.flags & PF_HAS_CF_NAME != 0 && !info.cf_name.is_null() {
            let n = CFString::wrap_under_get_rule(info.cf_name).to_string();
            if info.flags & PF_CF_NAME_RELEASE != 0 {
                CFRelease(info.cf_name as CFTypeRef);
            }
            n
        } else {
            CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned()
        };
        if info.flags & PF_WRITABLE == 0 || info.flags & PF_READABLE == 0 || info.max <= info.min {
            continue;
        }
        let r = Range { id, min: info.min, max: info.max, log: info.flags & PF_LOG != 0 };
        let steps = match info.unit {
            UNIT_BOOLEAN => 1,
            UNIT_INDEXED => (info.max - info.min).round() as i32,
            _ => 0,
        };
        out.push((r, name, r.to_norm(info.default), steps));
    }
    out
}

fn cfstring(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe { CFString::wrap_under_create_rule(s) }.to_string()
}

/// One AU on this Mac.
#[derive(Clone, Debug)]
pub struct AuInfo {
    pub path: String,
    pub name: String,
    pub vendor: String,
    pub instrument: bool,
}

/// Every effect and instrument AU on this Mac (no plugin is loaded for this).
pub fn list() -> Vec<AuInfo> {
    let mut out = Vec::new();
    for kind in [TYPE_EFFECT, TYPE_MUSIC_EFFECT, TYPE_INSTRUMENT] {
        let want = Desc { kind, ..Default::default() };
        let mut c: AudioComponent = ptr::null_mut();
        loop {
            c = unsafe { AudioComponentFindNext(c, &want) };
            if c.is_null() {
                break;
            }
            let mut d = Desc::default();
            let mut name: CFStringRef = ptr::null();
            unsafe {
                AudioComponentGetDescription(c, &mut d);
                AudioComponentCopyName(c, &mut name);
            }
            // "Vendor: Name"
            let full = cfstring(name);
            let (vendor, name) = match full.split_once(": ") {
                Some((v, n)) => (v.trim().to_string(), n.trim().to_string()),
                None => (String::new(), full.trim().to_string()),
            };
            out.push(AuInfo { path: path_of(d.kind, d.sub, d.maker), name, vendor, instrument: d.kind == TYPE_INSTRUMENT });
        }
    }
    out
}

/// A running AU, as the control side sees it (for its state and its window).
pub struct Instance {
    pub name: String,
    pub latency: u32,
    pub has_editor: bool,
    unit: usize, // AudioUnit
    window: Mutex<Option<usize>>, // a retained NSWindow*
    /// the values when its window was last opened, to tell which one you moved
    snap: Mutex<Vec<(u32, f32)>>,
}

unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Drop for Instance {
    fn drop(&mut self) {
        let unit = self.unit;
        let window = self.window.lock().unwrap().take();
        on_ui(move || unsafe {
            if let Some(w) = window {
                editor::close(w);
            }
            AudioUnitUninitialize(unit as AudioUnit);
            AudioComponentInstanceDispose(unit as AudioUnit);
        });
    }
}

fn registry() -> &'static Mutex<HashMap<u64, Weak<Instance>>> {
    static R: OnceLock<Mutex<HashMap<u64, Weak<Instance>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

pub fn instance(id: u64) -> Option<Arc<Instance>> {
    registry().lock().unwrap().get(&id).and_then(|w| w.upgrade())
}

impl Instance {
    /// The parameters automation can move, with where they are now (0..1).
    pub fn params(&self) -> Vec<crate::vst::ParamInfo> {
        let unit = self.unit as AudioUnit;
        unsafe {
            read_params(unit)
                .into_iter()
                .map(|(r, name, default, steps)| {
                    let mut v = 0f32;
                    AudioUnitGetParameter(unit, r.id, SCOPE_GLOBAL, 0, &mut v);
                    crate::vst::ParamInfo { id: r.id, name, default, value: r.to_norm(v), steps }
                })
                .collect()
        }
    }

    /// Which parameter moved most since its window was opened (an AU doesn't say on its own).
    pub fn touched(&self) -> Option<u32> {
        let unit = self.unit as AudioUnit;
        let snap = self.snap.lock().unwrap();
        let mut best: Option<(u32, f32)> = None;
        for &(id, before) in snap.iter() {
            let mut v = 0f32;
            unsafe { AudioUnitGetParameter(unit, id, SCOPE_GLOBAL, 0, &mut v) };
            let d = (v - before).abs();
            if d > 1e-5 && best.is_none_or(|(_, b)| d > b) {
                best = Some((id, d));
            }
        }
        best.map(|(id, _)| id)
    }

    fn take_snapshot(&self) {
        let unit = self.unit as AudioUnit;
        let now = unsafe { read_params(unit) }
            .into_iter()
            .map(|(r, ..)| {
                let mut v = 0f32;
                unsafe { AudioUnitGetParameter(unit, r.id, SCOPE_GLOBAL, 0, &mut v) };
                (r.id, v)
            })
            .collect();
        *self.snap.lock().unwrap() = now;
    }

    /// The AU's settings (its ClassInfo, as a binary property list), encoded "au:<base64>".
    pub fn state(&self) -> Option<String> {
        unsafe {
            let mut plist: CFPropertyListRef = ptr::null();
            let mut size = std::mem::size_of::<CFPropertyListRef>() as u32;
            let err = AudioUnitGetProperty(self.unit as AudioUnit, P_CLASS_INFO, SCOPE_GLOBAL, 0, &mut plist as *mut _ as *mut c_void, &mut size);
            if err != 0 || plist.is_null() {
                return None;
            }
            let data = CFPropertyListCreateData(ptr::null(), plist, BINARY_PLIST, 0, ptr::null_mut());
            CFRelease(plist as CFTypeRef);
            if data.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize).to_vec();
            CFRelease(data as CFTypeRef);
            Some(format!("au:{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
        }
    }

    /// Show the AU's own window (made the first time).
    pub fn show_editor(self: &Arc<Self>, title: String) -> Result<(), String> {
        self.take_snapshot();
        let me = Shared(self.clone());
        on_ui(move || unsafe {
            let me = me.0;
            let mut w = me.window.lock().unwrap();
            if let Some(win) = *w {
                editor::show(win);
                return Ok(());
            }
            *w = Some(editor::open(me.unit as AudioUnit, &title)?);
            Ok(())
        })
    }
}

unsafe fn restore(unit: AudioUnit, state: &str) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(state.strip_prefix("au:").unwrap_or(state))
        .map_err(|_| "the saved Audio Unit settings are damaged")?;
    let data = CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize);
    let plist = CFPropertyListCreateWithData(ptr::null(), data, 0, ptr::null_mut(), ptr::null_mut());
    CFRelease(data as CFTypeRef);
    if plist.is_null() {
        return Err("the saved Audio Unit settings are damaged".into());
    }
    let err = AudioUnitSetProperty(unit, P_CLASS_INFO, SCOPE_GLOBAL, 0, &plist as *const _ as *const c_void, std::mem::size_of::<CFPropertyListRef>() as u32);
    CFRelease(plist as CFTypeRef);
    if err != 0 { Err(format!("the Audio Unit didn't take its saved settings ({err})")) } else { Ok(()) }
}

/// What the render callback hands an effect: this block's input.
struct Input {
    bufs: [Vec<f32>; 2],
    frames: usize,
}

unsafe extern "C" fn render_input(refcon: *mut c_void, _flags: *mut u32, _ts: *const TimeStamp, _bus: u32, frames: u32, data: *mut BufferList) -> OSStatus {
    let input = &mut *(refcon as *mut Input);
    let list = &mut *data;
    let n = (frames as usize).min(input.frames).min(MAX_BLOCK);
    for (i, b) in list.bufs.iter_mut().take(list.count.min(2) as usize).enumerate() {
        if b.data.is_null() {
            b.data = input.bufs[i].as_mut_ptr() as *mut c_void;
        } else {
            ptr::copy_nonoverlapping(input.bufs[i].as_ptr(), b.data as *mut f32, n);
        }
        b.size = (n * 4) as u32;
    }
    0
}

unsafe extern "C" fn beat_and_tempo(_user: *mut c_void, beat: *mut f64, tempo: *mut f64) -> OSStatus {
    let t = f64::from_bits(crate::fx::TEMPO_BITS.load(Relaxed));
    if !tempo.is_null() {
        *tempo = if t > 0.0 { t } else { 120.0 };
    }
    if !beat.is_null() {
        *beat = 0.0;
    }
    0
}

fn stereo(sr: f64) -> Format {
    Format { rate: sr, format: FORMAT_LPCM, flags: FORMAT_FLAGS, bytes_per_packet: 4, frames_per_packet: 1, bytes_per_frame: 4, channels: 2, bits: 32, reserved: 0 }
}

/// The audio side of an AU: what process() needs, made beforehand.
pub struct AuProc {
    inst: Arc<Instance>,
    input: Box<Input>,
    out: [Vec<f32>; 2],
    dry: [Vec<f32>; 2],
    list: BufferList,
    ts: TimeStamp,
    instrument: bool,
    events: Vec<(u32, u32, u32, u32)>, // MIDI status, data 1, data 2, offset
    /// where each parameter's 0..1 goes (read once, for automation)
    ranges: Vec<Range>,
    /// automation for the next block: (index into ranges, value), and the last value sent
    auto: Vec<(usize, f32)>,
    sent: Vec<f32>,
    pub id: u64,
    sr: f64,
    _host: Box<HostCallbacks>,
}

unsafe impl Send for AuProc {}

impl AuProc {
    /// Load and start an AU (`path` as from `list`), with saved settings if any.
    pub fn create(path: &str, state: Option<&str>, sr: f64, instrument: bool) -> Result<AuProc, String> {
        let want = desc_of(path).ok_or("not an Audio Unit")?;
        unsafe {
            let c = AudioComponentFindNext(ptr::null_mut(), &want);
            if c.is_null() {
                return Err("this Audio Unit isn't on this Mac".into());
            }
            let mut name: CFStringRef = ptr::null();
            AudioComponentCopyName(c, &mut name);
            let full = cfstring(name);
            let name = full.split_once(": ").map_or(full.clone(), |(_, n)| n.to_string());
            let mut unit: AudioUnit = ptr::null_mut();
            let err = AudioComponentInstanceNew(c, &mut unit);
            if err != 0 || unit.is_null() {
                return Err(format!("the Audio Unit wouldn't start ({err})"));
            }
            let mut input = Box::new(Input { bufs: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]], frames: 0 });
            let host = Box::new(HostCallbacks { user: ptr::null_mut(), beat_and_tempo: Some(beat_and_tempo), musical_time: ptr::null(), transport: ptr::null(), transport2: ptr::null() });
            let fail = |unit: AudioUnit, why: String| {
                AudioComponentInstanceDispose(unit);
                Err::<AuProc, String>(why)
            };
            let fmt = stereo(sr);
            let fsize = std::mem::size_of::<Format>() as u32;
            if AudioUnitSetProperty(unit, P_STREAM_FORMAT, SCOPE_OUTPUT, 0, &fmt as *const _ as *const c_void, fsize) != 0 {
                return fail(unit, "the Audio Unit doesn't do stereo at this sample rate".into());
            }
            if !instrument {
                if AudioUnitSetProperty(unit, P_STREAM_FORMAT, SCOPE_INPUT, 0, &fmt as *const _ as *const c_void, fsize) != 0 {
                    return fail(unit, "the Audio Unit doesn't take stereo in".into());
                }
                let cb = RenderCallback { proc_: render_input, refcon: &mut *input as *mut Input as *mut c_void };
                if AudioUnitSetProperty(unit, P_RENDER_CALLBACK, SCOPE_INPUT, 0, &cb as *const _ as *const c_void, std::mem::size_of::<RenderCallback>() as u32) != 0 {
                    return fail(unit, "the Audio Unit wouldn't take its input".into());
                }
            }
            let max = MAX_BLOCK as u32;
            AudioUnitSetProperty(unit, P_MAX_FRAMES, SCOPE_GLOBAL, 0, &max as *const _ as *const c_void, 4);
            AudioUnitSetProperty(unit, P_HOST_CALLBACKS, SCOPE_GLOBAL, 0, &*host as *const _ as *const c_void, std::mem::size_of::<HostCallbacks>() as u32);
            let err = AudioUnitInitialize(unit);
            if err != 0 {
                return fail(unit, format!("the Audio Unit wouldn't start ({err})"));
            }
            let mut warn = None;
            if let Some(s) = state.filter(|s| !s.is_empty()) {
                if let Err(e) = restore(unit, s) {
                    warn = Some(e); // it still runs, with its defaults
                }
            }
            let _ = warn;
            let mut lat = 0f64;
            let mut size = 8u32;
            AudioUnitGetProperty(unit, P_LATENCY, SCOPE_GLOBAL, 0, &mut lat as *mut f64 as *mut c_void, &mut size);
            let mut ui_size = 0u32;
            let has_editor = AudioUnitGetPropertyInfo(unit, P_COCOA_UI, SCOPE_GLOBAL, 0, &mut ui_size, ptr::null_mut()) == 0 && ui_size > 0;
            let inst = Arc::new(Instance { name, latency: (lat.max(0.0) * sr).round() as u32, has_editor, unit: unit as usize, window: Mutex::new(None), snap: Mutex::new(Vec::new()) });
            let ranges: Vec<Range> = read_params(unit).into_iter().map(|(r, ..)| r).collect();
            let id = NEXT_ID.fetch_add(1, Relaxed);
            registry().lock().unwrap().insert(id, Arc::downgrade(&inst));
            Ok(AuProc {
                inst,
                input,
                out: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
                dry: [vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]],
                list: BufferList { count: 2, bufs: [Buffer { channels: 1, size: 0, data: ptr::null_mut() }, Buffer { channels: 1, size: 0, data: ptr::null_mut() }] },
                ts: TimeStamp { flags: TS_SAMPLE_TIME_VALID, rate_scalar: 1.0, ..Default::default() },
                instrument,
                events: Vec::with_capacity(512),
                sent: vec![f32::NAN; ranges.len()],
                auto: Vec::with_capacity(ranges.len()),
                ranges,
                id,
                sr,
                _host: host,
            })
        }
    }

    pub fn instance(&self) -> &Arc<Instance> {
        &self.inst
    }

    /// A new sample rate (the stream is stopped).
    pub fn set_sample_rate(&mut self, sr: f64) {
        self.sr = sr;
        let unit = self.inst.unit as AudioUnit;
        let instrument = self.instrument;
        unsafe {
            AudioUnitUninitialize(unit);
            let fmt = stereo(sr);
            let fsize = std::mem::size_of::<Format>() as u32;
            AudioUnitSetProperty(unit, P_SAMPLE_RATE, SCOPE_OUTPUT, 0, &sr as *const f64 as *const c_void, 8);
            AudioUnitSetProperty(unit, P_STREAM_FORMAT, SCOPE_OUTPUT, 0, &fmt as *const _ as *const c_void, fsize);
            if !instrument {
                AudioUnitSetProperty(unit, P_STREAM_FORMAT, SCOPE_INPUT, 0, &fmt as *const _ as *const c_void, fsize);
            }
            AudioUnitInitialize(unit);
        }
    }

    /// A note for the next block (instruments).
    pub fn note(&mut self, on: bool, key: u8, vel: u8, offset: usize) {
        if self.events.len() < self.events.capacity() {
            let status = if on { 0x90 } else { 0x80 };
            self.events.push((status, key as u32 & 127, if on { vel.max(1) as u32 } else { 0 }, offset.min(MAX_BLOCK - 1) as u32));
        }
    }

    /// One stereo block, in place; `mix` blends dry and wet (effects).
    /// Automation: a parameter's value (0..1) for the next block. Sent only when it changes.
    pub fn set_param(&mut self, id: u32, v: f32) {
        let Some(i) = self.ranges.iter().position(|r| r.id == id) else { return };
        if (self.sent[i] - v).abs() < 1e-6 {
            return;
        }
        self.sent[i] = v;
        match self.auto.iter_mut().find(|(k, _)| *k == i) {
            Some((_, x)) => *x = v,
            None if self.auto.len() < self.auto.capacity() => self.auto.push((i, v)),
            None => {}
        }
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], mix: f32, _tempo: f64) {
        let n = l.len().min(r.len()).min(MAX_BLOCK);
        let unit = self.inst.unit as AudioUnit;
        unsafe {
            for &(i, v) in &self.auto {
                let rg = self.ranges[i];
                AudioUnitSetParameter(unit, rg.id, SCOPE_GLOBAL, 0, rg.to_real(v), 0);
            }
            self.auto.clear();
            if self.instrument {
                for &(s, a, b, o) in &self.events {
                    MusicDeviceMIDIEvent(unit, s, a, b, o);
                }
                self.events.clear();
            } else {
                self.input.bufs[0][..n].copy_from_slice(&l[..n]);
                self.input.bufs[1][..n].copy_from_slice(&r[..n]);
                self.input.frames = n;
                self.dry[0][..n].copy_from_slice(&l[..n]);
                self.dry[1][..n].copy_from_slice(&r[..n]);
            }
            for (i, b) in self.list.bufs.iter_mut().enumerate() {
                b.channels = 1;
                b.size = (n * 4) as u32;
                b.data = self.out[i].as_mut_ptr() as *mut c_void;
            }
            self.list.count = 2;
            let mut flags = 0u32;
            let err = AudioUnitRender(unit, &mut flags, &self.ts, 0, n as u32, &mut self.list);
            self.ts.sample_time += n as f64;
            if err != 0 {
                if self.instrument {
                    l[..n].fill(0.0);
                    r[..n].fill(0.0);
                }
                return; // an effect that failed passes the sound through
            }
            // the AU may hand back its own buffers
            let a = self.list.bufs[0].data as *const f32;
            let b = if self.list.count > 1 { self.list.bufs[1].data as *const f32 } else { a };
            for k in 0..n {
                let (wl, wr) = (*a.add(k), *b.add(k));
                if self.instrument || mix >= 0.999 {
                    l[k] = wl;
                    r[k] = wr;
                } else {
                    l[k] = self.dry[0][k] * (1.0 - mix) + wl * mix;
                    r[k] = self.dry[1][k] * (1.0 - mix) + wr * mix;
                }
            }
        }
    }
}

/// AU windows: the AU's Cocoa view (AUv2), in a window of ours.
mod editor {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    pub unsafe fn open(unit: AudioUnit, title: &str) -> Result<usize, String> {
        let mtm = MainThreadMarker::new().ok_or("plugin windows need the main thread")?;
        let mut size = 0u32;
        if AudioUnitGetPropertyInfo(unit, P_COCOA_UI, SCOPE_GLOBAL, 0, &mut size, ptr::null_mut()) != 0 || size == 0 {
            return Err("this Audio Unit has no window of its own".into());
        }
        let mut info: CocoaViewInfo = std::mem::zeroed();
        let mut sz = std::mem::size_of::<CocoaViewInfo>() as u32;
        if AudioUnitGetProperty(unit, P_COCOA_UI, SCOPE_GLOBAL, 0, &mut info as *mut _ as *mut c_void, &mut sz) != 0 || info.bundle.is_null() {
            return Err("this Audio Unit has no window of its own".into());
        }
        let class_name = cfstring(info.class[0]);
        let url = info.bundle as *mut AnyObject; // CFURLRef is an NSURL
        let bundle: *mut AnyObject = msg_send![objc2::class!(NSBundle), bundleWithURL: url];
        CFRelease(info.bundle as CFTypeRef);
        if bundle.is_null() {
            return Err("couldn't open the Audio Unit's window bundle".into());
        }
        let _: bool = msg_send![bundle, load];
        let cname = std::ffi::CString::new(class_name.clone()).map_err(|_| "bad view class name")?;
        let cls = AnyClass::get(&cname).ok_or_else(|| format!("the Audio Unit's window class {class_name} isn't there"))?;
        let factory: *mut AnyObject = msg_send![cls, alloc];
        let factory: *mut AnyObject = msg_send![factory, init];
        if factory.is_null() {
            return Err("couldn't make the Audio Unit's window".into());
        }
        let view: *mut AnyObject = msg_send![factory, uiViewForAudioUnit: unit, withSize: NSSize::new(600.0, 400.0)];
        let _: () = msg_send![factory, release];
        if view.is_null() {
            return Err("the Audio Unit gave no window".into());
        }
        let view: Retained<NSView> = Retained::retain(view as *mut NSView).ok_or("no view")?;
        let frame = view.frame();
        let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(frame.size.width.max(200.0), frame.size.height.max(100.0)));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(mtm.alloc(), rect, style, NSBackingStoreType::Buffered, false);
        window.setReleasedWhenClosed(false);
        window.setTitle(&NSString::from_str(title));
        window.setContentView(Some(&view));
        window.center();
        window.makeKeyAndOrderFront(None);
        Ok(Retained::into_raw(window) as usize)
    }

    pub unsafe fn show(window: usize) {
        let w = &*(window as *const NSWindow);
        w.makeKeyAndOrderFront(None);
    }

    pub unsafe fn close(window: usize) {
        let w: Retained<NSWindow> = Retained::from_raw(window as *mut NSWindow).unwrap();
        w.setContentView(None);
        w.close();
    }
}
