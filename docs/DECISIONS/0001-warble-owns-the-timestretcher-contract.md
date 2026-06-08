# Decision: PhaseRack owns the `TimeStretcher` contract

## Status

Accepted (documents the crate's design as extracted from MixRack)

## Context

Time-stretching and pitch-shifting used to live inside the `mixrack` audio engine, under
`mixrack/src/sources/time_stretchers/`, where the `TimeStretcher` trait returned MixRack's own
`AudioLatency` type. As the q-lib audio code split into layers — **SineRack** (shared primitives /
value types), leaf DSP crates, and the **MixRack** engine — this functionality needed a home that:

1. is reusable and standalone-buildable, without dragging in engine/session knowledge;
2. has a clear owner for the `TimeStretcher` trait; and
3. can carry a heavyweight DSP backend without forcing every consumer to compile it.

## Decision

- **Extract into a leaf crate.** Time-stretching/pitch-shifting moves into `phaserack`, a leaf in the
  audio split. MixRack now re-exports PhaseRack's types and keeps only a session-aware
  `build_time_stretcher` factory; it drives stretchers through the trait.
- **The leaf owns its trait.** The `TimeStretcher` trait (and `TimeStretcherCapabilities` /
  `TimeStretcherParams` / `TimeStretchProcessResult`) lives **here in phaserack, not in sinerack**.
  Per the workspace convention, each leaf owns its own behavioral trait; SineRack holds only shared
  primitives and value types.
- **Latency via `sinerack::Latency`.** MixRack's `AudioLatency` was folded into `sinerack::Latency`. The
  trait's `latency()` returns `sinerack::Latency`, so the engine can sum stretcher delay with the other
  pipeline stages through one shared type.
- **Backend behind a feature.** The trait, the value types, and `NoopTimeStretcher` are
  dependency-free beyond `sinerack`. The real backend, `SignalSmithTimeStretcher` (over
  `signalsmith-stretch`), sits behind the `signalsmith` feature (on by default) so consumers can use
  the contract + Noop without pulling the DSP dependency.
- **AGPL licensing.** The crate is licensed `AGPL-3.0-or-later`.

## Consequences

- New stretchers fit the existing `TimeStretcher` trait and are added as modules (feature-gated if
  they pull a dependency), rather than by changing the trait.
- PhaseRack must stay engine-agnostic and standalone-buildable; it must not regain session/source
  knowledge. MixRack remains the hub that orchestrates it.
- The trait's latency surface is tied to `sinerack::Latency`; changing it is a sinerack-level decision, not
  a local one.
- Default features pull `signalsmith-stretch` (and its transitive build); a consumer that wants the
  trait + Noop only must build with `--no-default-features`. CI must keep both configurations
  building.
