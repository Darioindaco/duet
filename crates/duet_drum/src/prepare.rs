//! Prepare time: everything that is solved once, off the audio thread (§4.6).
//!
//! Stage 2: one head, in vacuo, no air, cavity, lugs or nonlinearity: every
//! (m, n) of the head's basis is one or two uncoupled modes with
//! `Ω = ω_vac`, `σ = σ0 + σ1·Ω²`.

use std::f64::consts::PI;

use crate::modes::{MembraneBasis, MembraneParams};
use crate::params::{DrumParams, Head};

#[derive(Clone, Debug, PartialEq)]
pub enum PrepareError {
    /// a head would need more modes than `n_max_per_head`
    TooManyModes { head: Head, count: usize, max: usize },
    InvalidParams(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareError::TooManyModes { head, count, max } => {
                write!(f, "the {head:?} head needs {count} modes at this f_max; the cap is {max} (lower f_max or raise n_max_per_head)")
            }
            PrepareError::InvalidParams(s) => write!(f, "invalid parameters: {s}"),
        }
    }
}

impl std::error::Error for PrepareError {}

/// One final mode of the prepared model.
#[derive(Clone, Debug)]
pub struct ModeInfo {
    pub head: Head,
    /// index into the head's basis, and which member of a pair
    pub basis_index: usize,
    pub sin: bool,
    pub m: usize,
    pub n: usize,
    /// Ω (rad/s): the modal oscillator is `e^{−σt}·cos(Ωt)`
    pub omega: f64,
    /// σ (1/s): amplitude decay rate
    pub sigma: f64,
}

impl ModeInfo {
    pub fn freq(&self) -> f64 {
        self.omega / (2.0 * PI)
    }

    /// Time for the amplitude to fall by 60 dB (s).
    pub fn t60(&self) -> f64 {
        if self.sigma > 0.0 {
            3.0 * std::f64::consts::LN_10 / self.sigma
        } else {
            f64::INFINITY
        }
    }
}

/// The exact per-mode coefficients of the time stepping (§4.9): returns
/// `(σ̃, Ω̃²)` with `kσ̃ = tanh(σk)` and
/// `Ω̃² = (2/k²)(1 − (1+kσ̃)·e^{−σk}·cos(Ωk))`, which make the scheme's
/// linear part ring at exactly Ω and decay at exactly σ. For σ = 0 this is
/// `Ω̃ = (2/k) sin(Ωk/2)`.
pub fn exact_coefficients(omega: f64, sigma: f64, k: f64) -> (f64, f64) {
    let t = (sigma * k).tanh();
    let o2 = 2.0 / (k * k) * (1.0 - (1.0 + t) * (-sigma * k).exp() * (omega * k).cos());
    (t / k, o2)
}

/// Everything prepare computes: immutable, shared by voices.
#[derive(Clone, Debug)]
pub struct DrumModel {
    pub fs: f64,
    pub k: f64,
    pub params: DrumParams,
    pub modes: Vec<ModeInfo>,
    /// the head's in-vacuo basis (its radial tables serve strikes and mics)
    pub basis: MembraneBasis,
    // per mode, contiguous (struct of arrays), padded to a multiple of 4
    pub(crate) omega_t2: Vec<f64>,
    /// 1 + kσ̃ and 1 − kσ̃
    pub(crate) d_plus: Vec<f64>,
    pub(crate) d_minus: Vec<f64>,
}

impl DrumModel {
    /// Stage 2: one head, linear, in vacuo.
    pub fn prepare_single_head(p: &DrumParams, head: Head, fs: f64) -> Result<Self, PrepareError> {
        if !(fs > 8000.0) {
            return Err(PrepareError::InvalidParams(format!("sample rate {fs}")));
        }
        let hp = p.head(head);
        let f_cut = hp.f_max.min(0.45 * fs);
        let basis = MembraneBasis::build(&MembraneParams::from_drum(p, head), f_cut);
        let mut modes = Vec::new();
        for (bi, bm) in basis.modes.iter().enumerate() {
            let omega = bm.omega_vac;
            let sigma = hp.sigma0 + hp.sigma1 * omega * omega;
            for sin in [false, true] {
                if sin && bm.m == 0 {
                    continue;
                }
                modes.push(ModeInfo { head, basis_index: bi, sin, m: bm.m, n: bm.n, omega, sigma });
            }
        }
        if modes.len() > p.n_max_per_head {
            return Err(PrepareError::TooManyModes { head, count: modes.len(), max: p.n_max_per_head });
        }
        let k = 1.0 / fs;
        let n = modes.len().div_ceil(4) * 4;
        let mut omega_t2 = vec![0.0; n];
        let (mut d_plus, mut d_minus) = (vec![1.0; n], vec![1.0; n]);
        for (i, m) in modes.iter().enumerate() {
            if !(m.omega.is_finite() && m.omega > 0.0 && m.sigma >= 0.0) {
                return Err(PrepareError::InvalidParams(format!("mode {i}: Ω = {}, σ = {}", m.omega, m.sigma)));
            }
            let (st, o2) = exact_coefficients(m.omega, m.sigma, k);
            omega_t2[i] = o2;
            d_plus[i] = 1.0 + k * st;
            d_minus[i] = 1.0 - k * st;
        }
        Ok(DrumModel { fs, k, params: p.clone(), modes, basis, omega_t2, d_plus, d_minus })
    }

    /// Number of modes (degrees of freedom).
    pub fn len(&self) -> usize {
        self.modes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modes.is_empty()
    }

    /// Φ_i at (r, θ), r as a fraction of the radius: mass-normalised, from the radial tables.
    pub fn shape(&self, i: usize, r_frac: f64, theta: f64) -> f64 {
        let md = &self.modes[i];
        self.basis.modes[md.basis_index].shape(r_frac * self.basis.params.radius, theta, md.sin)
    }
}
