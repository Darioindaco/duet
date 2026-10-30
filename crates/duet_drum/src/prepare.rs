//! Prepare time: everything that is solved once, off the audio thread (§4.6).
//!
//! Two heads (batter and resonant), air mass loading (§4.3), the cavity's air spring
//! (§4.4) and damping (§4.7), diagonalised into a bank of uncoupled oscillators:
//!
//! * **block A**, the axisymmetric (m = 0) modes of both heads, coupled by the cavity: a dense
//!   `T_A` (rows: every m = 0 head mode, columns: the kept final modes) with `q = T_A·η`;
//! * **pairs**, every (head, m ≥ 1, n): a degenerate (cos, sin) pair, `q = η/√(1+μ)` (the lugs, stage 6,
//!   will rotate the pair).
//!
//! The runtime state is the final modes' coordinates η, A's first, then the pairs.

use std::f64::consts::PI;

use nalgebra::{DMatrix, SymmetricEigen};

use crate::lugs::LugProfile;
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

/// Which physics is in the model (for tests and to build up the stages).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareOptions {
    /// the resonant head (without it, only the batter)
    pub resonant: bool,
    /// air mass loading (§4.3)
    pub air_loading: bool,
    /// the cavity's air spring coupling the m = 0 modes (§4.4)
    pub cavity: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        PrepareOptions { resonant: true, air_loading: true, cavity: true }
    }
}

/// How a final mode sits in the bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// an axisymmetric mode of the (possibly coupled) two heads
    Axisym,
    /// one member (cos or sin) of a degenerate pair
    Pair,
}

/// One final mode of the prepared model.
#[derive(Clone, Debug)]
pub struct ModeInfo {
    /// the head that takes most of the mode
    pub head: Head,
    pub block: Block,
    /// index into the dominant head's basis (a pair; for block A the dominant row)
    pub basis_index: usize,
    pub sin: bool,
    pub m: usize,
    /// n of the dominant (m, n) of the head
    pub n: usize,
    /// Ω (rad/s): the modal oscillator is `e^{−σt}·cos(Ωt)`
    pub omega: f64,
    /// σ = σ_int + σ_rad (1/s): amplitude decay rate
    pub sigma: f64,
    pub sigma_int: f64,
    pub sigma_rad: f64,
    /// how much of the mode's energy each head holds (batter, resonant); sums to 1
    pub participation: [f64; 2],
    /// net volume weight ã = (Tᵀa)_i and its batter-only part (m³ per √kg·m)
    pub vol_weight: f64,
    pub vol_weight_batter: f64,
    /// the pair's `1/√(1+μ)`
    pub scale: f64,
    /// the lugs rotate a pair: its members are the (cos, sin) pair turned by φ in the mθ angle (0 without lugs)
    pub phi: f64,
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

/// One m = 0 head mode (a row of block A).
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub head: usize,
    pub basis_index: usize,
    pub kappa2: f64,
}

