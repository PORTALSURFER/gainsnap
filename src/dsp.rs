//! Realtime-safe Peak/RMS matching DSP.

use crate::params::{GainSnapParams, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::status::MatchState;

/// Time used to slew a newly calculated gain into the audio path.
pub const GAIN_SMOOTHING_SECONDS: f32 = 0.01;
/// Gain increases settle more slowly than reductions to avoid sudden boosts.
pub const GAIN_INCREASE_SECONDS: f32 = 0.1;
/// Fade from silence when Match is engaged, starting with the first signal.
pub const MATCH_FADE_SECONDS: f32 = 0.3;
/// Fade the audible signal down before restarting Match, avoiding a hard mute.
pub const MATCH_FADE_OUT_SECONDS: f32 = 0.01;
/// Recovery time for the stereo-linked, instantaneous-attack sample peak guard.
pub const PEAK_GUARD_RELEASE_SECONDS: f32 = 0.1;
/// Length of each full RMS measurement window.
pub const RMS_AVERAGING_SECONDS: f32 = 0.3;
/// Host rates above this are treated as this rate to keep constructor storage bounded.
pub const MAX_SUPPORTED_SAMPLE_RATE: f32 = 384_000.0;

/// Peak below this threshold is treated as no usable signal.
pub const SILENCE_PEAK_LINEAR: f32 = 1.0e-6;

const MATCH_HISTORY_BUCKET_SECONDS: f32 = 0.1;
const MATCH_HISTORY_BUCKET_COUNT: usize = 31;
const MATCH_ADAPTATION_SETTLE_SECONDS: f32 = 0.3;
const MATCH_ADAPTATION_STABILITY_RATIO: f32 = 1.059_253_7;
const GAIN_MIN_LINEAR: f32 = 0.063_095_73;
const GAIN_MAX_LINEAR: f32 = 15.848_932;

#[derive(Clone, Copy)]
struct MeasurementBucket {
    peak: f32,
    rms: f32,
}

impl MeasurementBucket {
    const EMPTY: Self = Self {
        peak: 0.0,
        rms: 0.0,
    };
}

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
    /// Last calculated gain correction in dB.
    pub locked_gain_db: f32,
}

