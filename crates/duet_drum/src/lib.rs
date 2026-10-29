//! `duet_drum`: a physically modelled drum, by nonlinear modal synthesis.
//!
//! The linear physics (membranes, air loading, the cavity, uneven lug tension)
//! is solved once, at prepare time, into a diagonal modal basis. The nonlinear
//! physics (tension modulation, the stick's contact) is integrated with an
//! explicit, energy-stable scalar auxiliary variable scheme, O(N) per sample.
//!
//! Built stage by stage (see the spec): stage 1 is the Bessel functions and the
//! in-vacuo membrane mode basis; stage 2 the linear single head with the exact
//! modal discretisation.

pub mod analysis;
pub mod bessel;
pub mod modes;
pub mod prepare;
pub mod params;
pub mod quad;
pub mod voice;

pub use modes::{MembraneBasis, MembraneMode, MembraneParams, RADIAL_POINTS};
pub use params::{DrumParams, Head, HeadParams, MicParams, StickParams};
pub use prepare::{DrumModel, ModeInfo, PrepareError};
pub use voice::DrumVoice;