/// Everything prepare computes: immutable, shared by voices.
#[derive(Clone, Debug)]
pub struct DrumModel {
    pub fs: f64,
    pub k: f64,
    pub params: DrumParams,
    pub options: PrepareOptions,
    pub modes: Vec<ModeInfo>,
    /// the heads' in-vacuo bases (batter, resonant): their radial tables serve strikes and mics
    pub bases: [Option<MembraneBasis>; 2],
    /// things prepare noticed (a damping clamped, ...)
    pub warnings: Vec<String>,
    /// final modes in block A (they come first), and the rows of `t_a`
    pub(crate) n_a: usize,
    pub(crate) rows: Vec<Row>,
    /// row-major `rows × n_a`: q_row = Σ_i t_a[row, i]·η_i
    pub(crate) t_a: Vec<f64>,
    // per mode, contiguous (struct of arrays), padded to a multiple of 4
    pub(crate) omega_t2: Vec<f64>,
    /// κ²/(1+μ) of a pair's modes, in the slot of a batter pair (`w_b`) or a resonant one (`w_r`); 0 elsewhere
    pub(crate) w_b: Vec<f64>,
    pub(crate) w_r: Vec<f64>,
    /// the wavenumber of each mode's dominant (m, n) (the contact patch's weight), per mode (not per slot)
    pub(crate) mode_kappa: Vec<f64>,
    /// the state's layout: block A's modes in slots 0..n_a (padded to `pad_a` slots, a multiple of 4), then the
    /// pairs, the whole padded to a multiple of 4 (`n_slots`): the sweeps run in lanes of four
    pub(crate) pad_a: usize,
    pub(crate) n_slots: usize,
    /// 1 + kσ̃ and 1 − kσ̃
    pub(crate) d_plus: Vec<f64>,
    pub(crate) d_minus: Vec<f64>,
    /// 1/(1 + kσ̃)
    pub(crate) inv_d_plus: Vec<f64>,
    /// output weights per mode (§4.10): the far monopole `ρ_a/(4π r_far)·ã_i` and the close mic's `c_i`,
    /// both multiplying the mode's acceleration to give pascals
    pub(crate) far_w: Vec<f64>,
    pub(crate) close_w: Vec<f64>,
    /// the close mic's position (m: x, y in the head's plane, height) and the dipole term's factor
    /// `g_att/(4π c_a)`
    pub(crate) mic_xy: (f64, f64),
    pub(crate) mic_h: f64,
    pub(crate) attack_k: f64,
    /// linear output gains and the 20 Hz high-pass's coefficient
    pub(crate) gain_close: f64,
    pub(crate) gain_far: f64,
    pub(crate) hp_a: f64,
    /// the attack term's low-pass: two Butterworth biquads (4 poles; b0, b1, b2, a1, a2 each), or None
    pub(crate) attack_lp: Option<[[f64; 5]; 2]>,
    /// C of V_tm = Σ_h C_h·G_h², G_h = Σκ²q² (§4.8a): γ_nl·E·h / (8A(1−ν²)σ²), per head
    pub(crate) c_tm: [f64; 2],
}

/// The outputs' DC and infrasound filter (§4.10).
pub const HIGH_PASS_HZ: f64 = 20.0;

/// A 4-pole Butterworth low-pass as two biquads (bilinear transform; Q = 0.5412 and 1.3066); None when off.
fn butterworth_lowpass4(fc: f64, fs: f64) -> Option<[[f64; 5]; 2]> {
    if !(fc > 0.0 && fc < 0.45 * fs) {
        return None;
    }
    let w = (PI * fc / fs).tan();
    let section = |q: f64| {
        let n = 1.0 / (1.0 + w / q + w * w);
        let b0 = w * w * n;
        [b0, 2.0 * b0, b0, 2.0 * (w * w - 1.0) * n, (1.0 - w / q + w * w) * n]
    };
    Some([section(0.541196100146197), section(1.3065629648763766)])
}

fn omega_floor(lam: f64) -> bool {
    !(lam > 0.0 && lam.is_finite())
}

fn head_index(h: Head) -> usize {
    match h {
        Head::Batter => 0,
        Head::Resonant => 1,
    }
}

impl DrumModel {
    /// The full model: both heads, air loading, cavity, damping.
    pub fn prepare(p: &DrumParams, fs: f64) -> Result<Self, PrepareError> {
        Self::prepare_with(p, fs, &PrepareOptions::default())
    }

    /// Stage 2–4's model: the batter alone, in vacuo (no air, cavity or radiation).
    pub fn prepare_single_head(p: &DrumParams, head: Head, fs: f64) -> Result<Self, PrepareError> {
        assert_eq!(head, Head::Batter, "the single-head model is the batter");
        let mut q = p.clone();
        q.radiation_on = false;
        Self::prepare_with(&q, fs, &PrepareOptions { resonant: false, air_loading: false, cavity: false })
    }

