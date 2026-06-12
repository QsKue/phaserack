# Roadmap

The planned direction for `phaserack`. The crate's job is narrow and stable: own the `TimeStretcher`
contract and provide a useful set of stretcher/pitch-shifter backends behind it. Growth means
**adding backends** under the existing trait, not widening the trait.

Backends are bucketed by domain, the same way `pitchrack` groups its detectors —
`time_domain/` (grain overlap-add), `frequency_domain/` (spectral / phase vocoder),
`parametric/` (model resynthesis). A wrapped third-party algorithm (`signalsmith`) stays its own
module, outside the domain folders, because it is code we adapt rather than own.

The priority order is **autotune-first**: build the families that benefit realtime, formant-aware,
monophonic vocal pitch-shifting first, and keep everything else documented for a future
"for completion" version so nothing we learned is lost. The durable rationale — the three-family
taxonomy, the 2026-06 research survey, the namespace convention, and why we wrap external libraries —
is in [ADR 0002](DECISIONS/0002-stretcher-taxonomy-and-autotune-first-roadmap.md).

Keep this file current: check items off as they land (git is the detailed task log). Each backend
documents its capabilities, latency, and gotchas in its `docs/AREAS/*` entry.

---

## Current ✅

- **`TimeStretcher` contract** — the trait, `TimeStretcherCapabilities`, `TimeStretcherParams`,
  `TimeStretchProcessResult`, extracted out of the MixRack engine into this leaf crate (ADR 0001).
  Latency is reported as `sinerack::Latency`.
- **`NoopTimeStretcher`** — pass-through; the default and a test baseline. Dependency-free.
- **`SignalSmithTimeStretcher`** — realtime general-purpose time-stretcher **and** pitch-shifter over
  `signalsmith-stretch`, with independent pitch and speed control, behind the `signalsmith` feature
  (on by default). A *wrapped* third-party backend (adapter pattern): the engine's quality baseline
  and the reference our own backends are A/B'd against. See ADR 0002 §4 for why we wrap.

---

## Active plan — autotune-first

Build in this order; each is a new module implementing `TimeStretcher`, with honest capabilities,
`sinerack::Latency`, its own cargo feature if it pulls a dep, and a `docs/AREAS/*` entry.

1. **WSOLA** → `time_domain/` — ✅ **done (both increments)**
   (`WsolaTimeStretcher`, [docs/AREAS/wsola.md](AREAS/wsola.md)). Waveform Similarity Overlap-Add:
   time-domain, similarity-search-aligned grain overlap-add. Dependency-free (FFT-free), available in
   all builds. Streaming, multichannel.
   - ✅ *Increment 1 — time-stretch core* (pitch-preserving length change).
   - ✅ *Increment 2 — pitch shift via time-stretch-then-resample.* Attach a resampler with
     `with_resampler` (feature `pitch-shift`); WSOLA stretches by `pitch/speed` and the
     **dependency-injected** `samplerack::Resampler` runs at `1/pitch`, giving independent pitch and
     speed (`pitch_shift`/`independent_pitch_and_speed` become `true`). The resampler backend is the
     caller's choice (std vs `no_std` swappable at runtime); `pitch-shift` pulls only samplerack's
     trait, so phaserack stays FFT-free. WSOLA now satisfies `supports_realtime_autotune()`.
2. **PSOLA** → `time_domain/` — ✅ **done** (`PsolaTimeStretcher`, feature `psola`,
   [docs/AREAS/psola.md](AREAS/psola.md)). Pitch-Synchronous Overlap-Add: grains anchored to
   **pitch marks**, so it preserves formants for free — the classic monophonic vocal pitch-shifter,
   and the first backend built entirely on *our* stack (not wrapped). The **pitch-mark contract**
   lives in pitchrack (feature `pitch-marks`, ADR 0008); PSOLA consumes a `Box<dyn PitchMarker>` by
   **injection** (marker is the caller's choice — F0-peak now, GCI next — swappable without a PSOLA
   change), so `psola` pulls only the trait and phaserack stays FFT-free. Synthesis model is
   **parametrized** (`PsolaMode::{Realtime, Offline}`); both share one core. `pitch_shift` /
   `independent_pitch_and_speed` and (in Realtime) `realtime` capabilities are dynamic on the marker.
3. **SMS — sinusoids + residual** → `parametric/` — *ambitious, differentiated; not before 1–2 are
   proven.* Spectral Modelling Synthesis reusing `sinerack` (sinusoids) + `noiserack` (stochastic
   residual) + `pitchrack` (F0): independent pitch / time / formant control by construction. Highest
   voice quality of the planned set, heaviest, bounded by analysis accuracy. Park as its own ADR.

---

## For completion — documented, not scheduled

Real, well-documented techniques kept on record (ADR 0002 §1–2) so a future "completeness" version
can pick them up without re-researching. None is committed; promote one only when a consumer needs
it.

### Time-domain (`time_domain/`)

- **OLA / SOLA** — the simpler ancestors of WSOLA; mostly of reference value once WSOLA exists.

### Frequency-domain (`frequency_domain/`)

- **Phase vocoder (classic + phase-locked)** — STFT analysis/synthesis with phase propagation,
  optionally identity-phase-locked (Laroche–Dolson) to reduce smearing. *Deliberately not planned*
  while `signalsmith` covers this quadrant — re-implementing it is effort without new capability.
- **PGHI / RTPGHI — "Phase Vocoder Done Right"** (Průša & Holighaus,
  https://arxiv.org/pdf/2202.07382). The modern phase-coherence upgrade: integrate STFT phase
  gradients via heap integration, enforcing horizontal+vertical coherence even for broadband content.
  RTPGHI is the streaming variant. The reason to ever own a spectral backend — relevant only if we
  decide to drop the external `signalsmith` dependency.

### Parametric (`parametric/`)

- **Sinusoidal modelling (McAulay–Quatieri)** — the pure-sinusoid ancestor of SMS; the analysis core
  SMS builds on (`sinerack`).
- **WORLD / STRAIGHT-style vocoder** — F0 + spectral envelope + aperiodicity; the highest-quality
  classical voice manipulation, fully formant-decoupled. Heavier analysis than SMS; we already own
  F0 (`pitchrack`).

### Neural — far future

- **Neural / DDSP vocoders** — CLPCNet (https://arxiv.org/pdf/2110.02360) and 2024–2025
  differentiable-DSP vocoders (https://www2.eecs.berkeley.edu/Pubs/TechRpts/2024/EECS-2024-202.pdf),
  some already realtime on adequate hardware. Best voice quality. A neural backend would implement
  `TimeStretcher` like any other and declare its own `realtime` capability for the hardware it needs
  (CPU/GPU/NPU). Far-future, not excluded: the blocker is the heavy runtime + model-weights
  dependency (much larger than `signalsmith`) and a quality-vs-effort tradeoff that is not there yet.

---

Whatever lands, it implements `TimeStretcher` like the existing backends, advertises honest
`TimeStretcherCapabilities`, reports its `sinerack::Latency`, and gets a `docs/AREAS/*` entry. The
trait is expected to stay as-is; revisit it only if a real backend needs a capability it cannot
express (ADR 0001).
