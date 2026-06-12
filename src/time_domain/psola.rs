use std::collections::VecDeque;

use sinerack::Latency;

use crate::stretcher::{
    TimeStretchProcessResult, TimeStretcher, TimeStretcherCapabilities, TimeStretcherParams,
};

/// How a [`PsolaTimeStretcher`] runs the synthesis — the caller's choice when building the
/// shifter (parametrized, like the injected marker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsolaMode {
    /// Incremental, low-latency synthesis over a bounded sliding buffer — usable live.
    /// `capabilities().realtime == true`.
    Realtime,
    /// Buffer the whole input and synthesize at end-of-stream (`flush`). Simpler, exact,
    /// but emits nothing until EOF — `capabilities().realtime == false`. For offline render.
    Offline,
}

/// Tuning for [`PsolaTimeStretcher`]. The F0 range sets the grain period clamps; keep it in
/// step with the injected marker's range.
#[derive(Debug, Clone, Copy)]
pub struct PsolaConfig {
    /// Lowest F0 (Hz) — sets the **longest** grain period (`sr / f0_min`).
    pub f0_min: f32,
    /// Highest F0 (Hz) — sets the **shortest** grain period (`sr / f0_max`).
    pub f0_max: f32,
}

impl Default for PsolaConfig {
    fn default() -> Self {
        Self {
            f0_min: 75.0,
            f0_max: 600.0,
        }
    }
}

/// One analysis pitch mark in **absolute** input-frame coordinates.
#[derive(Clone, Copy)]
struct Mark {
    pos: i64,
    voiced: bool,
}

/// **TD-PSOLA** — Pitch-Synchronous Overlap-Add (Moulines–Charpentier), the
/// formant-preserving monophonic pitch shifter.
///
/// Grains are windowed around **pitch marks** (one per glottal period, from an injected
/// [`pitchrack::PitchMarker`]) and overlap-added at a *new* spacing: the synthesis period is
/// the analysis period divided by the pitch factor `β = 2^(semitones/12)`, so re-spacing the
/// grains changes the pitch while each grain's spectral envelope — the formants — is left
/// untouched. Time scaling (speed) advances the analysis pointer at `1/α` of the synthesis
/// rate (`α = output/input duration = 1/speed_ratio`), reusing or skipping grains. Pitch and
/// speed are independent by construction; unvoiced marks are overlap-added without pitch
/// change (noise is not pitch-shifted).
///
/// The marker is **dependency-injected** ([`with_marker`](Self::with_marker)) like WSOLA's
/// resampler — the marker (F0-peak now, GCI later) is the caller's choice. The synthesis
/// model is [parametrized](PsolaMode): `Realtime` (incremental, live) or `Offline` (exact,
/// at-flush). Both share one synthesis core; they differ only in *when* it runs.
///
/// Built for one fixed `(sample_rate, channels)`. Without a marker it is a pass-through that
/// advertises no pitch/stretch capability (honest [`capabilities`](TimeStretcher::capabilities)).
pub struct PsolaTimeStretcher {
    sample_rate: u32,
    channels: usize,
    params: TimeStretcherParams,
    mode: PsolaMode,
    config: PsolaConfig,
    marker: Option<Box<dyn pitchrack::pitch_marks::PitchMarker>>,

    // --- input (absolute per-channel frame addressing) ---
    in_buf: Vec<f32>,
    in_start: i64,
    mono: Vec<f32>,
    marks: Vec<Mark>,

    // --- synthesis cursors (absolute frames) ---
    /// Output frame of the next synthesis mark.
    synth_pos: f64,
    /// Input frame the next synthesis mark reads from (the time-scaled analysis pointer).
    anal_pos: f64,
    primed: bool,

    // --- output overlap-add accumulator (absolute output frames) ---
    acc: VecDeque<f32>,
    acc_start: i64,
    out_hold: VecDeque<f32>,
    flushed: bool,

    // --- cached Hann window, keyed by length ---
    window: Vec<f32>,
    window_len: usize,
}

