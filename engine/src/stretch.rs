//! Time-stretch and pitch-shift for clips (Signalsmith Stretch), and the
//! analysis the clip editor uses: a region's peak, where its hits are
//! (transients), and its tempo.
//!
//! Stretching runs offline on the control side, in the background: a whole
//! file at one setting at a time, kept while a clip uses it.

use ssstretch::Stretch;

use crate::daw::AudioData;

/// `a` at `speed` (2 = twice as fast, half as long) and `semis` semitones
/// higher, the pitch and the timing independent of each other.
pub fn stretch(a: &AudioData, speed: f64, semis: f32) -> AudioData {
    let ch = if a.right.is_some() { 2 } else { 1 };
    let n_in = a.samples.len();
    let n_out = ((n_in as f64) / speed).round().max(1.0) as usize;
    let mut st = Stretch::new();
    st.preset_default(ch as i32, a.sr as f32);
    st.set_transpose_semitones(semis, None);
    let inputs: Vec<&[f32]> = std::iter::once(&a.samples[..]).chain(a.right.as_deref()).collect();
    let in_lat = st.input_latency().max(0) as f64;
    let out_lat = st.output_latency().max(0) as usize;
    // what comes out is late by the stretcher's own delay: the output
    // latency plus the input latency at the new speed. Make that much more
    // and drop it from the front, so a hit lands where it belongs.
    let pad_in = in_lat as usize;
    let pad_out = (in_lat / speed).round() as usize;
    let delay = out_lat + pad_out;
    let mut outs: Vec<Vec<f32>> = (0..ch).map(|_| vec![0f32; n_out + delay]).collect();
    let zeros = vec![0f32; pad_in.max(1)];
    unsafe {
        // the file in pieces at exactly `speed` (input per output), so a long
        // file doesn't go in as one huge call
        const BLOCK: usize = 1 << 16;
        let (mut done_in, mut done_out) = (0usize, 0usize);
        while done_out < n_out {
            let o = BLOCK.min(n_out - done_out);
            let i_end = (((done_out + o) as f64 / n_out as f64) * n_in as f64).round() as usize;
            let i = i_end.saturating_sub(done_in).min(n_in - done_in);
            let ip: Vec<*const f32> = inputs.iter().map(|x| x.as_ptr().add(done_in)).collect();
            let mut op: Vec<*mut f32> = outs.iter_mut().map(|x| x.as_mut_ptr().add(done_out)).collect();
            st.process(&ip, i as i32, &mut op, o as i32);
            done_in += i;
            done_out += o;
        }
        // then silence at the same speed, to push the end of the file through
        let ip: Vec<*const f32> = (0..ch).map(|_| zeros.as_ptr()).collect();
        let mut op: Vec<*mut f32> = outs.iter_mut().map(|x| x.as_mut_ptr().add(n_out)).collect();
        st.process(&ip, pad_in as i32, &mut op, pad_out as i32);
        let mut op: Vec<*mut f32> = outs.iter_mut().map(|x| x.as_mut_ptr().add(n_out + pad_out)).collect();
        st.flush(&mut op, out_lat as i32);
    }
    for o in outs.iter_mut() {
        o.drain(..delay);
    }
    let mut it = outs.into_iter();
    let l = it.next().unwrap_or_default();
    AudioData { sr: a.sr, samples: l, right: it.next() }
}

/// The loudest sample in frames a..b (both sides).
pub fn peak(a: &AudioData, from: usize, to: usize) -> f32 {
    let to = to.min(a.samples.len());
    let from = from.min(to);
    let m = |x: &[f32]| x[from..to].iter().fold(0f32, |m, v| m.max(v.abs()));
    m(&a.samples).max(a.right.as_deref().map_or(0.0, m))
}

const HOP: usize = 256;

/// Loudness per HOP frames (dB), of both sides mixed, frames a..b.
fn envelope(a: &AudioData, from: usize, to: usize) -> Vec<f32> {
    let to = to.min(a.samples.len());
    let from = from.min(to);
    let r = a.right.as_deref().unwrap_or(&a.samples);
    (from..to)
        .step_by(HOP)
        .map(|s| {
            let e = (s + HOP).min(to);
            let sum: f32 = (s..e).map(|k| {
                let v = 0.5 * (a.samples[k] + r[k]);
                v * v
            }).sum();
            10.0 * (sum / (e - s).max(1) as f32 + 1e-10).log10()
        })
        .collect()
}

