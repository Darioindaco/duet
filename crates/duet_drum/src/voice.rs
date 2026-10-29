//! Real-time state and time stepping (§4.9).
//!
//! The state is the coordinates η of the final modes (block A's first, then the pairs), their
//! velocities p, the stick (one more degree of freedom `z`, Ω = σ = 0) and the scalar auxiliary
//! variable ψ. Per sample, with `x_h = x + (k/2)p`:
//!
//! ```text
//! ψ̂ = sqrt(2V(x_h) + ε),   g = ∇V(x_h)/ψ̂,   V = V_tension + V_contact
//! b = (1 − kσ̃)p − (k²/4)·g·(g·p) + k(−Ω̃²x_h − gψ)
//! (D + uuᵀ)p_new = b,   u = (k/2)g,   D = 1 + kσ̃              (Sherman–Morrison)
//! x_new = x_h + (k/2)p_new,   ψ_new = ψ + (k/2)g·(p_new + p)
//! ```
//!
//! The tension potential is `Σ_h C_h·G_h²` with `G_h = Σκ²q²` over head h's modes, `q = Tη`; block A's
//! `T_A` is dense (small), the pairs' are diagonal.

use crate::prepare::DrumModel;

/// A strike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// 0..1
    pub velocity: f32,
    /// where on the head, as a fraction of the radius (0..0.95)
    pub radius: f64,
    /// rad
    pub angle: f64,
    /// within the next `process()` block (stage 7; `trigger` itself acts at once)
    pub sample_offset: u32,
}

/// The stick leaves after this many consecutive samples out of contact and moving away.
const STICK_OFF_AFTER: u32 = 64;

/// One voice: the state of every mode, sized for one model. No allocation after `new`.
pub struct DrumVoice {
    /// mass-normalised modal displacement η_i and velocity p_i
    x: Vec<f64>,
    p: Vec<f64>,
    /// the scheme's acceleration (p_new − p)/k of the last step
    acc: Vec<f64>,
    /// scratch
    xh: Vec<f64>,
    y: Vec<f64>,
    g: Vec<f64>,
    /// block A's rows: q = T_A·x_h
    q: Vec<f64>,
    /// the strike vector e_i = ∂w_b(x_e)/∂η_i
    e: Vec<f64>,
    /// the stick: z = √m_s·(position), its velocity, and whether it is in play
    z: f64,
    pz: f64,
    stick_active: bool,
    away: u32,
    /// the scalar auxiliary variable
    psi: f64,
    /// contact force of the last step and the one before (N)
    force: f64,
    force_prev: f64,
    /// kinetic energy taken away with the stick when it left (J)
    stick_energy_removed: f64,
    /// the energy right after the last trigger
    e_peak: f64,
    n: usize,
}

impl DrumVoice {
    pub fn new(model: &DrumModel) -> Self {
        let n = model.omega_t2.len();
        let v = vec![0.0; n];
        DrumVoice {
            x: v.clone(), p: v.clone(), acc: v.clone(), xh: v.clone(), y: v.clone(), g: v.clone(), e: v,
            q: vec![0.0; model.rows.len()],
            z: 0.0, pz: 0.0, stick_active: false, away: 0,
            psi: model.params.sav_epsilon.sqrt(), force: 0.0, force_prev: 0.0, stick_energy_removed: 0.0, e_peak: 0.0, n: model.len(),
        }
    }

    /// Clears the state.
    pub fn reset(&mut self, model: &DrumModel) {
        self.x.fill(0.0);
        self.p.fill(0.0);
        self.acc.fill(0.0);
        self.e.fill(0.0);
        self.z = 0.0;
        self.pz = 0.0;
        self.stick_active = false;
        self.away = 0;
        self.psi = model.params.sav_epsilon.sqrt();
        self.force = 0.0;
        self.force_prev = 0.0;
        self.stick_energy_removed = 0.0;
        self.e_peak = 0.0;
    }

    /// One sample of the linear scheme. `force` (modal, length ≥ model.len(), or none) is
    /// the force on every mode during this sample.
    pub fn step_linear(&mut self, model: &DrumModel, force: Option<&[f64]>) {
        let k = model.k;
        let hk = 0.5 * k;
        for i in 0..self.x.len() {
            let xh = self.x[i] + hk * self.p[i];
            let f = force.map_or(0.0, |f| f.get(i).copied().unwrap_or(0.0));
            let b = model.d_minus[i] * self.p[i] + k * (f - model.omega_t2[i] * xh);
            let pn = b / model.d_plus[i];
            self.acc[i] = (pn - self.p[i]) / k;
            self.x[i] = xh + hk * pn;
            self.p[i] = pn;
        }
    }

    /// `E = ½p·p + ½ΣΩ̃²(x² − (k²/4)p²)`: conserved exactly (to rounding) when σ = 0 and there is no nonlinearity.
    pub fn energy_linear(&self, model: &DrumModel) -> f64 {
        let q = 0.25 * model.k * model.k;
        let mut e = 0.0;
        for i in 0..self.x.len() {
            e += 0.5 * self.p[i] * self.p[i] + 0.5 * model.omega_t2[i] * (self.x[i] * self.x[i] - q * self.p[i] * self.p[i]);
        }
        e
    }

