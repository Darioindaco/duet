//! Prints the in-vacuo modes of one head: `cargo run --release --example mode_table [-- --head resonant] [--params file.json] [--fmax 4000]`

use duet_drum::{DrumParams, Head, MembraneBasis, MembraneParams};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let params = match get("--params") {
        Some(path) => DrumParams::from_json(&std::fs::read_to_string(&path).expect("read params")).expect("params"),
        None => DrumParams::default(),
    };
    let head = if get("--head").as_deref() == Some("resonant") { Head::Resonant } else { Head::Batter };
    // 1.6·f_max: the headroom that air loading then lowers into the band (§4.2)
    let f_cut = get("--fmax").map(|s| s.parse().expect("--fmax")).unwrap_or(1.6 * params.head(head).f_max);
    let mp = MembraneParams::from_drum(&params, head);
    let basis = MembraneBasis::build(&mp, f_cut);
    println!(
        "{head:?} head, in vacuo: R = {} m, σ = {:.4} kg/m², T = {} N/m, D = {:.3e} N·m, wave speed sqrt(T/σ) = {:.1} m/s",
        mp.radius, mp.sigma, mp.tension, mp.bending, (mp.tension / mp.sigma).sqrt()
    );
    println!("modes below {f_cut:.0} Hz: {} (m, n) = {} degrees of freedom\n", basis.modes.len(), basis.dof());
    println!("{:>4} {:>3} {:>3} {:>11} {:>9} {:>10} {:>11} {:>12}", "#", "m", "n", "j_mn", "κ (1/m)", "f_vac (Hz)", "N_mn", "a = ∫Φ dA");
    for (i, m) in basis.modes.iter().enumerate() {
        let a = if m.m == 0 { format!("{:+.5}", m.vol_weight) } else { "0 (pair)".to_string() };
        println!("{:>4} {:>3} {:>3} {:>11.6} {:>9.3} {:>10.2} {:>11.4} {:>12}", i + 1, m.m, m.n, m.j, m.kappa, m.f_vac(), m.norm, a);
    }
}