/// Per-instance, audio-thread-owned matcher and gain smoother.
pub struct GainSnapEngine {
    rms_mode: bool,
    measurement_rms_mode: bool,
    rms_silence_frames: u64,
    rms_frames: u64,
    rms_quiet_frames: u64,
    input_rms: RmsDetector,
    output_rms: RmsDetector,
    measurement_level: f32,
    measurement_bucket_frames: u64,
    measurement_bucket_position: u64,
    measurement_bucket_index: usize,
    measurement_buckets: [MeasurementBucket; MATCH_HISTORY_BUCKET_COUNT],
    rolling_measurement_peak: f32,
    rolling_measurement_rms: f32,
    measurement_stability_window_frames: u64,
    measurement_stable_frames: u64,
    measurement_stability_anchor_gain: f32,
    measurement_stability_anchor_level: f32,
    measurement_gain_linear: f32,
    measurement_rms_candidate_seen: bool,
    force_measurement_update: bool,
    smoothing_coefficient: f64,
    gain_increase_coefficient: f64,
    peak_guard_release_coefficient: f64,
    peak_guard_gain: f64,
    match_fade_step: f32,
    match_fade_position: f32,
    fade_out_step: f32,
    fade_out_position: f32,
    fade_out_gain: f32,
    fade_out_ceiling: f32,
    last_output_gain: f32,
    last_output_ceiling: f32,
    last_output_peak: f32,
    block_input_peak: f32,
    block_output_peak: f32,
    measurement_target_db: f32,
    measurement_target_peak: f32,
    locked_gain_db: f32,
    current_gain: f64,
    target_gain: f64,
    previous_match_request: bool,
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
        let measurement_bucket_frames =
            (sample_rate * MATCH_HISTORY_BUCKET_SECONDS).ceil().max(1.0) as u64;
        let measurement_stability_window_frames = (sample_rate * MATCH_ADAPTATION_SETTLE_SECONDS)
            .ceil()
            .max(1.0) as u64;
        let smoothing_coefficient =
            -(-1.0 / (sample_rate as f64 * GAIN_SMOOTHING_SECONDS as f64)).exp_m1();
        let gain_increase_coefficient =
            -(-1.0 / (sample_rate as f64 * GAIN_INCREASE_SECONDS as f64)).exp_m1();
        let peak_guard_release_coefficient =
            -(-1.0 / (sample_rate as f64 * PEAK_GUARD_RELEASE_SECONDS as f64)).exp_m1();
        let locked_gain_db = sanitize_gain_db(stored_gain_db);
        let gain = db_to_linear(locked_gain_db);
        Self {
            rms_mode: false,
            measurement_rms_mode: false,
            rms_silence_frames: (sample_rate * 2.0).ceil().max(1.0) as u64,
            rms_frames: 0,
            rms_quiet_frames: 0,
            input_rms: RmsDetector::new(rms_warmup_frames),
            output_rms: RmsDetector::new(rms_warmup_frames),
            measurement_level: 0.0,
            measurement_bucket_frames,
            measurement_bucket_position: 0,
            measurement_bucket_index: 0,
            measurement_buckets: [MeasurementBucket::EMPTY; MATCH_HISTORY_BUCKET_COUNT],
            rolling_measurement_peak: 0.0,
            rolling_measurement_rms: 0.0,
            measurement_stability_window_frames,
            measurement_stable_frames: 0,
            measurement_stability_anchor_gain: 0.0,
            measurement_stability_anchor_level: 0.0,
            measurement_gain_linear: gain,
            measurement_rms_candidate_seen: false,
            force_measurement_update: false,
            smoothing_coefficient,
            gain_increase_coefficient,
            peak_guard_release_coefficient,
            peak_guard_gain: 1.0,
            match_fade_step: 1.0 / (sample_rate * MATCH_FADE_SECONDS),
            match_fade_position: 1.0,
            fade_out_step: 1.0 / (sample_rate * MATCH_FADE_OUT_SECONDS),
            fade_out_position: 1.0,
            fade_out_gain: 0.0,
            fade_out_ceiling: 1.0,
            last_output_gain: gain,
            last_output_ceiling: 1.0,
            last_output_peak: 0.0,
            block_input_peak: 0.0,
            block_output_peak: 0.0,
            measurement_target_db: -12.0,
            measurement_target_peak: 10.0_f32.powf(-12.0 / 20.0),
            locked_gain_db,
            current_gain: gain as f64,
            target_gain: gain as f64,
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
        self.measurement_level = 0.0;
        self.rms_mode = params.rms_mode();
        self.measurement_rms_mode = self.rms_mode;
        self.input_rms.reset();
        self.output_rms.reset();
        self.rms_frames = 0;
        self.rms_quiet_frames = 0;
        self.reset_measurement_history();
        self.reset_measurement_stability();
        self.measurement_rms_candidate_seen = false;
        self.block_input_peak = 0.0;
        self.block_output_peak = 0.0;
        self.measurement_target_db = params.target_db();
        self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
        self.locked_gain_db = sanitize_gain_db(params.locked_gain_db());
        self.current_gain = db_to_linear(self.locked_gain_db) as f64;
        self.target_gain = self.current_gain;
        self.measurement_gain_linear = self.current_gain as f32;
        self.peak_guard_gain = 1.0;
        self.match_fade_position = 1.0;
        self.fade_out_position = 1.0;
        self.fade_out_gain = 0.0;
        self.fade_out_ceiling = 1.0;
        self.last_output_gain = self.current_gain as f32;
        self.last_output_ceiling = 1.0;
        self.last_output_peak = 0.0;
        self.previous_match_request = false;
        self.state = MatchState::Ready;
    }

    /// Apply control changes after a sample-offset parameter event.
    pub fn sync_controls(&mut self, params: &GainSnapParams) {
        let request = params.match_requested();
        let mode_changed = self.rms_mode != params.rms_mode();
        self.rms_mode = params.rms_mode();
        let mut measurement_finished = false;

        if !request {
            if self.previous_match_request && self.state == MatchState::Measuring {
                self.finish_measurement();
                measurement_finished = true;
            }
            self.previous_match_request = false;
        } else if !self.previous_match_request || mode_changed {
            self.measurement_target_db = params.target_db();
            self.measurement_target_peak = 10.0_f32.powf(self.measurement_target_db / 20.0);
            self.state = MatchState::Measuring;
            self.previous_match_request = true;
            self.start_match_transition();
        } else if self.state == MatchState::Measuring {
            let target_db = params.target_db();
            if (target_db - self.measurement_target_db).abs() > f32::EPSILON {
                // A target edit while Match is active starts a fresh
                // measurement. This keeps a Normalize action host-safe even
                // when a host coalesces same-block Match parameter events.
                self.measurement_target_db = target_db;
                self.measurement_target_peak = 10.0_f32.powf(target_db / 20.0);
                self.start_match_transition();
            }
        }

        let stored_gain_db = params.locked_gain_db();
        if !measurement_finished
            && self.state != MatchState::Measuring
            && (stored_gain_db - self.locked_gain_db).abs() > 0.0001
        {
            self.locked_gain_db = stored_gain_db;
            self.target_gain = db_to_linear(stored_gain_db) as f64;
            self.measurement_gain_linear = self.target_gain as f32;
        }
    }

