//! Building blocks for Duet's own plugins (plugins.rs): oversampling, filters with any Q, crossovers,
//! envelope followers, a true-peak detector, and where the song is (for tempo-synced effects).
//! Everything here is real-time safe once made: no allocation while processing.

use std::cell::Cell;
use std::f64::consts::PI as PI64;

use crate::fx::Coefs;

// --- where the song is ---------------------------------------------------------------------

/// The song's position and tempo at the start of the block being processed, for effects that move
/// with the beat. Set by whoever runs the racks: the audio thread, or an export.
#[derive(Clone, Copy, Debug)]
pub struct SongTime {
    pub beat: f64,
    pub bpm: f64,
    pub playing: bool,
}

thread_local! {
    static SONG_TIME: Cell<SongTime> = const { Cell::new(SongTime { beat: 0.0, bpm: 120.0, playing: false }) };
}

pub fn song_time() -> SongTime {
    SONG_TIME.with(|t| t.get())
}

pub fn set_song_time(t: SongTime) {
    SONG_TIME.with(|c| c.set(t));
}

// --- small things --------------------------------------------------------------------------

#[inline]
pub fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[inline]
pub fn lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// One-pole coefficient: reaches ~63% in `ms`.
#[inline]
pub fn coef(ms: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (ms.max(0.001) * 0.001 * sr)).exp()
}

/// A knob value 0..1 on a log scale a..b.
#[inline]
pub fn logk(v: f32, a: f32, b: f32) -> f32 {
    a * (b / a).powf(v.clamp(0.0, 1.0))
}

/// A choice knob (0..1) as an index 0..n-1.
#[inline]
pub fn choice(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1)
}

// --- filters ------------------------------------------------------------------------------

/// RBJ cookbook filters with a free Q (fx::Coefs has the EQ's fixed ones).
pub mod rbj {
    use super::*;

