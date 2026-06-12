# Decision: stretcher backend taxonomy, namespaces, and an autotune-first roadmap

## Status

Accepted (2026-06-12) — sets the structure and priorities for growing phaserack's backend set.

## Context

phaserack owns the `TimeStretcher` contract (ADR 0001) and today ships two implementations:
`NoopTimeStretcher` (baseline) and `SignalSmithTimeStretcher` (a wrapped third-party backend). The
driving consumer is **mixrack autotune**, which needs realtime, low-latency, formant-aware pitch
shifting on monophonic vocals, with independent pitch and time.

We now have strong *analysis* crates in the audio split — `pitchrack` (F0 + pitch detectors),
`sinerack` (sinusoidal core / primitives), `noiserack` (noise modelling) — that change which
stretcher families are cheap for us to build. Before adding backends ad hoc, we want a documented
taxonomy, a namespace convention, and a priority order anchored to autotune. We also surveyed recent
literature so the plan reflects current best practice, not just textbook methods.

This ADR records the taxonomy, the recent-research findings, the namespace decision, the
"wrap an external library" rationale, and the prioritized roadmap. The checklist itself lives in
`docs/ROADMAP.md`; this ADR is the durable *why*.

## Decision

### 1. Three families, one synthesis mechanism

Time-scale modification (TSM) and pitch-shifting backends fall into three families. The first two
both *end* in **overlap-add (OLA)** — that is a synthesis mechanism, not a family. The real split is
**manipulate the signal vs. manipulate a model of it**:

- **Time-domain (the narrow "OLA family").** Window the waveform into grains, realign them, overlap-
  add. OLA → SOLA → WSOLA (cross-correlation aligned) → PSOLA / TD-PSOLA (aligned to *pitch marks*).
  Cheap, low-latency, FFT-free. Strong on monophonic voiced material; degrades on polyphony and
  transients. PSOLA preserves formants *for free* (it never warps the spectral envelope) — exactly
  what vocals want — but needs pitch marks, which `pitchrack` provides.
- **Frequency-domain (phase vocoder).** STFT → modify magnitude/phase → iSTFT (the iSTFT *is* an
  overlap-add of windowed frames). Flexible across any ratio, handles polyphony, independent
  pitch/time. Costs latency, compute, and "phasiness" on transients. **`signalsmith` already covers
  this quadrant well.**
- **Parametric / model-based.** Analyse → parameters → resynthesize (no grain overlap-add at all):
  sinusoidal modelling (McAulay–Quatieri) → SMS (sinusoids + stochastic residual) → harmonic+noise →
  WORLD/STRAIGHT (F0 + spectral envelope + aperiodicity). Highest quality for *voice* and the most
  flexible (pitch, time, and formants are independent parameters). Heaviest, and quality is bounded
  by analysis accuracy. **SMS is literally `sinerack` + `noiserack` + `pitchrack`** — the family that
  is uniquely cheap for *us* to build because we already own the analysis halves.

**Formant preservation** is a cross-cutting concern, not a family: for vocals, pitch must be
decoupled from the spectral envelope (or pitched-up vocals sound like chipmunks). Phase vocoder needs
explicit envelope estimation (cepstral/LPC) + re-warp; PSOLA and the parametric models get it largely
for free. For autotune this is a hard requirement.

### 2. Recent-research findings (2026-06 survey)

- **"Phase Vocoder Done Right" / (RT)PGHI** (Průša & Holighaus). The one genuine DSP advance in the
  spectral family: estimate both partial derivatives of the STFT phase and integrate them with a
  heap-based algorithm (PGHI), enforcing horizontal *and* vertical phase coherence automatically —
  even for broadband, non-sinusoidal content. **RTPGHI** is the streaming variant. This is the modern
  upgrade path *if* we ever build our own spectral backend, and the reason "phase vocoder" is not a
  fully solved black box. https://arxiv.org/pdf/2202.07382
- **Rubber Band R3 "Finer" engine** — open-source quality bar for general material (better than R2 on
  vocals/soft-onsets/bass, ~3× the CPU). `signalsmith` is considered comparable and is MIT-licensed,
  which is why we chose it. https://github.com/breakfastquay/rubberband
- **Neural / DDSP vocoders** — CLPCNet (controllable LPCNet: pitch-shift + time-stretch of speech at
  DSP-meeting/exceeding quality) and 2024–2025 *differentiable-DSP* vocoders, some already realtime
  on adequate hardware. Highest voice quality. **Far-future, not out of scope:** a neural backend
  would implement `TimeStretcher` like any other and declare its own `realtime` capability based on
  the hardware it needs (CPU/GPU/NPU) — it is not excluded for being "not realtime." The reason it is
  far-future is the heavy runtime + model-weights dependency (much larger than `signalsmith`) and a
  quality-vs-effort tradeoff that is not there yet, not a fundamental misfit.
  https://arxiv.org/pdf/2110.02360 · https://www2.eecs.berkeley.edu/Pubs/TechRpts/2024/EECS-2024-202.pdf