    pub fn prepare_with(p: &DrumParams, fs: f64, opt: &PrepareOptions) -> Result<Self, PrepareError> {
        if !(fs > 8000.0) {
            return Err(PrepareError::InvalidParams(format!("sample rate {fs}")));
        }
        let k = 1.0 / fs;
        let heads: Vec<Head> = if opt.resonant { vec![Head::Batter, Head::Resonant] } else { vec![Head::Batter] };
        let mut warnings = Vec::new();
        // the in-vacuo bases, with 1.6·f_max of headroom: air loading lowers frequencies
        let mut bases: [Option<MembraneBasis>; 2] = [None, None];
        let mut lugs: [Option<LugProfile>; 2] = [None, None];
        for &h in &heads {
            let hp = p.head(h);
            if !hp.lug_deviations.is_empty() && hp.lug_deviations.len() != p.lug_count {
                return Err(PrepareError::InvalidParams(format!("the {h:?} head has {} lug deviations for {} lugs", hp.lug_deviations.len(), p.lug_count)));
            }
            lugs[head_index(h)] = LugProfile::new(hp.tension, &hp.lug_deviations, p.lug_count, p.lug_angle_offset, p.lug_sharpness);
            let mut mp = MembraneParams::from_drum(p, h);
            // the lugs' mean tension shift (c_0) is a shift of every mode of the head
            mp.tension += lugs[head_index(h)].as_ref().map_or(0.0, |l| l.c0);
            bases[head_index(h)] = Some(MembraneBasis::build(&mp, 1.6 * hp.f_max));
        }
        // air mass loading per head mode: μ = β·ρ_a/(σ_h κ)
        let mu = |hi: usize, kappa: f64| -> f64 {
            if opt.air_loading {
                let h = if hi == 0 { Head::Batter } else { Head::Resonant };
                p.air_mass_factor * p.air_density / (p.surface_density(h) * kappa)
            } else {
                0.0
            }
        };
        let f_keep = |hi: usize| -> f64 {
            let h = if hi == 0 { Head::Batter } else { Head::Resonant };
            p.head(h).f_max.min(0.45 * fs)
        };
        let sigma_of = |part: [f64; 2], omega: f64| -> f64 {
            part[0] * (p.batter.sigma0 + p.batter.sigma1 * omega * omega) + part[1] * (p.resonant.sigma0 + p.resonant.sigma1 * omega * omega)
        };

        // ---- block A: the m = 0 modes of both heads, with the cavity's spring ----
        let mut rows: Vec<Row> = Vec::new();
        let (mut row_mu, mut row_w2, mut row_a) = (Vec::new(), Vec::new(), Vec::new());
        for &h in &heads {
            let hi = head_index(h);
            for (bi, bm) in bases[hi].as_ref().unwrap().modes.iter().enumerate().filter(|(_, m)| m.m == 0) {
                rows.push(Row { head: hi, basis_index: bi, kappa2: bm.kappa * bm.kappa });
                row_mu.push(mu(hi, bm.kappa));
                row_w2.push(bm.omega_vac * bm.omega_vac);
                row_a.push(bm.vol_weight);
            }
        }
        let nr = rows.len();
        let k_cav = if opt.cavity { p.cavity_scale * p.air_density * p.speed_of_sound * p.speed_of_sound / p.cavity_volume() } else { 0.0 };
        let mut s = DMatrix::<f64>::zeros(nr, nr);
        for i in 0..nr {
            for j in 0..nr {
                let kij = k_cav * row_a[i] * row_a[j] + if i == j { row_w2[i] } else { 0.0 };
                s[(i, j)] = kij / ((1.0 + row_mu[i]) * (1.0 + row_mu[j])).sqrt();
            }
        }
        let eig = SymmetricEigen::new(s);
        let mut order: Vec<usize> = (0..nr).collect();
        order.sort_by(|&a, &b| eig.eigenvalues[a].total_cmp(&eig.eigenvalues[b]));
        let mut modes: Vec<ModeInfo> = Vec::new();
        let mut t_cols: Vec<Vec<f64>> = Vec::new(); // kept columns of T_A, each of length nr
        for &c in &order {
            let lam = eig.eigenvalues[c];
            if !(lam.is_finite() && lam > 0.0) {
                return Err(PrepareError::InvalidParams(format!("block A: Ω² = {lam}")));
            }
            let omega = lam.sqrt();
            let mut v: Vec<f64> = (0..nr).map(|r| eig.eigenvectors[(r, c)]).collect();
            // a sign convention, so the result doesn't depend on the solver's whim
            let big = (0..nr).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap();
            if v[big] < 0.0 {
                v.iter_mut().for_each(|x| *x = -*x);
            }
            // participation: Σ_{k∈h}(1+μ_k)·T_ki² = Σ_{k∈h} V_ki²
            let mut part = [0.0; 2];
            for r in 0..nr {
                part[rows[r].head] += v[r] * v[r];
            }
            let dom = if part[1] > part[0] { 1 } else { 0 };
            if omega / (2.0 * PI) > f_keep(dom) {
                continue;
            }
            let t: Vec<f64> = (0..nr).map(|r| v[r] / (1.0 + row_mu[r]).sqrt()).collect();
            let vol: f64 = (0..nr).map(|r| row_a[r] * t[r]).sum();
            let vol_b: f64 = (0..nr).filter(|&r| rows[r].head == 0).map(|r| row_a[r] * t[r]).sum();
            let sigma_int = sigma_of(part, omega);
            let sigma_rad = if p.radiation_on { p.radiation_gain * p.air_density * lam * vol * vol / (8.0 * PI * p.speed_of_sound) } else { 0.0 };
            let dom_row = (0..nr).filter(|&r| rows[r].head == dom).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap();
            let bm = &bases[dom].as_ref().unwrap().modes[rows[dom_row].basis_index];
            modes.push(ModeInfo {
                head: if dom == 0 { Head::Batter } else { Head::Resonant },
                block: Block::Axisym,
                basis_index: rows[dom_row].basis_index,
                sin: false,
                m: 0,
                n: bm.n,
                omega,
                sigma: sigma_int + sigma_rad,
                sigma_int,
                sigma_rad,
                participation: part,
                vol_weight: vol,
                vol_weight_batter: vol_b,
                scale: 1.0,
                phi: 0.0,
            });
            t_cols.push(t);
        }
        let n_a = modes.len();

        // ---- the pairs: (head, m ≥ 1, n), air-loaded, uncoupled ----
        let mut pair_w2 = Vec::new();
        let mut pair_head = Vec::new();
        for &h in &heads {
            let hi = head_index(h);
            let basis = bases[hi].as_ref().unwrap();
            for (bi, bm) in basis.modes.iter().enumerate().filter(|(_, m)| m.m >= 1) {
                let mu_k = mu(hi, bm.kappa);
                // the pair's stiffness: ω_vac²·I + ΔK_lug, in the (cos, sin) basis, over the air-loaded mass:
                // ΔK_cc = −ΔK_ss = (π/2)·c_2m·(I1 − I2), ΔK_cs = (π/2)·s_2m·(I1 − I2)
                let (mut d_c, mut d_s) = (0.0, 0.0);
                if let Some(l) = &lugs[hi] {
                    let (c, s) = l.harmonic(2 * bm.m);
                    if c != 0.0 || s != 0.0 {
                        let (i1, i2) = bm.radial_integrals();
                        let check = PI * (i1 + i2) * basis.params.sigma / (bm.kappa * bm.kappa);
                        if (check - 1.0).abs() > 1e-6 {
                            warnings.push(format!("lug integrals of ({},{}) of the {h:?} head: π(I1+I2)σ/κ² = {check}", bm.m, bm.n));
                        }
                        d_c = 0.5 * PI * c * (i1 - i2);
                        d_s = 0.5 * PI * s * (i1 - i2);
                    }
                }
                let mean = bm.omega_vac * bm.omega_vac / (1.0 + mu_k);
                let half = (d_c * d_c + d_s * d_s).sqrt() / (1.0 + mu_k);
                let phi = 0.5 * d_s.atan2(d_c);
                if omega_floor(mean - half) {
                    return Err(PrepareError::InvalidParams(format!("lugs make the ({},{}) pair of the {h:?} head unstable", bm.m, bm.n)));
                }
                for (sin, lam) in [(false, mean + half), (true, mean - half)] {
                    let omega = lam.sqrt();
                    if omega / (2.0 * PI) > f_keep(hi) {
                        continue;
                    }
                    let mut part = [0.0; 2];
                    part[hi] = 1.0;
                    let sigma = sigma_of(part, omega);
                    modes.push(ModeInfo {
                        head: h, block: Block::Pair, basis_index: bi, sin, m: bm.m, n: bm.n, omega, sigma, sigma_int: sigma, sigma_rad: 0.0,
                        participation: part, vol_weight: 0.0, vol_weight_batter: 0.0, scale: 1.0 / (1.0 + mu_k).sqrt(), phi,
                    });
                    pair_w2.push(bm.kappa * bm.kappa / (1.0 + mu_k));
                    pair_head.push(hi);
                }
            }
        }
        // the cap per head
        for h in &heads {
            let count = modes.iter().filter(|m| m.head == *h).count();
            if count > p.n_max_per_head {
                return Err(PrepareError::TooManyModes { head: *h, count, max: p.n_max_per_head });
            }
        }
        // damping must leave every mode underdamped
        for (i, m) in modes.iter_mut().enumerate() {
            if !(m.omega.is_finite() && m.omega > 0.0 && m.sigma >= 0.0 && m.sigma.is_finite()) {
                return Err(PrepareError::InvalidParams(format!("mode {i}: Ω = {}, σ = {}", m.omega, m.sigma)));
            }
            if m.sigma >= 0.9 * m.omega {
                warnings.push(format!("mode {i} ({:.1} Hz): σ = {:.1}/s is not below Ω; clamped to 0.9Ω", m.freq(), m.sigma));
                m.sigma = 0.9 * m.omega;
            }
        }

        // ---- the runtime arrays: block A, then the pairs, in lanes of four ----
        let n = modes.len();
        let pad_a = n_a.div_ceil(4) * 4;
        let n_slots = (pad_a + (n - n_a)).div_ceil(4) * 4;
        let slot = |i: usize| if i < n_a { i } else { i + pad_a - n_a };
        let mut omega_t2 = vec![0.0; n_slots];
        let (mut d_plus, mut d_minus) = (vec![1.0; n_slots], vec![1.0; n_slots]);
        for (i, m) in modes.iter().enumerate() {
            let (st, o2) = exact_coefficients(m.omega, m.sigma, k);
            omega_t2[slot(i)] = o2;
            d_plus[slot(i)] = 1.0 + k * st;
            d_minus[slot(i)] = 1.0 - k * st;
        }
        let inv_d_plus: Vec<f64> = d_plus.iter().map(|d| 1.0 / d).collect();
        let (mut w_b, mut w_r) = (vec![0.0; n_slots], vec![0.0; n_slots]);
        for (j, (w, h)) in pair_w2.iter().zip(&pair_head).enumerate() {
            if *h == 0 {
                w_b[slot(n_a + j)] = *w;
            } else {
                w_r[slot(n_a + j)] = *w;
            }
        }
        let mut t_a = vec![0.0; nr * n_a];
        for (c, col) in t_cols.iter().enumerate() {
            for r in 0..nr {
                t_a[r * n_a + c] = col[r];
            }
        }
        let c_of = |h: Head| {
            let sg = p.surface_density(h);
            p.nonlinearity_scale * p.youngs_modulus * p.head(h).thickness / (8.0 * p.area() * (1.0 - p.poisson_ratio * p.poisson_ratio) * sg * sg)
        };
        let c_tm = [c_of(Head::Batter), if opt.resonant { c_of(Head::Resonant) } else { 0.0 }];
        // ---- outputs (§4.10) ----
        let batter = bases[0].as_ref().unwrap();
        let r_m = p.mic.close_radius * batter.params.radius;
        let (th_m, h_m) = (p.mic.close_angle, p.mic.close_height);
        let far_scale = p.air_density / (4.0 * PI * p.mic.far_distance);
        let (mut far_w, mut close_w) = (vec![0.0; n_slots], vec![0.0; n_slots]);
        // a batter mode of the head, as the close mic hears it: (ρ_a/κ)·e^{−κh}·Φ(x_m)
        let surface = |bi: usize| {
            let bm = &batter.modes[bi];
            p.air_density / bm.kappa * (-bm.kappa * h_m).exp() * bm.radial(r_m)
        };
        for i in 0..n_a {
            far_w[i] = far_scale * modes[i].vol_weight;
            close_w[i] = (0..rows.len()).filter(|&r| rows[r].head == 0).map(|r| surface(rows[r].basis_index) * t_a[r * n_a + i]).sum();
        }
        for i in n_a..n {
            let md = &modes[i];
            if md.head == Head::Batter {
                let a = md.m as f64 * th_m - md.phi;
                close_w[slot(i)] = md.scale * surface(md.basis_index) * if md.sin { a.sin() } else { a.cos() };
            }
        }
        // (a block-A mode mixes heads: its κ is the participation-weighted one)
        let mut mode_kappa = vec![0.0; n];
        for i in 0..n_a {
            // κ_eff = Σ_rows V_ri²·κ_r, V = T·√(1+μ) (the rows' shares of the mode, summing to 1)
            mode_kappa[i] = (0..rows.len())
                .map(|r| {
                    let v = t_a[r * n_a + i] * (1.0 + mu(rows[r].head, rows[r].kappa2.sqrt())).sqrt();
                    v * v * rows[r].kappa2.sqrt()
                })
                .sum::<f64>();
        }
        for i in n_a..n {
            mode_kappa[i] = bases[head_index(modes[i].head)].as_ref().unwrap().modes[modes[i].basis_index].kappa;
        }
        let db = |x: f64| 10f64.powf(x / 20.0);
        Ok(DrumModel {
            fs, k, params: p.clone(), options: *opt, modes, bases, warnings, n_a, rows, t_a, omega_t2, w_b, w_r, mode_kappa, pad_a, n_slots, d_plus, d_minus, inv_d_plus, c_tm,
            far_w, close_w, mic_xy: (r_m * th_m.cos(), r_m * th_m.sin()), mic_h: h_m, attack_k: p.attack_gain / (4.0 * PI * p.speed_of_sound),
            gain_close: db(p.gain_close_db), gain_far: db(p.gain_far_db), hp_a: 1.0 / (1.0 + 2.0 * PI * HIGH_PASS_HZ * k),
            attack_lp: butterworth_lowpass4(p.attack_lowpass_hz, fs),
        })
    }

