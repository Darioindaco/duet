//! duet_engine: the realtime half of Duet, as a Python extension.
//!
//! Python (duet.py + the web UI) does setup and display. Everything time-critical
//! runs here on native threads that never touch the GIL: the audio callback
//! (dsp.rs), the network receive thread, the control thread (punching,
//! pings, signaling resends) and the file thread (sending takes).

mod audio;
mod daw;
mod dsp;
mod fx;
mod instr;
mod jitter;
mod midi;
mod net;
mod signal;
mod transfer;
mod vst;

use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use daw::{AudioCache, ClipDesc, NoteDesc, MAX_TRACKS};
use dsp::{decode_peer, encode_peer, Cmd, Dsp, Garbage, InPacket, RecRings, Shared, StripParam, StripSettings, Target, MAX_LOCAL, MAX_REMOTE};
use fx::{kind_index, param_index, preset_slots, Slot, SlotCfg, FX, MAX_SLOTS};
use instr::{InstCfg, InstSlot, NoteEv};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

/// Non-realtime state shared by the network threads and Python calls.
struct Ctl {
    candidates: Vec<SocketAddrV4>,
    last_heard: Option<Instant>,
    rtt_ms: Option<f64>,
    warning: String,
    signals: signal::Signals,
    stun_txid: [u8; 12],
    stun_reply: Option<Vec<u8>>,
    // mirror of the mixer the audio thread has (the UI reads this, never the audio side)
    local: Vec<StripMirror>,
    remote: Vec<StripMirror>,
    tracks: Vec<StripMirror>,
    master: f32,
}

/// The control side of the timeline: loaded audio, the clip layout (kept to
/// rebuild after a sample-rate change) and the take being recorded.
struct DawCtl {
    cache: AudioCache,
    desc: Vec<Vec<ClipDesc>>,
    notes: Vec<Vec<NoteDesc>>,
    rec: Option<RecSession>,
    export: Option<ExportJob>,
}

struct ExportJob {
    done: Arc<std::sync::atomic::AtomicU64>,
    total: u64,
    worker: std::thread::JoinHandle<Result<Vec<String>, String>>,
}

struct RecSession {
    writer: std::thread::JoinHandle<Vec<daw::RecOut>>,
    sr: u32,
    /// what the sound cards add (input + output): a take lands this much earlier
    comp: f64,
}

#[derive(Clone)]
struct StripMirror {
    s: StripSettings,
    rack: Vec<SlotCfg>,
    inst: Option<InstCfg>,
}

struct Inner {
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    ctl: Mutex<Ctl>,
    cmds: Mutex<rtrb::Producer<Cmd>>,
    incoming: Mutex<Option<rtrb::Producer<InPacket>>>,
    /// things the audio thread is done with, freed here instead of there
    retired: Mutex<rtrb::Consumer<Garbage>>,
    /// notes for the strips' instruments (MIDI thread and Python both write)
    notes: Mutex<rtrb::Producer<(Target, NoteEv)>>,
    /// notes played on song tracks while recording, with their timeline position
    noterec: Mutex<rtrb::Consumer<(u8, NoteEv, i64)>>,
    /// the strip a MIDI keyboard plays (encoded Target; u64::MAX = none)
    midi_target: std::sync::atomic::AtomicU64,
    daw: Mutex<DawCtl>,
    files: Mutex<transfer::Transfers>,
    running: AtomicBool,
    start: Instant,
}

impl Inner {
    fn peer(&self) -> Option<SocketAddrV4> {
        decode_peer(self.shared.peer.load(Relaxed))
    }
    fn set_peer(&self, a: Option<SocketAddrV4>) {
        self.shared.peer.store(a.map(encode_peer).unwrap_or(0), Relaxed);
        self.shared.peer_epoch.fetch_add(1, Relaxed);
        self.files.lock().unwrap().abort_all();
    }
    fn send(&self, data: &[u8], to: Option<SocketAddrV4>) {
        let loss = self.shared.test_signal_loss.get();
        if loss > 0.0 && data.len() > 2 && (data[2] == net::T_SIGNAL || data[2] == net::T_SIGACK) && rand::random::<f32>() < loss {
            return;
        }
        if let Some(addr) = to.or_else(|| self.peer()) {
            let _ = self.socket.send_to(data, addr);
        }
    }
    fn connected(&self) -> bool {
        self.peer().is_some()
            && self.ctl.lock().unwrap().last_heard.is_some_and(|t| t.elapsed() < CONNECT_TIMEOUT)
    }
    fn cmd(&self, c: Cmd) {
        let _ = self.cmds.lock().unwrap().push(c);
    }
    fn note(&self, t: Target, ev: NoteEv) {
        let _ = self.notes.lock().unwrap().push((t, ev));
        self.shared.midi_events.fetch_add(1, Relaxed);
    }
    /// Drop effect objects the audio thread handed back (freeing happens here,
    /// not in the callback).
    fn free_retired(&self) {
        let mut r = self.retired.lock().unwrap();
        while r.pop().is_ok() {}
    }
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn recv_loop(self: Arc<Self>) {
        let mut buf = [0u8; 4096];
        let mut pkt = Box::new(InPacket::default());
        let mut reply = Vec::with_capacity(64);
        while self.running.load(Relaxed) {
            let (n, src) = match self.socket.recv_from(&mut buf) {
                Ok(x) => x,
                Err(_) => continue, // timeout or ICMP noise
            };
            let SocketAddr::V4(src) = src else { continue };
            let data = &buf[..n];
            let Some((ptype, seq, body)) = net::unpack(data) else {
                if net::is_stun_response(data) {
                    let mut c = self.ctl.lock().unwrap();
                    if data[8..20] == c.stun_txid {
                        c.stun_reply = Some(data.to_vec());
                    }
                }
                continue;
            };

            if ptype == net::T_HELLO {
                // whichever address their packets really come from is the one
                // their router opened for us: lock onto it
                if self.peer() != Some(src) {
                    self.set_peer(Some(src));
                    self.ctl.lock().unwrap().signals.reset();
                }
                let mut c = self.ctl.lock().unwrap();
                c.last_heard = Some(Instant::now());
                if body.len() >= 6 {
                    let sr = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as u64;
                    let fr = u16::from_be_bytes([body[4], body[5]]) as u64;
                    let (my_sr, my_fr) = (self.shared.samplerate.load(Relaxed), self.shared.frame.load(Relaxed));
                    let proto = if body.len() >= 7 { body[6] } else { 1 };
                    self.shared.peer_protocol.store(proto as u64, Relaxed);
                    c.warning = if (sr, fr) != (my_sr, my_fr) {
                        format!("Partner runs {sr} Hz / {fr} buffer, you run {my_sr} Hz / {my_fr}. Match them.")
                    } else {
                        String::new()
                    };
                }
                continue;
            }
            if self.peer() != Some(src) {
                continue;
            }
            self.ctl.lock().unwrap().last_heard = Some(Instant::now());

            match ptype {
                net::T_AUDIO | net::T_AUDIO_MC => {
                    // T_AUDIO (protocol 1) is one mono channel; T_AUDIO_MC has a
                    // 3-byte header: total channels, first channel, channels here
                    let (total, first, count, pcm) = if ptype == net::T_AUDIO {
                        (1u8, 0u8, 1u8, body)
                    } else if body.len() >= 3 && body[2] > 0 {
                        (body[0], body[1], body[2], &body[3..])
                    } else {
                        continue;
                    };
                    let samples = (pcm.len() / 2).min(jitter::MAX_FRAME);
                    pkt.seq = seq;
                    pkt.total = total;
                    pkt.first = first;
                    pkt.count = count;
                    pkt.len = (samples / count as usize) as u16;
                    for i in 0..samples {
                        pkt.data[i] = i16::from_le_bytes([pcm[2 * i], pcm[2 * i + 1]]);
                    }
                    if let Some(tx) = self.incoming.lock().unwrap().as_mut() {
                        let _ = tx.push(*pkt);
                    }
                    self.shared.packets_in.fetch_add(1, Relaxed);
                }
                net::T_PING => {
                    net::pack(net::T_PONG, seq, body, &mut reply);
                    self.send(&reply, None);
                }
                net::T_PONG if body.len() >= 8 => {
                    let t0 = f64::from_be_bytes(body[..8].try_into().unwrap());
                    let rtt = (self.now() - t0) * 1000.0;
                    let mut c = self.ctl.lock().unwrap();
                    c.rtt_ms = Some(c.rtt_ms.map_or(rtt, |r| r * 0.8 + rtt * 0.2));
                }
                net::T_SIGNAL => {
                    let complete = self.ctl.lock().unwrap().signals.on_chunk(seq, body);
                    if complete {
                        net::pack(net::T_SIGACK, seq, &[], &mut reply);
                        self.send(&reply, None);
                    }
                }
                net::T_SIGACK => self.ctl.lock().unwrap().signals.ack(seq),
                net::T_FILE => {
                    let r = self.files.lock().unwrap().on_file(seq, body);
                    if let Some(r) = r {
                        self.send(&r, None);
                    }
                }
                net::T_FILEACK => self.files.lock().unwrap().on_ack(seq, body),
                net::T_BYE => {
                    self.set_peer(None);
                    self.ctl.lock().unwrap().rtt_ms = None;
                }
                _ => {}
            }
        }
    }

