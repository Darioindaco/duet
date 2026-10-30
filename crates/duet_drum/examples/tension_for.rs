//! Prints the tension that puts a head's (1,1) pair at a pitch: `cargo run --example tension_for -- batter 110`
use duet_drum::{DrumParams, Head};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let head = if a.first().map(|s| s.as_str()) == Some("resonant") { Head::Resonant } else { Head::Batter };
    let f: f64 = a.get(1).map(|s| s.parse().unwrap()).unwrap_or(110.0);
    println!("{:.1}", DrumParams::default().tension_for_pitch(head, f));
}
