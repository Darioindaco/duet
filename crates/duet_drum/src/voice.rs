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
    /// the last step's output sums: Σ close_w·a and Σ far_w·a (a: the modes' accelerations)
    out_acc: (f64, f64),
    /// the strike vector per mode, before it is laid out in slots
    ec: Vec<f64>,
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
    /// hits queued for a later sample of the next block(s) (capacity reserved: no allocation)
    pending: Vec<Hit>,
    /// asleep: silent, nothing computed (§4.11)
    sleeping: bool,
    /// the dipole term's distance from the last strike to the close mic (m)
    d_s: f64,
    /// the outputs' 1-pole high-pass: (previous input, previous output) for close and far
    hp: [(f64, f64); 2],
    /// the attack term's low-pass: the filtered force of this sample and the last, and its state
    force_lp: (f64, f64),
    lp_state: [(f64, f64); 2],
    /// samples until the next sleep check
    sleep_check: u32,
    /// how many times a non-finite output made `process` reset the voice
    errors: u32,
}

/// Flush-to-zero and denormals-are-zero for the duration of a block (§5): decaying modes would
/// otherwise spend their tails in denormal arithmetic, which is tens of times slower. The one `unsafe`
/// of the crate: it reads and writes the CPU's floating-point control register and puts it back.
struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    saved: u32,
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl DenormalGuard {
    #[inline]
    #[allow(deprecated)]
    fn new() -> Self {
        // SAFETY: MXCSR / FPCR only change how this thread rounds tiny floating-point numbers; the
        // previous value is restored on drop.
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let saved = std::arch::x86_64::_mm_getcsr();
            std::arch::x86_64::_mm_setcsr(saved | 0x8040); // FTZ (bit 15) and DAZ (bit 6)
            DenormalGuard { saved }
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let saved: u64;
            std::arch::asm!("mrs {}, fpcr", out(reg) saved, options(nomem, nostack));
            std::arch::asm!("msr fpcr, {}", in(reg) saved | (1 << 24), options(nomem, nostack)); // FZ
            DenormalGuard { saved }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        DenormalGuard {}
    }
}

impl Drop for DenormalGuard {
    #[inline]
    #[allow(deprecated)]
    fn drop(&mut self) {
        // SAFETY: restores what `new` saved.
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_setcsr(self.saved);
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.saved, options(nomem, nostack));
        }
    }
}

const MAX_PENDING: usize = 16;
const SLEEP_CHECK_EVERY: u32 = 32;

