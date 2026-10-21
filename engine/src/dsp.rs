//! What runs once per audio block: the mixer. Everything here is realtime-safe:
//! no locks shared with other threads, no allocation, no Python. Other threads
//! talk to it through lock-free queues (commands and packets in, retired
//! effect objects out) and atomics (levels and stats out).
//!
//! Signal flow per block:
//!   interface inputs -> local strip: gain -> FX rack -> [send to partner]
//!                                                     -> [Mon] pan/fader -> master
//!   partner channels -> jitter buffer -> remote strip: gain -> FX rack -> pan/fader -> master
//!   master fader -> stereo out

use std::net::{SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

use crate::fx::{Rack, Slot};
use crate::jitter::{JitterBuffer, MAX_CH, MAX_FRAME};
use crate::net;

pub const MAX_LOCAL: usize = 4;
pub const MAX_REMOTE: usize = MAX_CH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Local(usize),
    Remote(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripParam {
    Active,
    Input,
    Gain,
    Pan,
    Fader,
    Mute,
    Solo,
    Send,
    Mon,
}

/// Messages from the control side into the audio thread.
pub enum Cmd {
    Strip(Target, StripParam, f32),
    RackInsert(Target, usize, Box<Slot>),
    RackRemove(Target, usize),
    RackMove(Target, usize, usize),
    RackReplace(Target, usize, Box<Slot>),
    RackOn(Target, usize, bool),
    RackParam(Target, usize, usize, f32),
    Master(f32),
    JitterTarget(i64),
    /// Shared metronome. `epoch` is the leader's block number where beat 0 falls.
    Metro { on: bool, bpm: f32, beats: u32, leader: bool, epoch: i64 },
    MetroVolume(f32),
}

/// One received audio packet, from the network thread to the audio thread.
#[derive(Clone, Copy)]
pub struct InPacket {
    pub seq: u32,
    pub total: u8,
    pub first: u8,
    pub count: u8,
    pub len: u16, // samples per channel
    pub data: [i16; MAX_FRAME],
}

impl Default for InPacket {
    fn default() -> Self {
        InPacket { seq: 0, total: 1, first: 0, count: 1, len: 0, data: [0; MAX_FRAME] }
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
    pub peer_protocol: AtomicU64,
    pub local_levels: [AtomicF32; MAX_LOCAL],
    pub remote_levels: [AtomicF32; MAX_REMOTE],
    pub master_level: AtomicF32,
    pub remote_channels: AtomicU64,
    pub sent_channels: AtomicU64,
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
    pub in_channels: AtomicU64,
    pub blocks: AtomicU64,
    pub cpu_ns: AtomicU64, // smoothed process() time per block
    pub raw_in_peak: AtomicF32,
    pub in_callbacks: AtomicU64,
    pub ring_empty: AtomicU64,
    pub cushion_frames: AtomicU64,
    pub test_signal_loss: AtomicF32,
    pub streaming: std::sync::atomic::AtomicBool,
    /// our block counter (the seq of the block being processed)
    pub seq: AtomicU64,
    /// metronome: beat number + 1 of the last click (0 = none yet), for the UI's blinker
    pub metro_beat: AtomicU64,
    /// metronome: is our click locked to the partner's timeline yet (follower)
    pub metro_synced: std::sync::atomic::AtomicBool,
}

pub fn encode_peer(a: SocketAddrV4) -> u64 {
    (1u64 << 63) | ((u32::from(*a.ip()) as u64) << 16) | a.port() as u64
}

pub fn decode_peer(v: u64) -> Option<SocketAddrV4> {
    (v >> 63 == 1).then(|| SocketAddrV4::new(((v >> 16) as u32).into(), v as u16))
}

/// Settings of one channel strip. Shared shape between the control-side mirror
/// and the audio thread.
#[derive(Clone, Copy, Debug)]
pub struct StripSettings {
    pub active: bool,
    pub input: usize,
    pub gain: f32,
    pub pan: f32, // -1 .. 1
    pub fader: f32,
    pub mute: bool,
    pub solo: bool,
    pub send: bool,
    pub mon: bool,
}

impl StripSettings {
    pub fn local(input: usize) -> Self {
        StripSettings { active: false, input, gain: 1.0, pan: 0.0, fader: 1.0, mute: false, solo: false, send: true, mon: false }
    }
    pub fn remote() -> Self {
        StripSettings { active: true, input: 0, gain: 1.0, pan: 0.0, fader: 1.0, mute: false, solo: false, send: false, mon: true }
    }
    pub fn set(&mut self, p: StripParam, v: f32) {
        match p {
            StripParam::Active => self.active = v != 0.0,
            StripParam::Input => self.input = v.max(0.0) as usize,
            StripParam::Gain => self.gain = v,
            StripParam::Pan => self.pan = v.clamp(-1.0, 1.0),
            StripParam::Fader => self.fader = v,
            StripParam::Mute => self.mute = v != 0.0,
            StripParam::Solo => self.solo = v != 0.0,
            StripParam::Send => self.send = v != 0.0,
            StripParam::Mon => self.mon = v != 0.0,
        }
    }
}

struct Strip {
    s: StripSettings,
    rack: Rack,
    buf: Vec<f32>,
    last: Vec<f32>, // remote strips: last good block, for concealment
    level: f32,
}

impl Strip {
    fn new(s: StripSettings) -> Self {
        Strip { s, rack: Rack::new(), buf: vec![0.0; MAX_FRAME], last: vec![0.0; MAX_FRAME], level: 0.0 }
    }
}

/// The click track. Lives entirely in the audio thread; only your own ears get
/// it (it's added after the mix, never sent).
///
/// Sync with latency in mind: the leader (whoever started it or last changed
/// the tempo) clicks on its own block timeline. The follower clicks on the
/// leader's timeline *as it arrives*: the jitter buffer knows which of the
/// leader's blocks is playing right now, so the follower's click lands exactly
/// on the leader's audio as heard, which is what the follower plays along to.
struct Metro {
    on: bool,
    bpm: f32,
    beats: u32,
    leader: bool,
    epoch: i64,
    volume: f32,
    last_beat: i64,
    left: usize, // samples of the current click still to play
    len: usize,
    phase: f32,
    inc: f32,
    accent: bool,
}

impl Metro {
    fn new() -> Self {
        Metro { on: false, bpm: 100.0, beats: 4, leader: true, epoch: 0, volume: 0.6, last_beat: i64::MIN,
                left: 0, len: 1, phase: 0.0, inc: 0.0, accent: false }
    }

    /// Add this block's clicks to `l`/`r`. `timeline` is the block number on the
    /// leader's timeline that this block corresponds to.
    fn render(&mut self, timeline: i64, frame: usize, sr: f32, l: &mut [f32], r: &mut [f32], shared: &Shared) {
        if !self.on {
            self.left = 0;
            return;
        }
        let spb = sr as f64 * 60.0 / self.bpm.max(20.0) as f64; // samples per beat
        let start = (timeline - self.epoch) as f64 * frame as f64;
        for k in 0..l.len() {
            let pos = start + k as f64;
            if pos >= 0.0 {
                let beat = (pos / spb).floor() as i64;
                if beat != self.last_beat {
                    self.last_beat = beat;
                    self.accent = beat.rem_euclid(self.beats.max(1) as i64) == 0;
                    let hz = if self.accent { 1760.0 } else { 1175.0 };
                    self.inc = 2.0 * std::f32::consts::PI * hz / sr;
                    self.phase = 0.0;
                    self.len = (sr * 0.035) as usize;
                    self.left = self.len;
                    shared.metro_beat.store((beat + 1).max(0) as u64, Relaxed);
                }
            }
            if self.left > 0 {
                let t = 1.0 - self.left as f32 / self.len as f32;
                let env = (1.0 - t).powi(3) * if self.accent { 1.0 } else { 0.7 };
                let s = self.phase.sin() * env * self.volume * 0.5;
                self.phase += self.inc;
                self.left -= 1;
                l[k] += s;
                r[k] += s;
            }
        }
    }
}

pub struct Dsp {
    pub frame: usize,
    pub samplerate: u32,
    local: Vec<Strip>,
    remote: Vec<Strip>,
    master: f32,
    jitter: JitterBuffer,
    cmds: rtrb::Consumer<Cmd>,
    incoming: rtrb::Consumer<InPacket>,
    retired: rtrb::Producer<Box<Slot>>,
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    seq: u32,
    epoch: u64,
    plc_gain: f32,
    remote_count: usize,
    rx: Vec<f32>, // jitter output, channel-major, stride MAX_FRAME
    unpacked: Vec<f32>,
    send_buf: Vec<u8>,
    mix_l: Vec<f32>,
    mix_r: Vec<f32>,
    master_level: f32,
    metro: Metro,
    /// partner's block number minus ours, learned from what the jitter buffer plays
    remote_offset: Option<i64>,
}

#[inline]
fn pan_gains(pan: f32) -> (f32, f32) {
    // constant power: -3 dB each side in the middle
    let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (a.cos(), a.sin())
}

impl Dsp {
    pub fn new(
        samplerate: u32,
        frame: usize,
        cmds: rtrb::Consumer<Cmd>,
        incoming: rtrb::Consumer<InPacket>,
        retired: rtrb::Producer<Box<Slot>>,
        socket: Arc<UdpSocket>,
        shared: Arc<Shared>,
    ) -> Self {
        shared.samplerate.store(samplerate as u64, Relaxed);
        shared.frame.store(frame as u64, Relaxed);
        shared.jitter_target.store(3, Relaxed);
        let mut local: Vec<Strip> = (0..MAX_LOCAL).map(|i| Strip::new(StripSettings::local(i))).collect();
        local[0].s.active = true;
        Dsp {
            frame,
            samplerate,
            local,
            remote: (0..MAX_REMOTE).map(|_| Strip::new(StripSettings::remote())).collect(),
            master: 1.0,
            jitter: JitterBuffer::new(3),
            cmds,
            incoming,
            retired,
            socket,
            shared,
            seq: 0,
            epoch: 0,
            plc_gain: 0.0,
            remote_count: 0,
            rx: vec![0.0; MAX_CH * MAX_FRAME],
            unpacked: vec![0.0; MAX_FRAME],
            send_buf: Vec::with_capacity(net::HEADER + 3 + net::MAX_AUDIO_PAYLOAD + 2 * MAX_FRAME),
            mix_l: vec![0.0; MAX_FRAME],
            mix_r: vec![0.0; MAX_FRAME],
            master_level: 0.0,
            metro: Metro::new(),
            remote_offset: None,
        }
    }

    /// Change sample rate / block size. Rebuilds effect state, so it allocates:
    /// only call while no stream runs.
    pub fn set_format(&mut self, samplerate: u32, frame: usize) {
        let frame = frame.min(MAX_FRAME);
        if samplerate != self.samplerate {
            for strip in self.local.iter_mut().chain(self.remote.iter_mut()) {
                for slot in strip.rack.slots.iter_mut() {
                    // plugins re-setup in place (keeping their state); the rest
                    // are rebuilt; a model that can't be reloaded keeps its old instance
                    if !slot.fx.set_sample_rate(samplerate as f32) {
                        if let Ok(fx) = crate::fx::make(&slot.cfg, samplerate as f32) {
                            slot.fx = fx;
                        }
                    }
                }
            }
        }
        self.samplerate = samplerate;
        self.frame = frame;
        self.jitter.reset();
        self.shared.samplerate.store(samplerate as u64, Relaxed);
        self.shared.frame.store(frame as u64, Relaxed);
    }

    fn strip(&mut self, t: Target) -> Option<&mut Strip> {
        match t {
            Target::Local(i) => self.local.get_mut(i),
            Target::Remote(i) => self.remote.get_mut(i),
        }
    }

    fn retire(&mut self, slot: Box<Slot>) {
        // hand it back so it's freed off the audio thread; if that queue is
        // somehow full, freeing here is the lesser evil
        let _ = self.retired.push(slot);
    }

    /// Apply queued commands outside the audio callback (no stream running).
    pub fn apply_pending(&mut self) {
        self.apply_commands();
    }

    fn apply_commands(&mut self) {
        while let Ok(cmd) = self.cmds.pop() {
            match cmd {
                Cmd::Strip(t, p, v) => {
                    if let Some(s) = self.strip(t) {
                        s.s.set(p, v);
                    }
                }
                Cmd::RackInsert(t, pos, slot) => {
                    let rejected = match self.strip(t) {
                        Some(s) => s.rack.insert(pos, slot).err(),
                        None => Some(slot),
                    };
                    if let Some(slot) = rejected {
                        self.retire(slot);
                    }
                }
                Cmd::RackRemove(t, pos) => {
                    if let Some(slot) = self.strip(t).and_then(|s| s.rack.remove(pos)) {
                        self.retire(slot);
                    }
                }
                Cmd::RackReplace(t, pos, slot) => {
                    let old = match self.strip(t) {
                        Some(s) => s.rack.replace(pos, slot),
                        None => slot,
                    };
                    self.retire(old);
                }
                Cmd::RackMove(t, from, to) => {
                    if let Some(s) = self.strip(t) {
                        s.rack.move_slot(from, to);
                    }
                }
                Cmd::RackOn(t, pos, on) => {
                    if let Some(s) = self.strip(t) {
                        s.rack.set_on(pos, on);
                    }
                }
                Cmd::RackParam(t, pos, k, v) => {
                    if let Some(slot) = self.strip(t).and_then(|s| s.rack.slots.get_mut(pos)) {
                        if k < slot.cfg.p.len() {
                            slot.cfg.p[k] = v;
                        }
                    }
                }
                Cmd::Master(v) => self.master = v,
                Cmd::JitterTarget(t) => {
                    self.jitter.target = t;
                    self.shared.jitter_target.store(t as u64, Relaxed);
                }
                Cmd::Metro { on, bpm, beats, leader, epoch } => {
                    let m = &mut self.metro;
                    m.on = on;
                    m.bpm = bpm;
                    m.beats = beats.max(1);
                    m.leader = leader;
                    m.epoch = epoch;
                    m.last_beat = i64::MIN; // the first beat on the new grid clicks
                }
                Cmd::MetroVolume(v) => self.metro.volume = v,
            }
        }
    }

    fn take_packets(&mut self) {
        let epoch = self.shared.peer_epoch.load(Relaxed);
        if epoch != self.epoch {
            self.epoch = epoch;
            self.jitter.reset();
            self.remote_offset = None;
        }
        while let Ok(pkt) = self.incoming.pop() {
            let count = pkt.count.max(1) as usize;
            let total_samples = (pkt.len as usize * count).min(MAX_FRAME);
            for (d, s) in self.unpacked[..total_samples].iter_mut().zip(&pkt.data[..total_samples]) {
                *d = *s as f32 / 32767.0;
            }
            self.jitter.push(pkt.seq, pkt.total as usize, pkt.first as usize, count, pkt.len as usize,
                             &self.unpacked[..total_samples]);
        }
    }

    /// Send every strip with Send on, as one block split into MTU-sized packets.
    fn send(&mut self, n: usize) {
        let Some(peer) = decode_peer(self.shared.peer.load(Relaxed)) else { return };
        let sending: [usize; MAX_LOCAL] = std::array::from_fn(|i| i);
        let mut list = [0usize; MAX_LOCAL];
        let mut total = 0;
        for &i in sending.iter() {
            if self.local[i].s.active && self.local[i].s.send {
                list[total] = i;
                total += 1;
            }
        }
        self.shared.sent_channels.store(total as u64, Relaxed);
        if total == 0 {
            return;
        }
        let seq = self.seq;

        if self.shared.peer_protocol.load(Relaxed) < 2 {
            // an old build on the other end: one mono mix, the original format
            self.send_buf.clear();
            self.send_buf.extend_from_slice(net::MAGIC);
            self.send_buf.push(net::T_AUDIO);
            self.send_buf.extend_from_slice(&seq.to_be_bytes());
            for k in 0..n {
                let v: f32 = list[..total].iter().map(|&i| self.local[i].buf[k]).sum();
                self.send_buf.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            }
            let _ = self.socket.send_to(&self.send_buf, peer);
            return;
        }

        let per_packet = (net::MAX_AUDIO_PAYLOAD / (2 * n)).max(1);
        let mut first = 0;
        while first < total {
            let count = per_packet.min(total - first);
            self.send_buf.clear();
            self.send_buf.extend_from_slice(net::MAGIC);
            self.send_buf.push(net::T_AUDIO_MC);
            self.send_buf.extend_from_slice(&seq.to_be_bytes());
            self.send_buf.extend_from_slice(&[total as u8, first as u8, count as u8]);
            for &i in &list[first..first + count] {
                for &s in &self.local[i].buf[..n] {
                    self.send_buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
            }
            let _ = self.socket.send_to(&self.send_buf, peer);
            first += count;
        }
    }

    /// One block. `input` is interleaved, `in_ch` channels; `out` is
    /// interleaved stereo (2 * frame). Both are exactly `self.frame` frames.
    pub fn process(&mut self, input: &[f32], in_ch: usize, out: &mut [f32]) {
        let t0 = std::time::Instant::now();
        let n = self.frame;
        self.apply_commands();
        self.take_packets();

        let any_solo = self.local.iter().chain(self.remote.iter()).any(|s| s.s.active && s.s.solo);
        let audible = |s: &StripSettings| !s.mute && (!any_solo || s.solo);
        self.mix_l[..n].fill(0.0);
        self.mix_r[..n].fill(0.0);

        // --- my strips ---
        for strip in self.local.iter_mut() {
            if !strip.s.active {
                continue;
            }
            let ch = strip.s.input;
            let g = strip.s.gain;
            for k in 0..n {
                strip.buf[k] = if ch < in_ch { input[k * in_ch + ch] * g } else { 0.0 };
            }
            strip.rack.process(&mut strip.buf[..n]);
            let pk = strip.buf[..n].iter().fold(0f32, |a, v| a.max(v.abs()));
            strip.level = pk.max(strip.level * 0.9);
            if strip.s.mon && audible(&strip.s) {
                let (l, r) = pan_gains(strip.s.pan);
                let f = strip.s.fader;
                for k in 0..n {
                    self.mix_l[k] += strip.buf[k] * f * l;
                    self.mix_r[k] += strip.buf[k] * f * r;
                }
            }
        }
        self.send(n);

        // --- partner's strips ---
        match self.jitter.pop(&mut self.rx, n) {
            Some((c, rseq)) => {
                // which of the partner's blocks is playing now, relative to ours
                self.remote_offset = Some(rseq - self.seq as i64);
                self.remote_count = c;
                self.plc_gain = 1.0;
                for (i, strip) in self.remote.iter_mut().enumerate().take(c) {
                    strip.buf[..n].copy_from_slice(&self.rx[i * MAX_FRAME..i * MAX_FRAME + n]);
                    strip.last[..n].copy_from_slice(&strip.buf[..n]);
                }
            }
            None => {
                // concealment: fade the last good block out instead of a hard click
                self.plc_gain *= 0.5;
                let g = self.plc_gain;
                for strip in self.remote.iter_mut().take(self.remote_count) {
                    for k in 0..n {
                        strip.buf[k] = strip.last[k] * g;
                    }
                }
            }
        }
        for strip in self.remote.iter_mut().take(self.remote_count) {
            let g = strip.s.gain;
            for v in strip.buf[..n].iter_mut() {
                *v *= g;
            }
            strip.rack.process(&mut strip.buf[..n]);
            let pk = strip.buf[..n].iter().fold(0f32, |a, v| a.max(v.abs()));
            strip.level = pk.max(strip.level * 0.9);
            if audible(&strip.s) {
                let (l, r) = pan_gains(strip.s.pan);
                let f = strip.s.fader;
                for k in 0..n {
                    self.mix_l[k] += strip.buf[k] * f * l;
                    self.mix_r[k] += strip.buf[k] * f * r;
                }
            }
        }

        // --- master ---
        let m = self.master;
        let mut pk = 0f32;
        for k in 0..n {
            let (l, r) = (self.mix_l[k] * m, self.mix_r[k] * m);
            out[2 * k] = l;
            out[2 * k + 1] = r;
            pk = pk.max(l.abs()).max(r.abs());
        }
        self.master_level = pk.max(self.master_level * 0.9);

        crate::fx::TEMPO_BITS.store((self.metro.bpm as f64).to_bits(), Relaxed);
        // click track: local only, after the mix and after the master fader
        let timeline = if self.metro.leader {
            self.seq as i64
        } else {
            self.seq as i64 + self.remote_offset.unwrap_or(0)
        };
        self.shared.metro_synced.store(self.metro.leader || self.remote_offset.is_some(), Relaxed);
        let (ml, mr) = (&mut self.mix_l[..n], &mut self.mix_r[..n]);
        ml.fill(0.0);
        mr.fill(0.0);
        self.metro.render(timeline, n, self.samplerate as f32, ml, mr, &self.shared);
        if self.metro.on {
            for k in 0..n {
                out[2 * k] += ml[k];
                out[2 * k + 1] += mr[k];
            }
        }
        self.shared.seq.store(self.seq as u64, Relaxed);
        self.seq = self.seq.wrapping_add(1);

        let sh = &self.shared;
        for (i, s) in self.local.iter().enumerate() {
            sh.local_levels[i].set(s.level);
        }
        for (i, s) in self.remote.iter_mut().enumerate() {
            if i >= self.remote_count {
                s.level = 0.0;
            }
            sh.remote_levels[i].set(s.level);
        }
        sh.master_level.set(self.master_level);
        sh.remote_channels.store(self.remote_count as u64, Relaxed);
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
