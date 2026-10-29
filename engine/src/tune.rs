//! Tuning a voice, Melodyne-style (the clip editor's Tune tab).
//!
//! - `track`: the pitch of a single voice over time (YIN, a few ms a step),
//!   and how loud it is.
//! - `notes`: that line cut into notes, each with its centre (what the ear
//!   hears as its pitch: the middle of it, not the scoop into it).
//! - `chroma`: how much of each of the 12 pitch classes a mix has (for
//!   finding the song's key).
//! - `render`: the file with some notes moved (and their drift smoothed), by
//!   PSOLA: the voice cut at its own periods and laid down again at the new
//!   pitch. That keeps the voice's character (its formants) and its timing,
//!   and leaves everything that isn't moved exactly as it was.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;

use crate::daw::AudioData;

/// a pitch every HOP samples (5.8 ms at 44.1 kHz)
pub const HOP: usize = 256;
const WIN: usize = 2048;
const F0_MIN: f32 = 60.0;
const F0_MAX: f32 = 1100.0;

pub struct Track {
    pub sr: u32,
    /// frame k is at sample `from + k * HOP`
    pub from: usize,
    /// MIDI note number (fractional), NaN where there's no clear pitch
    pub midi: Vec<f32>,
    /// loudness (RMS) per frame
    pub rms: Vec<f32>,
}

impl Track {
    pub fn secs(&self, k: usize) -> f64 {
        (self.from + k * HOP) as f64 / self.sr as f64
    }
}

fn mono(a: &AudioData, from: usize, to: usize) -> Vec<f32> {
    match &a.right {
        Some(r) => (from..to).map(|k| 0.5 * (a.samples[k] + r[k])).collect(),
        None => a.samples[from..to].to_vec(),
    }
}

