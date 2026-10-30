//! Stage 6 report: what lugs do to the pairs, and a render of the same hit with even lugs and with
//! one lug detuned by +3 % (the spec), +10 % and +30 %. The signal is the batter's velocity at a
//! point (the outputs come in stage 7): `renders/stage6/*.wav`.
//! `cargo run --release --example stage6_report`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Head, Hit};

fn main() {
    std::fs::create_dir_all("renders/stage6").unwrap();
    let fs = 48000.0;
    let cases: [(&str, f64); 4] = [("even", 0.0), ("lug_plus3pct", 0.03), ("lug_plus10pct", 0.10), ("lug_plus30pct", 0.30)];
    println!("split of the pair (Hz) for one lug detuned by d (θ_lug0 = π/8), the batter's lowest pairs:");
    println!("{:>14} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}", "", "(1,1)", "(2,1)", "(3,1)", "(1,2)", "(4,1)", "(2,2)");
    for (name, d) in cases {
        let mut p = DrumParams::default();
        if d != 0.0 {
            p.batter.lug_deviations = vec![d, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }
        let m = DrumModel::prepare(&p, fs).unwrap();
        print!("{name:>14}");
        for (mm, nn) in [(1, 1), (2, 1), (3, 1), (1, 2), (4, 1), (2, 2)] {
            let a = m.find_mode(Head::Batter, mm, nn, false).unwrap();
            let b = m.find_mode(Head::Batter, mm, nn, true).unwrap();
            print!(" {:>9.4}", m.modes[a].freq() - m.modes[b].freq());
        }
        println!();
        // the same hit, 4 s
        let w = m.projection(0.4, 1.0);
        let mut v = DrumVoice::new(&m);
        v.trigger(&m, Hit { velocity: 0.8, radius: 0.55, angle: 0.0, sample_offset: 0 });
        let mut sig = Vec::new();
        for _ in 0..(4.0 * fs) as usize {
            v.step(&m);
            sig.push(w.iter().zip(v.p(&m)).map(|(w, p)| w * p).sum::<f64>());
        }
        let peak = sig.iter().fold(0f64, |a, x| a.max(x.abs()));
        let spec = hound::WavSpec { channels: 1, sample_rate: fs as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut wr = hound::WavWriter::create(format!("renders/stage6/{name}.wav"), spec).unwrap();
        for x in &sig {
            wr.write_sample((x / peak * 0.8 * 32767.0) as i16).unwrap();
        }
        wr.finalize().unwrap();
    }
    println!("\nthe (1,1) pair of one +10 % lug: its principal axes sit at the lug's angle (φ = π/8 = {:.3} rad)", std::f64::consts::PI / 8.0);
    println!("(the split is first order in the lug's detune and falls off with m, because one lug's bump has little at high harmonics)");
}
