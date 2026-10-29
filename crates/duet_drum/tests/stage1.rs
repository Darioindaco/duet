//! Stage 1: the Bessel functions and the in-vacuo mode basis (spec §4.1, §6).

use std::f64::consts::PI;

use duet_drum::bessel::{bessel_j_dj, bessel_zero, ZeroTable};
use duet_drum::modes::{MembraneBasis, MembraneMode, MembraneParams, RADIAL_POINTS};
use duet_drum::quad::gauss_legendre_on;
use duet_drum::{DrumParams, Head};

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("data/bessel_fixture.json")).unwrap()
}

#[test]
fn zeros_match_the_reference_values_to_1e_12() {
    let refs = [
        (0, 1, 2.404825557695773),
        (0, 2, 5.520078110286311),
        (0, 3, 8.653727912911013),
        (1, 1, 3.831705970207512),
        (1, 2, 7.015586669815619),
        (2, 1, 5.135622301840683),
        (3, 1, 6.380161895923983),
        (4, 1, 7.588342434503805),
    ];
    for (m, n, want) in refs {
        let got = bessel_zero(m, n);
        assert!((got - want).abs() < 1e-12, "j_{m},{n}: {got} vs {want}");
    }
}

#[test]
fn zeros_match_the_scipy_fixture_to_1e_10_and_interlace() {
    let fx = fixture();
    let n_max = fx["n_max"].as_u64().unwrap() as usize;
    let table = ZeroTable::new(100, n_max);
    let mut worst = 0f64;
    for m in 0..=100usize {
        let want = fx["zeros"][m.to_string()].as_array().unwrap();
        for n in 1..=n_max {
            let w = want[n - 1].as_f64().unwrap();
            let e = (table.zero(m, n) - w).abs();
            worst = worst.max(e);
            assert!(e < 1e-10, "j_{m},{n}: {} vs {w}", table.zero(m, n));
        }
    }
    println!("worst zero error vs scipy: {worst:.2e}");
    // j_{m,n} < j_{m+1,n} < j_{m,n+1}: no zero missed
    for m in 0..100 {
        for n in 1..=n_max {
            assert!(table.zero(m, n) < table.zero(m + 1, n) && table.zero(m + 1, n) < table.zero(m, n + 1), "m={m} n={n}");
        }
    }
}

#[test]
fn values_and_derivatives_match_scipy() {
    let fx = fixture();
    let mut worst = 0f64;
    for row in fx["values"].as_array().unwrap() {
        let (m, x, j, dj) = (row[0].as_u64().unwrap() as usize, row[1].as_f64().unwrap(), row[2].as_f64().unwrap(), row[3].as_f64().unwrap());
        let (g, dg) = bessel_j_dj(m, x);
        worst = worst.max((g - j).abs()).max((dg - dj).abs());
        assert!((g - j).abs() < 5e-15 && (dg - dj).abs() < 5e-15, "m={m} x={x}: {g} vs {j}, {dg} vs {dj}");
    }
    println!("worst J, J' error vs scipy: {worst:.2e}");
}

/// The default batter head's basis (to 1.6·f_max, as the model will use it).
fn batter() -> MembraneBasis {
    let p = DrumParams::default();
    MembraneBasis::build(&MembraneParams::from_drum(&p, Head::Batter), 1.6 * p.batter.f_max)
}

/// ∫∫ f(r, θ) r dr dθ over the disc, Gauss–Legendre in r and equispaced in θ
/// (exact for the trigonometric polynomials here).
fn disc(radius: f64, m: usize, f: impl Fn(f64, f64) -> f64) -> f64 {
    let (rs, rw) = gauss_legendre_on(240, 0.0, radius);
    let nt = 4 * m + 16;
    let mut sum = 0.0;
    for (r, w) in rs.iter().zip(&rw) {
        for k in 0..nt {
            let th = 2.0 * PI * k as f64 / nt as f64;
            sum += w * r * f(*r, th) * 2.0 * PI / nt as f64;
        }
    }
    sum
}

#[test]
fn the_shapes_are_mass_normalised() {
    let b = batter();
    let sigma = b.params.sigma;
    let mut worst = 0f64;
    // every order, the lowest radial modes and a few high ones
    for md in b.modes.iter().filter(|m| m.n <= 2 || m.j > 60.0) {
        for sin in [false, true] {
            if sin && md.m == 0 {
                continue;
            }
            let v = sigma
                * disc(md.radius, md.m, |r, th| {
                    let a = md.m as f64 * th;
                    let phi = md.radial_exact(r) * if sin { a.sin() } else { a.cos() };
                    phi * phi
                });
            worst = worst.max((v - 1.0).abs());
            assert!((v - 1.0).abs() < 1e-6, "(m={}, n={}, sin={sin}): σ∫Φ² = {v}", md.m, md.n);
        }
    }
    println!("worst |σ∫Φ² − 1|: {worst:.2e}");
}