/// The pitch of a single voice in frames from..to (YIN: the difference
/// function through the FFT, its cumulative mean normalised, the first dip
/// under the threshold, refined to a fraction of a sample).
pub fn track(a: &AudioData, from: usize, to: usize) -> Track {
    let to = to.min(a.samples.len());
    let from = from.min(to);
    let x = mono(a, from, to);
    let sr = a.sr as f32;
    let tau_min = (sr / F0_MAX).floor() as usize;
    let tau_max = ((sr / F0_MIN).ceil() as usize).min(WIN - 2);
    let n_fft = (WIN + tau_max).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);
    let ifft = planner.plan_fft_inverse(n_fft);
    let frames = x.len().div_ceil(HOP);
    // prefix sums of squares, for the energy terms
    let mut sq = vec![0f64; x.len() + 1];
    for (k, v) in x.iter().enumerate() {
        sq[k + 1] = sq[k] + (*v as f64) * (*v as f64);
    }
    let energy = |a: usize, b: usize| -> f64 { sq[b.min(x.len())] - sq[a.min(x.len())] };
    let peak = x.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let mut buf_a = vec![0f32; n_fft];
    let mut buf_b = vec![0f32; n_fft];
    let mut spec_a = fft.make_output_vec();
    let mut spec_b = fft.make_output_vec();
    let mut corr = vec![0f32; n_fft];
    let mut d = vec![0f32; tau_max + 1];
    let mut midi = vec![f32::NAN; frames];
    let mut rms = vec![0f32; frames];
    for f in 0..frames {
        // the frame's window starts half a window before its centre
        let c = f * HOP;
        let s0 = c.saturating_sub(WIN / 2);
        let e = energy(s0, s0 + WIN);
        rms[f] = (e / WIN as f64).sqrt() as f32;
        if rms[f] < peak * 0.003 {
            continue; // silence (-50 dB under the clip's loudest)
        }
        buf_a.fill(0.0);
        buf_b.fill(0.0);
        for k in 0..WIN {
            buf_a[k] = *x.get(s0 + k).unwrap_or(&0.0);
        }
        for k in 0..WIN + tau_max {
            buf_b[k] = *x.get(s0 + k).unwrap_or(&0.0);
        }
        fft.process(&mut buf_a, &mut spec_a).expect("fft");
        fft.process(&mut buf_b, &mut spec_b).expect("fft");
        for (p, q) in spec_a.iter_mut().zip(&spec_b) {
            *p = p.conj() * q;
        }
        ifft.process(&mut spec_a, &mut corr).expect("fft");
        let norm = 1.0 / n_fft as f32;
        // d(tau) = e(0) + e(tau) - 2 r(tau); cumulative mean normalised
        let e0 = energy(s0, s0 + WIN);
        let mut run = 0f32;
        d[0] = 1.0;
        for tau in 1..=tau_max {
            let et = energy(s0 + tau, s0 + tau + WIN);
            let raw = (e0 + et) as f32 - 2.0 * corr[tau] * norm;
            run += raw.max(0.0);
            d[tau] = if run > 0.0 { raw.max(0.0) * tau as f32 / run } else { 1.0 };
        }
        let mut best = None;
        let mut tau = tau_min.max(2);
        while tau < tau_max {
            if d[tau] < 0.15 {
                while tau + 1 < tau_max && d[tau + 1] < d[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        if best.is_none() {
            // no dip under the threshold: the lowest, if it's still clear enough
            let (t, v) = (tau_min.max(2)..tau_max).map(|t| (t, d[t])).fold((0, f32::MAX), |b, x| if x.1 < b.1 { x } else { b });
            if v < 0.3 {
                best = Some(t);
            }
        }
        let Some(t) = best else { continue };
        // a parabola through the dip
        let (y0, y1, y2) = (d[t - 1], d[t], d[t + 1]);
        let den = y0 - 2.0 * y1 + y2;
        let shift = if den.abs() > 1e-9 { 0.5 * (y0 - y2) / den } else { 0.0 };
        let f0 = sr / (t as f32 + shift.clamp(-1.0, 1.0));
        midi[f] = 69.0 + 12.0 * (f0 / 440.0).log2();
    }
    // a lone frame an octave off its neighbours is a mistake: put it back
    for f in 1..frames.saturating_sub(1) {
        let (p, c, n) = (midi[f - 1], midi[f], midi[f + 1]);
        if p.is_finite() && n.is_finite() && c.is_finite() && (p - n).abs() < 1.0 {
            let mid = 0.5 * (p + n);
            if (c - mid).abs() > 9.0 {
                midi[f] = mid;
            }
        }
        if !c.is_finite() && p.is_finite() && n.is_finite() && (p - n).abs() < 1.0 {
            midi[f] = 0.5 * (p + n); // a one-frame gap in a held note
        }
    }
    Track { sr: a.sr, from, midi, rms }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// seconds in the file
    pub start: f64,
    pub end: f64,
    /// MIDI number: what the ear hears as its pitch
    pub center: f32,
}

/// The pitch line cut into notes: where the voice stops, and where it moves
/// to a new pitch and stays there.
pub fn notes(t: &Track) -> Vec<Note> {
    let n = t.midi.len();
    let fps = t.sr as f32 / HOP as f32;
    let min_len = (0.06 * fps).ceil() as usize; // 60 ms
    let settle = (0.035 * fps).ceil() as usize; // a new pitch held this long starts a note
    let mut out = Vec::new();
    let mut k = 0;
    while k < n {
        if !t.midi[k].is_finite() {
            k += 1;
            continue;
        }
        // a voiced run
        let a = k;
        while k < n && t.midi[k].is_finite() {
            k += 1;
        }
        let b = k;
        // cut it where the pitch moves and stays (a median a few frames wide follows it)
        let med = |i: usize| -> f32 {
            let lo = i.saturating_sub(3).max(a);
            let hi = (i + 4).min(b);
            let mut v: Vec<f32> = t.midi[lo..hi].to_vec();
            v.sort_by(f32::total_cmp);
            v[v.len() / 2]
        };
        let mut start = a;
        let mut cur = med(a);
        let mut i = a;
        let mut cuts = vec![];
        while i < b {
            let m = med(i);
            if (m - cur).abs() > 0.6 {
                // does it stay there?
                let held = (i..(i + settle).min(b)).all(|j| (med(j) - m).abs() < 0.5);
                if held && i + settle <= b {
                    if i - start >= min_len {
                        cuts.push((start, i));
                        start = i;
                    }
                    cur = m;
                }
            } else {
                // follow slow drift
                cur += 0.05 * (m - cur);
            }
            i += 1;
        }
        cuts.push((start, b));
        for (s, e) in cuts {
            if e - s < min_len {
                continue; // too short to be a note (a scoop, a breath)
            }
            out.push(Note { start: t.secs(s), end: t.secs(e), center: center(&t.midi[s..e], &t.rms[s..e]) });
        }
    }
    out
}

/// The pitch the ear hears for a note: the middle of it (no scoop in, no fall
/// off), weighted by how loud it is.
fn center(midi: &[f32], rms: &[f32]) -> f32 {
    let n = midi.len();
    let (a, b) = (n * 15 / 100, n - n * 15 / 100);
    let mut v: Vec<(f32, f32)> = (a..b.max(a + 1)).filter(|&k| midi[k].is_finite()).map(|k| (midi[k], rms[k])).collect();
    if v.is_empty() {
        return midi.iter().copied().find(|m| m.is_finite()).unwrap_or(60.0);
    }
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let total: f32 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for (m, w) in &v {
        acc += w;
        if acc >= total / 2.0 {
            return *m;
        }
    }
    v[v.len() / 2].0
}

/// How much of each pitch class (C, C#, ... B) frames from..to have: the
/// spectrum's peaks between 65 Hz and 2 kHz folded onto 12 semitones.
pub fn chroma(a: &AudioData, from: usize, to: usize) -> [f32; 12] {
    const N: usize = 8192;
    let to = to.min(a.samples.len());
    let from = from.min(to);
    let x = mono(a, from, to);
    let mut out = [0f32; 12];
    if x.len() < N {
        return out;
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut buf = vec![0f32; N];
    let mut spec = fft.make_output_vec();
    let win: Vec<f32> = (0..N).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / N as f32).cos()).collect();
    let sr = a.sr as f32;
    let (lo, hi) = ((65.0 * N as f32 / sr) as usize, (2000.0 * N as f32 / sr) as usize);
    // at most ~400 frames, however long the clip: plenty for a key
    let step = ((x.len() - N) / 400).max(N / 2);
    let mut s = 0;
    while s + N <= x.len() {
        for k in 0..N {
            buf[k] = x[s + k] * win[k];
        }
        fft.process(&mut buf, &mut spec).expect("fft");
        let mag: Vec<f32> = spec.iter().map(|c: &Complex32| c.norm()).collect();
        let frame_max = mag[lo..hi].iter().cloned().fold(0.0, f32::max);
        if frame_max > 1e-4 {
            for b in lo + 1..hi - 1 {
                // peaks only (the partials), compressed so one loud note doesn't decide it
                if mag[b] > mag[b - 1] && mag[b] >= mag[b + 1] && mag[b] > 0.05 * frame_max {
                    let f = b as f32 * sr / N as f32;
                    let pc = ((69.0 + 12.0 * (f / 440.0).log2()).round() as i32).rem_euclid(12) as usize;
                    out[pc] += (mag[b] / frame_max).sqrt();
                }
            }
        }
        s += step;
    }
    out
}

/// A note to move: seconds in the file, its centre (MIDI), how far to move
/// it (semitones), and how much of its drift to take out (0..1: its slow
/// wandering around the centre; vibrato stays).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TuneNote {
    pub start: f64,
    pub end: f64,
    pub center: f32,
    pub shift: f32,
    pub drift: f32,
}