    /// Modal displacements and velocities.
    pub fn x(&self) -> &[f64] {
        &self.x[..self.n]
    }
    pub fn p(&self) -> &[f64] {
        &self.p[..self.n]
    }

    /// Σ w_i·η_i: a displacement at a point, with the weights of [`DrumModel::projection`].
    pub fn displacement(&self, weights: &[f64]) -> f64 {
        weights.iter().zip(&self.x).map(|(w, x)| w * x).sum()
    }

    /// Σ w_i·a_i: the acceleration of the last step at a point.
    pub fn acceleration(&self, weights: &[f64]) -> f64 {
        weights.iter().zip(&self.acc).map(|(w, a)| w * a).sum()
    }

    /// Strikes the head (§4.11): the stick starts one sample of travel above it, moving down at
    /// `v0 = v_min + (v_max − v_min)·vel^γ_v`. A retrigger while the stick is in contact just
    /// resets the stick. Real-time safe: no Bessel evaluations, only the radial tables.
    pub fn trigger(&mut self, model: &DrumModel, hit: Hit) {
        let sp = &model.params.stick;
        let r = hit.radius.clamp(0.0, 0.95);
        model.strike_vector(r, hit.angle, &mut self.q, &mut self.e[..self.n]);
        let v0 = sp.v_min + (sp.v_max - sp.v_min) * (hit.velocity.clamp(0.0, 1.0) as f64).powf(sp.velocity_gamma);
        let sq = sp.mass.sqrt();
        let w: f64 = (0..self.n).map(|i| self.e[i] * self.x[i]).sum();
        self.z = sq * (w + v0 * model.k);
        self.pz = -sq * v0;
        self.stick_active = true;
        self.away = 0;
        // ψ re-sync: the stick is clear of the head, so V_contact = 0 (the tension's part is
        // not zero when the head is still ringing)
        let g = self.tension_g(model);
        let v_tm = model.c_tm[0] * g[0] * g[0] + model.c_tm[1] * g[1] * g[1];
        self.psi = (2.0 * v_tm + model.params.sav_epsilon).sqrt();
        self.force = 0.0;
        self.force_prev = 0.0;
        self.e_peak = self.energy(model);
    }

    /// `G_h = Σ_{k∈h} κ_k² q_k²` for both heads, from the current state.
    fn tension_g(&self, model: &DrumModel) -> [f64; 2] {
        let na = model.n_a;
        let mut gh = [0.0; 2];
        for (r, row) in model.rows.iter().enumerate() {
            let t = &model.t_a[r * na..(r + 1) * na];
            let q: f64 = t.iter().zip(&self.x[..na]).map(|(t, x)| t * x).sum();
            gh[row.head] += row.kappa2 * q * q;
        }
        for i in na..self.n {
            gh[model.head_idx[i]] += model.w_k2[i] * self.x[i] * self.x[i];
        }
        gh
    }

