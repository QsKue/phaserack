# AGENTS.md

`warble` is a small Rust **library crate** (edition 2024, `AGPL-3.0-or-later`) that **time-stretches
and pitch-shifts** interleaved `f32` audio — it changes a signal's length and/or pitch. It is a
**leaf** in the q-lib audio split: it owns the `TimeStretcher` contract and its implementations and
nothing else — no engine, no session, no I/O, no async. It depends only on **prism** for the shared
`Latency` value type. Keep changes minimal, allocation-conscious on the hot path, and
engine-agnostic.

**Consumer / workspace context.** warble was **extracted out of the `maestro` engine** (it used to
live in `maestro/src/sources/time_stretchers/`). maestro now re-exports warble's types and keeps
only a session-aware `build_time_stretcher` factory; it drives stretchers **through the
`TimeStretcher` trait**. The trait used to return maestro's `AudioLatency`; that type was folded
into `prism::Latency`, which `latency()` now returns. maestro is the **hub / main entry point** for
the audio docs ([QsKue/maestro](https://github.com/QsKue/maestro)); **prism** is the shared base
([QsKue/prism](https://github.com/QsKue/prism)). warble must stand alone — build and reason about it
without the engine — and must not gain engine/session knowledge. All three crates are git submodules
and path workspace members.

## Where to look

- `docs/ARCHITECTURE.md` — module tree, the `TimeStretcher` contract, the process/flush/latency data
  flow, the `signalsmith` feature gate, and the check commands. Read before touching the trait or the
  public API.
- `docs/AREAS/*.md` — per-module conventions and gotchas. Read the one for any file you change.
- `docs/DECISIONS/*.md` — durable design decisions with rationale (ADRs).
- `docs/ROADMAP.md` — current implementations and the plausible future stretcher family.

## Architecture in one screen

- `src/lib.rs` — crate root; declares modules, re-exports the trait + types + `NoopTimeStretcher`,
  and (behind the `signalsmith` feature) `SignalSmithTimeStretcher`.
- `src/stretcher.rs` — the **contract**: the `TimeStretcher` trait
  (`process`/`flush`/`reset`/`latency`/`capabilities`/`set_params`/`params` on interleaved `f32`),
  the value types `TimeStretcherCapabilities` / `TimeStretcherParams` / `TimeStretchProcessResult`,
  and `NoopTimeStretcher` (pass-through). Dependency-free beyond `prism::Latency`.
- `src/signalsmith.rs` — `SignalSmithTimeStretcher`, the real backend over `signalsmith-stretch`,
  **behind the `signalsmith` feature** (on by default). The only thing that pulls a DSP dependency.

## Conventions (the durable rules)

- **Interleaved `f32`, separate buffers.** `process` reads interleaved input and writes a *separate*
  interleaved output buffer; frame counts derive from `slice.len() / channels`. There is no in-place
  mode.
- **Partial consume/produce is normal.** `process` returns a `TimeStretchProcessResult`
  (`input_frames_consumed`, `output_frames_written`); a call may consume/write fewer frames than the
  buffers hold. Callers must loop and honor both counts — never assume a call drains the input or
  fills the output.
- **Tail via `flush`.** After the final input block, `flush` writes remaining tail frames (returns
  the frame count) until it returns `0`. Don't expect tail audio out of `process`.
- **Report latency as `prism::Latency`.** `latency()` returns a `prism::Latency` so the engine can
  sum it across pipeline stages. Don't invent a local latency type.
- **Guard the format.** A backend validates `(sample_rate, channels)` against what it was built with
  (`format_matches`) and `channels == 0`, returning an empty result rather than processing a
  mismatched buffer. Keep that guard on every entry point.
- **Capabilities are honest.** `TimeStretcherCapabilities` advertises what a stretcher can actually
  do; `supports_realtime_autotune()` = `realtime && pitch_shift && independent_pitch_and_speed`.
  Callers pick a stretcher by capability — keep the flags truthful.
- **Stay engine-agnostic and small.** No session, routing, device, or pipeline concepts. warble
  transforms a buffer the caller provides; the engine decides when and why.

## Warning signs

- A method assumes `process` drained the input or filled the output instead of reading
  `TimeStretchProcessResult`.
- `latency()` returns something other than `prism::Latency`, or a local latency type creeps back in.
- The `signalsmith` backend's `format_matches` / `channels == 0` guard is dropped, so a mismatched
  buffer reaches the DSP.
- The trait grows a method to suit exactly one backend, or `stretcher.rs` gains a dependency on
  `signalsmith-stretch` (it must stay buildable with the feature off).
- Engine/session concepts (sources, sessions, routing) appear anywhere in the crate.

## Making a change

1. Read `docs/ARCHITECTURE.md` (if touching the trait/API boundary) and the relevant
   `docs/AREAS/*.md`.
2. Keep the change small and engine-agnostic; keep doc updates near the behavior change.
3. Run the checks in `docs/ARCHITECTURE.md` (fmt/clippy/test). Default features pull the
   `signalsmith` backend — keep it building. When changing the trait or a result's semantics, update
   `NoopTimeStretcher`, the backend, and the docs together.

## Docs maintenance

- **Code is truth for behavior; docs explain why and what-not-to-do, not line-by-line how.**
- **Git is the task log** — there is no task-history / changelog directory, and you should not create
  one (it only duplicates `git log`).
- Update the smallest useful set: `docs/AREAS/*` for a changed convention/gotcha (one file per real
  module — keep `Source:` paths honest), `docs/ARCHITECTURE.md` for the trait / data flow / API
  shape, a new `docs/DECISIONS/` ADR (from `docs/TEMPLATES/decision-template.md`) for a durable
  choice, `docs/ROADMAP.md` for plan and status. Keep every doc short enough to read at task start.
