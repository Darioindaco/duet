//! Stage 5 report: the two-headed drum's mode table, the (0,1) breathing/sloshing pair, and what
//! air loading does to the first ten modes of each head.
//! `cargo run --release --example stage5_report`  (the whole table goes to renders/stage5/mode_table.txt)

use duet_drum::{Block, DrumModel, DrumParams, Head, PrepareOptions};
use std::fmt::Write;

fn label(m: &DrumModel, i: usize) -> String {
    let x = &m.modes[i];
    let h = if x.head == Head::Batter { "B" } else { "R" };
    let p = if x.block == Block::Pair { if x.sin { "s" } else { "c" } } else { " " };
    format!("{h}({},{}){p}", x.m, x.n)
}

fn main() {
    let p = DrumParams::default();
    let t = std::time::Instant::now();
    let m = DrumModel::prepare(&p, 48000.0).unwrap();
    println!("prepared in {:.0?}: {} modes: {} axisymmetric (block A), batter {} / resonant {} in all; warnings: {:?}\n", t.elapsed(), m.len(), m.n_axisym(),
        m.modes.iter().filter(|x| x.head == Head::Batter).count(), m.modes.iter().filter(|x| x.head == Head::Resonant).count(), m.warnings);

    let mut table = String::new();
    writeln!(table, "{:>4} {:>9} {:>10} {:>9} {:>8} {:>8} {:>9} {:>9}", "#", "mode", "f (Hz)", "T60 (s)", "batter %", "resonant", "ã", "σ_rad/σ").unwrap();
    for (n, i) in m.by_frequency().into_iter().enumerate() {
        let x = &m.modes[i];
        writeln!(table, "{:>4} {:>9} {:>10.2} {:>9.3} {:>8.0} {:>8.0} {:>9.4} {:>9.2}", n + 1, label(&m, i), x.freq(), x.t60(), x.participation[0] * 100.0, x.participation[1] * 100.0, x.vol_weight, x.sigma_rad / x.sigma).unwrap();
    }
    std::fs::write("renders/stage5/mode_table.txt", &table).unwrap();
    println!("the lowest 40 of {} modes (the whole table: renders/stage5/mode_table.txt); B = batter, R = resonant, c/s = member of a pair:", m.len());
    for line in table.lines().take(41) {
        println!("{line}");
    }

    println!("\nthe (0,1) pair: both heads' (0,1) modes, coupled by the cavity:");
    for i in 0..2 {
        let x = &m.modes[i];
        println!("  {:>7.1} Hz  T60 {:>6.3} s  batter {:>3.0} % / resonant {:>3.0} %  ã = {:+.3} (batter part {:+.3})  σ_int {:.2}/s  σ_rad {:.2}/s",
            x.freq(), x.t60(), x.participation[0] * 100.0, x.participation[1] * 100.0, x.vol_weight, x.vol_weight_batter, x.sigma_int, x.sigma_rad);
    }
    let mut nc = p.clone();
    nc.cavity_scale = 0.0;
    let u = DrumModel::prepare(&nc, 48000.0).unwrap();
    println!("  (without the cavity: {:.1} and {:.1} Hz: the heads' own air-loaded (0,1), nothing couples them)", u.modes[0].freq(), u.modes[1].freq());

    println!("\nair loading, the first ten modes of each head (cavity off, so each head is itself):");
    let off = DrumModel::prepare_with(&p, 48000.0, &PrepareOptions { resonant: true, air_loading: false, cavity: false }).unwrap();
    let on = DrumModel::prepare_with(&p, 48000.0, &PrepareOptions { resonant: true, air_loading: true, cavity: false }).unwrap();
    for head in [Head::Batter, Head::Resonant] {
        println!("  {head:?}:  mode   in vacuo   loaded   ratio");
        let firsts: Vec<(usize, usize)> = off.by_frequency().into_iter().filter(|&i| off.modes[i].head == head && !off.modes[i].sin).take(10).map(|i| (off.modes[i].m, off.modes[i].n)).collect();
        for (mm, nn) in firsts {
            let a = off.find_mode(head, mm, nn, false).unwrap();
            let b = on.find_mode(head, mm, nn, false).unwrap();
            println!("           ({mm},{nn})  {:>8.2}  {:>8.2}   {:.3}", off.modes[a].freq(), on.modes[b].freq(), on.modes[b].freq() / off.modes[a].freq());
        }
    }
}