**Net:** the only finding that changes the build plan is PGHI/RTPGHI. WSOLA, PSOLA, and SMS are
mature and stable — no recent shake-up; we implement them from established literature.

### 3. Namespaces mirror pitchrack

pitchrack groups its detectors by domain (`time_domain/`, `frequency_domain/`, plus `tracking/`,
`utils/`). phaserack adopts the same convention so the two crates read the same way:

```text
phaserack/src/
├── stretcher.rs        # the TimeStretcher contract (root, like pitchrack's detector.rs)
├── time_domain/        # OLA, SOLA, WSOLA, PSOLA           (grain overlap-add)
├── frequency_domain/   # phase vocoder, PGHI / "done right" (spectral overlap-add)
├── parametric/         # sinusoidal (MQ), SMS, WORLD-style  (model resynthesis; reuses sinerack/noiserack/pitchrack)
└── signalsmith.rs      # wrapped external backend — stays its own module, NOT in a domain folder
```

Our *own* DSP is bucketed by how it works. `signalsmith` is a wrapped third-party **algorithm**, not
our code, so it stays a top-level module (a `backends/` / `external/` folder only if more wrapped
libraries land). Domain folders mean "DSP we own, grouped by mechanism."

### 4. Why we wrap external libraries (the adapter rationale)

`SignalSmithTimeStretcher` is the **only** place in the leaf DSP crates where we adapt an entire
third-party *algorithm* behind one of our traits (pitchrack/sinerack pull `rustfft`, but that is a
*primitive* they build their own detectors on, not a wrapped algorithm; mixrack wraps *infrastructure*
— cpal/symphonia/rubato/egui — which is a different concern). We wrap signalsmith because it:

1. gives the engine **one interface** (`TimeStretcher`) regardless of backend, so backends can be
   swapped/added without touching mixrack;
2. provides a **working, high-quality baseline today**, and the **reference to A/B** our own
   implementations against;
3. lets us **feature-gate the heavy dependency** so the trait + Noop stay dependency-free; and
4. **normalizes** the backend's latency into `sinerack::Latency` for whole-pipeline summing.

Growing phaserack means *adding* domain-bucketed backends of our own under the same trait — and over
time, being able to replace signalsmith for the autotune path with code we own — not widening the
trait.

### 5. Autotune-first priority order

Build the families that benefit autotune first; keep the rest documented for a future "for
completion" version so nothing learned is lost.

1. **WSOLA** (`time_domain/`) — small, FFT-free, low-latency, no new deps. The missing lightweight
   time-domain backend and a non-trivial second implementation to validate the trait. Warm-up.
2. **PSOLA** (`time_domain/`) — the high-value autotune backend: pairs with pitchrack's detectors,
   preserves formants for free, the classic monophonic vocal pitch-shifter. First point where our
   own stack differentiates from "we wrapped signalsmith." The pitch-mark interface between pitchrack
   and phaserack is the key design question.
3. **SMS — sinusoids + residual** (`parametric/`) — the ambitious, differentiated play reusing
   sinerack + noiserack + pitchrack. Park as its own ADR; do not start until WSOLA/PSOLA are proven.

Explicitly **not** planned: re-implementing a phase vocoder from scratch — signalsmith owns that
quadrant; our own would be effort without new capability *unless* we adopt PGHI/RTPGHI, which is
itself only worth it if we need to drop the external dependency. Neural/DDSP backends are far-future
(heavy runtime + model-weights dependency), not excluded — phaserack is the home for *anything* that
changes pitch or speed, offline or realtime, and each backend declares its own `realtime` capability.

## Consequences

- New backends land as modules under `time_domain/`, `frequency_domain/`, or `parametric/`, each
  implementing `TimeStretcher`, advertising honest `TimeStretcherCapabilities`, reporting
  `sinerack::Latency`, gated behind its own cargo feature if it pulls a dependency, and documented in
  a `docs/AREAS/*` entry. The trait does not widen for any one of them (ADR 0001 holds).
- PSOLA and SMS create a **cross-crate coupling**: phaserack will consume pitch marks / F0 /
  sinusoidal+residual analysis from pitchrack/sinerack/noiserack. The pitch-mark contract gets its
  own ADR when PSOLA starts.
- `signalsmith` remains the default backend and the quality baseline; the roadmap's own backends are
  measured against it before they replace it on any path.
- The PGHI/RTPGHI and neural findings are recorded here so a future "for completion" effort can pick
  them up without re-researching; they are deliberately not scheduled.
