//! Real-time state and time stepping (§4.9).
//!
//! Stage 2: the linear part. Per sample, with `x_h = x + (k/2)p`:
//!
//! ```text
//! p_new = ((1 − kσ̃)·p − k·Ω̃²·x_h + k·f) / (1 + kσ̃)
//! x_new = x_h + (k/2)·p_new
//! ```
//!
//! Stage 3 adds the stick: one more degree of freedom `z` (Ω = σ = 0) and the contact
//! potential, integrated with the scalar auxiliary variable scheme of §4.9. All the
//! nonlinearity acts along one direction, `ĝ = [e, −1/√m_s]` (the strike vector and the
//! stick), so the Sherman–Morrison solve costs two dot products, not a matrix.

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
    /// the strike vector e_i = Φ_i(x_e), and Σ ĝ²/D (the Sherman–Morrison denominator's part)
    e: Vec<f64>,
    g_w: f64,
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
            x: v.clone(), p: v.clone(), acc: v.clone(), xh: v.clone(), y: v.clone(), e: v,
            g_w: 0.0, z: 0.0, pz: 0.0, stick_active: false, away: 0,
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

    /// `E = ½p·p + ½ΣΩ̃²(x² − (k²/4)p²)`: conserved exactly (to rounding) when σ = 0.
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

    /// The displacement of the head at (r, θ) (r a fraction of the radius): Σ Φ_i(x)·η_i.
    pub fn displacement_at(&self, model: &DrumModel, r_frac: f64, theta: f64) -> f64 {
        (0..self.n).map(|i| model.shape(i, r_frac, theta) * self.x[i]).sum()
    }

    /// Strikes the head (§4.11): the stick starts one sample of travel above it, moving down at
    /// `v0 = v_min + (v_max − v_min)·vel^γ_v`. A retrigger while the stick is in contact just
    /// resets the stick. Real-time safe: no Bessel evaluations, only the radial tables.
    pub fn trigger(&mut self, model: &DrumModel, hit: Hit) {
        let sp = &model.params.stick;
        let r = hit.radius.clamp(0.0, 0.95);
        let mut ew = 0.0;
        for i in 0..self.n {
            let mut e = model.shape(i, r, hit.angle);
            if model.params.contact_radius > 0.0 {
                let ka = model.basis.modes[model.modes[i].basis_index].kappa * model.params.contact_radius;
                e *= (-0.5 * ka * ka).exp();
            }
            self.e[i] = e;
            ew += e * e / model.d_plus[i];
        }
        let inv_ms = 1.0 / sp.mass;
        self.g_w = ew + inv_ms;
        let v0 = sp.v_min + (sp.v_max - sp.v_min) * (hit.velocity.clamp(0.0, 1.0) as f64).powf(sp.velocity_gamma);
        let sq = sp.mass.sqrt();
        let w: f64 = (0..self.n).map(|i| self.e[i] * self.x[i]).sum();
        self.z = sq * (w + v0 * model.k);
        self.pz = -sq * v0;
        self.stick_active = true;
        self.away = 0;
        // ψ re-sync: the stick is clear of the head, so V = 0
        self.psi = model.params.sav_epsilon.sqrt();
        self.force = 0.0;
        self.force_prev = 0.0;
        self.e_peak = self.energy(model);
    }

    /// One sample of the SAV scheme (§4.9): the modes plus the stick's contact.
    pub fn step(&mut self, model: &DrumModel) {
        let k = model.k;
        let hk = 0.5 * k;
        let sp = &model.params.stick;
        let (ks, alpha, eps) = (sp.stiffness, sp.exponent, model.params.sav_epsilon);
        let inv_sq = 1.0 / sp.mass.sqrt();
        let n = self.x.len();
        // 1. the half step, the head's displacement under the stick, and e·p
        let (mut ex, mut ep) = (0.0, 0.0);
        for i in 0..n {
            let xh = self.x[i] + hk * self.p[i];
            self.xh[i] = xh;
            ex += self.e[i] * xh;
            ep += self.e[i] * self.p[i];
        }
        let zh = self.z + hk * self.pz;
        let delta = ex - zh * inv_sq;
        // 2. the potential and the gradient's size along ĝ = [e, −1/√m_s]
        let (force, v) = if self.stick_active && delta > 0.0 {
            (ks * delta.powf(alpha), ks / (alpha + 1.0) * delta.powf(alpha + 1.0))
        } else {
            (0.0, 0.0)
        };
        let psi_hat = (2.0 * v + eps).sqrt();
        let s = force / psi_hat; // g = s·ĝ
        // 3. the right-hand side: b_i = (1−kσ̃)p_i − k·Ω̃²·x_h,i − ĝ_i·coef
        let gp = ep - inv_sq * self.pz; // ĝ·p
        let coef = s * (0.25 * k * k * s * gp + k * self.psi);
        let mut ey = 0.0;
        for i in 0..n {
            let b = model.d_minus[i] * self.p[i] - k * model.omega_t2[i] * self.xh[i] - self.e[i] * coef;
            let y = b / model.d_plus[i];
            self.y[i] = y;
            ey += self.e[i] * y;
        }
        let bz = self.pz + inv_sq * coef; // ĝ_z = −1/√m_s, D_z = 1
        let gy = ey - inv_sq * bz; // ĝ·y
        // 4. Sherman–Morrison: p_new = y − w·(u·y)/(1 + u·w), u = (k/2)·s·ĝ, w = u/D
        let uy = hk * s * gy;
        let uw = hk * hk * s * s * self.g_w;
        let fac = hk * s * uy / (1.0 + uw);
        let mut gpp = 0.0; // ĝ·(p_new + p)
        for i in 0..n {
            let pn = self.y[i] - fac * self.e[i] / model.d_plus[i];
            self.acc[i] = (pn - self.p[i]) / k;
            gpp += self.e[i] * (pn + self.p[i]);
            self.x[i] = self.xh[i] + hk * pn;
            self.p[i] = pn;
        }
        let pzn = bz + fac * inv_sq; // −(−1/√m_s)·fac/1
        gpp -= inv_sq * (pzn + self.pz);
        self.z = zh + hk * pzn;
        self.pz = pzn;
        // 5. ψ
        self.psi += hk * s * gpp;
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
    /// The scheme's ψ (and what it should be, `sqrt(2V(x) + ε)`): the SAV's accuracy.
    pub fn psi(&self) -> f64 {
        self.psi
    }

    /// The acceleration of the head at (r, θ) over the last step: Σ Φ_i(x)·a_i.
    pub fn acceleration_at(&self, model: &DrumModel, r_frac: f64, theta: f64) -> f64 {
        (0..self.n).map(|i| model.shape(i, r_frac, theta) * self.acc[i]).sum()
    }

    /// TEMPORARY, for the stage-2 tests: an impulse J (kg·m/s per unit of mass-normalised
    /// shape) at (r, θ): every mode gets `p_i += Φ_i(x_e)·J`.
    #[doc(hidden)]
    pub fn test_impulse(&mut self, model: &DrumModel, r_frac: f64, theta: f64, impulse: f64) {
        for i in 0..self.n {
            self.p[i] += model.shape(i, r_frac, theta) * impulse;
        }
    }
}