    fn start_match_transition(&mut self) {
        self.measurement_rms_mode = self.rms_mode;
        self.input_rms.reset();
        self.rms_frames = 0;
        self.rms_quiet_frames = 0;
        self.measurement_level = 0.0;
        self.reset_measurement_history();
        self.reset_measurement_stability();
        self.measurement_rms_candidate_seen = false;
        self.force_measurement_update = true;
        // Retain the gain actually heard, including any previous fade/guard.
        // Restarting mid-fade must not jump back to a stored or unity gain.
        self.fade_out_gain = self.last_output_gain;
        self.fade_out_ceiling = self.last_output_ceiling;
        self.fade_out_position = if self.last_output_peak > SILENCE_PEAK_LINEAR {
            0.0
        } else {
            1.0
        };
        self.match_fade_position = 0.0;
    }

    fn reset_measurement_history(&mut self) {
        self.measurement_bucket_position = 0;
        self.measurement_bucket_index = 0;
        self.measurement_buckets = [MeasurementBucket::EMPTY; MATCH_HISTORY_BUCKET_COUNT];
        self.rolling_measurement_peak = 0.0;
        self.rolling_measurement_rms = 0.0;
    }

    fn reset_measurement_stability(&mut self) {
        self.measurement_stable_frames = 0;
        self.measurement_stability_anchor_gain = 0.0;
        self.measurement_stability_anchor_level = 0.0;
    }

    fn observe_measurement_peak(&mut self, peak: f32) -> bool {
        let bucket = &mut self.measurement_buckets[self.measurement_bucket_index];
        bucket.peak = bucket.peak.max(peak);
        if peak > self.rolling_measurement_peak {
            self.rolling_measurement_peak = peak;
            true
        } else {
            false
        }
    }

    fn observe_measurement_rms(&mut self, level: f32) {
        if !level.is_finite() || level <= 0.0 {
            return;
        }
        let bucket = &mut self.measurement_buckets[self.measurement_bucket_index];
        bucket.rms = bucket.rms.max(level);
        self.rolling_measurement_rms = self.rolling_measurement_rms.max(level);
    }

    fn advance_measurement_history(&mut self) {
        if self.measurement_bucket_position + 1 < self.measurement_bucket_frames {
            self.measurement_bucket_position += 1;
            return;
        }

        self.measurement_bucket_position = 0;
        self.measurement_bucket_index =
            (self.measurement_bucket_index + 1) % MATCH_HISTORY_BUCKET_COUNT;
        self.measurement_buckets[self.measurement_bucket_index] = MeasurementBucket::EMPTY;
        self.rolling_measurement_peak = 0.0;
        self.rolling_measurement_rms = 0.0;
        for bucket in &self.measurement_buckets {
            self.rolling_measurement_peak = self.rolling_measurement_peak.max(bucket.peak);
            self.rolling_measurement_rms = self.rolling_measurement_rms.max(bucket.rms);
        }
    }

    fn measurement_candidate_level(&self) -> f32 {
        if self.measurement_rms_mode
            && self.input_rms.complete()
            && self.rolling_measurement_rms > SILENCE_PEAK_LINEAR
        {
            self.rolling_measurement_rms
        } else {
            self.rolling_measurement_peak
        }
    }

    fn gain_for_measurement(&self, level: f32, peak: f32) -> f32 {
        if !level.is_finite() || level <= SILENCE_PEAK_LINEAR {
            return 0.0;
        }
        let requested_gain = self.measurement_target_peak / level;
        let headroom_gain = if self.measurement_rms_mode && peak > SILENCE_PEAK_LINEAR {
            1.0 / peak
        } else {
            GAIN_MAX_LINEAR
        };
        requested_gain
            .min(headroom_gain)
            .clamp(GAIN_MIN_LINEAR, GAIN_MAX_LINEAR)
    }

