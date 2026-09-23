//! Realtime status values shared with the GainSnap editor.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::params::{GAIN_MAX_DB, GAIN_MIN_DB};

/// Lifecycle status reported while GainSnap is measuring or holding a gain
/// value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MatchState {
    /// Match is disabled and the engine is ready for a new measurement.
    Ready = 0,
    /// The matcher is collecting the input peak while Match is enabled.
    Measuring = 1,
    /// A gain has been calculated and is being held.
    Locked = 2,
    /// Match was disabled without a usable finite signal being measured.
    NoSignal = 3,
}

/// Informational activity reported by the live matcher.
///
/// This is intentionally separate from [`MatchState`]. MatchState describes
/// the enabled/disabled lifecycle, while activity describes what the audio
/// engine is doing during a continuous Match pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MatchActivity {
    /// Match is disabled before a measurement has been held.
    Ready = 0,
    /// No usable finite signal is currently available.
    NoSignal = 1,
    /// Match is disabled and the last calculated gain is being held.
    Held = 2,
    /// Match is enabled and waiting for usable measurement evidence.
    Listening = 3,
    /// Match is applying or waiting for a gain correction to settle.
    Adjusting = 4,
    /// Match is enabled and the correction has settled.
    Matched = 5,
}

impl MatchActivity {
    /// Map the lifecycle state to the safe activity fallback used by legacy
    /// status publishers that do not yet provide live telemetry.
    #[allow(dead_code)]
    pub const fn from_match_state(state: MatchState) -> Self {
        match state {
            MatchState::Ready => Self::Ready,
            MatchState::Measuring => Self::Listening,
            MatchState::Locked => Self::Held,
            MatchState::NoSignal => Self::NoSignal,
        }
    }

    /// Decode the compact atomic representation, defaulting safely to Ready.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn from_raw(value: u32) -> Self {
        match value {
            1 => Self::NoSignal,
            2 => Self::Held,
            3 => Self::Listening,
            4 => Self::Adjusting,
            5 => Self::Matched,
            _ => Self::Ready,
        }
    }
}

impl MatchState {
    /// Decode the compact atomic representation, defaulting safely to ready.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn from_raw(value: u32) -> Self {
        match value {
            1 => Self::Measuring,
            2 => Self::Locked,
            3 => Self::NoSignal,
            _ => Self::Ready,
        }
    }
}

/// Atomically published metering and matcher status.
pub struct GuiStatus {
    input_peak_db: AtomicU32,
    output_peak_db: AtomicU32,
    output_rms_db: AtomicU32,
    locked_gain_db: AtomicU32,
    progress: AtomicU32,
    state: AtomicU32,
    activity: AtomicU32,
}

impl Default for GuiStatus {
    fn default() -> Self {
        Self::new()
    }
}

impl GuiStatus {
    /// Construct status initialized to a quiet, ready plugin.
    pub fn new() -> Self {
        Self {
            input_peak_db: AtomicU32::new((-120.0_f32).to_bits()),
            output_peak_db: AtomicU32::new((-120.0_f32).to_bits()),
            output_rms_db: AtomicU32::new((-120.0_f32).to_bits()),
            locked_gain_db: AtomicU32::new(0.0_f32.to_bits()),
            progress: AtomicU32::new(0.0_f32.to_bits()),
            state: AtomicU32::new(MatchState::Ready as u32),
            activity: AtomicU32::new(MatchActivity::Ready as u32),
        }
    }

    /// Publish one audio-block status snapshot.
    #[allow(dead_code)]
    pub fn update(
        &self,
        input_peak_db: f32,
        output_peak_db: f32,
        locked_gain_db: f32,
        progress: f32,
        state: MatchState,
    ) {
        self.update_with_activity(
            input_peak_db,
            output_peak_db,
            locked_gain_db,
            progress,
            state,
            MatchActivity::from_match_state(state),
        );
    }

    /// Publish one audio-block status snapshot with live matcher activity.
    pub fn update_with_activity(
        &self,
        input_peak_db: f32,
        output_peak_db: f32,
        locked_gain_db: f32,
        progress: f32,
        state: MatchState,
        activity: MatchActivity,
    ) {
        self.input_peak_db
            .store(sanitize_db(input_peak_db).to_bits(), Ordering::Relaxed);
        self.output_peak_db
            .store(sanitize_db(output_peak_db).to_bits(), Ordering::Relaxed);
        self.locked_gain_db.store(
            sanitize_gain_db(locked_gain_db).to_bits(),
            Ordering::Relaxed,
        );
        self.progress
            .store(progress.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.state.store(state as u32, Ordering::Relaxed);
        self.activity.store(activity as u32, Ordering::Relaxed);
    }

    /// Publish protected output RMS without changing peak telemetry.
    pub fn update_rms(&self, db: f32) {
        self.output_rms_db
            .store(sanitize_db(db).to_bits(), Ordering::Relaxed);
    }

    /// Read the selected output measurement for the editor.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn output_level_db(&self, rms: bool) -> f32 {
        if rms {
            read_f32(&self.output_rms_db)
        } else {
            self.output_peak_db()
        }
    }

    /// Read the most recent input peak in decibels.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    #[allow(dead_code)]
    pub fn input_peak_db(&self) -> f32 {
        read_f32(&self.input_peak_db)
    }

    /// Read the most recent output peak in decibels.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn output_peak_db(&self) -> f32 {
        read_f32(&self.output_peak_db)
    }

    /// Read the held gain in decibels.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    #[allow(dead_code)]
    pub fn locked_gain_db(&self) -> f32 {
        read_f32(&self.locked_gain_db)
    }

    /// Read measurement progress from zero to one.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    #[allow(dead_code)]
    pub fn progress(&self) -> f32 {
        read_f32(&self.progress).clamp(0.0, 1.0)
    }

    /// Read the current matcher state.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn state(&self) -> MatchState {
        MatchState::from_raw(self.state.load(Ordering::Relaxed))
    }

    /// Read the live matcher activity.
    #[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
    pub fn activity(&self) -> MatchActivity {
        MatchActivity::from_raw(self.activity.load(Ordering::Relaxed))
    }
}

#[cfg(all(any(target_os = "macos", target_os = "windows"), feature = "gpui-gui"))]
fn read_f32(value: &AtomicU32) -> f32 {
    f32::from_bits(value.load(Ordering::Relaxed))
}

fn sanitize_db(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-120.0, 24.0)
    } else {
        -120.0
    }
}

fn sanitize_gain_db(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(GAIN_MIN_DB, GAIN_MAX_DB)
    } else {
        0.0
    }
}
