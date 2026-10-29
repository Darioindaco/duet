//! Does the contact converge as the sample rate goes up? Vel 0.7, r = 0.3, for a few contact
//! stiffnesses K_s and patch radii. `cargo run --release --example stage3_convergence`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

fn main() {
    println!("{:>8} {:>7} {:>7} | {:>7} {:>7} {:>8} {:>8} {:>8}", "K_s", "patch", "fs kHz", "ms", "pulses", "peak N", "v_out/in", "SAV err");
    for ks in [1e8, 1e7, 1e6] {
        for a in [0.0, 0.005] {
            for fs in [48000.0, 96000.0, 192000.0] {
                let mut p = DrumParams::default();
                p.stick.stiffness = ks;
                p.contact_radius = a;
                let m = DrumModel::prepare_single_head(&p, Head::Batter, fs).unwrap();
                let sp = &p.stick;
                let mut v = DrumVoice::new(&m);
                v.trigger(&m, Hit { velocity: 0.7, radius: 0.3, angle: 0.3, sample_offset: 0 });
                let pot = |f: f64| f.powf((sp.exponent + 1.0) / sp.exponent) / (sp.stiffness.powf(1.0 / sp.exponent) * (sp.exponent + 1.0));
                let (mut first, mut last, mut peak, mut pulses, mut prev) = (None, 0usize, 0f64, 0, false);
                let mut tr = Vec::new();
                for s in 0..(0.1 * fs) as usize {
                    v.step(&m);
                    let on = v.force() > 0.0;
                    if on {
                        first.get_or_insert(s);
                        last = s;
                        peak = peak.max(v.force());
                        if !prev {
                            pulses += 1;
                        }
                    }
                    prev = on;
                    tr.push((v.force(), v.psi()));
                }
                let vmax = pot(peak);
                let mut sav = 0f64;
                for (f, psi) in &tr {
                    if pot(*f) > 0.05 * vmax {
                        sav = sav.max(((psi * psi - (2.0 * pot(*f) + p.sav_epsilon)) / (2.0 * vmax)).abs());
                    }
                }
                let v_in = sp.v_min + (sp.v_max - sp.v_min) * 0.7f64.powf(sp.velocity_gamma);
                println!("{ks:>8.0e} {:>5.0}mm {:>7.0} | {:>7.2} {:>7} {:>8.1} {:>8.3} {:>8.1e}", a * 1e3, fs / 1e3, (last - first.unwrap()) as f64 / fs * 1e3, pulses, peak, (2.0 * v.stick_energy_removed() / sp.mass).sqrt() / v_in, sav);
            }
        }
    }
}
