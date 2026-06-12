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

**Dynamic, depending on whether a resampler is attached** (honest capabilities):
- **No resampler** (the default, and the only option without the `pitch-shift` feature):
  `time_stretch: true`, `pitch_shift: false`, `independent_pitch_and_speed: false`. Does not
  satisfy `supports_realtime_autotune()`.
- **With a resampler** (`with_resampler`, feature `pitch-shift`): `pitch_shift: true` and
  `independent_pitch_and_speed: true`, so it **does** satisfy `supports_realtime_autotune()`.

## Pitch shifting (increment 2 — time-stretch-then-resample)

Pitch shifting on the time-domain family is *stretch then resample*. Attach a resampler with
`with_resampler(Box<dyn samplerack::Resampler>)` (feature `pitch-shift`, which pulls only
samplerack's trait — `default-features = false`, no `rustfft`, so phaserack stays FFT-free).
The resampler is **dependency-injected**, so the backend — and the std vs `no_std` trade-off —
is the caller's choice, swappable at runtime by re-injecting (ADR 0002 §3 wraps; the resampler
half is samplerack ADR 0001/0003).

The math, for pitch ratio `p = 2^(semitones/12)` and `speed_ratio s`:
- WSOLA stretches by `p/s` — its `analysis_hop` uses **effective speed `s/p`**.
- The injected resampler runs at ratio **`1/p`** (WSOLA drives `set_ratio` from the pitch param).
- Net: duration `input/s`, pitch `× p` — independent pitch and speed.

The pipeline is `process → generate (stretched) → out_hold → pump_resampler → resampled_hold →
deliver`. When a resampler is attached, **all** output routes through it (even at 0 semitones,
where it runs at ratio 1) so there is no queue-switch discontinuity when the pitch sweeps
through zero — i.e. it stays click-free. `latency()` then sums WSOLA's lookahead and the
resampler's latency; `reset()`/`flush` drain both stages.

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
- **`pitch_semitones` without a resampler is stored but not applied** (kept only so `params()`
  reflects what was set) — this backend can shift pitch *only* through the injected resampler.
- **Resampler ratio bookkeeping.** Pitch up (`p > 1`) means resampler ratio `1/p < 1`
  (downsample, raises pitch); the samplerack sinc backend rebuilds its kernel table only when
  the cutoff moves (pitch up), so continuous up-sweeps cost a per-change table rebuild, down-
  sweeps don't. WSOLA owns the ratio; the caller never sets it.
- **Reset.** `reset()` clears all streaming state **and** the resampler (`resampler.reset()` +
  `resampled_hold`); a reused-then-reset stretcher produces byte-identical output to a fresh one.

## Tests

`#[cfg(test)] mod tests` in the same file. Time-stretch (always): format rejection, length
scaling at speed 1.0 / 0.5 / 2.0, **pitch preservation while stretching** (zero-crossing freq
stays ~440 Hz at 2× stretch — the defining TSM property), finite/bounded output,
silence→silence, reset-equals-fresh, honest capabilities. Pitch-shift (`#[cfg(feature =
"pitch-shift")]`, injecting a `samplerack::SincResampler`): a resampler enables the autotune
capabilities; octave-up doubles frequency and octave-down halves it, both keeping duration;
pitch and speed are independent (0.5× speed + octave up); and 0 semitones through the resampler
preserves pitch. Run with `cargo test --features pitch-shift`.
