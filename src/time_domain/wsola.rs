use std::collections::VecDeque;

use sinerack::Latency;

use crate::stretcher::{
    TimeStretchProcessResult, TimeStretcher, TimeStretcherCapabilities, TimeStretcherParams,
};

/// Waveform Similarity Overlap-Add time-stretcher.
///
/// A time-domain TSM backend: it changes a signal's **length** (playback speed)
/// while preserving pitch, by overlap-adding windowed grains taken from the input
/// at an analysis hop that differs from the synthesis hop. The "waveform
/// similarity" part is a short tolerance search around each analysis point that
/// picks the grain best continuing the previous one, which keeps the overlap-add
/// phase-coherent and avoids the buzzy artifacts of plain OLA.
///
/// This backend does **time-stretching only** — it advertises `pitch_shift:
/// false`. Pitch shifting on this family is time-stretch-then-resample and lands
/// as a later increment (see ROADMAP / ADR 0002). The grain math is FFT-free, so
/// the struct pulls no DSP dependency and needs no cargo feature.
///
/// Built for one fixed `(sample_rate, channels)`; a caller that changes format
/// must build a new stretcher (same contract as the signalsmith backend).
pub struct WsolaTimeStretcher {
    sample_rate: u32,
    channels: usize,
    params: TimeStretcherParams,

    // --- fixed grain geometry (per-channel frames) ---
    /// Synthesis hop `Hs`; with a Hann window of length `2*Hs` and 50% overlap the
    /// overlap-add sums to a constant, so amplitude is preserved.
    synth_hop: usize,
    /// Grain length `N = 2 * synth_hop`.
    frame: usize,
    /// Tolerance search radius `Δ` for the similarity match (per-channel frames).
    search: usize,
    /// Hann window of length `frame`.
    window: Vec<f32>,

    // --- streaming state ---
    /// Pending input, interleaved. `in_buf[0]` is absolute per-channel frame `in_start`.
    in_buf: Vec<f32>,
    /// Absolute per-channel frame index of `in_buf[0]`.
    in_start: i64,
    /// Absolute per-channel position of the next *ideal* analysis grain start.
    read_pos: f64,
    /// The "natural continuation" target (mono), length `synth_hop`: what would
    /// follow the previously chosen grain if we just kept reading. The next grain's
    /// leading overlap region is matched against this.
    target: Vec<f32>,
    /// Un-finalized tail of the synthesis overlap-add (interleaved, `synth_hop`
    /// frames) — the second half of the last grain, awaiting the next grain.
    tail: Vec<f32>,
    /// Finalized output not yet handed back to the caller, interleaved.
    out_hold: VecDeque<f32>,
    /// True once the first grain has primed `tail`/`target`.
    primed: bool,
}

