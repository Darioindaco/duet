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

use dsp::{decode_peer, encode_peer, Cmd, Dsp, InPacket, Shared};
use fx::{fx_index, param_index, FxParams, FX};

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
    fx: FxParams, // mirror of what the audio thread has
}

struct Inner {
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    ctl: Mutex<Ctl>,
    cmds: Mutex<rtrb::Producer<Cmd>>,
    incoming: Mutex<Option<rtrb::Producer<InPacket>>>,
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
                net::T_AUDIO => {
                    let samples = (body.len() / 2).min(jitter::MAX_FRAME);
                    pkt.seq = seq;
                    pkt.len = samples as u16;
                    for i in 0..samples {
                        pkt.data[i] = i16::from_le_bytes([body[2 * i], body[2 * i + 1]]);
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
            let mut body = [0u8; 6];
            body[..4].copy_from_slice(&(self.shared.samplerate.load(Relaxed) as u32).to_be_bytes());
            body[4..].copy_from_slice(&(self.shared.frame.load(Relaxed) as u16).to_be_bytes());
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
        let local = SocketAddrV4::new(net::local_ip(), socket.local_addr().unwrap().port());
        let socket = Arc::new(socket);
        let shared = Arc::new(Shared::default());
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(1024);
        let (pkt_tx, pkt_rx) = rtrb::RingBuffer::new(512);
        let dsp = Dsp::new(48000, 128, cmd_rx, pkt_rx, socket.clone(), shared.clone());
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
                fx: FxParams::default(),
            }),
            cmds: Mutex::new(cmd_tx),
            incoming: Mutex::new(Some(pkt_tx)),
            running: AtomicBool::new(false),
            start: Instant::now(),
        });
        Ok(Engine { inner, dsp: Arc::new(Mutex::new(dsp)), audio: audio::AudioThread::spawn(), local, public: Mutex::new(None) })
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

    /// Open the sound card. Returns (input latency ms, output latency ms).
    #[pyo3(signature = (input, output, channel, samplerate, frame))]
    fn start_audio(&self, py: Python<'_>, input: String, output: String, channel: Option<usize>, samplerate: u32, frame: usize) -> PyResult<(f32, f32)> {
        let args = audio::StartArgs { input, output, channel, samplerate, frame, dsp: self.dsp.clone(), shared: self.inner.shared.clone() };
        let ok = py.allow_threads(|| self.audio.start(args)).map_err(PyRuntimeError::new_err)?;
        Ok((ok.lat_in_ms, ok.lat_out_ms))
    }

    fn stop_audio(&self, py: Python<'_>) {
        py.allow_threads(|| self.audio.stop());
    }

    /// Run one block without a sound card (tests). Not while a stream is running.
    fn process(&self, input: Vec<f32>) -> Vec<f32> {
        let mut input = input;
        let mut out = vec![0f32; input.len()];
        self.dsp.lock().unwrap().process(&mut input, &mut out);
        out
    }

    fn set_format(&self, samplerate: u32, frame: usize) {
        self.dsp.lock().unwrap().set_format(samplerate, frame);
    }

    // --- controls ------------------------------------------------------------------

    fn set_fx(&self, effect: &str, key: &str, value: f64) -> PyResult<()> {
        let i = fx_index(effect).ok_or_else(|| PyValueError::new_err(format!("no effect {effect}")))?;
        let mut c = self.inner.ctl.lock().unwrap();
        if key == "on" {
            c.fx.on[i] = value != 0.0;
            self.inner.cmd(Cmd::FxOn(i, value != 0.0));
        } else {
            let k = param_index(i, key).ok_or_else(|| PyValueError::new_err(format!("no param {key}")))?;
            c.fx.p[i][k] = value as f32;
            self.inner.cmd(Cmd::FxParam(i, k, value as f32));
        }
        Ok(())
    }

    fn fx_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let fx = self.inner.ctl.lock().unwrap().fx;
        let d = PyDict::new(py);
        for (i, info) in FX.iter().enumerate() {
            let e = PyDict::new(py);
            e.set_item("on", fx.on[i])?;
            for (k, name) in info.params.iter().enumerate() {
                e.set_item(*name, fx.p[i][k])?;
            }
            d.set_item(info.name, e)?;
        }
        Ok(d)
    }

    fn preset<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyDict>> {
        {
            let mut c = self.inner.ctl.lock().unwrap();
            if !c.fx.apply_preset(name) {
                return Err(PyValueError::new_err(format!("no preset {name}")));
            }
            self.inner.cmd(Cmd::FxAll(c.fx));
        }
        self.fx_state(py)
    }

    #[staticmethod]
    fn presets() -> Vec<&'static str> {
        fx::PRESETS.iter().map(|(n, _)| *n).collect()
    }

    fn set_mix(&self, key: &str, value: f64) -> PyResult<()> {
        let v = value as f32;
        self.inner.cmd(match key {
            "partner" => Cmd::RemoteGain(v),
            "monitor" => Cmd::MonitorGain(v),
            "buffer" => Cmd::JitterTarget(value.round().clamp(1.0, 64.0) as i64),
            "room" => Cmd::Room(v),
            _ => return Err(PyValueError::new_err(format!("no mix control {key}"))),
        });
        Ok(())
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
        d.set_item("in_level", s.in_level.get())?;
        d.set_item("out_level", s.out_level.get())?;
        let cushion_ms = s.cushion_frames.load(Relaxed) as f64 * 1000.0 / s.samplerate.load(Relaxed).max(1) as f64;
        d.set_item("lat_in_ms", s.lat_in_us.load(Relaxed) as f64 / 1000.0 + cushion_ms)?;
        d.set_item("lat_out_ms", s.lat_out_us.load(Relaxed) as f64 / 1000.0)?;
        d.set_item("cpu_us", s.cpu_ns.load(Relaxed) as f64 / 1000.0)?;
        d.set_item("raw_in_peak", s.raw_in_peak.get())?;
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
