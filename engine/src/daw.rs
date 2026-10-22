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
use crate::mix::{render_tracks, Play, Strip, TrackBufs, Wiring};

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
    pub start: i64,  // timeline position of the clip's first sample
    pub offset: i64, // where in the audio that first sample is
    pub len: i64,
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
}

impl Timeline {
    pub fn empty() -> Self {
        Timeline { tracks: (0..MAX_TRACKS).map(|_| Vec::new()).collect(), notes: (0..MAX_TRACKS).map(|_| Vec::new()).collect() }
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
            for x in a..b {
                let rel = x - c.start;
                let si = c.offset + rel;
                if si < 0 || si as usize >= src.len() {
                    continue;
                }
                let mut g = c.gain;
                let left = (c.len - rel) as f32;
                if (rel as f32) < fi {
                    g *= (rel as f32 + 0.5) / fi;
                }
                if left <= fo {
                    g *= (left - 0.5) / fo;
                }
                let k = (x - pos) as usize;
                l[k] += src[si as usize] * g;
                r[k] += src_r[si as usize] * g;
            }
        }
    }
}

/// A clip as the project describes it, in seconds.
#[derive(Clone, Debug)]
pub struct ClipDesc {
    pub key: String,
    pub start: f64,
    pub offset: f64,
    pub len: f64,
    pub gain: f32,
    pub fade_in: f64,
    pub fade_out: f64,
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

/// Loaded audio, by file key, plus copies converted to the device rate.
#[derive(Default)]
pub struct AudioCache {
    files: HashMap<String, Arc<AudioData>>,
    converted: HashMap<(String, u32), Arc<AudioData>>,
}

impl AudioCache {
    pub fn insert(&mut self, key: &str, data: AudioData) -> Arc<AudioData> {
        self.converted.retain(|(k, _), _| k != key);
        let a = Arc::new(data);
        self.files.insert(key.to_string(), a.clone());
        a
    }

    pub fn get(&self, key: &str) -> Option<&Arc<AudioData>> {
        self.files.get(key)
    }

    pub fn remove(&mut self, key: &str) {
        self.files.remove(key);
        self.converted.retain(|(k, _), _| k != key);
    }

    /// The file at `sr` (converted once and kept).
    pub fn at_rate(&mut self, key: &str, sr: u32) -> Option<Arc<AudioData>> {
        let a = self.files.get(key)?;
        if a.sr == sr {
            return Some(a.clone());
        }
        let k = (key.to_string(), sr);
        if let Some(c) = self.converted.get(&k) {
            return Some(c.clone());
        }
        let c = Arc::new(AudioData { sr, samples: resample(&a.samples, a.sr, sr), right: a.right.as_ref().map(|r| resample(r, a.sr, sr)) });
        self.converted.insert(k, c.clone());
        Some(c)
    }

    /// Build the audio thread's timeline from the project's description.
    pub fn build(&mut self, desc: &[Vec<ClipDesc>], notes: &[Vec<NoteDesc>], sr: u32) -> Timeline {
        let s = |secs: f64| (secs * sr as f64).round() as i64;
        let mut tl = Timeline::empty();
        for (t, clips) in desc.iter().enumerate().take(MAX_TRACKS) {
            for c in clips {
                let Some(data) = self.at_rate(&c.key, sr) else { continue }; // not here (yet)
                let len = s(c.len);
                if len <= 0 {
                    continue;
                }
                tl.tracks[t].push(ClipRt {
                    start: s(c.start),
                    offset: s(c.offset),
                    len,
                    gain: c.gain,
                    fade_in: s(c.fade_in),
                    fade_out: s(c.fade_out),
                    data,
                });
            }
            tl.tracks[t].sort_by_key(|c| c.start);
        }
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
    let at = |i: isize| -> f32 { x[i.clamp(0, x.len() as isize - 1) as usize] };
    (0..n)
        .map(|k| {
            let p = k as f64 * step;
            let i = p.floor() as isize;
            let t = (p - i as f64) as f32;
            let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            ((a * t + b) * t + c) * t + y1
        })
        .collect()
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
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let inter: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(8, 32) - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
    };
    if ch == 1 {
        return Ok(AudioData::mono(spec.sample_rate, inter));
    }
    let l = inter.chunks_exact(ch).map(|f| f[0]).collect();
    let r = inter.chunks_exact(ch).map(|f| f[1]).collect();
    Ok(AudioData { sr: spec.sample_rate, samples: l, right: Some(r) })
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

    fn clip(start: i64, offset: i64, len: i64, data: &Arc<AudioData>) -> ClipRt {
        ClipRt { start, offset, len, gain: 1.0, fade_in: 0, fade_out: 0, data: data.clone() }
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
        let desc = vec![vec![ClipDesc { key: "a".into(), start: 1.0, offset: 0.0, len: 0.5, gain: 1.0, fade_in: 0.0, fade_out: 0.0 }]];
        let tl = cache.build(&desc, &[], 48000);
        let c = &tl.tracks[0][0];
        assert_eq!((c.start, c.len, c.data.sr), (48000, 24000, 48000));
        assert!((c.data.samples.len() as i64 - 48000).abs() <= 1);
        // a missing file just isn't there yet
        let desc = vec![vec![ClipDesc { key: "b".into(), ..desc[0][0].clone() }]];
        assert!(cache.build(&desc, &[], 48000).tracks[0].is_empty());
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
