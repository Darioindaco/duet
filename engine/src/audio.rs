//! Sound card side: device list and the input/output streams (cpal).
//!
//! All cpal calls happen on one dedicated thread. Two reasons: cpal streams
//! can't move between threads, and ASIO drivers are COM objects that only load
//! on a single-threaded-apartment thread (the webview calls us from others).
//!
//! Data path: input callback -> mono samples -> lock-free ring -> output
//! callback, which runs Dsp::process in fixed `frame`-sized blocks and plays the
//! result. With ASIO both callbacks come from the same driver interrupt, so the
//! ring holds at most one block.

use std::collections::VecDeque;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Data, SampleFormat, SampleRate, StreamConfig};

use crate::dsp::{Dsp, Shared};

const WRAPPERS: [&str; 5] = ["fl studio", "asio4all", "generic low latency", "voicemeeter", "realtek asio"];

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub api: String,
    pub inputs: u16,
    pub outputs: u16,
}

pub struct DeviceList {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

pub struct StartOk {
    pub lat_in_ms: f32,
    pub lat_out_ms: f32,
}

enum Req {
    List(mpsc::Sender<DeviceList>),
    Start(StartArgs, mpsc::Sender<Result<StartOk, String>>),
    Stop(mpsc::Sender<()>),
}

pub struct StartArgs {
    pub input: String,
    pub output: String,
    pub channel: Option<usize>, // None = mix all inputs
    pub samplerate: u32,
    pub frame: usize,
    pub dsp: Arc<Mutex<Dsp>>,
    pub shared: Arc<Shared>,
}

pub struct AudioThread {
    tx: Mutex<mpsc::Sender<Req>>,
}

impl AudioThread {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Req>();
        std::thread::Builder::new()
            .name("duet-audio-control".into())
            .spawn(move || {
                com_init_sta();
                // only ever replaced or dropped: dropping a cpal stream stops it
                let mut streams: Option<(cpal::Stream, cpal::Stream)> = None;
                for req in rx {
                    match req {
                        Req::List(reply) => {
                            let _ = reply.send(list_devices());
                        }
                        Req::Start(args, reply) => {
                            drop(streams.take()); // stop the old ones first (drop = stop)
                            let res = build(args).map(|(i, o, ok)| {
                                streams = Some((i, o));
                                ok
                            });
                            let _ = reply.send(res);
                        }
                        Req::Stop(reply) => {
                            drop(streams.take());
                            let _ = reply.send(());
                        }
                    }
                }
            })
            .expect("spawn audio thread");
        AudioThread { tx: Mutex::new(tx) }
    }

    fn call<T>(&self, make: impl FnOnce(mpsc::Sender<T>) -> Req) -> Option<T> {
        let (rtx, rrx) = mpsc::channel();
        self.tx.lock().unwrap().send(make(rtx)).ok()?;
        rrx.recv().ok()
    }

    pub fn list(&self) -> DeviceList {
        self.call(Req::List).unwrap_or(DeviceList { inputs: vec![], outputs: vec![], default_input: None, default_output: None })
    }

    pub fn start(&self, args: StartArgs) -> Result<StartOk, String> {
        self.call(|r| Req::Start(args, r)).unwrap_or_else(|| Err("audio thread gone".into()))
    }

    pub fn stop(&self) {
        self.call(Req::Stop);
    }
}

#[cfg(windows)]
fn com_init_sta() {
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut std::ffi::c_void, coinit: u32) -> i32;
    }
    unsafe {
        CoInitializeEx(std::ptr::null_mut(), 0x2); // COINIT_APARTMENTTHREADED
    }
}

#[cfg(not(windows))]
fn com_init_sta() {}

// --- devices -------------------------------------------------------------------

fn api_rank(api: &str) -> usize {
    ["ASIO", "WASAPI", "CoreAudio", "ALSA", "JACK"].iter().position(|a| *a == api).unwrap_or(9)
}

fn is_wrapper(name: &str) -> bool {
    let n = name.to_lowercase();
    WRAPPERS.iter().any(|w| n.contains(w))
}

/// Walk every device of every host, calling `visit` with its info. Each cpal
/// Device is dropped before the next is opened: the ASIO layer can only hold
/// one driver at a time, and keeping e.g. "FL Studio ASIO" alive makes the
/// next driver (the Focusrite) fail to load and silently vanish from the list.
/// `visit` returns true to keep that device (and stop walking).
fn walk(mut visit: impl FnMut(&DeviceInfo, &cpal::Device) -> bool) -> Option<cpal::Device> {
    for host_id in cpal::available_hosts() {
        let Ok(host) = cpal::host_from_id(host_id) else { continue };
        let api = host_id.name().to_string();
        let Ok(devices) = host.devices() else { continue };
        let mut seen: Vec<String> = Vec::new();
        for dev in devices {
            let Ok(name) = dev.name() else { continue };
            let max_ch = |r: Result<Vec<cpal::SupportedStreamConfigRange>, _>| {
                r.unwrap_or_default().iter().map(|c| c.channels()).max().unwrap_or(0)
            };
            let inputs = max_ch(dev.supported_input_configs().map(|c| c.collect()));
            let outputs = max_ch(dev.supported_output_configs().map(|c| c.collect()));
            let dup = seen.iter().filter(|n| **n == name).count();
            seen.push(name.clone());
            let id = if dup == 0 { format!("{api}|{name}") } else { format!("{api}|{name}#{dup}") };
            if visit(&DeviceInfo { id, name, api: api.clone(), inputs, outputs }, &dev) {
                return Some(dev);
            }
        }
    }
    None
}