    fn control_loop(self: Arc<Self>) {
        let mut next_ping = Instant::now();
        let mut ping_seq = 0u32;
        let mut hello = Vec::with_capacity(16);
        let mut pkt = Vec::with_capacity(32);
        while self.running.load(Relaxed) {
            let mut body = [0u8; 7];
            body[..4].copy_from_slice(&(self.shared.samplerate.load(Relaxed) as u32).to_be_bytes());
            body[4..6].copy_from_slice(&(self.shared.frame.load(Relaxed) as u16).to_be_bytes());
            body[6] = net::PROTOCOL;
            net::pack(net::T_HELLO, 0, &body, &mut hello);
            if !self.connected() {
                // punch: keep firing at every candidate until one gets through
                let cands = self.ctl.lock().unwrap().candidates.clone();
                for a in cands {
                    self.send(&hello, Some(a));
                }
                if self.peer().is_some() {
                    self.send(&hello, None);
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            if Instant::now() >= next_ping {
                self.send(&hello, None);
                net::pack(net::T_PING, ping_seq, &self.now().to_be_bytes(), &mut pkt);
                self.send(&pkt, None);
                ping_seq = ping_seq.wrapping_add(1);
                next_ping = Instant::now() + Duration::from_secs(1);
            }
            let due = self.ctl.lock().unwrap().signals.due();
            for p in due {
                self.send(&p, None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Paces file transfers: every few ms while one runs, idle otherwise.
    fn file_loop(self: Arc<Self>) {
        while self.running.load(Relaxed) {
            let rtt = self.ctl.lock().unwrap().rtt_ms;
            let (packets, active) = {
                let mut f = self.files.lock().unwrap();
                (f.tick(rtt), f.active())
            };
            if self.peer().is_some() {
                for p in packets {
                    self.send(&p, None);
                }
            }
            std::thread::sleep(Duration::from_millis(if active { 3 } else { 40 }));
        }
    }
}

#[pyclass]
struct Engine {
    inner: Arc<Inner>,
    dsp: Arc<Mutex<Dsp>>,
    audio: audio::AudioThread,
    midi: midi::MidiThread,
    local: SocketAddrV4,
    public: Mutex<Option<SocketAddrV4>>,
}

#[pymethods]
impl Engine {
    #[new]
    #[pyo3(signature = (port=0))]
    fn new(port: u16) -> PyResult<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        socket.set_read_timeout(Some(Duration::from_millis(200))).ok();
        disable_udp_connreset(&socket);
        let local_addr = SocketAddrV4::new(net::local_ip(), socket.local_addr().unwrap().port());
        let socket = Arc::new(socket);
        let shared = Arc::new(Shared::default());
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(1024);
        let (pkt_tx, pkt_rx) = rtrb::RingBuffer::new(512);
        let (ret_tx, ret_rx) = rtrb::RingBuffer::<Garbage>::new(256);
        let (note_tx, note_rx) = rtrb::RingBuffer::<(Target, NoteEv)>::new(1024);
        let (noterec_tx, noterec_rx) = rtrb::RingBuffer::<(u8, NoteEv, i64)>::new(8192);
        let dsp = Dsp::new(48000, 128, cmd_rx, pkt_rx, note_rx, noterec_tx, ret_tx, socket.clone(), shared.clone());
        let mut local: Vec<StripMirror> =
            (0..MAX_LOCAL).map(|i| StripMirror { s: StripSettings::local(i), rack: vec![], inst: None }).collect();
        local[0].s.active = true; // same default as the audio side
        let inner = Arc::new(Inner {
            socket,
            shared,
            ctl: Mutex::new(Ctl {
                candidates: vec![],
                last_heard: None,
                rtt_ms: None,
                warning: String::new(),
                signals: signal::Signals::new(),
                stun_txid: [0; 12],
                stun_reply: None,
                local,
                remote: vec![StripMirror { s: StripSettings::remote(), rack: vec![], inst: None }; MAX_REMOTE],
                tracks: vec![StripMirror { s: StripSettings::track(), rack: vec![], inst: None }; MAX_TRACKS],
                master: 1.0,
            }),
            cmds: Mutex::new(cmd_tx),
            incoming: Mutex::new(Some(pkt_tx)),
            retired: Mutex::new(ret_rx),
            notes: Mutex::new(note_tx),
            midi_target: std::sync::atomic::AtomicU64::new(u64::MAX),
            daw: Mutex::new(DawCtl { cache: AudioCache::default(), desc: vec![], notes: vec![], rec: None, export: None }),
            noterec: Mutex::new(noterec_rx),
            files: Mutex::new(transfer::Transfers::new(std::env::temp_dir().join("duet-incoming"))),
            running: AtomicBool::new(false),
            start: Instant::now(),
        });
        let sink_inner = inner.clone();
        let midi = midi::MidiThread::spawn(Arc::new(move |ev| {
            if let Some(t) = decode_target(sink_inner.midi_target.load(Relaxed)) {
                sink_inner.note(t, ev);
            }
        }));
        Ok(Engine { inner, dsp: Arc::new(Mutex::new(dsp)), audio: audio::AudioThread::spawn(), midi, local: local_addr, public: Mutex::new(None) })
    }

    /// Start the network threads.
    fn start(&self) {
        if self.inner.running.swap(true, Relaxed) {
            return;
        }
        let a = self.inner.clone();
        std::thread::Builder::new().name("duet-recv".into()).spawn(move || a.recv_loop()).unwrap();
        let b = self.inner.clone();
        std::thread::Builder::new().name("duet-control".into()).spawn(move || b.control_loop()).unwrap();
        let c = self.inner.clone();
        std::thread::Builder::new().name("duet-files".into()).spawn(move || c.file_loop()).unwrap();
    }

    /// Look up our public address (STUN, same socket as the audio) and return our code.
    fn discover(&self, py: Python<'_>) -> String {
        let inner = self.inner.clone();
        let local = self.local;
        let public = py.allow_threads(move || {
            for host in net::STUN_SERVERS {
                let Some(server) = net::resolve(host) else { continue };
                for _ in 0..2 {
                    let txid: [u8; 12] = rand::random();
                    {
                        let mut c = inner.ctl.lock().unwrap();
                        c.stun_txid = txid;
                        c.stun_reply = None;
                    }
                    inner.send(&net::stun_request(&txid), Some(server));
                    let until = Instant::now() + Duration::from_millis(1500);
                    while Instant::now() < until {
                        if let Some(r) = inner.ctl.lock().unwrap().stun_reply.take() {
                            if let Some(a) = net::parse_stun_response(&r) {
                                return Some(a);
                            }
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            None
        });
        *self.public.lock().unwrap() = public;
        net::make_code(public, local)
    }

    fn connect(&self, code: &str) -> PyResult<()> {
        let cands = net::parse_code(code).map_err(PyValueError::new_err)?;
        self.inner.ctl.lock().unwrap().candidates = cands;
        self.inner.set_peer(None);
        Ok(())
    }

    fn close(&self, py: Python<'_>) {
        if self.inner.peer().is_some() {
            let mut p = Vec::new();
            net::pack(net::T_BYE, 0, &[], &mut p);
            self.inner.send(&p, None);
        }
        py.allow_threads(|| self.audio.stop());
        self.inner.running.store(false, Relaxed);
    }

    // --- audio ---------------------------------------------------------------------

    /// ([inputs], [outputs], default input id, default output id, system default
    /// input name, system default output name); each device a dict.
    /// include_asio=False skips ASIO drivers (for polling while one streams).
    #[pyo3(signature = (include_asio=true))]
    #[allow(clippy::type_complexity)]
    fn devices<'py>(&self, py: Python<'py>, include_asio: bool) -> PyResult<(Vec<Bound<'py, PyDict>>, Vec<Bound<'py, PyDict>>, Option<String>, Option<String>, Option<String>, Option<String>)> {
        let list = py.allow_threads(|| self.audio.list(include_asio));
        let conv = |d: &audio::DeviceInfo| -> PyResult<Bound<'py, PyDict>> {
            let o = PyDict::new(py);
            o.set_item("id", &d.id)?;
            o.set_item("name", &d.name)?;
            o.set_item("api", &d.api)?;
            o.set_item("inputs", d.inputs)?;
            o.set_item("outputs", d.outputs)?;
            Ok(o)
        };
        Ok((
            list.inputs.iter().map(conv).collect::<PyResult<_>>()?,
            list.outputs.iter().map(conv).collect::<PyResult<_>>()?,
            list.default_input,
            list.default_output,
            list.default_input_name,
            list.default_output_name,
        ))
    }

    /// Open the sound card (all its inputs; strips pick theirs).
    /// Returns (input latency ms, output latency ms, number of inputs).
    #[pyo3(signature = (input, output, samplerate, frame))]
    fn start_audio(&self, py: Python<'_>, input: String, output: String, samplerate: u32, frame: usize) -> PyResult<(f32, f32, u16)> {
        let args = audio::StartArgs { input, output, samplerate, frame, dsp: self.dsp.clone(), shared: self.inner.shared.clone() };
        let res = py.allow_threads(|| self.audio.start(args));
        self.inner.shared.streaming.store(res.is_ok(), Relaxed);
        let ok = res.map_err(PyRuntimeError::new_err)?;
        self.rebuild_timeline(); // clip positions are samples at the (maybe new) rate
        Ok((ok.lat_in_ms, ok.lat_out_ms, ok.in_channels))
    }

    fn stop_audio(&self, py: Python<'_>) {
        py.allow_threads(|| self.audio.stop());
        self.inner.shared.streaming.store(false, Relaxed);
    }

    /// Run one block without a sound card (tests). Not while a stream is running.
    /// One block of interleaved input with `channels` interface inputs in; the
    /// left output channel out (or the right with right=True). Call set_format
    /// first if the block isn't 128.
    #[pyo3(signature = (input, channels=1, right=false))]
    fn process(&self, input: Vec<f32>, channels: usize, right: bool) -> Vec<f32> {
        let frames = input.len() / channels.max(1);
        let mut out = vec![0f32; 2 * frames];
        self.dsp.lock().unwrap().process(&input, channels.max(1), &mut out);
        self.inner.free_retired();
        out.chunks_exact(2).map(|p| p[right as usize]).collect()
    }

    fn set_format(&self, samplerate: u32, frame: usize) {
        self.dsp.lock().unwrap().set_format(samplerate, frame);
        self.rebuild_timeline();
    }

    // --- mixer ---------------------------------------------------------------------
    // Strips are addressed as "L0".."L3" (my inputs) and "R0".."R7" (the
    // partner's channels, in the order they send them). Every change updates
    // the mirror here and sends one command to the audio thread.

    /// Strip setting: active, input, gain, pan, fader, mute, solo, send, mon.
    fn strip_set(&self, target: &str, key: &str, value: f64) -> PyResult<()> {
        let t = parse_target(target)?;
        let p = match key {
            "active" => StripParam::Active,
            "input" => StripParam::Input,
            "gain" => StripParam::Gain,
            "pan" => StripParam::Pan,
            "fader" => StripParam::Fader,
            "mute" => StripParam::Mute,
            "solo" => StripParam::Solo,
            "send" => StripParam::Send,
            "mon" => StripParam::Mon,
            _ => return Err(PyValueError::new_err(format!("no strip setting {key}"))),
        };
        let mut c = self.inner.ctl.lock().unwrap();
        mirror(&mut c, t).s.set(p, value as f32);
        self.cmd(Cmd::Strip(t, p, value as f32));
        Ok(())
    }

    /// Add an effect to a strip's rack (at `pos`, default the end). Returns its position.
    #[pyo3(signature = (target, kind, pos=None))]
    fn rack_add(&self, target: &str, kind: &str, pos: Option<usize>) -> PyResult<usize> {
        let t = parse_target(target)?;
        let k = kind_index(kind).ok_or_else(|| PyValueError::new_err(format!("no effect {kind}")))?;
        if k == fx::VST {
            return Err(PyValueError::new_err("a VST slot needs a plugin: use rack_add_vst"));
        }
        let mut c = self.inner.ctl.lock().unwrap();
        let rack = &mut mirror(&mut c, t).rack;
        if rack.len() >= MAX_SLOTS {
            return Err(PyValueError::new_err(format!("a rack holds at most {MAX_SLOTS} effects")));
        }
        let pos = pos.unwrap_or(rack.len()).min(rack.len());
        let cfg = SlotCfg::new(k);
        rack.insert(pos, cfg.clone());
        // built (allocated) here, on the control side, then moved into the audio thread
        // a fresh slot has no model file to load, so this can't fail
        let slot = Slot::new(cfg, self.samplerate()).expect("plain slot");
        self.cmd(Cmd::RackInsert(t, pos, slot));
        Ok(pos)
    }

    fn rack_remove(&self, target: &str, pos: usize) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut c = self.inner.ctl.lock().unwrap();
        let rack = &mut mirror(&mut c, t).rack;
        if pos < rack.len() {
            rack.remove(pos);
            self.cmd(Cmd::RackRemove(t, pos));
        }
        Ok(())
    }

    fn rack_move(&self, target: &str, from: usize, to: usize) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut c = self.inner.ctl.lock().unwrap();
        let rack = &mut mirror(&mut c, t).rack;
        if from < rack.len() {
            let s = rack.remove(from);
            let to = to.min(rack.len());
            rack.insert(to, s);
            self.cmd(Cmd::RackMove(t, from, to));
        }
        Ok(())
    }

    /// Slot setting: "on" or one of the effect's parameter names (0..1).
    fn rack_set(&self, target: &str, pos: usize, key: &str, value: f64) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut c = self.inner.ctl.lock().unwrap();
        let Some(slot) = mirror(&mut c, t).rack.get_mut(pos) else {
            return Err(PyValueError::new_err(format!("no slot {pos}")));
        };
        if key == "on" {
            slot.on = value != 0.0;
            self.cmd(Cmd::RackOn(t, pos, slot.on));
        } else {
            let k = param_index(slot.kind, key)
                .ok_or_else(|| PyValueError::new_err(format!("{} has no {key}", FX[slot.kind].name)))?;
            slot.p[k] = value as f32;
            self.cmd(Cmd::RackParam(t, pos, k, value as f32));
        }
        Ok(())
    }

    /// Replace a strip's whole rack with a preset's effects.
    fn rack_preset(&self, target: &str, name: &str) -> PyResult<()> {
        let t = parse_target(target)?;
        let slots = preset_slots(name).ok_or_else(|| PyValueError::new_err(format!("no preset {name}")))?;
        self.load_rack(t, slots);
        Ok(())
    }

    /// Replace a strip's rack with explicit slots: [{"kind", "on", <param>: value}].
    fn rack_load(&self, target: &str, slots: Vec<Bound<'_, PyDict>>) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut cfgs = Vec::new();
        for d in slots.iter().take(MAX_SLOTS) {
            let kind: String = d.get_item("kind")?.ok_or_else(|| PyValueError::new_err("slot without kind"))?.extract()?;
            let Some(k) = kind_index(&kind) else { continue };
            let mut cfg = SlotCfg::new(k);
            if let Some(on) = d.get_item("on")? {
                cfg.on = on.extract()?;
            }
            for (i, name) in FX[k].params.iter().enumerate() {
                if let Some(v) = d.get_item(*name)? {
                    cfg.p[i] = v.extract::<f32>()?;
                }
            }
            if let Some(m) = d.get_item("model")? {
                cfg.model = m.extract::<Option<String>>()?;
            }
            if let Some(v) = d.get_item("vst")? {
                if let Ok(v) = v.downcast::<PyDict>() {
                    let get = |k: &str| -> PyResult<Option<String>> {
                        Ok(match v.get_item(k)? { Some(x) => x.extract::<Option<String>>()?, None => None })
                    };
                    cfg.vst = Some(fx::VstRef {
                        path: get("path")?.unwrap_or_default(),
                        cid: get("cid")?.unwrap_or_default(),
                        name: get("name")?.unwrap_or_default(),
                        state: get("state")?,
                        ..Default::default()
                    });
                }
            }
            cfgs.push(cfg);
        }
        self.load_rack(t, cfgs);
        Ok(())
    }

    /// Load a .nam file into a NAM slot (None empties it). Raises with a
    /// readable message if the file can't be used; the slot is left as it was.
    #[pyo3(signature = (target, pos, path))]
    fn rack_set_model(&self, py: Python<'_>, target: &str, pos: usize, path: Option<String>) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut cfg = {
            let mut c = self.inner.ctl.lock().unwrap();
            let Some(slot) = mirror(&mut c, t).rack.get_mut(pos) else {
                return Err(PyValueError::new_err(format!("no slot {pos}")));
            };
            if slot.kind != fx::NAM {
                return Err(PyValueError::new_err("that slot isn't a NAM amp"));
            }
            slot.clone()
        };
        cfg.model = path;
        cfg.info = None;
        cfg.error = None;
        let sr = self.samplerate();
        // parsing and building a model takes a moment: don't hold the GIL for it
        let slot = py.allow_threads(|| Slot::new(cfg, sr)).map_err(PyRuntimeError::new_err)?;
        let mut c = self.inner.ctl.lock().unwrap();
        if let Some(m) = mirror(&mut c, t).rack.get_mut(pos) {
            let (on, p) = (m.on, m.p); // knobs may have moved while it loaded
            *m = slot.cfg.clone();
            m.on = on;
            m.p = p;
        }
        drop(c);
        self.cmd(Cmd::RackReplace(t, pos, slot));
        // the fresh slot carries the on/params from when loading started; resend the current ones
        let m = self.inner.ctl.lock().unwrap().clone_slot(t, pos);
        if let Some(m) = m {
            self.cmd(Cmd::RackOn(t, pos, m.on));
            for (k, v) in m.p.iter().enumerate() {
                self.cmd(Cmd::RackParam(t, pos, k, *v));
            }
        }
        Ok(())
    }

    // --- instruments and notes --------------------------------------------------

    /// Make a strip play an instrument instead of its input: kind "drums",
    /// "synth" or "vst" (with vst = (path, cid, name)); None goes back to the
    /// input. `cfg` restores saved settings ({param: value, "pads": [...],
    /// "vst": {"state": ...}}).
    #[pyo3(signature = (target, kind, vst=None, cfg=None))]
    fn inst_set(&self, py: Python<'_>, target: &str, kind: Option<&str>, vst: Option<(String, String, String)>,
                cfg: Option<Bound<'_, PyDict>>) -> PyResult<()> {
        let t = parse_target(target)?;
        let Some(kind) = kind.filter(|k| !k.is_empty()) else {
            mirror(&mut self.inner.ctl.lock().unwrap(), t).inst = None;
            self.cmd(Cmd::Instrument(t, None));
            return Ok(());
        };
        let k = instr::kind_index(kind).ok_or_else(|| PyValueError::new_err(format!("no instrument {kind}")))?;
        let mut c = InstCfg::new(k);
        if let Some((path, cid, name)) = vst {
            c.vst = Some(fx::VstRef { path, cid, name, ..Default::default() });
        }
        if let Some(d) = cfg {
            for (i, name) in instr::PARAMS[k].iter().enumerate() {
                if let Some(v) = d.get_item(*name)? {
                    c.p[i] = v.extract::<f32>()?;
                }
            }
            if let Some(p) = d.get_item("pads")? {
                let pads: Vec<Option<String>> = p.extract()?;
                for (i, x) in pads.into_iter().take(c.pads.len()).enumerate() {
                    c.pads[i] = x;
                }
            }
            if let Some(v) = d.get_item("vst")? {
                if let Ok(v) = v.downcast::<PyDict>() {
                    let get = |k: &str| -> PyResult<Option<String>> {
                        Ok(match v.get_item(k)? { Some(x) => x.extract::<Option<String>>()?, None => None })
                    };
                    if let (Some(path), Some(cid)) = (get("path")?, get("cid")?) {
                        c.vst = Some(fx::VstRef { path, cid, name: get("name")?.unwrap_or_default(), state: get("state")?, ..Default::default() });
                    }
                }
            }
        }
        let sr = self.samplerate();
        let slot = py.allow_threads(|| InstSlot::new(c, sr)).map_err(PyRuntimeError::new_err)?;
        mirror(&mut self.inner.ctl.lock().unwrap(), t).inst = Some(slot.cfg.clone());
        self.cmd(Cmd::Instrument(t, Some(slot)));
        Ok(())
    }

    /// One of an instrument's knobs (0..1).
    fn inst_param(&self, target: &str, key: &str, value: f32) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut c = self.inner.ctl.lock().unwrap();
        let Some(i) = mirror(&mut c, t).inst.as_mut() else { return Err(PyValueError::new_err("no instrument there")) };
        let k = instr::PARAMS[i.kind].iter().position(|p| *p == key)
            .ok_or_else(|| PyValueError::new_err(format!("the {} has no {key}", instr::KINDS[i.kind])))?;
        i.p[k] = value;
        drop(c);
        self.cmd(Cmd::InstParam(t, k, value));
        Ok(())
    }

    /// Put your own sample on a drum pad (a WAV path), or None for the built-in sound.
    #[pyo3(signature = (target, pad, path=None))]
    fn inst_pad(&self, py: Python<'_>, target: &str, pad: usize, path: Option<String>) -> PyResult<()> {
        let t = parse_target(target)?;
        let mut cfg = {
            let mut c = self.inner.ctl.lock().unwrap();
            match mirror(&mut c, t).inst.as_ref() {
                Some(i) if i.kind == instr::DRUMS && pad < i.pads.len() => i.clone(),
                _ => return Err(PyValueError::new_err("not a drum pad")),
            }
        };
        cfg.pads[pad] = path;
        let sr = self.samplerate();
        let slot = py.allow_threads(|| InstSlot::new(cfg, sr)).map_err(PyRuntimeError::new_err)?;
        if let Some(e) = &slot.cfg.error {
            return Err(PyRuntimeError::new_err(e.clone()));
        }
        mirror(&mut self.inner.ctl.lock().unwrap(), t).inst = Some(slot.cfg.clone());
        self.cmd(Cmd::Instrument(t, Some(slot)));
        Ok(())
    }

    /// Play a note on a strip's instrument (the computer keyboard, a pad click).
    #[pyo3(signature = (target, on, key, vel=100))]
    fn note(&self, target: &str, on: bool, key: u8, vel: u8) -> PyResult<()> {
        let t = parse_target(target)?;
        self.inner.note(t, if on { NoteEv::On(key & 127, vel.clamp(1, 127)) } else { NoteEv::Off(key & 127) });
        Ok(())
    }

    /// The MIDI keyboards there are.
    fn midi_inputs(&self, py: Python<'_>) -> Vec<String> {
        py.allow_threads(|| self.midi.list())
    }

    /// Listen to this MIDI keyboard (None: none).
    #[pyo3(signature = (name=None))]
    fn midi_open(&self, py: Python<'_>, name: Option<String>) -> PyResult<()> {
        py.allow_threads(|| self.midi.open(name)).map_err(PyRuntimeError::new_err)
    }

    /// Which strip the MIDI keyboard plays (None: none). The old one gets an
    /// all-notes-off, so nothing hangs.
    #[pyo3(signature = (target=None))]
    fn midi_target(&self, target: Option<&str>) -> PyResult<()> {
        let new = match target {
            Some(t) => encode_target(parse_target(t)?),
            None => u64::MAX,
        };
        let old = self.inner.midi_target.swap(new, Relaxed);
        if old != new {
            if let Some(t) = decode_target(old) {
                self.inner.note(t, NoteEv::AllOff);
            }
        }
        Ok(())
    }

    // --- VST3 ----------------------------------------------------------------

    /// The usual VST3 folders on this system.
    #[staticmethod]
    fn vst_dirs() -> Vec<String> {
        vst::default_dirs().into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }

    /// Every .vst3 under these folders.
    #[staticmethod]
    fn vst_find(dirs: Vec<String>) -> Vec<String> {
        let dirs: Vec<std::path::PathBuf> = dirs.into_iter().map(Into::into).collect();
        vst::find_plugins(&dirs).into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
    }

    /// The effect classes inside one .vst3: [{"cid", "name", "vendor", "sub", "instrument"}].
    #[staticmethod]
    fn vst_scan<'py>(py: Python<'py>, path: String) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let classes = py.allow_threads(|| vst::scan(std::path::Path::new(&path))).map_err(PyRuntimeError::new_err)?;
        classes.iter().map(|c| {
            let d = PyDict::new(py);
            d.set_item("cid", &c.cid)?;
            d.set_item("name", &c.name)?;
            d.set_item("vendor", &c.vendor)?;
            d.set_item("sub", &c.sub)?;
            d.set_item("instrument", c.is_instrument())?;
            Ok(d)
        }).collect()
    }

