//! The DAW half: audio clips on a timeline, recording takes, and the audio
//! files behind them.
//!
//! Same realtime rule as the effects: the audio thread only ever gets finished
//! objects. A Timeline (every track's clips, with their audio) is built on the
//! control side and swapped in whole; the old one goes back to be freed there.
//! Recording runs the other way: the audio thread pushes samples into one
//! lock-free ring per source, and a writer thread turns them into WAV files.
//!
//! Positions inside the engine are samples at the device rate. The project
//! (Python) thinks in seconds, so the same project plays right at any rate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

use crate::dsp::StripSettings;
use crate::fx::{Rack, Slot, SlotCfg};
use crate::instr::{InstCfg, InstSlot, NoteEv};
use crate::mix::{apply_automation, render_tracks, Play, Strip, TrackBufs, Wiring};

pub const MAX_TRACKS: usize = 32;
/// Samples per waveform overview bucket (one min/max pair each).
pub const PEAK_BUCKET: usize = 512;
/// Short ramps at every clip edge, so a cut never clicks.
const DECLICK: i64 = 32;

/// One audio file at its own sample rate: mono (`samples`), or stereo
/// (`samples` left, `right` right).
pub struct AudioData {
    pub sr: u32,
    pub samples: Vec<f32>,
    pub right: Option<Vec<f32>>,
}

impl AudioData {
    pub fn mono(sr: u32, samples: Vec<f32>) -> Self {
        AudioData { sr, samples, right: None }
    }
    /// Both sides mixed (a waveform, a drum pad).
    pub fn to_mono(&self) -> Vec<f32> {
        match &self.right {
            Some(r) => self.samples.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect(),
            None => self.samples.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ClipRt {
    pub start: i64, // timeline position of the clip's first sample
    /// where in the audio that first sample is (in the audio's own samples)
    pub pos0: f64,
    pub len: i64,
    /// audio samples per timeline sample: 1 when the file is at the device
    /// rate and plays as it is; otherwise it's converted as it plays (another
    /// rate, a faster or slower clip in tape mode); negative plays it backwards
    pub step: f64,
    pub gain: f32,
    pub fade_in: i64,
    pub fade_out: i64,
    pub data: Arc<AudioData>,
}

/// A note event for a track's instrument, at a timeline position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteAt {
    pub pos: i64,
    pub on: bool,
    pub key: u8,
    pub vel: u8,
}

/// Every track's clips, sorted by start, and the notes its instrument plays,
/// sorted by position. Index = track slot.
pub struct Timeline {
    pub tracks: Vec<Vec<ClipRt>>,
    pub notes: Vec<Vec<NoteAt>>,
    pub autos: Vec<AutoLane>,
}

/// What an automation lane moves. Values on the lane are 0..1 (knob range);
/// a fader's is turned into gain the way the fader does it, a pan into -1..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AutoTarget {
    /// a song track's fader or pan
    Fader(usize),
    Pan(usize),
    /// a knob on an effect: track slot (None = the master rack), rack position, parameter
    Fx(Option<usize>, usize, usize),
    /// a knob on a track's instrument
    Inst(usize, usize),
}

/// One automation lane: points (timeline position, value 0..1) sorted by
/// position, straight lines between them, flat before the first and after the last.
#[derive(Clone, Debug)]
pub struct AutoLane {
    pub target: AutoTarget,
    pub pts: Vec<(i64, f32)>,
}

impl AutoLane {
    pub fn value_at(&self, pos: i64) -> Option<f32> {
        let p = &self.pts;
        let first = p.first()?;
        let i = p.partition_point(|q| q.0 <= pos);
        Some(if i == 0 {
            first.1
        } else if i == p.len() {
            p[i - 1].1
        } else {
            let (a, b) = (p[i - 1], p[i]);
            let t = (pos - a.0) as f32 / (b.0 - a.0).max(1) as f32;
            a.1 + (b.1 - a.1) * t
        })
    }
}

/// The fader law of the UI's faders: 0..1 travel to gain (0.5 = -12 dB, 0.794 = 0 dB).
pub fn fader_gain(v: f32) -> f32 {
    2.0 * v.clamp(0.0, 1.0).powi(3)
}

/// An automation lane as the project describes it: points in seconds.
#[derive(Clone, Debug)]
pub struct AutoDesc {
    pub target: AutoTarget,
    pub pts: Vec<(f64, f32)>,
}

pub fn auto_lanes(desc: &[AutoDesc], sr: u32) -> Vec<AutoLane> {
    desc.iter()
        .filter(|d| !d.pts.is_empty())
        .map(|d| {
            let mut pts: Vec<(i64, f32)> = d.pts.iter().map(|&(t, v)| ((t * sr as f64).round() as i64, v.clamp(0.0, 1.0))).collect();
            pts.sort_by_key(|p| p.0);
            AutoLane { target: d.target, pts }
        })
        .collect()
}

impl Timeline {
    pub fn empty() -> Self {
        Timeline { tracks: (0..MAX_TRACKS).map(|_| Vec::new()).collect(), notes: (0..MAX_TRACKS).map(|_| Vec::new()).collect(), autos: Vec::new() }
    }

    /// Track `t`'s note events in pos..end (a binary search: no scanning).
    pub fn notes_in(&self, t: usize, pos: i64, end: i64) -> &[NoteAt] {
        let Some(ns) = self.notes.get(t) else { return &[] };
        let a = ns.partition_point(|n| n.pos < pos);
        let b = a + ns[a..].partition_point(|n| n.pos < end);
        &ns[a..b]
    }

