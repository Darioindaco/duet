//! Prints the peak of both outputs for a velocity-1.0 hit at the centre, with the gains as set in
//! the parameters, and the gain (dB) that would put them at −6 dBFS.
//! `cargo run --release --example calibrate_gains [-- --params file.json]`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let p = match args.iter().position(|a| a == "--params").and_then(|i| args.get(i + 1)) {
        Some(path) => DrumParams::from_json(&std::fs::read_to_string(path).unwrap()).unwrap(),
        None => DrumParams::default(),
    };
    println!("gains in the parameters: close {:+.2} dB, far {:+.2} dB", p.gain_close_db, p.gain_far_db);
    for fs in [44100.0, 48000.0, 96000.0] {
        let m = DrumModel::prepare(&p, fs).unwrap();
        let mut v = DrumVoice::new(&m);
        v.trigger(&m, Hit { velocity: 1.0, radius: 0.0, angle: 0.0, sample_offset: 0 });
        let (mut pc, mut pf) = (0f32, 0f32);
        let (mut c, mut f) = (vec![0f32; 512], vec![0f32; 512]);
        for _ in 0..(1.5 * fs) as usize / 512 {
            v.process(&m, &mut c, &mut f);
            pc = c.iter().fold(pc, |a, x| a.max(x.abs()));
            pf = f.iter().fold(pf, |a, x| a.max(x.abs()));
        }
        let db = |x: f32| 20.0 * (x as f64).log10();
        println!("fs {fs:>6}: peak close {:+.2} dBFS, far {:+.2} dBFS  → to reach −6 dBFS: close {:+.2} dB, far {:+.2} dB",
            db(pc), db(pf), p.gain_close_db - 6.0 - db(pc), p.gain_far_db - 6.0 - db(pf));
    }
}
