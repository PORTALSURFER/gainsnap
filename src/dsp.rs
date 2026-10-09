//! Realtime-safe Peak/RMS matching DSP.

use crate::params::{GainSnapParams, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::status::{MatchActivity, MatchState};

/// Time used to slew a newly calculated gain into the audio path.
pub const GAIN_SMOOTHING_SECONDS: f32 = 0.01;
/// Gain increases settle more slowly than reductions to avoid sudden boosts.
pub const GAIN_INCREASE_SECONDS: f32 = 0.1;
/// Observe this much audio after the first signal before applying a match.
pub const MATCH_LISTEN_SECONDS: f32 = 1.0;
/// Smoothly blend the audible gain into the measured correction.
pub const MATCH_TRANSITION_SECONDS: f32 = 0.3;
/// Length of each full RMS measurement window.
pub const RMS_AVERAGING_SECONDS: f32 = 0.3;
/// Host rates above this are treated as this rate to keep constructor storage bounded.
pub const MAX_SUPPORTED_SAMPLE_RATE: f32 = 384_000.0;

/// Peak below this threshold is treated as no usable signal.
pub const SILENCE_PEAK_LINEAR: f32 = 1.0e-6;

/// Initial evidence must remain within 0.5 dB for this long.
const MATCH_CONFIDENCE_STABLE_SECONDS: f32 = 0.5;
const MATCH_EVIDENCE_INTERVAL_SECONDS: f32 = 0.1;
/// Bound automatic gain movement in both directions, including later corrections.
const MATCH_SLEW_DB_PER_SECOND: f32 = 24.0;
const MATCH_ACTIVITY_HOLD_SECONDS: f32 = 0.2;
const MATCH_ADAPTATION_STABILITY_RATIO: f32 = 1.059_253_7;
const MATCH_ACTIVITY_MEANINGFUL_GAIN_RATIO: f32 = 1.001_151_3;
const GAIN_MIN_LINEAR: f32 = 0.001;
// 10^(24 dB / 20), retained for the RMS path's existing boost limit.
const RMS_GAIN_MAX_LINEAR: f32 = 15.848_932;
const PEAK_GAIN_MAX_LINEAR: f32 = 1_000_000.0;

/// Constructor-owned, stereo-linked sliding mean-square window.
///
/// `sum` uses compensated additions because a hostile, but finite, sample can
/// otherwise erase ordinary program material when it leaves the ring.
struct RmsDetector {
    samples: Vec<[f64; 2]>,
    sum: [f64; 2],
    compensation: [f64; 2],
    next: usize,
    count: usize,
}
impl RmsDetector {
    fn new(window_frames: usize) -> Self {
        Self {
            samples: vec![[0.0; 2]; window_frames.max(1)],
            sum: [0.0; 2],
            compensation: [0.0; 2],
            next: 0,
            count: 0,
        }
    }

    fn update(&mut self, left: f32, right: f32) {
        let incoming = [(left as f64).powi(2), (right as f64).powi(2)];
        let outgoing = if self.count == self.samples.len() {
            self.samples[self.next]
        } else {
            [0.0; 2]
        };
        for channel in 0..2 {
            // Keep removal and insertion separate. Forming their difference
            // first can turn a huge outgoing sample and a tiny incoming one
            // into a cancellation that loses the retained ordinary power.
            self.add(channel, -outgoing[channel]);
            self.add(channel, incoming[channel]);
        }
        self.samples[self.next] = incoming;
        self.next = (self.next + 1) % self.samples.len();
        self.count = self.count.saturating_add(1).min(self.samples.len());
    }

    fn add(&mut self, channel: usize, value: f64) {
        let sum = self.sum[channel];
        let next = sum + value;
        let correction = if sum.abs() >= value.abs() {
            (sum - next) + value
        } else {
            (value - next) + sum
        };
        self.compensation[channel] += correction;
        self.sum[channel] = next;
    }

    fn complete(&self) -> bool {
        self.count == self.samples.len()
    }

    fn level(&self) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        ((self.sum[0] + self.compensation[0])
            .max(self.sum[1] + self.compensation[1])
            .max(0.0)
            / self.count as f64)
            .sqrt() as f32
    }

    /// Reset logical state without touching the constructor-owned ring storage.
    fn reset(&mut self) {
        self.sum = [0.0; 2];
        self.compensation = [0.0; 2];
        self.next = 0;
        self.count = 0;
    }
}

/// Audio-block status returned by the engine.
#[derive(Clone, Copy, Debug)]
pub struct EngineReport {
    /// Highest finite input sample seen in the block, in dBFS.
    pub input_peak_db: f32,
    /// Highest finite output sample generated in the block, in dBFS.
    pub output_peak_db: f32,
    /// Protected output RMS in dBFS, using the louder channel.
    pub output_rms_db: f32,
    /// Measurement progress from zero to one.
    ///
    /// An active measurement has no predetermined completion point, so this
    /// remains zero until the Match toggle is turned off and the result is
    /// finalized.
    pub progress: f32,
    /// Current matcher state.
    pub state: MatchState,
    /// Live audio activity, independent from the matcher lifecycle state.
    pub activity: MatchActivity,
    /// Last calculated gain correction in dB.
    pub locked_gain_db: f32,
    /// Gain actually applied after smoothing.
    pub applied_gain_db: f32,
    /// RMS target shortfall from retained measurement and available correction.
    pub target_shortfall_db: f32,
}

/// Per-instance, audio-thread-owned matcher and gain smoother.
pub struct GainSnapEngine {
    manual_mode: bool,
    rms_mode: bool,
    measurement_rms_mode: bool,
    rms_silence_frames: u64,
    rms_frames: u64,
    rms_quiet_frames: u64,
    input_rms: RmsDetector,
    output_rms: RmsDetector,
    measurement_level: f32,
    session_peak: f32,
    activity_adjustment_hold_limit: u64,
    activity_adjustment_hold_frames: u64,
    session_rms: f32,
    measurement_gain_linear: f32,
    measurement_rms_candidate_seen: bool,
    force_measurement_update: bool,
    smoothing_coefficient: f64,
    gain_increase_coefficient: f64,
    startup_window_frames: u64,
    startup_observed_frames: u64,
    confidence_stable_frames: u64,
    confidence_stable_limit: u64,
    confidence_level: f32,
    evidence_interval_frames: u64,
    evidence_since_observation: u64,
    evidence_observations: u8,
    match_slew_ratio: f64,
    match_transition_step: f32,
    match_transition_position: f32,
    match_start_gain: f32,
    last_output_gain: f32,
    block_input_peak: f32,
    block_output_peak: f32,
    activity_silence_frames: u64,
    measurement_target_db: f32,
    measurement_target_peak: f32,
    locked_gain_db: f32,
    current_gain: f64,
    target_gain: f64,
    previous_match_request: bool,
    restart_generation: u32,
    state: MatchState,
}