fn list_devices() -> DeviceList {
    let mut all: Vec<DeviceInfo> = Vec::new();
    walk(|d, _| {
        all.push(d.clone());
        false
    });
    all.sort_by_key(|d| (api_rank(&d.api), is_wrapper(&d.name)));
    let inputs: Vec<DeviceInfo> = all.iter().filter(|d| d.inputs > 0).cloned().collect();
    let outputs: Vec<DeviceInfo> = all.iter().filter(|d| d.outputs > 0).cloned().collect();

    // an ASIO interface with both directions wins; else the OS defaults
    let asio = inputs
        .iter()
        .find(|d| d.api == "ASIO" && !is_wrapper(&d.name) && outputs.iter().any(|o| o.id == d.id))
        .map(|d| d.id.clone());
    let host = cpal::default_host();
    let api = host.id().name().to_string();
    let def = |name: Option<String>, pool: &Vec<DeviceInfo>| {
        name.and_then(|n| pool.iter().find(|d| d.api == api && d.name == n).map(|d| d.id.clone()))
            .or_else(|| pool.first().map(|d| d.id.clone()))
    };
    let default_input = asio.clone().or_else(|| def(host.default_input_device().and_then(|d| d.name().ok()), &inputs));
    let default_output = asio.or_else(|| def(host.default_output_device().and_then(|d| d.name().ok()), &outputs));
    DeviceList { inputs, outputs, default_input, default_output }
}

// --- streams -------------------------------------------------------------------

fn read_mono(data: &Data, channels: usize, pick: Option<usize>, out: &mut Vec<f32>) {
    out.clear();
    macro_rules! go {
        ($t:ty, $conv:expr) => {
            if let Some(s) = data.as_slice::<$t>() {
                for frame in s.chunks(channels) {
                    let v = match pick {
                        Some(c) => $conv(frame[c.min(channels - 1)]),
                        None => frame.iter().map(|x| $conv(*x)).sum::<f32>() / channels as f32,
                    };
                    out.push(v);
                }
            }
        };
    }
    match data.sample_format() {
        SampleFormat::F32 => go!(f32, |x: f32| x),
        SampleFormat::I16 => go!(i16, |x: i16| x as f32 / 32768.0),
        SampleFormat::I32 => go!(i32, |x: i32| x as f32 / 2147483648.0),
        SampleFormat::U16 => go!(u16, |x: u16| (x as f32 - 32768.0) / 32768.0),
        _ => {}
    }
}

fn write_out(data: &mut Data, channels: usize, mono: impl Iterator<Item = f32>) {
    macro_rules! go {
        ($t:ty, $conv:expr) => {
            if let Some(s) = data.as_slice_mut::<$t>() {
                for (frame, v) in s.chunks_mut(channels).zip(mono) {
                    let v = v.clamp(-1.0, 1.0);
                    frame.fill($conv(v));
                }
            }
        };
    }
    match data.sample_format() {
        SampleFormat::F32 => go!(f32, |v: f32| v),
        SampleFormat::I16 => go!(i16, |v: f32| (v * 32767.0) as i16),
        SampleFormat::I32 => go!(i32, |v: f32| (v as f64 * 2147483647.0) as i32),
        SampleFormat::U16 => go!(u16, |v: f32| (v * 32767.0 + 32768.0) as u16),
        _ => {}
    }
}

fn find(id: &str) -> Option<(DeviceInfo, cpal::Device)> {
    let mut info = None;
    let dev = walk(|d, _| {
        let hit = d.id == id;
        if hit {
            info = Some(d.clone());
        }
        hit
    })?;
    Some((info?, dev))
}