impl PsolaTimeStretcher {
    /// Builds a PSOLA stretcher for one fixed `(sample_rate, channels)` and [`PsolaMode`].
    /// A marker must be attached with [`with_marker`](Self::with_marker) before it can
    /// pitch-shift; until then it passes audio through.
    pub fn new(sample_rate: u32, channels: usize, mode: PsolaMode) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".to_string());
        }
        if channels == 0 {
            return Err("channel count must be greater than zero".to_string());
        }
        Ok(Self {
            sample_rate,
            channels,
            params: TimeStretcherParams::default(),
            mode,
            config: PsolaConfig::default(),
            marker: None,
            in_buf: Vec::new(),
            in_start: 0,
            mono: Vec::new(),
            marks: Vec::new(),
            synth_pos: 0.0,
            anal_pos: 0.0,
            primed: false,
            acc: VecDeque::new(),
            acc_start: 0,
            out_hold: VecDeque::new(),
            flushed: false,
            window: Vec::new(),
            window_len: 0,
        })
    }

    /// Attaches the pitch-mark source, enabling pitch shifting. The marker is the caller's
    /// choice — any [`pitchrack::PitchMarker`] (F0-peak today, GCI later) — swappable by
    /// re-injecting. Without one, PSOLA passes audio through.
    pub fn with_marker(mut self, marker: Box<dyn pitchrack::pitch_marks::PitchMarker>) -> Self {
        self.marker = Some(marker);
        self
    }

    /// Replaces the F0-range config (e.g. a live knob); affects the grain period clamps.
    pub fn set_config(&mut self, config: PsolaConfig) {
        self.config = config;
    }

    /// The current config.
    pub fn config(&self) -> PsolaConfig {
        self.config
    }

    fn format_matches(&self, sample_rate: u32, channels: usize) -> bool {
        self.sample_rate == sample_rate && self.channels == channels
    }

    fn pitch_factor(&self) -> f64 {
        2.0_f64.powf(self.params.pitch_semitones as f64 / 12.0)
    }

    /// `α = output/input duration = 1 / speed_ratio`.
    fn time_scale(&self) -> f64 {
        let s = self.params.speed_ratio as f64;
        if s.is_finite() && s > 1e-6 {
            1.0 / s
        } else {
            1.0
        }
    }

    fn min_period(&self) -> usize {
        ((self.sample_rate as f64 / self.config.f0_max.max(1.0) as f64).round() as usize).max(2)
    }

    fn max_period(&self) -> usize {
        ((self.sample_rate as f64 / self.config.f0_min.max(1.0) as f64).round() as usize)
            .max(self.min_period() + 1)
    }

    fn buffered_end(&self) -> i64 {
        self.in_start + (self.in_buf.len() / self.channels) as i64
    }

    /// One interleaved sample at absolute input frame `abs`, channel `ch`; 0 outside buffer.
    #[inline]
    fn input_at(&self, abs: i64, ch: usize) -> f32 {
        let rel = abs - self.in_start;
        if rel < 0 || rel as usize >= self.in_buf.len() / self.channels {
            return 0.0;
        }
        self.in_buf[rel as usize * self.channels + ch]
    }

    /// (Re)builds the absolute analysis marks from the buffered input via the injected marker.
    fn remark(&mut self) {
        let ch = self.channels;
        let frames = self.in_buf.len() / ch;
        self.mono.clear();
        self.mono.reserve(frames);
        for f in 0..frames {
            let mut s = 0.0;
            for c in 0..ch {
                s += self.in_buf[f * ch + c];
            }
            self.mono.push(s / ch as f32);
        }
        let Some(marker) = self.marker.as_mut() else {
            self.marks.clear();
            return;
        };
        let rel = marker.pitch_marks(&self.mono, self.sample_rate as usize);
        self.marks.clear();
        self.marks.extend(rel.iter().map(|m| Mark {
            pos: self.in_start + m.position as i64,
            voiced: m.voiced,
        }));
    }

    /// Index of the mark nearest absolute input frame `pos`.
    fn nearest_mark(&self, pos: f64) -> Option<usize> {
        if self.marks.is_empty() {
            return None;
        }
        let p = pos.round() as i64;
        let idx = self.marks.partition_point(|m| m.pos < p);
        let cand = if idx == 0 {
            0
        } else if idx >= self.marks.len() {
            self.marks.len() - 1
        } else if (p - self.marks[idx - 1].pos) <= (self.marks[idx].pos - p) {
            idx - 1
        } else {
            idx
        };
        Some(cand)
    }

    /// The local analysis period at mark `i` (spacing to a neighbour), clamped to the F0 range.
    fn local_period(&self, i: usize) -> usize {
        let raw = if i + 1 < self.marks.len() {
            self.marks[i + 1].pos - self.marks[i].pos
        } else if i > 0 {
            self.marks[i].pos - self.marks[i - 1].pos
        } else {
            self.max_period() as i64
        };
        (raw.max(1) as usize).clamp(self.min_period(), self.max_period())
    }

    /// Ensures the accumulator covers `[start_out, start_out + len)` (absolute output frames),
    /// extending it with silence at the back. `start_out >= acc_start` always holds because
    /// the synthesis pointer only advances and we never finalize past a live grain.
    fn ensure_acc(&mut self, start_out: i64, len: usize) {
        let ch = self.channels;
        if self.acc.is_empty() {
            self.acc_start = start_out;
        }
        let have_end = self.acc_start + (self.acc.len() / ch) as i64;
        let need_end = start_out + len as i64;
        if need_end > have_end {
            let add = (need_end - have_end) as usize * ch;
            self.acc.extend(std::iter::repeat_n(0.0, add));
        }
    }

    /// Windows the grain centred at input frame `t_i` (length `2*period`) and overlap-adds it
    /// into the accumulator centred at output frame `round(synth_center)`.
    fn emit_grain(&mut self, t_i: i64, period: usize, synth_center: f64) {
        let ch = self.channels;
        let len = 2 * period;
        let start_in = t_i - period as i64;
        let start_out = synth_center.round() as i64 - period as i64;
        self.ensure_acc(start_out, len);
        let base = ((start_out - self.acc_start) as usize) * ch;
        // Copy the window out so the borrow of `self.window` doesn't clash with `self.acc`.
        let window_len = len;
        if self.window_len != window_len {
            self.window = hann(window_len);
            self.window_len = window_len;
        }
        for k in 0..len {
            let w = self.window[k];
            let in_f = start_in + k as i64;
            let idx = base + k * ch;
            for c in 0..ch {
                let s = self.input_at(in_f, c);
                self.acc[idx + c] += w * s;
            }
        }
    }

    /// Moves finalized frames (output frame `< up_to`) from the accumulator front into
    /// `out_hold` — no future grain can reach them.
    fn finalize_output(&mut self, up_to: i64) {
        let ch = self.channels;
        while self.acc_start < up_to && self.acc.len() >= ch {
            for _ in 0..ch {
                self.out_hold.push_back(self.acc.pop_front().unwrap());
            }
            self.acc_start += 1;
        }
    }

    /// Synthesizes output marks while the required analysis grain is fully buffered and lies
    /// at or before `stop_at` (the trusted input frontier). The shared core of both modes.
    fn synthesize(&mut self, stop_at: i64) {
        if self.marks.is_empty() || self.marker.is_none() {
            return;
        }
        let beta = self.pitch_factor();
        let alpha = self.time_scale();
        let max_period = self.max_period() as i64;

        if !self.primed {
            self.synth_pos = self.marks[0].pos as f64;
            self.anal_pos = self.marks[0].pos as f64;
            self.primed = true;
        }

        loop {
            let Some(i) = self.nearest_mark(self.anal_pos) else {
                break;
            };
            let period = self.local_period(i);
            let t_i = self.marks[i].pos;
            // The grain spans [t_i - period, t_i + period]; its forward end must be within the
            // trusted frontier. The backward end may read before the buffer start — `input_at`
            // zero-pads there (only the very first grain, at frame 0; `trim` keeps two periods
            // of slack so a live grain never reaches genuinely-discarded input).
            if t_i + period as i64 > stop_at {
                break;
            }

            let beta_eff = if self.marks[i].voiced { beta } else { 1.0 };
            self.emit_grain(t_i, period, self.synth_pos);

            let q = period as f64 / beta_eff;
            self.synth_pos += q;
            self.anal_pos += q / alpha;

            // Finalize output no future grain (centred at >= synth_pos, reaching back at most
            // max_period) can touch.
            let safe = self.synth_pos.floor() as i64 - max_period;
            self.finalize_output(safe);

            // Stop once the analysis pointer's next grain would run past the frontier.
            if self.anal_pos as i64 + max_period > stop_at {
                break;
            }
        }
    }

    /// Drops buffered input (and stale marks) no future grain can reach — keeps the realtime
    /// buffer bounded. Holds two max-periods of slack behind the analysis pointer.
    fn trim(&mut self) {
        if !self.primed {
            return;
        }
        let keep_from =
            (self.anal_pos.floor() as i64 - 2 * self.max_period() as i64).max(self.in_start);
        let drop = (keep_from - self.in_start) as usize;
        if drop > 0 {
            self.in_buf.drain(0..drop * self.channels);
            self.in_start += drop as i64;
        }
    }

    /// Moves up to `out_capacity_frames` finalized frames into `output`.
    fn drain(&mut self, output: &mut [f32], out_capacity_frames: usize) -> usize {
        let ch = self.channels;
        let frames = out_capacity_frames.min(self.out_hold.len() / ch);
        for slot in output.iter_mut().take(frames * ch) {
            *slot = self.out_hold.pop_front().unwrap();
        }
        frames
    }
}