    /// Add track `t`'s audio for timeline samples pos..pos+l.len() into `l`, `r`
    /// (a mono clip into both).
    pub fn render(&self, t: usize, pos: i64, l: &mut [f32], r: &mut [f32]) {
        let n = l.len() as i64;
        let Some(clips) = self.tracks.get(t) else { return };
        for c in clips {
            if c.start >= pos + n {
                break;
            }
            let (a, b) = (c.start.max(pos), (c.start + c.len).min(pos + n));
            if a >= b {
                continue;
            }
            let src = &c.data.samples;
            let src_r = c.data.right.as_ref().unwrap_or(src);
            let fi = c.fade_in.max(DECLICK) as f32;
            let fo = c.fade_out.max(DECLICK) as f32;
            let last = src.len() as i64 - 1;
            for x in a..b {
                let rel = x - c.start;
                let (sl, sr) = if c.step == 1.0 && c.pos0.fract() == 0.0 {
                    let si = c.pos0 as i64 + rel;
                    if si < 0 || si > last {
                        continue;
                    }
                    (src[si as usize], src_r[si as usize])
                } else {
                    // Catmull-Rom between the audio's own samples
                    let p = c.pos0 + rel as f64 * c.step;
                    let i = p.floor() as i64;
                    if i < 0 || i > last {
                        continue;
                    }
                    let t = (p - i as f64) as f32;
                    let ix = |d: i64| (i + d).clamp(0, last) as usize;
                    let (i0, i1, i2, i3) = (ix(-1), ix(0), ix(1), ix(2));
                    (cubic(src[i0], src[i1], src[i2], src[i3], t), cubic(src_r[i0], src_r[i1], src_r[i2], src_r[i3], t))
                };
                let mut g = c.gain;
                let left = (c.len - rel) as f32;
                if (rel as f32) < fi {
                    g *= (rel as f32 + 0.5) / fi;
                }
                if left <= fo {
                    g *= (left - 0.5) / fo;
                }
                let k = (x - pos) as usize;
                l[k] += sl * g;
                r[k] += sr * g;
            }
        }
    }
}

#[inline]
fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

/// A clip as the project describes it, in seconds.
#[derive(Clone, Debug)]
pub struct ClipDesc {
    pub key: String,
    pub start: f64,
    /// where in the file the clip begins (seconds of the file as it is)
    pub offset: f64,
    /// on the timeline (seconds)
    pub len: f64,
    pub gain: f32,
    pub fade_in: f64,
    pub fade_out: f64,
    /// 2 = twice as fast
    pub speed: f64,
    /// semitones
    pub pitch: f32,
    /// keep the pitch when the speed changes (and transpose without changing
    /// the speed): a stretched copy, made in the background. Off = tape:
    /// speed and pitch go together, as it plays.
    pub stretch: bool,
    /// the clip's stretch of the file, backwards
    pub reverse: bool,
    /// notes moved in the file (the Tune tab), with their identity; empty = none
    pub tune: crate::tune::Moves,
    pub tune_hash: u64,
}

impl ClipDesc {
    /// A plain clip: as the file is.
    pub fn plain(key: &str, start: f64, offset: f64, len: f64) -> Self {
        ClipDesc { key: key.into(), start, offset, len, gain: 1.0, fade_in: 0.0, fade_out: 0.0, speed: 1.0, pitch: 0.0, stretch: false, reverse: false,
                   tune: Default::default(), tune_hash: 0 }
    }

    /// File seconds per timeline second.
    pub fn ratio(&self) -> f64 {
        if self.stretch { self.speed } else { self.speed * 2f64.powf(self.pitch as f64 / 12.0) }
    }

    /// Which made copy it plays, if any: the tuned file (notes moved), and/or
    /// stretched (keep pitch). The tuned file alone plays like the file (at
    /// the clip's ratio, tape style); a stretched one in timeline time.
    pub fn stretch_key(&self) -> Option<StretchKey> {
        let k = StretchKey::new(&self.key, self.speed, self.pitch, self.tune_hash);
        let stretched = self.stretch && !k.plain();
        if stretched {
            Some(k)
        } else if self.tune_hash != 0 {
            Some(StretchKey::new(&self.key, 1.0, 0.0, self.tune_hash))
        } else {
            None
        }
    }
}

/// A stretched copy of a file: speed in 1/10000, pitch in cents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StretchKey {
    pub key: String,
    pub speed: u32,
    pub cents: i32,
    /// the tune it starts from (0: the file as it is)
    pub tune: u64,
}

impl StretchKey {
    pub fn new(key: &str, speed: f64, pitch: f32, tune: u64) -> Self {
        StretchKey { key: key.into(), speed: (speed.clamp(0.05, 20.0) * 10_000.0).round() as u32, cents: (pitch * 100.0).round() as i32, tune }
    }
    /// no stretching (only the tune, if any)
    pub fn plain(&self) -> bool {
        self.speed == 10_000 && self.cents == 0
    }
    pub fn speed(&self) -> f64 {
        self.speed as f64 / 10_000.0
    }
    pub fn semis(&self) -> f32 {
        self.cents as f32 / 100.0
    }
}

/// Play track `t`'s instrument for timeline samples pos..pos+l.len() into
/// `l`, `r` (replacing them), each note at its exact sample.
pub fn render_inst(tl: &Timeline, t: usize, inst: &mut InstSlot, pos: i64, l: &mut [f32], r: &mut [f32]) {
    let n = l.len();
    let mut done = 0;
    for e in tl.notes_in(t, pos, pos + n as i64) {
        let at = ((e.pos - pos).max(0) as usize).min(n);
        if at > done {
            inst.inst.process(&inst.cfg.p, &mut l[done..at], &mut r[done..at]);
            done = at;
        }
        inst.inst.note(if e.on { NoteEv::On(e.key, e.vel) } else { NoteEv::Off(e.key) });
    }
    if done < n {
        inst.inst.process(&inst.cfg.p, &mut l[done..], &mut r[done..]);
    }
}

/// A note as the song describes it, in seconds.
#[derive(Clone, Debug)]
pub struct NoteDesc {
    pub t: f64,
    pub dur: f64,
    pub key: u8,
    pub vel: u8,
}

/// Loaded audio, by file key: one copy each, at the file's own rate (an hour
/// of stereo is 1.4 GB as floats: no second, converted copy).
#[derive(Default)]
pub struct AudioCache {
    files: HashMap<String, Arc<AudioData>>,
    /// stretched copies the clips use
    stretched: HashMap<StretchKey, Arc<AudioData>>,
    /// stretched copies the last build wanted and didn't have
    pub missing: Vec<StretchKey>,
    /// the tunes the clips use, by their identity
    tunes: HashMap<u64, crate::tune::Moves>,
}

/// What it takes to make a copy (see `make_copy`): the file, and the tune and
/// the tuned file if there's a tune.
pub struct CopyJob {
    pub key: StretchKey,
    pub file: Arc<AudioData>,
    pub moves: Option<crate::tune::Moves>,
    pub tuned: Option<Arc<AudioData>>,
}

