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
- `with_block_length(sample_rate, channels, block_length) -> Result<Self, String>` — builds with an
  explicit FFT block (interval `block_length / 4`) instead of the preset. See **Pitch-shift accuracy**.
- `format_matches(sample_rate, channels)` — the per-call format guard.

## Pitch-shift accuracy vs FFT block (important)

SignalSmith is a **phase vocoder**: its pitch precision is set by the FFT block via the frequency
resolution `Δf = sample_rate / block`. At **low frequencies** a short block resolves the note coarsely
(few cycles per window), so a pitch *shift undershoots* — the output lands sharp of the target. The
error scales ~inversely with pitch. Measured at 48 kHz, shifting −35¢ and re-detecting (see
`mixrack/tests/shifter_accuracy.rs` and `tests/signalsmith_accuracy.rs`):

| block (samples) | input latency | C3 (~131 Hz) | A4 (440 Hz) |
| --- | --- | --- | --- |
| `preset_default` (~2880) | ~60 ms | **+8…+18¢** | +2…+3¢ |
| 12288 | ~128 ms | +2¢ | +3¢ |
| 16384 | ~171 ms | ~0¢ | +1¢ |

So: **the default preset is fine for typical/high vocal range but undershoots noticeably at low notes
(≈C3)**. It is *not* a SignalSmith defect — it is the inherent STFT resolution/latency trade, and the
preset deliberately favours latency. Use `with_block_length` (≈12–16 k at 48 kHz) when low-note
accuracy matters and the added latency is acceptable. For low notes with *no* latency penalty, the
time-domain **PSOLA** backend lands ~0¢ (see `psola.md`). This was long-standing behaviour (preset has
been used since the crate's first commit); it only became visible once consumers added a detector on
the *output* — and the prior end-to-end tests missed it because they tested at A4 with loose,
relative tolerances.

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
- **Latency.** `latency()` reports `sinerack::Latency::new(input_latency(), output_latency(), 0)` from
  the underlying stretch — input + output delay, no extra processing block.
- **Capabilities are constant `true`** for this backend; it genuinely does independent pitch and
  speed. Don't relax these without changing behavior.
- **Empty/edge inputs** (`input_frames == 0`, `output_capacity_frames == 0`, or a computed
  `consumed_frames == 0`) return a default `TimeStretchProcessResult` without touching the DSP.