/// How sharply the sound gets louder, per hop (dB of rise).
fn onsets(env: &[f32]) -> Vec<f32> {
    (0..env.len()).map(|i| if i >= 2 { (env[i] - env[i - 2]).max(0.0) } else { 0.0 }).collect()
}

/// Where the hits are in frames a..b: positions in frames from the start of
/// the file. `sensitivity` 0..1: higher finds softer hits.
pub fn transients(a: &AudioData, from: usize, to: usize, sensitivity: f32) -> Vec<usize> {
    let env = envelope(a, from, to);
    let odf = onsets(&env);
    let rise = 14.0 - 11.0 * sensitivity.clamp(0.0, 1.0); // dB
    let gap = (0.06 * a.sr as f32 / HOP as f32).ceil() as usize; // 60 ms between hits at least
    let mut out: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    let mut i = 0;
    while i < odf.len() {
        // the top of a rise that's big enough, out of silence-level noise
        if odf[i] >= rise && env[i] > -55.0 && last.is_none_or(|l| i >= l + gap) {
            let mut j = i;
            while j + 1 < odf.len() && odf[j + 1] > odf[j] {
                j += 1;
            }
            // the hit starts where it was still quiet: a little before the rise
            let at = from + i.saturating_sub(2) * HOP;
            let k = refine(a, at, from + (i + 1) * HOP);
            out.push(k);
            last = Some(j);
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// The quietest sample just before the attack in frames a..b: a cut there doesn't click.
fn refine(a: &AudioData, from: usize, to: usize) -> usize {
    let to = to.min(a.samples.len());
    let peak_at = (from..to).max_by(|&x, &y| a.samples[x].abs().total_cmp(&a.samples[y].abs())).unwrap_or(from);
    (from..=peak_at).rev().take(HOP * 2).min_by(|&x, &y| a.samples[x].abs().total_cmp(&a.samples[y].abs())).unwrap_or(from)
}

/// The tempo of frames a..b in BPM (70..180), or None when there's no beat
/// to speak of (or too little of it).
pub fn tempo(a: &AudioData, from: usize, to: usize) -> Option<f64> {
    let env = envelope(a, from, to);
    let odf = onsets(&env);
    let hops_per_sec = a.sr as f64 / HOP as f64;
    if (odf.len() as f64) < 4.0 * hops_per_sec {
        return None;
    }
    let mean = odf.iter().sum::<f32>() / odf.len() as f32;
    let x: Vec<f32> = odf.iter().map(|v| v - mean).collect();
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy <= 1e-6 {
        return None;
    }
    let score = |bpm: f64| -> f64 {
        // autocorrelation at the beat's lag (interpolated), with its double for support
        let ac = |lag: f64| -> f64 {
            let l = lag.floor() as usize;
            let t = lag - l as f64;
            if l + 1 >= x.len() {
                return 0.0;
            }
            let at = |l: usize| x.iter().zip(&x[l..]).map(|(a, b)| (a * b) as f64).sum::<f64>();
            (1.0 - t) * at(l) + t * at(l + 1)
        };
        let lag = 60.0 / bpm * hops_per_sec;
        let s = ac(lag) + 0.5 * ac(2.0 * lag) + 0.25 * ac(lag / 2.0);
        // a slight preference for the tempos songs usually have
        let pref = (-((bpm / 120.0).log2()).powi(2) / (2.0 * 0.5f64.powi(2))).exp();
        s * (0.6 + 0.4 * pref)
    };
    let mut best = (0.0, f64::MIN);
    let mut bpm = 70.0;
    while bpm <= 180.0 {
        let s = score(bpm);
        if s > best.1 {
            best = (bpm, s);
        }
        bpm += 0.5;
    }
    // finer around the best
    let (lo, hi) = (best.0 - 0.5, best.0 + 0.5);
    let mut b = lo;
    while b <= hi {
        let s = score(b);
        if s > best.1 {
            best = (b, s);
        }
        b += 0.05;
    }
    (best.1 > 0.05 * energy as f64).then_some((best.0 * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freq(x: &[f32], sr: f32) -> f32 {
        let c = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        c as f32 / 2.0 / (x.len() as f32 / sr)
    }

    #[test]
    fn stretch_changes_length_and_pitch_apart() {
        let sr = 44100;
        let x: Vec<f32> = (0..sr * 2).map(|k| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * k as f32 / sr as f32).sin()).collect();
        let a = AudioData::mono(sr as u32, x);
        let fast = stretch(&a, 2.0, 0.0);
        assert!((fast.samples.len() as i64 - sr as i64).abs() <= 1, "{}", fast.samples.len());
        let mid = &fast.samples[sr / 4..3 * sr / 4];
        assert!((freq(mid, sr as f32) - 440.0).abs() < 10.0, "{}", freq(mid, sr as f32));
        let up = stretch(&a, 1.0, 12.0);
        assert_eq!(up.samples.len(), 2 * sr);
        let mid = &up.samples[sr / 2..3 * sr / 2];
        assert!((freq(mid, sr as f32) - 880.0).abs() < 20.0, "{}", freq(mid, sr as f32));
        // lined up: a hit lands where it was (at the new speed)
        let mut x = vec![0f32; sr * 2];
        for k in 0..2000 {
            x[sr + k] = 0.8 * (-(k as f32) / 300.0).exp() * (k as f32 * 0.3).sin();
        }
        let a = AudioData::mono(sr as u32, x);
        // the hit's centre of energy (a stretcher smears an attack a little either way)
        let at = |x: &[f32]| {
            let (mut m, mut e) = (0f64, 0f64);
            for (k, v) in x.iter().enumerate() {
                m += k as f64 * (v * v) as f64;
                e += (v * v) as f64;
            }
            (m / e) as i64
        };
        let centre = at(&a.samples);
        for (speed, semis) in [(1.0, 0.0), (1.0, 3.0), (2.0, 3.0), (0.5, -5.0)] {
            let want = (centre as f64 / speed) as i64;
            let got = at(&stretch(&a, speed, semis).samples);
            assert!((got - want).abs() < 400, "speed {speed}, {semis} st: hit at {got}, want {want}");
        }
    }

    /// Clicks every `every` seconds.
    fn clicks(sr: usize, secs: f64, every: f64) -> AudioData {
        let mut x = vec![0f32; (sr as f64 * secs) as usize];
        let mut t = 0.25;
        while t < secs - 0.1 {
            let s = (t * sr as f64) as usize;
            for k in 0..400 {
                x[s + k] = 0.8 * (-(k as f32) / 80.0).exp() * if k % 2 == 0 { 1.0 } else { -1.0 };
            }
            t += every;
        }
        AudioData::mono(sr as u32, x)
    }

    #[test]
    fn finds_hits_and_tempo() {
        let sr = 44100;
        let a = clicks(sr, 8.0, 0.5); // 120 BPM
        let hits = transients(&a, 0, a.samples.len(), 0.5);
        assert_eq!(hits.len(), 16, "{hits:?}");
        for (i, h) in hits.iter().enumerate() {
            let want = ((0.25 + 0.5 * i as f64) * sr as f64) as i64;
            assert!((*h as i64 - want).abs() < 300, "hit {i} at {h}, want {want}");
        }
        let bpm = tempo(&a, 0, a.samples.len()).unwrap();
        assert!((bpm - 120.0).abs() < 1.0, "{bpm}");
        let b = clicks(sr, 10.0, 60.0 / 93.0);
        let bpm = tempo(&b, 0, b.samples.len()).unwrap();
        assert!((bpm - 93.0).abs() < 1.0, "{bpm}");
        assert!(tempo(&AudioData::mono(sr as u32, vec![0.0; sr * 6]), 0, sr * 6).is_none());
        assert_eq!(peak(&a, 0, a.samples.len()), 0.8);
    }
}
