# Architecture

This document describes the structure of the `warble` crate. Keep it aligned with `AGENTS.md` and
update it when the `TimeStretcher` contract, module boundaries, or data flow change.

## Shape

`warble` is a single library crate with a tiny module tree:

```text
lib.rs            crate root: module declarations + re-exports
├── stretcher.rs  the TimeStretcher trait + value types + NoopTimeStretcher (the contract)
└── signalsmith.rs  SignalSmithTimeStretcher — the real backend (feature "signalsmith")
```

The contract (`stretcher.rs`) is the public surface every consumer codes against; backends are
implementations of it. There is no engine, no session, no async, no I/O. The only unconditional
dependency is **prism** (for `Latency`); the backend adds `signalsmith-stretch`, but only when the
`signalsmith` feature is on. warble is the leaf; the `maestro` engine drives it through the trait and
is the hub for the wider audio docs.

## Public API

The public contract is intentionally small:

- `TimeStretcher: Send` — the trait. A stretcher operates on interleaved `f32`:
  ```rust
  fn process(&mut self, input: &[f32], output: &mut [f32],
             sample_rate: u32, channels: usize) -> TimeStretchProcessResult;
  fn flush(&mut self, output: &mut [f32], sample_rate: u32, channels: usize) -> usize;
  fn reset(&mut self);
  fn latency(&self) -> prism::Latency;
  fn capabilities(&self) -> TimeStretcherCapabilities;
  fn set_params(&mut self, params: TimeStretcherParams);
  fn params(&self) -> TimeStretcherParams;
  ```
- `TimeStretcherCapabilities { realtime, pitch_shift, time_stretch, independent_pitch_and_speed }`
  — what a stretcher can do, plus `noop()` and `supports_realtime_autotune()`.
- `TimeStretcherParams { speed_ratio, pitch_semitones }` — the requested transform (`Default` =
  1.0 / 0.0, i.e. unchanged).
- `TimeStretchProcessResult { input_frames_consumed, output_frames_written }` — what one `process`
  call actually did.
- `NoopTimeStretcher` — pass-through implementation; the default and a test baseline.
- `SignalSmithTimeStretcher` (feature `signalsmith`) — the realtime backend over
  `signalsmith-stretch`.

Adding a backend should not change this trait: add a new module with a struct that implements
`TimeStretcher` (gated behind its own feature if it pulls a dependency) and re-export it from
`lib.rs`.

## Data flow

A consumer drives a stretcher per audio block:

```text
set_params(speed_ratio, pitch_semitones)        // configure the transform
loop over input blocks:
  process(input, output, sample_rate, channels)
    -> guard: format_matches(sample_rate, channels) && channels != 0  (else empty result)
    -> derive input_frames / output_capacity_frames from len / channels
    -> backend transforms a partial slice, returns frames consumed + written
  (consume `input_frames_consumed`, emit `output_frames_written`; repeat)
after the last input block:
  flush(output, ...) -> writes tail frames, returns count; repeat until it returns 0
latency() -> prism::Latency, summed by the engine across pipeline stages
reset() between independent streams
```

`process` is **partial on both ends** — it may consume fewer input frames and write fewer output
frames than the buffers hold (see `TimeStretchProcessResult`). The signalsmith backend sizes the
consumed/written split from `speed_ratio` and tracks a tail (`output_latency()`) that `flush` then
drains. `NoopTimeStretcher` copies `min(input, output)` frames straight through with no tail.

## Key design properties

- **One uniform trait.** Every stretcher implements `TimeStretcher`; backends are added as modules,
  not by widening the trait for one of them. The trait lives **here in the leaf**, not in prism —
  see ADR 0001.
- **Latency via prism.** `latency()` returns `prism::Latency` so the engine can add it to the other
  stages. warble does not define its own latency type.
- **Feature-gated backend.** The trait, the value types, and `NoopTimeStretcher` are dependency-free
  beyond `prism`. The real DSP backend sits behind the `signalsmith` feature (on by default) so a
  consumer can use the contract + Noop without pulling `signalsmith-stretch`.
- **Engine-agnostic.** No session/source/routing concepts. warble transforms a caller-provided
  interleaved buffer; the engine owns scheduling and policy.

## Testing & checks

```bash
cargo build                                # default features (pulls the signalsmith backend)
cargo fmt --all --check                    # formatting
cargo clippy --all-targets -- -D warnings  # lints (CI gates on this)
cargo test                                 # tests
cargo build --no-default-features          # contract + Noop only, no backend
```

CI (`.github/workflows/main.yaml`) runs the fmt / clippy `-D warnings` / build / test gate on stable
(with default features, so the `signalsmith` backend is built) plus an MSRV check.

## Documentation coupling

When you change a module's responsibility, update the matching `docs/AREAS/*` file; when you change
the `TimeStretcher` contract, a result type's semantics, or the feature gate, update this document
and add a `docs/DECISIONS/` ADR if the choice is durable. See `AGENTS.md` for the full
docs-maintenance policy.
