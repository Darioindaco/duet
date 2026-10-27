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

/// Inputs captured per stream (interfaces with more get their first 8).
pub const MAX_INPUTS: usize = 8;

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
    /// what "System default" currently points at (to notice it changing)
    pub default_input_name: Option<String>,
    pub default_output_name: Option<String>,
}

pub struct StartOk {
    pub lat_in_ms: f32,
    pub lat_out_ms: f32,
    pub in_channels: u16,
}

enum Req {
    List(bool, mpsc::Sender<DeviceList>),
    Start(StartArgs, mpsc::Sender<Result<StartOk, String>>),
    Stop(mpsc::Sender<()>),
}

pub struct StartArgs {
    pub input: String,
    pub output: String,
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
                        Req::List(include_asio, reply) => {
                            let _ = reply.send(list_devices(include_asio));
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

    /// All devices. With include_asio=false, skip ASIO (cheap enough to poll,
    /// and safe while an ASIO stream runs).
    pub fn list(&self, include_asio: bool) -> DeviceList {
        self.call(|r| Req::List(include_asio, r)).unwrap_or(DeviceList {
            inputs: vec![], outputs: vec![], default_input: None, default_output: None,
            default_input_name: None, default_output_name: None,
        })
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
fn walk(include_asio: bool, mut visit: impl FnMut(&DeviceInfo, &cpal::Device) -> bool) -> Option<cpal::Device> {
    for host_id in cpal::available_hosts() {
        let api = host_id.name().to_string();
        if api == "ASIO" && !include_asio {
            continue; // loading ASIO drivers while one streams can break the stream
        }
        let Ok(host) = cpal::host_from_id(host_id) else { continue };
        let Ok(devices) = host.devices() else { continue };
        // Which devices the OS lists as inputs / outputs decides, not whether
        // their formats could be read: e.g. a Mac's "External Headphones" or
        // AirPods can fail that query and would otherwise vanish from the list.
        // (Not for ASIO: listing those loads every driver once more.)
        let (in_names, out_names): (Vec<String>, Vec<String>) = if api == "ASIO" {
            (Vec::new(), Vec::new())
        } else {
            (
                host.input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default(),
                host.output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default(),
            )
        };
        let mut seen: Vec<String> = Vec::new();
        for dev in devices {
            let Ok(name) = dev.name() else { continue };
            let max_ch = |r: Result<Vec<cpal::SupportedStreamConfigRange>, _>, fallback: Option<u16>| {
                let n = r.unwrap_or_default().iter().map(|c| c.channels()).max().unwrap_or(0);
                if n > 0 { n } else { fallback.unwrap_or(0) }
            };
            let mut inputs = max_ch(dev.supported_input_configs().map(|c| c.collect()), dev.default_input_config().ok().map(|c| c.channels()));
            let mut outputs = max_ch(dev.supported_output_configs().map(|c| c.collect()), dev.default_output_config().ok().map(|c| c.channels()));
            if inputs == 0 && in_names.contains(&name) {
                inputs = 1;
            }
            if outputs == 0 && out_names.contains(&name) {
                outputs = 2;
            }
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

pub const SYSTEM_IN: &str = "SYSTEM|in";
pub const SYSTEM_OUT: &str = "SYSTEM|out";

/// The OS's current default devices, as entries that follow it: plug in
/// headphones and the Mac (or Windows) switches its default output to them,
/// and so does Duet.
fn system_default(input: bool) -> Option<(DeviceInfo, cpal::Device)> {
    let host = cpal::default_host();
    let dev = if input { host.default_input_device() } else { host.default_output_device() }?;
    let name = dev.name().unwrap_or_else(|_| "unknown".into());
    let ch = if input {
        dev.default_input_config().map(|c| c.channels()).unwrap_or(1)
    } else {
        dev.default_output_config().map(|c| c.channels()).unwrap_or(2)
    };
    let info = DeviceInfo {
        id: (if input { SYSTEM_IN } else { SYSTEM_OUT }).into(),
        name: format!("System default ({name})"),
        api: host.id().name().to_string(),
        inputs: if input { ch } else { 0 },
        outputs: if input { 0 } else { ch },
    };
    Some((info, dev))
}

fn list_devices(include_asio: bool) -> DeviceList {
    let mut all: Vec<DeviceInfo> = Vec::new();
    walk(include_asio, |d, _| {
        all.push(d.clone());
        false
    });
    all.sort_by_key(|d| (api_rank(&d.api), is_wrapper(&d.name)));
    let mut inputs: Vec<DeviceInfo> = all.iter().filter(|d| d.inputs > 0).cloned().collect();
    let mut outputs: Vec<DeviceInfo> = all.iter().filter(|d| d.outputs > 0).cloned().collect();
    let sys_in = system_default(true).map(|(i, _)| i);
    let sys_out = system_default(false).map(|(i, _)| i);
    let default_input_name = sys_in.as_ref().map(|i| i.name.clone());
    let default_output_name = sys_out.as_ref().map(|i| i.name.clone());
    // right after the ASIO entries (if any)
    let at = |v: &Vec<DeviceInfo>| v.iter().position(|d| d.api != "ASIO").unwrap_or(v.len());
    if let Some(i) = sys_in.clone() {
        let p = at(&inputs);
        inputs.insert(p, i);
    }
    if let Some(o) = sys_out.clone() {
        let p = at(&outputs);
        outputs.insert(p, o);
    }

    // an ASIO interface with both directions wins; otherwise follow the system
    let asio = inputs
        .iter()
        .find(|d| d.api == "ASIO" && !is_wrapper(&d.name) && outputs.iter().any(|o| o.id == d.id))
        .map(|d| d.id.clone());
    let default_input = asio.clone().or(sys_in.map(|i| i.id)).or_else(|| inputs.first().map(|d| d.id.clone()));
    let default_output = asio.or(sys_out.map(|o| o.id)).or_else(|| outputs.first().map(|d| d.id.clone()));
    DeviceList { inputs, outputs, default_input, default_output, default_input_name, default_output_name }
}

// --- streams -------------------------------------------------------------------

/// Interleaved device samples of any format -> interleaved f32.
fn read_all(data: &Data, out: &mut Vec<f32>) {
    out.clear();
    macro_rules! go {
        ($t:ty, $conv:expr) => {
            if let Some(s) = data.as_slice::<$t>() {
                out.extend(s.iter().map(|x| $conv(*x)));
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

/// Interleaved stereo frames -> the device: L/R on the first two channels
/// (any further ones silent), or a mono sum on a one-channel device.
fn write_out(data: &mut Data, channels: usize, stereo: impl Iterator<Item = (f32, f32)>) {
    macro_rules! go {
        ($t:ty, $conv:expr) => {
            if let Some(s) = data.as_slice_mut::<$t>() {
                for (frame, (l, r)) in s.chunks_mut(channels).zip(stereo) {
                    if channels == 1 {
                        frame[0] = $conv(((l + r) * 0.7071).clamp(-1.0, 1.0));
                    } else {
                        frame.fill($conv(0.0));
                        frame[0] = $conv(l.clamp(-1.0, 1.0));
                        frame[1] = $conv(r.clamp(-1.0, 1.0));
                    }
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
    if id == SYSTEM_IN {
        return system_default(true);
    }
    if id == SYSTEM_OUT {
        return system_default(false);
    }
    let mut info = None;
    let dev = walk(true, |d, _| {
        let hit = d.id == id;
        if hit {
            info = Some(d.clone());
        }
        hit
    })?;
    Some((info?, dev))
}

/// Between the input callback and the output callback (separate threads on
/// CoreAudio and WASAPI, maybe separate devices): how much input to keep in
/// hand so every output callback finds what it needs, and when the input has
/// really run ahead (two clocks drifting) rather than just arrived in a burst.
///
/// Input can come in bigger chunks than the output asks for (or the other way
/// round): that's normal and must neither starve the output nor be thrown away
/// as "too much". So the cushion grows a block each time the input runs dry
/// (up to MAX_CUSHION), and input is only dropped when even the *lowest* level
/// over about a second stayed above what's needed: real drift.
struct Cushion {
    block: usize,
    /// blocks kept in hand on top of what a callback reads
    extra: usize,
    primed: bool,
    /// the largest input chunk seen (samples)
    max_in: usize,
    win_min: usize,
    win_n: usize,
    win_len: usize,
}

const MAX_CUSHION: usize = 12;

impl Cushion {
    fn new(block: usize, extra: usize, win_len: usize) -> Self {
        Cushion { block, extra, primed: false, max_in: 0, win_min: usize::MAX, win_n: 0, win_len: win_len.max(1) }
    }
    fn input_chunk(&mut self, n: usize) {
        self.max_in = self.max_in.max(n);
    }
    /// May this callback read `need` samples, with the ring at `level`?
    fn ready(&mut self, level: usize, need: usize) -> bool {
        if !self.primed && level >= need + self.extra * self.block {
            self.primed = true;
        }
        self.primed && level >= self.block
    }
    /// The input ran dry mid-callback: keep more in hand from now on.
    fn ran_dry(&mut self) {
        self.primed = false;
        self.extra = (self.extra + 1).min(MAX_CUSHION);
    }
    /// After a callback, with the ring at `level`: how much input to drop
    /// because it has piled up for good (0 nearly always).
    fn trim(&mut self, level: usize, need: usize) -> usize {
        self.win_min = self.win_min.min(level);
        self.win_n += 1;
        if self.win_n < self.win_len {
            return 0;
        }
        let keep = (self.extra + 1) * self.block + self.max_in.max(need);
        let low = std::mem::replace(&mut self.win_min, usize::MAX);
        self.win_n = 0;
        if low > keep + self.block { low - keep } else { 0 }
    }
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
    // every input of the interface, so each mixer strip can pick its own
    let in_ch = in_info.inputs.clamp(1, MAX_INPUTS as u16);
    let out_ch = out_info.outputs.min(2).max(1);
    a.shared.in_channels.store(in_ch as u64, Relaxed);
    let cfg = |ch: u16| StreamConfig {
        channels: ch,
        sample_rate: SampleRate(a.samplerate),
        buffer_size: BufferSize::Fixed(frame as u32),
    };

    let in_channels = in_ch as usize;
    let (mut ring_tx, mut ring_rx) = rtrb::RingBuffer::<f32>::new(8 * MAX_INPUTS * 1024);
    let shared_in = a.shared.clone();
    let err_in = a.shared.clone();
    let err_out = a.shared.clone();
    a.shared.stream_error.store(false, Relaxed);
    let mut samples: Vec<f32> = Vec::with_capacity(MAX_INPUTS * 4096);
    let input = in_dev
        .build_input_stream_raw(
            &cfg(in_ch),
            in_fmt,
            move |data: &Data, info: &cpal::InputCallbackInfo| {
                let ts = info.timestamp();
                if let Some(d) = ts.callback.duration_since(&ts.capture) {
                    shared_in.lat_in_us.store(d.as_micros() as u64, Relaxed);
                }
                read_all(data, &mut samples);
                shared_in.in_callbacks.fetch_add(1, Relaxed);
                shared_in.in_chunk.fetch_max(samples.len() as u64, Relaxed);
                let pk = samples.iter().fold(0f32, |a, v| a.max(v.abs()));
                shared_in.raw_in_peak.set(pk.max(shared_in.raw_in_peak.get() * 0.95));
                if ring_tx.slots() < samples.len() {
                    shared_in.xruns.fetch_add(1, Relaxed);
                    return;
                }
                if let Ok(chunk) = ring_tx.write_chunk_uninit(samples.len()) {
                    let n = chunk.fill_from_iter(samples.iter().copied());
                    debug_assert_eq!(n, samples.len());
                }
            },
            move |e| {
                eprintln!("input stream error: {e}");
                err_in.stream_error.store(true, Relaxed);
            },
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| format!("input: {e}"))?;

    let shared_out = a.shared.clone();
    let dsp = a.dsp.clone();
    let mut fifo: VecDeque<(f32, f32)> = VecDeque::with_capacity(8 * 1024);
    let block = frame * in_channels; // one input block, in samples
    let mut in_blk = vec![0f32; block];
    let mut out_blk = vec![0f32; 2 * frame];
    let out_channels = out_ch as usize;
    // With ASIO, input and output come from the same driver interrupt, input
    // first, so the input block is always there in time: no cushion, no added
    // delay. Elsewhere (CoreAudio, WASAPI, two devices) the callbacks run on
    // separate threads in no fixed order; one block of cushion absorbs that.
    let start = if in_info.api == "ASIO" { 0 } else { 1 };
    a.shared.cushion_frames.store((start * frame) as u64, Relaxed);
    a.shared.in_chunk.store(0, Relaxed);
    // about a second of output callbacks per drift check
    let mut cushion = Cushion::new(block, start, (a.samplerate as usize / frame.max(1)).max(8));
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
                cushion.input_chunk(shared_out.in_chunk.load(Relaxed) as usize);
                // what this callback reads: whole blocks, enough for its frames
                let need = frames.saturating_sub(fifo.len()).div_ceil(frame.max(1)) * block;
                let mut ok = cushion.ready(ring_rx.slots(), need);
                while fifo.len() < frames {
                    if ok && ring_rx.slots() >= block {
                        let chunk = ring_rx.read_chunk(block).unwrap();
                        let (x, y) = chunk.as_slices();
                        in_blk[..x.len()].copy_from_slice(x);
                        in_blk[x.len()..].copy_from_slice(y);
                        chunk.commit_all();
                    } else {
                        in_blk.fill(0.0);
                        if ok {
                            // input fell behind: count it, keep more in hand from now on
                            shared_out.ring_empty.fetch_add(1, Relaxed);
                            cushion.ran_dry();
                            shared_out.cushion_frames.store((cushion.extra * frame) as u64, Relaxed);
                            ok = false;
                        }
                    }
                    match dsp.try_lock() {
                        Ok(mut d) => d.process(&in_blk, in_channels, &mut out_blk),
                        Err(_) => out_blk.fill(0.0),
                    }
                    fifo.extend(out_blk.chunks_exact(2).map(|p| (p[0], p[1])));
                }
                write_out(data, out_channels, fifo.drain(..frames));
                // two devices' clocks drift apart: drop input only once it has
                // really piled up (frame-aligned)
                let drop = cushion.trim(ring_rx.slots(), need);
                let drop = drop - drop % in_channels;
                if drop > 0 {
                    if let Ok(chunk) = ring_rx.read_chunk(drop) {
                        chunk.commit_all();
                        shared_out.in_dropped.fetch_add(1, Relaxed);
                    }
                }
            },
            move |e| {
                eprintln!("output stream error: {e}");
                err_out.stream_error.store(true, Relaxed);
            },
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| format!("output: {e}"))?;

    input.play().map_err(|e| e.to_string())?;
    output.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(150)); // let the callbacks report their latency
    let ok = StartOk {
        lat_in_ms: a.shared.lat_in_us.load(Relaxed) as f32 / 1000.0,
        lat_out_ms: a.shared.lat_out_us.load(Relaxed) as f32 / 1000.0,
        in_channels: in_ch,
    };
    Ok((input, output, ok))
}

#[cfg(test)]
mod tests {
    use super::Cushion;

    /// Input and output at the same rate, in chunks of `inp` and `out` frames
    /// (2 channels, blocks of 128): returns (times it ran dry, samples dropped).
    fn run(inp: usize, out: usize, drift_ppm: f64, seconds: usize) -> (usize, usize) {
        let (ch, frame) = (2, 128);
        let block = frame * ch;
        let mut c = Cushion::new(block, 1, 48000 / out);
        let (mut level, mut fifo) = (0usize, 0usize);
        let (mut dry, mut dropped) = (0, 0);
        let total = 48000 * seconds;
        let (mut t_in, mut t_out) = (0.0f64, 0usize);
        let step_in = inp as f64 * (1.0 - drift_ppm * 1e-6); // its clock runs a bit fast (+) or slow (-)
        while t_out < total {
            // whichever callback is due next
            if t_in <= t_out as f64 {
                level += inp * ch;
                c.input_chunk(inp * ch);
                t_in += step_in;
            } else {
                let need = out.saturating_sub(fifo).div_ceil(frame) * block;
                let mut ok = c.ready(level, need);
                while fifo < out {
                    if ok && level >= block {
                        level -= block;
                    } else if ok {
                        dry += 1;
                        c.ran_dry();
                        ok = false;
                    }
                    fifo += frame;
                }
                fifo -= out;
                let d = c.trim(level, need);
                level -= d;
                dropped += d;
                t_out += out;
            }
        }
        (dry, dropped)
    }

    #[test]
    fn bursts_either_way_are_neither_starved_nor_thrown_away() {
        for (inp, out) in [(128, 128), (512, 128), (128, 512), (480, 128), (128, 441)] {
            let (dry, dropped) = run(inp, out, 0.0, 30);
            assert!(dry <= 3, "in {inp} / out {out}: ran dry {dry} times");
            assert_eq!(dropped, 0, "in {inp} / out {out}: nothing dropped at the same rate");
        }
    }

    #[test]
    fn drift_between_two_clocks_is_trimmed_rarely() {
        // input 1000 ppm fast: 48 frames a second pile up; trimmed, but no more
        // than what piled up (2 channels: 96 samples a second)
        let (dry, dropped) = run(512, 128, 1000.0, 60);
        assert!(dry <= 3, "{dry}");
        assert!(dropped > 0 && dropped <= 96 * 60, "{dropped}");
        // input slow: it runs dry now and then, not all the time
        let (dry, dropped) = run(128, 128, -100.0, 60);
        assert!(dry < 20 && dropped == 0, "{dry} {dropped}");
    }
}