impl TimeStretcher for PsolaTimeStretcher {
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        sample_rate: u32,
        channels: usize,
    ) -> TimeStretchProcessResult {
        if !self.format_matches(sample_rate, channels) || channels == 0 {
            return TimeStretchProcessResult::default();
        }
        let input_frames = input.len() / channels;
        let out_capacity_frames = output.len() / channels;

        self.in_buf
            .extend_from_slice(&input[..input_frames * channels]);

        // Pass-through when no marker is attached (honest: PSOLA can't shift without marks).
        if self.marker.is_none() {
            let take = out_capacity_frames.min(self.in_buf.len() / channels);
            for slot in output.iter_mut().take(take * channels) {
                *slot = self.in_buf.remove(0);
            }
            self.in_start += take as i64;
            return TimeStretchProcessResult {
                input_frames_consumed: input_frames,
                output_frames_written: take,
            };
        }

        if self.mode == PsolaMode::Realtime {
            self.remark();
            // Trust marks a grain-and-margin back from the buffered edge.
            let stop_at = self.buffered_end() - 2 * self.max_period() as i64;
            self.synthesize(stop_at);
            self.trim();
        }
        // Offline mode buffers only; synthesis happens in `flush`.

        let written = self.drain(output, out_capacity_frames);
        TimeStretchProcessResult {
            input_frames_consumed: input_frames,
            output_frames_written: written,
        }
    }

    fn flush(&mut self, output: &mut [f32], sample_rate: u32, channels: usize) -> usize {
        if !self.format_matches(sample_rate, channels) || channels == 0 {
            return 0;
        }
        let out_capacity_frames = output.len() / channels;
        if out_capacity_frames == 0 {
            return 0;
        }

        // Pass-through: drain the buffered remainder.
        if self.marker.is_none() {
            let take = out_capacity_frames.min(self.in_buf.len() / channels);
            for slot in output.iter_mut().take(take * channels) {
                *slot = self.in_buf.remove(0);
            }
            self.in_start += take as i64;
            return take;
        }

        // Run the remaining synthesis once (both modes): mark everything buffered, synthesize
        // to the end, finalize the whole accumulator. Repeated calls then just drain.
        if !self.flushed {
            self.remark();
            self.synthesize(self.buffered_end());
            self.finalize_output(i64::MAX / 2);
            self.flushed = true;
        }
        self.drain(output, out_capacity_frames)
    }

    fn reset(&mut self) {
        self.in_buf.clear();
        self.in_start = 0;
        self.mono.clear();
        self.marks.clear();
        self.synth_pos = 0.0;
        self.anal_pos = 0.0;
        self.primed = false;
        self.acc.clear();
        self.acc_start = 0;
        self.out_hold.clear();
        self.flushed = false;
    }

    fn latency(&self) -> Latency {
        // Pitch marks need a couple of periods of lookahead before the first grain can form;
        // Offline mode buffers the whole stream, so its effective latency is the stream — but
        // we report the algorithmic minimum (the engine treats Offline as non-realtime anyway).
        Latency::new(2 * self.max_period(), 0, 0)
    }

    fn capabilities(&self) -> TimeStretcherCapabilities {
        let can = self.marker.is_some();
        TimeStretcherCapabilities {
            realtime: can && self.mode == PsolaMode::Realtime,
            pitch_shift: can,
            time_stretch: can,
            independent_pitch_and_speed: can,
        }
    }

    fn set_params(&mut self, params: TimeStretcherParams) {
        let speed_ratio = if params.speed_ratio.is_finite() {
            params.speed_ratio.max(f32::EPSILON)
        } else {
            1.0
        };
        let pitch_semitones = if params.pitch_semitones.is_finite() {
            params.pitch_semitones
        } else {
            0.0
        };
        self.params = TimeStretcherParams {
            speed_ratio,
            pitch_semitones,
        };
    }

    fn params(&self) -> TimeStretcherParams {
        self.params
    }
}

