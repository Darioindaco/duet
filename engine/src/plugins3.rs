//! Duet Tune live: pitch correction as you play it back (or sing through it). It hears the pitch
//! (YIN on a 12 kHz copy), picks the nearest note in the key and scale, and moves the voice there with
//! Signalsmith Stretch, as fast as the retune speed says. The stretcher's delay is reported, so the
//! rest of the mix waits for it.

use std::sync::Mutex;

use ssstretch::Stretch;

use crate::fx::{Effect, Params};
use crate::kit::*;

/// scales as the notes they keep, from the key's root
pub const SCALES: [&[i32]; 7] = [
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], // chromatic
    &[0, 2, 4, 5, 7, 9, 11],                // major
    &[0, 2, 3, 5, 7, 8, 10],                // minor
    &[0, 2, 3, 5, 7, 8, 11],                // harmonic minor
    &[0, 2, 4, 7, 9],                       // pentatonic major
    &[0, 3, 5, 7, 10],                      // pentatonic minor
    &[0, 3, 5, 6, 7, 10],                   // blues
];

/// The nearest note to `m` (a MIDI number with cents) in `scale` from `key`.
pub fn snap(m: f32, key: usize, scale: usize) -> i32 {
    let notes = SCALES[scale.min(SCALES.len() - 1)];
    let c = m.round() as i32;
    (c - 6..=c + 6)
        .filter(|n| notes.contains(&((n - key as i32).rem_euclid(12))))
        .min_by(|a, b| (*a as f32 - m).abs().total_cmp(&(*b as f32 - m).abs()))
        .unwrap_or(c)
}

/// The stretcher, sized for a voice: short blocks, so the delay stays small.
fn stretcher(sr: f32) -> Stretch {
    let mut st = Stretch::new();
    let block = ((1536.0 * sr / 48000.0) as i32).max(256);
    st.configure(2, block, block / 4);
    st
}

static LATENCY: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// How late the corrected voice comes out, in samples, at `sr`.
pub fn latency(sr: f32) -> u32 {
    let key = sr as u32;
    let mut cache = LATENCY.lock().unwrap();
    if let Some((_, l)) = cache.iter().find(|(s, _)| *s == key) {
        return *l;
    }
    let st = stretcher(sr);
    let l = (st.input_latency() + st.output_latency()).max(0) as u32;
    cache.push((key, l));
    l
}

/// The C++ stretcher only ever runs on one thread at a time (built on the control thread, then
/// handed to the audio thread), which is what Send asks.
struct Shifter(Stretch);
unsafe impl Send for Shifter {}

const YIN_W: usize = 384;
const YIN_TAU: usize = 180;
const RING: usize = 1024;
const HOP: usize = 48;

/// key (C..B), scale, speed (0 = instant .. 400 ms), humanize (let vibrato and slides through),
/// amount, transpose -12..+12, mix
pub struct AutoTune {
    sr: f32,
    st: Shifter,
    lat: usize,
    // the 12 kHz copy for listening
    down: usize,
    aa: [Biquad; 2],
    dcount: usize,
    ring: [f32; RING],
    rpos: usize,
    since: usize,
    // what it hears and does
    level: Env,
    heard: f32,
    center: f32,
    target: i32,
    corr: f32,
    voiced: bool,
    // the dry voice, delayed to line up with the corrected one
    dry: [Vec<f32>; 2],
    dpos: usize,
    out: [Vec<f32>; 2],
    scope: [f32; 4],
}

