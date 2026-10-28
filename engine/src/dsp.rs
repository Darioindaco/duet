//! What runs once per audio block: the mixer. Everything here is realtime-safe:
//! no locks shared with other threads, no allocation, no Python. Other threads
//! talk to it through lock-free queues (commands and packets in, retired
//! effect objects out) and atomics (levels and stats out).
//!
//! Signal flow per block (strips are stereo from their input on):
//!   interface inputs -> local strip: gain -> FX rack -> [send to partner, as mono]
//!                                                     -> [Mon] pan/fader -> master
//!   partner channels -> jitter buffer -> remote strip: gain -> FX rack -> pan/fader -> master
//!   timeline -> track strips: the song mixer (mix.rs: buses, sends, delay compensation) -> master
//!   master -> master FX rack -> master fader -> stereo out
//! While recording, each recorded local strip's signal (after its rack, so the
//! same as what the partner hears) also goes into a ring for the take writer.

use std::net::{SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

use crate::daw::{Timeline, MAX_TRACKS};
use crate::fx::{no_keys, Slot};
use crate::instr::{InstSlot, NoteEv};
use crate::mix::{render_tracks, Play, Strip, TrackBufs, Wiring};
use crate::jitter::{JitterBuffer, MAX_CH, MAX_FRAME};
use crate::net;

pub const MAX_LOCAL: usize = 4;
pub const MAX_REMOTE: usize = MAX_CH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Local(usize),
    Remote(usize),
    Track(usize),
    /// the master's FX rack
    Master,
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
    JitterAuto(bool),
    /// Shared metronome. `epoch` is the leader's block number where beat 0 falls.
    Metro { on: bool, bpm: f32, beats: u32, leader: bool, epoch: i64 },
    MetroVolume(f32),
    /// The timeline to play from now on (the old one comes back as garbage).
    Timeline(Box<Timeline>),
    /// Start the transport at `pos`; the click plays at least until `click_until`
    /// (count-in). With rings, record the local strips that have one.
    Play { pos: i64, click_until: i64, rec: Option<Box<RecRings>> },
    Stop,
    Locate(i64),
    Loop { on: bool, start: i64, end: i64 },
    /// The click while the transport plays (tempo and bar from the project).
    Click { on: bool, bpm: f32, beats: u32 },
    /// What a strip plays instead of its input (None: back to the input).
    Instrument(Target, Option<Box<InstSlot>>),
    InstParam(Target, usize, f32),
    /// How the song's tracks are wired (the old one comes back as garbage).
    Wiring(Box<Wiring>),
    /// A send's level: (track, bus, level).
    SendLevel(usize, usize, f32),
    /// What a live strip (yours or the partner's) sends to the song's buses.
    LiveSends(Target, LiveSends),
    /// Hear a sample from the library (None: stop). Only you hear it.
    Preview(Option<std::sync::Arc<crate::daw::AudioData>>),
}

/// A live strip's sends: (bus track slot, level), after its fader.
pub type LiveSends = [Option<(u8, f32)>; crate::mix::MAX_SENDS];

/// One ring per local strip to record (None = not recorded).
pub struct RecRings(pub [Option<rtrb::Producer<f32>>; MAX_LOCAL]);