    /// Put a VST3 plugin into a rack. Loads and starts it (this can take a moment).
    #[pyo3(signature = (target, path, cid, name, pos=None))]
    fn rack_add_vst(&self, py: Python<'_>, target: &str, path: String, cid: String, name: String, pos: Option<usize>) -> PyResult<usize> {
        let t = parse_target(target)?;
        if mirror(&mut self.inner.ctl.lock().unwrap(), t).rack.len() >= MAX_SLOTS {
            return Err(PyValueError::new_err(format!("a rack holds at most {MAX_SLOTS} effects")));
        }
        let mut cfg = SlotCfg::new(fx::VST);
        cfg.vst = Some(fx::VstRef { path, cid, name, ..Default::default() });
        let sr = self.samplerate();
        let slot = py.allow_threads(|| Slot::new(cfg, sr)).map_err(PyRuntimeError::new_err)?;
        let mut c = self.inner.ctl.lock().unwrap();
        let rack = &mut mirror(&mut c, t).rack;
        let pos = pos.unwrap_or(rack.len()).min(rack.len());
        rack.insert(pos, slot.cfg.clone());
        drop(c);
        self.cmd(Cmd::RackInsert(t, pos, slot));
        Ok(pos)
    }

    /// Show a plugin's own window: rack slot `pos`, or (pos < 0) the strip's instrument.
    fn vst_open_editor(&self, py: Python<'_>, target: &str, pos: i64) -> PyResult<()> {
        let t = parse_target(target)?;
        let (id, title) = {
            let mut c = self.inner.ctl.lock().unwrap();
            let m = mirror(&mut c, t);
            let s = if pos < 0 {
                m.inst.as_ref().and_then(|i| i.vst.clone())
            } else {
                m.rack.get(pos as usize).and_then(|s| s.vst.clone())
            };
            let s = s.ok_or_else(|| PyValueError::new_err("not a VST slot"))?;
            (s.id, format!("{} · Duet", s.name))
        };
        let inst = vst::instance(id).ok_or_else(|| PyRuntimeError::new_err("that plugin isn't running"))?;
        py.allow_threads(|| inst.show_editor(title)).map_err(PyRuntimeError::new_err)
    }

