# Decision: warble owns the `TimeStretcher` contract

## Status

Accepted (documents the crate's design as extracted from maestro)

## Context

Time-stretching and pitch-shifting used to live inside the `maestro` audio engine, under
`maestro/src/sources/time_stretchers/`, where the `TimeStretcher` trait returned maestro's own
`AudioLatency` type. As the q-lib audio code split into layers — **prism** (shared primitives /
value types), leaf DSP crates, and the **maestro** engine — this functionality needed a home that:

1. is reusable and standalone-buildable, without dragging in engine/session knowledge;
2. has a clear owner for the `TimeStretcher` trait; and
3. can carry a heavyweight DSP backend without forcing every consumer to compile it.

## Decision

- **Extract into a leaf crate.** Time-stretching/pitch-shifting moves into `warble`, a leaf in the
  audio split. maestro now re-exports warble's types and keeps only a session-aware
  `build_time_stretcher` factory; it drives stretchers through the trait.
- **The leaf owns its trait.** The `TimeStretcher` trait (and `TimeStretcherCapabilities` /
  `TimeStretcherParams` / `TimeStretchProcessResult`) lives **here in warble, not in prism**.
  Per the workspace convention, each leaf owns its own behavioral trait; prism holds only shared
  primitives and value types.
- **Latency via `prism::Latency`.** maestro's `AudioLatency` was folded into `prism::Latency`. The
  trait's `latency()` returns `prism::Latency`, so the engine can sum stretcher delay with the other
  pipeline stages through one shared type.
- **Backend behind a feature.** The trait, the value types, and `NoopTimeStretcher` are
  dependency-free beyond `prism`. The real backend, `SignalSmithTimeStretcher` (over
  `signalsmith-stretch`), sits behind the `signalsmith` feature (on by default) so consumers can use
  the contract + Noop without pulling the DSP dependency.
- **AGPL licensing.** The crate is licensed `AGPL-3.0-or-later`.

## Consequences

- New stretchers fit the existing `TimeStretcher` trait and are added as modules (feature-gated if
  they pull a dependency), rather than by changing the trait.
- warble must stay engine-agnostic and standalone-buildable; it must not regain session/source
  knowledge. maestro remains the hub that orchestrates it.
- The trait's latency surface is tied to `prism::Latency`; changing it is a prism-level decision, not
  a local one.
- Default features pull `signalsmith-stretch` (and its transitive build); a consumer that wants the
  trait + Noop only must build with `--no-default-features`. CI must keep both configurations
  building.
