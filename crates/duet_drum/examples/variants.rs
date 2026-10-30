//! A/B renders of the same three hits (vel 0.4, 0.7, 1.0 at r = 0.5) with a few parameters changed:
//! the mix of both outputs, 48 kHz mono float: `renders/variants/*.wav`.
//! `cargo run --release --example variants`

use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit};

fn main() {
    let fs = 48000.0;
    let variants: Vec<(&str, DrumParams)> = {
        let base = DrumParams::default();
        let mut b = base.clone();
        b.attack_gain = 0.2;
        let mut c = base.clone();
        c.nonlinearity_scale = 0.3;
        let mut d = base.clone();
        for h in [&mut d.batter, &mut d.resonant] {
            h.sigma0 = 10.0;
            h.sigma1 = 2.0e-7;
        }
        let mut e = base.clone();
        e.attack_gain = 0.2;
        e.nonlinearity_scale = 0.3;
        e.batter.sigma0 = 10.0;
        e.resonant.sigma0 = 10.0;
        e.batter.sigma1 = 2.0e-7;
        e.resonant.sigma1 = 2.0e-7;
        e.stick.mass = 0.01;
        vec![("A_now", base), ("B_less_attack_click", b), ("C_less_pitch_glide", c), ("D_shorter_ring", d), ("E_all_of_it_and_lighter_stick", e)]
    };
    for (name, p) in variants {
        let m = DrumModel::prepare(&p, fs).unwrap();
        let mut v = DrumVoice::new(&m);
        let (mut c, mut f) = (vec![0f32; 512], vec![0f32; 512]);
        let mut out = Vec::new();
        for t in 0..(4.5 * fs) as usize / 512 {
            let gap = (1.5 * fs) as usize;
            if t * 512 % gap < 512 {
                let k = t * 512 / gap;
                v.trigger(&m, Hit { velocity: [0.4, 0.7, 1.0][k.min(2)], radius: 0.5, angle: 0.0, sample_offset: 0 });
            }
            v.process(&m, &mut c, &mut f);
            out.extend(c.iter().zip(&f).map(|(a, b)| 0.5 * (a + b)));
        }
        let spec = hound::WavSpec { channels: 1, sample_rate: fs as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(format!("renders/variants/{name}.wav"), spec).unwrap();
        for x in &out {
            w.write_sample(*x).unwrap();
        }
        w.finalize().unwrap();
        println!("{name}: peak {:.1} dBFS", 20.0 * (out.iter().fold(1e-9f32, |a, x| a.max(x.abs())) as f64).log10());
    }
}