fn build(a: StartArgs) -> Result<(cpal::Stream, cpal::Stream, StartOk), String> {
    let (in_info, in_dev) = find(&a.input).ok_or("input device not found")?;
    // Same device for both directions (the normal ASIO case): reuse it rather
    // than walking again, which would try to load a second driver instance.
    let (out_info, out_dev) = if a.output == a.input {
        (in_info.clone(), in_dev.clone())
    } else {
        find(&a.output).ok_or("output device not found")?
    };
    if in_info.api != out_info.api {
        return Err("pick input and output from the same driver type (e.g. both ASIO)".into());
    }
    let frame = a.frame;
    a.dsp.lock().unwrap().set_format(a.samplerate, frame);

    let in_fmt = in_dev.default_input_config().map_err(|e| e.to_string())?.sample_format();
    let out_fmt = out_dev.default_output_config().map_err(|e| e.to_string())?.sample_format();
    let in_ch = match a.channel {
        Some(c) => (c + 1) as u16,
        None => in_info.inputs,
    }
    .max(1);
    let out_ch = out_info.outputs.min(2).max(1);
    let cfg = |ch: u16| StreamConfig {
        channels: ch,
        sample_rate: SampleRate(a.samplerate),
        buffer_size: BufferSize::Fixed(frame as u32),
    };

    let (mut ring_tx, mut ring_rx) = rtrb::RingBuffer::<f32>::new(16 * 1024);
    let shared_in = a.shared.clone();
    let mut mono: Vec<f32> = Vec::with_capacity(8192);
    let pick = a.channel;
    let in_channels = in_ch as usize;
    let input = in_dev
        .build_input_stream_raw(
            &cfg(in_ch),
            in_fmt,
            move |data: &Data, info: &cpal::InputCallbackInfo| {
                let ts = info.timestamp();
                if let Some(d) = ts.callback.duration_since(&ts.capture) {
                    shared_in.lat_in_us.store(d.as_micros() as u64, Relaxed);
                }
                read_mono(data, in_channels, pick, &mut mono);
                shared_in.in_callbacks.fetch_add(1, Relaxed);
                let pk = mono.iter().fold(0f32, |a, v| a.max(v.abs()));
                shared_in.raw_in_peak.set(pk.max(shared_in.raw_in_peak.get() * 0.95));
                if ring_tx.slots() < mono.len() {
                    shared_in.xruns.fetch_add(1, Relaxed);
                }
                for s in &mono {
                    if ring_tx.push(*s).is_err() {
                        break;
                    }
                }
            },
            |e| eprintln!("input stream error: {e}"),
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| format!("input: {e}"))?;

    let shared_out = a.shared.clone();
    let dsp = a.dsp.clone();
    let mut fifo: VecDeque<f32> = VecDeque::with_capacity(16 * 1024);
    let mut in_blk = vec![0f32; frame];
    let mut out_blk = vec![0f32; frame];
    let out_channels = out_ch as usize;
    // With ASIO, input and output come from the same driver interrupt, input
    // first, so the input block is always there in time: no cushion, no added
    // delay. Elsewhere (CoreAudio, WASAPI, two devices) the callbacks run on
    // separate threads in no fixed order; one block of cushion absorbs that.
    let cushion = if in_info.api == "ASIO" { 0 } else { 1 };
    a.shared.cushion_frames.store((cushion * frame) as u64, Relaxed);
    let mut primed = false;
    let output = out_dev
        .build_output_stream_raw(
            &cfg(out_ch),
            out_fmt,
            move |data: &mut Data, info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                if let Some(d) = ts.playback.duration_since(&ts.callback) {
                    shared_out.lat_out_us.store(d.as_micros() as u64, Relaxed);
                }
                let frames = data.len() / out_channels;
                // Separate input/output devices drift apart; never let the input
                // ring build up more than the cushion plus one spare block.
                let allowed = frame * (1 + cushion) + frames;
                let spare = ring_rx.slots().saturating_sub(allowed);
                if spare > frame {
                    if let Ok(chunk) = ring_rx.read_chunk(spare) {
                        chunk.commit_all();
                    }
                }
                if !primed && ring_rx.slots() >= frame * (1 + cushion) {
                    primed = true;
                }
                while fifo.len() < frames {
                    if primed && ring_rx.slots() >= frame {
                        let chunk = ring_rx.read_chunk(frame).unwrap();
                        let (x, y) = chunk.as_slices();
                        in_blk[..x.len()].copy_from_slice(x);
                        in_blk[x.len()..].copy_from_slice(y);
                        chunk.commit_all();
                    } else {
                        in_blk.fill(0.0);
                        if primed {
                            // input fell behind: count it and rebuild the cushion
                            shared_out.ring_empty.fetch_add(1, Relaxed);
                            primed = false;
                        }
                    }
                    match dsp.try_lock() {
                        Ok(mut d) => d.process(&mut in_blk, &mut out_blk),
                        Err(_) => out_blk.fill(0.0),
                    }
                    fifo.extend(out_blk.iter().copied());
                }
                write_out(data, out_channels, fifo.drain(..frames));
            },
            |e| eprintln!("output stream error: {e}"),
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| format!("output: {e}"))?;

    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(150)); // let the callbacks report their latency
    let ok = StartOk {
        lat_in_ms: a.shared.lat_in_us.load(Relaxed) as f32 / 1000.0,
        lat_out_ms: a.shared.lat_out_us.load(Relaxed) as f32 / 1000.0,
    };
    Ok((input, output, ok))
}
