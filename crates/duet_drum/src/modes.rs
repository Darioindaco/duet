//! The in-vacuo mode basis of one membrane head (§4.1, §4.2).
//!
//! A clamped circular membrane (Dirichlet at r = R) has modes `Φ_mn = ρ_mn(r)·cos(mθ)`
//! (and `sin(mθ)` for m ≥ 1, a degenerate pair), with `ρ_mn = N_mn·J_m(κ_mn r)` and
//! `κ_mn = j_mn/R`. The shapes are *mass-normalised*: `σ ∫ Φ² dA = 1`.
//!
//! Every (m, n) keeps a radial table, `ρ` and `dρ/dr` on [`RADIAL_POINTS`] uniform
//! points, so that trigger-time code (strike and microphone projections) never
//! evaluates a Bessel function.

use std::f64::consts::PI;

use crate::bessel::{bessel_j_dj, ZeroTable};
use crate::quad::gauss_legendre_on;
use crate::params::{DrumParams, Head};

/// Points in a radial table, r = 0 ..= R.
pub const RADIAL_POINTS: usize = 256;

/// What the in-vacuo basis of one head depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct MembraneParams {
    /// R (m)
    pub radius: f64,
    /// σ_h = ρ_f·h (kg/m²)
    pub sigma: f64,
    /// T_eff: nominal tension plus the lug mean (N/m)
    pub tension: f64,
    /// D_h (N·m)
    pub bending: f64,
}

impl MembraneParams {
    /// The head's parameters with its nominal tension (no lug mean shift yet).
    pub fn from_drum(p: &DrumParams, head: Head) -> Self {
        MembraneParams { radius: p.shell_radius, sigma: p.surface_density(head), tension: p.head(head).tension, bending: p.bending_stiffness(head) }
    }

    /// ω_vac² for a wavenumber: `(T/σ)κ² + (D/σ)κ⁴`.
    pub fn omega2(&self, kappa: f64) -> f64 {
        let k2 = kappa * kappa;
        self.tension / self.sigma * k2 + self.bending / self.sigma * k2 * k2
    }

    /// The wavenumber with `ω_vac = 2π f`.
    pub fn kappa_at(&self, f_hz: f64) -> f64 {
        let (a, b) = (self.tension / self.sigma, self.bending / self.sigma);
        let w2 = (2.0 * PI * f_hz).powi(2);
        let k2 = if b > 0.0 { (-a + (a * a + 4.0 * b * w2).sqrt()) / (2.0 * b) } else { w2 / a };
        k2.sqrt()
    }
}

/// One (m, n): a single mode for m = 0, a degenerate (cos, sin) pair for m ≥ 1.
#[derive(Clone, Debug)]
pub struct MembraneMode {
    pub m: usize,
    pub n: usize,
    /// j_mn
    pub j: f64,
    /// κ_mn = j_mn/R (1/m)
    pub kappa: f64,
    /// ω_vac (rad/s)
    pub omega_vac: f64,
    /// N_mn
    pub norm: f64,
    /// a = ∫Φ dA (the net volume weight): the closed form for m = 0, zero for m ≥ 1
    pub vol_weight: f64,
    /// R (m), for the table below
    pub radius: f64,
    /// ρ_mn(r_i) = N_mn·J_m(κ r_i), r_i = i·R/(RADIAL_POINTS−1)
    pub rho: Vec<f64>,
    /// dρ_mn/dr at the same points
    pub drho: Vec<f64>,
}

impl MembraneMode {
    /// 1 for m = 0, 2 for m ≥ 1.
    pub fn degeneracy(&self) -> usize {
        if self.m == 0 {
            1
        } else {
            2
        }
    }

    pub fn f_vac(&self) -> f64 {
        self.omega_vac / (2.0 * PI)
    }

    /// ρ(r) from the table (cubic Hermite, using the stored slopes). r outside [0, R] is clamped.
    pub fn radial(&self, r: f64) -> f64 {
        self.hermite(r).0
    }

    /// dρ/dr of the interpolant.
    pub fn radial_slope(&self, r: f64) -> f64 {
        self.hermite(r).1
    }