/// Things the audio thread is done with, handed back to be freed elsewhere.
#[allow(dead_code)] // never read: only held so they're dropped on the other side
pub enum Garbage {
    Slot(Box<Slot>),
    Timeline(Box<Timeline>),
    Rings(Box<RecRings>),
    Inst(Box<InstSlot>),
    Wiring(Box<Wiring>),
    Preview(std::sync::Arc<crate::daw::AudioData>),
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
    pub jitter_auto: std::sync::atomic::AtomicBool,
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
    /// the largest input callback (samples), and input dropped for clock drift
    pub in_chunk: AtomicU64,
    /// the library's preview: where it is and how long it is (frames)
    pub preview_pos: AtomicU64,
    pub preview_len: AtomicU64,
    pub in_dropped: AtomicU64,
    pub cushion_frames: AtomicU64,
    pub test_signal_loss: AtomicF32,
    pub streaming: std::sync::atomic::AtomicBool,
    /// the sound card went away (unplugged) or failed mid-stream
    pub stream_error: std::sync::atomic::AtomicBool,
    /// our block counter (the seq of the block being processed)
    pub seq: AtomicU64,
    /// metronome: beat number + 1 of the last click (0 = none yet), for the UI's blinker
    pub metro_beat: AtomicU64,
    /// metronome: is our click locked to the partner's timeline yet (follower)
    pub metro_synced: std::sync::atomic::AtomicBool,
    /// transport position (samples) and state
    pub daw_pos: AtomicI64,
    pub daw_playing: AtomicBool,
    pub rec_on: AtomicBool,
    /// timeline position of a take's first sample (once rec_started)
    pub rec_first: AtomicI64,
    pub rec_started: AtomicBool,
    /// samples the take writer couldn't keep up with
    pub rec_dropped: AtomicU64,
    pub track_levels: [AtomicF32; MAX_TRACKS],
    /// notes sent to instruments so far (the UI's MIDI activity light)
    pub midi_events: AtomicU64,
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
    pub fn track() -> Self {
        StripSettings { active: false, ..StripSettings::remote() }
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
    /// clicks before 0 too (the timeline's count-in)
    negative: bool,
}

impl Metro {
    fn new() -> Self {
        Metro { on: false, bpm: 100.0, beats: 4, leader: true, epoch: 0, volume: 0.6, last_beat: i64::MIN,
                left: 0, len: 1, phase: 0.0, inc: 0.0, accent: false, negative: false }
    }

    /// Add this block's clicks to `l`/`r`. `timeline` is the block number on the
    /// leader's timeline that this block corresponds to.
    fn render(&mut self, timeline: i64, frame: usize, sr: f32, l: &mut [f32], r: &mut [f32], shared: &Shared) {
        if !self.on {
            self.left = 0;
            return;
        }
        let start = (timeline - self.epoch) as f64 * frame as f64;
        self.render_at(start, sr, l, r, shared);
    }

