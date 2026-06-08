# The TimeStretcher contract

Source: `src/stretcher.rs`

The crate's public contract: the `TimeStretcher` trait, its value types, and the pass-through
`NoopTimeStretcher`. This module is **dependency-free beyond `sinerack::Latency`** — no DSP, no backend
code — so a consumer can use the contract and the Noop stretcher without the `signalsmith` feature.
Backends (e.g. `signalsmith.rs`) implement this trait; they live in their own modules.

## What belongs here

- **`TimeStretcher: Send`** — the trait every stretcher implements. Operates on **interleaved `f32`**:
  `process` (transform input into a separate output buffer), `flush` (drain the tail), `reset`,
  `latency` (`-> sinerack::Latency`), `capabilities`, `set_params`, `params`.
- **`TimeStretcherCapabilities`** — `realtime`, `pitch_shift`, `time_stretch`,
  `independent_pitch_and_speed`. `noop()` is the pass-through profile (realtime, no transform).
  `supports_realtime_autotune()` = `realtime && pitch_shift && independent_pitch_and_speed` — the
  capability the engine queries to choose an autotune-capable stretcher.
- **`TimeStretcherParams`** — `speed_ratio` (1.0 = unchanged) and `pitch_semitones` (0.0 = unchanged);
  `Default` is the identity transform.
- **`TimeStretchProcessResult`** — `input_frames_consumed` + `output_frames_written` from one
  `process` call. `Default` (0, 0) is the "did nothing" result the guards return.
- **`NoopTimeStretcher`** — copies `min(input_frames, output_frames)` straight through, no tail, zero
  latency, `noop()` capabilities. The default and a test baseline.

## Patterns to follow

- **A new backend = a new module implementing this trait.** Don't widen the trait to suit one
  backend. If the backend pulls a dependency, gate it behind its own cargo feature (as `signalsmith`
  is) and re-export from `lib.rs`.
- **Frame counts come from `len / channels`.** Inputs and outputs are interleaved; one frame is
  `channels` samples. Return how much you actually consumed/produced in `TimeStretchProcessResult` —
  callers loop on it.
- **Guard `channels == 0`** at every entry point (Noop already returns a default result for it) so the
  `len / channels` division is safe.
- **Validate before transforming finite inputs.** Backends that hold a fixed format should reject a
  mismatched `(sample_rate, channels)` rather than process it (see `signalsmith.md`); `set_params`
  should sanitize non-finite values.

## What should not be placed here

- Backend/DSP code or any `signalsmith-stretch` reference — that lives in `src/signalsmith.rs` behind
  the feature. This module must keep building with the `signalsmith` feature off.
- A local latency type. `latency()` returns `sinerack::Latency`; don't reintroduce MixRack's old
  `AudioLatency` (it was folded into `sinerack::Latency` — see ADR 0001).
- Engine/session concepts.

## Gotchas

- `process` is **partial on both ends**: a backend may consume fewer input frames *and* write fewer
  output frames than the buffers hold. Code against `TimeStretchProcessResult`; never assume a call
  drained the input or filled the output.
- Tail audio comes out of `flush`, not `process`. `flush` returns the frame count written and should
  be called until it returns `0`.
- `capabilities()` must stay truthful — the engine selects a stretcher from these flags
  (`supports_realtime_autotune`). A backend that advertises a capability it doesn't deliver will be
  mis-selected.