/// Periodic Hann window of length `n`.
fn hann(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f32::consts::PI * i as f32 / n as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pitchrack::pitch_marks::{PitchMark, PitchMarker};

    const SR: u32 = 44_100;

    /// A stub marker emitting voiced marks at a fixed period over the whole signal — isolates
    /// the PSOLA *synthesis* from any real marker so pitch/time assertions are exact.
    struct PeriodicMarker {
        period: usize,
    }
    impl PitchMarker for PeriodicMarker {
        fn pitch_marks(&mut self, signal: &[f32], _sample_rate: usize) -> Vec<PitchMark> {
            let mut marks = Vec::new();
            let mut p = self.period / 2;
            while p < signal.len() {
                marks.push(PitchMark {
                    position: p,
                    voiced: true,
                });
                p += self.period;
            }
            marks
        }
    }

    fn sine(freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / SR as f32).sin())
            .collect()
    }

    /// A pulse train at `f0` (impulses at `period/2 + k·period`) — the canonical PSOLA test
    /// signal: impulsive excitation with a flat envelope, so re-spacing the grains cleanly
    /// changes the fundamental. The stub marker marks the same period, so marks land on pulses.
    fn pulse_train(f0: f32, frames: usize) -> Vec<f32> {
        let period = (SR as f32 / f0).round() as usize;
        let mut v = vec![0.0f32; frames];
        let mut p = period / 2;
        while p < frames {
            v[p] = 1.0;
            p += period;
        }
        v
    }

    fn frames(v: &[f32]) -> usize {
        v.len()
    }

    /// Pulse rate (Hz) of an impulse-train-like signal: counts thresholded local maxima over
    /// the duration. PSOLA re-spaces pulses, so this is the output fundamental.
    fn pulse_rate(v: &[f32]) -> f32 {
        if v.len() < 3 {
            return 0.0;
        }
        let max = v.iter().cloned().fold(0.0f32, f32::max);
        let thresh = (max * 0.5).max(1e-3);
        let mut count = 0;
        for i in 1..v.len() - 1 {
            if v[i] > thresh && v[i] >= v[i - 1] && v[i] > v[i + 1] {
                count += 1;
            }
        }
        count as f32 * SR as f32 / v.len() as f32
    }

    fn marker(f0: f32) -> Box<dyn PitchMarker> {
        Box::new(PeriodicMarker {
            period: (SR as f32 / f0).round() as usize,
        })
    }

    /// Drives a stretcher to completion (mono), returning output.
    fn run(st: &mut PsolaTimeStretcher, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 4096];
        let block = 2048;
        let mut pos = 0;
        while pos < input.len() {
            let end = (pos + block).min(input.len());
            let r = st.process(&input[pos..end], &mut buf, SR, 1);
            out.extend_from_slice(&buf[..r.output_frames_written]);
            pos += r.input_frames_consumed.max(1);
        }
        loop {
            let n = st.flush(&mut buf, SR, 1);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    fn psola(mode: PsolaMode, f0: f32) -> PsolaTimeStretcher {
        PsolaTimeStretcher::new(SR, 1, mode)
            .unwrap()
            .with_marker(marker(f0))
    }

    #[test]
    fn rejects_bad_format() {
        assert!(PsolaTimeStretcher::new(0, 1, PsolaMode::Realtime).is_err());
        assert!(PsolaTimeStretcher::new(SR, 0, PsolaMode::Offline).is_err());
    }

    #[test]
    fn config_round_trips() {
        let mut st = psola(PsolaMode::Realtime, 200.0);
        assert_eq!(st.config().f0_min, 75.0);
        st.set_config(PsolaConfig {
            f0_min: 50.0,
            f0_max: 800.0,
        });
        assert_eq!(st.config().f0_min, 50.0);
        assert_eq!(st.config().f0_max, 800.0);
    }

    #[test]
    fn without_a_marker_passes_through() {
        let mut st = PsolaTimeStretcher::new(SR, 1, PsolaMode::Realtime).unwrap();
        let caps = st.capabilities();
        assert!(!caps.pitch_shift, "no marker → no pitch shift");
        let input = sine(220.0, 8000);
        let out = run(&mut st, &input);
        assert_eq!(out, input, "pass-through must be sample-exact");
    }

    #[test]
    fn capabilities_reflect_mode() {
        let rt = psola(PsolaMode::Realtime, 200.0);
        assert!(rt.capabilities().realtime && rt.capabilities().supports_realtime_autotune());
        let off = psola(PsolaMode::Offline, 200.0);
        assert!(!off.capabilities().realtime);
        assert!(off.capabilities().pitch_shift && off.capabilities().independent_pitch_and_speed);
    }

    fn assert_pitch_up_octave(mode: PsolaMode) {
        let f0 = 200.0;
        let mut st = psola(mode, f0);
        st.set_params(TimeStretcherParams {
            speed_ratio: 1.0,
            pitch_semitones: 12.0, // ×2
        });
        let input = pulse_train(f0, SR as usize); // 1 s
        let out = run(&mut st, &input);

        let dur = frames(&out) as f32 / frames(&input) as f32;
        assert!((dur - 1.0).abs() < 0.15, "{mode:?}: duration {dur} ~ 1.0");
        let rate = pulse_rate(&out);
        assert!(
            (rate - 400.0).abs() < 40.0,
            "{mode:?}: pulse rate {rate} ~ 400 (octave up)"
        );
    }

    #[test]
    fn pitch_up_octave_realtime() {
        assert_pitch_up_octave(PsolaMode::Realtime);
    }

    #[test]
    fn pitch_up_octave_offline() {
        assert_pitch_up_octave(PsolaMode::Offline);
    }

    #[test]
    fn pitch_down_octave_realtime() {
        let f0 = 200.0;
        let mut st = psola(PsolaMode::Realtime, f0);
        st.set_params(TimeStretcherParams {
            speed_ratio: 1.0,
            pitch_semitones: -12.0,
        });
        let input = pulse_train(f0, SR as usize);
        let out = run(&mut st, &input);
        let dur = frames(&out) as f32 / frames(&input) as f32;
        assert!((dur - 1.0).abs() < 0.15, "duration {dur} ~ 1.0");
        let rate = pulse_rate(&out);
        assert!(
            (rate - 100.0).abs() < 20.0,
            "pulse rate {rate} ~ 100 (octave down)"
        );
    }

    #[test]
    fn time_stretch_without_pitch_change() {
        // speed 0.5 → ~2× longer, pitch unchanged (the TSM property, via PSOLA).
        let f0 = 200.0;
        let mut st = psola(PsolaMode::Realtime, f0);
        st.set_params(TimeStretcherParams {
            speed_ratio: 0.5,
            pitch_semitones: 0.0,
        });
        let input = pulse_train(f0, SR as usize);
        let out = run(&mut st, &input);
        let dur = frames(&out) as f32 / frames(&input) as f32;
        assert!(dur > 1.7 && dur < 2.3, "duration {dur} ~ 2.0");
        // Pitch (pulse rate) unchanged even though duration doubled.
        let rate = pulse_rate(&out);
        assert!(
            (rate - f0).abs() < 25.0,
            "pulse rate {rate} should stay ~{f0}"
        );
    }

    #[test]
    fn realtime_and_offline_agree() {
        // Same transform, same input → the two modes should produce near-identical output.
        let f0 = 200.0;
        let make = |mode| {
            let mut st = psola(mode, f0);
            st.set_params(TimeStretcherParams {
                speed_ratio: 1.0,
                pitch_semitones: 7.0,
            });
            st
        };
        let input = pulse_train(f0, SR as usize / 2);
        let rt = run(&mut make(PsolaMode::Realtime), &input);
        let off = run(&mut make(PsolaMode::Offline), &input);
        let dl = (rt.len() as i64 - off.len() as i64).abs();
        assert!(dl < SR as i64 / 50, "lengths differ by {dl} (>20 ms)");
        assert!(
            (pulse_rate(&rt) - pulse_rate(&off)).abs() < 25.0,
            "pulse rate should match across modes"
        );
    }

    #[test]
    fn output_is_finite_and_bounded() {
        let mut st = psola(PsolaMode::Realtime, 150.0);
        st.set_params(TimeStretcherParams {
            speed_ratio: 0.8,
            pitch_semitones: 4.0,
        });
        let out = run(&mut st, &pulse_train(150.0, SR as usize / 2));
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| s.is_finite() && s.abs() < 4.0));
    }

    #[test]
    fn reset_behaves_like_fresh() {
        let f0 = 200.0;
        let input = sine(f0, 20_000);
        let mut reused = psola(PsolaMode::Offline, f0);
        reused.set_params(TimeStretcherParams {
            speed_ratio: 1.0,
            pitch_semitones: 5.0,
        });
        let _ = run(&mut reused, &input);
        reused.reset();
        let after = run(&mut reused, &input);

        let mut fresh = psola(PsolaMode::Offline, f0);
        fresh.set_params(TimeStretcherParams {
            speed_ratio: 1.0,
            pitch_semitones: 5.0,
        });
        let from_fresh = run(&mut fresh, &input);
        assert_eq!(after, from_fresh, "reset must behave like a fresh build");
    }
}