    /// Start/stop/change the metronome as the leader. Returns the epoch (our block
    /// number where beat 0 falls) to share with the partner.
    fn metro_lead(&self, on: bool, bpm: f32, beats: u32) -> i64 {
        // a couple of blocks ahead so beat 0 isn't already half gone when it lands
        let epoch = self.inner.shared.seq.load(Relaxed) as i64 + 2;
        self.cmd(Cmd::Metro { on, bpm, beats, leader: true, epoch });
        epoch
    }

    /// Follow the partner's metronome (epoch on their timeline).
    fn metro_follow(&self, on: bool, bpm: f32, beats: u32, epoch: i64) {
        self.cmd(Cmd::Metro { on, bpm, beats, leader: false, epoch });
    }

    fn metro_volume(&self, volume: f32) {
        self.cmd(Cmd::MetroVolume(volume));
    }

    fn set_master(&self, value: f64) {
        self.inner.ctl.lock().unwrap().master = value as f32;
        self.cmd(Cmd::Master(value as f32));
    }

    fn set_buffer(&self, blocks: f64) {
        self.cmd(Cmd::JitterTarget(blocks.round().clamp(1.0, 64.0) as i64));
    }

    /// Let the engine pick the safety buffer: up on a dropout, down after a calm
    /// stretch. A manual set_buffer switches it off again.
    fn set_buffer_auto(&self, on: bool) {
        self.cmd(Cmd::JitterAuto(on));
    }