    fn norm(b: [f64; 3], a: [f64; 3]) -> Coefs {
        Coefs([(b[0] / a[0]) as f32, (b[1] / a[0]) as f32, (b[2] / a[0]) as f32, (a[1] / a[0]) as f32, (a[2] / a[0]) as f32])
    }
    fn w(f: f64, sr: f64) -> (f64, f64) {
        let w = 2.0 * PI64 * f.clamp(5.0, sr * 0.49) / sr;
        (w.cos(), w.sin())
    }
    pub fn lowpass(f: f64, q: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let al = s / (2.0 * q);
        norm([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn highpass(f: f64, q: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let al = s / (2.0 * q);
        norm([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn bandpass(f: f64, q: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let al = s / (2.0 * q);
        norm([al, 0.0, -al], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn notch(f: f64, q: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let al = s / (2.0 * q);
        norm([1.0, -2.0 * c, 1.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }
    pub fn peak(f: f64, q: f64, gain_db: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let a = 10f64.powf(gain_db / 40.0);
        let al = s / (2.0 * q);
        norm([1.0 + al * a, -2.0 * c, 1.0 - al * a], [1.0 + al / a, -2.0 * c, 1.0 - al / a])
    }
    /// shelves: q 0.707 is the classic slope; higher bumps before the shelf
    pub fn shelf(high: bool, f: f64, q: f64, gain_db: f64, sr: f64) -> Coefs {
        let (c, s) = w(f, sr);
        let a = 10f64.powf(gain_db / 40.0);
        let al = s / (2.0 * q);
        let sq = 2.0 * a.sqrt() * al;
        if high {
            norm(
                [a * ((a + 1.0) + (a - 1.0) * c + sq), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - sq)],
                [(a + 1.0) - (a - 1.0) * c + sq, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sq],
            )
        } else {
            norm(
                [a * ((a + 1.0) - (a - 1.0) * c + sq), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - sq)],
                [(a + 1.0) + (a - 1.0) * c + sq, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - sq],
            )
        }
    }
}

/// One biquad (transposed direct form II).
#[derive(Clone, Copy)]
pub struct Biquad {
    pub c: Coefs,
    z: [f32; 2],
}

impl Biquad {
    pub const fn new() -> Self {
        Biquad { c: Coefs::FLAT, z: [0.0; 2] }
    }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = self.c.0;
        let y = b0 * x + self.z[0];
        self.z[0] = b1 * x - a1 * y + self.z[1];
        self.z[1] = b2 * x - a2 * y;
        y
    }
    pub fn reset(&mut self) {
        self.z = [0.0; 2];
    }
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

/// A 4th-order Linkwitz-Riley split: low + high add back up to the input (in level; the phase
/// turns like an all-pass).
#[derive(Clone, Copy, Default)]
pub struct Crossover {
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    f: f32,
}

impl Crossover {
    pub fn set(&mut self, f: f32, sr: f32) {
        if (f - self.f).abs() < 1e-3 {
            return;
        }
        self.f = f;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for b in &mut self.lp {
            b.c = rbj::lowpass(f as f64, q, sr as f64);
        }
        for b in &mut self.hp {
            b.c = rbj::highpass(f as f64, q, sr as f64);
        }
    }
    #[inline]
    pub fn split(&mut self, x: f32) -> (f32, f32) {
        let lo = self.lp[0].run(x);
        let lo = self.lp[1].run(lo);
        let hi = self.hp[0].run(x);
        let hi = self.hp[1].run(hi);
        (lo, hi)
    }
    /// the phase turn of a split without splitting (to keep a band in step with ones that were)
    #[inline]
    pub fn allpass(&mut self, x: f32) -> f32 {
        let (lo, hi) = self.split(x);
        lo + hi
    }
    pub fn reset(&mut self) {
        for b in self.lp.iter_mut().chain(self.hp.iter_mut()) {
            b.reset();
        }
    }
}

/// Three bands from one signal: low | mid | high, phase-matched so they add back up flat.
#[derive(Clone, Copy, Default)]
pub struct ThreeWay {
    a: Crossover,
    b: Crossover,
    ap: Crossover,
}

impl ThreeWay {
    pub fn set(&mut self, f_low: f32, f_high: f32, sr: f32) {
        let f_high = f_high.max(f_low * 1.5);
        self.a.set(f_low, sr);
        self.b.set(f_high, sr);
        self.ap.set(f_high, sr);
    }
    #[inline]
    pub fn split(&mut self, x: f32) -> [f32; 3] {
        let (lo, rest) = self.a.split(x);
        let (mid, hi) = self.b.split(rest);
        [self.ap.allpass(lo), mid, hi]
    }
    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
        self.ap.reset();
    }
}

/// The topology-preserving state-variable filter (Simper): stays put while its cutoff moves
/// fast, so it can be swept and modulated. Gives low, band and high at once.
#[derive(Clone, Copy, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// (low, band, high) for cutoff `f`, resonance `k` (2 = none .. ~0.05 = rings)
    #[inline]
    pub fn run(&mut self, x: f32, f: f32, k: f32, sr: f32) -> (f32, f32, f32) {
        let g = (std::f32::consts::PI * (f / sr).clamp(1e-5, 0.49)).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - k * v1 - v2)
    }
    pub fn reset(&mut self) {
        *self = Svf::default();
    }
}

/// Takes out what doesn't move (DC), below ~10 Hz.
#[derive(Clone, Copy, Default)]
pub struct DcBlock {
    x1: f32,
    y1: f32,
}

impl DcBlock {
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + 0.9987 * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Follows a level: up at the attack's speed, down at the release's.
#[derive(Clone, Copy, Default)]
pub struct Env {
    pub v: f32,
}

impl Env {
    #[inline]
    pub fn run(&mut self, x: f32, att: f32, rel: f32) -> f32 {
        let c = if x > self.v { att } else { rel };
        self.v += (x - self.v) * c;
        self.v
    }
}

// --- oversampling -------------------------------------------------------------------------

const HB: usize = 33;
const HB_HALF: usize = HB / 2 + 1; // 17 taps per phase (the even phase), 16 for the odd one

/// The half-band low-pass every 2x stage uses (Blackman-Harris windowed sinc at a quarter of the
/// high rate).
fn halfband() -> [f32; HB] {
    let c = (HB - 1) as f64 / 2.0;
    let mut h = [0f64; HB];
    for (i, v) in h.iter_mut().enumerate() {
        let t = i as f64 - c;
        let sinc = if t == 0.0 { 0.5 } else { (PI64 * t / 2.0).sin() / (PI64 * t) };
        let x = 2.0 * PI64 * i as f64 / (HB - 1) as f64;
        let win = 0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos();
        *v = sinc * win;
    }
    let sum: f64 = h.iter().sum();
    let mut out = [0f32; HB];
    for i in 0..HB {
        out[i] = (h[i] / sum) as f32;
    }
    out
}

#[derive(Clone, Copy)]
struct Stage {
    up_even: [f32; HB_HALF],
    up_odd: [f32; HB_HALF],
    h: [f32; HB],
    xin: [f32; HB_HALF], // the last base-rate inputs, newest first
    xdown: [f32; HB + 1], // the last high-rate samples, newest first
}

impl Stage {
    fn new() -> Self {
        let h = halfband();
        let mut up_even = [0f32; HB_HALF];
        let mut up_odd = [0f32; HB_HALF];
        for j in 0..HB_HALF {
            up_even[j] = 2.0 * h[2 * j];
            if 2 * j + 1 < HB {
                up_odd[j] = 2.0 * h[2 * j + 1];
            }
        }
        Stage { up_even, up_odd, h, xin: [0.0; HB_HALF], xdown: [0.0; HB + 1] }
    }
    #[inline]
    fn up(&mut self, x: f32) -> (f32, f32) {
        self.xin.copy_within(0..HB_HALF - 1, 1);
        self.xin[0] = x;
        let (mut a, mut b) = (0f32, 0f32);
        for j in 0..HB_HALF {
            a += self.up_even[j] * self.xin[j];
            b += self.up_odd[j] * self.xin[j];
        }
        (a, b)
    }
    #[inline]
    fn down(&mut self, a: f32, b: f32) -> f32 {
        self.xdown.copy_within(0..HB - 1, 2);
        self.xdown[1] = a;
        self.xdown[0] = b;
        // centred on the even sample, so the delay is a whole number of base-rate samples
        let mut y = 0f32;
        for k in 0..HB {
            y += self.h[k] * self.xdown[k + 1];
        }
        y
    }
    fn reset(&mut self) {
        self.xin = [0.0; HB_HALF];
        self.xdown = [0.0; HB + 1];
    }
}

const OS_CHUNK: usize = 64;

/// Runs a nonlinear stage at 2x, 4x or 8x the sample rate, so what it adds above the audible range
/// doesn't fold back down as harshness. Stereo.
pub struct Oversampler {
    st: [[Stage; 3]; 2],
    last: usize,
}

impl Oversampler {
    pub fn new() -> Self {
        Oversampler { st: [[Stage::new(); 3]; 2], last: 0 }
    }

    /// The delay it adds at `stages` (0 = off, 1 = 2x, 2 = 4x, 3 = 8x), in base-rate samples.
    pub fn latency(stages: usize) -> u32 {
        [0, 16, 24, 28][stages.min(3)]
    }

    /// `l`/`r` through `f` at 2^stages times the rate; `f` gets the high-rate blocks and the factor.
    pub fn run(&mut self, stages: usize, l: &mut [f32], r: &mut [f32], mut f: impl FnMut(&mut [f32], &mut [f32], usize)) {
        let stages = stages.min(3);
        if stages != self.last {
            for ch in &mut self.st {
                for s in ch.iter_mut() {
                    s.reset();
                }
            }
            self.last = stages;
        }
        if stages == 0 {
            f(l, r, 1);
            return;
        }
        let fac = 1 << stages;
        let n = l.len().min(r.len());
        let mut at = 0;
        while at < n {
            let m = (n - at).min(OS_CHUNK);
            let mut bufs = [[0f32; OS_CHUNK * 8]; 2];
            let mut tmp = [0f32; OS_CHUNK * 8];
            for (ch, src) in [&l[at..at + m], &r[at..at + m]].into_iter().enumerate() {
                let b = &mut bufs[ch];
                b[..m].copy_from_slice(src);
                let mut len = m;
                for s in 0..stages {
                    for i in 0..len {
                        let (a, c) = self.st[ch][s].up(b[i]);
                        tmp[2 * i] = a;
                        tmp[2 * i + 1] = c;
                    }
                    len *= 2;
                    b[..len].copy_from_slice(&tmp[..len]);
                }
            }
            let [bl, br] = &mut bufs;
            f(&mut bl[..m * fac], &mut br[..m * fac], fac);
            for (ch, dst) in [&mut l[at..at + m], &mut r[at..at + m]].into_iter().enumerate() {
                let b = &mut bufs[ch];
                let mut len = m * fac;
                for s in (0..stages).rev() {
                    for i in 0..len / 2 {
                        b[i] = self.st[ch][s].down(b[2 * i], b[2 * i + 1]);
                    }
                    len /= 2;
                }
                dst.copy_from_slice(&b[..m]);
            }
            at += m;
        }
    }
}

impl Default for Oversampler {
    fn default() -> Self {
        Self::new()
    }
}

// --- true peak --------------------------------------------------------------------------

const TP_TAPS: usize = 12;

/// The highest point between the samples too (4x interpolated, like a true-peak meter), per channel.
pub struct TruePeak {
    h: [[f32; TP_TAPS]; 4],
    x: [[f32; TP_TAPS]; 2],
}

impl TruePeak {
    pub fn new() -> Self {
        let len = 4 * TP_TAPS;
        let mut h = [[0f32; TP_TAPS]; 4];
        let c = (len as f64 - 1.0) / 2.0;
        for i in 0..len {
            let t = (i as f64 - c) / 4.0;
            let sinc = if t.abs() < 1e-9 { 1.0 } else { (PI64 * t).sin() / (PI64 * t) };
            let w = 0.5 - 0.5 * (2.0 * PI64 * (i as f64 + 0.5) / len as f64).cos();
            h[i % 4][i / 4] = (sinc * w) as f32;
        }
        TruePeak { h, x: [[0.0; TP_TAPS]; 2] }
    }
    #[inline]
    pub fn peak(&mut self, ch: usize, x: f32) -> f32 {
        let hist = &mut self.x[ch];
        hist.copy_within(1.., 0);
        hist[TP_TAPS - 1] = x;
        let mut m = x.abs();
        for p in &self.h {
            let mut y = 0f32;
            for (a, b) in hist.iter().zip(p.iter().rev()) {
                y += a * b;
            }
            m = m.max(y.abs());
        }
        m
    }
}

impl Default for TruePeak {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversampling_gives_back_the_input_delayed() {
        for stages in 1..=3 {
            let mut os = Oversampler::new();
            let sr = 48000.0;
            let n = 4096;
            let x: Vec<f32> = (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()).collect();
            let (mut l, mut r) = (x.clone(), x.clone());
            for (a, b) in l.chunks_mut(100).zip(r.chunks_mut(100)) {
                os.run(stages, a, b, |_, _, _| {});
            }
            let d = Oversampler::latency(stages) as usize;
            let err = (1000..n).map(|i| (l[i] - x[i - d]).abs()).fold(0f32, f32::max);
            assert!(err < 0.01, "{stages} stages: off by {err}");
        }
    }

    #[test]
    fn the_crossover_adds_back_up_flat() {
        let sr = 48000.0;
        let mut t = ThreeWay::default();
        t.set(200.0, 3000.0, sr);
        for f in [60.0, 200.0, 1000.0, 3000.0, 9000.0] {
            let mut peak = 0f32;
            for i in 0..48000 {
                let x = (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin();
                let b = t.split(x);
                if i > 24000 {
                    peak = peak.max((b[0] + b[1] + b[2]).abs());
                }
            }
            assert!((db(peak)).abs() < 0.3, "{f} Hz: {} dB", db(peak));
        }
    }
}