    /// One sample of the SAV scheme (§4.9): the modes, the tension modulation and the stick's contact.
    pub fn step(&mut self, model: &DrumModel) {
        let k = model.k;
        let hk = 0.5 * k;
        let sp = &model.params.stick;
        let (ks, alpha, eps) = (sp.stiffness, sp.exponent, model.params.sav_epsilon);
        let inv_sq = 1.0 / sp.mass.sqrt();
        let n = self.x.len();
        let (na, nr) = (model.n_a, model.rows.len());
        // 1. the half step, and the head's displacement under the stick
        let mut ex = 0.0;
        for i in 0..n {
            let xh = self.x[i] + hk * self.p[i];
            self.xh[i] = xh;
            ex += self.e[i] * xh;
        }
        let zh = self.z + hk * self.pz;
        let delta = ex - zh * inv_sq;
        // 2. G_h per head (block A through its dense T, the pairs diagonally)
        let mut gh = [0.0f64; 2];
        for r in 0..nr {
            let t = &model.t_a[r * na..(r + 1) * na];
            let mut q = 0.0;
            for i in 0..na {
                q += t[i] * self.xh[i];
            }
            self.q[r] = q;
            gh[model.rows[r].head] += model.rows[r].kappa2 * q * q;
        }
        for i in na..n {
            gh[model.head_idx[i]] += model.w_k2[i] * self.xh[i] * self.xh[i];
        }
        let (force, vc) = if self.stick_active && delta > 0.0 {
            (ks * delta.powf(alpha), ks / (alpha + 1.0) * delta.powf(alpha + 1.0))
        } else {
            (0.0, 0.0)
        };
        let v = model.c_tm[0] * gh[0] * gh[0] + model.c_tm[1] * gh[1] * gh[1] + vc;
        let inv_psi_hat = 1.0 / (2.0 * v + eps).sqrt();
        // 3. g = ∇V/ψ̂: ∂V_tm/∂q_k = 4·C_h·G_h·κ_k²·q_k, pulled back with Tᵀ; then the contact's F·e
        let tm = [4.0 * model.c_tm[0] * gh[0], 4.0 * model.c_tm[1] * gh[1]];
        self.g[..na].fill(0.0);
        for r in 0..nr {
            let dq = tm[model.rows[r].head] * model.rows[r].kappa2 * self.q[r];
            if dq != 0.0 {
                let t = &model.t_a[r * na..(r + 1) * na];
                for i in 0..na {
                    self.g[i] += t[i] * dq;
                }
            }
        }
        for i in na..n {
            self.g[i] = tm[model.head_idx[i]] * model.w_k2[i] * self.xh[i];
        }
        let mut gp = 0.0;
        for i in 0..n {
            let g = (self.g[i] + force * self.e[i]) * inv_psi_hat;
            self.g[i] = g;
            gp += g * self.p[i];
        }
        let gz = -force * inv_sq * inv_psi_hat;
        gp += gz * self.pz;
        // 4. b, y = b/D, and the dot products of the update
        let (mut uy, mut uw) = (0.0, 0.0);
        let q = 0.25 * k * k * gp;
        for i in 0..n {
            let b = model.d_minus[i] * self.p[i] - q * self.g[i] + k * (-model.omega_t2[i] * self.xh[i] - self.g[i] * self.psi);
            let y = b / model.d_plus[i];
            self.y[i] = y;
            uy += hk * self.g[i] * y;
            uw += hk * hk * self.g[i] * self.g[i] / model.d_plus[i];
        }
        let bz = self.pz - q * gz + k * (-gz * self.psi);
        uy += hk * gz * bz;
        uw += hk * hk * gz * gz;
        // 5. p_new = y − w·(u·y)/(1 + u·w), w = u/D; x_new = x_h + (k/2)p_new; ψ_new = ψ + (k/2)g·(p_new + p)
        let fac = uy / (1.0 + uw);
        let mut gpp = 0.0;
        for i in 0..n {
            let pn = self.y[i] - hk * self.g[i] / model.d_plus[i] * fac;
            self.acc[i] = (pn - self.p[i]) / k;
            gpp += self.g[i] * (pn + self.p[i]);
            self.x[i] = self.xh[i] + hk * pn;
            self.p[i] = pn;
        }
        let pzn = bz - hk * gz * fac;
        gpp += gz * (pzn + self.pz);
        self.z = zh + hk * pzn;
        self.pz = pzn;
        self.psi += hk * gpp;
        self.force_prev = self.force;
        self.force = force;
        // 6. the stick leaves once it has been clear of the head and moving away for a while
        if self.stick_active {
            if delta < 0.0 && self.pz > 0.0 {
                self.away += 1;
                if self.away >= STICK_OFF_AFTER {
                    self.stick_energy_removed += 0.5 * self.pz * self.pz;
                    self.stick_active = false;
                    self.pz = 0.0;
                }
            } else {
                self.away = 0;
            }
        }
    }

    /// The batter's tension modulation: its rise in tension as a fraction of the nominal tension,
    /// `ΔT/T = E·h/(2A(1−ν²)σ)·G / T` (§4.8a, Berger).
    pub fn tension_rise(&self, model: &DrumModel) -> f64 {
        let g = self.tension_g(model);
        let sigma = model.params.surface_density(crate::params::Head::Batter);
        // C = E h/(8A(1−ν²)σ²) (× γ_nl): ΔT = 4·C·σ·G
        4.0 * model.c_tm[0] * sigma * g[0] / model.params.batter.tension
    }

    /// `E = ½p·p + ½ΣΩ̃²(x² − (k²/4)p²) + ½p_z² + ½ψ²` (the stick has Ω̃ = 0). Conserved to
    /// rounding when σ = 0, apart from the stick's kinetic energy when it leaves
    /// ([`Self::stick_energy_removed`]).
    pub fn energy(&self, model: &DrumModel) -> f64 {
        self.energy_linear(model) + 0.5 * self.pz * self.pz + 0.5 * self.psi * self.psi
    }

    /// Kinetic energy that left with the stick (J).
    pub fn stick_energy_removed(&self) -> f64 {
        self.stick_energy_removed
    }

    /// The energy right after the last trigger (J).
    pub fn peak_energy(&self) -> f64 {
        self.e_peak
    }

    /// The contact force of the last step (N), and the step before.
    pub fn force(&self) -> f64 {
        self.force
    }
    pub fn force_prev(&self) -> f64 {
        self.force_prev
    }
    pub fn stick_active(&self) -> bool {
        self.stick_active
    }
    /// The scheme's ψ (what it should be: `sqrt(2V(x) + ε)`).
    pub fn psi(&self) -> f64 {
        self.psi
    }

    /// TEMPORARY, for the stage-2 tests: an impulse J at (r, θ): every mode gets
    /// `p_i += (∂w_b/∂η_i)·J`.
    #[doc(hidden)]
    pub fn test_impulse(&mut self, model: &DrumModel, r_frac: f64, theta: f64, impulse: f64) {
        let w = model.projection(r_frac, theta);
        for i in 0..self.n {
            self.p[i] += w[i] * impulse;
        }
    }
}