    /// The whole mixer as plain data, for the UI. With states=True, plugin
    /// states are read fresh first (for saving; asks every plugin, so not for
    /// every repaint).
    #[pyo3(signature = (states=false))]
    fn mixer<'py>(&self, py: Python<'py>, states: bool) -> PyResult<Bound<'py, PyDict>> {
        if states {
            let ids: Vec<(Target, usize, u64)> = {
                let mut c = self.inner.ctl.lock().unwrap();
                let mut out = Vec::new();
                for (side, n) in [(0, MAX_LOCAL), (1, MAX_REMOTE), (2, MAX_TRACKS)] {
                    for i in 0..n {
                        let t = match side { 0 => Target::Local(i), 1 => Target::Remote(i), _ => Target::Track(i) };
                        let m = mirror(&mut c, t);
                        for (pos, s) in m.rack.iter().enumerate() {
                            if let Some(v) = &s.vst {
                                out.push((t, pos, v.id));
                            }
                        }
                        if let Some(v) = m.inst.as_ref().and_then(|i| i.vst.as_ref()) {
                            out.push((t, usize::MAX, v.id)); // MAX: the instrument, not a rack slot
                        }
                    }
                }
                out
            };
            let fresh: Vec<(Target, usize, Option<String>)> = py.allow_threads(|| {
                ids.into_iter()
                    .map(|(t, pos, id)| (t, pos, vst::instance(id).and_then(|i| i.state()).map(|s| s.encode())))
                    .collect()
            });
            let mut c = self.inner.ctl.lock().unwrap();
            for (t, pos, st) in fresh {
                let m = mirror(&mut c, t);
                let v = if pos == usize::MAX {
                    m.inst.as_mut().and_then(|i| i.vst.as_mut())
                } else {
                    m.rack.get_mut(pos).and_then(|s| s.vst.as_mut())
                };
                if let (Some(st), Some(v)) = (st, v) {
                    v.state = Some(st);
                }
            }
        }
        let c = self.inner.ctl.lock().unwrap();
        let strip = |m: &StripMirror| -> PyResult<Bound<'py, PyDict>> {
            let d = PyDict::new(py);
            let s = &m.s;
            d.set_item("active", s.active)?;
            d.set_item("input", s.input)?;
            d.set_item("gain", s.gain)?;
            d.set_item("pan", s.pan)?;
            d.set_item("fader", s.fader)?;
            d.set_item("mute", s.mute)?;
            d.set_item("solo", s.solo)?;
            d.set_item("send", s.send)?;
            d.set_item("mon", s.mon)?;
            let rack = m.rack.iter().map(|slot| -> PyResult<Bound<'py, PyDict>> {
                let e = PyDict::new(py);
                e.set_item("kind", FX[slot.kind].name)?;
                e.set_item("on", slot.on)?;
                for (i, name) in FX[slot.kind].params.iter().enumerate() {
                    e.set_item(*name, slot.p[i])?;
                }
                if slot.kind == fx::NAM {
                    e.set_item("model", slot.model.clone())?;
                    e.set_item("model_name", slot.info.as_ref().map(|i| i.name.clone()))?;
                    e.set_item("model_sr", slot.info.as_ref().map(|i| i.sample_rate))?;
                    e.set_item("includes_cab", slot.info.as_ref().and_then(|i| i.includes_cab))?;
                    e.set_item("error", slot.error.clone())?;
                }
                if let Some(v) = &slot.vst {
                    let d = PyDict::new(py);
                    d.set_item("path", &v.path)?;
                    d.set_item("cid", &v.cid)?;
                    d.set_item("name", &v.name)?;
                    d.set_item("state", v.state.clone())?;
                    d.set_item("latency", v.latency)?;
                    d.set_item("has_editor", v.has_editor)?;
                    e.set_item("vst", d)?;
                    e.set_item("error", slot.error.clone())?;
                }
                Ok(e)
            }).collect::<PyResult<Vec<_>>>()?;
            d.set_item("rack", rack)?;
            let inst = match &m.inst {
                None => None,
                Some(i) => {
                    let e = PyDict::new(py);
                    e.set_item("kind", instr::KINDS[i.kind])?;
                    for (k, name) in instr::PARAMS[i.kind].iter().enumerate() {
                        e.set_item(*name, i.p[k])?;
                    }
                    if i.kind == instr::DRUMS {
                        e.set_item("pads", i.pads.clone())?;
                        e.set_item("pad_names", instr::PADS.iter().map(|(n, name)| (*n, *name)).collect::<Vec<_>>())?;
                    }
                    if let Some(v) = &i.vst {
                        let d = PyDict::new(py);
                        d.set_item("path", &v.path)?;
                        d.set_item("cid", &v.cid)?;
                        d.set_item("name", &v.name)?;
                        d.set_item("state", v.state.clone())?;
                        d.set_item("has_editor", v.has_editor)?;
                        e.set_item("vst", d)?;
                    }
                    e.set_item("error", i.error.clone())?;
                    Some(e)
                }
            };
            d.set_item("inst", inst)?;
            Ok(d)
        };
        let d = PyDict::new(py);
        d.set_item("local", c.local.iter().map(strip).collect::<PyResult<Vec<_>>>()?)?;
        d.set_item("remote", c.remote.iter().map(strip).collect::<PyResult<Vec<_>>>()?)?;
        d.set_item("tracks", c.tracks.iter().map(strip).collect::<PyResult<Vec<_>>>()?)?;
        d.set_item("master", c.master)?;
        Ok(d)
    }

    /// [{"name", "params": [...], "defaults": [...]}] for every effect type.
    #[staticmethod]
    fn effects<'py>(py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        FX.iter().map(|f| {
            let d = PyDict::new(py);
            d.set_item("name", f.name)?;
            d.set_item("params", f.params.to_vec())?;
            d.set_item("defaults", f.defaults[..f.params.len()].to_vec())?;
            Ok(d)
        }).collect()
    }

    #[staticmethod]
    fn presets() -> Vec<&'static str> {
        fx::PRESETS.iter().map(|(n, _)| *n).collect()
    }

    // --- signaling -----------------------------------------------------------------

    fn send_signal(&self, text: &str) {
        self.inner.ctl.lock().unwrap().signals.queue(text);
        let due = self.inner.ctl.lock().unwrap().signals.due();
        for p in due {
            self.inner.send(&p, None);
        }
    }

    fn drain_signals(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.ctl.lock().unwrap().signals.inbox)
    }

    /// Test hook: drop this share (0..1) of outgoing signal packets and acks.
    fn set_test_signal_loss(&self, share: f32) {
        self.inner.shared.test_signal_loss.set(share);
    }

    #[getter]
    fn signals_pending(&self) -> usize {
        self.inner.ctl.lock().unwrap().signals.pending()
    }

    // --- DAW: audio files, timeline, transport, recording ---------------------------
    // Positions are seconds; the engine turns them into samples at the device rate.

    /// Load a WAV for the timeline under `key`. Returns (frames, sample rate,
    /// waveform peaks: a signed min/max byte pair per 512 frames).
    fn daw_load<'py>(&self, py: Python<'py>, key: String, path: String) -> PyResult<(usize, u32, Bound<'py, PyBytes>)> {
        let data = py.allow_threads(|| daw::read_wav(std::path::Path::new(&path))).map_err(PyRuntimeError::new_err)?;
        let (frames, sr, peaks) = (data.samples.len(), data.sr, daw::peaks(&data.samples));
        self.inner.daw.lock().unwrap().cache.insert(&key, data);
        Ok((frames, sr, PyBytes::new(py, &peaks)))
    }

    fn daw_loaded(&self, key: &str) -> bool {
        self.inner.daw.lock().unwrap().cache.get(key).is_some()
    }

    fn daw_unload(&self, key: &str) {
        self.inner.daw.lock().unwrap().cache.remove(key);
    }

    /// The whole clip layout: tracks[i] = [(key, start, offset, length, gain,
    /// fade in, fade out)] for track strip Ti, all times in seconds. Clips whose
    /// audio isn't loaded yet are skipped (send the layout again once it is).
    /// notes[i] = [(start, duration, key, velocity)] for Ti's instrument.
    #[pyo3(signature = (tracks, notes=None))]
    fn daw_timeline(&self, py: Python<'_>, tracks: Vec<Vec<(String, f64, f64, f64, f32, f64, f64)>>,
                    notes: Option<Vec<Vec<(f64, f64, u8, u8)>>>) {
        let notes: Vec<Vec<NoteDesc>> = notes
            .unwrap_or_default()
            .into_iter()
            .take(MAX_TRACKS)
            .map(|ns| ns.into_iter().map(|(t, dur, key, vel)| NoteDesc { t, dur, key, vel }).collect())
            .collect();
        self.inner.daw.lock().unwrap().notes = notes;
        let desc = tracks
            .into_iter()
            .take(MAX_TRACKS)
            .map(|clips| {
                clips
                    .into_iter()
                    .map(|(key, start, offset, len, gain, fade_in, fade_out)| ClipDesc { key, start, offset, len, gain, fade_in, fade_out })
                    .collect()
            })
            .collect();
        self.inner.daw.lock().unwrap().desc = desc;
        py.allow_threads(|| self.rebuild_timeline());
    }

    /// Play from `pos`. The click plays at least until `click_until` (count-in).
    #[pyo3(signature = (pos, click_until=None))]
    fn daw_play(&self, pos: f64, click_until: Option<f64>) {
        let s = |x: f64| (x * self.samplerate() as f64).round() as i64;
        self.cmd(Cmd::Play { pos: s(pos), click_until: click_until.map_or(i64::MIN, s), rec: None });
    }

    /// Play from `pos` and record: sources = [(local strip, key, wav path)].
    /// Each strip is recorded after its effects (what the partner hears).
    #[pyo3(signature = (sources, pos, click_until=None))]
    fn daw_record(&self, sources: Vec<(usize, String, String)>, pos: f64, click_until: Option<f64>) -> PyResult<()> {
        if self.inner.daw.lock().unwrap().rec.is_some() {
            return Err(PyRuntimeError::new_err("already recording"));
        }
        let sr = self.samplerate() as u32;
        let mut rings = Box::new(RecRings(Default::default()));
        let mut takes = Vec::new();
        for (src, key, path) in sources {
            if src >= MAX_LOCAL || rings.0[src].is_some() {
                continue;
            }
            let (tx, rx) = rtrb::RingBuffer::<f32>::new(sr as usize * 4);
            rings.0[src] = Some(tx);
            takes.push((src, key, std::path::PathBuf::from(path), rx));
        }
        let sh = &self.inner.shared;
        let comp = (sh.lat_in_us.load(Relaxed) + sh.lat_out_us.load(Relaxed)) as f64 / 1e6
            + sh.cushion_frames.load(Relaxed) as f64 / sr as f64;
        let writer = std::thread::Builder::new()
            .name("duet-takes".into())
            .spawn(move || daw::write_takes(takes, sr))
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        self.inner.daw.lock().unwrap().rec = Some(RecSession { writer, sr, comp });
        let s = |x: f64| (x * sr as f64).round() as i64;
        self.cmd(Cmd::Play { pos: s(pos), click_until: click_until.map_or(i64::MIN, s), rec: Some(rings) });
        Ok(())
    }

    /// Stop the transport. If it was recording, returns the takes: [{"src",
    /// "key", "path", "frames", "sr", "first" (timeline seconds of the first
    /// sample as recorded), "comp" (seconds the sound cards add: the take
    /// belongs that much earlier), "peaks", "dropped", "error"}].
    fn daw_stop<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.cmd(Cmd::Stop);
        let Some(sess) = self.inner.daw.lock().unwrap().rec.take() else { return Ok(vec![]) };
        // the writer finishes once the audio thread has let go of the rings
        let until = Instant::now() + Duration::from_secs(3);
        while !sess.writer.is_finished() && Instant::now() < until {
            if !self.inner.shared.streaming.load(Relaxed) {
                if let Ok(mut d) = self.dsp.try_lock() {
                    d.apply_pending();
                }
            }
            py.allow_threads(|| std::thread::sleep(Duration::from_millis(5)));
        }
        if !sess.writer.is_finished() {
            return Err(PyRuntimeError::new_err("the recording didn't stop (is the sound card still running?)"));
        }
        let outs = py.allow_threads(|| sess.writer.join()).map_err(|_| PyRuntimeError::new_err("take writer crashed"))?;
        self.inner.free_retired();
        let sh = &self.inner.shared;
        let first = if sh.rec_started.load(Relaxed) { sh.rec_first.load(Relaxed) as f64 / sess.sr as f64 } else { 0.0 };
        let dropped = sh.rec_dropped.load(Relaxed);
        let mut res = Vec::new();
        for o in outs {
            let d = PyDict::new(py);
            d.set_item("src", o.src)?;
            d.set_item("key", &o.key)?;
            d.set_item("path", o.path.to_string_lossy().into_owned())?;
            d.set_item("frames", o.samples.len())?;
            d.set_item("sr", sess.sr)?;
            d.set_item("first", first)?;
            d.set_item("comp", sess.comp)?;
            d.set_item("peaks", PyBytes::new(py, &daw::peaks(&o.samples)))?;
            d.set_item("dropped", dropped)?;
            d.set_item("error", o.error.clone())?;
            if o.error.is_none() {
                self.inner.daw.lock().unwrap().cache.insert(&o.key, daw::AudioData { sr: sess.sr, samples: o.samples });
            }
            res.push(d);
        }
        Ok(res)
    }

    /// Notes played on song tracks while recording (since last asked):
    /// [(track slot, "on" / "off" / "pedal down" / "pedal up", key, velocity, timeline seconds)].
    fn daw_rec_notes(&self) -> Vec<(usize, &'static str, u8, u8, f64)> {
        let sr = self.samplerate() as f64;
        let mut r = self.inner.noterec.lock().unwrap();
        let mut out = Vec::new();
        while let Ok((t, ev, pos)) = r.pop() {
            let (kind, key, vel) = match ev {
                NoteEv::On(k, v) => ("on", k, v),
                NoteEv::Off(k) => ("off", k, 0),
                NoteEv::Sustain(true) => ("pedal down", 0, 0),
                NoteEv::Sustain(false) => ("pedal up", 0, 0),
                _ => continue,
            };
            out.push((t as usize, kind, key, vel, pos as f64 / sr));
        }
        out
    }

    fn daw_locate(&self, pos: f64) {
        self.cmd(Cmd::Locate((pos * self.samplerate() as f64).round() as i64));
    }

    fn daw_loop(&self, on: bool, start: f64, end: f64) {
        let sr = self.samplerate() as f64;
        self.cmd(Cmd::Loop { on, start: (start * sr).round() as i64, end: (end * sr).round() as i64 });
    }

    /// The click while the timeline plays.
    fn daw_click(&self, on: bool, bpm: f32, beats: u32) {
        self.cmd(Cmd::Click { on, bpm, beats });
    }

    /// Render the song (seconds start..end) to WAV files in the background:
    /// the mix to `mix` (if given) and stems = [(track slot, path)]. Uses the
    /// song tracks' current faders, pans, mute/solo and effects.
    #[pyo3(signature = (mix, stems, start, end))]
    fn daw_export(&self, mix: Option<String>, stems: Vec<(usize, String)>, start: f64, end: f64) -> PyResult<()> {
        let sr = self.samplerate() as u32;
        let (tracks, master) = {
            let c = self.inner.ctl.lock().unwrap();
            let tracks: Vec<daw::TrackMix> = c
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, m)| m.s.active)
                .map(|(slot, m)| daw::TrackMix {
                    slot,
                    s: m.s,
                    rack: m.rack.clone(),
                    inst: m.inst.clone(),
                    stem: stems.iter().find(|(s, _)| *s == slot).map(|(_, p)| p.into()),
                })
                .collect();
            (tracks, c.master)
        };
        let mut d = self.inner.daw.lock().unwrap();
        if d.export.as_ref().is_some_and(|j| !j.worker.is_finished()) {
            return Err(PyRuntimeError::new_err("an export is already running"));
        }
        let tl = {
            let DawCtl { cache, desc, notes, .. } = &mut *d;
            cache.build(desc, notes, sr)
        };
        let s = |x: f64| (x * sr as f64).round() as i64;
        let (a, b) = (s(start), s(end).max(s(start)));
        let done = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let done2 = done.clone();
        let mix = mix.map(std::path::PathBuf::from);
        let worker = std::thread::Builder::new()
            .name("duet-export".into())
            .spawn(move || daw::render(&tl, tracks, master, sr, a, b, mix, &done2))
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        d.export = Some(ExportJob { done, total: (b - a) as u64, worker });
        Ok(())
    }

    /// {"running", "progress" (0..1), "result": None while running / when
    /// there was none, else {"ok", "warnings" | "error"}}.
    fn daw_export_status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let mut d = self.inner.daw.lock().unwrap();
        let out = PyDict::new(py);
        let (running, progress) = match &d.export {
            Some(j) => (!j.worker.is_finished(), j.done.load(Relaxed) as f64 / j.total.max(1) as f64),
            None => (false, 0.0),
        };
        out.set_item("running", running)?;
        out.set_item("progress", progress.min(1.0))?;
        if !running {
            if let Some(j) = d.export.take() {
                let r = PyDict::new(py);
                match j.worker.join() {
                    Ok(Ok(warnings)) => {
                        r.set_item("ok", true)?;
                        r.set_item("warnings", warnings)?;
                    }
                    Ok(Err(e)) => {
                        r.set_item("ok", false)?;
                        r.set_item("error", e)?;
                    }
                    Err(_) => {
                        r.set_item("ok", false)?;
                        r.set_item("error", "the export crashed")?;
                    }
                }
                out.set_item("result", r)?;
                return Ok(out);
            }
        }
        out.set_item("result", py.None())?;
        Ok(out)
    }

    // --- files to and from the partner ---------------------------------------------

    /// Where received files are saved.
    fn file_dir(&self, path: String) {
        self.inner.files.lock().unwrap().set_dir(path.into());
    }

    /// Send a file to the partner under `name` (a plain file name).
    fn file_send(&self, py: Python<'_>, name: String, path: String) -> PyResult<()> {
        let data = py.allow_threads(|| transfer::read_file(std::path::Path::new(&path))).map_err(PyRuntimeError::new_err)?;
        self.inner.files.lock().unwrap().send(&name, data);
        Ok(())
    }

    /// {"out": [(name, bytes done, total)], "in": [...], "rate": bytes/s}
    fn file_status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let f = self.inner.files.lock().unwrap();
        let (out, inc) = f.progress();
        let d = PyDict::new(py);
        d.set_item("out", out.into_iter().map(|p| (p.name, p.done, p.total)).collect::<Vec<_>>())?;
        d.set_item("in", inc.into_iter().map(|p| (p.name, p.done, p.total)).collect::<Vec<_>>())?;
        d.set_item("rate", f.rate())?;
        Ok(d)
    }

    /// What finished since last time: (received [(name, path)], sent [name], failed [name]).
    #[allow(clippy::type_complexity)]
    fn file_drain(&self) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
        let mut f = self.inner.files.lock().unwrap();
        (
            std::mem::take(&mut f.received).into_iter().map(|(n, p)| (n, p.to_string_lossy().into_owned())).collect(),
            std::mem::take(&mut f.sent),
            std::mem::take(&mut f.failed),
        )
    }

    // --- status --------------------------------------------------------------------

    #[getter]
    fn local_port(&self) -> u16 {
        self.local.port()
    }

    #[getter]
    fn public_addr(&self) -> Option<(String, u16)> {
        self.public.lock().unwrap().map(|a| (a.ip().to_string(), a.port()))
    }

    #[getter]
    fn connected(&self) -> bool {
        self.inner.connected()
    }

    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.inner.free_retired();
        let i = &self.inner;
        let s = &i.shared;
        let connected = i.connected();
        let (rtt, warning, calling) = {
            let c = i.ctl.lock().unwrap();
            (c.rtt_ms, c.warning.clone(), !c.candidates.is_empty())
        };
        let d = PyDict::new(py);
        d.set_item("connected", connected)?;
        d.set_item("calling", calling && !connected)?;
        d.set_item("peer", if connected { i.peer().map(|a| a.ip().to_string()) } else { None })?;
        d.set_item("rtt", if connected { rtt } else { None })?;
        d.set_item("warning", warning)?;
        d.set_item("samplerate", s.samplerate.load(Relaxed))?;
        d.set_item("frame", s.frame.load(Relaxed))?;
        d.set_item("jitter_target", s.jitter_target.load(Relaxed))?;
        d.set_item("jitter_auto", s.jitter_auto.load(Relaxed))?;
        d.set_item("local_levels", s.local_levels.iter().map(|l| l.get()).collect::<Vec<_>>())?;
        d.set_item("remote_levels", s.remote_levels.iter().map(|l| l.get()).collect::<Vec<_>>())?;
        d.set_item("master_level", s.master_level.get())?;
        d.set_item("remote_channels", s.remote_channels.load(Relaxed))?;
        d.set_item("sent_channels", s.sent_channels.load(Relaxed))?;
        d.set_item("in_channels", s.in_channels.load(Relaxed))?;
        d.set_item("peer_protocol", s.peer_protocol.load(Relaxed))?;
        let cushion_ms = s.cushion_frames.load(Relaxed) as f64 * 1000.0 / s.samplerate.load(Relaxed).max(1) as f64;
        d.set_item("lat_in_ms", s.lat_in_us.load(Relaxed) as f64 / 1000.0 + cushion_ms)?;
        d.set_item("lat_out_ms", s.lat_out_us.load(Relaxed) as f64 / 1000.0)?;
        d.set_item("cpu_us", s.cpu_ns.load(Relaxed) as f64 / 1000.0)?;
        d.set_item("raw_in_peak", s.raw_in_peak.get())?;
        d.set_item("metro_beat", s.metro_beat.load(Relaxed) as i64 - 1)?;
        d.set_item("stream_error", s.stream_error.load(Relaxed))?;
        d.set_item("metro_synced", s.metro_synced.load(Relaxed))?;
        d.set_item("in_callbacks", s.in_callbacks.load(Relaxed))?;
        d.set_item("ring_empty", s.ring_empty.load(Relaxed))?;
        let sr = s.samplerate.load(Relaxed).max(1) as f64;
        d.set_item("daw_pos", s.daw_pos.load(Relaxed) as f64 / sr)?;
        d.set_item("daw_playing", s.daw_playing.load(Relaxed))?;
        d.set_item("daw_recording", s.rec_on.load(Relaxed))?;
        d.set_item("rec_dropped", s.rec_dropped.load(Relaxed))?;
        d.set_item("midi_events", s.midi_events.load(Relaxed))?;
        d.set_item("track_levels", s.track_levels.iter().map(|l| l.get()).collect::<Vec<_>>())?;
        for (k, v) in [
            ("depth", &s.depth), ("underruns", &s.underruns), ("lost", &s.lost), ("late", &s.late),
            ("trimmed", &s.dropped), ("xruns", &s.xruns), ("packets_in", &s.packets_in), ("blocks", &s.blocks),
        ] {
            d.set_item(k, v.load(Relaxed))?;
        }
        Ok(d)
    }
}

