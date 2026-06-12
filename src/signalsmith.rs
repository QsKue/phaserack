use sinerack::Latency;

use crate::stretcher::{
    TimeStretchProcessResult, TimeStretcher, TimeStretcherCapabilities, TimeStretcherParams,
};

/// Signalsmith-backed general-purpose time stretcher and pitch shifter.
pub struct SignalSmithTimeStretcher {
    stretch: signalsmith_stretch::Stretch,
    channels: usize,
    sample_rate: u32,
    params: TimeStretcherParams,
    tail_frames_remaining: usize,
}

impl SignalSmithTimeStretcher {
    /// Builds the stretcher with signalsmith's **default preset** (block/interval derived from
    /// the sample rate). Lower latency, but the short FFT block limits pitch-shift accuracy at
    /// low frequencies — the resolution `Δf = sample_rate / block` is a large fraction of a low
    /// note, so a shift undershoots there (e.g. ~8¢ at C3, ~2¢ at A4). Use
    /// [`with_block_length`](Self::with_block_length) to trade latency for low-note accuracy.
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".to_string());
        }
        if channels == 0 {
            return Err("channel count must be greater than zero".to_string());
        }

        Ok(Self {
            stretch: signalsmith_stretch::Stretch::preset_default(channels as u32, sample_rate),
            channels,
            sample_rate,
            params: TimeStretcherParams::default(),
            tail_frames_remaining: 0,
        })
    }

    /// Builds the stretcher with an explicit FFT **`block_length`** (samples) and an interval of
    /// `block_length / 4` (signalsmith's conventional 4× overlap). A larger block sharpens
    /// low-frequency pitch-shift accuracy — the frequency resolution `Δf = sample_rate / block`
    /// shrinks, so low notes (few cycles per window) are resolved precisely — at the cost of
    /// proportionally more latency (~`block` input frames). Measured at 48 kHz: block 2880
    /// (~preset) ≈ 8¢ at C3, block 12288 ≈ 2¢, block 16384 ≈ 0¢ (see the shifter-accuracy test).
    pub fn with_block_length(
        sample_rate: u32,
        channels: usize,
        block_length: usize,
    ) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".to_string());
        }
        if channels == 0 {
            return Err("channel count must be greater than zero".to_string());
        }
        if block_length < 4 {
            return Err("block length must be at least 4 samples".to_string());
        }

        let interval = (block_length / 4).max(1);
        Ok(Self {
            stretch: signalsmith_stretch::Stretch::new(channels as u32, block_length, interval),
            channels,
            sample_rate,
            params: TimeStretcherParams::default(),
            tail_frames_remaining: 0,
        })
    }

    fn format_matches(&self, sample_rate: u32, channels: usize) -> bool {
        self.sample_rate == sample_rate && self.channels == channels
    }
}

impl TimeStretcher for SignalSmithTimeStretcher {
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
        let output_capacity_frames = output.len() / channels;
        if input_frames == 0 || output_capacity_frames == 0 {
            return TimeStretchProcessResult::default();
        }

        let speed = self.params.speed_ratio.max(f32::EPSILON);
        let consumed_frames = input_frames.min((output_capacity_frames as f32 * speed) as usize);
        if consumed_frames == 0 {
            return TimeStretchProcessResult::default();
        }
        let output_frames =
            ((consumed_frames as f32 / speed).round() as usize).clamp(1, output_capacity_frames);

        self.stretch.process(
            &input[..consumed_frames * channels],
            &mut output[..output_frames * channels],
        );
        self.tail_frames_remaining = self.stretch.output_latency();

        TimeStretchProcessResult {
            input_frames_consumed: consumed_frames,
            output_frames_written: output_frames,
        }
    }

    fn flush(&mut self, output: &mut [f32], sample_rate: u32, channels: usize) -> usize {
        if !self.format_matches(sample_rate, channels) || channels == 0 {
            return 0;
        }

        let frames = self.tail_frames_remaining.min(output.len() / channels);
        if frames == 0 {
            return 0;
        }

        self.stretch.flush(&mut output[..frames * channels]);
        self.tail_frames_remaining -= frames;
        frames
    }

    fn reset(&mut self) {
        self.stretch.reset();
        self.tail_frames_remaining = 0;
    }

    fn latency(&self) -> Latency {
        Latency::new(
            self.stretch.input_latency(),
            self.stretch.output_latency(),
            0,
        )
    }

    fn capabilities(&self) -> TimeStretcherCapabilities {
        TimeStretcherCapabilities {
            realtime: true,
            pitch_shift: true,
            time_stretch: true,
            independent_pitch_and_speed: true,
        }
    }

    fn set_params(&mut self, params: TimeStretcherParams) {
        self.params = TimeStretcherParams {
            speed_ratio: if params.speed_ratio.is_finite() {
                params.speed_ratio.max(f32::EPSILON)
            } else {
                1.0
            },
            pitch_semitones: if params.pitch_semitones.is_finite() {
                params.pitch_semitones
            } else {
                0.0
            },
        };
        self.stretch
            .set_transpose_factor_semitones(self.params.pitch_semitones, None);
    }

    fn params(&self) -> TimeStretcherParams {
        self.params
    }
}