impl WsolaTimeStretcher {
    /// Builds a stretcher for one fixed `(sample_rate, channels)`. Grain geometry
    /// is derived from the sample rate (~30 ms grain, ±12 ms search) so behavior
    /// is consistent across rates.
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".to_string());
        }
        if channels == 0 {
            return Err("channel count must be greater than zero".to_string());
        }

        // ~30 ms grain → synth_hop ~15 ms; ±12 ms similarity search (covers down
        // to ~80 Hz, i.e. low male voice). Clamp to sane minimums for tiny rates.
        let synth_hop = ((sample_rate as f32 * 0.015).round() as usize).max(64);
        let frame = synth_hop * 2;
        let search = ((sample_rate as f32 * 0.012).round() as usize).max(16);
        let window = hann(frame);

        Ok(Self {
            sample_rate,
            channels,
            params: TimeStretcherParams::default(),
            synth_hop,
            frame,
            search,
            window,
            in_buf: Vec::new(),
            in_start: 0,
            read_pos: 0.0,
            target: vec![0.0; synth_hop],
            tail: vec![0.0; synth_hop * channels],
            out_hold: VecDeque::new(),
            primed: false,
        })
    }

    fn format_matches(&self, sample_rate: u32, channels: usize) -> bool {
        self.sample_rate == sample_rate && self.channels == channels
    }

    /// Analysis hop `Ha = Hs * speed_ratio`. At `speed > 1` (faster playback) the
    /// analysis pointer advances faster than synthesis → shorter output.
    fn analysis_hop(&self) -> f64 {
        self.synth_hop as f64 * self.params.speed_ratio.max(f32::EPSILON) as f64
    }

    /// Per-channel frames currently buffered.
    fn buffered_frames(&self) -> usize {
        self.in_buf.len() / self.channels
    }

    /// Absolute per-channel frame just past the buffered input.
    fn buffered_end(&self) -> i64 {
        self.in_start + self.buffered_frames() as i64
    }

    /// One interleaved sample at absolute per-channel frame `abs`, channel `ch`.
    /// Returns 0 outside the buffered range (callers gate on availability first).
    #[inline]
    fn at(&self, abs: i64, ch: usize) -> f32 {
        let rel = abs - self.in_start;
        if rel < 0 || rel as usize >= self.buffered_frames() {
            return 0.0;
        }
        self.in_buf[rel as usize * self.channels + ch]
    }

    /// Mono mix at absolute per-channel frame `abs`.
    #[inline]
    fn mono(&self, abs: i64) -> f32 {
        let mut s = 0.0;
        for ch in 0..self.channels {
            s += self.at(abs, ch);
        }
        s / self.channels as f32
    }

    /// Produces finalized output into `out_hold` until either the output buffer is
    /// full or there isn't enough buffered input for the next grain. Returns true
    /// if it produced at least one grain.
    fn generate(&mut self, out_capacity_samples: usize) {
        loop {
            if self.out_hold.len() >= out_capacity_samples {
                break;
            }

            let ideal = self.read_pos.round() as i64;
            // We need lookahead for the search window, the grain, and one synth_hop
            // beyond it to build the next continuation target.
            let need_end = ideal + self.search as i64 + self.frame as i64 + self.synth_hop as i64;
            if need_end > self.buffered_end() {
                break; // wait for more input (or flush)
            }

            // Similarity search: pick the delta whose grain leading region best
            // matches the natural-continuation target. Clamp so reads stay in-buffer.
            let lo = (-(self.search as i64)).max(self.in_start - ideal);
            let hi = self.search as i64;
            let best_delta = if self.primed {
                self.best_match(ideal, lo, hi)
            } else {
                0
            };
            let start = ideal + best_delta;

            self.emit_grain(start);

            // Natural continuation for the next match: what follows this grain by
            // one synth_hop, as mono.
            for i in 0..self.synth_hop {
                let v = self.mono(start + self.synth_hop as i64 + i as i64);
                self.target[i] = v;
            }
            self.primed = true;

            self.read_pos += self.analysis_hop();
            self.trim();
        }
    }

    /// Finds the delta in `[lo, hi]` minimizing the squared error between the
    /// grain's leading `synth_hop` (mono) and the continuation `target`.
    fn best_match(&self, ideal: i64, lo: i64, hi: i64) -> i64 {
        let mut best_delta = 0;
        let mut best_err = f32::INFINITY;
        let mut delta = lo;
        while delta <= hi {
            let start = ideal + delta;
            let mut err = 0.0;
            for (i, &t) in self.target.iter().enumerate() {
                let d = self.mono(start + i as i64) - t;
                err += d * d;
                if err >= best_err {
                    break;
                }
            }
            if err < best_err {
                best_err = err;
                best_delta = delta;
            }
            delta += 1;
        }
        best_delta
    }

    /// Windows the grain at `start` and overlap-adds it: the leading `synth_hop`
    /// frames mix with `tail` and become finalized output; the trailing
    /// `synth_hop` frames become the new `tail`.
    fn emit_grain(&mut self, start: i64) {
        let h = self.synth_hop;
        let ch = self.channels;

        // Leading half: tail + windowed grain → finalized.
        for i in 0..h {
            for c in 0..ch {
                let w = self.window[i];
                let v = self.tail[i * ch + c] + w * self.at(start + i as i64, c);
                self.out_hold.push_back(v);
            }
        }
        // Trailing half: windowed grain → new tail.
        for i in 0..h {
            for c in 0..ch {
                let w = self.window[h + i];
                self.tail[i * ch + c] = w * self.at(start + (h + i) as i64, c);
            }
        }
    }

    /// Drops buffered input that no future grain can reach.
    fn trim(&mut self) {
        let keep_from = (self.read_pos.round() as i64 - self.search as i64).max(self.in_start);
        let drop = (keep_from - self.in_start) as usize;
        if drop > 0 {
            self.in_buf.drain(0..drop * self.channels);
            self.in_start += drop as i64;
        }
    }

    /// Moves up to `out_capacity_frames` from `out_hold` into `output`.
    fn drain(&mut self, output: &mut [f32], out_capacity_frames: usize) -> usize {
        let frames = out_capacity_frames.min(self.out_hold.len() / self.channels);
        for slot in output.iter_mut().take(frames * self.channels) {
            *slot = self.out_hold.pop_front().unwrap();
        }
        frames
    }
}

