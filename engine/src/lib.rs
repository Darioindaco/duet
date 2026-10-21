//! duet_engine: the realtime half of Duet, as a Python extension.
//!
//! Python (duet.py + the web UI) does setup and display. Everything time-critical
//! runs here on native threads that never touch the GIL: the audio callback
//! (dsp.rs), the network receive thread, and the control thread (punching,
//! pings, signaling resends).

mod audio;
mod dsp;
mod fx;
mod jitter;
mod net;
mod signal;

use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use dsp::{decode_peer, encode_peer, Cmd, Dsp, InPacket, Shared, StripParam, StripSettings, Target, MAX_LOCAL, MAX_REMOTE};
use fx::{kind_index, param_index, preset_slots, Slot, SlotCfg, FX, MAX_SLOTS};

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
    master: f32,
}

#[derive(Clone)]
struct StripMirror {
    s: StripSettings,
    rack: Vec<SlotCfg>,
}

struct Inner {
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    ctl: Mutex<Ctl>,
    cmds: Mutex<rtrb::Producer<Cmd>>,
    incoming: Mutex<Option<rtrb::Producer<InPacket>>>,
    /// effect objects the audio thread is done with, freed here instead of there
    retired: Mutex<rtrb::Consumer<Box<Slot>>>,
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
}

#[pyclass]
struct Engine {
    inner: Arc<Inner>,
    dsp: Arc<Mutex<Dsp>>,
    audio: audio::AudioThread,
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
        let (ret_tx, ret_rx) = rtrb::RingBuffer::new(256);
        let dsp = Dsp::new(48000, 128, cmd_rx, pkt_rx, ret_tx, socket.clone(), shared.clone());
        let mut local: Vec<StripMirror> =
            (0..MAX_LOCAL).map(|i| StripMirror { s: StripSettings::local(i), rack: vec![] }).collect();
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
                remote: vec![StripMirror { s: StripSettings::remote(), rack: vec![] }; MAX_REMOTE],
                master: 1.0,
            }),
            cmds: Mutex::new(cmd_tx),
            incoming: Mutex::new(Some(pkt_tx)),
            retired: Mutex::new(ret_rx),
            running: AtomicBool::new(false),
            start: Instant::now(),
        });
        Ok(Engine { inner, dsp: Arc::new(Mutex::new(dsp)), audio: audio::AudioThread::spawn(), local: local_addr, public: Mutex::new(None) })
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

    /// ([inputs], [outputs], default input id, default output id); each device a dict.
    fn devices<'py>(&self, py: Python<'py>) -> PyResult<(Vec<Bound<'py, PyDict>>, Vec<Bound<'py, PyDict>>, Option<String>, Option<String>)> {
        let list = py.allow_threads(|| self.audio.list());
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

    /// The whole mixer as plain data, for the UI.
    fn mixer<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
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
                Ok(e)
            }).collect::<PyResult<Vec<_>>>()?;
            d.set_item("rack", rack)?;
            Ok(d)
        };
        let d = PyDict::new(py);
        d.set_item("local", c.local.iter().map(strip).collect::<PyResult<Vec<_>>>()?)?;
        d.set_item("remote", c.remote.iter().map(strip).collect::<PyResult<Vec<_>>>()?)?;
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
        d.set_item("metro_synced", s.metro_synced.load(Relaxed))?;
        d.set_item("in_callbacks", s.in_callbacks.load(Relaxed))?;
        d.set_item("ring_empty", s.ring_empty.load(Relaxed))?;
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

fn parse_target(s: &str) -> PyResult<Target> {
    let bad = || PyValueError::new_err(format!("strip {s:?}: use L0-L{} or R0-R{}", MAX_LOCAL - 1, MAX_REMOTE - 1));
    let (side, n) = s.split_at(1.min(s.len()));
    let i: usize = n.parse().map_err(|_| bad())?;
    match side {
        "L" if i < MAX_LOCAL => Ok(Target::Local(i)),
        "R" if i < MAX_REMOTE => Ok(Target::Remote(i)),
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