/// Make a copy: the tune first (unless it's made already), then the stretch.
/// Returns every copy made (the tuned one too, to keep).
pub fn make_copy(job: &CopyJob) -> Vec<(StretchKey, AudioData)> {
    let k = &job.key;
    let mut out = Vec::new();
    let base: Arc<AudioData> = match (&job.moves, &job.tuned) {
        (_, Some(t)) => t.clone(),
        (Some(m), None) => {
            let t = crate::tune::render(&job.file, m);
            if k.plain() {
                return vec![(k.clone(), t)];
            }
            let t = Arc::new(t);
            out.push((StretchKey::new(&k.key, 1.0, 0.0, k.tune), AudioData { sr: t.sr, samples: t.samples.clone(), right: t.right.clone() }));
            t
        }
        (None, None) => job.file.clone(),
    };
    if !k.plain() {
        out.push((k.clone(), crate::stretch::stretch(&base, k.speed(), k.semis())));
    }
    out
}

impl AudioCache {
    pub fn insert(&mut self, key: &str, data: AudioData) -> Arc<AudioData> {
        let a = Arc::new(data);
        self.files.insert(key.to_string(), a.clone());
        a
    }


    pub fn get(&self, key: &str) -> Option<&Arc<AudioData>> {
        self.files.get(key)
    }

    pub fn remove(&mut self, key: &str) {
        self.files.remove(key);
        self.stretched.retain(|k, _| k.key != key);
    }

    pub fn file(&self, key: &str) -> Option<Arc<AudioData>> {
        self.files.get(key).cloned()
    }

    /// What making copy `k` takes (None: its file isn't here any more).
    pub fn job(&self, k: &StretchKey) -> Option<CopyJob> {
        let file = self.files.get(&k.key)?.clone();
        let moves = (k.tune != 0).then(|| self.tunes.get(&k.tune).cloned()).flatten();
        if k.tune != 0 && moves.is_none() {
            return None;
        }
        let tuned = (k.tune != 0 && !k.plain()).then(|| self.stretched.get(&StretchKey::new(&k.key, 1.0, 0.0, k.tune)).cloned()).flatten();
        Some(CopyJob { key: k.clone(), file, moves, tuned })
    }

    pub fn add_stretched(&mut self, k: StretchKey, data: AudioData) {
        if self.files.contains_key(&k.key) {
            self.stretched.insert(k, Arc::new(data));
        }
    }


    /// Build the audio thread's timeline from the project's description.
    pub fn build(&mut self, desc: &[Vec<ClipDesc>], notes: &[Vec<NoteDesc>], sr: u32) -> Timeline {
        let s = |secs: f64| (secs * sr as f64).round() as i64;
        let mut tl = Timeline::empty();
        let mut used = std::collections::HashSet::new();
        self.missing.clear();
        self.tunes.clear();
        for c in desc.iter().flatten() {
            if c.tune_hash != 0 {
                self.tunes.insert(c.tune_hash, c.tune.clone());
            }
        }
        for (t, clips) in desc.iter().enumerate().take(MAX_TRACKS) {
            for c in clips {
                let Some(file) = self.files.get(&c.key).cloned() else { continue }; // not here (yet)
                let len = s(c.len);
                if len <= 0 {
                    continue;
                }
                // which audio, where in it the clip starts, and how fast it's read
                let (data, at, rate) = match c.stretch_key() {
                    Some(k) => {
                        let Some(d) = self.stretched.get(&k).cloned() else {
                            if !self.missing.contains(&k) {
                                self.missing.push(k);
                            }
                            continue; // being made
                        };
                        let plain = k.plain();
                        used.insert(k.clone());
                        if k.tune != 0 && !plain {
                            used.insert(StretchKey::new(&k.key, 1.0, 0.0, k.tune)); // (kept: re-stretching is quicker)
                        }
                        if plain {
                            (d, c.offset, c.ratio()) // the tuned file: like the file
                        } else {
                            // the stretched copy is in timeline time: read as it is
                            (d, c.offset / c.speed.clamp(0.05, 20.0), 1.0)
                        }
                    }
                    None => (file, c.offset, c.ratio()),
                };
                let per = data.sr as f64 / sr as f64 * rate; // audio samples per timeline sample
                let pos0 = at * data.sr as f64;
                let (pos0, step) = if c.reverse { (pos0 + (len - 1) as f64 * per, -per) } else { (pos0, per) };
                let pos0 = if step == 1.0 { pos0.round() } else { pos0 };
                tl.tracks[t].push(ClipRt {
                    start: s(c.start),
                    pos0,
                    len,
                    step,
                    gain: c.gain,
                    fade_in: s(c.fade_in),
                    fade_out: s(c.fade_out),
                    data,
                });
            }
            tl.tracks[t].sort_by_key(|c| c.start);
        }
        // stretched copies nothing uses any more: let go (they're big)
        self.stretched.retain(|k, _| used.contains(k));
        for (t, ns) in notes.iter().enumerate().take(MAX_TRACKS) {
            let ev = &mut tl.notes[t];
            for n in ns {
                let on = s(n.t);
                let off = s(n.t + n.dur).max(on + 1);
                ev.push(NoteAt { pos: on, on: true, key: n.key & 127, vel: n.vel.clamp(1, 127) });
                ev.push(NoteAt { pos: off, on: false, key: n.key & 127, vel: 0 });
            }
            // at the same moment a note ends before the next one starts (repeated notes)
            ev.sort_by_key(|e| (e.pos, e.on));
        }
        tl
    }
}

/// Cubic (Catmull-Rom) rate conversion: fine for playing a take recorded at
/// 44.1k on a 48k session and vice versa.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = ((x.len() as f64) / step).floor() as usize;
    let mut out = vec![0f32; n];
    // an hour of audio is a few hundred million samples: every core takes a stretch
    let cores = std::thread::available_parallelism().map_or(1, |c| c.get()).clamp(1, 16);
    let per = n.div_ceil(cores).max(1 << 16);
    std::thread::scope(|sc| {
        for (j, chunk) in out.chunks_mut(per).enumerate() {
            sc.spawn(move || resample_into(x, step, j * per, chunk));
        }
    });
    out
}

