//! Real-time state and time stepping (§4.9).
//!
//! Stage 2: the linear part only. Per sample, with `x_h = x + (k/2)p`:
//!
//! ```text
//! p_new = ((1 − kσ̃)·p − k·Ω̃²·x_h + k·f) / (1 + kσ̃)
//! x_new = x_h + (k/2)·p_new
//! ```
//!
//! (the nonlinear terms of the SAV scheme join in at stage 3.)

use crate::prepare::DrumModel;

/// One voice: the state of every mode, sized for one model. No allocation after `new`.
pub struct DrumVoice {
    /// mass-normalised modal displacement η_i and velocity p_i
    x: Vec<f64>,
    p: Vec<f64>,
    /// the scheme's acceleration (p_new − p)/k of the last step
    acc: Vec<f64>,
    /// scratch for the impulse's strike vector
    n: usize,
}

impl DrumVoice {
    pub fn new(model: &DrumModel) -> Self {
        let n = model.omega_t2.len();
        DrumVoice { x: vec![0.0; n], p: vec![0.0; n], acc: vec![0.0; n], n: model.len() }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.x.fill(0.0);
        self.p.fill(0.0);
        self.acc.fill(0.0);
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

    /// TEMPORARY, for the stage-2 tests: an impulse J (kg·m/s per unit of mass-normalised
    /// shape) at (r, θ): every mode gets `p_i += Φ_i(x_e)·J`.
    #[doc(hidden)]
    pub fn test_impulse(&mut self, model: &DrumModel, r_frac: f64, theta: f64, impulse: f64) {
        for i in 0..self.n {
            self.p[i] += model.shape(i, r_frac, theta) * impulse;
        }
    }
}
