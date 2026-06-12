# PSOLA backend (`time_domain/`, feature `psola`)

Source: `src/time_domain/psola.rs` — `PsolaTimeStretcher`. **TD-PSOLA** (Pitch-Synchronous
Overlap-Add, Moulines–Charpentier): the formant-preserving monophonic pitch shifter, the first
phaserack backend built entirely on *our* stack rather than a wrapped third party. Behind the
`psola` cargo feature, which pulls only pitchrack's `pitch-marks` trait (`default-features =
false` → no detector, no `rustfft`), so phaserack stays FFT-free. Rationale: pitchrack
[ADR 0008](https://github.com/QsKue/pitchrack) (the pitch-mark contract) + phaserack ADR 0002.

## The idea

A grain is one Hann-windowed glottal period centred on a **pitch mark**. Overlap-adding the
grains at a *new* spacing changes the fundamental while each grain's spectral envelope (the
formants) is untouched — so a voice shifts pitch without turning into a chipmunk. This only
works on signals with impulsive per-period structure (voice, pulse trains); a pure sine is a
degenerate case (no excitation/formant separation), which is why the tests use a pulse train.

## Injected marker (the analysis half)

The pitch marks come from a **dependency-injected** `Box<dyn pitchrack::PitchMarker>`
([`with_marker`](../../src/time_domain/psola.rs)) — exactly as WSOLA takes a resampler. The
marker (F0-peak today, GCI later) is the caller's choice, swappable without touching PSOLA.
Without a marker, PSOLA is a sample-exact pass-through and advertises no pitch/stretch capability
(honest, dynamic `capabilities`). PSOLA marks a **mono mix** and applies the marks to all
channels.

## The math (`synthesize`)

For pitch factor `β = 2^(semitones/12)` and time-scale `α = output/input = 1/speed_ratio`, iterate
synthesis marks: find the analysis mark nearest the analysis pointer `anal_pos`, take its local
period `P` (mark spacing, clamped to the F0 range), overlap-add its `2P` Hann grain into the
output accumulator centred at `synth_pos`, then advance `synth_pos += P/β` and `anal_pos +=
(P/β)/α`. Sanity: `β=1,α=1` copies each grain once (identity); `α=2` advances analysis at half
rate (each grain reused → 2× longer); `β=2` spaces output marks at `P/2` (octave up, same length).
**Unvoiced** marks use `β_eff = 1` (noise is not pitch-shifted), OLA pass-through.

## Modes (`PsolaMode`, parametrized)

The synthesis core is shared; the mode only changes *when* it runs:
- **`Realtime`** — `process` re-marks a bounded sliding buffer, synthesizes the settled region
  (frontier = `buffered_end − 2·max_period`), emits, and `trim`s consumed input. Low-latency,
  usable live; `capabilities().realtime == true`.
- **`Offline`** — `process` only buffers; `flush` marks the whole signal and synthesizes at once.
  Exact, but emits nothing until EOF; `capabilities().realtime == false`. For offline render.

Both produce near-identical output (a test asserts it). The caller picks the mode when building the
shifter — the same "parametrize, user chooses" pattern as the resampler backend and the marker.

## Amplitude (window-sum normalization)

Re-spacing the grains for a pitch shift breaks the Hann constant-overlap, so without correction
pitch-up comes out ~louder and pitch-down quieter (in proportion to the pitch ratio). A parallel
per-frame **window accumulator** (`wacc`) sums the overlapped window values, and the output is
divided by it at finalization (`out = acc / max(wacc, 0.1)`). The `0.1` floor caps the gain so
near-zero-coverage gaps stay quiet instead of amplifying noise. Loudness is then constant across
pitch shifts. Large pitch-*down* still carries some inherent overlap ripple (grains abut at ~0%
overlap) — a documented TD-PSOLA property, not a normalization failure.

## Streaming mechanics

Absolute per-channel frame addressing throughout (`in_start`, `acc_start`). `input_at` zero-pads
outside the buffer (only the first grain, at frame 0). An output overlap-add accumulator (`acc` +
`wacc` + `acc_start`) is grown at the back per grain and finalized (and normalized) at the front
once `synth_pos − max_period` passes a frame (no future grain can reach it). `trim` keeps two max-periods of slack behind
`anal_pos`, so a live grain never reaches genuinely-discarded input. `latency()` reports the
algorithmic minimum (`2·max_period`); Offline's effective latency is the whole stream (the engine
treats it as non-realtime).

## Gotchas

- **Marker required for any effect** — no marker ⇒ pass-through.
- **Pulse-train test signal** — PSOLA can't shift a pure sine (no excitation structure); tests use
  a pulse train and measure **pulse rate**, not zero-crossings.
- **Fixed format** — built for one `(sample_rate, channels)`; a different format returns empty/`0`.
- **F0-range config** — `PsolaConfig { f0_min, f0_max }` sets the grain period clamps; keep it in
  step with the injected marker's range. `config()`/`set_config()` are the live-knob surface.

## Next

A **GCI marker** (pitchrack ADR 0008 §Next) drops in via the same `PitchMarker` injection — no
PSOLA change. PSOLA itself is the v1 of the parametric/high-quality voice path; SMS (parametric
family) is the next phaserack backend.

## Tests

`#[cfg(test)] mod tests`, injecting a periodic stub marker (isolates synthesis from any real
marker): format rejection, config round-trip, no-marker pass-through, mode-reflecting
capabilities, octave up/down keep duration while doubling/halving pulse rate (both modes),
time-stretch without pitch change, realtime≈offline agreement, finite/bounded output,
reset-equals-fresh. Run with `cargo test --features psola` (or `--all-features`).