impl GainSnapEngine {
    /// Construct a matcher at the host sample rate with the stored gain.
    pub fn new(sample_rate: f32, stored_gain_db: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() {
            sample_rate.clamp(1.0, MAX_SUPPORTED_SAMPLE_RATE)
        } else {
            48_000.0
        };
        let rms_warmup_frames = (sample_rate * RMS_AVERAGING_SECONDS).ceil().max(1.0) as usize;
        let activity_adjustment_hold_limit =
            (sample_rate * MATCH_ACTIVITY_HOLD_SECONDS).ceil().max(1.0) as u64;
        let smoothing_coefficient =
            -(-1.0 / (sample_rate as f64 * GAIN_SMOOTHING_SECONDS as f64)).exp_m1();
        let gain_increase_coefficient =
            -(-1.0 / (sample_rate as f64 * GAIN_INCREASE_SECONDS as f64)).exp_m1();
        let locked_gain_db = sanitize_gain_db(stored_gain_db);
        let gain = db_to_linear(locked_gain_db);
        Self {
            manual_mode: false,
            rms_mode: true,
            measurement_rms_mode: true,
            rms_silence_frames: (sample_rate * 2.0).ceil().max(1.0) as u64,
            rms_frames: 0,
            rms_quiet_frames: 0,
            input_rms: RmsDetector::new(rms_warmup_frames),
            output_rms: RmsDetector::new(rms_warmup_frames),
            measurement_level: 0.0,
            session_peak: 0.0,
            activity_adjustment_hold_limit,
            activity_adjustment_hold_frames: 0,
            session_rms: 0.0,
            measurement_gain_linear: gain,
            measurement_rms_candidate_seen: false,
            force_measurement_update: false,
            smoothing_coefficient,
            gain_increase_coefficient,
            startup_window_frames: (sample_rate * MATCH_LISTEN_SECONDS).ceil().max(1.0) as u64,
            startup_observed_frames: 0,
            confidence_stable_frames: 0,
            confidence_stable_limit: (sample_rate * MATCH_CONFIDENCE_STABLE_SECONDS).ceil() as u64,
            confidence_level: 0.0,
            evidence_interval_frames: (sample_rate * MATCH_EVIDENCE_INTERVAL_SECONDS)
                .ceil()
                .max(1.0) as u64,
            evidence_since_observation: 0,
            evidence_observations: 0,
            match_slew_ratio: 10.0_f64
                .powf(MATCH_SLEW_DB_PER_SECOND as f64 / (20.0 * sample_rate as f64)),
            match_transition_step: 1.0 / (sample_rate * MATCH_TRANSITION_SECONDS),
            match_transition_position: 1.0,
            match_start_gain: gain,
            last_output_gain: gain,
            block_input_peak: 0.0,
            block_output_peak: 0.0,
            activity_silence_frames: 0,
            measurement_target_db: -12.0,
            measurement_target_peak: 10.0_f32.powf(-12.0 / 20.0),
            locked_gain_db,
            current_gain: gain as f64,
            target_gain: gain as f64,
            previous_match_request: false,
            restart_generation: 0,
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
    pub fn reset(&mut self, params: &GainSnapParams) {
        self.measurement_level = 0.0;
        self.rms_mode = params.rms_mode();
        self.measurement_rms_mode = self.rms_mode;
        self.input_rms.reset();
        self.output_rms.reset();
        self.rms_frames = 0;
        self.rms_quiet_frames = 0;
        self.session_peak = 0.0;
        self.session_rms = 0.0;
        self.measurement_rms_candidate_seen = false;
        self.block_input_peak = 0.0;
        self.block_output_peak = 0.0;
        self.activity_silence_frames = 0;
        self.activity_adjustment_hold_frames = 0;
        self.measurement_target_db = params.target_db();
        self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
        self.locked_gain_db = sanitize_gain_db(params.locked_gain_db());
        self.manual_mode = !params.match_requested();
        self.current_gain = db_to_linear(self.locked_gain_db) as f64;
        self.target_gain = self.current_gain;
        self.measurement_gain_linear = self.current_gain as f32;
        self.startup_observed_frames = 0;
        self.confidence_stable_frames = 0;
        self.confidence_level = 0.0;
        self.evidence_since_observation = 0;
        self.evidence_observations = 0;
        self.match_transition_position = 1.0;
        self.match_start_gain = self.current_gain as f32;
        self.last_output_gain = self.current_gain as f32;
        self.previous_match_request = false;
        self.restart_generation = params.restart_generation();
        self.state = MatchState::Ready;
    }

    /// Apply control changes after a sample-offset parameter event.
    pub fn sync_controls(&mut self, params: &GainSnapParams) {
        let request = params.match_requested();
        self.manual_mode = !request;
        let mode_changed = self.rms_mode != params.rms_mode();
        let restart_generation = params.restart_generation();
        let restart_requested = restart_generation != self.restart_generation;
        self.rms_mode = params.rms_mode();
        self.restart_generation = restart_generation;
        if !request {
            if self.previous_match_request && self.state == MatchState::Measuring {
                let externally_changed =
                    (params.locked_gain_db() - self.locked_gain_db).abs() > 0.0001;
                self.finish_measurement();
                if !externally_changed {
                    params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, self.locked_gain_db);
                }
            }
            self.previous_match_request = false;
        } else if !self.previous_match_request || mode_changed {
            self.measurement_target_db = params.target_db();
            self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
            self.state = MatchState::Measuring;
            self.previous_match_request = true;
            self.start_match_transition();
        } else if restart_requested {
            self.start_match_transition();
        } else if self.state == MatchState::Measuring {
            let target_db = params.target_db();
            if (target_db - self.measurement_target_db).abs() > f32::EPSILON {
                self.measurement_target_db = target_db;
                self.measurement_target_peak = 10.0_f32.powf(target_db / 20.0);
                // Changing the target keeps the session's strongest evidence.
                self.start_gain_transition();
            }
        }

        let stored_gain_db = params.locked_gain_db();
        if self.state != MatchState::Measuring
            && (stored_gain_db - self.locked_gain_db).abs() > 0.0001
        {
            if !request {
                self.state = MatchState::Ready;
            }
            self.locked_gain_db = stored_gain_db;
            self.target_gain = db_to_linear(stored_gain_db) as f64;
            self.measurement_gain_linear = self.target_gain as f32;
        }
    }

    fn start_match_transition(&mut self) {
        self.session_peak = 0.0;
        self.start_new_measurement();
    }

    fn start_new_measurement(&mut self) {
        self.startup_observed_frames = 0;
        self.confidence_stable_frames = 0;
        self.confidence_level = 0.0;
        self.evidence_since_observation = 0;
        self.evidence_observations = 0;
        self.measurement_rms_mode = self.rms_mode;
        self.input_rms.reset();
        self.rms_frames = 0;
        self.rms_quiet_frames = 0;
        self.activity_silence_frames = 0;
        self.activity_adjustment_hold_frames = self.activity_adjustment_hold_limit;
        self.measurement_level = 0.0;
        self.session_rms = 0.0;
        self.measurement_rms_candidate_seen = false;
        self.force_measurement_update = true;
        self.start_gain_transition();
    }

    fn start_gain_transition(&mut self) {
        self.force_measurement_update = true;
        // Listen at the gain actually heard, including
        // a partially completed transition. Do not mute or apply a provisional
        // correction while the measurement window is still filling.
        self.match_start_gain = self.last_output_gain;
        self.current_gain = self.match_start_gain as f64;
        self.target_gain = self.current_gain;
        self.measurement_gain_linear = self.match_start_gain;
        self.match_transition_position = 0.0;
    }

    fn observe_measurement_peak(&mut self, peak: f32) -> bool {
        if peak > self.session_peak {
            self.session_peak = peak;
            true
        } else {
            false
        }
    }

    fn observe_measurement_rms(&mut self, level: f32) {
        if level.is_finite() {
            self.session_rms = self.session_rms.max(level);
        }
    }

    fn measurement_candidate_level(&self) -> f32 {
        if self.measurement_rms_mode {
            if self.input_rms.complete() && self.session_rms > SILENCE_PEAK_LINEAR {
                self.session_rms
            } else {
                self.session_peak
            }
        } else {
            self.session_peak
        }
    }

    fn gain_for_measurement(&self, level: f32) -> f32 {
        if !level.is_finite() || level <= SILENCE_PEAK_LINEAR {
            return 0.0;
        }
        let maximum_gain = if self.measurement_rms_mode {
            RMS_GAIN_MAX_LINEAR
        } else {
            PEAK_GAIN_MAX_LINEAR
        };
        let requested_gain = self.measurement_target_peak / level;
        requested_gain.clamp(GAIN_MIN_LINEAR, maximum_gain)
    }

    fn rms_target_limited(&self, level: f32) -> bool {
        if !self.measurement_rms_mode || !self.input_rms.complete() || level <= SILENCE_PEAK_LINEAR
        {
            return false;
        }
        let requested_gain = self.measurement_target_peak / level;
        requested_gain > RMS_GAIN_MAX_LINEAR * MATCH_ADAPTATION_STABILITY_RATIO
    }

    fn consider_measurement_candidate(
        &mut self,
        params: &GainSnapParams,
        first_complete_rms: bool,
    ) {
        if self.measurement_rms_mode && !self.input_rms.complete() {
            return;
        }
        let level = self.measurement_candidate_level();
        if level <= SILENCE_PEAK_LINEAR {
            return;
        }
        if self.force_measurement_update {
            // Count signal observations separated in time; a lone transient
            // followed by silence must never authorize a large automatic boost.
            if self.confidence_level <= SILENCE_PEAK_LINEAR
                || level > self.confidence_level * MATCH_ADAPTATION_STABILITY_RATIO
            {
                self.confidence_level = level;
                self.confidence_stable_frames = 0;
            }
            self.confidence_stable_frames = self.confidence_stable_frames.saturating_add(1);
            if self.startup_observed_frames < self.startup_window_frames
                || self.confidence_stable_frames < self.confidence_stable_limit
                || self.evidence_observations < 3
                || self.activity_silence_frames >= self.confidence_stable_limit
            {
                return;
            }
        }
        let candidate_gain = self.gain_for_measurement(level);
        if !candidate_gain.is_finite() || candidate_gain <= 0.0 {
            return;
        }

        if self.force_measurement_update || first_complete_rms {
            self.publish_measurement_gain(params, level, candidate_gain);
            self.force_measurement_update = false;
            return;
        }

        // Session maxima never age out. Only newly stronger evidence may
        // lower the correction; quieter passages cannot raise it again.
        if candidate_gain < self.measurement_gain_linear {
            self.publish_measurement_gain(params, level, candidate_gain);
        }
    }

    fn publish_measurement_gain(&mut self, params: &GainSnapParams, level: f32, gain: f32) {
        let previous_gain = self.measurement_gain_linear;
        let gain_db = (20.0 * gain.log10()).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        self.measurement_level = level;
        self.measurement_gain_linear = gain;
        self.locked_gain_db = sanitize_gain_db(gain_db);
        self.target_gain = db_to_linear(self.locked_gain_db) as f64;
        let meaningful_change = !previous_gain.is_finite()
            || previous_gain <= 0.0
            || gain > previous_gain * MATCH_ACTIVITY_MEANINGFUL_GAIN_RATIO
            || gain < previous_gain / MATCH_ACTIVITY_MEANINGFUL_GAIN_RATIO;
        if meaningful_change {
            self.activity_adjustment_hold_frames = self.activity_adjustment_hold_limit;
        }
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, self.locked_gain_db);
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

        let mut rms_updated = false;
        if self.state == MatchState::Measuring {
            self.activity_adjustment_hold_frames =
                self.activity_adjustment_hold_frames.saturating_sub(1);
            self.evidence_since_observation = self.evidence_since_observation.saturating_add(1);
            if input_peak > SILENCE_PEAK_LINEAR {
                if self.evidence_observations == 0
                    || self.evidence_since_observation >= self.evidence_interval_frames
                {
                    self.evidence_observations =
                        self.evidence_observations.saturating_add(1).min(3);
                    self.evidence_since_observation = 0;
                }
                self.activity_silence_frames = 0;
            } else {
                self.activity_silence_frames = self.activity_silence_frames.saturating_add(1);
            }
            if self.rms_mode {
                if input_peak > SILENCE_PEAK_LINEAR {
                    self.rms_quiet_frames = 0;
                } else {
                    self.rms_quiet_frames = self.rms_quiet_frames.saturating_add(1);
                }
                let observed_signal =
                    self.session_peak > SILENCE_PEAK_LINEAR || input_peak > SILENCE_PEAK_LINEAR;
                if self.rms_quiet_frames < self.rms_silence_frames && observed_signal {
                    self.input_rms.update(input_left, input_right);
                    self.rms_frames = self.rms_frames.saturating_add(1);
                    rms_updated = true;
                }
            }

            let new_peak = self.observe_measurement_peak(input_peak);
            if self.session_peak > SILENCE_PEAK_LINEAR {
                self.startup_observed_frames = self
                    .startup_observed_frames
                    .saturating_add(1)
                    .min(self.startup_window_frames);
            }
            let mut first_complete_rms = false;
            if self.rms_mode && self.rms_frames > 0 && self.input_rms.complete() && rms_updated {
                let level = self.input_rms.level();
                self.observe_measurement_rms(level);
                if level > SILENCE_PEAK_LINEAR && !self.measurement_rms_candidate_seen {
                    self.measurement_rms_candidate_seen = true;
                    first_complete_rms = true;
                }
            }

            if new_peak || self.measurement_candidate_level() > SILENCE_PEAK_LINEAR {
                self.consider_measurement_candidate(params, first_complete_rms);
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
        // The transition begins only after a confident measurement has been
        // published. Blend from the held audible gain, never through silence.
        let position = self.match_transition_position;
        let blend = position * position * (3.0 - 2.0 * position);
        if !self.force_measurement_update || self.state != MatchState::Measuring {
            self.match_transition_position = (position + self.match_transition_step).min(1.0);
        }
        let desired_gain = self.match_start_gain * (1.0 - blend) + self.current_gain as f32 * blend;
        let desired_gain = if self.state == MatchState::Measuring {
            (desired_gain as f64).clamp(
                self.last_output_gain as f64 / self.match_slew_ratio,
                self.last_output_gain as f64 * self.match_slew_ratio,
            )
        } else {
            desired_gain as f64
        };
        // 0 dBFS is a reference level, not a floating-point output ceiling.
        // Only prevent numeric overflow near f32::MAX, preserving stereo ratio.
        // Multiplication in f64 avoids overflow before conversion to the host's f32.
        let numeric_max_gain = if input_peak > 0.0 {
            f32::MAX as f64 / input_peak as f64
        } else {
            f64::MAX
        };
        let applied_gain = desired_gain.min(numeric_max_gain);
        self.last_output_gain = applied_gain as f32;
        let output_left = (input_left as f64 * applied_gain) as f32;
        let output_right = (input_right as f64 * applied_gain) as f32;
        self.output_rms.update(output_left, output_right);
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
            output_rms_db: linear_to_db(self.output_rms.level()),
            progress: progress.clamp(0.0, 1.0),
            state: self.state,
            activity: self.activity(),
            locked_gain_db: self.locked_gain_db,
            applied_gain_db: 20.0 * self.last_output_gain.max(f32::MIN_POSITIVE).log10(),
            target_shortfall_db: self.target_shortfall_db(),
        }
    }

    fn target_shortfall_db(&self) -> f32 {
        let level = self.measurement_candidate_level();
        if self.state != MatchState::Measuring || !self.rms_target_limited(level) {
            return 0.0;
        }
        let attainable = level * self.gain_for_measurement(level);
        (20.0 * (self.measurement_target_peak / attainable).log10()).max(0.0)
    }

    fn activity(&self) -> MatchActivity {
        match self.state {
            MatchState::Ready => MatchActivity::Ready,
            MatchState::NoSignal => MatchActivity::NoSignal,
            MatchState::Locked => MatchActivity::Held,
            MatchState::Measuring => {
                let candidate_level = self.measurement_candidate_level();
                if candidate_level <= SILENCE_PEAK_LINEAR || self.force_measurement_update {
                    return if self.activity_silence_frames >= self.rms_silence_frames
                        && self.session_peak <= SILENCE_PEAK_LINEAR
                    {
                        MatchActivity::NoSignal
                    } else {
                        MatchActivity::Listening
                    };
                }

                let gain_slewing = (self.target_gain - self.last_output_gain as f64).abs()
                    > self.target_gain.max(self.last_output_gain as f64) * 0.001;
                let transition_active = self.match_transition_position < 1.0
                    || self.activity_adjustment_hold_frames > 0;
                if gain_slewing || transition_active {
                    MatchActivity::Adjusting
                } else if self.rms_target_limited(candidate_level) {
                    MatchActivity::BelowTarget
                } else {
                    MatchActivity::Matched
                }
            }
        }
    }

    fn finish_measurement(&mut self) {
        // Stop at the audible correction, including halfway through a slew.
        self.current_gain = self.last_output_gain as f64;
        self.target_gain = self.current_gain;
        self.match_start_gain = self.last_output_gain;
        self.match_transition_position = 1.0;
        self.locked_gain_db = sanitize_gain_db(20.0 * self.last_output_gain.log10());
        if self.measurement_level <= SILENCE_PEAK_LINEAR {
            self.match_transition_position = 1.0;
            self.state = if self.session_peak > SILENCE_PEAK_LINEAR {
                MatchState::Locked
            } else {
                MatchState::NoSignal
            };
            return;
        }
        self.state = MatchState::Locked;
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

/// Convert a positive meter peak to decibels, with finite silence handling.
///
/// The meter remains bounded at +24 dBFS for display safety; locked gain uses
/// the wider correction range through [`db_to_linear`] and `sanitize_gain_db`.
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

    fn peak_params() -> GainSnapParams {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_RMS_MODE, 0.0);
        params
    }
    use crate::params::{PARAM_MATCH, PARAM_TARGET_DB};

    fn run_frames(engine: &mut GainSnapEngine, params: &GainSnapParams, left: f32, frames: usize) {
        for _ in 0..frames {
            let _ = engine.process_frame(params, left, left);
        }
    }

    fn run_block(engine: &mut GainSnapEngine, params: &GainSnapParams, left: f32, frames: usize) {
        engine.begin_block(params);
        run_frames(engine, params, left, frames);
    }

    fn rms_params() -> GainSnapParams {
        let params = peak_params();
        params.set_param(crate::params::PARAM_RMS_MODE, 1.0);
        params.set_param(PARAM_MATCH, 1.0);
        params
    }

    #[test]
    fn quiet_lead_in_and_unsettled_levels_do_not_authorize_a_boost() {
        for rms in [false, true] {
            let params = rms_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_TARGET_DB, 0.0);
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            engine.begin_block(&params);
            for _ in 0..900 {
                assert_eq!(
                    engine.process_frame(&params, 0.001, -0.0005),
                    (0.001, -0.0005)
                );
            }
            for _ in 0..499 {
                assert_eq!(engine.process_frame(&params, 0.5, -0.25), (0.5, -0.25));
                assert_eq!(params.locked_gain_db(), 0.0);
            }
            run_frames(&mut engine, &params, 0.5, 2_000);
            assert!((engine.report().applied_gain_db - 6.0206).abs() < 0.01);
        }
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        for frame in 0..3_000 {
            let sample = 0.001 * 2.0_f32.powf(frame as f32 / 300.0);
            assert_eq!(
                engine.process_frame(&params, sample, -sample),
                (sample, -sample)
            );
            assert_eq!(params.locked_gain_db(), 0.0);
        }
    }

