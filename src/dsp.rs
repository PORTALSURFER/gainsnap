//! Realtime-safe toggle peak matching DSP.

use crate::params::{GainSnapParams, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::status::MatchState;

/// Time used to slew a newly calculated gain into the audio path.
pub const GAIN_SMOOTHING_SECONDS: f32 = 0.01;
/// Gain increases settle more slowly than reductions to avoid sudden boosts.
pub const GAIN_INCREASE_SECONDS: f32 = 0.1;
/// Fade from silence when Match is engaged, starting with the first signal.
pub const MATCH_FADE_SECONDS: f32 = 0.3;
/// Recovery time for the stereo-linked, instantaneous-attack sample peak guard.
pub const PEAK_GUARD_RELEASE_SECONDS: f32 = 0.1;
/// Peak below this threshold is treated as no usable signal.
pub const SILENCE_PEAK_LINEAR: f32 = 1.0e-6;

/// Audio-block status returned by the engine.
#[derive(Clone, Copy, Debug)]
pub struct EngineReport {
    /// Highest finite input sample seen in the block, in dBFS.
    pub input_peak_db: f32,
    /// Highest finite output sample generated in the block, in dBFS.
    pub output_peak_db: f32,
    /// Measurement progress from zero to one.
    ///
    /// An active measurement has no predetermined completion point, so this
    /// remains zero until the Match toggle is turned off and the result is
    /// finalized.
    pub progress: f32,
    /// Current matcher state.
    pub state: MatchState,
    /// Last calculated gain correction in dB.
    pub locked_gain_db: f32,
}

/// Per-instance, audio-thread-owned matcher and gain smoother.
pub struct GainSnapEngine {
    smoothing_coefficient: f32,
    gain_increase_coefficient: f32,
    peak_guard_release_coefficient: f64,
    peak_guard_gain: f64,
    match_fade_step: f32,
    match_fade_position: f32,
    measurement_peak: f32,
    block_input_peak: f32,
    block_output_peak: f32,
    measurement_target_db: f32,
    measurement_target_peak: f32,
    locked_gain_db: f32,
    current_gain: f32,
    target_gain: f32,
    previous_match_request: bool,
    state: MatchState,
}

impl GainSnapEngine {
    /// Construct a matcher at the host sample rate with the stored gain.
    pub fn new(sample_rate: f32, stored_gain_db: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() {
            sample_rate.max(1.0)
        } else {
            48_000.0
        };
        let smoothing_coefficient = 1.0 - (-1.0 / (sample_rate * GAIN_SMOOTHING_SECONDS)).exp();
        let gain_increase_coefficient = 1.0 - (-1.0 / (sample_rate * GAIN_INCREASE_SECONDS)).exp();
        let peak_guard_release_coefficient =
            -(-1.0 / (sample_rate as f64 * PEAK_GUARD_RELEASE_SECONDS as f64)).exp_m1();
        let locked_gain_db = sanitize_gain_db(stored_gain_db);
        let gain = db_to_linear(locked_gain_db);
        Self {
            smoothing_coefficient: smoothing_coefficient.clamp(0.0001, 1.0),
            gain_increase_coefficient,
            peak_guard_release_coefficient,
            peak_guard_gain: 1.0,
            match_fade_step: 1.0 / (sample_rate * MATCH_FADE_SECONDS),
            match_fade_position: 1.0,
            measurement_peak: 0.0,
            block_input_peak: 0.0,
            block_output_peak: 0.0,
            measurement_target_db: -12.0,
            measurement_target_peak: 10.0_f32.powf(-12.0 / 20.0),
            locked_gain_db,
            current_gain: gain,
            target_gain: gain,
            previous_match_request: false,
            state: MatchState::Ready,
        }
    }

    /// Reset per-block metering and synchronize controls at a block boundary.
    pub fn begin_block(&mut self, params: &GainSnapParams) {
        self.block_input_peak = 0.0;
        self.block_output_peak = 0.0;
        self.sync_controls(params);
    }

    /// Reset transient measurement state at a host processing boundary.
    #[cfg(feature = "vst3")]
    pub fn reset(&mut self, params: &GainSnapParams) {
        self.measurement_peak = 0.0;
        self.block_input_peak = 0.0;
        self.block_output_peak = 0.0;
        self.measurement_target_db = params.target_db();
        self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
        self.locked_gain_db = sanitize_gain_db(params.locked_gain_db());
        self.current_gain = db_to_linear(self.locked_gain_db);
        self.target_gain = self.current_gain;
        self.peak_guard_gain = 1.0;
        self.match_fade_position = 1.0;
        self.previous_match_request = false;
        self.state = MatchState::Ready;
    }

    /// Apply control changes after a sample-offset parameter event.
    pub fn sync_controls(&mut self, params: &GainSnapParams) {
        let request = params.match_requested();
        let mut measurement_finished = false;

        if !request {
            if self.previous_match_request && self.state == MatchState::Measuring {
                self.finish_measurement(params);
                measurement_finished = true;
            }
            self.previous_match_request = false;
        } else if !self.previous_match_request {
            self.measurement_peak = 0.0;
            self.measurement_target_db = params.target_db();
            self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
            self.state = MatchState::Measuring;
            self.previous_match_request = true;
            self.match_fade_position = 0.0;
        } else if self.state == MatchState::Measuring {
            let target_db = params.target_db();
            if (target_db - self.measurement_target_db).abs() > f32::EPSILON {
                // A target edit while Match is active starts a fresh
                // measurement. This keeps a Normalize action host-safe even
                // when a host coalesces same-block Match parameter events.
                self.measurement_peak = 0.0;
                self.measurement_target_db = target_db;
                self.measurement_target_peak = 10.0_f32.powf(target_db / 20.0);
            }
        }

        let stored_gain_db = params.locked_gain_db();
        if !measurement_finished
            && self.state != MatchState::Measuring
            && (stored_gain_db - self.locked_gain_db).abs() > 0.0001
        {
            self.locked_gain_db = stored_gain_db;
            self.target_gain = db_to_linear(stored_gain_db);
            self.current_gain = self.target_gain;
        }
    }

    /// Process one stereo frame without allocating, locking, or blocking.
    pub fn process_frame(
        &mut self,
        params: &GainSnapParams,
        input_left: f32,
        input_right: f32,
    ) -> (f32, f32) {
        let input_left = finite_or_zero(input_left);
        let input_right = finite_or_zero(input_right);
        let input_peak = input_left.abs().max(input_right.abs());
        self.block_input_peak = self.block_input_peak.max(input_peak);

        if self.state == MatchState::Measuring && input_peak > self.measurement_peak {
            self.measurement_peak = input_peak;
            if self.measurement_peak > SILENCE_PEAK_LINEAR {
                self.apply_measurement_gain(params);
            }
        }

        let coefficient = if self.target_gain > self.current_gain {
            self.gain_increase_coefficient
        } else {
            self.smoothing_coefficient
        };
        self.current_gain += (self.target_gain - self.current_gain) * coefficient;
        if !self.current_gain.is_finite() {
            self.current_gain = 1.0;
            self.target_gain = 1.0;
            self.locked_gain_db = 0.0;
        }
        // The fade starts on the first usable signal, not on silent transport
        // preroll. Finishing Match early keeps the fade instead of bypassing it.
        let position = self.match_fade_position;
        let fade = position * position * (3.0 - 2.0 * position);
        if self.state != MatchState::Measuring || self.measurement_peak > SILENCE_PEAK_LINEAR {
            self.match_fade_position = (position + self.match_fade_step).min(1.0);
        }
        let desired_gain = self.current_gain;
        let ceiling = if self.state == MatchState::Measuring || position < 1.0 {
            // Target peaks extend below the correction-gain range, so do not
            // use db_to_linear(), which clamps to the gain parameter bounds.
            self.measurement_target_peak
        } else {
            1.0
        };
        // Divide before multiplying: even a finite f32::MAX input with a
        // stored boost must be attenuated before it can overflow the output.
        let allowed_gain = if input_peak > 0.0 {
            ceiling / input_peak
        } else {
            f32::MAX
        };
        let required_guard = if desired_gain > allowed_gain {
            allowed_gain / desired_gain
        } else {
            1.0
        };
        // Higher precision lets the release settle back to unity at high host
        // sample rates instead of stalling slightly below the matched target.
        self.peak_guard_gain = (required_guard as f64).min(
            self.peak_guard_gain
                + (1.0 - self.peak_guard_gain) * self.peak_guard_release_coefficient,
        );
        let applied_gain = (desired_gain * self.peak_guard_gain as f32).min(allowed_gain);
        // The final clamp catches rounding at the ceiling; attenuation itself
        // uses the same gain for both channels, preserving their balance.
        // Fade the protected samples, not the requested gain. Otherwise a
        // strong input can hit the fixed ceiling while the fade is still low.
        let output_left = (input_left * applied_gain).clamp(-ceiling, ceiling) * fade;
        let output_right = (input_right * applied_gain).clamp(-ceiling, ceiling) * fade;
        self.block_output_peak = self
            .block_output_peak
            .max(output_left.abs().max(output_right.abs()));
        (output_left, output_right)
    }

    /// Return this block's metering and matcher state.
    pub fn report(&self) -> EngineReport {
        let progress: f32 = if self.state == MatchState::Measuring {
            0.0
        } else {
            1.0
        };
        EngineReport {
            input_peak_db: linear_to_db(self.block_input_peak),
            output_peak_db: linear_to_db(self.block_output_peak),
            progress: progress.clamp(0.0, 1.0),
            state: self.state,
            locked_gain_db: self.locked_gain_db,
        }
    }

    fn finish_measurement(&mut self, params: &GainSnapParams) {
        if self.measurement_peak <= SILENCE_PEAK_LINEAR {
            self.state = MatchState::NoSignal;
            return;
        }
        self.apply_measurement_gain(params);
        self.state = MatchState::Locked;
    }

    // Update from the unmodified input peak so the applied correction cannot
    // feed back into the measurement. Only new peaks need a recalculation.
    fn apply_measurement_gain(&mut self, params: &GainSnapParams) {
        let measured_db = linear_to_db(self.measurement_peak);
        let gain_db = (self.measurement_target_db - measured_db).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        self.locked_gain_db = sanitize_gain_db(gain_db);
        self.target_gain = db_to_linear(self.locked_gain_db);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, self.locked_gain_db);
    }
}

