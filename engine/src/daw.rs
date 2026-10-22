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

use crate::dsp::{pan_gains, StripSettings};
use crate::fx::{Rack, Slot, SlotCfg};

pub const MAX_TRACKS: usize = 32;
/// Samples per waveform overview bucket (one min/max pair each).
pub const PEAK_BUCKET: usize = 512;
/// Short ramps at every clip edge, so a cut never clicks.
const DECLICK: i64 = 32;

/// One audio file, mono, at its own sample rate.
pub struct AudioData {
    pub sr: u32,
    pub samples: Vec<f32>,
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

/// Every track's clips, sorted by start. Index = track slot.
pub struct Timeline {
    pub tracks: Vec<Vec<ClipRt>>,
}

impl Timeline {
    pub fn empty() -> Self {
        Timeline { tracks: (0..MAX_TRACKS).map(|_| Vec::new()).collect() }
    }

    /// Add track `t`'s audio for timeline samples pos..pos+out.len() into `out`.
    pub fn render(&self, t: usize, pos: i64, out: &mut [f32]) {
        let n = out.len() as i64;
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
                out[(x - pos) as usize] += src[si as usize] * g;
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
        let c = Arc::new(AudioData { sr, samples: resample(&a.samples, a.sr, sr) });
        self.converted.insert(k, c.clone());
        Some(c)
    }