impl Engine {
    fn samplerate(&self) -> f32 {
        self.inner.shared.samplerate.load(Relaxed) as f32
    }

    /// Build the timeline for the current rate from the last layout and hand it over.
    fn rebuild_timeline(&self) {
        let sr = self.samplerate() as u32;
        let tl = {
            let mut d = self.inner.daw.lock().unwrap();
            let DawCtl { cache, desc, notes, .. } = &mut *d;
            cache.build(desc, notes, sr)
        };
        self.cmd(Cmd::Timeline(Box::new(tl)));
    }

    /// Queue a command for the audio thread. With no stream running nothing
    /// would drain the queue (and it would fill up and drop commands), so then
    /// apply it right away.
    fn cmd(&self, c: Cmd) {
        self.inner.cmd(c);
        if !self.inner.shared.streaming.load(Relaxed) {
            if let Ok(mut d) = self.dsp.try_lock() {
                d.apply_pending();
            }
            self.inner.free_retired();
        }
    }

    fn load_rack(&self, t: Target, slots: Vec<SlotCfg>) {
        let sr = self.samplerate();
        let mut c = self.inner.ctl.lock().unwrap();
        let rack = &mut mirror(&mut c, t).rack;
        for _ in 0..rack.len() {
            self.cmd(Cmd::RackRemove(t, 0));
        }
        rack.clear();
        for (i, s) in slots.into_iter().take(MAX_SLOTS).enumerate() {
            let slot = match Slot::new(s.clone(), sr) {
                Ok(slot) => slot,
                Err(e) => {
                    let mut empty = s.clone();
                    empty.model = None;
                    let mut slot = Slot::new(empty, sr).expect("plain slot");
                    slot.cfg.model = s.model.clone(); // remembered, so it can be retried
                    slot.cfg.error = Some(e);
                    slot
                }
            };
            rack.push(slot.cfg.clone());
            self.cmd(Cmd::RackInsert(t, i, slot));
        }
    }
}

