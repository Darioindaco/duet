//! What the master sounds like, for the Master window: a spectrum (1/3-octave-ish bands, dB) and
//! the stereo correlation (+1 mono, 0 wide, -1 out of phase). Real-time safe once made: the FFT is
//! planned and every buffer allocated up front.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use realfft::num_complex::Complex;

pub const BANDS: usize = 31;
const N: usize = 4096;

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    ring: Vec<f32>,
    at: usize,
    filled: usize,
    since: usize,
    hop: usize,
    window: Vec<f32>,
    input: Vec<f32>,
    spec: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// which FFT bins make each band
    edges: Vec<(usize, usize)>,
    pub bands: Vec<f32>,
    // correlation: decaying sums
    lr: f64,
    ll: f64,
    rr: f64,
    pub correlation: f32,
}

impl Analyzer {
    pub fn new(sr: f32) -> Self {
        Self::with_bands(sr, BANDS, 20.0)
    }

    /// `nb` bands from 20 Hz to 20 kHz, updated `rate` times a second.
    pub fn with_bands(sr: f32, nb: usize, rate: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
        let window = (0..N).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / N as f32).cos()).collect();
        let edges = (0..nb)
            .map(|b| {
                let f = |x: f32| 20.0 * 1000f32.powf(x / nb as f32);
                let bin = |hz: f32| ((hz / sr * N as f32).round() as usize).clamp(1, N / 2);
                let (a, z) = (bin(f(b as f32)), bin(f(b as f32 + 1.0)));
                (a, z.max(a + 1))
            })
            .collect();
        let scratch = fft.make_scratch_vec();
        Analyzer {
            ring: vec![0.0; N],
            at: 0,
            filled: 0,
            since: 0,
            hop: (sr / rate) as usize,
            input: vec![0.0; N],
            spec: fft.make_output_vec(),
            scratch,
            fft,
            window,
            edges,
            bands: vec![-120.0; nb],
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
            correlation: 1.0,
        }
    }

    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        let n = l.len().min(r.len());
        // correlation over about a third of a second
        let decay = 0.9999f64.powi(n as i32 * 2);
        let (mut lr, mut ll, mut rr) = (0f64, 0f64, 0f64);
        for k in 0..n {
            let (a, b) = (l[k] as f64, r[k] as f64);
            lr += a * b;
            ll += a * a;
            rr += b * b;
            self.ring[self.at] = 0.5 * (l[k] + r[k]);
            self.at = (self.at + 1) % N;
        }
        self.lr = self.lr * decay + lr;
        self.ll = self.ll * decay + ll;
        self.rr = self.rr * decay + rr;
        let den = (self.ll * self.rr).sqrt();
        self.correlation = if den > 1e-9 { (self.lr / den).clamp(-1.0, 1.0) as f32 } else { 1.0 };
        self.filled = (self.filled + n).min(N);
        self.since += n;
        if self.since < self.hop || self.filled < N {
            return;
        }
        self.since = 0;
        for k in 0..N {
            self.input[k] = self.ring[(self.at + k) % N] * self.window[k];
        }
        if self.fft.process_with_scratch(&mut self.input, &mut self.spec, &mut self.scratch).is_err() {
            return;
        }
        let norm = 2.0 / (N as f32 * 0.5); // a full-scale sine reads about 0 dB
        for (b, &(a, z)) in self.edges.iter().enumerate() {
            let mut p = 0f32;
            for c in &self.spec[a..z.min(self.spec.len())] {
                p = p.max(c.norm() * norm);
            }
            let db = 20.0 * p.max(1e-7).log10();
            // fast up, slow down
            self.bands[b] = if db > self.bands[b] { db } else { self.bands[b] * 0.8 + db * 0.2 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lights_its_band_and_mono_correlates() {
        let sr = 48000.0;
        let mut a = Analyzer::new(sr);
        let x: Vec<f32> = (0..48000).map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()).collect();
        for c in x.chunks(256) {
            a.process(c, c);
        }
        let top = (0..BANDS).max_by(|&i, &j| a.bands[i].total_cmp(&a.bands[j])).unwrap();
        let center = 20.0 * 1000f32.powf((top as f32 + 0.5) / BANDS as f32);
        assert!((center / 1000.0).log2().abs() < 0.34, "loudest band at {center} Hz");
        assert!(a.bands[top] > -12.0 && a.bands[top] < 0.0, "{}", a.bands[top]);
        assert!(a.correlation > 0.99);
        let neg: Vec<f32> = x.iter().map(|v| -v).collect();
        for (c, d) in x.chunks(256).zip(neg.chunks(256)) {
            a.process(c, d);
        }
        assert!(a.correlation < -0.9, "{}", a.correlation);
    }
}
