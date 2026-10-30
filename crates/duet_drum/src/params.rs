//! The drum's parameters (SI units throughout) and their defaults: a 16"×16"
//! floor tom, a single-ply 10 mil batter head over a 7.5 mil resonant head.

use serde::{Deserialize, Serialize};

use crate::bessel::bessel_zero;

/// Which head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Head {
    Batter,
    Resonant,
}

/// What differs from one head to the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadParams {
    /// film thickness h (m)
    pub thickness: f64,
    /// nominal tension T: force per unit edge length (N/m)
    pub tension: f64,
    /// each lug's deviation from the nominal tension, relative (+0.02 = 2 % tighter);
    /// one per lug, or empty for none
    pub lug_deviations: Vec<f64>,
    /// the highest mode kept (Hz); never above 0.45·fs
    pub f_max: f64,
    /// internal damping σ = σ0 + σ1·Ω² (amplitude decay rate, 1/s): σ0 (1/s)
    pub sigma0: f64,
    /// ... σ1 (s)
    pub sigma1: f64,
}

impl HeadParams {
    pub fn batter() -> Self {
        HeadParams { thickness: 0.254e-3, tension: 1500.0, lug_deviations: vec![0.0; 8], f_max: 2500.0, sigma0: 3.0, sigma1: 6.0e-8 }
    }
    pub fn resonant() -> Self {
        HeadParams { thickness: 0.190e-3, tension: 1600.0, lug_deviations: vec![0.0; 8], f_max: 1500.0, sigma0: 3.0, sigma1: 6.0e-8 }
    }
}

/// The stick (a lumped mass) and its contact with the head.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StickParams {
    /// effective mass m_s (kg)
    pub mass: f64,
    /// contact stiffness K_s: F = K_s·[δ]₊^α. The spec's 1e8 is too hard to resolve at audio rates
    /// (the stick chatters, and the force depends on the sample rate); 1e6 is a soft tip and converges
    /// (stage-3 report)
    pub stiffness: f64,
    /// contact exponent α
    pub exponent: f64,
    /// strike speed at velocity 0 and 1 (m/s), and the curve between:
    /// v0 = v_min + (v_max − v_min)·vel^γ_v
    pub v_min: f64,
    pub v_max: f64,
    pub velocity_gamma: f64,
}

/// The two virtual microphones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicParams {
    /// close mic: radius over the batter, as a fraction of the shell radius R
    pub close_radius: f64,
    /// close mic: angle (rad)
    pub close_angle: f64,
    /// close mic: height above the batter (m)
    pub close_height: f64,
    /// far mic: distance (m)
    pub far_distance: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrumParams {
    /// shell radius R (m)
    pub shell_radius: f64,
    /// shell depth L (m); the cavity is πR²L
    pub shell_depth: f64,
    pub batter: HeadParams,
    pub resonant: HeadParams,
    /// film density ρ_f (kg/m³)
    pub film_density: f64,
    /// Young's modulus E (Pa)
    pub youngs_modulus: f64,
    /// Poisson ratio ν
    pub poisson_ratio: f64,
    /// lugs per head
    pub lug_count: usize,
    /// angle of the first lug (rad); lug l sits at θ_lug0 + 2πl/N_lug
    pub lug_angle_offset: f64,
    /// von Mises concentration of each lug's bump of tension
    pub lug_sharpness: f64,
    /// air density ρ_a (kg/m³)
    pub air_density: f64,
    /// speed of sound c_a (m/s)
    pub speed_of_sound: f64,
    /// air-mass factor β_air (2 = air on both faces)
    pub air_mass_factor: f64,
    /// multiplies the cavity stiffness K_cav (for calibration)
    pub cavity_scale: f64,
    /// multiplies the tension-modulation potential (for calibration)
    pub nonlinearity_scale: f64,
    /// radiation damping on, and its scale γ_rad
    pub radiation_on: bool,
    pub radiation_gain: f64,
    /// prepare fails if a head would need more modes than this
    pub n_max_per_head: usize,
    pub stick: StickParams,
    pub mic: MicParams,
    /// gain of the contact force's dipole term in the close mic
    pub attack_gain: f64,
    /// EXTENSION (not in the spec): radius a (m) of the stick's contact patch: the strike vector is weighted by
    /// `exp(−(κa)²/2)` (a Gaussian patch instead of a point), so the stick hardly drives the highest kept modes.
    /// Without it the contact force ripples at ~3 kHz (the stick against the top of the modal band): a buzz at
    /// every strike. 0 = a point (the spec).
    pub contact_radius: f64,
    /// EXTENSION (not in the spec): a 4-pole low-pass (Hz) on the contact force before the close mic's
    /// dipole term differentiates it. The spec's raw `(F_n − F_{n−1})/k` makes a high-pitched click at
    /// every strike: the force rises in a fraction of a millisecond and then ripples at ~3 kHz (the stick against the
    /// highest kept modes), which the derivative turns into a buzz.
    /// 0 = off (the spec).
    pub attack_lowpass_hz: f64,
    /// SAV gauge ε (J)
    pub sav_epsilon: f64,
    /// SAV drift control λ (0 = off)
    pub sav_drift: f64,
    /// output gains (dB): velocity 1.0 at the centre peaks near −6 dBFS
    pub gain_close_db: f64,
    pub gain_far_db: f64,
}