/// Output samples k0.. of a Catmull-Rom conversion into `out`.
fn resample_into(x: &[f32], step: f64, k0: usize, out: &mut [f32]) {
    let last = x.len() as isize - 1;
    let at = |i: isize| -> f32 { x[i.clamp(0, last) as usize] };
    for (k, o) in out.iter_mut().enumerate() {
        let p = (k0 + k) as f64 * step;
        let i = p as isize;
        let t = (p - i as f64) as f32;
        let (y0, y1, y2, y3) = if i >= 1 && i + 2 <= last {
            let i = i as usize;
            (x[i - 1], x[i], x[i + 1], x[i + 2])
        } else {
            (at(i - 1), at(i), at(i + 1), at(i + 2))
        };
        let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c = -0.5 * y0 + 0.5 * y2;
        *o = ((a * t + b) * t + c) * t + y1;
    }
}

/// Peaks of both sides mixed (a stereo file's waveform).
pub fn audio_peaks(a: &AudioData) -> Vec<u8> {
    let Some(r) = &a.right else { return peaks(&a.samples) };
    let mut out = Vec::with_capacity(2 * a.samples.len() / PEAK_BUCKET + 2);
    for (cl, cr) in a.samples.chunks(PEAK_BUCKET).zip(r.chunks(PEAK_BUCKET)) {
        let (lo, hi) = cl.iter().zip(cr).fold((0f32, 0f32), |(lo, hi), (a, b)| {
            let v = 0.5 * (a + b);
            (lo.min(v), hi.max(v))
        });
        out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
        out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
    }
    out
}

