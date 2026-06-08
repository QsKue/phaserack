# Signalsmith backend

Source: `src/signalsmith.rs` (feature `signalsmith`, on by default)

`SignalSmithTimeStretcher` — the real general-purpose **time-stretcher and pitch-shifter**, backed
by `signalsmith-stretch`. It is realtime and advertises full capabilities (realtime, pitch_shift,
time_stretch, independent_pitch_and_speed), so it satisfies `supports_realtime_autotune()`. This is
the only module that pulls a DSP dependency; it sits **behind the `signalsmith` feature** so the
contract + Noop stay dependency-free.

## What belongs here

- The `SignalSmithTimeStretcher` struct (wraps `signalsmith_stretch::Stretch`, plus the bound
  `sample_rate`/`channels`, current `params`, and a `tail_frames_remaining` counter) and its
  `TimeStretcher` impl.
- `new(sample_rate, channels) -> Result<Self, String>` — rejects zero sample rate / zero channels,
  builds the underlying stretch with `preset_default`.
- `format_matches(sample_rate, channels)` — the per-call format guard.

## Patterns to follow

- **Guard first.** `process`/`flush` bail out (empty result / `0`) unless `format_matches` holds and
  `channels != 0`. The backend is built for one fixed `(sample_rate, channels)`; it does not
  reconfigure mid-stream — a caller that changes format must build a new stretcher.
- **Sanitize params.** `set_params` clamps `speed_ratio` to `>= f32::EPSILON` and replaces non-finite
  `speed_ratio`/`pitch_semitones` with `1.0`/`0.0`, then pushes the pitch to
  `set_transpose_factor_semitones`. Keep params finite and positive before they reach the DSP.

## Gotchas

- **Speed / consumed-frames math.** `process` derives the split from `speed_ratio`:
  `consumed_frames = min(input_frames, output_capacity_frames * speed)` and
  `output_frames = round(consumed_frames / speed)` clamped to `[1, output_capacity_frames]`. So at
  `speed > 1` (faster playback) it consumes more input than it writes, and vice versa. This is why
  `process` is partial on both ends — don't assume the whole input buffer is consumed.
- **Tail handling.** After each `process`, `tail_frames_remaining` is set to the stretch's
  `output_latency()`. `flush` then drains it `min(tail, output_capacity)` frames at a time,
  decrementing the counter, and must be called until it returns `0` to get all tail audio. `reset`
  zeroes the counter and resets the underlying stretch.
- **Latency.** `latency()` reports `prism::Latency::new(input_latency(), output_latency(), 0)` from
  the underlying stretch — input + output delay, no extra processing block.
- **Capabilities are constant `true`** for this backend; it genuinely does independent pitch and
  speed. Don't relax these without changing behavior.
- **Empty/edge inputs** (`input_frames == 0`, `output_capacity_frames == 0`, or a computed
  `consumed_frames == 0`) return a default `TimeStretchProcessResult` without touching the DSP.
