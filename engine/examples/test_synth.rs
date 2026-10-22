// Test instrument for Duet's VST3 host: plays a sine at the pitch of the
// last note held, so the self-test can check that notes reach a plugin.
// Built from the vst3 crate's gain example (MIT OR Apache-2.0, (c) Micah
// Johnston): cargo build --release --example test_synth

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::slice;

use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c_string = CString::new(src).unwrap_or_else(|_| CString::default());
    let bytes = c_string.as_bytes_with_nul();

    for (src, dst) in bytes.iter().zip(dst.iter_mut()) {
        *dst = *src as c_char;
    }

    if bytes.len() > dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (src, dst) in src.encode_utf16().zip(dst.iter_mut()) {
        *dst = src as TChar;
        len += 1;
    }

    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

unsafe fn len_wstring(string: *const TChar) -> usize {
    let mut len = 0;

    while *string.offset(len) != 0 {
        len += 1;
    }

    len as usize
}

const PLUGIN_NAME: &str = "Sine (Duet test instrument)";

struct SynthProcessor {
    key: AtomicU32,   // 0 = silent, else the MIDI note + 1
    phase: AtomicU64, // f64 bits
    sr: AtomicU64,    // f64 bits
}

impl Class for SynthProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl SynthProcessor {
    const CID: TUID = uid(0x5D7E1A21, 0x0C6B4F55, 0x9A0E2B44, 0x31D7C9E1);
}

impl IPluginBaseTrait for SynthProcessor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult { kResultOk }
    unsafe fn terminate(&self) -> tresult { kResultOk }
}

impl IComponentTrait for SynthProcessor {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult { kNotImplemented }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult { kResultOk }
    unsafe fn getBusCount(&self, mediaType: MediaType, dir: BusDirection) -> i32 {
        match (mediaType as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kOutput) => 1,
            (MediaTypes_::kEvent, BusDirections_::kInput) => 1,
            _ => 0,
        }
    }
    unsafe fn getBusInfo(&self, mediaType: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if index != 0 { return kInvalidArgument; }
        let bus = &mut *bus;
        match (mediaType as MediaTypes, dir as BusDirections) {
            (MediaTypes_::kAudio, BusDirections_::kOutput) => {
                bus.mediaType = MediaTypes_::kAudio as MediaType;
                bus.direction = BusDirections_::kOutput as BusDirection;
                bus.channelCount = 2;
                copy_wstring("Output", &mut bus.name);
            }
            (MediaTypes_::kEvent, BusDirections_::kInput) => {
                bus.mediaType = MediaTypes_::kEvent as MediaType;
                bus.direction = BusDirections_::kInput as BusDirection;
                bus.channelCount = 1;
                copy_wstring("Notes", &mut bus.name);
            }
            _ => return kInvalidArgument,
        }
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult { kNotImplemented }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult { kResultOk }
    unsafe fn setActive(&self, _state: TBool) -> tresult { kResultOk }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult { kResultOk }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult { kResultOk }
}

impl IAudioProcessorTrait for SynthProcessor {
    unsafe fn setBusArrangements(&self, _inputs: *mut SpeakerArrangement, num_ins: i32, outputs: *mut SpeakerArrangement, num_outs: i32) -> tresult {
        if num_ins != 0 || num_outs != 1 || *outputs != SpeakerArr::kStereo { return kResultFalse; }
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if dir as BusDirections == BusDirections_::kOutput && index == 0 {
            *arr = SpeakerArr::kStereo;
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 { 0 }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        self.sr.store((*setup).sampleRate.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult { kResultOk }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let d = &*data;
        if let Some(events) = ComRef::from_raw(d.inputEvents) {
            for i in 0..events.getEventCount() {
                let mut e: Event = std::mem::zeroed();
                if events.getEvent(i, &mut e) != kResultOk { continue; }
                match e.r#type as u32 {
                    t if t == Event_::EventTypes_::kNoteOnEvent as u32 => self.key.store(e.__field0.noteOn.pitch as u32 + 1, Ordering::Relaxed),
                    t if t == Event_::EventTypes_::kNoteOffEvent as u32 => {
                        if self.key.load(Ordering::Relaxed) == e.__field0.noteOff.pitch as u32 + 1 {
                            self.key.store(0, Ordering::Relaxed);
                        }
                    }
                    _ => {}
                }
            }
        }
        if d.numOutputs < 1 { return kResultOk; }
        let out = &slice::from_raw_parts(d.outputs, d.numOutputs as usize)[0];
        let chans = slice::from_raw_parts(out.__field0.channelBuffers32, out.numChannels as usize);
        let n = d.numSamples as usize;
        let key = self.key.load(Ordering::Relaxed);
        let sr = f64::from_bits(self.sr.load(Ordering::Relaxed)).max(1.0);
        let mut ph = f64::from_bits(self.phase.load(Ordering::Relaxed));
        let f = 440.0 * 2f64.powf((key as f64 - 1.0 - 69.0) / 12.0);
        for k in 0..n {
            let v = if key == 0 { 0.0 } else { (ph * std::f64::consts::TAU).sin() as f32 * 0.5 };
            for c in chans {
                *(*c).add(k) = v;
            }
            ph = (ph + f / sr).fract();
        }
        self.phase.store(ph.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 { 0 }
}

impl IProcessContextRequirementsTrait for SynthProcessor {
    unsafe fn getProcessContextRequirements(&self) -> u32 { 0 }
}

struct Factory {}

impl Class for Factory {
    type Interfaces = (IPluginFactory, IPluginFactory2);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;
        copy_cstring("Duet", &mut info.vendor);
        copy_cstring("https://example.com", &mut info.url);
        copy_cstring("someone@example.com", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 { 1 }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        if index != 0 { return kInvalidArgument; }
        let info = &mut *info;
        info.cid = SynthProcessor::CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(PLUGIN_NAME, &mut info.name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        if *(cid as *const TUID) != SynthProcessor::CID { return kInvalidArgument; }
        let instance = ComWrapper::new(SynthProcessor { key: AtomicU32::new(0), phase: AtomicU64::new(0), sr: AtomicU64::new(48000f64.to_bits()) })
            .to_com_ptr::<FUnknown>()
            .unwrap();
        let ptr = instance.as_ptr();
        ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj)
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        if index != 0 { return kInvalidArgument; }
        let info = &mut *info;
        info.cid = SynthProcessor::CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring("Audio Module Class", &mut info.category);
        copy_cstring(PLUGIN_NAME, &mut info.name);
        copy_cstring("Instrument|Synth", &mut info.subCategories);
        copy_cstring("Duet", &mut info.vendor);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory {})
        .to_com_ptr::<IPluginFactory>()
        .unwrap()
        .into_raw()
}