    /// Clicks for samples start..start+l.len() of a grid whose beat 0 is at 0.
    fn render_at(&mut self, start: f64, sr: f32, l: &mut [f32], r: &mut [f32], shared: &Shared) {
        let spb = sr as f64 * 60.0 / self.bpm.max(20.0) as f64; // samples per beat
        for k in 0..l.len() {
            let pos = start + k as f64;
            if pos >= 0.0 || self.negative {
                let beat = (pos / spb).floor() as i64;
                if self.last_beat == i64::MIN && pos - beat as f64 * spb >= 1.0 {
                    self.last_beat = beat; // started mid-beat: wait for the next one
                }
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

/// Where the timeline is and what it does.
struct Transport {
    playing: bool,
    pos: i64,
    loop_on: bool,
    loop_start: i64,
    loop_end: i64,
    click_on: bool,
    click_until: i64,
    recording: bool,
    rec_started: bool,
}

pub struct Dsp {
    pub frame: usize,
    pub samplerate: u32,
    local: Vec<Strip>,
    remote: Vec<Strip>,
    tracks: Vec<Strip>,
    master_strip: Strip,
    wiring: Option<Box<Wiring>>,
    bufs: TrackBufs,
    timeline: Option<Box<Timeline>>,
    preview: Option<(std::sync::Arc<crate::daw::AudioData>, usize)>,
    tp: Transport,
    click: Metro,
    rec: [Option<rtrb::Producer<f32>>; MAX_LOCAL],
    master: f32,
    jitter: JitterBuffer,
    cmds: rtrb::Consumer<Cmd>,
    incoming: rtrb::Consumer<InPacket>,
    notes: rtrb::Consumer<(Target, NoteEv, bool)>,
    /// while recording: notes played on song tracks, with where they fell
    noterec: rtrb::Producer<(u8, NoteEv, i64)>,
    retired: rtrb::Producer<Garbage>,
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
    /// the live strips' sends to the song's buses: yours, then the partner's
    live_sends: [LiveSends; MAX_LOCAL + MAX_REMOTE],
}

#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
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
        notes: rtrb::Consumer<(Target, NoteEv, bool)>,
        noterec: rtrb::Producer<(u8, NoteEv, i64)>,
        retired: rtrb::Producer<Garbage>,
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
            tracks: (0..MAX_TRACKS).map(|_| Strip::new(StripSettings::track())).collect(),
            master_strip: Strip::new(StripSettings::remote()),
            wiring: Some(Wiring::plain()),
            bufs: TrackBufs::new(),
            timeline: Some(Box::new(Timeline::empty())),
            preview: None,
            tp: Transport { playing: false, pos: 0, loop_on: false, loop_start: 0, loop_end: 0, click_on: false,
                            click_until: i64::MIN, recording: false, rec_started: false },
            click: Metro { negative: true, ..Metro::new() },
            rec: Default::default(),
            master: 1.0,
            jitter: JitterBuffer::new(3),
            cmds,
            incoming,
            notes,
            noterec,
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
            live_sends: [[None; crate::mix::MAX_SENDS]; MAX_LOCAL + MAX_REMOTE],
        }
    }

    /// Change sample rate / block size. Rebuilds effect state, so it allocates:
    /// only call while no stream runs.
    pub fn set_format(&mut self, samplerate: u32, frame: usize) {
        let frame = frame.min(MAX_FRAME);
        if samplerate != self.samplerate {
            for strip in self.local.iter_mut().chain(self.remote.iter_mut()).chain(self.tracks.iter_mut())
                .chain(std::iter::once(&mut self.master_strip)) {
                for slot in strip.rack.slots.iter_mut() {
                    // plugins re-setup in place (keeping their state); the rest
                    // are rebuilt; a model that can't be reloaded keeps its old instance
                    if !slot.fx.set_sample_rate(samplerate as f32) {
                        if let Ok(fx) = crate::fx::make(&slot.cfg, samplerate as f32) {
                            slot.fx = fx;
                        }
                    }
                }
                if let Some(inst) = strip.inst.as_mut() {
                    if !inst.inst.set_sample_rate(samplerate as f32) {
                        if let Ok(fresh) = InstSlot::new(inst.cfg.clone(), samplerate as f32) {
                            *inst = fresh;
                        }
                    }
                }
            }
        }
        if self.jitter.is_auto() {
            // keep the same safety time in ms with the new block length; auto takes it from there
            let old = self.frame as f64 / self.samplerate as f64;
            let new = frame as f64 / samplerate as f64;
            self.jitter.target = ((self.jitter.target as f64 * old / new).ceil() as i64).clamp(1, 32);
            self.jitter.set_auto(true, 1.0 / new);
        }
        if samplerate != self.samplerate {
            // positions are in samples: a new rate means the caller sends a new
            // timeline and starts again from a position in seconds
            self.tp.playing = false;
            self.stop_recording();
        }
        self.samplerate = samplerate;
        self.frame = frame;
        self.jitter.reset();
        self.shared.jitter_target.store(self.jitter.target as u64, Relaxed);
        self.shared.samplerate.store(samplerate as u64, Relaxed);
        self.shared.frame.store(frame as u64, Relaxed);
    }

    fn strip(&mut self, t: Target) -> Option<&mut Strip> {
        match t {
            Target::Local(i) => self.local.get_mut(i),
            Target::Remote(i) => self.remote.get_mut(i),
            Target::Track(i) => self.tracks.get_mut(i),
            Target::Master => Some(&mut self.master_strip),
        }
    }

    fn retire(&mut self, slot: Box<Slot>) {
        self.discard(Garbage::Slot(slot));
    }

    fn discard(&mut self, g: Garbage) {
        // hand it back so it's freed off the audio thread; if that queue is
        // somehow full, freeing here is the lesser evil
        let _ = self.retired.push(g);
    }

    /// The song's instruments let go of whatever they play (the transport
    /// stopped or jumped, or the notes changed under them).
    fn release_tracks(&mut self) {
        for s in self.tracks.iter_mut() {
            if let Some(inst) = s.inst.as_mut() {
                inst.inst.note(NoteEv::Release);
            }
        }
    }

    /// Let go of the take rings: the writer sees them abandoned and finishes.
    /// (Dropping a producer frees nothing while the writer still holds its end.)
    fn stop_recording(&mut self) {
        for r in self.rec.iter_mut() {
            *r = None;
        }
        self.tp.recording = false;
        self.tp.rec_started = false;
        self.shared.rec_on.store(false, Relaxed);
    }

    /// Apply queued commands outside the audio callback (no stream running).
    pub fn apply_pending(&mut self) {
        self.apply_commands();
    }

    fn apply_commands(&mut self) {
        let had = self.cmds.slots() > 0;
        self.apply_queue();
        if had {
            // seen right away, also when no stream runs to process a block
            self.shared.daw_pos.store(self.tp.pos, Relaxed);
            self.shared.daw_playing.store(self.tp.playing, Relaxed);
        }
    }

    fn apply_queue(&mut self) {
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
                    self.jitter.set_target(t);
                    self.shared.jitter_target.store(t as u64, Relaxed);
                    self.shared.jitter_auto.store(false, Relaxed);
                }
                Cmd::JitterAuto(on) => {
                    if on {
                        self.jitter.set_auto(true, self.samplerate as f64 / self.frame as f64);
                    } else {
                        let t = self.jitter.target;
                        self.jitter.set_target(t); // stay where auto got to
                    }
                    self.shared.jitter_auto.store(on, Relaxed);
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
                Cmd::MetroVolume(v) => {
                    self.metro.volume = v;
                    self.click.volume = v;
                }
                Cmd::Timeline(tl) => {
                    if let Some(old) = self.timeline.replace(tl) {
                        self.discard(Garbage::Timeline(old));
                    }
                    if self.tp.playing {
                        self.release_tracks(); // a note's end may be gone with the old one
                    }
                }
                Cmd::Play { pos, click_until, rec } => {
                    self.stop_recording();
                    self.release_tracks();
                    self.tp.playing = true;
                    self.tp.pos = pos;
                    self.tp.click_until = click_until;
                    self.click.last_beat = i64::MIN;
                    if let Some(mut rings) = rec {
                        for (mine, theirs) in self.rec.iter_mut().zip(rings.0.iter_mut()) {
                            *mine = theirs.take();
                        }
                        self.discard(Garbage::Rings(rings));
                        self.tp.recording = true;
                        self.shared.rec_started.store(false, Relaxed);
                        self.shared.rec_dropped.store(0, Relaxed);
                        self.shared.rec_on.store(true, Relaxed);
                    }
                }
                Cmd::Stop => {
                    self.tp.playing = false;
                    self.stop_recording();
                    self.release_tracks();
                }
                Cmd::Locate(pos) => {
                    self.release_tracks();
                    self.tp.pos = pos;
                    self.click.last_beat = i64::MIN;
                }
                Cmd::Loop { on, start, end } => {
                    self.tp.loop_on = on && end > start;
                    self.tp.loop_start = start;
                    self.tp.loop_end = end;
                }
                Cmd::Click { on, bpm, beats } => {
                    self.tp.click_on = on;
                    self.click.bpm = bpm;
                    self.click.beats = beats.max(1);
                }
                Cmd::Instrument(t, inst) => {
                    let old = match self.strip(t) {
                        Some(s) => std::mem::replace(&mut s.inst, inst),
                        None => inst,
                    };
                    if let Some(old) = old {
                        self.discard(Garbage::Inst(old));
                    }
                }
                Cmd::InstParam(t, k, v) => {
                    if let Some(inst) = self.strip(t).and_then(|s| s.inst.as_mut()) {
                        if k < inst.cfg.p.len() {
                            inst.cfg.p[k] = v;
                        }
                    }
                }
                Cmd::Wiring(w) => {
                    if let Some(old) = self.wiring.replace(w) {
                        self.discard(Garbage::Wiring(old));
                    }
                }
                Cmd::Preview(data) => {
                    if let Some((old, _)) = self.preview.take() {
                        self.discard(Garbage::Preview(old));
                    }
                    self.shared.preview_len.store(data.as_ref().map_or(0, |d| d.samples.len() as u64), Relaxed);
                    self.shared.preview_pos.store(0, Relaxed);
                    self.preview = data.map(|d| (d, 0));
                }
                Cmd::SendLevel(t, bus, level) => {
                    if let Some(w) = self.wiring.as_mut() {
                        w.set_send(t, bus, level);
                    }
                }
                Cmd::LiveSends(t, sends) => match t {
                    Target::Local(i) if i < MAX_LOCAL => self.live_sends[i] = sends,
                    Target::Remote(i) if i < MAX_REMOTE => self.live_sends[MAX_LOCAL + i] = sends,
                    _ => {}
                },
            }
        }
    }

