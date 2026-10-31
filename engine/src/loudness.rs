//! Loudness as ITU-R BS.1770-4 / EBU R128 measure it: K-weighting, momentary (400 ms),
//! short-term (3 s), integrated (gated: -70 LUFS absolute, -10 LU relative), loudness range
//! (EBU Tech 3342) and true peak (4x oversampled). Real-time safe once made: everything is a
//! fixed size, so the audio thread can meter the master with it, and an export the mix.

const BINS: usize = 900; // gating histograms: -70..+20 LUFS in 0.1 LU steps
const LO: f64 = -70.0;
const STEP: f64 = 0.1;
const TP_TAPS: usize = 12; // per phase of the 4x interpolator

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The two K-weighting stages for sample rate `sr` (the formulas libebur128 uses: any rate).
fn k_weighting(sr: f64) -> [Biquad; 2] {
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
        ..Default::default()
    };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad { b0: 1.0, b1: -2.0, b2: 1.0, a1: 2.0 * (k * k - 1.0) / a0, a2: (1.0 - k / q + k * k) / a0, ..Default::default() };
    [shelf, hp]
}

/// LUFS from a mean square (both channels summed).
pub fn lufs(ms: f64) -> f64 {
    if ms <= 1e-20 { f64::NEG_INFINITY } else { -0.691 + 10.0 * ms.log10() }
}

/// A histogram of gating blocks: (count, summed mean squares) per 0.1 LU.
struct Hist {
    n: Vec<u32>,
    e: Vec<f64>,
}

impl Hist {
    fn new() -> Self {
        Hist { n: vec![0; BINS], e: vec![0.0; BINS] }
    }
    fn clear(&mut self) {
        self.n.fill(0);
        self.e.fill(0.0);
    }
    fn add(&mut self, ms: f64) {
        let l = lufs(ms);
        if !(l > LO) {
            return; // the absolute gate
        }
        let i = (((l - LO) / STEP) as usize).min(BINS - 1);
        self.n[i] += 1;
        self.e[i] += ms;
    }
    /// The first bin at or above `l`.
    fn bin(l: f64) -> usize {
        (((l - LO) / STEP).ceil().max(0.0) as usize).min(BINS)
    }
    /// The mean square of the blocks in the bins from `from` on, and how many there are.
    fn mean(&self, from: usize) -> (f64, u64) {
        let (mut n, mut e) = (0u64, 0.0);
        for i in from..BINS {
            n += self.n[i] as u64;
            e += self.e[i];
        }
        (if n > 0 { e / n as f64 } else { 0.0 }, n)
    }
}

pub struct Loudness {
    k: [[Biquad; 2]; 2],
    sub_len: usize, // 100 ms
    acc: f64,
    n: usize,
    subs: [f64; 30], // the last 3 s of 100 ms mean squares (a ring)
    nsubs: u64,
    hist_i: Hist,
    hist_s: Hist,
    // true peak: the last samples of each channel, and the interpolator
    tp_x: [[f32; TP_TAPS]; 2],
    tp_h: [[f32; TP_TAPS]; 4],
    pub momentary: f64,
    pub short: f64,
    pub max_momentary: f64,
    pub max_short: f64,
    pub true_peak: f32,
    pub sample_peak: f32,
}

/// What a measurement came to.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub integrated: f64,
    pub range: f64,
    pub true_peak_db: f64,
    pub sample_peak_db: f64,
    pub max_momentary: f64,
    pub max_short: f64,
}

