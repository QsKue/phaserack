//! Time-domain stretchers — the "OLA family": window the waveform into grains,
//! realign them, and overlap-add. Cheap, low-latency, FFT-free. These operate
//! directly on the signal (no spectral transform), so they shine on monophonic /
//! voiced material at moderate ratios.
//!
//! See `docs/DECISIONS/0002-stretcher-taxonomy-and-autotune-first-roadmap.md` for
//! how this namespace fits the wider backend taxonomy.

#[cfg(feature = "psola")]
mod psola;
mod wsola;

#[cfg(feature = "psola")]
pub use psola::{PsolaConfig, PsolaMode, PsolaTimeStretcher};
pub use wsola::WsolaTimeStretcher;
