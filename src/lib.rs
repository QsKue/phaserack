//! `warble` — time-stretching and pitch-shifting for the q-lib audio engine.
//!
//! A leaf DSP crate in the q-lib audio split: it owns the [`TimeStretcher`]
//! contract (change a signal's length and/or pitch) and its implementations,
//! built on the shared [`prism`] core. The `maestro` engine drives stretchers
//! through this trait; warble has no engine/session knowledge of its own.
//!
//! Stretchers report their delay as a [`prism::Latency`] so the engine can sum
//! it with the other pipeline stages.

mod stretcher;

pub use stretcher::{
    NoopTimeStretcher, TimeStretchProcessResult, TimeStretcher, TimeStretcherCapabilities,
    TimeStretcherParams,
};

#[cfg(feature = "signalsmith")]
mod signalsmith;

#[cfg(feature = "signalsmith")]
pub use signalsmith::SignalSmithTimeStretcher;
