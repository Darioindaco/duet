//! Stem splitting: a mix into drums, bass, vocals and the rest, with Demucs
//! (htdemucs, Meta's hybrid transformer model, MIT licensed).
//!
//! The network itself runs in ONNX Runtime (the model file is downloaded
//! once, see stems.py's side in daw.py); everything around it is here and
//! follows Demucs exactly: its spectrogram (a normalized STFT with its own
//! padding), the two branches' outputs added back together, and the song done
//! in 7.8-second stretches that overlap by a quarter and are blended with a
//! triangle. All of it on this machine: nothing leaves it.

use std::path::Path;
use std::sync::Arc;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;

use crate::daw::{resample, AudioData};

pub const SR: u32 = 44100;
/// in the model's order
pub const SOURCES: [&str; 4] = ["drums", "bass", "other", "vocals"];
const NFFT: usize = 4096;
const HOP: usize = 1024;
const BINS: usize = NFFT / 2; // the model leaves out the top bin
/// the length the model was trained on and exported at (7.8 s)
const SEG: usize = 343_980;
const FRAMES: usize = SEG.div_ceil(HOP); // 336
const OVERLAP: f64 = 0.25;

pub struct Separator {
    session: Session,
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    ifft: Arc<dyn realfft::ComplexToReal<f32>>,
    window: Vec<f32>,
}