fn encode_target(t: Target) -> u64 {
    match t {
        Target::Local(i) => i as u64,
        Target::Remote(i) => 0x100 | i as u64,
        Target::Track(i) => 0x200 | i as u64,
    }
}

fn decode_target(v: u64) -> Option<Target> {
    let i = (v & 0xFF) as usize;
    match v >> 8 {
        0 if i < MAX_LOCAL => Some(Target::Local(i)),
        1 if i < MAX_REMOTE => Some(Target::Remote(i)),
        2 if i < MAX_TRACKS => Some(Target::Track(i)),
        _ => None,
    }
}

fn parse_target(s: &str) -> PyResult<Target> {
    let bad = || PyValueError::new_err(format!("strip {s:?}: use L0-L{}, R0-R{} or T0-T{}", MAX_LOCAL - 1, MAX_REMOTE - 1, MAX_TRACKS - 1));
    let (side, n) = s.split_at(1.min(s.len()));
    let i: usize = n.parse().map_err(|_| bad())?;
    match side {
        "L" if i < MAX_LOCAL => Ok(Target::Local(i)),
        "R" if i < MAX_REMOTE => Ok(Target::Remote(i)),
        "T" if i < MAX_TRACKS => Ok(Target::Track(i)),
        _ => Err(bad()),
    }
}