impl DrumVoice {
    pub fn new(model: &DrumModel) -> Self {
        let n = model.n_slots;
        let v = vec![0.0; n];
        DrumVoice {
            x: v.clone(), p: v.clone(), acc: v.clone(), xh: v.clone(), y: v.clone(), g: v.clone(), e: v,
            q: vec![0.0; model.rows.len()],
            out_acc: (0.0, 0.0),
            ec: vec![0.0; model.len()],
            z: 0.0, pz: 0.0, stick_active: false, away: 0,
            psi: model.params.sav_epsilon.sqrt(), force: 0.0, force_prev: 0.0, stick_energy_removed: 0.0, e_peak: 0.0, n: model.len(),
            pending: Vec::with_capacity(MAX_PENDING), sleeping: true, d_s: 1.0, hp: [(0.0, 0.0); 2], force_lp: (0.0, 0.0), lp_state: [(0.0, 0.0); 2], sleep_check: SLEEP_CHECK_EVERY, errors: 0,
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
        self.pending.clear();
        self.sleeping = true;
        self.hp = [(0.0, 0.0); 2];
        self.force_lp = (0.0, 0.0);
        self.lp_state = [(0.0, 0.0); 2];
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

    /// Modal displacements and velocities, per mode (copies, in the order of `model.modes`).
    pub fn x(&self, model: &DrumModel) -> Vec<f64> {
        (0..self.n).map(|i| self.x[model.slot(i)]).collect()
    }
    pub fn p(&self, model: &DrumModel) -> Vec<f64> {
        (0..self.n).map(|i| self.p[model.slot(i)]).collect()
    }

    /// Σ w_i·η_i: a displacement at a point, with the weights of [`DrumModel::projection`].
    pub fn displacement(&self, model: &DrumModel, weights: &[f64]) -> f64 {
        (0..self.n).map(|i| weights[i] * self.x[model.slot(i)]).sum()
    }

    /// Σ w_i·a_i: the acceleration of the last step at a point.
    pub fn acceleration(&self, model: &DrumModel, weights: &[f64]) -> f64 {
        (0..self.n).map(|i| weights[i] * self.acc[model.slot(i)]).sum()
    }

    /// Strikes the head (§4.11): the stick starts one sample of travel above it, moving down at
    /// `v0 = v_min + (v_max − v_min)·vel^γ_v`. A retrigger while the stick is in contact just
    /// resets the stick. Real-time safe: no Bessel evaluations, only the radial tables.
    ///
    /// A hit with `sample_offset == 0` acts at once; one with an offset waits in a small queue and
    /// is applied by [`Self::process`] at that sample of its next block.
    pub fn trigger(&mut self, model: &DrumModel, hit: Hit) {
        if hit.sample_offset == 0 {
            self.apply_hit(model, hit);
        } else if self.pending.len() < MAX_PENDING {
            self.pending.push(hit);
        }
    }

    fn apply_hit(&mut self, model: &DrumModel, hit: Hit) {
        if self.sleeping {
            // on wake, clear the state (and the outputs' filters)
            let pending = std::mem::take(&mut self.pending);
            self.reset(model);
            self.pending = pending;
            self.sleeping = false;
        }
        let sp = &model.params.stick;
        let r = hit.radius.clamp(0.0, 0.95);
        model.strike_vector(r, hit.angle, &mut self.q, &mut self.ec);
        if model.params.contact_radius > 0.0 {
            // a Gaussian patch: the highest modes hardly feel the stick
            for i in 0..self.n {
                let ka = model.mode_kappa[i] * model.params.contact_radius;
                self.ec[i] *= (-0.5 * ka * ka).exp();
            }
        }
        self.e.fill(0.0);
        for i in 0..self.n {
            self.e[model.slot(i)] = self.ec[i];
        }
        let v0 = sp.v_min + (sp.v_max - sp.v_min) * (hit.velocity.clamp(0.0, 1.0) as f64).powf(sp.velocity_gamma);
        let sq = sp.mass.sqrt();
        let w: f64 = (0..self.n).map(|i| self.ec[i] * self.x[model.slot(i)]).sum();
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
        // the dipole term's distance: from the strike point to the close mic
        let (xe, ye) = (r * model.bases[0].as_ref().unwrap().params.radius * hit.angle.cos(), r * model.bases[0].as_ref().unwrap().params.radius * hit.angle.sin());
        self.d_s = (model.mic_h * model.mic_h + (model.mic_xy.0 - xe).powi(2) + (model.mic_xy.1 - ye).powi(2)).sqrt();
        self.e_peak = self.physical_energy(model);
        self.sleep_check = SLEEP_CHECK_EVERY;
    }

    /// The energy without the SAV's gauge: what the head and the stick really hold.
    pub fn physical_energy(&self, model: &DrumModel) -> f64 {
        self.energy_linear(model) + 0.5 * self.pz * self.pz
    }

    /// Asleep: silent, and `process` costs nothing until the next hit.
    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }

    /// How many times a non-finite output made `process` reset the voice.
    pub fn error_count(&self) -> u32 {
        self.errors
    }

    /// Renders a block (§4.10): the close mic and the far monopole, in pascals times the output
    /// gains, high-passed at 20 Hz. Hits queued with a `sample_offset` land at that sample.
    /// Real-time safe: no allocation, no locks.
    pub fn process(&mut self, model: &DrumModel, close: &mut [f32], far: &mut [f32]) {
        let n = close.len().min(far.len());
        let k = model.k;
        let _denormals = DenormalGuard::new();
        for s in 0..n {
            // queued hits whose sample has come
            let mut i = 0;
            while i < self.pending.len() {
                if self.pending[i].sample_offset as usize == s {
                    let h = Hit { sample_offset: 0, ..self.pending.remove(i) };
                    self.apply_hit(model, h);
                } else {
                    i += 1;
                }
            }
            if self.sleeping {
                close[s] = 0.0;
                far[s] = 0.0;
                continue;
            }
            self.step(model);
            let mut pc = self.out_acc.0;
            let pf = self.out_acc.1;
            // the contact force, low-passed (4 poles, transposed direct form II), then differentiated
            let mut fl = self.force;
            if let Some(sections) = &model.attack_lp {
                for (st, [b0, b1, b2, a1, a2]) in self.lp_state.iter_mut().zip(sections) {
                    let y = b0 * fl + st.0;
                    st.0 = b1 * fl - a1 * y + st.1;
                    st.1 = b2 * fl - a2 * y;
                    fl = y;
                }
            }
            self.force_lp = (fl, self.force_lp.0);
            pc += model.attack_k * (self.force_lp.0 - self.force_lp.1) / k / self.d_s;
            // 1-pole high-pass: y = a·(y_prev + x − x_prev)
            let a = model.hp_a;
            let yc = a * (self.hp[0].1 + pc - self.hp[0].0);
            let yf = a * (self.hp[1].1 + pf - self.hp[1].0);
            self.hp = [(pc, yc), (pf, yf)];
            close[s] = (yc * model.gain_close) as f32;
            far[s] = (yf * model.gain_far) as f32;
            self.sleep_check -= 1;
            if self.sleep_check == 0 {
                self.sleep_check = SLEEP_CHECK_EVERY;
                if !self.stick_active && self.physical_energy(model) < 1e-12 * self.e_peak {
                    self.sleeping = true;
                }
            }
        }
        for h in self.pending.iter_mut() {
            h.sample_offset = h.sample_offset.saturating_sub(n as u32);
        }
        // a non-finite output: reset the voice and say so
        if close[..n].iter().chain(far[..n].iter()).any(|x| !x.is_finite()) {
            debug_assert!(false, "non-finite output");
            let pending = std::mem::take(&mut self.pending);
            self.reset(model);
            self.pending = pending;
            self.errors += 1;
            close[..n].fill(0.0);
            far[..n].fill(0.0);
        }
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
        for i in model.pad_a..model.n_slots {
            gh[0] += model.w_b[i] * self.x[i] * self.x[i];
            gh[1] += model.w_r[i] * self.x[i] * self.x[i];
        }
        gh
    }

    /// One sample of the SAV scheme (§4.9): the modes, the tension modulation and the stick's contact.
    ///
    /// The potential is `V = Σ_h C_h·G_h² + V_contact`, `G_h = Σκ²q²`; its gradient `g = ∇V/ψ̂` is one vector,
    /// so the implicit solve `(D + u uᵀ)p_new = b` (`u = (k/2)g`) is a Sherman–Morrison update. The pairs
    /// (most of the modes) are diagonal, and each head's share of g is `tm_h·w_i·x_i + F·e_i`, so the work is
    /// three sweeps over the modes with no division in them: (1) the half step and the sums, (2) g, b, y and
    /// the two dot products, (3) the update, ψ and the outputs' sums. Block A (22 modes) has its dense T.
    pub fn step(&mut self, model: &DrumModel) {
        let k = model.k;
        let hk = 0.5 * k;
        let sp = &model.params.stick;
        let (ks, alpha, eps) = (sp.stiffness, sp.exponent, model.params.sav_epsilon);
        let inv_sq = 1.0 / sp.mass.sqrt();
        let (na, pa, n) = (model.n_a, model.pad_a, model.n_slots);
        let nr = model.rows.len();

        // ---- sweep 1: the half step, e·x_h, and the sums of the tension's gradient (pairs, in lanes of four)
        let mut ex = 0.0;
        for i in 0..na {
            let xh = self.x[i] + hk * self.p[i];
            self.xh[i] = xh;
            ex += self.e[i] * xh;
        }
        let s1 = sweep1(hk, &self.x[pa..], &self.p[pa..], &self.e[pa..], &model.w_b[pa..], &model.w_r[pa..], &mut self.xh[pa..]);
        let lane = |a: &[f64; 4]| a[0] + a[1] + a[2] + a[3];
        ex += lane(&s1[0]);
        let s_ep = lane(&s1[1]);
        let mut gh = [lane(&s1[2]), lane(&s1[3])];
        let swxp = [lane(&s1[4]), lane(&s1[5])];
        // block A's G through its dense T
        for r in 0..nr {
            let t = &model.t_a[r * na..(r + 1) * na];
            let mut q = 0.0;
            for i in 0..na {
                q += t[i] * self.xh[i];
            }
            self.q[r] = q;
            gh[model.rows[r].head] += model.rows[r].kappa2 * q * q;
        }
        let zh = self.z + hk * self.pz;
        let delta = ex - zh * inv_sq;
        let (force, vc) = if self.stick_active && delta > 0.0 {
            // (F = K δ^α, V = K δ^(α+1)/(α+1) = F·δ/(α+1): one powf)
            let f = ks * delta.powf(alpha);
            (f, f * delta / (alpha + 1.0))
        } else {
            (0.0, 0.0)
        };
        let v = model.c_tm[0] * gh[0] * gh[0] + model.c_tm[1] * gh[1] * gh[1] + vc;
        let ipsi = 1.0 / (2.0 * v + eps).sqrt();
        let tm = [4.0 * model.c_tm[0] * gh[0], 4.0 * model.c_tm[1] * gh[1]];

        // ---- block A's g (dense), and g·p over everything
        self.g[..na].fill(0.0);
        for r in 0..nr {
            let dq = tm[model.rows[r].head] * model.rows[r].kappa2 * self.q[r];
            let t = &model.t_a[r * na..(r + 1) * na];
            for i in 0..na {
                self.g[i] += t[i] * dq;
            }
        }
        let mut gp = 0.0;
        for i in 0..na {
            let g = (self.g[i] + force * self.e[i]) * ipsi;
            self.g[i] = g;
            gp += g * self.p[i];
        }
        gp += ipsi * (tm[0] * swxp[0] + tm[1] * swxp[1] + force * s_ep);
        let gz = -force * inv_sq * ipsi;
        gp += gz * self.pz;
        let q = 0.25 * k * k * gp;
        let coef = q + k * self.psi; // b_i = (1−kσ̃)p_i − k·Ω̃²·x_h,i − g_i·coef

        // ---- sweep 2: g (pairs), b, y = b/D, u·y and u·w
        let (mut uy, mut uw) = (0.0, 0.0);
        for i in 0..na {
            let b = model.d_minus[i] * self.p[i] - k * model.omega_t2[i] * self.xh[i] - self.g[i] * coef;
            let y = b * model.inv_d_plus[i];
            self.y[i] = y;
            uy += self.g[i] * y;
            uw += self.g[i] * self.g[i] * model.inv_d_plus[i];
        }
        let s2 = sweep2(
            k, coef, tm[0] * ipsi, tm[1] * ipsi, force * ipsi,
            &self.p[pa..], &self.e[pa..], &model.w_b[pa..], &model.w_r[pa..], &self.xh[pa..],
            &model.d_minus[pa..], &model.omega_t2[pa..], &model.inv_d_plus[pa..], &mut self.g[pa..], &mut self.y[pa..],
        );
        uy += lane(&s2.0);
        uw += lane(&s2.1);
        let bz = self.pz - q * gz - k * gz * self.psi;
        uy += gz * bz;
        uw += gz * gz;
        // u = (k/2)g: u·y = hk·(g·y), u·w = hk²·Σg²/D
        let fac = hk * uy / (1.0 + hk * hk * uw);

        // ---- sweep 3: p_new = y − (k/2)·fac·g/D, x_new, the acceleration, g·(p_new + p), and the outputs' sums
        let inv_k = 1.0 / k;
        let s3 = sweep3(
            hk, hk * fac, inv_k, &mut self.x[..n], &mut self.p[..n], &mut self.acc[..n], &self.xh[..n], &self.y[..n], &self.g[..n],
            &model.inv_d_plus[..n], &model.close_w[..n], &model.far_w[..n],
        );
        let (mut gpp, ac, af) = (lane(&s3[0]), lane(&s3[1]), lane(&s3[2]));
        let pzn = bz - hk * fac * gz;
        gpp += gz * (pzn + self.pz);
        self.z = zh + hk * pzn;
        self.pz = pzn;
        self.psi += hk * gpp;
        self.out_acc = (ac * inv_k, af * inv_k);
        self.force_prev = self.force;
        self.force = force;
        // the stick leaves once it has been clear of the head and moving away for a while
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
            self.p[model.slot(i)] += w[i] * impulse;
        }
    }
}