/// Convert decibels to a finite linear gain.
pub fn db_to_linear(db: f32) -> f32 {
    if db.is_finite() {
        (10.0_f32.powf(db.clamp(GAIN_MIN_DB, GAIN_MAX_DB) / 20.0)).max(0.0)
    } else {
        1.0
    }
}

/// Convert a positive linear peak to decibels, with finite silence handling.
pub fn linear_to_db(linear: f32) -> f32 {
    if linear.is_finite() && linear > SILENCE_PEAK_LINEAR {
        (20.0 * linear.log10()).clamp(-120.0, 24.0)
    } else {
        -120.0
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn sanitize_gain_db(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(GAIN_MIN_DB, GAIN_MAX_DB)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{PARAM_MATCH, PARAM_TARGET_DB};

    fn run_frames(engine: &mut GainSnapEngine, params: &GainSnapParams, left: f32, frames: usize) {
        for _ in 0..frames {
            let _ = engine.process_frame(params, left, left);
        }
    }

    #[test]
    fn matching_applies_gain_live_and_turning_it_off_holds_it() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 1_200);

        assert_eq!(engine.report().state, MatchState::Measuring);
        assert_eq!(engine.report().progress, 0.0);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
        assert!((params.locked_gain_db() - engine.report().locked_gain_db).abs() < 0.001);
        // Read a fresh block after the slew settles: telemetry must measure
        // the samples we actually output while Match is still enabled.
        engine.begin_block(&params);
        let (left, right) = engine.process_frame(&params, 0.5, -0.5);
        assert!((linear_to_db(left) - (-12.0)).abs() < 0.001);
        assert_eq!(right, -left);
        assert!((engine.report().output_peak_db - (-12.0)).abs() < 0.001);
        assert!((engine.report().input_peak_db - (-6.0206)).abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        let report = engine.report();
        assert_eq!(report.state, MatchState::Locked);
        assert!((report.locked_gain_db - (-5.9794)).abs() < 0.02);
        assert!((params.locked_gain_db() - report.locked_gain_db).abs() < 0.001);

        run_frames(&mut engine, &params, 0.25, 200);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
        assert!((engine.report().output_peak_db - (-18.0206)).abs() < 0.001);
    }

    #[test]
    fn extended_measurement_keeps_later_peak_until_toggle_off() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 500);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);

        // The peak arrives after the old fixed 0.5 second window. It must
        // still participate in the result while Match remains enabled.
        run_frames(&mut engine, &params, 0.5, 2_000);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 64);
        assert!((engine.report().output_peak_db + 12.0).abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
    }

    #[test]
    fn turning_match_on_again_starts_a_fresh_measurement() {
        let params = GainSnapParams::new();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);

        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 100);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);

        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 100);
        assert_eq!(engine.report().state, MatchState::Measuring);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
    }

    #[test]
    fn changing_target_while_matching_restarts_measurement_with_new_target() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);

        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 100);

        // Normalize changes the target while Match is already enabled. The
        // engine must discard the old peak even if the host delivers only the
        // final Match=true state to the next processing block.
        params.set_param(PARAM_TARGET_DB, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 1_200);

        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 64);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!(engine.report().output_peak_db.abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);

        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 12.0412).abs() < 0.02);
    }

    #[test]
    fn silence_does_not_create_unbounded_gain() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 3.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.0, 10_000);
        assert_eq!(engine.report().state, MatchState::Measuring);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::NoSignal);
        assert!((engine.report().locked_gain_db - 3.0).abs() < 0.001);
    }

    #[test]
    fn non_finite_audio_is_silenced() {
        let params = GainSnapParams::new();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let output = engine.process_frame(&params, f32::NAN, f32::INFINITY);
        assert_eq!(output, (0.0, 0.0));
        assert!(engine.report().input_peak_db <= -119.0);
    }

    #[test]
    fn live_match_slews_and_toggle_off_does_not_change_audio() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let first = engine.process_frame(&params, 0.5, 0.25).0;
        assert_eq!(first, 0.0);
        let mut previous = first;
        for _ in 0..48_000 {
            let (left, right) = engine.process_frame(&params, 0.5, 0.25);
            assert!(left >= previous);
            assert_eq!(right, left * 0.5);
            previous = left;
        }
        assert!((linear_to_db(previous) + 12.0).abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        let held = engine.process_frame(&params, 0.5, 0.25).0;
        assert!((held - previous).abs() < 1.0e-6);
    }

    #[test]
    fn live_match_keeps_gain_bounded_and_ignores_silence_and_invalid_samples() {
        for (input, expected_gain) in [(1.0e-5, GAIN_MAX_DB), (16.0, GAIN_MIN_DB)] {
            let params = GainSnapParams::new();
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, input, 9_600);
            assert_eq!(engine.report().locked_gain_db, expected_gain);
            let output = engine.process_frame(&params, f32::NAN, f32::INFINITY);
            assert_eq!(output, (0.0, 0.0));
            run_frames(&mut engine, &params, 0.0, 9_600);
            assert_eq!(params.locked_gain_db(), expected_gain);
            assert!(engine.report().output_peak_db.is_finite());
        }
    }

    #[test]
    fn match_fades_from_silence_even_after_preroll_with_a_stored_boost() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let params = GainSnapParams::new();
            params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 24.0);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(sample_rate, 24.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, 0.0, sample_rate as usize);
            let ceiling = 10.0_f32.powf(-12.0 / 20.0);
            let mut output = 0.0;
            for frame in 0..(sample_rate * 1.5) as usize {
                if frame % 64 == 0 {
                    engine.begin_block(&params);
                }
                let (left, right) = engine.process_frame(&params, 1.0, -0.5);
                assert!(left <= ceiling);
                assert_eq!(right, -left * 0.5);
                if frame == 0 {
                    assert_eq!(left, 0.0);
                }
                if frame == (sample_rate * 0.1) as usize {
                    assert!(left < ceiling * 0.3 && left > 0.0);
                }
                output = left;
            }
            assert!((linear_to_db(output) + 12.0).abs() < 0.002);
        }
    }

    #[test]
    fn a_loud_burst_after_quiet_matching_cannot_exceed_the_target() {
        for target in [-36.0, -12.0, 0.0] {
            let params = GainSnapParams::new();
            params.set_param(PARAM_TARGET_DB, target);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            // Give the matcher time to learn a large boost on quiet audio.
            run_frames(&mut engine, &params, 0.0001, 96_000);
            assert_eq!(params.locked_gain_db(), GAIN_MAX_DB);
            engine.begin_block(&params);
            let ceiling = 10.0_f32.powf(target / 20.0);
            let mut actual_peak = 0.0_f32;
            for input in [1.0, -1.0, 8.0, -8.0, f32::MAX, -f32::MAX] {
                let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                assert!(left.is_finite() && right.is_finite());
                assert!(left.abs() <= ceiling && right.abs() <= ceiling);
                assert!((right + left * 0.5).abs() < 1.0e-6);
                actual_peak = actual_peak.max(left.abs()).max(right.abs());
            }
            assert_eq!(engine.report().output_peak_db, linear_to_db(actual_peak));
        }
    }

    #[test]
    fn extreme_startup_peaks_follow_the_fade_ceiling() {
        for input in [8.0, f32::MAX] {
            let params = GainSnapParams::new();
            params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 24.0);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 24.0);
            engine.begin_block(&params);
            let ceiling = 10.0_f32.powf(-12.0 / 20.0);
            for frame in 0..14_400 {
                let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                let position = frame as f32 / 14_400.0;
                let envelope = position * position * (3.0 - 2.0 * position);
                assert!(
                    left <= ceiling * envelope + 0.0001,
                    "startup escaped the fade at sample {frame}: {left}"
                );
                assert!((right + left * 0.5).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn output_guard_protects_held_gain_and_recovers_smoothly_without_boosting() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 24.0);
        let mut engine = GainSnapEngine::new(48_000.0, 24.0);
        engine.begin_block(&params);
        let (left, right) = engine.process_frame(&params, 8.0, -4.0);
        assert!(left <= 1.0 && left > 0.99);
        assert_eq!(right, -left * 0.5);
        let mut previous = 0.0;
        for _ in 0..96_000 {
            let left = engine.process_frame(&params, 0.001, 0.0).0;
            assert!(left >= previous);
            assert!(left <= 0.001 * db_to_linear(24.0));
            previous = left;
        }
        assert!((linear_to_db(previous) - (-36.0)).abs() < 0.002);
    }

    #[test]
    fn early_match_off_and_reengagement_keep_the_quiet_start() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 1.0, 480);
        let before_off = engine.process_frame(&params, 1.0, 1.0).0;
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        let after_off = engine.process_frame(&params, 1.0, 1.0).0;
        assert!((after_off - before_off).abs() < 0.0001);
        assert!(after_off < 0.01);

        run_frames(&mut engine, &params, 1.0, 48_000);
        params.set_param(PARAM_MATCH, 1.0);
        engine.sync_controls(&params);
        assert_eq!(engine.process_frame(&params, 1.0, -1.0), (0.0, -0.0));
    }

    #[test]
    fn target_edits_enforce_the_new_ceiling_at_the_event_boundary() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 1.0, 48_000);
        params.set_param(PARAM_TARGET_DB, -36.0);
        engine.sync_controls(&params);
        let (left, right) = engine.process_frame(&params, 1.0, 0.5);
        assert!(left <= 10.0_f32.powf(-36.0 / 20.0));
        assert_eq!(right, left * 0.5);
    }

    #[test]
    fn disabling_match_without_signal_does_not_leave_output_muted() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.0, 48_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        assert_eq!(engine.report().state, MatchState::NoSignal);
        run_frames(&mut engine, &params, 0.1, 48_000);
        assert_eq!(engine.process_frame(&params, 0.1, -0.1), (0.1, -0.1));
    }

    #[test]
    #[cfg(feature = "vst3")]
    fn host_processing_reset_restarts_the_match_fade() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 48_000);
        engine.reset(&params);
        engine.begin_block(&params);
        assert_eq!(engine.process_frame(&params, 1.0, -1.0), (0.0, -0.0));
    }
}