impl Ctl {
    fn clone_slot(&mut self, t: Target, pos: usize) -> Option<SlotCfg> {
        mirror(self, t).rack.get(pos).cloned()
    }
}

fn mirror(c: &mut Ctl, t: Target) -> &mut StripMirror {
    match t {
        Target::Local(i) => &mut c.local[i],
        Target::Remote(i) => &mut c.remote[i],
        Target::Track(i) => &mut c.tracks[i],
    }
}

#[cfg(windows)]
fn disable_udp_connreset(socket: &UdpSocket) {
    // Otherwise an ICMP "port unreachable" (partner not up yet) makes the next
    // recv fail with WSAECONNRESET. SIO_UDP_CONNRESET = _WSAIOW(IOC_VENDOR, 12).
    use std::os::windows::io::AsRawSocket;
    #[link(name = "ws2_32")]
    extern "system" {
        fn WSAIoctl(s: usize, code: u32, inb: *const u32, inlen: u32, outb: *mut u8, outlen: u32,
                    ret: *mut u32, ov: *mut u8, cr: *mut u8) -> i32;
    }
    let off: u32 = 0;
    let mut ret = 0u32;
    unsafe {
        WSAIoctl(socket.as_raw_socket() as usize, 0x9800_000C, &off, 4, std::ptr::null_mut(), 0,
                 &mut ret, std::ptr::null_mut(), std::ptr::null_mut());
    }
}

#[cfg(not(windows))]
fn disable_udp_connreset(_: &UdpSocket) {}

#[pymodule]
fn duet_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Engine>()?;
    Ok(())
}