impl Default for DrumParams {
    fn default() -> Self {
        DrumParams {
            shell_radius: 0.2032,
            shell_depth: 0.4064,
            batter: HeadParams::batter(),
            resonant: HeadParams::resonant(),
            film_density: 1390.0,
            youngs_modulus: 4.0e9,
            poisson_ratio: 0.38,
            lug_count: 8,
            lug_angle_offset: std::f64::consts::PI / 8.0,
            lug_sharpness: 4.0,
            air_density: 1.2,
            speed_of_sound: 343.0,
            air_mass_factor: 2.0,
            cavity_scale: 1.0,
            nonlinearity_scale: 1.0,
            radiation_on: true,
            radiation_gain: 1.0,
            n_max_per_head: 1200,
            stick: StickParams { mass: 0.02, stiffness: 1.0e6, exponent: 1.5, v_min: 0.2, v_max: 6.0, velocity_gamma: 1.5 },
            mic: MicParams { close_radius: 0.5, close_angle: 0.0, close_height: 0.06, far_distance: 1.5 },
            attack_gain: 1.0,
            contact_radius: 0.010,
            attack_lowpass_hz: 800.0,
            sav_epsilon: 1e-12,
            sav_drift: 0.0,
            // (calibrate_gains: a velocity-1.0 hit at the centre peaks at −6 dBFS on both outputs, at
            // 44.1, 48 and 96 kHz to within 0.3 dB)
            gain_close_db: -45.3,
            gain_far_db: -30.9,
        }
    }
}

/// How many modes to keep: the cost of a voice goes with the count (stage-8 report). The spec's own
/// cut-offs are `High`; the lower two trade the top of the band for speed (the waveform differs
/// more, the pitch glide and the low modes, which carry the tone, don't).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// f_max 800 / 600 Hz (batter / resonant): ~80 modes, about 18× real time
    Draft,
    /// f_max 1800 / 1200 Hz: ~370 modes, about 4× real time
    Normal,
    /// f_max 2500 / 1500 Hz (the spec): ~630 modes, about 2× real time
    High,
}

impl DrumParams {
    /// The defaults with the mode cut-offs of a quality level.
    pub fn with_quality(q: Quality) -> Self {
        let mut p = Self::default();
        let (b, r) = match q {
            Quality::Draft => (800.0, 600.0),
            Quality::Normal => (1800.0, 1200.0),
            Quality::High => (2500.0, 1500.0),
        };
        p.batter.f_max = b;
        p.resonant.f_max = r;
        p
    }

    pub fn head(&self, head: Head) -> &HeadParams {
        match head {
            Head::Batter => &self.batter,
            Head::Resonant => &self.resonant,
        }
    }

    pub fn head_mut(&mut self, head: Head) -> &mut HeadParams {
        match head {
            Head::Batter => &mut self.batter,
            Head::Resonant => &mut self.resonant,
        }
    }

    /// Head area A = πR² (m²).
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.shell_radius * self.shell_radius
    }

    /// Cavity volume πR²L (m³).
    pub fn cavity_volume(&self) -> f64 {
        self.area() * self.shell_depth
    }

    /// σ_h = ρ_f·h (kg/m²).
    pub fn surface_density(&self, head: Head) -> f64 {
        self.film_density * self.head(head).thickness
    }

    /// D_h = E·h³ / (12(1−ν²)) (N·m).
    pub fn bending_stiffness(&self, head: Head) -> f64 {
        let h = self.head(head).thickness;
        self.youngs_modulus * h * h * h / (12.0 * (1.0 - self.poisson_ratio * self.poisson_ratio))
    }

    /// The tension that puts the head's lowest (1,1) pair at `f_hz` once the air
    /// is loading it:
    ///
    /// `T = σ_h·((2πf)²(1+μ_11) − (D_h/σ_h)·κ_11⁴) / κ_11²`
    ///
    /// This ignores the lug mean shift (c_0, §4.5), and the cavity (it doesn't touch
    /// the (1,1) pair).
    pub fn tension_for_pitch(&self, head: Head, f_hz: f64) -> f64 {
        let sigma = self.surface_density(head);
        let d = self.bending_stiffness(head);
        let kappa = bessel_zero(1, 1) / self.shell_radius;
        let mu = self.air_mass_factor * self.air_density / (sigma * kappa);
        let w = 2.0 * std::f64::consts::PI * f_hz;
        sigma * (w * w * (1.0 + mu) - d / sigma * kappa.powi(4)) / (kappa * kappa)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("params serialise")
    }

    /// Reads parameters from JSON; anything left out keeps its default (also inside
    /// `batter`, `resonant`, `stick`, `mic`). Unknown names are an error.
    pub fn from_json(s: &str) -> Result<Self, String> {
        let patch: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let mut base = serde_json::to_value(Self::default()).map_err(|e| e.to_string())?;
        merge(&mut base, patch);
        serde_json::from_value(base).map_err(|e| e.to_string())
    }
}

/// Objects merge key by key; anything else (numbers, arrays) is replaced.
fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v); // (unknown: the strict read says so)
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_partial_json_keeps_the_rest() {
        let d = DrumParams::default();
        assert_eq!(DrumParams::from_json(&d.to_json()).unwrap(), d);
        let p = DrumParams::from_json(r#"{"resonant": {"tension": 1400}, "stick": {"mass": 0.03}}"#).unwrap();
        assert_eq!(p.resonant.tension, 1400.0);
        assert_eq!(p.resonant.thickness, 0.190e-3, "the resonant head's own default, not the batter's");
        assert_eq!(p.batter, d.batter);
        assert_eq!(p.stick.mass, 0.03);
        assert_eq!(p.stick.stiffness, 1.0e6);
        assert!(DrumParams::from_json(r#"{"shel_radius": 1}"#).is_err(), "typos are caught");
    }
}
