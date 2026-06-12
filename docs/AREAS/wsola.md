# WSOLA backend (`time_domain/`)

Source: `src/time_domain/wsola.rs` — `WsolaTimeStretcher`. Dependency-free (FFT-free
grain math), so it is available in **all** builds, including `--no-default-features`
(unlike the signalsmith backend, which is feature-gated).

`WsolaTimeStretcher` — a **Waveform Similarity Overlap-Add** time-domain TSM backend. It
changes a signal's **length** (playback speed) while preserving pitch, by overlap-adding
windowed grains taken from the input at an analysis hop that differs from the synthesis hop.
The "waveform similarity" step is a short tolerance search around each analysis point that
picks the grain best continuing the previous one, keeping the overlap-add phase-coherent and
avoiding plain-OLA buzz.

## Capabilities

Honest `TimeStretcherCapabilities`: `realtime: true`, `time_stretch: true`,
**`pitch_shift: false`**, `independent_pitch_and_speed: false`. It does **not** satisfy
`supports_realtime_autotune()` — that needs pitch shift. Pitch shifting on this family is
time-stretch-then-resample and is a planned later increment (see ROADMAP / ADR 0002).

## How it works (the streaming model)

- **Fixed grain geometry**, derived once from the sample rate: synth hop `Hs` ≈ 15 ms, grain
  `N = 2*Hs` (~30 ms), Hann window (50% overlap → constant-amplitude overlap-add), similarity
  search radius `Δ` ≈ ±12 ms (covers down to ~80 Hz). Minimums are clamped for tiny rates.
- **Analysis hop `Ha = Hs * speed_ratio`.** At `speed > 1` (faster playback) the analysis
  pointer advances faster than synthesis → shorter output; at `speed < 1` → longer output. The
  synthesis hop is fixed, so amplitude is preserved by the Hann COLA property.
- **Internal buffering.** `process` takes the whole input block into `in_buf` (so it always
  reports `input_frames_consumed == input_frames`), forms as many grains as the output buffer
  and the buffered input allow, and drains finalized frames into `output`
  (`output_frames_written`). Leftover finalized audio is held in `out_hold` for the next call;
  spent input is trimmed from the front of `in_buf`.
- **Similarity search.** Each grain's leading `Hs` (mono) is matched (min squared error,
  early-out) against the *natural continuation* target — what would follow the previously
  chosen grain by one `Hs`. The first grain is taken at `delta = 0` (nothing to match yet).
- **Multichannel.** The match is computed on a mono mix and the **same** delta is applied to
  all channels, so they stay phase-aligned. State (`tail`, `out_hold`) is interleaved.

## Gotchas

- **Start latency.** A grain needs `Δ + N + Hs` frames of lookahead before it can form, so the
  first output appears only after that much input — and the output is shorter than
  `input / speed_ratio` by roughly that latency on short clips. This is inherent to TSM and is
  reported by `latency()` as `input_frames = Δ + N + Hs`. Tests assert length ratios on
  ≥0.5 s inputs for this reason; very short clips deviate.
- **`flush` must be called until it returns 0.** It first drains `out_hold`, then finalizes the
  last grain's un-overlapped tail once. Skipping it drops the final ~`Hs` frames.
- **Fixed format.** Built for one `(sample_rate, channels)`; `process`/`flush` return
  empty/`0` if called with a different format. Changing format means a new stretcher.
- **`pitch_semitones` is stored but not applied** (kept only so `params()` reflects what was
  set). Setting it does nothing audible until the pitch-shift increment lands.
- **Reset.** `reset()` clears all streaming state; a reused-then-reset stretcher produces
  byte-identical output to a fresh one (asserted by `reset_clears_state`).

## Tests

`#[cfg(test)] mod tests` in the same file: format rejection, length scaling at speed
1.0 / 0.5 / 2.0, **pitch preservation while stretching** (zero-crossing freq estimate stays
~440 Hz at 2× stretch — the defining TSM property), finite/bounded output, silence→silence,
reset-equals-fresh, and honest capabilities.