/// Waveform overview: a (min, max) pair of i8 per PEAK_BUCKET samples.
pub fn peaks(x: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * x.len() / PEAK_BUCKET + 2);
    for c in x.chunks(PEAK_BUCKET) {
        let (lo, hi) = c.iter().fold((0f32, 0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
        out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
    }
    out
}

/// Any PCM or float WAV: mono stays mono, two channels or more come in as
/// stereo (the first two).
pub fn read_wav(path: &Path) -> Result<AudioData, String> {
    let (frames, _, ch) = wav_header(path)?;
    let mut l = Vec::with_capacity(frames);
    let mut r = Vec::with_capacity(if ch > 1 { frames } else { 0 });
    let (sr, _) = wav_blocks(path, |block, ch| {
        if ch == 1 {
            l.extend_from_slice(block);
        } else {
            for f in block.chunks_exact(ch) {
                l.push(f[0]);
                r.push(f[1]);
            }
        }
    })?;
    Ok(AudioData { sr, samples: l, right: (ch > 1).then_some(r) })
}

/// (frames, sample rate, channels), from the header alone.
pub fn wav_header(path: &Path) -> Result<(usize, u32, usize), String> {
    let rd = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = rd.spec();
    Ok((rd.duration() as usize, spec.sample_rate, spec.channels.max(1) as usize))
}

/// Read a WAV in big blocks of whole frames (interleaved floats, and the
/// channel count): fast, and nothing but the block is kept. Returns (sample
/// rate, channels). Plain PCM (8/16/24/32-bit) and float (32/64-bit), also in
/// the "extensible" header; anything else goes through hound, slower.
fn wav_blocks(path: &Path, mut block: impl FnMut(&[f32], usize)) -> Result<(u32, usize), String> {
    use std::io::{Read, Seek, SeekFrom};
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut f = std::fs::File::open(path).map_err(err)?;
    let mut h = [0u8; 12];
    f.read_exact(&mut h).map_err(err)?;
    if &h[0..4] != b"RIFF" || &h[8..12] != b"WAVE" {
        return Err(format!("{}: not a WAV file", path.display()));
    }
    let (mut fmt, mut ch, mut sr, mut bits) = (0u16, 0usize, 0u32, 0u16);
    let data_len = loop {
        let mut c = [0u8; 8];
        if f.read_exact(&mut c).is_err() {
            return Err(format!("{}: no audio in this WAV", path.display()));
        }
        let size = u32::from_le_bytes([c[4], c[5], c[6], c[7]]) as u64;
        match &c[0..4] {
            b"fmt " => {
                let mut b = vec![0u8; size as usize];
                f.read_exact(&mut b).map_err(err)?;
                if b.len() < 16 {
                    return Err(format!("{}: broken WAV header", path.display()));
                }
                fmt = u16::from_le_bytes([b[0], b[1]]);
                ch = u16::from_le_bytes([b[2], b[3]]).max(1) as usize;
                sr = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                bits = u16::from_le_bytes([b[14], b[15]]);
                if fmt == 0xFFFE && b.len() >= 26 {
                    fmt = u16::from_le_bytes([b[24], b[25]]); // the subformat
                }
                if size % 2 == 1 {
                    f.seek(SeekFrom::Current(1)).map_err(err)?;
                }
            }
            b"data" => break size,
            _ => {
                f.seek(SeekFrom::Current((size + size % 2) as i64)).map_err(err)?;
            }
        }
    };
    let bps = (bits as usize).div_ceil(8);
    let known = matches!((fmt, bps), (1, 1..=4) | (3, 4) | (3, 8));
    if !known || sr == 0 {
        return wav_blocks_hound(path, block);
    }
    let frame = bps * ch;
    // a header that says 0 or "unknown" (some recorders): up to the end of the file
    let avail = f.metadata().map_err(err)?.len().saturating_sub(f.stream_position().map_err(err)?);
    let mut left = if data_len == 0 || data_len == u32::MAX as u64 { avail } else { data_len.min(avail) };
    left -= left % frame as u64;
    let frames_per = (1 << 20) / frame;
    let mut raw = vec![0u8; frames_per * frame];
    let mut out = vec![0f32; frames_per * ch];
    while left > 0 {
        let n = (left as usize).min(raw.len());
        f.read_exact(&mut raw[..n]).map_err(err)?;
        left -= n as u64;
        let m = n / bps;
        let src = &raw[..n];
        let dst = &mut out[..m];
        match (fmt, bps) {
            (1, 1) => dst.iter_mut().zip(src).for_each(|(o, &b)| *o = (b as f32 - 128.0) / 128.0),
            (1, 2) => dst.iter_mut().zip(src.chunks_exact(2)).for_each(|(o, b)| *o = i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
            (1, 3) => dst.iter_mut().zip(src.chunks_exact(3)).for_each(|(o, b)| {
                *o = (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
            }),
            (1, 4) => dst.iter_mut().zip(src.chunks_exact(4)).for_each(|(o, b)| {
                *o = i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
            }),
            (3, 4) => dst.iter_mut().zip(src.chunks_exact(4)).for_each(|(o, b)| *o = f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            _ => dst.iter_mut().zip(src.chunks_exact(8)).for_each(|(o, b)| {
                *o = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }),
        }
        block(&out[..m], ch);
    }
    Ok((sr, ch))
}

/// The slow, general way (hound), for WAVs the fast reader doesn't know.
fn wav_blocks_hound(path: &Path, mut block: impl FnMut(&[f32], usize)) -> Result<(u32, usize), String> {
    let mut rd = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = rd.spec();
    let ch = spec.channels.max(1) as usize;
    let mut buf = Vec::with_capacity(4096 * ch);
    let mut flush = |buf: &mut Vec<f32>, all: bool| {
        if buf.len() >= 4096 * ch || (all && !buf.is_empty()) {
            let whole = buf.len() - buf.len() % ch;
            block(&buf[..whole], ch);
            buf.clear();
        }
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for v in rd.samples::<f32>() {
                buf.push(v.map_err(|e| e.to_string())?);
                flush(&mut buf, false);
            }
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(8, 32) - 1)) as f32;
            for v in rd.samples::<i32>() {
                buf.push(v.map_err(|e| e.to_string())? as f32 * scale);
                flush(&mut buf, false);
            }
        }
    }
    flush(&mut buf, true);
    Ok((spec.sample_rate, ch))
}

/// A WAV's (frames, sample rate, channels, waveform peaks of both sides mixed),
/// read straight through without keeping the audio.
pub fn wav_peaks(path: &Path) -> Result<(usize, u32, usize, Vec<u8>), String> {
    let mut out = Vec::new();
    let (mut lo, mut hi, mut n, mut frames) = (0f32, 0f32, 0usize, 0usize);
    let (sr, ch) = wav_blocks(path, |block, ch| {
        for f in block.chunks_exact(ch) {
            let v = if ch > 1 { 0.5 * (f[0] + f[1]) } else { f[0] };
            lo = lo.min(v);
            hi = hi.max(v);
            n += 1;
            frames += 1;
            if n == PEAK_BUCKET {
                out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
                out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
                (lo, hi, n) = (0.0, 0.0, 0);
            }
        }
    })?;
    if n > 0 {
        out.push((lo.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
        out.push((hi.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
    }
    Ok((frames, sr, ch, out))
}

/// What one recorded source turned into.
pub struct RecOut {
    pub src: usize,
    pub key: String,
    pub path: PathBuf,
    pub samples: Vec<f32>,
    pub error: Option<String>,
}

/// The writer side of a recording: drains the audio thread's rings into 24-bit
/// WAV files until the audio thread lets go of them (recording stopped).
pub fn write_takes(sources: Vec<(usize, String, PathBuf, rtrb::Consumer<f32>)>, sr: u32) -> Vec<RecOut> {
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    struct W {
        out: RecOut,
        ring: rtrb::Consumer<f32>,
        wav: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    }
    let mut ws: Vec<W> = sources
        .into_iter()
        .map(|(src, key, path, ring)| {
            let (wav, error) = match hound::WavWriter::create(&path, spec) {
                Ok(w) => (Some(w), None),
                Err(e) => (None, Some(format!("can't write {}: {e}", path.display()))),
            };
            W { out: RecOut { src, key, path, samples: Vec::with_capacity(sr as usize * 60), error }, ring, wav }
        })
        .collect();
    loop {
        let mut done = true;
        for w in ws.iter_mut() {
            let abandoned = w.ring.is_abandoned(); // before draining, so nothing slips through
            let n = w.ring.slots();
            if n > 0 {
                if let Ok(chunk) = w.ring.read_chunk(n) {
                    let (a, b) = chunk.as_slices();
                    for &v in a.iter().chain(b) {
                        if let Some(wav) = w.wav.as_mut() {
                            if let Err(e) = wav.write_sample((v.clamp(-1.0, 1.0) * 8_388_607.0) as i32) {
                                w.out.error = Some(e.to_string());
                                w.wav = None;
                            }
                        }
                        w.out.samples.push(v);
                    }
                    chunk.commit_all();
                }
            }
            if !abandoned {
                done = false;
            }
        }
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ws.into_iter()
        .map(|mut w| {
            if let Some(wav) = w.wav.take() {
                if let Err(e) = wav.finalize() {
                    w.out.error = Some(e.to_string());
                }
            }
            w.out
        })
        .collect()
}

/// One song track as an export renders it.
pub struct TrackMix {
    pub slot: usize,
    pub s: StripSettings,
    pub rack: Vec<SlotCfg>,
    pub inst: Option<InstCfg>,
    /// also write this track on its own (after its fader and pan)
    pub stem: Option<PathBuf>,
}

/// Render timeline samples start..end to stereo 24-bit WAV files: the mix
/// (if `mix`) and any stems. The same mixer as playback (wiring, buses,
/// sends, delay compensation, the master rack and fader), with effect
/// instances of its own so the live audio keeps running. `done` counts
/// rendered frames. Returns warnings (effects that couldn't load).
#[allow(clippy::too_many_arguments)]
pub fn render(tl: &Timeline, tracks: Vec<TrackMix>, mut wiring: Box<Wiring>, master_rack: Vec<SlotCfg>, master: f32,
              sr: u32, start: i64, end: i64, mix: Option<PathBuf>, done: &AtomicU64) -> Result<Vec<String>, String> {
    const BLOCK: usize = 512;
    type Wav = hound::WavWriter<std::io::BufWriter<std::fs::File>>;
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    let create = |p: &Path| hound::WavWriter::create(p, spec).map_err(|e| format!("can't write {}: {e}", p.display()));
    let put = |w: &mut Wav, v: f32| w.write_sample((v.clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(|e| e.to_string());
    let mut warnings = Vec::new();
    let build_rack = |cfgs: Vec<SlotCfg>, what: &str, warnings: &mut Vec<String>| {
        let mut rack = Rack::new();
        for cfg in cfgs {
            match Slot::new(cfg, sr as f32) {
                Ok(slot) => {
                    let _ = rack.insert(MAX_TRACKS, slot);
                }
                Err(e) => warnings.push(format!("an effect on {what} was left out: {e}")),
            }
        }
        rack
    };
    let mut strips: Vec<Strip> = (0..MAX_TRACKS).map(|_| Strip::new(StripSettings::track())).collect();
    let mut stems: Vec<Option<Wav>> = (0..MAX_TRACKS).map(|_| None).collect();
    for t in tracks {
        let Some(strip) = strips.get_mut(t.slot) else { continue };
        strip.s = t.s;
        strip.rack = build_rack(t.rack, &format!("track {}", t.slot + 1), &mut warnings);
        strip.inst = match t.inst {
            Some(cfg) => match InstSlot::new(cfg, sr as f32) {
                Ok(i) => Some(i),
                Err(e) => {
                    warnings.push(format!("the instrument on track {} was left out: {e}", t.slot + 1));
                    None
                }
            },
            None => None,
        };
        if let Some(p) = &t.stem {
            stems[t.slot] = Some(create(p)?);
        }
    }
    let mut master_rack = build_rack(master_rack, "the master", &mut warnings);
    let mut mixw = match &mix {
        Some(p) => Some(create(p)?),
        None => None,
    };
    let mut bufs = TrackBufs::new();
    // run in early so slow plugins are full when `start` comes out of them, and
    // on past the end by the master rack's delay
    let preroll = (0..MAX_TRACKS).map(|i| wiring.ahead_notes[i].max(wiring.ahead_audio[i])).max().unwrap_or(0).max(0);
    let m_lat = wiring.master_lat.max(0);
    let (mut l, mut r) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let mut x = start - preroll;
    while x < end + m_lat {
        let n = (end + m_lat - x).min(BLOCK as i64) as usize;
        l[..n].fill(0.0);
        r[..n].fill(0.0);
        let play = Play { segs: [(0, x, n), (0, 0, 0)], nseg: 1, looping: None };
        apply_automation(tl, x, &mut strips, &mut master_rack);
        render_tracks(&mut strips, &mut wiring, &mut bufs, Some(tl), Some(&play), n, &mut l[..n], &mut r[..n]);
        // stems: the tracks as they leave their faders (no master rack: no delay)
        for (slot, w) in stems.iter_mut().enumerate() {
            let Some(w) = w.as_mut() else { continue };
            for k in 0..n {
                let at = x + k as i64;
                if at >= start && at < end {
                    put(w, bufs.post_l[slot][k])?;
                    put(w, bufs.post_r[slot][k])?;
                }
            }
        }
        master_rack.process(&mut l[..n], &mut r[..n], &|slot| bufs.key(slot, n));
        if let Some(w) = mixw.as_mut() {
            for k in 0..n {
                let at = x + k as i64 - m_lat; // what comes out of the master rack now
                if at >= start && at < end {
                    put(w, l[k] * master)?;
                    put(w, r[k] * master)?;
                }
            }
        }
        x += n as i64;
        done.store((x - start).clamp(0, end - start) as u64, Relaxed);
    }
    for w in stems.into_iter().flatten() {
        w.finalize().map_err(|e| e.to_string())?;
    }
    if let Some(w) = mixw {
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_wav_reader_matches_hound() {
        let dir = std::env::temp_dir().join(format!("duet-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (ch, bits, float) in [(1u16, 16u16, false), (2, 16, false), (2, 24, false), (1, 32, false), (2, 32, true), (3, 24, false)] {
            let p = dir.join(format!("t{ch}-{bits}-{float}.wav"));
            let spec = hound::WavSpec { channels: ch, sample_rate: 44100, bits_per_sample: bits,
                                        sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int } };
            let mut w = hound::WavWriter::create(&p, spec).unwrap();
            let n = 300_000; // more than one block
            for k in 0..n * ch as usize {
                let v = ((k as f32) * 0.001).sin() * 0.7;
                if float {
                    w.write_sample(v).unwrap();
                } else {
                    w.write_sample((v * ((1i64 << (bits - 1)) - 1) as f32) as i32).unwrap();
                }
            }
            w.finalize().unwrap();
            let a = read_wav(&p).unwrap();
            let mut slow = Vec::new();
            wav_blocks_hound(&p, |b, c| slow.extend(b.chunks_exact(c).map(|f| f[0]))).unwrap();
            assert_eq!(a.sr, 44100);
            assert_eq!(a.samples.len(), n, "{ch}ch {bits}bit");
            assert_eq!(a.right.is_some(), ch > 1);
            let worst = a.samples.iter().zip(&slow).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max);
            assert!(worst < 1e-6, "{ch}ch {bits}bit float={float}: off by {worst}");
            let (frames, sr, c, pk) = wav_peaks(&p).unwrap();
            assert_eq!((frames, sr, c), (n, 44100, ch as usize));
            assert_eq!(pk.len(), 2 * n.div_ceil(PEAK_BUCKET));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// DUET_BENCH_WAV=file.wav cargo test --release bench_load -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_load() {
        let p = std::env::var("DUET_BENCH_WAV").expect("DUET_BENCH_WAV");
        let t = std::time::Instant::now();
        let a = read_wav(Path::new(&p)).unwrap();
        println!("read {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let pk = audio_peaks(&a);
        println!("peaks {:?} ({})", t.elapsed(), pk.len());
    }

    #[test]
    fn parallel_resample_is_seamless() {
        let x: Vec<f32> = (0..400_000).map(|k| (k as f32 * 0.01).sin()).collect();
        let y = resample(&x, 44100, 48000);
        assert_eq!(y.len(), (400_000f64 * 48000.0 / 44100.0).floor() as usize);
        // a sine stays a sine across the stretches the cores did (no jumps)
        let step = 0.01 * 44100.0 / 48000.0;
        let worst = y.iter().enumerate().skip(2).take(y.len() - 8).map(|(k, v)| (v - (k as f32 * step).sin()).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-3, "{worst}");
    }

    #[test]
    fn automation_lines_between_points() {
        let lane = &auto_lanes(&[AutoDesc { target: AutoTarget::Pan(0), pts: vec![(1.0, 0.2), (0.0, 0.0), (2.0, 1.0)] }], 100)[0];
        assert_eq!(lane.pts[0], (0, 0.0)); // sorted
        assert_eq!(lane.value_at(-50), Some(0.0)); // flat before the first point
        assert!((lane.value_at(50).unwrap() - 0.1).abs() < 1e-6);
        assert!((lane.value_at(150).unwrap() - 0.6).abs() < 1e-6);
        assert_eq!(lane.value_at(500), Some(1.0)); // and after the last
        assert!(auto_lanes(&[AutoDesc { target: AutoTarget::Pan(0), pts: vec![] }], 100).is_empty());
        assert!((fader_gain(0.7937) - 1.0).abs() < 1e-3); // 0 dB where the fader has it
    }

    fn clip(start: i64, offset: i64, len: i64, data: &Arc<AudioData>) -> ClipRt {
        ClipRt { start, pos0: offset as f64, len, step: 1.0, gain: 1.0, fade_in: 0, fade_out: 0, data: data.clone() }
    }

    #[test]
    fn clips_play_at_their_place_with_offset_and_declick() {
        let data = Arc::new(AudioData::mono(48000, (0..1000).map(|i| i as f32).collect()));
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(100, 200, 300, &data));
        let render = |pos: i64| {
            let (mut l, mut r) = (vec![0f32; 128], vec![0f32; 128]);
            tl.render(0, pos, &mut l, &mut r);
            assert_eq!(l, r, "mono: the same on both sides");
            l
        };
        let out = render(0);
        assert!(out[..100].iter().all(|v| *v == 0.0));
        // first sample ramps in, sample 200 of the file lands on timeline 100
        assert!(out[100] > 0.0 && out[100] < 200.0);
        assert_eq!(render(200)[0], 300.0); // timeline 200 = file sample 300, past the ramp
        let out = render(380);
        assert!(out[19] > 0.0 && out[19] < 500.0, "ramps out at the end");
        assert_eq!(out[20], 0.0, "nothing after the clip");
        // a stereo file plays each side on its own side
        let st = Arc::new(AudioData { sr: 48000, samples: vec![0.25; 1000], right: Some(vec![-0.5; 1000]) });
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(0, 0, 1000, &st));
        let (mut l, mut r) = (vec![0f32; 128], vec![0f32; 128]);
        tl.render(0, 200, &mut l, &mut r);
        assert_eq!((l[0], r[0]), (0.25, -0.5));
    }

    #[test]
    fn build_converts_seconds_and_rates() {
        let mut cache = AudioCache::default();
        cache.insert("a", AudioData::mono(24000, vec![0.5; 24000]));
        let desc = vec![vec![ClipDesc::plain("a", 1.0, 0.0, 0.5)]];
        let tl = cache.build(&desc, &[], 48000);
        let c = &tl.tracks[0][0];
        // the audio stays at its own rate and is read at half speed
        assert_eq!((c.start, c.len, c.data.sr, c.step), (48000, 24000, 24000, 0.5));
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        tl.render(0, 48000 + 1000, &mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| (v - 0.5).abs() < 1e-5), "{:?}", &l[..4]);
        // a missing file just isn't there yet
        let desc = vec![vec![ClipDesc { key: "b".into(), ..desc[0][0].clone() }]];
        assert!(cache.build(&desc, &[], 48000).tracks[0].is_empty());
    }

    #[test]
    fn tape_speed_reverse_and_stretched_copies() {
        let mut cache = AudioCache::default();
        // a ramp: the value says where in the file it was read
        cache.insert("r", AudioData::mono(48000, (0..48000).map(|k| k as f32 / 48000.0).collect()));
        let read = |cache: &mut AudioCache, c: ClipDesc, at: i64| {
            let tl = cache.build(&[vec![c]], &[], 48000);
            let (mut l, mut r) = (vec![0f32; 1], vec![0f32; 1]);
            tl.render(0, at, &mut l, &mut r);
            l[0]
        };
        // twice as fast (tape): 0.1 s in, it's 0.2 s into the file (+ the 0.1 s offset)
        let c = ClipDesc { speed: 2.0, ..ClipDesc::plain("r", 0.0, 0.1, 0.4) };
        assert!((read(&mut cache, c, 4800) - 0.3).abs() < 1e-3);
        // an octave up in tape mode is twice as fast too
        let c = ClipDesc { pitch: 12.0, ..ClipDesc::plain("r", 0.0, 0.0, 0.4) };
        assert!((read(&mut cache, c, 4800) - 0.2).abs() < 1e-3);
        // backwards: the clip's stretch (0.1..0.5 s of the file) from its end
        let c = ClipDesc { reverse: true, ..ClipDesc::plain("r", 0.0, 0.1, 0.4) };
        assert!((read(&mut cache, c.clone(), 2400) - 0.45).abs() < 1e-3, "{}", read(&mut cache, c, 2400));
        // keep pitch: left out until the stretched copy is there, then read from it
        let c = ClipDesc { speed: 2.0, stretch: true, ..ClipDesc::plain("r", 0.0, 0.1, 0.4) };
        assert!(cache.build(&[vec![c.clone()]], &[], 48000).tracks[0].is_empty());
        assert_eq!(cache.missing, vec![StretchKey::new("r", 2.0, 0.0, 0)]);
        let half: Vec<f32> = (0..24000).map(|k| k as f32 / 24000.0).collect(); // (a stand-in for the stretched copy)
        cache.add_stretched(StretchKey::new("r", 2.0, 0.0, 0), AudioData::mono(48000, half));
        assert!((read(&mut cache, c.clone(), 4800) - 0.3).abs() < 1e-3);
        assert!(cache.missing.is_empty());
        // not used any more: let go
        cache.build(&[vec![ClipDesc::plain("r", 0.0, 0.0, 0.4)]], &[], 48000);
        assert!(cache.stretched.is_empty());
    }

    #[test]
    fn export_renders_the_mix_and_stems() {
        let dir = std::env::temp_dir().join(format!("duet-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = Arc::new(AudioData::mono(48000, vec![0.5; 4800]));
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(1000, 0, 4800, &data));
        tl.tracks[3].push(clip(0, 0, 4800, &data));
        let s = |pan: f32, mute: bool| StripSettings { active: true, pan, mute, ..StripSettings::track() };
        let tracks = vec![
            TrackMix { slot: 0, s: s(-1.0, false), rack: vec![], inst: None, stem: Some(dir.join("a.wav")) },
            TrackMix { slot: 3, s: s(0.0, true), rack: vec![SlotCfg::new(5)], inst: None, stem: Some(dir.join("b.wav")) },
        ];
        let done = AtomicU64::new(0);
        let warn = render(&tl, tracks, Wiring::plain(), vec![], 1.0, 48000, 0, 8000, Some(dir.join("mix.wav")), &done).unwrap();
        assert!(warn.is_empty());
        assert_eq!(done.load(Relaxed), 8000);
        let read = |n: &str| {
            let mut r = hound::WavReader::open(dir.join(n)).unwrap();
            assert_eq!((r.spec().channels, r.spec().sample_rate), (2, 48000));
            r.samples::<i32>().map(|v| v.unwrap() as f32 / 8_388_607.0).collect::<Vec<_>>()
        };
        let mix = read("mix.wav");
        assert_eq!(mix.len(), 16000);
        assert_eq!(mix[2 * 500], 0.0); // before the clip
        assert!((mix[2 * 3000] - 0.5).abs() < 1e-3, "hard left: all of it on the left");
        assert!(mix[2 * 3000 + 1].abs() < 1e-3, "nothing on the right; the muted track isn't in the mix");
        let b = read("b.wav");
        assert!(b[2 * 2000].abs() > 0.1, "a muted track still gets its own stem");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_lines_up_slow_plugins_buses_and_the_master() {
        // an impulse on track 0 (straight out) and on track 1 (through bus 5,
        // which has a limiter: a look-ahead delay), a limiter on the master too:
        // both impulses come out on the same sample, the one they're on
        use crate::mix::{Route, WireDesc};
        let dir = std::env::temp_dir().join(format!("duet-pdc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut one = vec![0f32; 2000];
        one[100] = 0.5; // (past the clip's de-click ramp)
        let data = Arc::new(AudioData::mono(48000, one));
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(900, 0, 2000, &data));
        tl.tracks[1].push(clip(2900, 0, 2000, &data));
        let s = StripSettings { active: true, ..StripSettings::track() };
        let mut lim = SlotCfg::new(crate::fx::LIMITER);
        lim.p[1] = 1.0;
        let tracks = vec![
            TrackMix { slot: 0, s, rack: vec![], inst: None, stem: None },
            TrackMix { slot: 1, s, rack: vec![], inst: None, stem: None },
            TrackMix { slot: 5, s, rack: vec![lim.clone()], inst: None, stem: None },
        ];
        let mut routes = vec![crate::mix::Route::default(); MAX_TRACKS];
        routes[1] = Route { out: Some(5), sends: vec![] };
        let la = crate::fx::Limiter::lookahead(48000.0) as u32;
        let mut rack_lat = [0; MAX_TRACKS];
        rack_lat[5] = la;
        let mut active = [false; MAX_TRACKS];
        for i in [0, 1, 5] {
            active[i] = true;
        }
        let w = Wiring::build(&WireDesc { active, routes: &routes, rack_lat, inst_lat: [0; MAX_TRACKS], keys: vec![], master_lat: la, max_pad: 48000 });
        let done = AtomicU64::new(0);
        render(&tl, tracks, w, vec![lim], 1.0, 48000, 0, 6000, Some(dir.join("mix.wav")), &done).unwrap();
        let mix: Vec<f32> = hound::WavReader::open(dir.join("mix.wav")).unwrap().samples::<i32>()
            .map(|v| v.unwrap() as f32 / 8_388_607.0).step_by(2).collect();
        assert_eq!(mix.len(), 6000);
        let hits: Vec<usize> = (0..mix.len()).filter(|&k| mix[k].abs() > 0.05).collect();
        assert_eq!(hits, vec![1000, 3000], "{hits:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Clicks once on the first sample after each note on.
    struct Clicker(bool);
    impl crate::instr::Instrument for Clicker {
        fn note(&mut self, ev: NoteEv) {
            if let NoteEv::On(..) = ev {
                self.0 = true;
            }
        }
        fn process(&mut self, _p: &[f32; crate::instr::N_PARAMS], out: &mut [f32], r: &mut [f32]) {
            out.fill(0.0);
            if std::mem::take(&mut self.0) && !out.is_empty() {
                out[0] = 1.0;
            }
            r.copy_from_slice(out);
        }
    }

    #[test]
    fn notes_play_on_their_exact_sample() {
        let mut cache = AudioCache::default();
        let notes = vec![vec![], vec![
            NoteDesc { t: 0.0, dur: 0.1, key: 60, vel: 100 },
            NoteDesc { t: 300.0 / 48000.0, dur: 0.01, key: 62, vel: 100 },
            NoteDesc { t: 1000.0 / 48000.0, dur: 0.01, key: 64, vel: 100 },
        ]];
        let tl = cache.build(&[], &notes, 48000);
        assert_eq!(tl.notes[1].len(), 6);
        assert_eq!(tl.notes_in(1, 256, 384).iter().filter(|n| n.on).count(), 1);
        let mut inst = InstSlot { cfg: InstCfg::new(crate::instr::SYNTH), inst: Box::new(Clicker(false)) };
        let mut clicks = vec![];
        let (mut buf, mut right) = ([0f32; 128], [0f32; 128]);
        for b in 0..10i64 {
            render_inst(&tl, 1, &mut inst, b * 128, &mut buf, &mut right);
            clicks.extend(buf.iter().enumerate().filter(|(_, v)| **v > 0.5).map(|(k, _)| b * 128 + k as i64));
        }
        assert_eq!(clicks, vec![0, 300, 1000]);
    }

    #[test]
    fn takes_round_trip_through_wav() {
        let dir = std::env::temp_dir().join(format!("duet-daw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("take.wav");
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(4096);
        let p2 = path.clone();
        let writer = std::thread::spawn(move || write_takes(vec![(0, "take".into(), p2, rx)], 48000));
        for i in 0..3000 {
            while tx.push((i as f32 / 3000.0) - 0.5).is_err() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        drop(tx); // recording stopped
        let out = writer.join().unwrap();
        assert!(out[0].error.is_none());
        assert_eq!(out[0].samples.len(), 3000);
        let back = read_wav(&path).unwrap();
        assert_eq!((back.sr, back.samples.len()), (48000, 3000));
        assert!((back.samples[1500] - out[0].samples[1500]).abs() < 1e-6);
        assert_eq!(peaks(&back.samples).len(), 2 * 6);
        std::fs::remove_dir_all(&dir).ok();
    }
}