impl AutoTune {
    pub fn new(sr: f32) -> Self {
        let down = ((sr / 12000.0).round() as usize).max(1);
        let mut aa = [Biquad::new(); 2];
        let c = rbj::lowpass((sr / down as f32 * 0.42) as f64, 0.7071, sr as f64);
        for b in &mut aa {
            b.c = c;
        }
        let lat = latency(sr) as usize;
        AutoTune {
            sr, st: Shifter(stretcher(sr)), lat, down, aa, dcount: 0, ring: [0.0; RING], rpos: 0, since: 0,
            level: Env::default(), heard: -1.0, center: 60.0, target: 60, corr: 0.0, voiced: false,
            dry: [vec![0.0; lat + 8192], vec![0.0; lat + 8192]], dpos: 0,
            out: [vec![0.0; 8192], vec![0.0; 8192]], scope: [-1.0, -1.0, -1.0, 0.0],
        }
    }

    /// YIN on the last YIN_W + YIN_TAU samples of the 12 kHz copy: the pitch in Hz, if there is one.
    fn detect(&self) -> Option<f32> {
        let sr = self.sr / self.down as f32;
        let n = YIN_W + YIN_TAU;
        let x = |i: usize| self.ring[(self.rpos + RING - n + i) % RING];
        let tau_min = (sr / 1000.0) as usize;
        let mut d = [0f32; YIN_TAU];
        let mut sum = 0f32;
        let mut best: Option<(usize, f32)> = None;
        let mut cm = [1f32; YIN_TAU];
        for tau in 1..YIN_TAU {
            let mut s = 0f32;
            for j in 0..YIN_W {
                let v = x(j) - x(j + tau);
                s += v * v;
            }
            d[tau] = s;
            sum += s;
            cm[tau] = if sum > 0.0 { s * tau as f32 / sum } else { 1.0 };
        }
        for tau in tau_min.max(2)..YIN_TAU - 1 {
            if cm[tau] < 0.15 && cm[tau] <= cm[tau + 1] {
                best = Some((tau, cm[tau]));
                break;
            }
        }
        let (tau, _) = best?;
        // between the samples: a parabola through the dip
        let (a, b, c) = (cm[tau - 1], cm[tau], cm[tau + 1]);
        let den = a - 2.0 * b + c;
        let t = tau as f32 + if den.abs() > 1e-9 { 0.5 * (a - c) / den } else { 0.0 };
        Some(sr / t)
    }
}