impl Separator {
    pub fn load(model: &Path) -> Result<Self, String> {
        let env = ort::init().with_name("duet").build().map_err(oerr)?;
        let session = Session::builder(&env)
            .map_err(oerr)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(oerr)?
            .commit_from_file(model)
            .map_err(oerr)?;
        let mut planner = RealFftPlanner::<f32>::new();
        // periodic Hann, as torch.hann_window
        let window = (0..NFFT).map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / NFFT as f32).cos()).collect();
        Ok(Separator { session, fft: planner.plan_fft_forward(NFFT), ifft: planner.plan_fft_inverse(NFFT), window })
    }

    /// `a` split into the four stems (SOURCES order), at 44.1 kHz stereo.
    /// `progress` hears 0..1 after each stretch and says whether to go on.
    pub fn separate(&mut self, a: &AudioData, progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<AudioData>, String> {
        // stereo at the model's rate
        let mut l = resample(&a.samples, a.sr, SR);
        let mut r = match &a.right {
            Some(r) => resample(r, a.sr, SR),
            None => l.clone(),
        };
        let n = l.len();
        if n == 0 {
            return Err("there's no audio there".into());
        }
        // Demucs's normalization: by the mono mix's mean and spread
        let (mean, std) = {
            let m: f64 = l.iter().zip(&r).map(|(a, b)| 0.5 * (*a + *b) as f64).sum::<f64>() / n as f64;
            let v: f64 = l.iter().zip(&r).map(|(a, b)| (0.5 * (*a + *b) as f64 - m).powi(2)).sum::<f64>() / (n.max(2) - 1) as f64;
            (m as f32, (v.sqrt() as f32).max(1e-8))
        };
        for x in l.iter_mut().chain(r.iter_mut()) {
            *x = (*x - mean) / std;
        }
        // stretches, a quarter overlapping, blended with a triangle
        let stride = ((1.0 - OVERLAP) * SEG as f64) as usize;
        let weight: Vec<f32> = (1..=SEG / 2).chain((1..=SEG - SEG / 2).rev()).map(|k| k as f32).collect();
        let wmax = weight.iter().cloned().fold(0.0, f32::max);
        let mut out = vec![vec![0f32; n]; 8]; // source * 2 + channel
        let mut sum_w = vec![0f32; n];
        let offsets: Vec<usize> = (0..n).step_by(stride).collect();
        for (i, &off) in offsets.iter().enumerate() {
            let len = SEG.min(n - off);
            // short (the end): centred in a full-length window, with what's before it
            let start = off as i64 - (SEG - len) as i64 / 2;
            let take = |x: &[f32]| -> Vec<f32> {
                (0..SEG as i64).map(|k| {
                    let at = start + k;
                    if at >= 0 && (at as usize) < n { x[at as usize] } else { 0.0 }
                }).collect()
            };
            let (cl, cr) = (take(&l), take(&r));
            let res = self.segment(&cl, &cr)?; // [8][SEG]
            let trim = (SEG - len) / 2;
            for (s, o) in out.iter_mut().enumerate() {
                for k in 0..len {
                    o[off + k] += weight[k] / wmax * res[s][trim + k];
                }
            }
            for k in 0..len {
                sum_w[off + k] += weight[k] / wmax;
            }
            if !progress((i + 1) as f32 / offsets.len() as f32) {
                return Err("stopped".into());
            }
        }
        let mut it = out.into_iter();
        let mut stems = Vec::with_capacity(4);
        for _ in 0..4 {
            let mut ch = [it.next().unwrap(), it.next().unwrap()];
            for c in ch.iter_mut() {
                for (k, v) in c.iter_mut().enumerate() {
                    *v = *v / sum_w[k] * std + mean;
                }
            }
            let [sl, sr] = ch;
            stems.push(AudioData { sr: SR, samples: sl, right: Some(sr) });
        }
        Ok(stems)
    }

    /// One stretch of SEG samples through the model: [source*2 + channel][SEG].
    fn segment(&mut self, l: &[f32], r: &[f32]) -> Result<Vec<Vec<f32>>, String> {
        let specs = [self.spec(l), self.spec(r)]; // [ch][frame][bin]
        // the model's input: [1, ch*2 (re, im), BINS, FRAMES]
        let mut mag = vec![0f32; 4 * BINS * FRAMES];
        for (c, z) in specs.iter().enumerate() {
            for t in 0..FRAMES {
                for f in 0..BINS {
                    let v = z[t][f];
                    mag[((2 * c) * BINS + f) * FRAMES + t] = v.re;
                    mag[((2 * c + 1) * BINS + f) * FRAMES + t] = v.im;
                }
            }
        }
        let mut mix = Vec::with_capacity(2 * SEG);
        mix.extend_from_slice(l);
        mix.extend_from_slice(r);
        let mix = Tensor::from_array(([1usize, 2, SEG], mix.into_boxed_slice())).map_err(oerr)?;
        let mag = Tensor::from_array(([1usize, 4, BINS, FRAMES], mag.into_boxed_slice())).map_err(oerr)?;
        let outs = self.session.run(ort::inputs!["mix" => mix, "mag" => mag]).map_err(oerr)?;
        let (_, spec) = outs["spec"].try_extract_tensor::<f32>().map_err(oerr)?; // [1, 4, 4, BINS, FRAMES]
        let (_, wave) = outs["wave"].try_extract_tensor::<f32>().map_err(oerr)?; // [1, 4, 2, SEG]
        let (spec, wave) = (spec.to_vec(), wave.to_vec());
        drop(outs);
        let mut res = Vec::with_capacity(8);
        for s in 0..4 {
            for c in 0..2 {
                let at = |k: usize, f: usize, t: usize| spec[(((s * 4) + k) * BINS + f) * FRAMES + t];
                let frames: Vec<Vec<Complex32>> = (0..FRAMES)
                    .map(|t| (0..BINS).map(|f| Complex32::new(at(2 * c, f, t), at(2 * c + 1, f, t))).collect())
                    .collect();
                let mut x = self.ispec(&frames);
                let w = &wave[(s * 2 + c) * SEG..(s * 2 + c + 1) * SEG];
                for (v, t) in x.iter_mut().zip(w) {
                    *v += t;
                }
                res.push(x);
            }
        }
        Ok(res)
    }

    /// Demucs's spectrogram of SEG samples: FRAMES frames of BINS bins.
    fn spec(&self, x: &[f32]) -> Vec<Vec<Complex32>> {
        // its own padding (so frames line up with hops), then torch.stft's
        // centring (n_fft/2 each side), both reflecting
        let pad = HOP / 2 * 3;
        let extra = FRAMES * HOP - x.len();
        let a = reflect(x, pad, pad + extra);
        let b = reflect(&a, NFFT / 2, NFFT / 2);
        let n_frames = 1 + (b.len() - NFFT) / HOP; // FRAMES + 4
        let scale = 1.0 / (NFFT as f32).sqrt(); // normalized=True
        let mut buf = vec![0f32; NFFT];
        let mut spec = self.fft.make_output_vec();
        (2..2 + FRAMES.min(n_frames - 2))
            .map(|t| {
                for (k, v) in buf.iter_mut().enumerate() {
                    *v = b[t * HOP + k] * self.window[k];
                }
                self.fft.process(&mut buf, &mut spec).expect("fft sizes");
                spec[..BINS].iter().map(|v| v * scale).collect()
            })
            .collect()
    }

    /// Back to SEG samples (Demucs's _ispec: the top bin and two frames each side back as zeros).
    fn ispec(&self, frames: &[Vec<Complex32>]) -> Vec<f32> {
        let pad = HOP / 2 * 3;
        let le = HOP * SEG.div_ceil(HOP) + 2 * pad;
        let n_frames = frames.len() + 4;
        let total = NFFT + HOP * (n_frames - 1);
        let mut y = vec![0f32; total];
        let mut env = vec![0f32; total];
        let mut spec = self.ifft.make_input_vec();
        let mut buf = vec![0f32; NFFT];
        let scale = (NFFT as f32).sqrt() / NFFT as f32; // undo normalized=True; the inverse FFT is unscaled
        for t in 0..n_frames {
            let at = t * HOP;
            for (k, w) in self.window.iter().enumerate() {
                env[at + k] += w * w;
            }
            if t < 2 || t >= 2 + frames.len() {
                continue; // a zero frame
            }
            let z = &frames[t - 2];
            for (k, v) in spec.iter_mut().enumerate() {
                *v = if k < BINS { z[k] * scale } else { Complex32::new(0.0, 0.0) };
            }
            spec[0].im = 0.0; // (real at DC and Nyquist, as irfft takes them)
            spec[NFFT / 2].im = 0.0;
            self.ifft.process(&mut spec, &mut buf).expect("fft sizes");
            for (k, v) in buf.iter().enumerate() {
                y[at + k] += v * self.window[k];
            }
        }
        // torch.istft: divided by the window's overlap, the centring taken off, `le` long
        let off = NFFT / 2;
        (0..le)
            .map(|k| {
                let i = off + k;
                if i < total && env[i] > 1e-11 { y[i] / env[i] } else { 0.0 }
            })
            .skip(pad)
            .take(SEG)
            .collect()
    }
}