impl TimeStretcher for WsolaTimeStretcher {
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
        if out_capacity_frames == 0 {
            return TimeStretchProcessResult::default();
        }

        // Take the whole input block into our buffer, then produce what fits.
        self.in_buf
            .extend_from_slice(&input[..input_frames * channels]);
        self.generate(out_capacity_frames * channels);
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

        // Drain whatever is already finalized first.
        if !self.out_hold.is_empty() {
            return self.drain(output, out_capacity_frames);
        }

        // Nothing buffered to emit and no grains left to form: finalize the tail
        // (the un-overlapped second half of the last grain) once.
        if self.primed {
            let h = self.synth_hop;
            let ch = self.channels;
            for i in 0..h {
                for c in 0..ch {
                    self.out_hold.push_back(self.tail[i * ch + c]);
                }
            }
            self.tail.iter_mut().for_each(|t| *t = 0.0);
            self.primed = false;
            return self.drain(output, out_capacity_frames);
        }

        0
    }

    fn reset(&mut self) {
        self.in_buf.clear();
        self.in_start = 0;
        self.read_pos = 0.0;
        self.target.iter_mut().for_each(|t| *t = 0.0);
        self.tail.iter_mut().for_each(|t| *t = 0.0);
        self.out_hold.clear();
        self.primed = false;
    }

    fn latency(&self) -> Latency {
        // We require search + frame + synth_hop frames of lookahead before a grain
        // can be formed; report it as input-side latency.
        Latency::new(self.search + self.frame + self.synth_hop, 0, 0)
    }

    fn capabilities(&self) -> TimeStretcherCapabilities {
        TimeStretcherCapabilities {
            realtime: true,
            pitch_shift: false,
            time_stretch: true,
            independent_pitch_and_speed: false,
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
        // pitch_semitones is stored for honest `params()` reflection but not
        // applied: this backend is time-stretch only (see `capabilities`).
        self.params = TimeStretcherParams {
            speed_ratio,
            pitch_semitones,
        };
    }

    fn params(&self) -> TimeStretcherParams {
        self.params
    }
}

