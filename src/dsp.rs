//! Realtime-safe toggle peak matching DSP.

use crate::params::{GainSnapParams, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::status::MatchState;

/// Time used to slew a newly requested boost into the audio path.
///
/// Attenuation is applied immediately when a newly observed peak requires it;
/// only upward correction is slewed. This keeps matching responsive without
/// allowing a quiet lead-in to leave a stale boost on a later loud sample.
pub const GAIN_SMOOTHING_SECONDS: f32 = 0.08;
/// Peak below this threshold is treated as no usable signal.
pub const SILENCE_PEAK_LINEAR: f32 = 1.0e-6;
/// Hard sample peak ceiling applied before writing a processed sample.
///
/// The guard is a bounded safety net for an unexpectedly large input or a
/// previously stored boost. It has no lookahead, latency, or envelope state;
/// ordinary samples are not changed by it.
pub const OUTPUT_PEAK_CEILING_LINEAR: f32 = 1.0;

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
    measurement_peak: f32,
    block_input_peak: f32,
    block_output_peak: f32,
    measurement_target_db: f32,
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
        let locked_gain_db = sanitize_gain_db(stored_gain_db);
        let gain = db_to_linear(locked_gain_db);
        Self {
            smoothing_coefficient: smoothing_coefficient.clamp(0.0001, 1.0),
            measurement_peak: 0.0,
            block_input_peak: 0.0,
            block_output_peak: 0.0,
            measurement_target_db: -12.0,
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
        self.locked_gain_db = sanitize_gain_db(params.locked_gain_db());
        self.current_gain = db_to_linear(self.locked_gain_db);
        self.target_gain = self.current_gain;
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
            self.state = MatchState::Measuring;
            self.previous_match_request = true;
        } else if self.state == MatchState::Measuring {
            let target_db = params.target_db();
            if (target_db - self.measurement_target_db).abs() > f32::EPSILON {
                // A target edit while Match is active starts a fresh
                // measurement. This keeps a Normalize action host-safe even
                // when a host coalesces same-block Match parameter events.
                self.measurement_peak = 0.0;
                self.measurement_target_db = target_db;
                self.target_gain = db_to_linear(self.locked_gain_db);
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
        _params: &GainSnapParams,
        input_left: f32,
        input_right: f32,
    ) -> (f32, f32) {
        let input_left = finite_or_zero(input_left);
        let input_right = finite_or_zero(input_right);
        let input_peak = input_left.abs().max(input_right.abs());
        self.block_input_peak = self.block_input_peak.max(input_peak);

        if self.state == MatchState::Measuring {
            self.measurement_peak = self.measurement_peak.max(input_peak);
            self.update_live_target_gain();
        }

        self.advance_gain();
        self.apply_sample_peak_guard(input_peak);
        let output_left = clamp_output_sample(input_left * self.current_gain);
        let output_right = clamp_output_sample(input_right * self.current_gain);
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
        if self.measurement_peak < SILENCE_PEAK_LINEAR {
            self.state = MatchState::NoSignal;
            return;
        }
        let measured_db = linear_to_db(self.measurement_peak);
        let gain_db = (self.measurement_target_db - measured_db).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        self.locked_gain_db = sanitize_gain_db(gain_db);
        self.target_gain = db_to_linear(self.locked_gain_db);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, self.locked_gain_db);
        self.state = MatchState::Locked;
    }

    /// Recalculate the desired live correction from the running input peak.
    ///
    /// The running maximum is intentionally never decayed while Match is
    /// active. A later louder sample can therefore only request attenuation,
    /// while a quiet lead-in cannot create a permanent +24 dB boost from
    /// silence.
    fn update_live_target_gain(&mut self) {
        if self.measurement_peak < SILENCE_PEAK_LINEAR {
            self.target_gain = db_to_linear(self.locked_gain_db);
            return;
        }
        let measured_db = linear_to_db(self.measurement_peak);
        let gain_db = (self.measurement_target_db - measured_db).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        self.target_gain = db_to_linear(gain_db);
    }

    /// Move the current gain toward the live target, slewing only boosts.
    fn advance_gain(&mut self) {
        if self.target_gain < self.current_gain {
            // Newly required attenuation must precede the current sample so a
            // newly observed peak cannot be amplified by stale gain.
            self.current_gain = self.target_gain;
        } else {
            self.current_gain +=
                (self.target_gain - self.current_gain) * self.smoothing_coefficient;
        }
        if !self.current_gain.is_finite() || self.current_gain < 0.0 {
            self.current_gain = 1.0;
            self.target_gain = 1.0;
            self.locked_gain_db = 0.0;
        }
    }

    /// Cap a sample's gain before multiplication when it would exceed the
    /// finite output ceiling. This is deliberately a per-sample guard rather
    /// than a compressor or lookahead stage.
    fn apply_sample_peak_guard(&mut self, input_peak: f32) {
        if !input_peak.is_finite() || input_peak <= SILENCE_PEAK_LINEAR {
            return;
        }
        let safe_gain = OUTPUT_PEAK_CEILING_LINEAR / input_peak;
        if safe_gain.is_finite() && safe_gain < self.current_gain {
            // Keep this intervention transient: the requested target remains
            // intact so the normal upward slew can recover on quieter samples.
            self.current_gain = safe_gain.max(0.0);
        }
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

fn clamp_output_sample(value: f32) -> f32 {
    finite_or_zero(value).clamp(-OUTPUT_PEAK_CEILING_LINEAR, OUTPUT_PEAK_CEILING_LINEAR)
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
    use crate::params::{PARAM_LOCKED_GAIN_DB, PARAM_MATCH, PARAM_TARGET_DB};

    fn run_frames(engine: &mut GainSnapEngine, params: &GainSnapParams, left: f32, frames: usize) {
        for _ in 0..frames {
            let _ = engine.process_frame(params, left, left);
        }
    }

    #[test]
    fn turning_match_off_calculates_gain_and_holds_it() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 500);

        assert_eq!(engine.report().state, MatchState::Measuring);
        assert_eq!(engine.report().progress, 0.0);
        assert!((engine.report().locked_gain_db - 0.0).abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        let report = engine.report();
        assert_eq!(report.state, MatchState::Locked);
        assert!((report.locked_gain_db - (-5.9794)).abs() < 0.02);
        assert!((params.locked_gain_db() - report.locked_gain_db).abs() < 0.001);

        run_frames(&mut engine, &params, 0.25, 200);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
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

        // The peak arrives after the old fixed 0.5 second window. It must
        // still participate in the result while Match remains enabled.
        run_frames(&mut engine, &params, 0.5, 2_000);
        assert_eq!(engine.report().state, MatchState::Measuring);

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
        run_frames(&mut engine, &params, 0.25, 100);

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
    fn matching_applies_live_correction_while_preserving_final_lock() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);

        let first_output = engine.process_frame(&params, 0.5, 0.5);
        assert!((first_output.0 - 0.2506).abs() < 0.001);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((engine.report().output_peak_db - (-12.0)).abs() < 0.02);
        // The exposed lock remains unchanged until Match is turned off.
        assert!((engine.report().locked_gain_db - 0.0).abs() < 0.001);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
    }

    #[test]
    fn live_boost_slews_and_a_new_peak_is_attenuated_before_processing() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);

        // A quiet lead-in requests a large boost, but the 80 ms upward slew
        // keeps it from arriving as an abrupt jump.
        let quiet_output = engine.process_frame(&params, 0.01, 0.01);
        assert!(quiet_output.0 > 0.01);
        assert!(quiet_output.0 < 0.02);

        // A later full-scale peak updates the running maximum and lowers the
        // gain before that sample is multiplied by the stale boost.
        let loud_output = engine.process_frame(&params, 1.0, 1.0);
        assert!(loud_output.0.is_finite());
        assert!(loud_output.0.abs() <= OUTPUT_PEAK_CEILING_LINEAR);
        assert!(engine.current_gain <= db_to_linear(-12.0) + f32::EPSILON);
    }

    #[test]
    fn output_guard_caps_stored_boost_without_nan_or_clipping() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_LOCKED_GAIN_DB, GAIN_MAX_DB);
        let mut engine = GainSnapEngine::new(48_000.0, GAIN_MAX_DB);
        engine.begin_block(&params);

        let output = engine.process_frame(&params, 1.0, -1.0);
        assert!(output.0.is_finite() && output.1.is_finite());
        assert!(output.0.abs() <= OUTPUT_PEAK_CEILING_LINEAR);
        assert!(output.1.abs() <= OUTPUT_PEAK_CEILING_LINEAR);

        // The full-scale sample is capped without rewriting the requested
        // stored boost; a quieter run can recover toward that target through
        // the bounded upward slew.
        assert!((engine.target_gain - db_to_linear(GAIN_MAX_DB)).abs() < f32::EPSILON);
        let capped_gain = engine.current_gain;
        run_frames(&mut engine, &params, 0.1, 256);
        assert!(engine.current_gain > capped_gain);
        assert!(engine.current_gain < db_to_linear(GAIN_MAX_DB));
    }
}