    fn hermite(&self, r: f64) -> (f64, f64) {
        let h = self.radius / (RADIAL_POINTS - 1) as f64;
        let u = (r / h).clamp(0.0, (RADIAL_POINTS - 1) as f64 - 1e-9);
        let i = u as usize;
        let t = u - i as f64;
        let (y0, y1, d0, d1) = (self.rho[i], self.rho[i + 1], self.drho[i] * h, self.drho[i + 1] * h);
        let (t2, t3) = (t * t, t * t * t);
        let v = (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * d0 + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * d1;
        let s = ((6.0 * t2 - 6.0 * t) * y0 + (3.0 * t2 - 4.0 * t + 1.0) * d0 + (-6.0 * t2 + 6.0 * t) * y1 + (3.0 * t2 - 2.0 * t) * d1) / h;
        (v, s)
    }

    /// ρ(r) evaluated exactly (a Bessel evaluation: prepare-time and tests only).
    pub fn radial_exact(&self, r: f64) -> f64 {
        self.norm * bessel_j_dj(self.m, self.kappa * r).0
    }

    /// The radial integrals of §4.5, by Gauss–Legendre quadrature of the exact Bessel functions:
    /// `I1 = ∫ρ'² r dr`, `I2 = ∫m²ρ²/r dr`. (`π(I1 + I2) = κ²/σ` for m ≥ 1.) Prepare-time only.
    pub fn radial_integrals(&self) -> (f64, f64) {
        let n = 64.max(self.j as usize + 40);
        let (rs, ws) = gauss_legendre_on(n, 0.0, self.radius);
        let (mut i1, mut i2) = (0.0, 0.0);
        for (r, w) in rs.iter().zip(&ws) {
            let (jv, dj) = bessel_j_dj(self.m, self.kappa * r);
            let (rho, drho) = (self.norm * jv, self.norm * self.kappa * dj);
            i1 += w * drho * drho * r;
            i2 += w * (self.m * self.m) as f64 * rho * rho / r;
        }
        (i1, i2)
    }

    /// Φ(r, θ) for the cos (or sin) member of the pair; m = 0 has only the cos one.
    pub fn shape(&self, r: f64, theta: f64, sin: bool) -> f64 {
        let a = self.m as f64 * theta;
        self.radial(r) * if sin { a.sin() } else { a.cos() }
    }
}

/// All the modes of one head below a cutoff, lowest first.
#[derive(Clone, Debug)]
pub struct MembraneBasis {
    pub params: MembraneParams,
    pub modes: Vec<MembraneMode>,
}

impl MembraneBasis {
    /// Every (m, n) with `f_vac < f_cut`.
    pub fn build(params: &MembraneParams, f_cut: f64) -> Self {
        let r = params.radius;
        let j_max = params.kappa_at(f_cut) * r;
        let mut modes = Vec::new();
        if j_max > 2.0 {
            // j_{m,1} > m, so no order above ceil(j_max) has a zero below j_max
            let (m_cap, n_cap) = (j_max.ceil() as usize + 1, (j_max / PI).ceil() as usize + 2);
            let table = ZeroTable::new(m_cap, n_cap);
            for m in 0..=m_cap {
                for (i, &j) in table.row(m).iter().enumerate() {
                    if j >= j_max {
                        break;
                    }
                    modes.push(make_mode(params, m, i + 1, j));
                }
            }
            modes.sort_by(|a, b| a.omega_vac.total_cmp(&b.omega_vac).then(a.m.cmp(&b.m)));
        }
        MembraneBasis { params: params.clone(), modes }
    }

    /// Degrees of freedom: a pair counts twice.
    pub fn dof(&self) -> usize {
        self.modes.iter().map(|m| m.degeneracy()).sum()
    }
}

fn make_mode(p: &MembraneParams, m: usize, n: usize, j: f64) -> MembraneMode {
    let r = p.radius;
    let kappa = j / r;
    // J_{m+1}(j_mn) and the table of ρ, dρ/dr
    let jm1 = bessel_j_dj(m + 1, j).0.abs();
    let root = (PI * p.sigma).sqrt();
    let norm = if m == 0 { 1.0 / (r * jm1 * root) } else { 2f64.sqrt() / (r * jm1 * root) };
    let vol_weight = if m == 0 { (if n % 2 == 1 { 1.0 } else { -1.0 }) * 2.0 * r * (PI / p.sigma).sqrt() / j } else { 0.0 };
    let (mut rho, mut drho) = (Vec::with_capacity(RADIAL_POINTS), Vec::with_capacity(RADIAL_POINTS));
    for i in 0..RADIAL_POINTS {
        let x = kappa * r * i as f64 / (RADIAL_POINTS - 1) as f64;
        let (v, d) = bessel_j_dj(m, x);
        rho.push(norm * v);
        drho.push(norm * kappa * d);
    }
    MembraneMode { m, n, j, kappa, omega_vac: p.omega2(kappa).sqrt(), norm, vol_weight, radius: r, rho, drho }
}