/// Periodic Hann window of length `n` (`w[i] = 0.5 - 0.5*cos(2πi/n)`), which sums
/// to a constant under 50% overlap so the overlap-add preserves amplitude.
fn hann(n: usize) -> Vec<f32> {
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

    const SR: u32 = 44_100;

    fn sine(freq: f32, frames: usize, channels: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * channels);
        for n in 0..frames {
            let s = (2.0 * std::f32::consts::PI * freq * n as f32 / SR as f32).sin();
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    /// Runs the stretcher to completion, returning interleaved output.
    fn run(st: &mut WsolaTimeStretcher, input: &[f32], channels: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 4096 * channels];
        // Feed input in blocks; drain output each call.
        let block = 2048 * channels;
        let mut pos = 0;
        while pos < input.len() {
            let end = (pos + block).min(input.len());
            let r = st.process(&input[pos..end], &mut buf, SR, channels);
            out.extend_from_slice(&buf[..r.output_frames_written * channels]);
            pos += r.input_frames_consumed.max(1) * channels;
        }
        loop {
            let n = st.flush(&mut buf, SR, channels);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n * channels]);
        }
        out
    }

    fn frames(v: &[f32], channels: usize) -> usize {
        v.len() / channels
    }

    /// Estimate frequency of a mono signal via zero-crossing count.
    fn est_freq(v: &[f32], channels: usize) -> f32 {
        let n = v.len() / channels;
        let mut crossings = 0;
        for i in 1..n {
            let a = v[(i - 1) * channels];
            let b = v[i * channels];
            if (a <= 0.0 && b > 0.0) || (a >= 0.0 && b < 0.0) {
                crossings += 1;
            }
        }
        // crossings ≈ 2 * cycles; freq = cycles * SR / n.
        (crossings as f32 / 2.0) * SR as f32 / n as f32
    }

    #[test]
    fn rejects_bad_format() {
        assert!(WsolaTimeStretcher::new(0, 1).is_err());
        assert!(WsolaTimeStretcher::new(SR, 0).is_err());
    }

    #[test]
    fn speed_one_preserves_length_approximately() {
        let mut st = WsolaTimeStretcher::new(SR, 1).unwrap();
        let input = sine(440.0, 44_100, 1); // 1 s
        let out = run(&mut st, &input, 1);
        let ratio = frames(&out, 1) as f32 / frames(&input, 1) as f32;
        assert!(
            (ratio - 1.0).abs() < 0.05,
            "speed=1 length ratio {ratio} should be ~1.0"
        );
    }

    #[test]
    fn slower_speed_lengthens_output() {
        let mut st = WsolaTimeStretcher::new(SR, 1).unwrap();
        st.set_params(TimeStretcherParams {
            speed_ratio: 0.5,
            pitch_semitones: 0.0,
        });
        let input = sine(440.0, 44_100, 1);
        let out = run(&mut st, &input, 1);
        let ratio = frames(&out, 1) as f32 / frames(&input, 1) as f32;
        // speed 0.5 → ~2x output length.
        assert!(ratio > 1.8 && ratio < 2.2, "ratio {ratio} should be ~2.0");
    }

    #[test]
    fn faster_speed_shortens_output() {
        let mut st = WsolaTimeStretcher::new(SR, 1).unwrap();
        st.set_params(TimeStretcherParams {
            speed_ratio: 2.0,
            pitch_semitones: 0.0,
        });
        let input = sine(440.0, 44_100, 1);
        let out = run(&mut st, &input, 1);
        let ratio = frames(&out, 1) as f32 / frames(&input, 1) as f32;
        assert!(ratio > 0.4 && ratio < 0.6, "ratio {ratio} should be ~0.5");
    }

    #[test]
    fn preserves_pitch_when_stretching() {
        let mut st = WsolaTimeStretcher::new(SR, 1).unwrap();
        st.set_params(TimeStretcherParams {
            speed_ratio: 0.5,
            pitch_semitones: 0.0,
        });
        let input = sine(440.0, 44_100, 1);
        let out = run(&mut st, &input, 1);
        let f = est_freq(&out, 1);
        // The defining property of TSM: time changed, pitch did not.
        assert!(
            (f - 440.0).abs() < 20.0,
            "stretched pitch {f} should stay ~440"
        );
    }

    #[test]
    fn output_is_finite_and_bounded() {
        let mut st = WsolaTimeStretcher::new(SR, 2).unwrap();
        st.set_params(TimeStretcherParams {
            speed_ratio: 0.7,
            pitch_semitones: 0.0,
        });
        let input = sine(220.0, 22_050, 2);
        let out = run(&mut st, &input, 2);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|s| s.is_finite() && s.abs() <= 1.5),
            "output must be finite and not blow up"
        );
    }

    #[test]
    fn silence_in_silence_out() {
        let mut st = WsolaTimeStretcher::new(SR, 1).unwrap();
        let input = vec![0.0f32; 44_100];
        let out = run(&mut st, &input, 1);
        assert!(out.iter().all(|&s| s == 0.0), "silence must stay silent");
    }

    #[test]
    fn reset_clears_state() {
        let input = sine(440.0, 20_000, 1);

        // A reused stretcher after reset must produce byte-identical output to a
        // brand-new one — that is what "reset clears state" means.
        let mut reused = WsolaTimeStretcher::new(SR, 1).unwrap();
        let _ = run(&mut reused, &input, 1);
        reused.reset();
        let after_reset = run(&mut reused, &input, 1);

        let mut fresh = WsolaTimeStretcher::new(SR, 1).unwrap();
        let from_fresh = run(&mut fresh, &input, 1);

        assert_eq!(
            after_reset, from_fresh,
            "reset must behave like a fresh build"
        );
    }

    #[test]
    fn capabilities_are_honest() {
        let st = WsolaTimeStretcher::new(SR, 1).unwrap();
        let caps = st.capabilities();
        assert!(caps.time_stretch);
        assert!(!caps.pitch_shift, "WSOLA increment 1 is time-stretch only");
        assert!(!caps.supports_realtime_autotune());
    }
}