/// The file with the notes moved. Stretches nothing touches are copied as they are.
pub fn render(a: &AudioData, moves: &[TuneNote]) -> AudioData {
    let sr = a.sr as f64;
    let n = a.samples.len();
    let mut l = a.samples.clone();
    let mut r = a.right.clone();
    // the stretches to work on: around every note that moves, a little either side
    let pad = (0.15 * sr) as usize;
    let mut spans: Vec<(usize, usize)> = moves
        .iter()
        .filter(|m| m.shift.abs() > 0.001 || m.drift > 0.001)
        .map(|m| (((m.start * sr) as usize).saturating_sub(pad), (((m.end * sr) as usize) + pad).min(n)))
        .filter(|(x, y)| x < y)
        .collect();
    spans.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for s in spans {
        match merged.last_mut() {
            Some(m) if s.0 <= m.1 => m.1 = m.1.max(s.1),
            _ => merged.push(s),
        }
    }
    for (x, y) in merged {
        let t = track(a, x, y);
        let shift = shift_curve(&t, moves, x, y);
        let marks = analysis_marks(&mono(a, x, y), &t, x);
        psola(&a.samples[x..y], &marks, &shift, &mut l[x..y]);
        if let (Some(src), Some(dst)) = (&a.right, r.as_mut()) {
            psola(&src[x..y], &marks, &shift, &mut dst[x..y]);
        }
    }
    AudioData { sr: a.sr, samples: l, right: r }
}