    /// Notes from the MIDI keyboard / computer keyboard (ours, or the partner's
    /// played live on a song track) to the strips' instruments.
    fn take_notes(&mut self) {
        while let Ok((t, ev, remote)) = self.notes.pop() {
            if let Target::Track(i) = t {
                // the partner's live notes are theirs to record, on their side
                if self.tp.recording && self.tp.playing && !remote {
                    let _ = self.noterec.push((i as u8, ev, self.tp.pos));
                }
            }
            if let Some(inst) = self.strip(t).and_then(|s| s.inst.as_mut()) {
                inst.inst.note(ev);
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
                let v: f32 = list[..total].iter().map(|&i| 0.5 * (self.local[i].l[k] + self.local[i].r[k])).sum();
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
                let st = &self.local[i];
                for (a, b) in st.l[..n].iter().zip(&st.r[..n]) {
                    let s = 0.5 * (a + b); // live audio goes out mono
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
        self.take_notes();
        self.take_packets();

        let any_solo = self.local.iter().chain(self.remote.iter()).any(|s| s.s.active && s.s.solo);
        let audible = |s: &StripSettings| !s.mute && (!any_solo || s.solo);
        self.mix_l[..n].fill(0.0);
        self.mix_r[..n].fill(0.0);
        self.bufs.clear_live();

        // --- my strips ---
        for (i, strip) in self.local.iter_mut().enumerate() {
            if !strip.s.active {
                continue;
            }
            let ch = strip.s.input;
            let g = strip.s.gain;
            if let Some(inst) = strip.inst.as_mut() {
                // an instrument strip plays its instrument instead of an input
                let InstSlot { cfg, inst } = &mut **inst;
                inst.process(&cfg.p, &mut strip.l[..n], &mut strip.r[..n]);
                for k in 0..n {
                    strip.l[k] *= g;
                    strip.r[k] *= g;
                }
            } else {
                for k in 0..n {
                    let v = if ch < in_ch { input[k * in_ch + ch] * g } else { 0.0 };
                    strip.l[k] = v;
                    strip.r[k] = v;
                }
            }
            strip.rack.process(&mut strip.l[..n], &mut strip.r[..n], &no_keys);
            strip.meter(n);
            let (gl, gr) = pan_gains(strip.s.pan);
            let f = strip.s.fader;
            if strip.s.mon && audible(&strip.s) {
                for k in 0..n {
                    self.mix_l[k] += strip.l[k] * f * gl;
                    self.mix_r[k] += strip.r[k] * f * gr;
                }
            }
            // its sends to the song's buses (also without MON: a reverb on what
            // your interface already lets you hear dry)
            if audible(&strip.s) {
                for &(bus, level) in self.live_sends[i].iter().flatten() {
                    self.bufs.live_into(bus as usize, &strip.l[..n], &strip.r[..n], f * gl * level, f * gr * level);
                }
            }
        }
        // --- recording: each recorded strip's signal, as sent (mono) ---
        if self.tp.recording {
            if !self.tp.rec_started {
                self.tp.rec_started = true;
                self.shared.rec_first.store(self.tp.pos, Relaxed);
                self.shared.rec_started.store(true, Relaxed);
            }
            for (i, ring) in self.rec.iter_mut().enumerate() {
                let Some(ring) = ring else { continue };
                let strip = &self.local[i];
                match ring.write_chunk_uninit(n) {
                    Ok(chunk) if strip.s.active => {
                        chunk.fill_from_iter(strip.l[..n].iter().zip(&strip.r[..n]).map(|(a, b)| 0.5 * (a + b)));
                    }
                    Ok(chunk) => {
                        chunk.fill_from_iter(std::iter::repeat(0.0));
                    }
                    Err(_) => {
                        self.shared.rec_dropped.fetch_add(n as u64, Relaxed);
                    }
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
                    strip.l[..n].copy_from_slice(&self.rx[i * MAX_FRAME..i * MAX_FRAME + n]);
                    strip.last[..n].copy_from_slice(&strip.l[..n]);
                }
            }
            None => {
                // concealment: fade the last good block out instead of a hard click
                self.plc_gain *= 0.5;
                let g = self.plc_gain;
                for strip in self.remote.iter_mut().take(self.remote_count) {
                    for k in 0..n {
                        strip.l[k] = strip.last[k] * g;
                    }
                }
            }
        }
        for (i, strip) in self.remote.iter_mut().enumerate().take(self.remote_count) {
            let g = strip.s.gain;
            for k in 0..n {
                strip.l[k] *= g;
                strip.r[k] = strip.l[k];
            }
            strip.rack.process(&mut strip.l[..n], &mut strip.r[..n], &no_keys);
            strip.meter(n);
            if audible(&strip.s) {
                let (gl, gr) = pan_gains(strip.s.pan);
                let f = strip.s.fader;
                for k in 0..n {
                    self.mix_l[k] += strip.l[k] * f * gl;
                    self.mix_r[k] += strip.r[k] * f * gr;
                }
                for &(bus, level) in self.live_sends[MAX_LOCAL + i].iter().flatten() {
                    self.bufs.live_into(bus as usize, &strip.l[..n], &strip.r[..n], f * gl * level, f * gr * level);
                }
            }
        }

        // --- the song: one or (wrapping around a loop) two stretches of the timeline ---
        let tp_pos = self.tp.pos;
        let mut play = Play { segs: [(0usize, tp_pos, n); 2], nseg: 1, looping: None };
        let looping = self.tp.playing && self.tp.loop_on && !self.tp.recording;
        // (>=: a block that ends right on the loop's end still jumps back, with an
        // empty second stretch, or the next block would start past the end)
        if looping && tp_pos < self.tp.loop_end && tp_pos + n as i64 >= self.tp.loop_end {
            let k = (self.tp.loop_end - tp_pos) as usize;
            play.segs[0] = (0, tp_pos, k);
            play.segs[1] = (k, self.tp.loop_start, n - k);
            play.nseg = 2;
        }
        if looping {
            play.looping = Some((self.tp.loop_start, self.tp.loop_end));
        }
        if let Some(tl) = self.timeline.as_deref() {
            crate::mix::apply_automation(tl, tp_pos, &mut self.tracks, &mut self.master_strip.rack);
        }
        if let Some(w) = self.wiring.as_deref_mut() {
            let playing = self.tp.playing.then_some(&play);
            render_tracks(&mut self.tracks, w, &mut self.bufs, self.timeline.as_deref(), playing, n,
                          &mut self.mix_l[..n], &mut self.mix_r[..n]);
        }
        let segs = play.segs;
        let nseg = play.nseg;
        if self.tp.playing {
            self.tp.pos = if nseg == 2 { segs[1].1 + segs[1].2 as i64 } else { tp_pos + n as i64 };
        }

        // --- master: its rack (part of the song), then the fader ---
        {
            let bufs = &self.bufs;
            self.master_strip.rack.process(&mut self.mix_l[..n], &mut self.mix_r[..n], &|slot| bufs.key(slot, n));
        }
        let m = self.master;
        let mut pk = 0f32;
        for k in 0..n {
            let (l, r) = (self.mix_l[k] * m, self.mix_r[k] * m);
            out[2 * k] = l;
            out[2 * k + 1] = r;
            pk = pk.max(l.abs()).max(r.abs());
        }
        self.master_level = pk.max(self.master_level * 0.9);

        // tempo-synced plugins follow the song while it plays, the metronome otherwise
        let tempo = if self.tp.playing { self.click.bpm } else { self.metro.bpm };
        crate::fx::TEMPO_BITS.store((tempo as f64).to_bits(), Relaxed);
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
        let daw_click = self.tp.playing && (self.tp.click_on || tp_pos < self.tp.click_until);
        if daw_click {
            // the timeline's own click (and count-in) while it plays; the music
            // comes out of the master rack that much later, so the click waits too
            let m_lat = self.wiring.as_ref().map_or(0, |w| w.master_lat) as f64;
            for &(off, p, len) in &segs[..nseg] {
                self.click.render_at(p as f64 - m_lat, self.samplerate as f32, &mut ml[off..off + len], &mut mr[off..off + len], &self.shared);
            }
        } else {
            self.click.left = 0;
            self.metro.render(timeline, n, self.samplerate as f32, ml, mr, &self.shared);
        }
        if self.metro.on || daw_click {
            for k in 0..n {
                out[2 * k] += ml[k];
                out[2 * k + 1] += mr[k];
            }
        }
        // the library's preview: local only, after everything, like the click
        let mut done = false;
        if let Some((d, pos)) = self.preview.as_mut() {
            let right = d.right.as_ref().unwrap_or(&d.samples);
            let m = (d.samples.len() - (*pos).min(d.samples.len())).min(n);
            for k in 0..m {
                out[2 * k] += d.samples[*pos + k] * 0.8;
                out[2 * k + 1] += right[*pos + k] * 0.8;
            }
            *pos += m;
            self.shared.preview_pos.store(*pos as u64, Relaxed);
            done = *pos >= d.samples.len();
        }
        if done {
            if let Some((d, _)) = self.preview.take() {
                self.discard(Garbage::Preview(d));
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
        for (i, s) in self.tracks.iter().enumerate() {
            sh.track_levels[i].set(s.level);
        }
        sh.daw_pos.store(self.tp.pos, Relaxed);
        sh.daw_playing.store(self.tp.playing, Relaxed);
        sh.master_level.set(self.master_level);
        sh.remote_channels.store(self.remote_count as u64, Relaxed);
        let st = self.jitter.stats;
        sh.underruns.store(st.underruns, Relaxed);
        sh.lost.store(st.lost, Relaxed);
        sh.late.store(st.late, Relaxed);
        sh.dropped.store(st.dropped, Relaxed);
        sh.depth.store(self.jitter.depth() as u64, Relaxed);
        sh.jitter_target.store(self.jitter.target as u64, Relaxed); // moves by itself in auto mode
        sh.blocks.fetch_add(1, Relaxed);
        let ns = t0.elapsed().as_nanos() as u64;
        let prev = sh.cpu_ns.load(Relaxed);
        sh.cpu_ns.store(if prev == 0 { ns } else { (prev * 31 + ns) / 32 }, Relaxed);
    }
}
