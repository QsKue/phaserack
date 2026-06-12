//! `phaserack` — time-stretching and pitch-shifting for the q-lib audio engine.
//!
//! A leaf DSP crate in the q-lib audio split: it owns the [`TimeStretcher`]
//! contract (change a signal's length and/or pitch) and its implementations,
//! built on the shared [`sinerack`] core. The `mixrack` engine drives stretchers
//! through this trait; phaserack has no engine/session knowledge of its own.
//!
//! Stretchers report their delay as a [`sinerack::Latency`] so the engine can sum
//! it with the other pipeline stages.

mod stretcher;
mod time_domain;

pub use stretcher::{
    NoopTimeStretcher, TimeStretchProcessResult, TimeStretcher, TimeStretcherCapabilities,
    TimeStretcherParams,
};
pub use time_domain::WsolaTimeStretcher;

#[cfg(feature = "psola")]
pub use time_domain::{PsolaConfig, PsolaMode, PsolaTimeStretcher};

#[cfg(feature = "signalsmith")]
mod signalsmith;

#[cfg(feature = "signalsmith")]
pub use signalsmith::SignalSmithTimeStretcher;