/// Semitones to move by, per sample of x..y: each note's own shift, plus
/// its drift taken out; gliding from one to the next where they're close,
/// back to nothing where they aren't, never a jump.
fn shift_curve(t: &Track, moves: &[TuneNote], x: usize, y: usize) -> Vec<f32> {
    let fps = t.sr as f64 / HOP as f64;
    let frames = t.midi.len();
    // the slow line through the pitch (150 ms): its distance from the centre is drift; vibrato is faster
    let slow: Vec<f32> = (0..frames)
        .map(|k| {
            let w = (0.075 * fps) as usize;
            let (lo, hi) = (k.saturating_sub(w), (k + w + 1).min(frames));
            let v: Vec<f32> = t.midi[lo..hi].iter().copied().filter(|m| m.is_finite()).collect();
            if v.is_empty() { f32::NAN } else { v.iter().sum::<f32>() / v.len() as f32 }
        })
        .collect();
    let mut f = vec![f32::NAN; frames];
    for m in moves {
        let a = ((m.start * t.sr as f64 - t.from as f64) / HOP as f64).floor().max(0.0) as usize;
        let b = (((m.end * t.sr as f64 - t.from as f64) / HOP as f64).ceil().max(0.0) as usize).min(frames);
        for k in a..b {
            let d = if slow[k].is_finite() { m.drift * (m.center - slow[k]) } else { 0.0 };
            f[k] = m.shift + d;
        }
    }
    // gaps: a glide between two notes up to 120 ms apart, else back to nothing
    let gap = (0.12 * fps) as usize;
    let mut k = 0;
    while k < frames {
        if f[k].is_finite() {
            k += 1;
            continue;
        }
        let a = k;
        while k < frames && !f[k].is_finite() {
            k += 1;
        }
        let (before, after) = (if a > 0 { f[a - 1] } else { f32::NAN }, if k < frames { f[k] } else { f32::NAN });
        let len = k - a;
        for (j, v) in f[a..k].iter_mut().enumerate() {
            let u = (j as f32 + 0.5) / len as f32;
            *v = match (before.is_finite(), after.is_finite()) {
                (true, true) if len <= gap => before + (after - before) * u,
                (true, _) => before * (1.0 - ((j as f32 + 0.5) / gap as f32).min(1.0)),
                (_, true) => after * (1.0 - (((len - j) as f32 - 0.5) / gap as f32).min(1.0)),
                _ => 0.0,
            };
        }
    }
    // smoothed (20 ms) so a move never clicks in, then per sample
    let w = (0.01 * fps).ceil() as usize;
    let sm: Vec<f32> = (0..frames)
        .map(|k| {
            let (lo, hi) = (k.saturating_sub(w), (k + w + 1).min(frames));
            f[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        })
        .collect();
    // the very ends of the stretch: nothing moved
    let edge = (0.05 * fps) as usize;
    (0..y - x)
        .map(|s| {
            let p = s as f64 / HOP as f64;
            let k = (p.floor() as usize).min(frames.saturating_sub(1));
            let k2 = (k + 1).min(frames.saturating_sub(1));
            let u = (p - k as f64) as f32;
            let v = sm.get(k).copied().unwrap_or(0.0) * (1.0 - u) + sm.get(k2).copied().unwrap_or(0.0) * u;
            let ramp = ((k.min(frames.saturating_sub(1 + k))) as f32 / edge.max(1) as f32).min(1.0);
            v * ramp
        })
        .collect()
}

/// Where the voice's periods are: a mark every period, by the pitch line
/// (every 5 ms where there's no pitch), in samples of the stretch.
fn analysis_marks(m: &[f32], t: &Track, x: usize) -> Vec<(usize, usize)> {
    let sr = t.sr as f32;
    let n = m.len();
    let period = |p: usize| -> usize {
        let k = ((p + x).saturating_sub(t.from) / HOP).min(t.midi.len().saturating_sub(1));
        let v = t.midi.get(k).copied().unwrap_or(f32::NAN);
        if v.is_finite() {
            let f0 = 440.0 * 2f32.powf((v - 69.0) / 12.0);
            (sr / f0).round().max(8.0) as usize
        } else {
            (0.005 * sr) as usize
        }
    };
    // (position, its period): one period apart, following the pitch line
    let _ = m;
    let mut marks = Vec::new();
    let mut p = 0usize;
    while p < n {
        let tp = period(p);
        marks.push((p, tp));
        p += tp;
    }
    marks
}

/// Pitch-synchronous overlap-add: two periods of the voice around each mark,
/// laid down again a period over the factor apart; divided by the windows'
/// overlap, so it's exactly the original where nothing moves.
fn psola(src: &[f32], marks: &[(usize, usize)], shift: &[f32], dst: &mut [f32]) {
    let n = src.len();
    if marks.len() < 3 || n == 0 {
        return;
    }
    let mut out = vec![0f32; n];
    let mut wsum = vec![0f32; n];
    let mut ts = marks[0].0 as f64;
    let mut i = 0usize;
    while (ts as usize) < n {
        // the analysis mark nearest to where we are
        while i + 1 < marks.len() && (marks[i + 1].0 as f64 - ts).abs() < (marks[i].0 as f64 - ts).abs() {
            i += 1;
        }
        let s = shift[(ts as usize).min(n - 1)];
        let (ta, tp) = marks[i];
        let left = if i > 0 { ta - marks[i - 1].0 } else { tp };
        let right = if i + 1 < marks.len() { marks[i + 1].0 - ta } else { tp };
        let off = ts - ta as f64;
        // place the segment ta-left..ta+right at ts (whole samples, the fraction
        // carried in ts), each half with its own Hann half
        let base = ts.round() as i64 - ta as i64;
        for k in ta.saturating_sub(left)..(ta + right).min(n) {
            let d = k as i64 - ta as i64;
            let w = if d < 0 {
                0.5 + 0.5 * (std::f32::consts::PI * (-d) as f32 / left as f32).cos()
            } else {
                0.5 + 0.5 * (std::f32::consts::PI * d as f32 / right as f32).cos()
            };
            let at = k as i64 + base;
            if at >= 0 && (at as usize) < n {
                out[at as usize] += w * src[k];
                wsum[at as usize] += w;
            }
        }
        let _ = off;
        if s.abs() < 1e-3 {
            // nothing moved here: back in step with the original's marks
            ts = marks.get(i + 1).map_or(n as f64, |m| m.0 as f64);
            i = (i + 1).min(marks.len() - 1);
        } else {
            let beta = 2f64.powf(s as f64 / 12.0);
            ts += tp as f64 / beta;
        }
    }
    for k in 0..n {
        if wsum[k] > 1e-3 {
            dst[k] = out[k] / wsum[k];
        } else {
            dst[k] = src[k];
        }
    }
}

/// A tune's identity (for its rendered copy), from its moves.
pub fn tune_hash(moves: &[TuneNote]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in moves {
        ((m.start * 1e4) as i64, (m.end * 1e4) as i64, (m.center * 1e3) as i32, (m.shift * 1e4) as i32, (m.drift * 1e3) as i32).hash(&mut h);
    }
    h.finish() | 1 // never 0 (0 is "not tuned")
}

pub type Moves = Arc<Vec<TuneNote>>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up voice: harmonics, each note with vibrato and a little off.
    fn voice(sr: u32, plan: &[(f32, f32)], note: f32, gap: f32) -> AudioData {
        let mut x = Vec::new();
        let mut ph = 0f64;
        for &(midi, _) in plan {
            let n = (note * sr as f32) as usize;
            for k in 0..n {
                let t = k as f32 / sr as f32;
                let vib = 0.25 * (2.0 * std::f32::consts::PI * 5.5 * t).sin(); // ±25 cents
                let f0 = 440.0 * 2f32.powf((midi + vib - 69.0) / 12.0);
                ph += 2.0 * std::f64::consts::PI * f0 as f64 / sr as f64;
                let env = (t / 0.02).min(1.0) * ((note - t) / 0.02).min(1.0);
                let s: f64 = (1..=12).map(|h| (ph * h as f64).sin() / h as f64).sum();
                x.push(0.3 * env * s as f32);
            }
            x.extend(std::iter::repeat_n(0.0, (gap * sr as f32) as usize));
        }
        AudioData::mono(sr, x)
    }

    fn centers(a: &AudioData) -> Vec<f32> {
        notes(&track(a, 0, a.samples.len())).iter().map(|n| n.center).collect()
    }

    #[test]
    fn finds_the_notes_and_their_centres() {
        let plan = [(69.3, 0.0), (71.6, 0.0), (76.1, 0.0)];
        let a = voice(44100, &plan, 0.5, 0.08);
        let c = centers(&a);
        assert_eq!(c.len(), 3, "{c:?}");
        for (got, (want, _)) in c.iter().zip(plan) {
            assert!((got - want).abs() < 0.08, "{got} vs {want}");
        }
    }

    #[test]
    fn moves_notes_keeps_the_rest() {
        let a = voice(44100, &[(69.3, 0.0), (71.6, 0.0), (76.1, 0.0)], 0.5, 0.08);
        let found = notes(&track(&a, 0, a.samples.len()));
        // into tune: A4, C5; the last one is left alone
        let moves: Vec<TuneNote> = found
            .iter()
            .enumerate()
            .map(|(i, n)| TuneNote { start: n.start, end: n.end, center: n.center, shift: if i < 2 { n.center.round() - n.center } else { 0.0 }, drift: 0.0 })
            .collect();
        let b = render(&a, &moves);
        assert_eq!(b.samples.len(), a.samples.len());
        let c = centers(&b);
        assert_eq!(c.len(), 3, "{c:?}");
        assert!((c[0] - 69.0).abs() < 0.06 && (c[1] - 72.0).abs() < 0.06, "{c:?}");
        assert!((c[2] - 76.1).abs() < 0.05, "{c:?}");
        // the vibrato is still there: the pitch still swings in the moved note
        let t = track(&b, 0, b.samples.len());
        let k0 = (found[0].start * 44100.0 / HOP as f64) as usize + 20;
        let v: Vec<f32> = t.midi[k0..k0 + 40].iter().copied().filter(|m| m.is_finite()).collect();
        let swing = v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min);
        assert!(swing > 0.3, "vibrato gone: {swing}");
        // and nothing moved is exactly as it was
        let s = (found[2].start * 44100.0) as usize + 4410;
        assert!(a.samples[s..s + 4410].iter().zip(&b.samples[s..s + 4410]).all(|(x, y)| (x - y).abs() < 1e-6));
    }

    #[test]
    fn bigger_moves() {
        let a = voice(44100, &[(64.0, 0.0), (67.0, 0.0)], 0.6, 0.1);
        let found = notes(&track(&a, 0, a.samples.len()));
        assert_eq!(found.len(), 2);
        let moves: Vec<TuneNote> = found.iter().zip([3.0f32, -5.0]).map(|(n, sh)| TuneNote { start: n.start, end: n.end, center: n.center, shift: sh, drift: 0.0 }).collect();
        let c = centers(&render(&a, &moves));
        assert_eq!(c.len(), 2, "{c:?}");
        assert!((c[0] - 67.0).abs() < 0.08 && (c[1] - 62.0).abs() < 0.08, "{c:?}");
    }

    #[test]
    fn drift_out_vibrato_in() {
        // a note that sags 40 cents over its length, with vibrato
        let sr = 44100u32;
        let n = (0.8 * sr as f32) as usize;
        let mut ph = 0f64;
        let x: Vec<f32> = (0..n)
            .map(|k| {
                let t = k as f32 / sr as f32;
                let midi = 69.0 - 0.4 * t / 0.8 + 0.2 * (2.0 * std::f32::consts::PI * 6.0 * t).sin();
                ph += 2.0 * std::f64::consts::PI * (440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0)) / sr as f64;
                let s: f64 = (1..=10).map(|h| (ph * h as f64).sin() / h as f64).sum();
                0.3 * (t / 0.02).min(1.0) * ((0.8 - t) / 0.02).min(1.0) * s as f32
            })
            .collect();
        let a = AudioData::mono(sr, x);
        let nt = notes(&track(&a, 0, n));
        assert_eq!(nt.len(), 1, "{nt:?}");
        let b = render(&a, &[TuneNote { start: nt[0].start, end: nt[0].end, center: nt[0].center, shift: 0.0, drift: 1.0 }]);
        let slope = |a: &AudioData| {
            let t = track(a, 0, n);
            let k = t.midi.len();
            let avg = |lo: usize, hi: usize| {
                let v: Vec<f32> = t.midi[lo..hi].iter().copied().filter(|m| m.is_finite()).collect();
                v.iter().sum::<f32>() / v.len() as f32
            };
            avg(k / 6, k / 3) - avg(2 * k / 3, 5 * k / 6)
        };
        let (before, after) = (slope(&a), slope(&b));
        assert!(before > 0.15 && after.abs() < 0.08, "sag {before} -> {after}");
    }

    #[test]
    fn chroma_of_a_chord() {
        let sr = 44100;
        let x: Vec<f32> = (0..sr * 2)
            .map(|k| {
                let t = k as f32 / sr as f32;
                [60.0f32, 64.0, 67.0].iter().map(|m| (2.0 * std::f32::consts::PI * 440.0 * 2f32.powf((m - 69.0) / 12.0) * t).sin()).sum::<f32>() * 0.2
            })
            .collect();
        let c = chroma(&AudioData::mono(sr as u32, x), 0, sr);
        let mut idx: Vec<usize> = (0..12).collect();
        idx.sort_by(|a, b| c[*b].total_cmp(&c[*a]));
        let top: std::collections::BTreeSet<usize> = idx[..3].iter().copied().collect();
        assert_eq!(top, [0, 4, 7].into_iter().collect(), "{c:?}");
    }
}
