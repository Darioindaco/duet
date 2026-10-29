//! Stage 3 report: the stick's contact on the bare batter head, for the spec's K_s = 1e8 and
//! the default K_s = 1e6.
//! `cargo run --release --example stage3_report`

use duet_drum::analysis::spectral_centroid;
use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

struct Contact {
    span_ms: f64,
    pulses: usize,
    peak: f64,
    impulse: f64,
    v_out: f64,
    sav: f64,
}

fn contact(m: &DrumModel, vel: f32, r: f64) -> Contact {
    let sp = &m.params.stick;
    let mut v = DrumVoice::new(m);
    v.trigger(m, Hit { velocity: vel, radius: r, angle: 0.3, sample_offset: 0 });
    let (mut first, mut last, mut peak, mut impulse, mut pulses, mut prev) = (None, 0usize, 0f64, 0f64, 0usize, false);
    let mut samples = Vec::new();
    for s in 0..(0.1 * m.fs) as usize {
        v.step(m);
        let on = v.force() > 0.0;
        if on {
            first.get_or_insert(s);
            last = s;
            peak = peak.max(v.force());
            impulse += v.force() / m.fs;
            if !prev {
                pulses += 1;
            }
            samples.push(v.force());
        }
        prev = on;
    }
    // ψ against its definition ψ² = 2V + ε, V = F^((α+1)/α) / (K^(1/α)(α+1)), where V is large
    let pot = |f: f64| f.powf((sp.exponent + 1.0) / sp.exponent) / (sp.stiffness.powf(1.0 / sp.exponent) * (sp.exponent + 1.0));
    let vmax = pot(peak);
    let mut sav = 0f64;
    let mut w = DrumVoice::new(m);
    w.trigger(m, Hit { velocity: vel, radius: r, angle: 0.3, sample_offset: 0 });
    for _ in 0..(0.1 * m.fs) as usize {
        w.step(m);
        if pot(w.force()) > 0.05 * vmax {
            sav = sav.max(((w.psi() * w.psi() - (2.0 * pot(w.force()) + m.params.sav_epsilon)) / (2.0 * vmax)).abs());
        }
    }
    Contact { span_ms: (last - first.unwrap()) as f64 / m.fs * 1e3, pulses, peak, impulse, v_out: (2.0 * v.stick_energy_removed() / sp.mass).sqrt(), sav }
}

fn main() {
    for (label, ks) in [("K_s = 1e8 (the spec)", 1e8), ("K_s = 1e6 (the default)", 1e6)] {
        for fs in [48000.0, 96000.0] {
            let mut p = DrumParams::default();
            p.stick.stiffness = ks;
            let m = DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap();
            println!("== {label}, fs = {fs} Hz, {} modes ==", m.len());
            println!("first-to-last contact (ms) · separate pulses · peak force (N) · impulse (N·s) · v_out/v_in · SAV: |ψ²−(2V+ε)|/2V_peak where V > 5 % of peak");
            for vel in [0.1f32, 0.4, 0.7, 1.0] {
                print!("vel {vel:<4}");
                for r in [0.0, 0.3, 0.6, 0.9] {
                    let c = contact(&m, vel, r);
                    let v_in = p.stick.v_min + (p.stick.v_max - p.stick.v_min) * (vel as f64).powf(p.stick.velocity_gamma);
                    let flag = if !(0.5..=4.0).contains(&c.span_ms) { "*" } else { " " };
                    print!(" | r={r}: {:5.1}{flag} {:3} {:6.0} {:.4} {:.2} {:.0e}", c.span_ms, c.pulses, c.peak, c.impulse, c.v_out / v_in, c.sav);
                }
                println!();
            }
            println!();
        }
    }
    println!("(* = outside the plausible 0.5–4 ms)\n\nSpectral centroid of the head's acceleration over 0.25 s, vel 0.1 / 0.4 / 0.7 / 1.0:");
    for (label, ks) in [("K_s = 1e8", 1e8), ("K_s = 1e6", 1e6)] {
        let mut p = DrumParams::default();
        p.stick.stiffness = ks;
        let m = DrumModel::prepare_single_head(&p, Head::Batter, 48000.0).unwrap();
        let mut cs = Vec::new();
        for vel in [0.1f32, 0.4, 0.7, 1.0] {
            let mut v = DrumVoice::new(&m);
            v.trigger(&m, Hit { velocity: vel, radius: 0.5, angle: 0.4, sample_offset: 0 });
            let w = m.projection(0.4, 2.0);
            let mut acc = Vec::new();
            for _ in 0..12000 {
                v.step(&m);
                acc.push(v.acceleration(&w));
            }
            cs.push(spectral_centroid(&acc, 48000.0));
        }
        println!("  {label:<11} {cs:.0?} Hz");
    }
}
