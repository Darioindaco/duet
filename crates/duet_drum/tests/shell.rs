//! The shell extension: modes driven by the contact force, heard by both outputs.

use duet_drum::analysis::{magnitude_spectrum, peak_near};
use duet_drum::{DrumModel, DrumParams, DrumVoice, Hit, Quality, ShellMode};

fn render(p: &DrumParams) -> (Vec<f64>, Vec<f64>) {
    let m = DrumModel::prepare(p, 48000.0).unwrap();
    let mut v = DrumVoice::new(&m);
    v.trigger(&m, Hit { velocity: 0.8, radius: 0.4, angle: 0.0, sample_offset: 0 });
    let (mut c, mut f) = (vec![0f32; 512], vec![0f32; 512]);
    let (mut oc, mut of) = (Vec::new(), Vec::new());
    for _ in 0..(1.5 * 48000.0 / 512.0) as usize {
        v.process(&m, &mut c, &mut f);
        oc.extend(c.iter().map(|x| *x as f64));
        of.extend(f.iter().map(|x| *x as f64));
    }
    (oc, of)
}

#[test]
fn a_shell_mode_rings_at_its_frequency_and_decays_at_its_rate() {
    let base = DrumParams::with_quality(Quality::Draft);
    let mut p = base.clone();
    p.shell = vec![ShellMode { freq: 1234.0, t60: 0.5, gain: 1.0 }];
    let (a, af) = render(&base);
    let (b, bf) = render(&p);
    // what the shell adds, on each output
    for (x, y) in [(&a, &b), (&af, &bf)] {
        let d: Vec<f64> = x.iter().zip(y.iter()).map(|(x, y)| y - x).collect();
        assert!(d.iter().any(|v| v.abs() > 1e-6), "the shell is heard");
        let n = 1 << 17;
        let f = peak_near(&magnitude_spectrum(&d[2400..], n), 48000.0, n, 1234.0, 20.0);
        assert!((f - 1234.0).abs() < 1.0, "{f}");
        // −60 dB in 0.5 s: from 0.1 s to 0.35 s the level falls by 30 dB
        let rms = |s: &[f64]| (s.iter().map(|v| v * v).sum::<f64>() / s.len() as f64).sqrt();
        let drop = 20.0 * (rms(&d[4800..7200]) / rms(&d[16800..19200])).log10();
        assert!((drop - 30.0).abs() < 2.0, "{drop} dB");
    }
    // no shell: the spec's drum, bit for bit
    assert_eq!(render(&base).0, a);
}