fn oerr(e: impl std::fmt::Display) -> String {
    format!("the stem model: {e}")
}

/// `x` with `left` / `right` samples mirrored on (torch's "reflect": the edge sample isn't repeated).
fn reflect(x: &[f32], left: usize, right: usize) -> Vec<f32> {
    let n = x.len() as i64;
    let at = |i: i64| -> f32 {
        // fold back and forth until it's inside (for pads longer than the signal too)
        let p = 2 * (n - 1).max(1);
        let mut j = i.rem_euclid(p);
        if j >= n {
            j = p - j;
        }
        x[j as usize]
    };
    (-(left as i64)..n + right as i64).map(at).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_pads_like_torch() {
        assert_eq!(reflect(&[1.0, 2.0, 3.0, 4.0], 2, 2), vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
    }

    /// The real model on the reference mix, against PyTorch Demucs's own
    /// result (made by tools/stems_reference.py): DUET_STEMS_DIR=dir with
    /// htdemucs.onnx, ref_mix.bin and ref_stems.bin.
    #[test]
    #[ignore]
    fn matches_pytorch_demucs() {
        let dir = std::path::PathBuf::from(std::env::var("DUET_STEMS_DIR").expect("DUET_STEMS_DIR"));
        let read = |p: &str| -> Vec<f32> {
            std::fs::read(dir.join(p)).unwrap().chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
        };
        let mix = read("ref_mix.bin"); // interleaved stereo
        let a = AudioData { sr: SR, samples: mix.iter().step_by(2).copied().collect(), right: Some(mix.iter().skip(1).step_by(2).copied().collect()) };
        let mut sep = Separator::load(&dir.join("htdemucs.onnx")).unwrap();
        let t = std::time::Instant::now();
        let stems = sep.separate(&a, &mut |p| { eprintln!("{:.0}%", p * 100.0); true }).unwrap();
        eprintln!("separated {:.1} s of audio in {:?}", a.samples.len() as f32 / SR as f32, t.elapsed());
        let want = read("ref_stems.bin"); // [4][2][n]
        let n = a.samples.len();
        for (s, st) in stems.iter().enumerate() {
            for (c, x) in [&st.samples, st.right.as_ref().unwrap()].into_iter().enumerate() {
                let w = &want[(s * 2 + c) * n..(s * 2 + c + 1) * n];
                let worst = x.iter().zip(w).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
                let peak = w.iter().fold(0f32, |m, v| m.max(v.abs()));
                eprintln!("{} ch{c}: off by {worst:.2e} (peak {peak:.3})", SOURCES[s]);
                assert!(worst < 2e-3, "{} ch{c} off by {worst}", SOURCES[s]);
            }
        }
    }
}
