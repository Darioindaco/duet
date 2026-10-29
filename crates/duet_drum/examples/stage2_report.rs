//! Stage 2 report: the first ten modes of the batter head, measured against what
//! the model says, at 44.1 and 96 kHz; the undamped energy drift; and a render of
//! the bare head (a membrane in vacuo, struck by an impulse) to renders/stage2/.

use duet_drum::analysis::{log_slope, magnitude_spectrum, peak_near};
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head};

fn main() {
    std::fs::create_dir_all("renders/stage2").unwrap();
    for fs in [44100.0, 96000.0] {
        let p = DrumParams::default();
        let m = DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap();
        let mut v = DrumVoice::new(&m);
        v.test_impulse(&m, 0.55, 0.3, 1.0);
        let n = (6.0 * fs) as usize;
        let mic_w = m.projection(0.35, 1.1);
        let firsts: Vec<usize> = m.by_frequency().into_iter().filter(|&i| !m.modes[i].sin).take(10).collect();
        let mut env = vec![Vec::new(); 10];
        let mut sig = Vec::with_capacity(n);
        for _ in 0..n {
            v.step_linear(&m, None);
            sig.push(v.displacement(&mic_w));
            for (e, &i) in env.iter_mut().zip(&firsts) {
                let (x, pp, mo) = (v.x()[i], v.p()[i], &m.modes[i]);
                e.push((x * x + ((pp + mo.sigma * x) / mo.omega).powi(2)).sqrt());
            }
        }
        let n_fft = 1 << 20;
        let mag = magnitude_spectrum(&sig, n_fft);
        println!("fs = {fs} Hz: {} modes (degrees of freedom) up to {} Hz", m.len(), p.batter.f_max);
        println!("  {:>5} {:>10} {:>10} {:>8} {:>8} {:>8} {:>9}", "(m,n)", "f model", "f FFT", "err ppm", "σ model", "σ meas", "err %");
        for (b, &i) in firsts.iter().enumerate() {
            let mo = &m.modes[i];
            let f = peak_near(&mag, fs, n_fft, mo.freq(), 3.0);
            let len = ((4.0 / mo.sigma) * fs) as usize;
            let t: Vec<f64> = (0..len).map(|s| s as f64 / fs).collect();
            let s = -log_slope(&t, &env[b][..len]);
            println!("  ({},{}) {:>10.3} {:>10.3} {:>8.0} {:>8.3} {:>8.3} {:>9.3}", mo.m, mo.n, mo.freq(), f, (f / mo.freq() - 1.0) * 1e6, mo.sigma, s, (s / mo.sigma - 1.0) * 100.0);
        }
        // undamped energy
        let mut q = p.clone();
        q.batter.sigma0 = 0.0;
        q.batter.sigma1 = 0.0;
        let mu = DrumModel::prepare_single_head(&q, Head::Batter, fs).unwrap();
        let mut vu = DrumVoice::new(&mu);
        vu.test_impulse(&mu, 0.55, 0.3, 1.0);
        let e0 = vu.energy_linear(&mu);
        let mut worst = 0f64;
        for s in 0..(10.0 * fs) as usize {
            vu.step_linear(&mu, None);
            if s % 97 == 0 {
                worst = worst.max(((vu.energy_linear(&mu) - e0) / e0).abs());
            }
        }
        println!("  undamped energy drift over 10 s: {worst:.2e}\n");
        if fs == 44100.0 {
            let peak = sig.iter().fold(0f64, |a, x| a.max(x.abs()));
            let spec = hound::WavSpec { channels: 1, sample_rate: fs as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut w = hound::WavWriter::create("renders/stage2/batter_impulse.wav", spec).unwrap();
            for x in &sig {
                w.write_sample((x / peak * 0.8 * 32767.0) as i16).unwrap();
            }
            w.finalize().unwrap();
        }
    }
}