    fn consider_measurement_candidate(
        &mut self,
        params: &GainSnapParams,
        input_peak: f32,
        first_complete_rms: bool,
    ) {
        let level = self.measurement_candidate_level();
        if level <= SILENCE_PEAK_LINEAR {
            self.reset_measurement_stability();
            return;
        }
        let candidate_gain = self.gain_for_measurement(level, self.rolling_measurement_peak);
        if !candidate_gain.is_finite() || candidate_gain <= 0.0 {
            self.reset_measurement_stability();
            return;
        }

        if self.force_measurement_update || first_complete_rms {
            self.publish_measurement_gain(params, level, candidate_gain);
            self.force_measurement_update = false;
            self.reset_measurement_stability();
            return;
        }

        // Louder evidence lowers the requested gain and takes effect promptly.
        // Only a quieter candidate needs settling, so a decaying tail cannot
        // ratchet the gain upward one sample at a time.
        if candidate_gain < self.measurement_gain_linear {
            self.publish_measurement_gain(params, level, candidate_gain);
            self.reset_measurement_stability();
            return;
        }
        if candidate_gain <= self.measurement_gain_linear {
            self.reset_measurement_stability();
            return;
        }

        let anchor = self.measurement_stability_anchor_gain;
        let anchor_level = self.measurement_stability_anchor_level;
        if anchor <= 0.0
            || !anchor.is_finite()
            || anchor_level <= SILENCE_PEAK_LINEAR
            || !anchor_level.is_finite()
            || candidate_gain < anchor / MATCH_ADAPTATION_STABILITY_RATIO
            || candidate_gain > anchor * MATCH_ADAPTATION_STABILITY_RATIO
            || level < anchor_level / MATCH_ADAPTATION_STABILITY_RATIO
            || level > anchor_level * MATCH_ADAPTATION_STABILITY_RATIO
        {
            self.measurement_stability_anchor_gain = candidate_gain;
            self.measurement_stability_anchor_level = level;
            self.measurement_stable_frames = 0;
        } else {
            self.measurement_stable_frames = self.measurement_stable_frames.saturating_add(1);
        }

        if self.measurement_stable_frames >= self.measurement_stability_window_frames
            && input_peak > SILENCE_PEAK_LINEAR
        {
            self.publish_measurement_gain(params, level, candidate_gain);
            self.reset_measurement_stability();
        }
    }

