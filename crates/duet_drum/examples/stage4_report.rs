//! Stage 4 report: tension modulation. Pitch glide of the lowest partials against velocity
//! (the instantaneous frequency of the (0,1) and (1,1) modes, from their state), at three rates.
//! `cargo run --release --example stage4_report`

use duet_drum::analysis::mode_frequency_track;
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

/// Frequency (Hz) of a mode in windows centred at the given times, and its peak tension rise.
fn track(m: &DrumModel, vel: f32, mode: usize, times: &[f64]) -> (Vec<f64>, f64) {
    let mut v = DrumVoice::new(m);
    v.trigger(m, Hit { velocity: vel, radius: 0.5, angle: 0.4, sample_offset: 0 });
    let n = (0.8 * m.fs) as usize;
    let (mut x, mut p, mut peak) = (Vec::with_capacity(n), Vec::with_capacity(n), 0f64);
    for _ in 0..n {
        v.step(m);
        x.push(v.x()[mode]);
        p.push(v.p()[mode]);
        peak = peak.max(v.tension_rise(m));
    }
    let mo = &m.modes[mode];
    let win = (0.04 * m.fs) as usize;
    let tr = mode_frequency_track(&x, &p, mo.omega, mo.sigma, m.fs, win);
    let f: Vec<f64> = times.iter().map(|t| tr.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).unwrap().1).collect();
    (f, peak)
}

fn main() {
    let times = [0.035, 0.1, 0.2, 0.5];
    for fs in [48000.0, 96000.0, 192000.0] {
        let m = DrumModel::prepare_single_head(&DrumParams::default(), Head::Batter, fs).unwrap();
        println!("== fs = {fs} Hz: glide in cents above the linear frequency, at t = 35 / 100 / 200 / 500 ms; peak ΔT/T ==");
        for (label, (mm, nn)) in [("(0,1)", (0usize, 1usize)), ("(1,1)", (1, 1))] {
            let mode = m.find_mode(Head::Batter, mm, nn, false).unwrap();
            let f0 = m.modes[mode].freq();
            for vel in [0.1f32, 0.4, 0.7, 1.0] {
                let (f, rise) = track(&m, vel, mode, &times);
                let cents: Vec<String> = f.iter().map(|f| format!("{:6.1}", 1200.0 * (f / f0).log2())).collect();
                println!("  {label} {f0:6.1} Hz, vel {vel:<4}: {} cents   ΔT/T = {:5.1} %", cents.join(" "), rise * 100.0);
            }
        }
        println!();
    }
}