    #[test]
    fn isolated_peak_and_silence_never_establish_confidence() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        engine.process_frame(&params, 0.001, -0.001);
        run_frames(&mut engine, &params, 0.0, 10_000);
        assert_eq!(params.locked_gain_db(), 0.0);
        assert_eq!(engine.report().applied_gain_db, 0.0);
        assert_eq!(engine.report().activity, MatchActivity::Listening);
    }

    #[test]
    fn automatic_gain_slew_is_bounded_and_stopping_holds_the_audible_gain() {
        for rate in [1_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let params = peak_params();
            params.set_param(PARAM_TARGET_DB, 0.0);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(rate, 0.0);
            engine.begin_block(&params);
            let mut previous_db = 0.0;
            for frame in 0..(rate * 3.0) as usize {
                let input = if frame < (rate * 2.0) as usize {
                    0.0001
                } else {
                    0.5
                };
                let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                assert_eq!(right, -left * 0.5);
                let db = 20.0 * (left / input).log10();
                assert!((db - previous_db).abs() <= MATCH_SLEW_DB_PER_SECOND / rate + 0.00003);
                previous_db = db;
            }
            let before = engine.process_frame(&params, 0.5, -0.25);
            let audible = engine.report().applied_gain_db;
            params.set_param(PARAM_MATCH, 0.0);
            engine.sync_controls(&params);
            assert_eq!(engine.process_frame(&params, 0.5, -0.25), before);
            assert!((params.locked_gain_db() - audible).abs() < 0.0001);
        }
    }

    #[test]
    fn peak_and_rms_match_minus_sixty_and_hold_it() {
        for rms in [false, true] {
            let params = rms_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_TARGET_DB, -60.0);
            assert_eq!(params.target_db(), -60.0);
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            run_block(&mut engine, &params, 1.0, 4_000);
            let (left, right) = engine.process_frame(&params, 1.0, -0.5);
            assert!((20.0 * left.log10() + 60.0).abs() < 0.01);
            assert_eq!(right, -left * 0.5);
            params.set_param(PARAM_MATCH, 0.0);
            engine.sync_controls(&params);
            assert!((params.locked_gain_db() + 60.0).abs() < 0.01);
            let restored = GainSnapParams::new();
            let payload = crate::state::encode_payload(&params);
            let state =
                crate::state::decode_payload(crate::state::STATE_VERSION, &payload).unwrap();
            crate::state::apply_snapshot(&restored, state);
            assert_eq!(restored.target_db(), -60.0);
            assert!((restored.locked_gain_db() + 60.0).abs() < 0.01);
        }
    }

    #[test]
    fn manual_gain_is_fixed_across_louder_input_and_auto_can_resume() {
        let params = peak_params();
        params.set_param(crate::params::PARAM_MANUAL_GAIN_DB, 6.0);
        params.set_param(crate::params::PARAM_MANUAL_MODE, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 6.0);
        engine.reset(&params);
        engine.begin_block(&params);
        let (restored_left, _) = engine.process_frame(&params, 0.2, 0.2);
        assert!((restored_left - 0.2 * db_to_linear(6.0)).abs() < 0.002);
        run_block(&mut engine, &params, 0.2, 2_000);
        let (left, _) = engine.process_frame(&params, 0.2, 0.2);
        assert!((left - 0.2 * db_to_linear(6.0)).abs() < 0.002);
        run_block(&mut engine, &params, 0.4, 2_000);
        assert_eq!(params.manual_gain_db(), 6.0);
        assert_eq!(params.locked_gain_db(), 6.0);
        assert_eq!(engine.report().state, MatchState::Ready);

        params.set_param(crate::params::PARAM_MANUAL_GAIN_DB, -6.0);
        run_block(&mut engine, &params, 0.4, 2_000);
        let (left, _) = engine.process_frame(&params, 0.4, 0.4);
        assert!((left - 0.4 * db_to_linear(-6.0)).abs() < 0.002);

        params.set_param(PARAM_MATCH, 1.0);
        run_block(&mut engine, &params, 0.4, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        let (left, _) = engine.process_frame(&params, 0.4, 0.4);
        assert!((left - db_to_linear(params.target_db())).abs() < 0.002);
    }

    #[test]
    fn established_measurement_stays_confident_through_ten_minutes_of_silence() {
        for rms in [false, true] {
            let params = peak_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, 0.5, 2_000);
            run_frames(&mut engine, &params, 0.0, 2_000);
            let gain = params.locked_gain_db();
            let activity = engine.report().activity;
            assert!(matches!(
                activity,
                MatchActivity::Matched | MatchActivity::BelowTarget
            ));
            for _ in 0..600 {
                run_block(&mut engine, &params, 0.0, 1_000);
                assert_eq!(params.locked_gain_db(), gain);
                assert_eq!(engine.report().activity, activity);
                assert_eq!(engine.session_peak, 0.5);
            }
            // A quieter passage cannot invalidate the loudest evidence.
            run_block(&mut engine, &params, 0.001, 4_000);
            assert_eq!(params.locked_gain_db(), gain);
            assert_eq!(engine.report().activity, activity);
            // A newly higher peak remains actionable in the same session.
            run_frames(&mut engine, &params, 0.9, 600);
            run_frames(&mut engine, &params, 0.0, 2_000);
            assert_eq!(engine.session_peak, 0.9);
            assert!(params.locked_gain_db() < gain);
            assert!(matches!(
                engine.report().activity,
                MatchActivity::Matched | MatchActivity::BelowTarget
            ));
        }
    }

    #[test]
    fn rms_restart_and_new_session_clear_the_retained_measurement() {
        for restart in [false, true] {
            let params = rms_params();
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            run_block(&mut engine, &params, 0.5, 2_000);
            let old_gain = params.locked_gain_db();
            if restart {
                params.request_restart();
            } else {
                params.set_param(PARAM_MATCH, 0.0);
                engine.sync_controls(&params);
                params.set_param(PARAM_MATCH, 1.0);
            }
            engine.sync_controls(&params);
            run_frames(&mut engine, &params, 0.125, 299);
            assert_eq!(params.locked_gain_db(), old_gain);
            run_frames(&mut engine, &params, 0.125, 2_000);
            assert_eq!(engine.session_peak, 0.125);
            assert!((params.locked_gain_db() - 6.0618).abs() < 0.02);
            assert_eq!(engine.report().activity, MatchActivity::Matched);
        }
    }

    #[test]
    fn activity_moves_from_listening_through_adjusting_to_matched() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);

        engine.begin_block(&params);
        assert_eq!(engine.report().activity, MatchActivity::Listening);

        run_frames(&mut engine, &params, 0.5, 999);
        assert_eq!(engine.report().activity, MatchActivity::Listening);
        engine.process_frame(&params, 0.5, 0.5);
        assert_eq!(engine.report().activity, MatchActivity::Adjusting);

        run_frames(&mut engine, &params, 0.5, 1_200);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
    }

    #[test]
    fn activity_stays_matched_after_a_quieter_peak_passage() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.5, 2_000);
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        // The retained session maximum keeps the activity settled through a
        // quieter passage that outlives the old rolling history window.
        run_block(&mut engine, &params, 0.25, 4_000);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
    }

    #[test]
    fn activity_reports_louder_adjustment_without_losing_confidence_in_silence() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.25, 2_000);
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        run_block(&mut engine, &params, 0.5, 1);
        assert_eq!(engine.report().activity, MatchActivity::Adjusting);
        run_block(&mut engine, &params, 0.5, 100);
        assert_eq!(engine.report().activity, MatchActivity::Adjusting);
        run_block(&mut engine, &params, 0.5, 1_000);
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        run_block(&mut engine, &params, 0.0, 500);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
        run_block(&mut engine, &params, 0.0, 2_000);
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert_eq!(engine.report().activity, MatchActivity::Held);
    }

    #[test]
    fn enabled_peak_match_rematches_louder_upstream_audio_without_editor_events() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        let target = db_to_linear(params.target_db());

        run_block(&mut engine, &params, 0.1, 2_000);
        let original_gain = params.locked_gain_db();
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        // Closing the editor supplies no parameter events. Only incoming
        // audio changes, so the audio processor must find the new correction.
        engine.begin_block(&params);
        for _ in 0..2_000 {
            let (left, right) = engine.process_frame(&params, 0.8, -0.4);
            assert!((right + left * 0.5).abs() < 1.0e-6);
        }

        assert!(params.match_requested());
        assert!((params.locked_gain_db() - (original_gain - 20.0 * 8.0_f32.log10())).abs() < 0.01);
        let (left, right) = engine.process_frame(&params, 0.8, -0.4);
        assert!((left - target).abs() < 1.0e-4);
        assert!((right + target * 0.5).abs() < 1.0e-4);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
    }

    #[test]
    fn stopped_peak_match_keeps_the_user_gain_on_louder_input() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.1, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        let original_gain = params.locked_gain_db();
        run_frames(&mut engine, &params, 0.05, 1_000);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert_eq!(params.locked_gain_db(), original_gain);

        for _ in 0..2_000 {
            let (left, right) = engine.process_frame(&params, 0.8, -0.4);
            let expected = 0.8 * db_to_linear(original_gain);
            assert!((left - expected).abs() < 1.0e-5);
            assert!((right + left * 0.5).abs() < 1.0e-6);
        }
        assert!(!params.match_requested());
        assert_eq!(params.locked_gain_db(), original_gain);
        let reduced_gain = params.locked_gain_db();
        run_frames(&mut engine, &params, 0.1, 1_000);
        assert_eq!(params.locked_gain_db(), reduced_gain);
    }

    #[test]
    fn stopped_rms_match_keeps_the_user_gain_on_louder_input() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.1, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        let original_gain = params.locked_gain_db();

        run_frames(&mut engine, &params, 0.4, 2_000);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!(!params.match_requested());
        assert_eq!(params.locked_gain_db(), original_gain);
        let reduced_gain = params.locked_gain_db();
        run_frames(&mut engine, &params, 0.05, 1_000);
        assert_eq!(params.locked_gain_db(), reduced_gain);
    }

    #[test]
    fn saved_match_gain_stays_manual_after_fresh_engine_activation() {
        for rms_mode in [false, true] {
            let params = peak_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms_mode));
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            run_block(&mut engine, &params, 0.1, 2_000);
            params.set_param(PARAM_MATCH, 0.0);
            engine.begin_block(&params);
            assert_eq!(engine.report().state, MatchState::Locked);

            let payload = crate::state::encode_payload(&params);
            let snapshot = crate::state::decode_payload(crate::state::STATE_VERSION, &payload)
                .expect("valid saved match");
            let restored = peak_params();
            crate::state::apply_snapshot(&restored, snapshot);
            let previous_gain = restored.locked_gain_db();
            let mut reactivated = GainSnapEngine::new(1_000.0, previous_gain);
            reactivated.begin_block(&restored);
            assert_eq!(reactivated.report().state, MatchState::Ready);
            assert!(!restored.match_requested());

            run_frames(&mut reactivated, &restored, 0.8, 2_000);
            assert_eq!(restored.locked_gain_db(), previous_gain);
            assert_eq!(reactivated.report().state, MatchState::Ready);
        }
    }

    #[test]
    fn untouched_instance_remains_unarmed_after_activation() {
        let params = peak_params();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Ready);
        assert_eq!(engine.process_frame(&params, 0.8, -0.4), (0.8, -0.4));
    }

    #[test]
    fn loading_unarmed_state_disarms_an_existing_held_match() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.1, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);

        let unarmed = peak_params();
        let snapshot = crate::state::decode_payload(
            crate::state::STATE_VERSION,
            &crate::state::encode_payload(&unarmed),
        )
        .expect("valid unarmed state");
        crate::state::apply_snapshot(&params, snapshot);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Ready);
        run_frames(&mut engine, &params, 0.8, 2_000);
        assert_eq!(params.locked_gain_db(), 0.0);
    }

    #[test]
    fn loading_state_during_match_adopts_saved_manual_gain() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.1, 2_000);

        let saved = peak_params();
        saved.set_param(crate::params::PARAM_LOCKED_GAIN_DB, -6.0);
        let snapshot = crate::state::decode_payload(
            crate::state::STATE_VERSION,
            &crate::state::encode_payload(&saved),
        )
        .unwrap();
        crate::state::apply_snapshot(&params, snapshot);
        engine.begin_block(&params);

        assert_eq!(engine.report().state, MatchState::Ready);
        assert_eq!(params.locked_gain_db(), -6.0);
        run_frames(&mut engine, &params, 0.1, 1_000);
        assert!((engine.report().locked_gain_db + 6.0).abs() < 0.001);
    }

    #[test]
    fn activity_keeps_rms_warmup_listening_and_maps_disabled_gain_to_held() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.5, 100);
        assert_eq!(engine.report().activity, MatchActivity::Listening);

        run_block(&mut engine, &params, 0.5, 1_500);
        assert_eq!(engine.report().activity, MatchActivity::Matched);

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert_eq!(engine.report().activity, MatchActivity::Held);
    }

    #[test]
    fn stable_sine_stays_matched_across_typical_block_phases() {
        const SAMPLE_RATE: usize = 48_000;
        const TOTAL_FRAMES: usize = SAMPLE_RATE * 5;
        const SETTLED_AFTER: usize = SAMPLE_RATE * 3;

        for block_size in [64, 512] {
            let params = peak_params();
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(SAMPLE_RATE as f32, 0.0);
            let mut frame = 0;
            while frame < TOTAL_FRAMES {
                engine.begin_block(&params);
                let block_end = (frame + block_size).min(TOTAL_FRAMES);
                while frame < block_end {
                    let phase = std::f32::consts::TAU * 440.1 * frame as f32 / SAMPLE_RATE as f32;
                    let input = 0.5 * phase.sin();
                    let _ = engine.process_frame(&params, input, input);
                    frame += 1;
                }
                if frame >= SETTLED_AFTER {
                    assert_eq!(
                        engine.report().activity,
                        MatchActivity::Matched,
                        "stable sine cycled at block size {block_size}, frame {frame}"
                    );
                }
            }
        }
    }

    #[test]
    fn sparse_pulses_stay_matched_across_typical_block_phases() {
        const SAMPLE_RATE: usize = 48_000;
        const PULSE_PERIOD: usize = SAMPLE_RATE * 3 / 10;
        const PULSE_WIDTH: usize = 3;
        const TOTAL_FRAMES: usize = SAMPLE_RATE * 5;
        const SETTLED_AFTER: usize = SAMPLE_RATE * 3;

        for block_size in [64, 512] {
            let params = peak_params();
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(SAMPLE_RATE as f32, 0.0);
            let mut frame = 0;
            while frame < TOTAL_FRAMES {
                engine.begin_block(&params);
                let block_end = (frame + block_size).min(TOTAL_FRAMES);
                while frame < block_end {
                    let input = if frame % PULSE_PERIOD < PULSE_WIDTH {
                        0.5
                    } else {
                        0.0
                    };
                    let _ = engine.process_frame(&params, input, input);
                    frame += 1;
                }
                if frame >= SETTLED_AFTER {
                    assert_eq!(
                        engine.report().activity,
                        MatchActivity::Matched,
                        "sparse pulses cycled at block size {block_size}, frame {frame}"
                    );
                }
            }
        }
    }

    #[test]
    fn rms_gain_updates_for_sparse_impulses_at_any_control_tick_phase() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, -24.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let mut energy = 0.0_f64;
        for n in 0..144_000 {
            let input = if n % 64 == 0 { 0.1 } else { 0.0 };
            let out = engine.process_frame(&params, input, input).0;
            if n >= 96_000 {
                energy += (out as f64).powi(2);
            }
        }
        let db = 20.0 * (energy / 48_000.0).sqrt().log10();
        assert!((db + 24.0).abs() < 0.03, "{db}");
    }

    #[test]
    fn short_gaps_between_hits_do_not_restart_the_rms_listening_period() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.1, 48_000);
        for _ in 0..5 {
            run_frames(&mut engine, &params, 0.0, 19_200);
            let first = engine.process_frame(&params, 0.1, -0.1);
            assert!(
                first.0 > 0.01,
                "ordinary rhythmic gaps must not mute the next hit"
            );
            run_frames(&mut engine, &params, 0.1, 4_800);
        }
    }

    #[test]
    fn rms_matches_sine_average_instead_of_its_peak_at_host_sample_rates() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for rms in [false, true] {
                let params = rms_params();
                params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
                let mut engine = GainSnapEngine::new(rate as f32, 0.0);
                engine.begin_block(&params);
                let mut energy = 0.0_f64;
                let mut peak = 0.0_f32;
                for n in 0..rate * 3 {
                    let input = (std::f64::consts::TAU * 1000.0 * n as f64 / rate as f64).sin()
                        as f32
                        * 0.25;
                    let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                    assert_eq!(right, -left * 0.5);
                    if n >= rate * 2 {
                        energy += (left as f64).powi(2);
                        peak = peak.max(left.abs());
                    }
                }
                let actual = 20.0 * (energy / rate as f64).sqrt().log10();
                let expected = if rms {
                    -12.0
                } else {
                    -12.0 - 10.0 * 2.0_f64.log10()
                };
                assert!(
                    (actual - expected).abs() < 0.03,
                    "rate={rate} rms={rms}: {actual}"
                );
                if rms {
                    assert!(
                        peak > 10.0_f32.powf(-12.0 / 20.0) * 1.3,
                        "RMS must permit crest above target"
                    );
                    assert!(
                        (engine.report().output_rms_db + 12.0).abs() < 0.03,
                        "{} gain {} level {} frames {}",
                        engine.report().output_rms_db,
                        params.locked_gain_db(),
                        engine.measurement_level,
                        engine.rms_frames
                    );
                }
            }
        }
    }

    #[test]
    fn rms_holds_session_gain_until_match_stops() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        run_block(&mut engine, &params, 0.5, 96_000);
        let gain = params.locked_gain_db();
        run_block(&mut engine, &params, 0.125, 216_000);
        assert_eq!(params.locked_gain_db(), gain);
        assert!((engine.report().output_rms_db + 24.0412).abs() < 0.03);
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        run_frames(&mut engine, &params, 0.0625, 96_000);
        assert_eq!(params.locked_gain_db(), gain);
        assert_eq!(engine.report().state, MatchState::Locked);
    }

    #[test]
    fn rms_session_does_not_boost_quieter_passages() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.5, 2_000);
        let gain = params.locked_gain_db();
        for input in [0.125, 0.5, 0.125, 0.0] {
            run_block(&mut engine, &params, input, 4_000);
            assert_eq!(params.locked_gain_db(), gain);
            assert_eq!(engine.report().activity, MatchActivity::Matched);
        }
        run_block(&mut engine, &params, 0.75, 1_000);
        assert!(params.locked_gain_db() < gain);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
    }

    #[test]
    fn peak_matching_does_not_progressively_boost_a_decaying_tail() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 1_000);
        let loud_gain = params.locked_gain_db();

        // The tail falls by 1 dB per 100 ms. Its rolling candidate therefore
        // never stays within the 0.5 dB settling band for the full 300 ms.
        let decay_per_frame = 10.0_f32.powf(-1.0 / (20.0 * 100.0));
        let mut input = 0.5;
        for _ in 0..12_000 {
            engine.process_frame(&params, input, input);
            input *= decay_per_frame;
        }

        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!(params.match_requested());
        assert!(
            params.locked_gain_db() <= loud_gain + 0.2,
            "decaying tail ratcheted gain from {loud_gain} to {}",
            params.locked_gain_db()
        );
    }

    #[test]
    fn rms_matches_average_without_capping_transient_headroom() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        let amplitude = 0.25 * 2.0_f32.sqrt();
        for frame in 0..3_500 {
            let phase = std::f32::consts::TAU * frame as f32 / 20.0;
            engine.process_frame(&params, phase.sin() * amplitude, 0.0);
        }
        let gain = params.locked_gain_db();
        assert!((gain - 12.0412).abs() < 0.2);
        for frame in 0..3_800 {
            let input = if frame % 2 == 0 { 0.25 } else { -0.25 };
            engine.process_frame(&params, input, input);
        }
        assert!((params.locked_gain_db() - gain).abs() < 0.05);
        assert_eq!(engine.report().activity, MatchActivity::Matched);
    }

    fn damped_kick(frame: usize, sample_rate: usize, bpm: usize) -> f32 {
        let beat_frames = sample_rate * 60 / bpm;
        let within_beat = frame % beat_frames;
        let burst_frames = sample_rate * 3 / 20;
        if within_beat >= burst_frames {
            return 0.001;
        }
        let time = within_beat as f32 / sample_rate as f32;
        0.8 * (-time * 24.0).exp() * (std::f32::consts::TAU * 58.0 * time).sin() + 0.001
    }

    fn measure_kicks(
        sample_rate: usize,
        bpm: usize,
        target_db: f32,
        release_phase: f32,
    ) -> (f32, f32, f32, f32, MatchActivity) {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, target_db);
        let mut engine = GainSnapEngine::new(sample_rate as f32, 0.0);
        engine.begin_block(&params);
        let beat_frames = sample_rate * 60 / bpm;
        let release_at = beat_frames * 8 + (beat_frames as f32 * release_phase) as usize;
        let mut output_peak = 0.0_f32;
        let mut strongest_output_rms_db = -120.0_f32;
        for frame in 0..=release_at {
            let input = damped_kick(frame, sample_rate, bpm);
            let output = engine.process_frame(&params, input, -input * 0.7);
            assert!(output.0.is_finite() && output.1.is_finite());
            if frame >= beat_frames * 6 {
                output_peak = output_peak.max(output.0.abs()).max(output.1.abs());
                strongest_output_rms_db =
                    strongest_output_rms_db.max(engine.report().output_rms_db);
            }
        }
        let gain = params.locked_gain_db();
        let report = engine.report();
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        (
            gain,
            output_peak,
            report.output_rms_db,
            strongest_output_rms_db,
            report.activity,
        )
    }

    #[test]
    fn rms_kick_windows_hold_one_gain_across_tempo_rate_and_release_phase() {
        for sample_rate in [44_100, 48_000, 96_000, 192_000] {
            for bpm in [60, 120, 180] {
                let early = measure_kicks(sample_rate, bpm, -18.0, 0.35);
                let late = measure_kicks(sample_rate, bpm, -18.0, 0.85);
                assert!(
                    (early.0 - late.0).abs() < 0.001,
                    "rate={sample_rate} bpm={bpm}: {early:?} vs {late:?}"
                );
                // The ordinary -18 dBFS target remains below full scale.
                assert!(early.1 < 1.0 && late.1 < 1.0);
            }
        }
    }

    #[test]
    fn correction_feedback_reports_actual_gain_and_retains_shortfall_through_gaps() {
        let params = rms_params();
        params.set_param(crate::params::PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        for frame in 0..3_000 {
            let input = if frame % 500 < 10 { 0.00002 } else { 0.0 };
            let (left, _) = engine.process_frame(&params, input, input);
            if input > 0.0 {
                assert!(
                    (engine.report().applied_gain_db - 20.0 * (left / input).log10()).abs() < 0.001
                );
            }
        }
        let deficit = engine.report().target_shortfall_db;
        assert!(
            deficit > 70.0 && deficit < 110.0,
            "measured deficit={deficit}"
        );
        assert_eq!(engine.report().activity, MatchActivity::BelowTarget);
        for _ in 0..2_000 {
            engine.process_frame(&params, 0.0, 0.0);
        }
        assert!((engine.report().target_shortfall_db - deficit).abs() < 0.001);
        params.set_param(crate::params::PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().target_shortfall_db, 0.0);
    }

    #[test]
    fn rms_kick_targets_preserve_transients_above_full_scale() {
        for target in [-18.0, -12.0, 0.0] {
            let (_, output_peak, _, strongest_rms, activity) =
                measure_kicks(48_000, 120, target, 0.8);
            assert!(
                (strongest_rms - target).abs() < 0.05,
                "target={target}, RMS={strongest_rms}"
            );
            if target >= -12.0 {
                assert!(output_peak > 1.0, "transients must pass above full scale");
            }
            assert_eq!(activity, MatchActivity::Matched);
        }
    }

    #[test]
    #[ignore = "prints representative kick gain and output-peak measurements"]
    fn rms_kick_fixture_metrics() {
        for target in [-18.0, -12.0, 0.0] {
            let (gain, output_peak, output_rms_db, max_rms_db, activity) =
                measure_kicks(48_000, 120, target, 0.8);
            println!(
                "target={target:.0} dBFS, held gain={gain:.3} dB, output peak={output_peak:.6}, current RMS={output_rms_db:.3} dBFS, strongest RMS={max_rms_db:.3} dBFS, activity={activity:?}"
            );
        }
    }

    #[test]
    fn rms_short_one_shot_completes_through_zeroes_and_early_off_keeps_original_gain() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        for frame in 0..14_401 {
            let input = if frame < 960 { 0.5 } else { 0.0 };
            engine.process_frame(&params, input, input);
        }
        assert!(engine.input_rms.complete());
        assert!(engine.session_rms > SILENCE_PEAK_LINEAR);
        assert_eq!(
            engine.locked_gain_db, 0.0,
            "one shot is insufficient confidence"
        );
        run_frames(&mut engine, &params, 0.0, 96_000);
        assert_eq!(params.locked_gain_db(), 0.0);

        let early_params = rms_params();
        let mut early = GainSnapEngine::new(48_000.0, 0.0);
        early.begin_block(&early_params);
        run_frames(&mut early, &early_params, 0.5, 960);
        early_params.set_param(PARAM_MATCH, 0.0);
        early.sync_controls(&early_params);
        assert_eq!(early_params.locked_gain_db(), 0.0);
    }

    #[test]
    fn rms_square_and_sine_keep_full_scale_square_calibration() {
        for square in [false, true] {
            let params = rms_params();
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            for frame in 0..144_000 {
                let input = if square {
                    if frame % 48 < 24 {
                        0.25
                    } else {
                        -0.25
                    }
                } else {
                    (std::f32::consts::TAU * frame as f32 / 48.0).sin() * 0.25
                };
                engine.process_frame(&params, input, input);
            }
            assert!((engine.report().output_rms_db + 12.0).abs() < 0.04);
        }
    }

    #[test]
    fn hostile_sample_rate_is_bounded_at_construction() {
        let mut engine = GainSnapEngine::new(f32::MAX, 0.0);
        assert_eq!(engine.input_rms.samples.len(), 115_201);
        let params = rms_params();
        engine.begin_block(&params);
        for input in [f32::MAX, -f32::MAX, 0.1, 0.0] {
            let output = engine.process_frame(&params, input, -input);
            assert!(output.0.is_finite() && output.1.is_finite());
        }
    }

    #[test]
    fn rms_recovers_ordinary_power_after_an_extreme_finite_sample_leaves_window() {
        let mut detector = RmsDetector::new(16);
        detector.update(f32::MAX, -f32::MAX);
        for _ in 0..32 {
            detector.update(0.25, -0.25);
        }
        assert!(detector.complete());
        assert!(
            (detector.level() - 0.25).abs() < 1.0e-6,
            "{}",
            detector.level()
        );
    }

    #[test]
    fn rms_silence_retains_the_session_and_extreme_audio_stays_finite() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 24.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.0, 96_000);
        assert!(engine.process_frame(&params, 0.1, -0.1).0 > 0.0);
        run_frames(&mut engine, &params, 0.1, 96_000);
        let gain = params.locked_gain_db();
        run_frames(&mut engine, &params, 0.0, 144_000);
        assert_eq!(params.locked_gain_db(), gain);
        assert!(engine.process_frame(&params, 1.0, -1.0).0 > 0.0);
        for n in 0..48_000 {
            let x = if n % 2 == 0 { f32::MAX } else { f32::NAN };
            let (left, right) = engine.process_frame(&params, x, -x);
            assert!(left.is_finite() && right.is_finite());
            assert_eq!(right, -left);
        }
    }

    #[test]
    fn mode_changes_during_matching_preserve_the_first_output_sample() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        for mode in [true, false, true] {
            run_frames(&mut engine, &params, 0.5, 96_000);
            let before = engine.process_frame(&params, 0.5, -0.25);
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(mode));
            engine.sync_controls(&params);
            let after = engine.process_frame(&params, 0.5, -0.25);
            assert!((before.0 - after.0).abs() < 1.0e-6);
            run_frames(&mut engine, &params, 0.5, 96_000);
            assert!(
                (engine.report().output_rms_db + 12.0).abs() < 0.03,
                "{} gain {} level {} frames {}",
                engine.report().output_rms_db,
                params.locked_gain_db(),
                engine.measurement_level,
                engine.rms_frames
            );
        }
    }

    #[test]
    fn rms_zero_target_keeps_sine_shape_above_full_scale() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let mut peak = 0.0_f32;
        for n in 0..144_000 {
            let input = (std::f32::consts::TAU * n as f32 / 48.0).sin() * 0.5;
            let (left, right) = engine.process_frame(&params, input, -input);
            if n > 96_000 {
                assert!((left - input * 2.0_f32.sqrt() * 2.0).abs() < 0.001);
                peak = peak.max(left.abs());
            }
            assert_eq!(right, -left);
        }
        assert!((peak - 2.0_f32.sqrt()).abs() < 0.001);
        assert!(engine.report().output_rms_db.abs() < 0.01);
        assert_eq!(engine.report().target_shortfall_db, 0.0);
    }

    #[test]
    fn matching_applies_gain_live_and_turning_it_off_holds_it() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 2_000);

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
    fn peak_session_keeps_loudest_sample_after_history_window() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, -12.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 2_000);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);

        // A quiet passage longer than the old evicting history must not erase
        // the session maximum or raise the applied gain.
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.125, 4_000);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
        assert!((linear_to_db(engine.current_gain as f32) - 0.0412).abs() < 0.02);
        assert!((engine.report().output_peak_db + 18.0206).abs() < 0.01);

        // A louder sample later in the same session must reduce the gain.
        run_frames(&mut engine, &params, 0.5, 1);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 1_000);
        // Read a fresh block after the smooth reduction has settled.
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 10);
        assert!(
            (engine.report().output_peak_db + 12.0).abs() < 0.01,
            "output peak {}",
            engine.report().output_peak_db
        );

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - (-5.9794)).abs() < 0.02);
    }

    #[test]
    fn peak_zero_target_reaches_quiet_input_peak() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);

        let input_peak = 10.0_f32.powf(-32.0 / 20.0);
        run_frames(&mut engine, &params, input_peak, 4_000);

        assert!(
            engine.report().output_peak_db.abs() < 0.01,
            "input={input_peak} dB=-32, locked gain={}, output peak={} dB",
            engine.report().locked_gain_db,
            engine.report().output_peak_db
        );
    }

    #[test]
    fn peak_normalize_reaches_near_silence_floor_without_a_window_cap() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);

        let input_peak = 10.0_f32.powf(-119.0 / 20.0);
        run_frames(&mut engine, &params, input_peak, 7_000);

        assert!((engine.report().locked_gain_db - 119.0).abs() < 0.02);
        assert!(engine.report().output_peak_db.abs() < 0.01);
    }

    #[test]
    fn rms_matching_retains_the_existing_gain_cap() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.0001, 4_000);

        assert!((engine.report().locked_gain_db - 24.0).abs() < 0.001);
        assert!((params.locked_gain_db() - 24.0).abs() < 0.001);
    }

    #[test]
    fn peak_restart_discards_the_previous_session_maximum() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 1_000);
        assert!((params.locked_gain_db() + 5.9794).abs() < 0.02);

        params.request_restart();
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 1_000);

        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!((params.locked_gain_db() - 0.0412).abs() < 0.02);
    }

    #[test]
    fn turning_match_on_again_starts_a_fresh_measurement() {
        let params = peak_params();
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);

        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);

        params.set_param(PARAM_MATCH, 1.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 2_000);
        assert_eq!(engine.report().state, MatchState::Measuring);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
    }

    #[test]
    fn changing_target_while_peak_matching_reuses_session_maximum() {
        let params = peak_params();
        params.set_param(PARAM_TARGET_DB, -12.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);

        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 100);

        // A target edit while Match is already enabled keeps the old session
        // maximum even when the host delivers only the final Match=true state
        // to the next processing block.
        params.set_param(PARAM_TARGET_DB, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 2_000);

        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.25, 64);
        assert_eq!(engine.report().state, MatchState::Measuring);
        assert!(
            (engine.report().output_peak_db + 6.0206).abs() < 0.1,
            "output peak {}",
            engine.report().output_peak_db
        );

        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);

        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 6.0206).abs() < 0.02);
    }

    #[test]
    fn silence_does_not_create_unbounded_gain() {
        let params = peak_params();
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 3.0);
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
        let params = peak_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let output = engine.process_frame(&params, f32::NAN, f32::INFINITY);
        assert_eq!(output, (0.0, 0.0));
        assert!(engine.report().input_peak_db <= -119.0);
    }

    #[test]
    fn live_match_slews_and_toggle_off_does_not_change_audio() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        let first = engine.process_frame(&params, 0.5, 0.25).0;
        assert_eq!(first, 0.5);
        let mut previous = first;
        for _ in 0..96_000 {
            let (left, right) = engine.process_frame(&params, 0.5, 0.25);
            assert!(left <= previous + 1.0e-6);
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
        for input in [1.0e-5, 16.0] {
            let params = peak_params();
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, input, 48_000);
            let expected_gain = (-12.0 - 20.0 * input.log10()).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            assert!((engine.report().locked_gain_db - expected_gain).abs() < 0.01);
            let output = engine.process_frame(&params, f32::NAN, f32::INFINITY);
            assert_eq!(output, (0.0, 0.0));
            run_frames(&mut engine, &params, 0.0, 48_000);
            assert!((params.locked_gain_db() - expected_gain).abs() < 0.01);
            assert!(engine.report().output_peak_db.is_finite());
        }
    }

    #[test]
    fn match_listens_at_the_current_gain_after_silent_preroll() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for rms in [false, true] {
                let params = peak_params();
                params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
                params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 6.0);
                params.set_param(PARAM_MATCH, 1.0);
                let mut engine = GainSnapEngine::new(rate, 6.0);
                engine.begin_block(&params);
                run_frames(&mut engine, &params, 0.0, rate as usize);
                let held = 0.1 * db_to_linear(6.0);
                let frames = (rate * MATCH_LISTEN_SECONDS).ceil() as usize;
                for _ in 0..frames - 1 {
                    let (left, right) = engine.process_frame(&params, 0.1, -0.05);
                    assert!((left - held).abs() < 1.0e-6);
                    assert_eq!(right, -left * 0.5);
                    assert_eq!(params.locked_gain_db(), 6.0);
                    assert_eq!(engine.report().activity, MatchActivity::Listening);
                }
                let mut previous = engine.process_frame(&params, 0.1, -0.05).0;
                for _ in 0..rate as usize {
                    let left = engine.process_frame(&params, 0.1, -0.05).0;
                    assert!(left >= previous - 1.0e-6);
                    assert!((left - previous).abs() < 0.002);
                    previous = left;
                }
                assert!((linear_to_db(previous) + 12.0).abs() < 0.002);
            }
        }
    }

    #[test]
    fn loud_burst_after_quiet_matching_passes_through_the_smooth_gain() {
        for target in [-36.0, -12.0, 0.0] {
            let params = peak_params();
            params.set_param(PARAM_TARGET_DB, target);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, 0.0001, 240_000);
            let held_gain = db_to_linear(params.locked_gain_db());
            let (left, right) = engine.process_frame(&params, 1.0, -0.5);
            assert!(
                left > db_to_linear(target),
                "a new transient is not hard limited"
            );
            assert!(left > held_gain * 0.99 && left <= held_gain);
            assert_eq!(right, -left * 0.5);
        }
    }

    #[test]
    fn listening_preserves_over_full_scale_audio_and_stored_gain() {
        for rms in [false, true] {
            let params = peak_params();
            params.set_param(PARAM_MATCH, 1.0);
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 24.0);
            let mut engine = GainSnapEngine::new(48_000.0, 24.0);
            engine.begin_block(&params);
            for _ in 0..14_000 {
                let (left, right) = engine.process_frame(&params, 8.0, -4.0);
                assert!((left - 8.0 * db_to_linear(24.0)).abs() < 0.0001);
                assert_eq!(right, -left * 0.5);
            }
        }
    }

    #[test]
    fn held_gain_is_linear_across_zero_dbfs_without_limiter_recovery() {
        for gain_db in [-12.0, 0.0, 24.0, 80.0] {
            let params = peak_params();
            params.set_param(PARAM_MATCH, 0.0);
            params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, gain_db);
            let mut engine = GainSnapEngine::new(48_000.0, gain_db);
            engine.begin_block(&params);
            for input in [0.5, 1.0, 1.001, 8.0, -8.0, 0.001] {
                let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                let expected = input * db_to_linear(gain_db);
                assert!((left - expected).abs() <= expected.abs() * 1.0e-6);
                assert_eq!(right, -left * 0.5);
                assert!((engine.report().applied_gain_db - gain_db).abs() < 0.001);
            }
            for input in [f32::MAX, -f32::MAX] {
                let (left, right) = engine.process_frame(&params, input, -input * 0.5);
                assert!(left.is_finite() && right.is_finite());
                assert_eq!(right, -left * 0.5);
            }
            let (left, _) = engine.process_frame(&params, 0.001, 0.0);
            assert!((left - 0.001 * db_to_linear(gain_db)).abs() < left.abs() * 1.0e-6);
        }
    }

    #[test]
    fn early_match_off_keeps_the_original_gain_and_reengagement_listens_again() {
        for rms in [false, true] {
            let params = peak_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, 0.5, 480);
            params.set_param(PARAM_MATCH, 0.0);
            engine.sync_controls(&params);
            assert_eq!(params.locked_gain_db(), 0.0);
            assert_eq!(engine.process_frame(&params, 0.5, -0.5), (0.5, -0.5));
            params.set_param(PARAM_MATCH, 1.0);
            engine.sync_controls(&params);
            run_frames(&mut engine, &params, 0.25, 480);
            assert_eq!(params.locked_gain_db(), 0.0);
            assert_eq!(engine.process_frame(&params, 0.25, -0.25), (0.25, -0.25));
        }
    }

    #[test]
    fn target_edits_ramp_without_muting_or_forgetting_the_session() {
        for rms in [false, true] {
            let params = peak_params();
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_TARGET_DB, 0.0);
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            run_block(&mut engine, &params, 0.5, 96_000);
            let peak = engine.session_peak;
            let before = engine.process_frame(&params, 0.5, -0.25).0;
            params.set_param(PARAM_TARGET_DB, -36.0);
            engine.sync_controls(&params);
            let mut previous = engine.process_frame(&params, 0.5, -0.25).0;
            assert!((previous - before).abs() < 1.0e-6);
            for _ in 0..144_000 {
                let (left, right) = engine.process_frame(&params, 0.5, -0.25);
                assert!(left > 0.0);
                assert!(left <= previous + 1.0e-6);
                assert!((left - previous).abs() < 0.002);
                assert_eq!(right, -left * 0.5);
                previous = left;
            }
            assert_eq!(engine.session_peak, peak);
            assert!((linear_to_db(previous) + 36.0).abs() < 0.002);
        }
    }

    #[test]
    fn engaging_match_on_playing_audio_has_no_hard_mute_or_large_step() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for target in [-36.0, -12.0, 0.0] {
                let params = peak_params();
                params.set_param(PARAM_TARGET_DB, target);
                let mut engine = GainSnapEngine::new(sample_rate, 0.0);
                engine.begin_block(&params);
                run_frames(&mut engine, &params, 0.5, 128);
                let mut previous = engine.process_frame(&params, 0.5, -0.25).0;
                params.set_param(PARAM_MATCH, 1.0);
                engine.sync_controls(&params);
                let first = engine.process_frame(&params, 0.5, -0.25).0;
                assert!((first - previous).abs() < 1.0e-6);
                let mut quietest = first;
                for _ in 0..(sample_rate * 3.0) as usize {
                    let (left, right) = engine.process_frame(&params, 0.5, -0.25);
                    assert!(
                        (left - previous).abs() < 0.002,
                        "Match introduced a step at {sample_rate} Hz: {previous} -> {left}"
                    );
                    assert_eq!(right, -left * 0.5);
                    quietest = quietest.min(left);
                    previous = left;
                }
                assert!(
                    quietest >= 0.5_f32.min(db_to_linear(target)) - 1.0e-6,
                    "transition must stay between the original level and target"
                );
                assert!(
                    (linear_to_db(previous) - target).abs() < 0.002,
                    "rate {sample_rate}, target {target}, actual {}",
                    linear_to_db(previous)
                );
            }
        }
    }

    #[test]
    fn rapid_match_restarts_keep_the_current_audible_gain() {
        for restart_after in [1, 17, 240, 481, 2_000, 14_410, 16_000, 25_000] {
            let params = peak_params();
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            run_frames(&mut engine, &params, 0.5, 128);
            params.set_param(PARAM_MATCH, 1.0);
            engine.sync_controls(&params);
            run_frames(&mut engine, &params, 0.5, restart_after);
            let before = engine.process_frame(&params, 0.5, -0.25);
            params.set_param(PARAM_MATCH, 0.0);
            engine.sync_controls(&params);
            params.set_param(PARAM_MATCH, 1.0);
            engine.sync_controls(&params);
            let after = engine.process_frame(&params, 0.5, -0.25);
            assert!((after.0 - before.0).abs() < 1.0e-6);
            assert_eq!(after.1, -after.0 * 0.5);
        }
    }

    #[test]
    fn engagement_preserves_the_waveform_at_nonzero_sine_phases() {
        for phase in [0.1_f32, 0.7, 1.5, 2.7, 3.3, 4.6, 5.8] {
            let params = peak_params();
            let mut engine = GainSnapEngine::new(48_000.0, 0.0);
            engine.begin_block(&params);
            let input = phase.sin() * 0.7;
            engine.process_frame(&params, input, -input);
            params.set_param(PARAM_MATCH, 1.0);
            engine.sync_controls(&params);
            let next = (phase + std::f32::consts::TAU * 1_000.0 / 48_000.0).sin() * 0.7;
            let output = engine.process_frame(&params, next, -next);
            assert!((output.0 - next).abs() < 1.0e-6);
            assert_eq!(output.1, -output.0);
        }
    }

    #[test]
    fn disabling_match_without_signal_does_not_leave_output_muted() {
        let params = peak_params();
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
    fn host_processing_reset_listens_at_the_stored_gain() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 48_000);
        engine.reset(&params);
        engine.begin_block(&params);
        let held = db_to_linear(params.locked_gain_db());
        let (left, right) = engine.process_frame(&params, 1.0, -1.0);
        assert!((left - held).abs() < 1.0e-6);
        assert_eq!(right, -left);
    }

    #[test]
    #[cfg(feature = "vst3")]
    fn host_processing_reset_preserves_stopped_manual_gain() {
        let params = peak_params();
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        run_block(&mut engine, &params, 0.1, 2_000);
        params.set_param(PARAM_MATCH, 0.0);
        engine.begin_block(&params);
        let previous_gain = params.locked_gain_db();

        engine.reset(&params);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Ready);
        run_frames(&mut engine, &params, 0.8, 1_000);
        assert_eq!(params.locked_gain_db(), previous_gain);
        assert!(!params.match_requested());
    }
}
