# Roadmap

The planned direction for `phaserack`. The crate's job is narrow and stable: own the `TimeStretcher`
contract and provide a useful set of stretcher/pitch-shifter backends behind it. Growth means
**adding backends** under the existing trait, not widening the trait.

Keep this file current: check items off as they land (git is the detailed task log). Each backend
should document its capabilities, latency characteristics, and gotchas in its `docs/AREAS/*` entry.

---

## Current ✅

- **`TimeStretcher` contract** — the trait, `TimeStretcherCapabilities`, `TimeStretcherParams`,
  `TimeStretchProcessResult`, extracted out of the MixRack engine into this leaf crate (ADR 0001).
  Latency is reported as `sinerack::Latency`.
- **`NoopTimeStretcher`** — pass-through; the default and a test baseline. Dependency-free.
- **`SignalSmithTimeStretcher`** — realtime general-purpose time-stretcher **and** pitch-shifter over
  `signalsmith-stretch`, with independent pitch and speed control, behind the `signalsmith` feature
  (on by default).

---

## Plausible future backends (speculative — not scheduled)

> Everything below is **speculative**. These are real, well-documented techniques that *could* land
> as additional `TimeStretcher` backends if a concrete need appears; none is committed. Promote one
> into the active list only when a consumer actually needs it. Each would be a new module, likely
> behind its own cargo feature so minimal builds stay minimal.

A family of classic time-scale-modification backends, roughly lightest → heaviest:

- **WSOLA** (Waveform Similarity Overlap-Add) — time-domain, cross-correlation-aligned overlap-add.
  Cheap and low-latency; good for moderate stretch ratios on monophonic/voiced material. The natural
  FFT-free counterpart to the spectral methods.
- **Phase vocoder** — STFT analysis/synthesis with phase propagation (optionally phase-locked /
  identity-phase-locked to reduce smearing). The reference spectral stretcher; more flexible across
  ratios than WSOLA, at higher compute and latency cost.
- **PSOLA** (Pitch-Synchronous Overlap-Add) — overlap-add anchored to detected pitch marks; strong
  on monophonic voice, but needs pitch marking (would pair with a detector from `pitchrack`).

If any of these lands, it implements `TimeStretcher` like the existing backends, advertises honest
`TimeStretcherCapabilities`, reports its `sinerack::Latency`, and gets a `docs/AREAS/*` entry. The trait
is expected to stay as-is; revisit it only if a real backend needs a capability it cannot express.