// ---- the sweeps' kernels: slices of equal length, a multiple of four, four partial sums per
// reduction (a reduction in one variable can't be vectorised: floating point isn't associative)

#[inline(always)]
fn sweep1(hk: f64, x: &[f64], p: &[f64], e: &[f64], wb: &[f64], wr: &[f64], xh: &mut [f64]) -> [[f64; 4]; 6] {
    let n = x.len();
    let (p, e, wb, wr, xh) = (&p[..n], &e[..n], &wb[..n], &wr[..n], &mut xh[..n]);
    let mut a = [[0.0f64; 4]; 6];
    let mut i = 0;
    while i < n {
        for l in 0..4 {
            let j = i + l;
            let v = x[j] + hk * p[j];
            xh[j] = v;
            a[0][l] += e[j] * v;
            a[1][l] += e[j] * p[j];
            a[2][l] += wb[j] * v * v;
            a[3][l] += wr[j] * v * v;
            a[4][l] += wb[j] * v * p[j];
            a[5][l] += wr[j] * v * p[j];
        }
        i += 4;
    }
    a
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn sweep2(
    k: f64, coef: f64, tmb: f64, tmr: f64, f: f64, p: &[f64], e: &[f64], wb: &[f64], wr: &[f64], xh: &[f64], dm: &[f64], o2: &[f64], id: &[f64],
    g_out: &mut [f64], y_out: &mut [f64],
) -> ([f64; 4], [f64; 4]) {
    let n = p.len();
    let (e, wb, wr, xh, dm, o2, id, g_out, y_out) = (&e[..n], &wb[..n], &wr[..n], &xh[..n], &dm[..n], &o2[..n], &id[..n], &mut g_out[..n], &mut y_out[..n]);
    let (mut uy, mut uw) = ([0.0f64; 4], [0.0f64; 4]);
    let mut i = 0;
    while i < n {
        for l in 0..4 {
            let j = i + l;
            let g = (tmb * wb[j] + tmr * wr[j]) * xh[j] + f * e[j];
            let y = (dm[j] * p[j] - k * o2[j] * xh[j] - g * coef) * id[j];
            g_out[j] = g;
            y_out[j] = y;
            uy[l] += g * y;
            uw[l] += g * g * id[j];
        }
        i += 4;
    }
    (uy, uw)
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn sweep3(
    hk: f64, hf: f64, inv_k: f64, x: &mut [f64], p: &mut [f64], acc: &mut [f64], xh: &[f64], y: &[f64], g: &[f64], id: &[f64], cw: &[f64], fw: &[f64],
) -> [[f64; 4]; 3] {
    let n = x.len();
    let (p, acc, xh, y, g, id, cw, fw) = (&mut p[..n], &mut acc[..n], &xh[..n], &y[..n], &g[..n], &id[..n], &cw[..n], &fw[..n]);
    let mut a = [[0.0f64; 4]; 3];
    let mut i = 0;
    while i < n {
        for l in 0..4 {
            let j = i + l;
            let pn = y[j] - hf * g[j] * id[j];
            let dp = pn - p[j];
            acc[j] = dp * inv_k;
            a[0][l] += g[j] * (pn + p[j]);
            x[j] = xh[j] + hk * pn;
            p[j] = pn;
            a[1][l] += cw[j] * dp;
            a[2][l] += fw[j] * dp;
        }
        i += 4;
    }
    a
}
