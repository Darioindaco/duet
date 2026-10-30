//! Stage 7 renders (48 kHz, mono, 32-bit float, the output gains as they are: nothing is normalised or clipped):
//! the velocity × radius grid, a 16-stroke roll at 12 Hz, and the lug comparison; for both outputs.
//! Writes `renders/stage7/` and its README. `cargo run --release --example stage7_render`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit};
use std::fmt::Write;

const FS: f64 = 48000.0;

fn render(m: &DrumModel, hits: &[(usize, Hit)], total: usize) -> (Vec<f32>, Vec<f32>) {
    let block = 512;
    let mut v = DrumVoice::new(m);
    let (mut close, mut far) = (Vec::with_capacity(total), Vec::with_capacity(total));
    let (mut c, mut f) = (vec![0f32; block], vec![0f32; block]);
    let mut t = 0;
    while t < total {
        let len = block.min(total - t);
        for (at, h) in hits {
            if *at >= t && *at < t + len {
                v.trigger(m, Hit { sample_offset: (*at - t) as u32, ..*h });
            }
        }
        v.process(m, &mut c[..len], &mut f[..len]);
        close.extend_from_slice(&c[..len]);
        far.extend_from_slice(&f[..len]);
        t += len;
    }
    (close, far)
}

fn write(path: &str, x: &[f32]) {
    let spec = hound::WavSpec { channels: 1, sample_rate: FS as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for v in x {
        w.write_sample(*v).unwrap();
    }
    w.finalize().unwrap();
}

fn peak_db(x: &[f32]) -> f64 {
    20.0 * (x.iter().fold(1e-9f32, |a, v| a.max(v.abs())) as f64).log10()
}

fn main() {
    let dir = "renders/stage7";
    std::fs::create_dir_all(dir).unwrap();
    let mut readme = String::from("# duet_drum, stage 7 renders\n\n48 kHz, mono, 32-bit float, `cargo run --release --example stage7_render`. Every file comes as `*_close.wav` (the close mic, 6 cm above the batter at half the radius) and `*_far.wav` (the monopole at 1.5 m). The output gains are the defaults (a velocity-1.0 hit at the centre peaks at −6 dBFS); nothing is normalised, so the levels between files are comparable.\n\n");
    let m = DrumModel::prepare(&DrumParams::default(), FS).unwrap();

    // ---- the grid: 4 velocities × 4 radii, one hit every 1.5 s ----
    let (vels, radii) = ([0.15f32, 0.4, 0.7, 1.0], [0.0f64, 0.3, 0.6, 0.85]);
    let step = (1.5 * FS) as usize;
    let mut hits = Vec::new();
    let mut order = String::new();
    for (iv, v) in vels.iter().enumerate() {
        for (ir, r) in radii.iter().enumerate() {
            let k = iv * 4 + ir;
            hits.push((k * step, Hit { velocity: *v, radius: *r, angle: 0.0, sample_offset: 0 }));
            writeln!(order, "| {:4.1} s | {v} | {r} |", k as f64 * 1.5).unwrap();
        }
    }
    let (c, f) = render(&m, &hits, 16 * step);
    write(&format!("{dir}/grid_close.wav"), &c);
    write(&format!("{dir}/grid_far.wav"), &f);
    writeln!(readme, "## grid_close.wav, grid_far.wav\n\nSixteen hits, one every 1.5 s (a later hit rings into the end of the one before; the head is the same voice), velocity-major.\n\n| at | velocity | radius (× R) |\n|---|---|---|\n{order}").unwrap();
    // where the close mic is loudest (the dipole term goes as 1/d_s: loudest under the mic)
    let mut best = (f32::MIN, 0usize);
    for k in 0..16 {
        let pk = c[k * step..(k + 1) * step].iter().fold(0f32, |a, v| a.max(v.abs()));
        if pk > best.0 {
            best = (pk, k);
        }
        print!("{:5.1} ", 20.0 * (pk.max(1e-9) as f64).log10());
        if k % 4 == 3 {
            println!("  (velocity {}; radius 0 / 0.3 / 0.6 / 0.85, close peaks in dBFS)", vels[k / 4]);
        }
    }
    println!("grid: close peak {:.1} dBFS, far peak {:.1} dBFS", peak_db(&c), peak_db(&f));

    // ---- a roll: 16 strokes at 12 Hz, radius alternating 0.3 / 0.35 ----
    let hits: Vec<(usize, Hit)> = (0..16)
        .map(|i| ((i as f64 * FS / 12.0) as usize, Hit { velocity: 0.7, radius: if i % 2 == 0 { 0.3 } else { 0.35 }, angle: 0.5 * i as f64, sample_offset: 0 }))
        .collect();
    let (c, f) = render(&m, &hits, (3.5 * FS) as usize);
    write(&format!("{dir}/roll_close.wav"), &c);
    write(&format!("{dir}/roll_far.wav"), &f);
    writeln!(readme, "## roll_close.wav, roll_far.wav\n\nSixteen strokes at 12 Hz, velocity 0.7, radius alternating 0.3 / 0.35, each at a new angle; one voice, so every stroke lands on a head still ringing from the last.\n").unwrap();
    println!("roll: close peak {:.1} dBFS, far peak {:.1} dBFS", peak_db(&c), peak_db(&f));

    // ---- the lugs: the same hit with even lugs, and with one lug detuned ----
    for (name, d) in [("lugs_even", 0.0), ("lugs_plus3pct", 0.03), ("lugs_plus30pct", 0.30)] {
        let mut p = DrumParams::default();
        if d != 0.0 {
            p.batter.lug_deviations = vec![d, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }
        let m = DrumModel::prepare(&p, FS).unwrap();
        let (c, f) = render(&m, &[(0, Hit { velocity: 0.7, radius: 0.6, angle: 0.0, sample_offset: 0 })], (5.0 * FS) as usize);
        write(&format!("{dir}/{name}_close.wav"), &c);
        write(&format!("{dir}/{name}_far.wav"), &f);
    }
    writeln!(readme, "## lugs_even, lugs_plus3pct, lugs_plus30pct\n\nThe same hit (velocity 0.7, radius 0.6, 5 s) with all lugs even, with one lug 3 % tighter (the spec) and with one 30 % tighter. The splits are 0.1 Hz and 1 Hz for the (1,1) pair, so only the last one has a chance of being heard as a beat (stage-6 report).\n").unwrap();
    std::fs::write(format!("{dir}/README.md"), readme).unwrap();
    println!("written to {dir}/");
}
