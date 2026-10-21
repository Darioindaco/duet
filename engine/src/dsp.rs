//! What runs once per audio block. Everything here is realtime-safe: no
//! locks shared with other threads, no allocation, no Python. Other threads
//! talk to it through lock-free queues (commands in, packets in) and atomics
//! (levels and stats out).

use std::net::{SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

use crate::fx::{Chain, FxParams, Reverb, MAX_PARAMS};
use crate::jitter::{JitterBuffer, MAX_FRAME};
use crate::net;

/// Messages from the control side into the audio thread.
pub enum Cmd {
    FxOn(usize, bool),
    FxParam(usize, usize, f32),
    FxAll(FxParams),
    RemoteGain(f32),
    MonitorGain(f32),
    JitterTarget(i64),
    Room(f32),
}

/// One received audio frame, handed from the network thread to the audio thread.
#[derive(Clone, Copy)]
pub struct InPacket {
    pub seq: u32,
    pub len: u16,
    pub data: [i16; MAX_FRAME],
}

impl Default for InPacket {
    fn default() -> Self {
        InPacket { seq: 0, len: 0, data: [0; MAX_FRAME] }
    }
}

#[derive(Default)]
pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
}

/// State shared between threads, all atomics.
#[derive(Default)]
pub struct Shared {
    /// partner address: 0 = none, else 1<<63 | ip<<16 | port
    pub peer: AtomicU64,
    /// bumped whenever the partner (re)appears, so the audio thread resets its buffer
    pub peer_epoch: AtomicU64,
    pub in_level: AtomicF32,
    pub out_level: AtomicF32,
    pub underruns: AtomicU64,
    pub lost: AtomicU64,
    pub late: AtomicU64,
    pub dropped: AtomicU64,
    pub depth: AtomicU64,
    pub jitter_target: AtomicU64,
    pub xruns: AtomicU64,
    pub packets_in: AtomicU64,
    pub lat_in_us: AtomicU64,
    pub lat_out_us: AtomicU64,
    pub samplerate: AtomicU64,
    pub frame: AtomicU64,
    pub blocks: AtomicU64,
    pub cpu_ns: AtomicU64, // smoothed process() time per block
    pub raw_in_peak: AtomicF32, // straight from the input callback, before anything else
    pub in_callbacks: AtomicU64,
    pub ring_empty: AtomicU64, // output needed input that hadn't arrived yet
    pub cushion_frames: AtomicU64, // extra input->output delay outside ASIO
    pub test_signal_loss: AtomicF32, // selftest: drop this share of signal packets
}

pub fn encode_peer(a: SocketAddrV4) -> u64 {
    (1u64 << 63) | ((u32::from(*a.ip()) as u64) << 16) | a.port() as u64
}

pub fn decode_peer(v: u64) -> Option<SocketAddrV4> {
    (v >> 63 == 1).then(|| SocketAddrV4::new(((v >> 16) as u32).into(), v as u16))
}

pub struct Dsp {
    pub frame: usize,
    pub samplerate: u32,
    chain: Chain,
    room: Reverb,
    room_params: [f32; MAX_PARAMS],
    room_on: bool,
    jitter: JitterBuffer,
    cmds: rtrb::Consumer<Cmd>,
    incoming: rtrb::Consumer<InPacket>,
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    seq: u32,
    epoch: u64,
    remote_gain: f32,
    monitor_gain: f32,
    plc_gain: f32,
    last: Vec<f32>,
    remote: Vec<f32>,
    unpacked: Vec<f32>,
    send_buf: Vec<u8>,
    in_level: f32,
    out_level: f32,
}

impl Dsp {
    pub fn new(
        samplerate: u32,
        frame: usize,
        cmds: rtrb::Consumer<Cmd>,
        incoming: rtrb::Consumer<InPacket>,
        socket: Arc<UdpSocket>,
        shared: Arc<Shared>,
    ) -> Self {
        let sr = samplerate as f32;
        shared.samplerate.store(samplerate as u64, Relaxed);
        shared.frame.store(frame as u64, Relaxed);
        shared.jitter_target.store(3, Relaxed);
        Dsp {
            frame,
            samplerate,
            chain: Chain::new(sr),
            room: Reverb::new(sr),
            room_params: [0.6, 0.4, 0.0],
            room_on: false,
            jitter: JitterBuffer::new(3),
            cmds,
            incoming,
            socket,
            shared,
            seq: 0,
            epoch: 0,
            remote_gain: 1.0,
            monitor_gain: 0.0,
            plc_gain: 0.0,
            last: vec![0.0; MAX_FRAME],
            remote: vec![0.0; MAX_FRAME],
            unpacked: vec![0.0; MAX_FRAME],
            send_buf: Vec::with_capacity(net::HEADER + 2 * MAX_FRAME),
            in_level: 0.0,
            out_level: 0.0,
        }
    }