#[test]
fn the_closed_form_volume_weight_matches_quadrature() {
    let b = batter();
    let mut worst = 0f64;
    for md in b.modes.iter().filter(|m| m.m == 0 || (m.m <= 6 && m.n <= 2)) {
        let q = disc(md.radius, md.m, |r, th| md.radial_exact(r) * (md.m as f64 * th).cos());
        let want = md.vol_weight;
        worst = worst.max((q - want).abs() / want.abs().max(1e-3));
        if md.m == 0 {
            assert!((q - want).abs() < 1e-6 * want.abs().max(1e-3), "a_0{}: closed form {want}, quadrature {q}", md.n);
        } else {
            assert!(q.abs() < 1e-9, "m ≥ 1 has no net volume: {q}");
            assert_eq!(want, 0.0);
        }
    }
    println!("worst relative a_0n error: {worst:.2e}");
}

/// The radial integrals of §4.5: I1 = ∫ρ'² r dr, I2 = ∫ m²ρ²/r dr.
fn radial_integrals(md: &MembraneMode, points: usize) -> (f64, f64) {
    let (rs, rw) = gauss_legendre_on(points, 0.0, md.radius);
    let (mut i1, mut i2) = (0.0, 0.0);
    for (r, w) in rs.iter().zip(&rw) {
        let rho = md.radial_exact(*r);
        let drho = md.norm * md.kappa * bessel_j_dj(md.m, md.kappa * r).1;
        i1 += w * drho * drho * r;
        i2 += w * (md.m * md.m) as f64 * rho * rho / r;
    }
    (i1, i2)
}

#[test]
fn pi_times_i1_plus_i2_is_kappa_squared_over_sigma() {
    let b = batter();
    let sigma = b.params.sigma;
    let mut worst = 0f64;
    for md in b.modes.iter().filter(|m| m.m >= 1 && m.n <= 3) {
        let (i1, i2) = radial_integrals(md, 128);
        let want = md.kappa * md.kappa / sigma;
        worst = worst.max((PI * (i1 + i2) - want).abs() / want);
        assert!((PI * (i1 + i2) - want).abs() < 1e-6 * want, "(m={}, n={}): {} vs {want}", md.m, md.n, PI * (i1 + i2));
    }
    // (for m = 0 the angle integral is 2π, not π: 2π·I1 = κ²/σ)
    for md in b.modes.iter().filter(|m| m.m == 0) {
        let (i1, _) = radial_integrals(md, 128);
        let want = md.kappa * md.kappa / sigma;
        assert!((2.0 * PI * i1 - want).abs() < 1e-6 * want, "m=0 n={}", md.n);
    }
    println!("worst relative π(I1+I2) error: {worst:.2e}");
}

#[test]
fn the_radial_tables_interpolate_well() {
    let b = batter();
    let (mut low, mut all) = (0f64, 0f64);
    for md in &b.modes {
        let peak = md.rho.iter().fold(0f64, |a, v| a.max(v.abs()));
        let mut e = 0f64;
        for k in 0..1000 {
            // off the grid points, where the Hermite error is largest
            let r = md.radius * (k as f64 + 0.37) / 1000.0;
            e = e.max((md.radial(r) - md.radial_exact(r)).abs() / peak);
        }
        if md.j < 10.0 {
            low = low.max(e);
        }
        all = all.max(e);
    }
    println!("{RADIAL_POINTS}-point table, cubic Hermite: worst error {low:.1e} (j < 10), {all:.1e} (all {} modes)", b.modes.len());
    assert!(low < 1e-7 && all < 1e-4);
}

#[test]
fn the_basis_is_complete_and_ordered() {
    let b = batter();
    // every zero below the cutoff is there, in order
    assert!(b.modes.windows(2).all(|w| w[0].omega_vac <= w[1].omega_vac));
    assert_eq!((b.modes[0].m, b.modes[0].n, b.modes[1].m, b.modes[1].n), (0, 1, 1, 1));
    let f_cut = 1.6 * DrumParams::default().batter.f_max;
    assert!(b.modes.iter().all(|m| m.f_vac() < f_cut));
    // a count by brute force from the fixture's zeros
    let fx = fixture();
    let kmax = b.params.kappa_at(f_cut);
    let j_max = kmax * b.params.radius;
    let mut count = 0;
    for m in 0..=100usize {
        for z in fx["zeros"][m.to_string()].as_array().unwrap() {
            if z.as_f64().unwrap() < j_max {
                count += 1;
            }
        }
    }
    assert_eq!(b.modes.len(), count, "no mode lost below j = {j_max:.2}");
    // the frequency law
    let md = &b.modes[1];
    let w2 = b.params.tension / b.params.sigma * md.kappa.powi(2) + b.params.bending / b.params.sigma * md.kappa.powi(4);
    assert!((md.omega_vac * md.omega_vac - w2).abs() < 1e-9 * w2);
}