    /// Build the audio thread's timeline from the project's description.
    pub fn build(&mut self, desc: &[Vec<ClipDesc>], sr: u32) -> Timeline {
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

/// Any PCM or float WAV, mixed down to mono.
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
    let samples = if ch == 1 {
        inter
    } else {
        inter.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect()
    };
    Ok(AudioData { sr: spec.sample_rate, samples })
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
    /// also write this track on its own (after its fader and pan)
    pub stem: Option<PathBuf>,
}

/// Render timeline samples start..end to stereo 24-bit WAV files: the mix
/// (if `mix`) and any stems. Same clips, gains, pans and effects as playback,
/// but with effect instances of its own, so the live audio keeps running.
/// `done` counts rendered frames. Returns warnings (effects that couldn't load).
#[allow(clippy::too_many_arguments)]
pub fn render(tl: &Timeline, tracks: Vec<TrackMix>, master: f32, sr: u32, start: i64, end: i64,
              mix: Option<PathBuf>, done: &AtomicU64) -> Result<Vec<String>, String> {
    const BLOCK: usize = 512;
    type Wav = hound::WavWriter<std::io::BufWriter<std::fs::File>>;
    let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    let create = |p: &Path| hound::WavWriter::create(p, spec).map_err(|e| format!("can't write {}: {e}", p.display()));
    let put = |w: &mut Wav, v: f32| w.write_sample((v.clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(|e| e.to_string());
    struct T {
        slot: usize,
        s: StripSettings,
        rack: Rack,
        stem: Option<Wav>,
        buf: Vec<f32>,
    }
    let mut warnings = Vec::new();
    let mut ts = Vec::new();
    for t in tracks {
        let mut rack = Rack::new();
        for cfg in t.rack {
            match Slot::new(cfg, sr as f32) {
                Ok(slot) => {
                    let _ = rack.insert(MAX_TRACKS, slot);
                }
                Err(e) => warnings.push(format!("an effect on track {} was left out: {e}", t.slot + 1)),
            }
        }
        let stem = match &t.stem {
            Some(p) => Some(create(p)?),
            None => None,
        };
        ts.push(T { slot: t.slot, s: t.s, rack, stem, buf: vec![0.0; BLOCK] });
    }
    let mut mixw = match &mix {
        Some(p) => Some(create(p)?),
        None => None,
    };
    let any_solo = ts.iter().any(|t| t.s.solo);
    let (mut l, mut r) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
    let mut pos = start;
    while pos < end {
        let n = (end - pos).min(BLOCK as i64) as usize;
        l[..n].fill(0.0);
        r[..n].fill(0.0);
        for t in ts.iter_mut() {
            let buf = &mut t.buf[..n];
            buf.fill(0.0);
            tl.render(t.slot, pos, buf);
            for v in buf.iter_mut() {
                *v *= t.s.gain;
            }
            t.rack.process(buf);
            let (pl, pr) = pan_gains(t.s.pan);
            let f = t.s.fader;
            if let Some(w) = t.stem.as_mut() {
                for &v in buf.iter() {
                    put(w, v * f * pl)?;
                    put(w, v * f * pr)?;
                }
            }
            if !t.s.mute && (!any_solo || t.s.solo) {
                for k in 0..n {
                    l[k] += buf[k] * f * pl;
                    r[k] += buf[k] * f * pr;
                }
            }
        }
        if let Some(w) = mixw.as_mut() {
            for k in 0..n {
                put(w, l[k] * master)?;
                put(w, r[k] * master)?;
            }
        }
        pos += n as i64;
        done.store((pos - start) as u64, Relaxed);
    }
    for t in ts {
        if let Some(w) = t.stem {
            w.finalize().map_err(|e| e.to_string())?;
        }
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
        let data = Arc::new(AudioData { sr: 48000, samples: (0..1000).map(|i| i as f32).collect() });
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(100, 200, 300, &data));
        let mut out = vec![0f32; 128];
        tl.render(0, 0, &mut out);
        assert!(out[..100].iter().all(|v| *v == 0.0));
        // first sample ramps in, sample 200 of the file lands on timeline 100
        assert!(out[100] > 0.0 && out[100] < 200.0);
        let mut out = vec![0f32; 128];
        tl.render(0, 200, &mut out);
        assert_eq!(out[0], 300.0); // timeline 200 = file sample 300, past the ramp
        let mut out = vec![0f32; 128];
        tl.render(0, 380, &mut out);
        assert!(out[19] > 0.0 && out[19] < 500.0, "ramps out at the end");
        assert_eq!(out[20], 0.0, "nothing after the clip");
    }

    #[test]
    fn build_converts_seconds_and_rates() {
        let mut cache = AudioCache::default();
        cache.insert("a", AudioData { sr: 24000, samples: vec![0.5; 24000] });
        let desc = vec![vec![ClipDesc { key: "a".into(), start: 1.0, offset: 0.0, len: 0.5, gain: 1.0, fade_in: 0.0, fade_out: 0.0 }]];
        let tl = cache.build(&desc, 48000);
        let c = &tl.tracks[0][0];
        assert_eq!((c.start, c.len, c.data.sr), (48000, 24000, 48000));
        assert!((c.data.samples.len() as i64 - 48000).abs() <= 1);
        // a missing file just isn't there yet
        let desc = vec![vec![ClipDesc { key: "b".into(), ..desc[0][0].clone() }]];
        assert!(cache.build(&desc, 48000).tracks[0].is_empty());
    }

    #[test]
    fn export_renders_the_mix_and_stems() {
        let dir = std::env::temp_dir().join(format!("duet-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = Arc::new(AudioData { sr: 48000, samples: vec![0.5; 4800] });
        let mut tl = Timeline::empty();
        tl.tracks[0].push(clip(1000, 0, 4800, &data));
        tl.tracks[3].push(clip(0, 0, 4800, &data));
        let s = |pan: f32, mute: bool| StripSettings { active: true, pan, mute, ..StripSettings::track() };
        let tracks = vec![
            TrackMix { slot: 0, s: s(-1.0, false), rack: vec![], stem: Some(dir.join("a.wav")) },
            TrackMix { slot: 3, s: s(0.0, true), rack: vec![SlotCfg::new(5)], stem: Some(dir.join("b.wav")) },
        ];
        let done = AtomicU64::new(0);
        let warn = render(&tl, tracks, 1.0, 48000, 0, 8000, Some(dir.join("mix.wav")), &done).unwrap();
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
