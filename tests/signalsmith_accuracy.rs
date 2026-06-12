#![cfg(feature = "signalsmith")]

//! Pitch-shift ACCURACY of the SignalSmith backend across pitch and block size.
//!
//! A phase vocoder's pitch precision is set by its FFT block: `Δf = sample_rate / block`.
//! At low frequencies a short block (the `preset_default`) resolves the note coarsely and a
//! shift undershoots; a longer block ([`SignalSmithTimeStretcher::with_block_length`]) fixes it
//! at the cost of latency. These tests pin that relationship so a regression is visible.

use phaserack::{SignalSmithTimeStretcher, TimeStretcher, TimeStretcherParams};

const SR: u32 = 48_000;

fn sine(hz: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (2.0 * std::f32::consts::PI * hz * n as f32 / SR as f32).sin())
        .collect()
}

/// Drive `stretcher` over `input` at a fixed pitch shift; return the output.
fn shift(mut s: SignalSmithTimeStretcher, input: &[f32], semitones: f32) -> Vec<f32> {
    s.set_params(TimeStretcherParams {
        speed_ratio: 1.0,
        pitch_semitones: semitones,
    });
    let mut out = Vec::with_capacity(input.len());
    let mut scratch = vec![0.0f32; 4096];
    let mut consumed = 0;
    while consumed < input.len() {
        let r = s.process(&input[consumed..], &mut scratch, SR, 1);
        if r.input_frames_consumed == 0 {
            break;
        }
        consumed += r.input_frames_consumed;
        out.extend_from_slice(&scratch[..r.output_frames_written]);
    }
    loop {
        let n = s.flush(&mut scratch, SR, 1);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&scratch[..n]);
    }
    out
}

/// Robust fundamental estimate via parabolic-interpolated autocorrelation, searching ±20%
/// around `expect_hz`. Uses the steady second half (skips the stretcher warm-up).
fn estimate_hz(signal: &[f32], expect_hz: f32) -> f32 {
    let tail = &signal[signal.len() / 2..];
    let n = tail.len();
    let center = (SR as f32 / expect_hz).round() as usize;
    let lo = (center as f32 * 0.8) as usize;
    let hi = ((center as f32 * 1.2) as usize).min(n - 2);
    let acf = |lag: usize| -> f64 {
        let mut s = 0.0f64;
        for i in 0..(n - lag) {
            s += tail[i] as f64 * tail[i + lag] as f64;
        }
        s
    };
    let mut best = lo;
    let mut best_v = acf(lo);
    for lag in (lo + 1)..=hi {
        let v = acf(lag);
        if v > best_v {
            best_v = v;
            best = lag;
        }
    }
    let (ym1, y0, yp1) = (acf(best - 1), acf(best), acf(best + 1));
    let denom = ym1 - 2.0 * y0 + yp1;
    let delta = if denom != 0.0 { 0.5 * (ym1 - yp1) / denom } else { 0.0 };
    SR as f32 / (best as f64 + delta) as f32
}

fn cents(hz: f32, target: f32) -> f32 {
    1200.0 * (hz / target).log2()
}

/// Shift `in_hz` by `semitones` and return the cents error of the result vs the ideal target.
fn shift_error_cents(stretcher: SignalSmithTimeStretcher, in_hz: f32, semitones: f32) -> f32 {
    let target = in_hz * 2f32.powf(semitones / 12.0);
    let out = shift(stretcher, &sine(in_hz, 6 * SR as usize), semitones);
    cents(estimate_hz(&out, target), target)
}

const C3: f32 = 130.81;
const A4: f32 = 440.0;

#[test]
fn high_notes_are_accurate_on_the_default_preset() {
    // A4 is well-resolved even by the short preset block.
    let err = shift_error_cents(
        SignalSmithTimeStretcher::new(SR, 1).unwrap(),
        A4 * 2f32.powf(0.35 / 12.0),
        -0.35,
    );
    assert!(err.abs() < 8.0, "A4 preset shift error {err:.1}c should be small");
}

#[test]
fn low_notes_undershoot_on_the_default_preset() {
    // The documented limitation: C3 on the short preset block undershoots noticeably.
    let err = shift_error_cents(
        SignalSmithTimeStretcher::new(SR, 1).unwrap(),
        C3 * 2f32.powf(0.35 / 12.0),
        -0.35,
    );
    assert!(
        err > 5.0,
        "C3 preset shift should undershoot (sharp); got {err:.1}c — if this fails the preset improved"
    );
}

#[test]
fn a_larger_block_makes_low_notes_accurate() {
    let in_hz = C3 * 2f32.powf(0.35 / 12.0);
    let preset = shift_error_cents(SignalSmithTimeStretcher::new(SR, 1).unwrap(), in_hz, -0.35);
    let big = shift_error_cents(
        SignalSmithTimeStretcher::with_block_length(SR, 1, 16_384).unwrap(),
        in_hz,
        -0.35,
    );
    assert!(
        big.abs() < 5.0,
        "C3 with a 16k block should land within 5c; got {big:.1}c"
    );
    assert!(
        big.abs() < preset.abs(),
        "the larger block ({big:.1}c) must beat the preset ({preset:.1}c) at C3"
    );
}