impl Loudness {
    pub fn new(sr: f32) -> Self {
        let sr = sr as f64;
        // 4x interpolation: a windowed sinc, split into its four phases
        let len = 4 * TP_TAPS;
        let mut tp_h = [[0f32; TP_TAPS]; 4];
        let c = (len as f64 - 1.0) / 2.0;
        for i in 0..len {
            let t = (i as f64 - c) / 4.0;
            let sinc = if t.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / len as f64).cos();
            tp_h[i % 4][i / 4] = (sinc * w) as f32;
        }
        let kw = k_weighting(sr);
        Loudness {
            k: [kw, kw],
            sub_len: ((sr / 10.0).round() as usize).max(1),
            acc: 0.0,
            n: 0,
            subs: [0.0; 30],
            nsubs: 0,
            hist_i: Hist::new(),
            hist_s: Hist::new(),
            tp_x: [[0.0; TP_TAPS]; 2],
            tp_h,
            momentary: f64::NEG_INFINITY,
            short: f64::NEG_INFINITY,
            max_momentary: f64::NEG_INFINITY,
            max_short: f64::NEG_INFINITY,
            true_peak: 0.0,
            sample_peak: 0.0,
        }
    }

    /// Start over (a new play, a new export). No allocation.
    pub fn reset(&mut self) {
        for ch in &mut self.k {
            for b in ch.iter_mut() {
                b.z1 = 0.0;
                b.z2 = 0.0;
            }
        }
        self.acc = 0.0;
        self.n = 0;
        self.subs = [0.0; 30];
        self.nsubs = 0;
        self.hist_i.clear();
        self.hist_s.clear();
        self.tp_x = [[0.0; TP_TAPS]; 2];
        self.momentary = f64::NEG_INFINITY;
        self.short = f64::NEG_INFINITY;
        self.max_momentary = f64::NEG_INFINITY;
        self.max_short = f64::NEG_INFINITY;
        self.true_peak = 0.0;
        self.sample_peak = 0.0;
    }

    fn mean_of_last(&self, k: usize) -> f64 {
        let k = k.min(self.nsubs as usize).max(1);
        let mut e = 0.0;
        for j in 0..k {
            let i = ((self.nsubs as usize + 30 - 1 - j) % 30) as usize;
            e += self.subs[i];
        }
        e / k as f64
    }

    #[inline]
    fn peak(&mut self, ch: usize, x: f32) -> f32 {
        let h = &mut self.tp_x[ch];
        h.copy_within(1.., 0);
        h[TP_TAPS - 1] = x;
        let mut m = x.abs();
        for p in &self.tp_h {
            let mut y = 0f32;
            for (a, b) in h.iter().zip(p.iter().rev()) {
                y += a * b;
            }
            m = m.max(y.abs());
        }
        m
    }

    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        for (&a, &b) in l.iter().zip(r) {
            self.sample_peak = self.sample_peak.max(a.abs()).max(b.abs());
            let tp = self.peak(0, a).max(self.peak(1, b));
            self.true_peak = self.true_peak.max(tp);
            let [k0, k1] = &mut self.k;
            let ya = k0[0].run(a as f64);
            let ya = k0[1].run(ya);
            let yb = k1[0].run(b as f64);
            let yb = k1[1].run(yb);
            self.acc += ya * ya + yb * yb;
            self.n += 1;
            if self.n == self.sub_len {
                self.subs[(self.nsubs % 30) as usize] = self.acc / self.n as f64;
                self.nsubs += 1;
                self.acc = 0.0;
                self.n = 0;
                if self.nsubs >= 4 {
                    let m = self.mean_of_last(4);
                    self.momentary = lufs(m);
                    self.max_momentary = self.max_momentary.max(self.momentary);
                    self.hist_i.add(m); // a 400 ms gating block every 100 ms (75% overlap)
                }
                if self.nsubs >= 30 {
                    let s = self.mean_of_last(30);
                    self.short = lufs(s);
                    self.max_short = self.max_short.max(self.short);
                    self.hist_s.add(s);
                } else {
                    self.short = lufs(self.mean_of_last(30));
                }
            }
        }
    }

    /// Integrated loudness: the absolute gate, then 10 LU under what passed it.
    pub fn integrated(&self) -> f64 {
        let (m, n) = self.hist_i.mean(0);
        if n == 0 {
            return f64::NEG_INFINITY;
        }
        let (m2, n2) = self.hist_i.mean(Hist::bin(lufs(m) - 10.0));
        if n2 == 0 { f64::NEG_INFINITY } else { lufs(m2) }
    }

    /// Loudness range (LU): the short-term loudness from its 10th to its 95th percentile, gated
    /// at -70 LUFS and 20 LU under the mean.
    pub fn range(&self) -> f64 {
        let (m, n) = self.hist_s.mean(0);
        if n == 0 {
            return 0.0;
        }
        let from = Hist::bin(lufs(m) - 20.0);
        let total: u64 = self.hist_s.n[from..].iter().map(|&c| c as u64).sum();
        if total == 0 {
            return 0.0;
        }
        let at = |q: f64| {
            let want = (q * total as f64).ceil().max(1.0) as u64;
            let mut seen = 0;
            for i in from..BINS {
                seen += self.hist_s.n[i] as u64;
                if seen >= want {
                    return LO + (i as f64 + 0.5) * STEP;
                }
            }
            LO + BINS as f64 * STEP
        };
        (at(0.95) - at(0.10)).max(0.0)
    }

    pub fn report(&self) -> Report {
        let db = |x: f32| if x > 0.0 { 20.0 * (x as f64).log10() } else { f64::NEG_INFINITY };
        Report {
            integrated: self.integrated(),
            range: self.range(),
            true_peak_db: db(self.true_peak),
            sample_peak_db: db(self.sample_peak),
            max_momentary: self.max_momentary,
            max_short: self.max_short,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: f32, hz: f32, db: f32, secs: f32) -> Vec<f32> {
        let a = 10f32.powf(db / 20.0);
        (0..(sr * secs) as usize).map(|i| a * (2.0 * std::f32::consts::PI * hz * i as f32 / sr).sin()).collect()
    }

    #[test]
    fn a_1k_sine_in_both_channels_reads_its_level() {
        // BS.1770: a 1 kHz sine at 0 dBFS in one channel is -3.01 LUFS; in both, 0 LUFS
        for sr in [44100.0, 48000.0, 96000.0] {
            let x = sine(sr, 1000.0, -20.0, 10.0);
            let mut m = Loudness::new(sr);
            m.process(&x, &x);
            let i = m.integrated();
            assert!((i - (-20.0)).abs() < 0.1, "{sr}: {i}");
        }
    }

    #[test]
    fn gating_ignores_silence_and_quiet_parts() {
        let sr = 48000.0;
        let loud = sine(sr, 1000.0, -20.0, 10.0);
        let mut x = loud.clone();
        x.extend(vec![0.0; 48000 * 10]); // silence: the absolute gate
        x.extend(sine(sr, 1000.0, -45.0, 10.0)); // 25 dB down: the relative gate
        let mut m = Loudness::new(sr);
        m.process(&x, &x);
        assert!((m.integrated() - (-20.0)).abs() < 0.15, "{}", m.integrated());
    }

    #[test]
    fn true_peak_sees_between_the_samples() {
        // a sine at fs/4 phased 45°: every sample is at 0.707 of the real peak
        let sr = 48000.0;
        let x: Vec<f32> = (0..48000).map(|i| (std::f32::consts::FRAC_PI_2 * (i % 4) as f32 + std::f32::consts::FRAC_PI_4).sin() * 0.9).collect();
        let mut m = Loudness::new(sr);
        m.process(&x, &x);
        let r = m.report();
        assert!((r.sample_peak_db - 20.0 * (0.9f64 * 0.7071).log10()).abs() < 0.05, "{r:?}");
        assert!((r.true_peak_db - 20.0 * 0.9f64.log10()).abs() < 0.3, "{r:?}");
    }

    #[test]
    fn range_of_two_levels() {
        let sr = 48000.0;
        let mut x = sine(sr, 1000.0, -20.0, 20.0);
        x.extend(sine(sr, 1000.0, -30.0, 20.0));
        let mut m = Loudness::new(sr);
        m.process(&x, &x);
        let lra = m.range();
        assert!((lra - 10.0).abs() < 0.6, "{lra}");
    }
}