    /// Change sample rate / block size. Allocates: only call while no stream runs.
    pub fn set_format(&mut self, samplerate: u32, frame: usize) {
        let frame = frame.min(MAX_FRAME);
        if samplerate != self.samplerate {
            let params = self.chain.params;
            self.chain = Chain::new(samplerate as f32);
            self.chain.params = params;
            self.room = Reverb::new(samplerate as f32);
        }
        self.samplerate = samplerate;
        self.frame = frame;
        self.jitter.reset();
        self.shared.samplerate.store(samplerate as u64, Relaxed);
        self.shared.frame.store(frame as u64, Relaxed);
    }

    fn apply_commands(&mut self) {
        while let Ok(cmd) = self.cmds.pop() {
            match cmd {
                Cmd::FxOn(i, on) => self.chain.set_on(i, on),
                Cmd::FxParam(i, k, v) => self.chain.params.p[i][k] = v,
                Cmd::FxAll(p) => {
                    for i in 0..p.on.len() {
                        self.chain.set_on(i, p.on[i]);
                    }
                    self.chain.params.p = p.p;
                }
                Cmd::RemoteGain(g) => self.remote_gain = g,
                Cmd::MonitorGain(g) => self.monitor_gain = g,
                Cmd::JitterTarget(t) => {
                    self.jitter.target = t;
                    self.shared.jitter_target.store(t as u64, Relaxed);
                }
                Cmd::Room(v) => {
                    if v > 0.001 && !self.room_on {
                        self.room.reset();
                    }
                    self.room_on = v > 0.001;
                    self.room_params[2] = v;
                }
            }
        }
    }

    /// One block: `input` is our mono signal (effects are applied in place),
    /// `out` receives what to play. Both are exactly `self.frame` long.
    pub fn process(&mut self, input: &mut [f32], out: &mut [f32]) {
        let t0 = std::time::Instant::now();
        let n = input.len();
        self.apply_commands();

        let epoch = self.shared.peer_epoch.load(Relaxed);
        if epoch != self.epoch {
            self.epoch = epoch;
            self.jitter.reset();
        }
        while let Ok(pkt) = self.incoming.pop() {
            let len = pkt.len as usize;
            for (d, s) in self.unpacked[..len].iter_mut().zip(&pkt.data[..len]) {
                *d = *s as f32 / 32767.0;
            }
            self.jitter.push(pkt.seq, &self.unpacked[..len]);
        }

        self.chain.process(input);

        if let Some(peer) = decode_peer(self.shared.peer.load(Relaxed)) {
            self.send_buf.clear();
            self.send_buf.extend_from_slice(net::MAGIC);
            self.send_buf.push(net::T_AUDIO);
            self.send_buf.extend_from_slice(&self.seq.to_be_bytes());
            for s in input.iter() {
                let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                self.send_buf.extend_from_slice(&v.to_le_bytes());
            }
            let _ = self.socket.send_to(&self.send_buf, peer);
            self.seq = self.seq.wrapping_add(1);
        }

        let remote = &mut self.remote[..n];
        if self.jitter.pop(remote) {
            self.last[..n].copy_from_slice(remote);
            self.plc_gain = 1.0;
        } else {
            // concealment: fade the last good frame out instead of a hard click
            self.plc_gain *= 0.5;
            for (r, l) in remote.iter_mut().zip(&self.last[..n]) {
                *r = l * self.plc_gain;
            }
        }
        if self.room_on {
            self.room.process(&self.room_params, remote);
        }

        let (rg, mg) = (self.remote_gain, self.monitor_gain);
        let (mut pk_in, mut pk_out) = (0f32, 0f32);
        for i in 0..n {
            pk_in = pk_in.max(input[i].abs());
            pk_out = pk_out.max(remote[i].abs());
            out[i] = remote[i] * rg + input[i] * mg;
        }
        self.in_level = pk_in.max(self.in_level * 0.9);
        self.out_level = pk_out.max(self.out_level * 0.9);

        let sh = &self.shared;
        sh.in_level.set(self.in_level);
        sh.out_level.set(self.out_level);
        let st = self.jitter.stats;
        sh.underruns.store(st.underruns, Relaxed);
        sh.lost.store(st.lost, Relaxed);
        sh.late.store(st.late, Relaxed);
        sh.dropped.store(st.dropped, Relaxed);
        sh.depth.store(self.jitter.depth() as u64, Relaxed);
        sh.blocks.fetch_add(1, Relaxed);
        let ns = t0.elapsed().as_nanos() as u64;
        let prev = sh.cpu_ns.load(Relaxed);
        sh.cpu_ns.store(if prev == 0 { ns } else { (prev * 31 + ns) / 32 }, Relaxed);
    }
}
