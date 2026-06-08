use sinerack::Latency;

/// What a stretcher can do, so callers can pick one that fits (e.g. realtime
/// autotune needs independent pitch and speed control with low latency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeStretcherCapabilities {
    pub realtime: bool,
    pub pitch_shift: bool,
    pub time_stretch: bool,
    pub independent_pitch_and_speed: bool,
}

impl TimeStretcherCapabilities {
    pub const fn noop() -> Self {
        Self {
            realtime: true,
            pitch_shift: false,
            time_stretch: false,
            independent_pitch_and_speed: false,
        }
    }

    pub const fn supports_realtime_autotune(self) -> bool {
        self.realtime && self.pitch_shift && self.independent_pitch_and_speed
    }
}

/// The requested transform: playback speed and pitch shift, applied independently
/// by stretchers that advertise `independent_pitch_and_speed`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeStretcherParams {
    pub speed_ratio: f32,
    pub pitch_semitones: f32,
}

impl Default for TimeStretcherParams {
    fn default() -> Self {
        Self {
            speed_ratio: 1.0,
            pitch_semitones: 0.0,
        }
    }
}

/// How much input was consumed and output produced by a single `process` call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeStretchProcessResult {
    pub input_frames_consumed: usize,
    pub output_frames_written: usize,
}

/// A time-stretcher / pitch-shifter operating on interleaved `f32` audio.
pub trait TimeStretcher: Send {
    /// Processes interleaved input into a separate interleaved output buffer.
    /// Implementations may consume/write partial frame counts.
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        sample_rate: u32,
        channels: usize,
    ) -> TimeStretchProcessResult;

    /// Writes remaining tail frames after the final input block.
    fn flush(&mut self, output: &mut [f32], sample_rate: u32, channels: usize) -> usize;

    fn reset(&mut self);

    fn latency(&self) -> Latency;

    fn capabilities(&self) -> TimeStretcherCapabilities;

    fn set_params(&mut self, params: TimeStretcherParams);

    fn params(&self) -> TimeStretcherParams;
}

/// A pass-through stretcher: copies input to output unchanged. Useful as a
/// default and in tests.
pub struct NoopTimeStretcher;

impl TimeStretcher for NoopTimeStretcher {
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _sample_rate: u32,
        channels: usize,
    ) -> TimeStretchProcessResult {
        if channels == 0 {
            return TimeStretchProcessResult::default();
        }

        let frames = (input.len() / channels).min(output.len() / channels);
        let samples = frames * channels;
        output[..samples].copy_from_slice(&input[..samples]);
        TimeStretchProcessResult {
            input_frames_consumed: frames,
            output_frames_written: frames,
        }
    }

    fn flush(&mut self, _output: &mut [f32], _sample_rate: u32, _channels: usize) -> usize {
        0
    }

    fn reset(&mut self) {}

    fn latency(&self) -> Latency {
        Latency::default()
    }

    fn capabilities(&self) -> TimeStretcherCapabilities {
        TimeStretcherCapabilities::noop()
    }

    fn set_params(&mut self, _params: TimeStretcherParams) {}

    fn params(&self) -> TimeStretcherParams {
        TimeStretcherParams::default()
    }
}
