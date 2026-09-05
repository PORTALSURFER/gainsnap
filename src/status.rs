//! Realtime status values shared with the GainSnap editor.

use std::sync::atomic::{AtomicU32, Ordering};

/// Status reported while GainSnap is measuring or holding a gain value.
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

impl MatchState {
    /// Decode the compact atomic representation, defaulting safely to ready.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
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
    locked_gain_db: AtomicU32,
    progress: AtomicU32,
    state: AtomicU32,
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
            locked_gain_db: AtomicU32::new(0.0_f32.to_bits()),
            progress: AtomicU32::new(0.0_f32.to_bits()),
            state: AtomicU32::new(MatchState::Ready as u32),
        }
    }

    /// Publish one audio-block status snapshot.
    pub fn update(
        &self,
        input_peak_db: f32,
        output_peak_db: f32,
        locked_gain_db: f32,
        progress: f32,
        state: MatchState,
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
    }

    /// Read the most recent input peak in decibels.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    #[allow(dead_code)]
    pub fn input_peak_db(&self) -> f32 {
        read_f32(&self.input_peak_db)
    }

    /// Read the most recent output peak in decibels.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    pub fn output_peak_db(&self) -> f32 {
        read_f32(&self.output_peak_db)
    }

    /// Read the held gain in decibels.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    pub fn locked_gain_db(&self) -> f32 {
        read_f32(&self.locked_gain_db)
    }

    /// Read measurement progress from zero to one.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    pub fn progress(&self) -> f32 {
        read_f32(&self.progress).clamp(0.0, 1.0)
    }

    /// Read the current matcher state.
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    pub fn state(&self) -> MatchState {
        MatchState::from_raw(self.state.load(Ordering::Relaxed))
    }
}

#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
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
        value.clamp(-24.0, 24.0)
    } else {
        0.0
    }
}