impl Effect for AutoTune {
    fn process(&mut self, p: &Params, l: &mut [f32], r: &mut [f32]) {
        let key = choice(p[0], 12);
        let scale = choice(p[1], SCALES.len());
        let speed_ms = 400.0 * p[2] * p[2];
        let humanize = p[3];
        let amount = p[4];
        let transpose = (-12.0 + 24.0 * p[5]).round();
        let mix = p[6];
        let n = l.len().min(r.len()).min(self.out[0].len());
        let hop_s = (HOP * self.down) as f32 / self.sr;
        let a_speed = if speed_ms < 1.0 { 1.0 } else { 1.0 - (-hop_s * 1000.0 / speed_ms).exp() };
        let a_center = 1.0 - (-hop_s / (0.01 + 0.25 * humanize)).exp();
        let (la, lr) = (coef(5.0, self.sr), coef(100.0, self.sr));
        // listen: decimate to ~12 kHz and look for the pitch every HOP samples there
        for k in 0..n {
            let m = 0.5 * (l[k] + r[k]);
            let lvl = self.level.run(m.abs(), la, lr);
            let y = self.aa[0].run(m);
            let y = self.aa[1].run(y);
            self.dcount += 1;
            if self.dcount < self.down {
                continue;
            }
            self.dcount = 0;
            self.ring[self.rpos] = y;
            self.rpos = (self.rpos + 1) % RING;
            self.since += 1;
            if self.since < HOP {
                continue;
            }
            self.since = 0;
            let f = if lvl > 0.003 { self.detect().filter(|f| (60.0..1100.0).contains(f)) } else { None };
            match f {
                Some(f) => {
                    let m = 69.0 + 12.0 * (f / 440.0).log2();
                    if !self.voiced {
                        self.center = m;
                    }
                    self.voiced = true;
                    self.heard = m;
                    self.center += (m - self.center) * a_center;
                    // stay on a note until the voice is clearly nearer another one
                    let near = snap(self.center, key, scale);
                    if near != self.target && (self.center - self.target as f32).abs() > 0.6 {
                        self.target = near;
                    } else if !SCALES[scale].contains(&((self.target - key as i32).rem_euclid(12))) {
                        self.target = near;
                    }
                    let follow = m + (self.center - m) * humanize;
                    let want = (self.target as f32 - follow) * amount;
                    self.corr += (want - self.corr) * a_speed;
                }
                None => {
                    self.voiced = false;
                    self.heard = -1.0;
                    self.corr *= 0.85;
                }
            }
        }
        let ratio = 2f32.powf((self.corr + transpose) / 12.0);
        let st = &mut self.st.0;
        st.set_transpose_factor(ratio, Some(8000.0 / self.sr));
        {
            let ins = [l.as_ptr(), r.as_ptr()];
            let [o0, o1] = &mut self.out;
            let mut outs = [o0.as_mut_ptr(), o1.as_mut_ptr()];
            // SAFETY: two channels, each with at least n samples in and out
            unsafe { st.process(&ins, n as i32, &mut outs, n as i32) };
        }
        let len = self.dry[0].len();
        for k in 0..n {
            for (ch, x) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                self.dry[ch][self.dpos] = *x;
                let d = self.dry[ch][(self.dpos + len - self.lat) % len];
                *x = d + (self.out[ch][k] - d) * mix;
            }
            self.dpos = (self.dpos + 1) % len;
        }
        self.scope = [
            if self.voiced { self.heard } else { -1.0 },
            if self.voiced { self.heard + self.corr + transpose } else { -1.0 },
            if self.voiced { self.target as f32 } else { -1.0 },
            self.corr * 100.0,
        ];
    }
    fn reset(&mut self) {
        self.st.0.reset();
        for d in &mut self.dry {
            d.fill(0.0);
        }
        self.corr = 0.0;
    }
    fn scope(&self) -> &[f32] {
        &self.scope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{kind_index, param_index, SlotCfg};

    #[test]
    fn snaps_to_the_scale() {
        assert_eq!(snap(60.8, 0, 1), 60); // C major: no C♯, C is nearer than D
        assert_eq!(snap(61.6, 0, 1), 62);
        assert_eq!(snap(61.2, 0, 0), 61);
        assert_eq!(snap(66.4, 9, 2), 67); // A minor: no F♯, G is nearer than F
    }

    #[test]
    fn a_flat_note_comes_out_in_tune_and_late_by_what_it_says() {
        let sr = 48000.0;
        let mut c = SlotCfg::new(kind_index("autotune").unwrap());
        c.p[param_index(c.kind, "speed").unwrap()] = 0.0;
        // A4 a third of a semitone flat
        let f = 440.0 * 2f32.powf(-0.33 / 12.0);
        let x: Vec<f32> = (0..sr as usize * 2).map(|i| {
            let t = i as f32 / sr;
            0.4 * ((2.0 * std::f32::consts::PI * f * t).sin() + 0.4 * (4.0 * std::f32::consts::PI * f * t).sin())
        }).collect();
        let mut fx = crate::fx::make(&c, sr).unwrap();
        let (mut l, mut r) = (x.clone(), x.clone());
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            fx.process(&c.p, a, b);
        }
        // measure the output's pitch: zero crossings over the last half second
        let tail = &l[l.len() - 24000..];
        let ups = tail.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32;
        let got = ups / 0.5;
        let cents = 1200.0 * (got / 440.0).log2();
        assert!(cents.abs() < 12.0, "came out at {got} Hz ({cents} cents)");
        let sc = fx.scope();
        assert_eq!(sc[2], 69.0, "{sc:?}");
        eprintln!("live tune latency at 48 kHz: {} samples", latency(sr));
        assert!(latency(sr) > 0 && latency(sr) < 4800, "{}", latency(sr));
    }
}