    /// Where mode `i`'s state lives (block A first, then the pairs, after the padding).
    pub(crate) fn slot(&self, i: usize) -> usize {
        if i < self.n_a {
            i
        } else {
            i + self.pad_a - self.n_a
        }
    }

    /// Number of modes (degrees of freedom).
    pub fn len(&self) -> usize {
        self.modes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modes.is_empty()
    }

    /// Final modes in block A (the axisymmetric ones; they come first).
    pub fn n_axisym(&self) -> usize {
        self.n_a
    }

    /// Mode indices, lowest frequency first.
    pub fn by_frequency(&self) -> Vec<usize> {
        let mut o: Vec<usize> = (0..self.len()).collect();
        o.sort_by(|&a, &b| self.modes[a].omega.total_cmp(&self.modes[b].omega).then(a.cmp(&b)));
        o
    }

    /// The first mode (lowest frequency) of a head's (m, n) and member; for block A, the
    /// mode that has most of that head's (0, n).
    pub fn find_mode(&self, head: Head, m: usize, n: usize, sin: bool) -> Option<usize> {
        self.by_frequency().into_iter().find(|&i| {
            let md = &self.modes[i];
            md.head == head && md.m == m && md.n == n && md.sin == sin
        })
    }

    /// `∂w_b(x)/∂η_i` for every mode: how the batter's displacement at (r, θ) depends on the
    /// state (r a fraction of the radius). This is the strike vector, and the weight of every mode in a
    /// displacement or acceleration at a point. Uses the radial tables only. `scratch`: `rows` long.
    pub fn strike_vector(&self, r_frac: f64, theta: f64, scratch: &mut [f64], out: &mut [f64]) {
        let basis = self.bases[0].as_ref().unwrap();
        let r = r_frac * basis.params.radius;
        for (k, row) in self.rows.iter().enumerate() {
            scratch[k] = if row.head == 0 { basis.modes[row.basis_index].radial(r) } else { 0.0 };
        }
        out[..self.n_a].fill(0.0);
        for (k, &phi) in scratch[..self.rows.len()].iter().enumerate() {
            if phi != 0.0 {
                let row = &self.t_a[k * self.n_a..(k + 1) * self.n_a];
                for (o, t) in out[..self.n_a].iter_mut().zip(row) {
                    *o += phi * t;
                }
            }
        }
        for i in self.n_a..self.len() {
            let md = &self.modes[i];
            out[i] = if md.head == Head::Batter {
                // the pair, turned by φ: cos(mθ − φ) and sin(mθ − φ)
                let a = md.m as f64 * theta - md.phi;
                md.scale * basis.modes[md.basis_index].radial(r) * if md.sin { a.sin() } else { a.cos() }
            } else {
                0.0
            };
        }
    }

    /// [`Self::strike_vector`], allocated.
    pub fn projection(&self, r_frac: f64, theta: f64) -> Vec<f64> {
        let mut scratch = vec![0.0; self.rows.len()];
        let mut out = vec![0.0; self.len()];
        self.strike_vector(r_frac, theta, &mut scratch, &mut out);
        out
    }

    /// Each mode's weight in an output (pascals per unit of modal acceleration, before the output gain):
    /// the close mic's `c_i` or the far monopole's `ρ_a/(4π r_far)·ã_i`. Per mode, in the order of `modes`.
    pub fn output_weights(&self, close: bool) -> Vec<f64> {
        let w = if close { &self.close_w } else { &self.far_w };
        (0..self.len()).map(|i| w[self.slot(i)]).collect()
    }

    /// `∂w_b/∂η_i` of one mode (slow: a whole strike vector; for tests and tables).
    pub fn shape(&self, i: usize, r_frac: f64, theta: f64) -> f64 {
        self.projection(r_frac, theta)[i]
    }
}