    fn publish_measurement_gain(&mut self, params: &GainSnapParams, level: f32, gain: f32) {
        let gain_db = (20.0 * gain.log10()).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        self.measurement_level = level;
        self.measurement_gain_linear = gain;
        self.locked_gain_db = sanitize_gain_db(gain_db);
        self.target_gain = db_to_linear(self.locked_gain_db) as f64;
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
            if self.rms_mode {
                if input_peak > SILENCE_PEAK_LINEAR {
                    if self.rms_quiet_frames >= self.rms_silence_frames {
                        // A new signal after silence gets the same protected startup.
                        self.start_match_transition();
                    }
                    self.rms_quiet_frames = 0;
                } else {
                    self.rms_quiet_frames = self.rms_quiet_frames.saturating_add(1);
                }
                let observed_signal = self.rolling_measurement_peak > SILENCE_PEAK_LINEAR
                    || input_peak > SILENCE_PEAK_LINEAR;
                if self.rms_quiet_frames < self.rms_silence_frames && observed_signal {
                    self.input_rms.update(input_left, input_right);
                    self.rms_frames = self.rms_frames.saturating_add(1);
                    rms_updated = true;
                }
            }

            let new_peak = self.observe_measurement_peak(input_peak);
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
                self.consider_measurement_candidate(params, input_peak, first_complete_rms);
            }
            self.advance_measurement_history();
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
        let fading_out = self.fade_out_position < 1.0;
        if !fading_out
            && (self.state != MatchState::Measuring
                || self.rolling_measurement_peak > SILENCE_PEAK_LINEAR)
        {
            self.match_fade_position = (position + self.match_fade_step).min(1.0);
        }
        let desired_gain = self.current_gain as f32;
        let ceiling = if !self.measurement_rms_mode
            && (self.state == MatchState::Measuring || position < 1.0)
        {
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
        let (output_left, output_right) = if fading_out {
            // The outgoing path keeps its previous ceiling during this short
            // fade. Switching to the new target ceiling immediately would
            // recreate the discontinuity this transition is meant to remove.
            // Its gain can only decrease, even if a louder input arrives.
            let old_ceiling = self.fade_out_ceiling;
            if input_peak > 0.0 {
                self.fade_out_gain = self.fade_out_gain.min(old_ceiling / input_peak);
            }
            let p = self.fade_out_position;
            let outgoing = 1.0 - p * p * (3.0 - 2.0 * p);
            self.fade_out_position = (p + self.fade_out_step).min(1.0);
            self.last_output_gain = self.fade_out_gain * outgoing;
            self.last_output_ceiling = old_ceiling * outgoing;
            (
                (input_left * self.fade_out_gain).clamp(-old_ceiling, old_ceiling) * outgoing,
                (input_right * self.fade_out_gain).clamp(-old_ceiling, old_ceiling) * outgoing,
            )
        } else {
            self.last_output_gain = applied_gain * fade;
            self.last_output_ceiling = ceiling * fade;
            (
                (input_left * applied_gain).clamp(-ceiling, ceiling) * fade,
                (input_right * applied_gain).clamp(-ceiling, ceiling) * fade,
            )
        };
        self.output_rms.update(output_left, output_right);
        self.last_output_peak = output_left.abs().max(output_right.abs());
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
            locked_gain_db: self.locked_gain_db,
        }
    }

    fn finish_measurement(&mut self) {
        if self.measurement_level <= SILENCE_PEAK_LINEAR {
            self.state = MatchState::NoSignal;
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

    fn rms_params() -> GainSnapParams {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_RMS_MODE, 1.0);
        params.set_param(PARAM_MATCH, 1.0);
        params
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
    fn short_gaps_between_hits_do_not_restart_the_rms_startup_fade() {
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
    fn rms_adapts_to_quieter_material_and_holds_gain_when_match_stops() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.5, 96_000);
        run_frames(&mut engine, &params, 0.125, 216_000);
        assert!(
            (engine.report().output_rms_db + 12.0).abs() < 0.03,
            "{} gain {} level {} frames {}",
            engine.report().output_rms_db,
            params.locked_gain_db(),
            engine.measurement_level,
            engine.rms_frames
        );
        assert!((params.locked_gain_db() - 6.0618).abs() < 0.02);
        let gain = params.locked_gain_db();
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        run_frames(&mut engine, &params, 0.0625, 144_000);
        assert!((params.locked_gain_db() - gain).abs() < 0.01);
        assert!(
            (engine.report().output_rms_db + 18.0206).abs() < 0.03,
            "{} gain {} current {}",
            engine.report().output_rms_db,
            params.locked_gain_db(),
            linear_to_db(engine.current_gain as f32)
        );
    }

    #[test]
    fn matching_adapts_loud_quiet_loud_repeatedly_without_toggle() {
        for rms in [false, true] {
            let params = GainSnapParams::new();
            params.set_param(PARAM_TARGET_DB, -12.0);
            params.set_param(crate::params::PARAM_RMS_MODE, f32::from(rms));
            params.set_param(PARAM_MATCH, 1.0);
            let mut engine = GainSnapEngine::new(1_000.0, 0.0);
            engine.begin_block(&params);

            run_frames(&mut engine, &params, 0.5, 3_200);
            let loud_gain = params.locked_gain_db();
            assert!((loud_gain + 5.9794).abs() < 0.02, "rms={rms}: {loud_gain}");

            // Recent loud evidence keeps a quiet transition from boosting on
            // its first samples.
            run_frames(&mut engine, &params, 0.125, 100);
            assert!((params.locked_gain_db() - loud_gain).abs() < 0.02);
            run_frames(&mut engine, &params, 0.125, 4_000);
            assert!((params.locked_gain_db() - 6.0618).abs() < 0.2);
            assert!(
                (linear_to_db(engine.current_gain as f32) - 6.0618).abs() < 0.2,
                "rms={rms}: locked {} current {} level {} rolling peak {} rolling rms {} anchor {} anchor level {} stable {}",
                params.locked_gain_db(),
                linear_to_db(engine.current_gain as f32),
                engine.measurement_level,
                engine.rolling_measurement_peak,
                engine.rolling_measurement_rms,
                engine.measurement_stability_anchor_gain,
                engine.measurement_stability_anchor_level,
                engine.measurement_stable_frames,
            );

            // A louder transition lowers gain as soon as the new peak is
            // usable; RMS mode confirms it after its full window completes.
            run_frames(&mut engine, &params, 0.5, 500);
            assert!((params.locked_gain_db() + 5.9794).abs() < 0.2);

            run_frames(&mut engine, &params, 0.125, 4_000);
            assert!((params.locked_gain_db() - 6.0618).abs() < 0.2);
            run_frames(&mut engine, &params, 0.5, 500);
            assert!((params.locked_gain_db() + 5.9794).abs() < 0.2);
            assert_eq!(engine.report().state, MatchState::Measuring);
            assert!(params.match_requested());
        }
    }

    #[test]
    fn peak_matching_does_not_progressively_boost_a_decaying_tail() {
        let params = GainSnapParams::new();
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
    fn rms_headroom_recovers_after_a_recent_crest_expires() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(1_000.0, 0.0);
        engine.begin_block(&params);

        let sine_amplitude = 0.25 * 2.0_f32.sqrt();
        for frame in 0..3_500 {
            let phase = std::f32::consts::TAU * frame as f32 / 20.0;
            engine.process_frame(&params, phase.sin() * sine_amplitude, 0.0);
        }
        let sine_gain = params.locked_gain_db();
        assert!((sine_gain - 9.0309).abs() < 0.2, "{sine_gain}");

        // The square wave has the same 0.25 RMS but a lower 0.25 peak. Once
        // the sine crest leaves the recent history, its available headroom
        // grows to the full +12.04 dB requested correction.
        for frame in 0..3_800 {
            let input = if frame % 2 == 0 { 0.25 } else { -0.25 };
            engine.process_frame(&params, input, input);
        }
        assert!((params.locked_gain_db() - 12.0412).abs() < 0.2);
        assert!((linear_to_db(engine.current_gain as f32) - 12.0412).abs() < 0.2);
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
    ) -> (f32, f32) {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, target_db);
        let mut engine = GainSnapEngine::new(sample_rate as f32, 0.0);
        engine.begin_block(&params);
        let beat_frames = sample_rate * 60 / bpm;
        let release_at = beat_frames * 4 + (beat_frames as f32 * release_phase) as usize;
        let mut output_peak = 0.0_f32;
        for frame in 0..=release_at {
            let input = damped_kick(frame, sample_rate, bpm);
            let output = engine.process_frame(&params, input, -input * 0.7);
            assert!(output.0.is_finite() && output.1.is_finite());
            if frame >= beat_frames * 2 {
                output_peak = output_peak.max(output.0.abs()).max(output.1.abs());
            }
        }
        let gain = params.locked_gain_db();
        params.set_param(PARAM_MATCH, 0.0);
        engine.sync_controls(&params);
        (gain, output_peak)
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
                // -18 dBFS has crest headroom for this fixture, so the
                // repeated hits retain their envelope without guard limiting.
                assert!(early.1 < 1.0 && late.1 < 1.0);
            }
        }
    }

    #[test]
    fn rms_kick_headroom_bounds_unreachable_targets_before_the_guard() {
        for target in [-12.0, 0.0] {
            let (gain, output_peak) = measure_kicks(48_000, 120, target, 0.8);
            assert!(output_peak <= 1.0);
            assert!(gain < 3.0, "target={target} gain={gain}");
        }
    }

    #[test]
    #[ignore = "prints representative kick gain and output-peak measurements"]
    fn rms_kick_fixture_metrics() {
        for target in [-18.0, -12.0, 0.0] {
            let (gain, output_peak) = measure_kicks(48_000, 120, target, 0.8);
            println!(
                "target={target:.0} dBFS, held gain={gain:.3} dB, output peak={output_peak:.6}"
            );
        }
    }

    #[test]
    fn rms_short_one_shot_completes_through_zeroes_and_off_keeps_peak_startup() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        for frame in 0..14_401 {
            let input = if frame < 960 { 0.5 } else { 0.0 };
            engine.process_frame(&params, input, input);
        }
        assert!(engine.input_rms.complete());
        assert!(engine.rolling_measurement_rms > SILENCE_PEAK_LINEAR);
        assert!(
            engine.locked_gain_db > -5.0,
            "the first complete RMS window should publish after the impulse"
        );

        let early_params = rms_params();
        let mut early = GainSnapEngine::new(48_000.0, 0.0);
        early.begin_block(&early_params);
        run_frames(&mut early, &early_params, 0.5, 960);
        early_params.set_param(PARAM_MATCH, 0.0);
        early.sync_controls(&early_params);
        assert!((early_params.locked_gain_db() + 5.9794).abs() < 0.02);
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
    fn rms_silence_does_not_raise_gain_and_returning_signal_fades_safely() {
        let params = rms_params();
        let mut engine = GainSnapEngine::new(48_000.0, 24.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 0.0, 96_000);
        assert_eq!(engine.process_frame(&params, 0.1, -0.1).0, 0.0);
        run_frames(&mut engine, &params, 0.1, 96_000);
        let gain = params.locked_gain_db();
        run_frames(&mut engine, &params, 0.0, 144_000);
        assert_eq!(params.locked_gain_db(), gain);
        assert_eq!(engine.process_frame(&params, 1.0, -1.0).0, 0.0);
        for n in 0..48_000 {
            let x = if n % 2 == 0 { f32::MAX } else { f32::NAN };
            let (left, right) = engine.process_frame(&params, x, -x);
            assert!(left.is_finite() && right.is_finite());
            assert!(left.abs() <= 1.0 && right.abs() <= 1.0);
        }
    }

    #[test]
    fn mode_changes_during_matching_preserve_the_first_output_sample() {
        let params = GainSnapParams::new();
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
    fn rms_zero_target_keeps_the_peak_guard_even_when_average_cannot_reach_target() {
        let params = rms_params();
        params.set_param(PARAM_TARGET_DB, 0.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        for n in 0..144_000 {
            let input = (std::f32::consts::TAU * n as f32 / 48.0).sin() * 0.5;
            let (left, right) = engine.process_frame(&params, input, -input);
            assert!(left.abs() <= 1.0 && right.abs() <= 1.0);
        }
        assert!(engine.report().output_rms_db < -2.0);
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
        let before_restart = engine.process_frame(&params, 1.0, -1.0);
        params.set_param(PARAM_MATCH, 1.0);
        engine.sync_controls(&params);
        let after_restart = engine.process_frame(&params, 1.0, -1.0);
        assert!((after_restart.0 - before_restart.0).abs() < 1.0e-6);
        run_frames(&mut engine, &params, 1.0, 480);
        assert!(engine.process_frame(&params, 1.0, -1.0).0 < 0.0001);
    }

    #[test]
    fn target_edits_fade_out_before_enforcing_the_new_ceiling() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, 0.0);
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        run_frames(&mut engine, &params, 1.0, 48_000);
        params.set_param(PARAM_TARGET_DB, -36.0);
        engine.sync_controls(&params);
        let (left, right) = engine.process_frame(&params, 1.0, 0.5);
        assert!((left - 1.0).abs() < 0.0001);
        assert_eq!(right, left * 0.5);
        run_frames(&mut engine, &params, 1.0, 480);
        for _ in 0..48_000 {
            let (left, right) = engine.process_frame(&params, 1.0, 0.5);
            assert!(left <= 10.0_f32.powf(-36.0 / 20.0));
            assert_eq!(right, left * 0.5);
        }
    }

    #[test]
    fn engaging_match_on_playing_audio_has_no_hard_mute_or_large_step() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for target in [-36.0, -12.0, 0.0] {
                let params = GainSnapParams::new();
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
                for _ in 0..sample_rate as usize {
                    let (left, right) = engine.process_frame(&params, 0.5, -0.25);
                    assert!(
                        (left - previous).abs() < 0.002,
                        "Match introduced a step at {sample_rate} Hz: {previous} -> {left}"
                    );
                    assert_eq!(right, -left * 0.5);
                    quietest = quietest.min(left);
                    previous = left;
                }
                assert!(quietest < 1.0e-6, "transition must pass through silence");
                assert!((linear_to_db(previous) - target).abs() < 0.002);
            }
        }
    }

    #[test]
    fn rapid_match_restarts_keep_the_current_audible_gain() {
        for restart_after in [1, 17, 240, 481, 2_000] {
            let params = GainSnapParams::new();
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
            let params = GainSnapParams::new();
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
