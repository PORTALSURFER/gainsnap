//! Native GPUI editor for GainSnap on macOS and Windows.
//!
//! The editor keeps the compact GainSnap surface deliberately small: the
//! controller owns only retained UI state and forwards parameter gestures to
//! the CLAP queue or the VST3 component handler. GPUI owns drawing, focus,
//! pointer routing, and native text input/IME transport.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use toybox::clack_plugin::utils::ClapId;
use toybox::clap::automation::{AutomationConfig, AutomationQueue};
use toybox::gpui::{
    self as gpui, canvas, div, fill, font, linear_color_stop, linear_gradient, point, prelude::*,
    px, rgba, App, Bounds, Context, Entity, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Subscription, TextRun, Window,
};
use toybox::gpui_gui::{
    NumericInput, NumericInputCanceled, NumericInputChanged, NumericInputConfig, NumericInputRange,
    NumericInputStepped, NumericInputStyle, NumericInputSubmitted,
};

use crate::clap_plugin::HostParamRequester;
use crate::params::{
    GAIN_MAX_DB, GAIN_MIN_DB, MANUAL_GAIN_MAX_DB, MANUAL_GAIN_MIN_DB, PARAM_LOCKED_GAIN_DB,
    PARAM_MATCH, PARAM_RMS_MODE, PARAM_TARGET_DB, TARGET_MAX_DB, TARGET_MIN_DB,
};
#[cfg(feature = "screenshot-test")]
use crate::params::{PARAM_MANUAL_GAIN_DB, PARAM_MANUAL_MODE};
use crate::status::{GuiStatus, MatchActivity, MatchState};

/// Preferred logical editor width for the compact toggle control surface.
pub const WINDOW_WIDTH: u32 = 200;
/// Preferred logical editor height for the compact toggle control surface.
pub const WINDOW_HEIGHT: u32 = 424;
/// Minimum logical editor width.
pub const MIN_WINDOW_WIDTH: u32 = WINDOW_WIDTH;
/// Minimum logical editor height.
pub const MIN_WINDOW_HEIGHT: u32 = WINDOW_HEIGHT;
/// Maximum logical editor width.
pub const MAX_WINDOW_WIDTH: u32 = 480;
/// Maximum logical editor height.
pub const MAX_WINDOW_HEIGHT: u32 = 520;

const TARGET_TEXT_SYNC_EPSILON: f32 = 0.0001;
const TARGET_CONTROL_WIDTH: f32 = 90.0;
const TARGET_METER_HEIGHT: f32 = 296.0;
const TARGET_ENTRY_HEIGHT: f32 = 24.0;
const ACTIVITY_RAIL_HEIGHT: f32 = 2.0;
const ACTIVITY_RAIL_GAP: f32 = 7.0;
const ACTIVITY_SEGMENT_HEIGHT: f32 = 2.0;
const ACTIVITY_RAIL_BOTTOM_INSET: f32 = 14.0;
const ACTIVITY_LABEL_HEIGHT: f32 = 12.0;
const TARGET_METER_TRACK_WIDTH: f32 = 25.0;
const TARGET_METER_VERTICAL_INSET: f32 = 2.0;
const TARGET_METER_TICK_COUNT: usize = 13;
const TARGET_METER_TICK_WIDTH: f32 = 3.0;
const TARGET_METER_TICK_HEIGHT: f32 = 1.0;
const TARGET_METER_LABELS: [(&str, f32); 6] = [
    ("0", 0.0),
    ("−12", -12.0),
    ("−24", -24.0),
    ("−36", -36.0),
    ("−48", -48.0),
    ("−60", -60.0),
];
const TARGET_METER_LABEL_WIDTH: f32 = 21.0;
const TARGET_METER_LABEL_GAP: f32 = 0.0;
const TARGET_METER_LABEL_FONT_SIZE: f32 = 8.0;
const TARGET_METER_LABEL_ALPHA: u8 = 190;
const TARGET_MARKER_GAP: f32 = 7.0;
const TARGET_MARKER_WIDTH: f32 = 6.0;
const TARGET_MARKER_HEIGHT: f32 = 8.0;
const GAIN_MARKER_GAP: f32 = 3.0;
const METER_ATTACK_SECONDS: f32 = 0.010;
const METER_RELEASE_SECONDS: f32 = 0.300;
const METER_MAX_ELAPSED_SECONDS: f32 = 0.100;
const METER_SETTLE_EPSILON_DB: f32 = 0.01;
const PEAK_HOLD_SECONDS: f32 = 2.0;
const PEAK_HOLD_RELEASE_DB_PER_SECOND: f32 = 1.5;
const TARGET_KEYBOARD_STEP_DB: f32 = 1.0;
const TARGET_FINE_KEYBOARD_STEP_DB: f32 = 0.1;
const GAIN_MARKER_NEUTRAL_DB: f32 = -12.0;
const GAIN_MARKER_HIGH_DB: f32 = 36.0;
const GAIN_MARKER_HIGH_LEVEL_DB: f32 = -3.0;
const METER_LEFT: f32 = 10.0;
const METER_TOP: f32 = 64.0;
const TARGET_ENTRY_LEFT: f32 = 34.0;
const TARGET_ENTRY_TOP: f32 = 374.0;
const TARGET_ENTRY_WIDTH: f32 = 56.0;
const MATCH_LEFT: f32 = 98.0;
const MATCH_TOP: f32 = 374.0;
const MATCH_WIDTH: f32 = 54.0;
const MATCH_HEIGHT: f32 = 28.0;
const RESTART_LEFT: f32 = 160.0;
const RESTART_TOP: f32 = 374.0;
const RESTART_WIDTH: f32 = 24.0;
const RESTART_HEIGHT: f32 = 28.0;
const KNOB_LEFT: f32 = 122.0;
const KNOB_TOP: f32 = 282.0;
const KNOB_SIZE: f32 = 44.0;
const GAIN_ENTRY_LEFT: f32 = 120.0;
const GAIN_ENTRY_TOP: f32 = 334.0;
const GAIN_ENTRY_WIDTH: f32 = 48.0;
const INFO_LEFT: f32 = 108.0;
const MATCH_PULSE_SECONDS: f32 = 1.2;
const TARGET_ENTRY_AUTOMATION_LABEL: &str = "Target level, dBFS";
const MATCH_AUTOMATION_DESCRIPTION: &str = "Match the output to the selected target";
const RESTART_AUTOMATION_DESCRIPTION: &str = "Restart matching measurement";

// The original GainSnap dark theme values are part of the compact visual
// contract preserved by the GPUI editor.
const BG_PRIMARY: u32 = 0x272b28;
const BORDER: u32 = 0x354038;
const BORDER_EMPHASIS: u32 = 0x49534c;
const ACCENT: u32 = 0xe96b50;
const RMS_COLOR: u32 = 0x7658ae;
const ACCENT_DANGER: u32 = 0xe96b50;
const TEXT_PRIMARY: u32 = 0xc8cdc9;
const TEXT_MUTED: u32 = 0x929b95;
const TRACK_INSET: u32 = 0x141b16;
const OUTLINE: u32 = 0x667369;
const BODY_TOP: u32 = 0x2c312c;
const RECESS_TOP: u32 = 0x202622;
const RECESS_BOTTOM: u32 = 0x1b201d;
const BUTTON_IDLE_TOP: u32 = 0x414e47;
const BUTTON_IDLE_BOTTOM: u32 = 0x2a302c;
const BUTTON_ACTIVE_TOP: u32 = 0xb65c32;
const BUTTON_ACTIVE_BOTTOM: u32 = 0x813d24;
const KNOB_OUTER_TOP: u32 = 0x697469;
const KNOB_OUTER_BOTTOM: u32 = 0x343e36;
const KNOB_RING_TOP: u32 = 0x4e5d51;
const KNOB_RING_BOTTOM: u32 = 0x303c32;
const KNOB_CAP_TOP: u32 = 0x343e36;
const KNOB_CAP_BOTTOM: u32 = 0x222a24;
const FIELD_TOP: u32 = 0x222823;
const FIELD_BOTTOM: u32 = 0x1b211d;

fn solid(value: u32) -> gpui::Rgba {
    rgba((value << 8) | 0xff)
}

fn vertical_gradient(top: u32, bottom: u32) -> gpui::Background {
    linear_gradient(
        180.0,
        linear_color_stop(solid(top), 0.0),
        linear_color_stop(solid(bottom), 1.0),
    )
}

fn vertical_gradient_alpha(top: u32, bottom: u32, alpha: u8) -> gpui::Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(top << 8 | u32::from(alpha)), 0.0),
        linear_color_stop(rgba(bottom << 8 | u32::from(alpha)), 1.0),
    )
}

/// Format-neutral host automation sink used by the VST3 editor.
pub(crate) trait HostParamEditSink: Send + Sync {
    /// Begin a host gesture for a parameter.
    fn gesture_started(&self, config: &AutomationConfig, param_id: ClapId);
    /// Send one plain parameter value during a host gesture.
    fn gesture_value(&self, config: &AutomationConfig, param_id: ClapId, value: f64);
    /// End a host gesture for a parameter.
    fn gesture_ended(&self, config: &AutomationConfig, param_id: ClapId);
}

#[derive(Clone, Copy, Debug)]
struct ParamRange {
    min: f32,
    max: f32,
    default: f32,
}

impl ParamRange {
    const fn new(min: f32, max: f32, default: f32) -> Self {
        Self { min, max, default }
    }

    fn normalize(self, value: f32) -> f32 {
        ((value - self.min) / (self.max - self.min).max(f32::EPSILON)).clamp(0.0, 1.0)
    }

    fn denormalize(self, value: f32) -> f32 {
        (self.min + value.clamp(0.0, 1.0) * (self.max - self.min)).clamp(self.min, self.max)
    }
}

const TARGET_RANGE: ParamRange = ParamRange::new(TARGET_MIN_DB, TARGET_MAX_DB, -12.0);
const FULL_SCALE_RANGE: ParamRange = ParamRange::new(-60.0, 0.0, -12.0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetStepDirection {
    Up,
    Down,
}

impl TargetStepDirection {
    fn sign(self) -> f32 {
        match self {
            Self::Up => 1.0,
            Self::Down => -1.0,
        }
    }
}

fn step_target_db(target_db: f32, direction: TargetStepDirection, shift_held: bool) -> f32 {
    let step_db = if shift_held {
        TARGET_FINE_KEYBOARD_STEP_DB
    } else {
        TARGET_KEYBOARD_STEP_DB
    };
    (target_db + direction.sign() * step_db).clamp(TARGET_RANGE.min, TARGET_RANGE.max)
}

fn snap_drag_db(value_db: f32, shift_held: bool, min_db: f32, max_db: f32) -> f32 {
    let value_db = if shift_held {
        value_db.round()
    } else {
        value_db
    };
    value_db.clamp(min_db, max_db)
}

fn format_target_text(value: f32) -> String {
    format!("{value:.1}")
}

fn parse_target_text(text: &str) -> Option<f32> {
    let raw = text.trim();
    let raw = raw
        .strip_suffix("dBFS")
        .or_else(|| raw.strip_suffix("dB"))
        .unwrap_or(raw)
        .trim();
    let value = raw.parse::<f32>().ok()?;
    value
        .is_finite()
        .then(|| value.clamp(TARGET_RANGE.min, TARGET_RANGE.max))
}

fn format_gain_text(value: f32) -> String {
    if value == 0.0 {
        "0.0".into()
    } else {
        format!("{value:+.1}")
    }
}

fn format_meter_readout(value_db: f32) -> String {
    if !value_db.is_finite() || value_db <= -60.0 {
        "−∞".to_owned()
    } else if value_db < 0.0 {
        format!("−{:.1}", -value_db)
    } else {
        format!("{value_db:.1}")
    }
}

fn parse_gain_text(text: &str) -> Option<f32> {
    let raw = text.trim().strip_suffix("dB").unwrap_or(text.trim()).trim();
    let value = raw.parse::<f32>().ok()?;
    value
        .is_finite()
        .then(|| value.clamp(GAIN_MIN_DB, GAIN_MAX_DB))
}

fn clamp_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_meter_level_db(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-120.0, TARGET_MAX_DB)
    } else {
        -120.0
    }
}

fn smooth_meter_level_db(current_db: f32, target_db: f32, elapsed: Duration) -> f32 {
    let current_db = sanitize_meter_level_db(current_db);
    let target_db = sanitize_meter_level_db(target_db);
    let elapsed_seconds = elapsed.as_secs_f32().min(METER_MAX_ELAPSED_SECONDS);
    if elapsed_seconds <= 0.0 || (target_db - current_db).abs() <= METER_SETTLE_EPSILON_DB {
        return if (target_db - current_db).abs() <= METER_SETTLE_EPSILON_DB {
            target_db
        } else {
            current_db
        };
    }
    let response_seconds = if target_db > current_db {
        METER_ATTACK_SECONDS
    } else {
        METER_RELEASE_SECONDS
    };
    let amount = (elapsed_seconds / response_seconds).clamp(0.0, 1.0);
    sanitize_meter_level_db(current_db + (target_db - current_db) * amount)
}

#[derive(Clone, Copy, Debug)]
struct PeakHold {
    level_db: f32,
    hold_until: Instant,
    last_update: Instant,
}

impl PeakHold {
    fn new(level_db: f32, now: Instant) -> Self {
        let level_db = sanitize_meter_level_db(level_db);
        Self {
            level_db,
            hold_until: if level_db > -120.0 {
                now + Duration::from_secs_f32(PEAK_HOLD_SECONDS)
            } else {
                now
            },
            last_update: now,
        }
    }

    /// Promote new highs immediately, then release toward the live meter after
    /// a fixed hold. `live_db` keeps the line above both raw and smoothed bars.
    fn advance_at(&mut self, observed_db: f32, live_db: f32, now: Instant) -> bool {
        let observed_db = sanitize_meter_level_db(observed_db);
        let floor_db = observed_db.max(sanitize_meter_level_db(live_db));
        let before = self.level_db;
        if observed_db >= self.level_db && observed_db > -120.0 {
            self.level_db = self.level_db.max(observed_db);
            self.hold_until = now + Duration::from_secs_f32(PEAK_HOLD_SECONDS);
        } else if floor_db > self.level_db {
            self.level_db = floor_db;
        } else {
            let release_from = self.last_update.max(self.hold_until);
            if now > release_from {
                let released = self.level_db
                    - now.saturating_duration_since(release_from).as_secs_f32()
                        * PEAK_HOLD_RELEASE_DB_PER_SECOND;
                self.level_db = released.max(floor_db);
            }
        }
        self.level_db = self.level_db.max(floor_db);
        self.last_update = now;
        (self.level_db - before).abs() > f32::EPSILON
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisplaySnapshot {
    target_db: u32,
    match_requested: bool,
    rms_mode: bool,
    output_peak_db: u32,
    output_rms_db: u32,
    gain_db: u32,
    state: MatchState,
    activity: MatchActivity,
    applied_gain_db: u32,
    shortfall_db: u32,
}

impl DisplaySnapshot {
    fn capture(params: &crate::params::GainSnapParams, status: &GuiStatus) -> Self {
        Self {
            target_db: params
                .get_param(PARAM_TARGET_DB)
                .unwrap_or(TARGET_RANGE.default)
                .clamp(TARGET_RANGE.min, TARGET_RANGE.max)
                .to_bits(),
            match_requested: params.match_requested(),
            rms_mode: params.rms_mode(),
            output_peak_db: sanitize_meter_level_db(status.output_peak_db()).to_bits(),
            output_rms_db: sanitize_meter_level_db(status.output_rms_db()).to_bits(),
            gain_db: params.locked_gain_db().to_bits(),
            state: status.state(),
            activity: status.activity(),
            applied_gain_db: status.applied_gain_db().to_bits(),
            shortfall_db: status.target_shortfall_db().to_bits(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum InfoPanel {
    Help,
}

struct ControlHint(&'static str);
impl Render for ControlHint {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .w(px(176.0))
            .p(px(8.0))
            .bg(solid(RECESS_TOP))
            .border_1()
            .border_color(solid(OUTLINE))
            .font(font("Ioskeley Mono"))
            .text_size(px(10.0))
            .line_height(px(14.0))
            .text_color(solid(TEXT_PRIMARY))
            .child(self.0)
    }
}

/// Retained editor state independent from GPUI's view tree.
#[derive(Clone)]
struct EditorController {
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    automation_config: AutomationConfig,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
    edit_sink: Option<Arc<dyn HostParamEditSink>>,
    target_text_param: f32,
    output_peak_db: f32,
    output_rms_db: f32,
    output_peak_hold: PeakHold,
    output_rms_hold: PeakHold,
    meter_last_update: Instant,
    pulse_started: Option<Instant>,
    pulse_alpha: u8,
    remember_mode_choices: bool,
}

impl EditorController {
    fn new(
        params: Arc<crate::params::GainSnapParams>,
        automation_queue: Arc<AutomationQueue>,
        status: Arc<GuiStatus>,
        param_requester: Option<HostParamRequester>,
        edit_sink: Option<Arc<dyn HostParamEditSink>>,
    ) -> Self {
        let target_db = params
            .get_param(PARAM_TARGET_DB)
            .unwrap_or(TARGET_RANGE.default)
            .clamp(TARGET_RANGE.min, TARGET_RANGE.max);
        let output_peak_db = sanitize_meter_level_db(status.output_peak_db());
        let output_rms_db = sanitize_meter_level_db(status.output_rms_db());
        let now = Instant::now();
        let (pending_peak, pending_rms) = status.take_meter_maxima();
        let output_peak_hold = PeakHold::new(output_peak_db.max(pending_peak), now);
        let output_rms_hold = PeakHold::new(output_rms_db.max(pending_rms), now);
        Self {
            params,
            automation_queue,
            automation_config: AutomationConfig::default(),
            remember_mode_choices: false,
            status,
            param_requester,
            edit_sink,
            target_text_param: target_db,
            output_peak_db,
            output_rms_db,
            output_peak_hold,
            output_rms_hold,
            meter_last_update: now,
            pulse_started: None,
            pulse_alpha: 255,
        }
    }

    fn target_db(&self) -> f32 {
        self.parameter_value(PARAM_TARGET_DB, TARGET_RANGE)
    }

    fn manual_gain_db(&self) -> f32 {
        self.parameter_value(
            PARAM_LOCKED_GAIN_DB,
            ParamRange {
                min: GAIN_MIN_DB,
                max: GAIN_MAX_DB,
                default: 0.0,
            },
        )
    }

    fn set_manual_gain_db(&self, gain_db: f32) {
        self.begin(PARAM_LOCKED_GAIN_DB);
        self.value(
            PARAM_LOCKED_GAIN_DB,
            gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB),
        );
        self.end(PARAM_LOCKED_GAIN_DB);
    }

    fn activate_manual(&self) {
        if self.params.match_requested() {
            self.toggle_value(PARAM_MATCH, false);
        }
    }

    fn target_text(&self) -> String {
        format_target_text(self.target_text_param)
    }

    fn parameter_value(&self, id: ClapId, range: ParamRange) -> f32 {
        self.params
            .get_param(id)
            .unwrap_or(range.default)
            .clamp(range.min, range.max)
    }

    fn request_flush(&self) {
        if let Some(requester) = self.param_requester {
            requester.request_flush();
        }
    }

    fn begin(&self, id: ClapId) {
        if let Some(sink) = self.edit_sink.as_ref() {
            sink.gesture_started(&self.automation_config, id);
        } else {
            self.automation_queue
                .push_gesture_begin(&self.automation_config, id);
            self.request_flush();
        }
    }

    fn value(&self, id: ClapId, value: f32) {
        self.params.set_param(id, value);
        let value = self.params.get_param(id).unwrap_or(value);
        if let Some(sink) = self.edit_sink.as_ref() {
            sink.gesture_value(&self.automation_config, id, value as f64);
        } else {
            self.automation_queue
                .push_value(&self.automation_config, id, value as f64);
            self.request_flush();
        }
    }

    fn end(&self, id: ClapId) {
        if let Some(sink) = self.edit_sink.as_ref() {
            sink.gesture_ended(&self.automation_config, id);
        } else {
            self.automation_queue
                .push_gesture_end(&self.automation_config, id);
            self.request_flush();
        }
    }

    fn toggle_value(&self, id: ClapId, checked: bool) {
        self.begin(id);
        self.value(id, f32::from(checked));
        self.end(id);
    }

    fn restart_matching(&self) {
        if self.params.match_requested() {
            self.params.request_restart();
        }
    }

    fn sync_target_text(&mut self, target_db: f32) -> Option<String> {
        if (target_db - self.target_text_param).abs() > TARGET_TEXT_SYNC_EPSILON {
            self.target_text_param = target_db;
            Some(format_target_text(target_db))
        } else {
            None
        }
    }

    fn set_target_db(&mut self, value: f32) {
        self.begin(PARAM_TARGET_DB);
        self.value(PARAM_TARGET_DB, value);
        self.end(PARAM_TARGET_DB);
        self.target_text_param = self.parameter_value(PARAM_TARGET_DB, TARGET_RANGE);
    }

    fn set_target_from_position(
        &mut self,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        shift_held: bool,
    ) {
        let Some(geometry) = target_marker_geometry(bounds) else {
            return;
        };
        let fraction = if geometry.travel <= 0.0 {
            0.0
        } else {
            clamp_fraction((geometry.bottom_center_y - f32::from(position.y)) / geometry.travel)
        };
        let target_db = FULL_SCALE_RANGE.denormalize(fraction);
        self.set_target_db(snap_drag_db(
            target_db,
            shift_held,
            TARGET_RANGE.min,
            TARGET_RANGE.max,
        ));
    }

    fn target_text_changed(&mut self, text: &str, submitted: bool) -> Option<String> {
        if let Some(value) = parse_target_text(text) {
            self.set_target_db(value);
            if submitted {
                return Some(format_target_text(self.target_text_param));
            }
        } else if submitted {
            return Some(format_target_text(self.target_text_param));
        }
        None
    }

    fn set_visible(&mut self, visible: bool) {
        if !visible && self.params.match_requested() {
            self.toggle_value(PARAM_MATCH, false);
        }
    }

    fn advance_meter_at(&mut self, now: Instant) -> bool {
        let peak_db = sanitize_meter_level_db(self.status.output_peak_db());
        let rms_db = sanitize_meter_level_db(self.status.output_rms_db());
        let (pending_peak, pending_rms) = self.status.take_meter_maxima();
        let observed_peak = peak_db.max(sanitize_meter_level_db(pending_peak));
        let observed_rms = rms_db.max(sanitize_meter_level_db(pending_rms));
        let elapsed = now.saturating_duration_since(self.meter_last_update);
        self.meter_last_update = now;
        let next_peak = smooth_meter_level_db(self.output_peak_db, peak_db, elapsed);
        let next_rms = smooth_meter_level_db(self.output_rms_db, rms_db, elapsed);
        let peak_hold_changed = self
            .output_peak_hold
            .advance_at(observed_peak, next_peak, now);
        let rms_hold_changed = self.output_rms_hold.advance_at(observed_rms, next_rms, now);
        let changed = (next_peak - self.output_peak_db).abs() > f32::EPSILON
            || (next_rms - self.output_rms_db).abs() > f32::EPSILON
            || peak_hold_changed
            || rms_hold_changed;
        self.output_peak_db = next_peak;
        self.output_rms_db = next_rms;
        changed
    }

    fn advance_pulse_at(&mut self, now: Instant) -> bool {
        let activity = self.status.activity();
        let should_pulse = self.params.match_requested()
            && matches!(
                activity,
                MatchActivity::Listening | MatchActivity::Adjusting
            );
        let next = if should_pulse {
            let start = *self.pulse_started.get_or_insert(now);
            let phase = now.saturating_duration_since(start).as_secs_f32() / MATCH_PULSE_SECONDS
                * std::f32::consts::TAU;
            (255.0 * (0.825 + 0.175 * phase.cos())).round() as u8
        } else {
            self.pulse_started = None;
            255
        };
        let changed = next != self.pulse_alpha;
        self.pulse_alpha = next;
        changed
    }

    fn meter_needs_realtime_redraw(&self) -> bool {
        self.status.has_pending_meter_maxima()
            || (self.output_peak_db - sanitize_meter_level_db(self.status.output_peak_db())).abs()
                > METER_SETTLE_EPSILON_DB
            || (self.output_rms_db - sanitize_meter_level_db(self.status.output_rms_db())).abs()
                > METER_SETTLE_EPSILON_DB
            || self.output_peak_hold.level_db
                > sanitize_meter_level_db(self.status.output_peak_db()) + METER_SETTLE_EPSILON_DB
            || self.output_rms_hold.level_db
                > sanitize_meter_level_db(self.status.output_rms_db()) + METER_SETTLE_EPSILON_DB
    }
}

pub(crate) struct GainSnapEditor {
    controller: EditorController,
    target_input: Entity<NumericInput>,
    gain_input: Entity<NumericInput>,
    target_entry_pointer_start: Rc<RefCell<Option<Point<Pixels>>>>,
    gain_entry_pointer_start: Rc<RefCell<Option<Point<Pixels>>>>,
    #[allow(dead_code)]
    target_text_subscription: Subscription,
    #[allow(dead_code)]
    target_submit_subscription: Subscription,
    #[allow(dead_code)]
    target_cancel_subscription: Subscription,
    #[allow(dead_code)]
    target_step_subscription: Subscription,
    #[allow(dead_code)]
    gain_text_subscription: Subscription,
    #[allow(dead_code)]
    gain_submit_subscription: Subscription,
    #[allow(dead_code)]
    gain_cancel_subscription: Subscription,
    #[allow(dead_code)]
    gain_step_subscription: Subscription,
    meter_focus_handle: FocusHandle,
    knob_focus_handle: FocusHandle,
    peak_focus_handle: FocusHandle,
    rms_focus_handle: FocusHandle,
    match_focus_handle: FocusHandle,
    restart_focus_handle: FocusHandle,
    meter_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    target_dragging: bool,
    gain_dragging: bool,
    gain_marker_selected: bool,
    knob_dragging: bool,
    knob_drag_start_y: f32,
    knob_drag_start_gain_db: f32,
    target_drag_offset_y: f32,
    numeric_drag: Option<(bool, Point<Pixels>, f32, bool)>,
    panel: Option<InfoPanel>,
    help_focus: FocusHandle,
    normalize_focus: FocusHandle,
    close_focus: FocusHandle,
    last_display_snapshot: DisplaySnapshot,
}

impl GainSnapEditor {
    fn new(controller: EditorController, cx: &mut Context<Self>) -> Self {
        let target_style = NumericInputStyle {
            font: font("Ioskeley Mono"),
            text_size: 13.0,
            line_height: 14.0,
            text_color: solid(TEXT_PRIMARY).into(),
            selection_color: rgba(0xe9584380),
            cursor_color: solid(TEXT_PRIMARY),
            text_align: gpui::TextAlign::Center,
            horizontal_padding: 0.0,
        };
        let target_config = NumericInputConfig::new(
            controller.target_db(),
            NumericInputRange::new(TARGET_RANGE.min, TARGET_RANGE.max),
            TARGET_KEYBOARD_STEP_DB,
            TARGET_FINE_KEYBOARD_STEP_DB,
            format_target_text,
        )
        .with_style(target_style.clone());
        let target_input = cx.new(move |cx| NumericInput::new(target_config, cx));
        let target_text_subscription =
            cx.subscribe(&target_input, |view, _, event: &NumericInputChanged, cx| {
                view.apply_target_text(event.text.clone(), false, cx);
            });
        let target_submit_subscription = cx.subscribe(
            &target_input,
            |view, _, event: &NumericInputSubmitted, cx| {
                view.apply_target_text(event.text.clone(), true, cx);
            },
        );
        let target_cancel_subscription = cx.subscribe(
            &target_input,
            |view, input, _: &NumericInputCanceled, cx| {
                let text = view.controller.target_text();
                input.update(cx, |input, cx| input.set_text(text, cx));
                cx.notify();
            },
        );
        let target_step_subscription =
            cx.subscribe(&target_input, |view, _, event: &NumericInputStepped, cx| {
                let value = (view.controller.target_db() + event.delta)
                    .clamp(TARGET_RANGE.min, TARGET_RANGE.max);
                view.controller.set_target_db(value);
                view.set_target_text_force(view.controller.target_text(), cx);
                cx.notify();
            });
        let gain_config = NumericInputConfig::new(
            controller.manual_gain_db(),
            NumericInputRange::new(GAIN_MIN_DB, GAIN_MAX_DB),
            1.0,
            0.1,
            format_gain_text,
        )
        .with_style(target_style);
        let gain_input = cx.new(move |cx| NumericInput::new(gain_config, cx));
        let gain_text_subscription =
            cx.subscribe(&gain_input, |view, _, event: &NumericInputChanged, cx| {
                view.apply_gain_text(&event.text, false, cx);
            });
        let gain_submit_subscription =
            cx.subscribe(&gain_input, |view, _, event: &NumericInputSubmitted, cx| {
                view.apply_gain_text(&event.text, true, cx);
            });
        let gain_cancel_subscription =
            cx.subscribe(&gain_input, |view, input, _: &NumericInputCanceled, cx| {
                input.update(cx, |input, cx| {
                    input.set_text(format_gain_text(view.controller.manual_gain_db()), cx)
                });
                cx.notify();
            });
        let gain_step_subscription =
            cx.subscribe(&gain_input, |view, _, event: &NumericInputStepped, cx| {
                view.controller.activate_manual();
                view.controller
                    .set_manual_gain_db(view.controller.manual_gain_db() + event.delta);
                let gain_db = view.controller.manual_gain_db();
                view.gain_input.update(cx, |input, cx| {
                    input.set_value(gain_db, cx);
                    input.set_text(format_gain_text(gain_db), cx);
                });
                cx.notify();
            });
        let last_display_snapshot =
            DisplaySnapshot::capture(&controller.params, &controller.status);
        Self {
            controller,
            target_input,
            gain_input,
            target_entry_pointer_start: Rc::new(RefCell::new(None)),
            gain_entry_pointer_start: Rc::new(RefCell::new(None)),
            target_text_subscription,
            target_submit_subscription,
            target_cancel_subscription,
            target_step_subscription,
            gain_text_subscription,
            gain_submit_subscription,
            gain_cancel_subscription,
            gain_step_subscription,
            meter_focus_handle: cx.focus_handle(),
            knob_focus_handle: cx.focus_handle(),
            peak_focus_handle: cx.focus_handle(),
            rms_focus_handle: cx.focus_handle(),
            match_focus_handle: cx.focus_handle(),
            restart_focus_handle: cx.focus_handle(),
            meter_bounds: Rc::new(RefCell::new(None)),
            target_dragging: false,
            gain_dragging: false,
            gain_marker_selected: false,
            knob_dragging: false,
            knob_drag_start_y: 0.0,
            knob_drag_start_gain_db: 0.0,
            target_drag_offset_y: 0.0,
            numeric_drag: None,
            panel: None,
            help_focus: cx.focus_handle(),
            normalize_focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            last_display_snapshot,
        }
    }

    fn apply_target_text(&mut self, text: String, submitted: bool, cx: &mut Context<Self>) {
        let formatted = self.controller.target_text_changed(&text, submitted);
        let value = self.controller.target_db();
        self.target_input
            .update(cx, |input, cx| input.set_value(value, cx));
        if let Some(formatted) = formatted {
            self.set_target_text_force(formatted, cx);
        }
        cx.notify();
    }

    fn apply_gain_text(&mut self, text: &str, submitted: bool, cx: &mut Context<Self>) {
        if let Some(gain_db) = parse_gain_text(text) {
            self.controller.activate_manual();
            self.controller.set_manual_gain_db(gain_db);
        }
        if submitted {
            let gain_db = self.controller.manual_gain_db();
            self.gain_input.update(cx, |input, cx| {
                input.set_value(gain_db, cx);
                input.set_text(format_gain_text(gain_db), cx);
            });
        }
        cx.notify();
    }

    fn set_target_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.target_input.update(cx, |input, cx| {
            input.set_value(self.controller.target_db(), cx);
            if !input.is_editing() {
                input.set_text(text, cx);
            }
        });
    }

    fn set_target_text_force(&mut self, text: String, cx: &mut Context<Self>) {
        self.target_input.update(cx, |input, cx| {
            input.set_value(self.controller.target_db(), cx);
            input.set_text(text, cx);
        });
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panel.is_some() {
            if event.keystroke.key == "escape" {
                self.close_panel(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let button_focused = self.help_focus.is_focused(window)
            || self.normalize_focus.is_focused(window)
            || self.peak_focus_handle.is_focused(window)
            || self.rms_focus_handle.is_focused(window)
            || self.match_focus_handle.is_focused(window)
            || self.restart_focus_handle.is_focused(window);
        if button_focused {
            // Enter activates the focused button; Space belongs to the host.
            if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                cx.stop_propagation();
            }
            return;
        }
        let meter_focused = self.meter_focus_handle.is_focused(window);
        let knob_focused = self.knob_focus_handle.is_focused(window);
        if !meter_focused && !knob_focused {
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
        {
            return;
        }
        // Native AppKit arrow events can carry the Function modifier as
        // keyboard metadata. It does not change the meaning of a semantic
        // arrow key, so keep it available for the selected meter marker. This
        // mirrors the numeric field's arrow-step handling.
        let direction = match event.keystroke.key.as_str() {
            "up" | "right" => Some(TargetStepDirection::Up),
            "down" | "left" => Some(TargetStepDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            if knob_focused || self.gain_marker_selected {
                self.controller.activate_manual();
                let step = if knob_focused {
                    if event.keystroke.modifiers.shift {
                        0.1
                    } else {
                        1.0
                    }
                } else if event.keystroke.modifiers.shift {
                    0.01
                } else {
                    0.1
                };
                self.controller
                    .set_manual_gain_db(self.controller.manual_gain_db() + direction.sign() * step);
            } else {
                let next = step_target_db(
                    self.controller.target_db(),
                    direction,
                    event.keystroke.modifiers.shift,
                );
                self.controller.set_target_db(next);
                self.set_target_text_force(self.controller.target_text(), cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Keep function-modified non-arrow keys available to the host. Native
        // AppKit arrow metadata is the only Function case this control owns.
        if event.keystroke.modifiers.function {
            return;
        }
        if meter_focused && !self.gain_marker_selected {
            let text = match event.keystroke.key.as_str() {
                "home" => {
                    self.controller.set_target_db(TARGET_RANGE.min);
                    Some(self.controller.target_text())
                }
                "end" => {
                    self.controller.set_target_db(TARGET_RANGE.max);
                    Some(self.controller.target_text())
                }
                _ => None,
            };
            if let Some(text) = text {
                self.set_target_text_force(text, cx);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    fn meter_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.meter_focus_handle, cx);
        let bounds = self.meter_bounds.borrow().as_ref().copied();
        if let Some(bounds) = bounds {
            let track = target_meter_track(bounds);
            self.gain_marker_selected = f32::from(event.position.x) >= f32::from(track.right());
            if event.click_count >= 2 {
                self.target_dragging = false;
                self.gain_dragging = false;
                if self.gain_marker_selected {
                    self.reset_gain(cx);
                } else {
                    self.reset_target(cx);
                }
                cx.notify();
                return;
            }
            if self.gain_marker_selected {
                self.controller.activate_manual();
                self.gain_dragging = true;
                let gain_drag_start_level_db =
                    gain_marker_level_db(self.controller.manual_gain_db());
                self.target_drag_offset_y =
                    target_marker_center_y(bounds, gain_drag_start_level_db)
                        .map(|center_y| f32::from(event.position.y) - center_y)
                        .unwrap_or(0.0);
            } else {
                self.target_dragging = true;
                self.target_drag_offset_y =
                    target_marker_center_y(bounds, self.controller.target_db())
                        .map(|center_y| f32::from(event.position.y) - center_y)
                        .unwrap_or(0.0);
            }
        }
        cx.notify();
    }

    fn meter_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if (!self.target_dragging && !self.gain_dragging)
            || event.pressed_button != Some(MouseButton::Left)
        {
            return;
        }
        let bounds = self.meter_bounds.borrow().as_ref().copied();
        if let Some(bounds) = bounds {
            let position = point(
                event.position.x,
                event.position.y - px(self.target_drag_offset_y),
            );
            if self.gain_dragging {
                let geometry = target_marker_geometry(bounds);
                if let Some(geometry) = geometry {
                    let fraction = clamp_fraction(
                        (geometry.bottom_center_y - f32::from(position.y))
                            / geometry.travel.max(1.0),
                    );
                    let level_db = FULL_SCALE_RANGE.denormalize(fraction);
                    let gain_db = gain_from_marker_level_db(level_db);
                    self.controller.set_manual_gain_db(snap_drag_db(
                        gain_db,
                        event.modifiers.shift,
                        GAIN_MIN_DB,
                        GAIN_MAX_DB,
                    ));
                }
            } else {
                self.controller
                    .set_target_from_position(bounds, position, event.modifiers.shift);
                self.set_target_text(self.controller.target_text(), cx);
            }
            cx.notify();
        }
    }

    fn meter_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.target_dragging = false;
        self.gain_dragging = false;
        self.target_drag_offset_y = 0.0;
    }

    fn knob_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.controller.activate_manual();
        if event.click_count >= 2 {
            self.knob_dragging = false;
            self.reset_gain(cx);
            window.focus(&self.knob_focus_handle, cx);
            cx.notify();
            return;
        }
        window.focus(&self.knob_focus_handle, cx);
        self.knob_dragging = true;
        self.knob_drag_start_y = f32::from(event.position.y);
        self.knob_drag_start_gain_db = self.controller.manual_gain_db();
        cx.notify();
    }

    fn knob_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.knob_dragging && event.pressed_button == Some(MouseButton::Left) {
            let gain_db = self.knob_drag_start_gain_db
                + (self.knob_drag_start_y - f32::from(event.position.y)) * 0.5;
            self.controller.set_manual_gain_db(snap_drag_db(
                gain_db,
                event.modifiers.shift,
                GAIN_MIN_DB,
                GAIN_MAX_DB,
            ));
            cx.notify();
        }
    }

    fn knob_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.knob_dragging = false;
    }

    fn target_entry_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.numeric_drag = None;
            self.reset_target(cx);
            cx.stop_propagation();
            return;
        }
        self.numeric_drag = Some((false, event.position, self.controller.target_db(), false));
    }

    fn gain_entry_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count >= 2 {
            self.numeric_drag = None;
            self.reset_gain(cx);
            cx.stop_propagation();
            return;
        }
        self.numeric_drag = Some((
            true,
            event.position,
            self.controller.manual_gain_db(),
            false,
        ));
    }

    fn reset_target(&mut self, cx: &mut Context<Self>) {
        self.controller.set_target_db(TARGET_RANGE.default);
        self.target_input
            .update(cx, |input, cx| input.set_editing(false, cx));
        self.set_target_text_force(self.controller.target_text(), cx);
        cx.notify();
    }

    fn reset_gain(&mut self, cx: &mut Context<Self>) {
        self.controller.activate_manual();
        self.controller.set_manual_gain_db(0.0);
        self.gain_input.update(cx, |input, cx| {
            input.set_editing(false, cx);
            input.set_value(0.0, cx);
            input.set_text(format_gain_text(0.0), cx);
        });
        cx.notify();
    }

    fn open_panel(&mut self, panel: InfoPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Some(panel);
        self.target_dragging = false;
        self.gain_dragging = false;
        self.knob_dragging = false;
        self.numeric_drag = None;
        window.focus(&self.close_focus, cx);
        cx.notify();
    }

    fn close_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        window.focus(&self.help_focus, cx);
        cx.notify();
    }

    fn normalize(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.controller.params.match_requested() {
            self.controller.toggle_value(PARAM_MATCH, false);
        } else {
            self.controller.toggle_value(PARAM_RMS_MODE, false);
            self.remember_mode_choice(false);
            self.controller.set_target_db(0.0);
            self.target_input
                .update(cx, |input, cx| input.set_editing(false, cx));
            self.set_target_text_force(self.controller.target_text(), cx);
            self.controller.toggle_value(PARAM_MATCH, true);
        }
        cx.notify();
    }

    fn render_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let mut body = div()
            .absolute()
            .left(px(12.0))
            .top(px(54.0))
            .w(px(176.0))
            .flex()
            .flex_col()
            .gap(px(10.0));
        for (heading, text) in [
                ("Peak / RMS", "Peak: transients. RMS: strongest 300 ms window, not LUFS."),
                ("Match / Normalize", "Normalize toggles Peak Match at 0 dBFS. Restart measures again."),
                ("Target / Gain", "Left: target dBFS. Right: gain dB, on its own scale."),
                ("RMS shortfall", "Shortfall means the gain range prevents reaching the RMS target."),
                ("Shortcuts", "Drag or type numbers. Shift: finer steps. Double-click: gain 0, target −12. Enter: confirm. Space: DAW transport."),
            ] {
                body = body.child(div().flex().flex_col().gap(px(2.0))
                    .child(div().text_size(px(11.0)).text_color(solid(TEXT_PRIMARY)).child(heading))
                    .child(div().text_size(px(10.0)).line_height(px(13.0)).text_color(solid(TEXT_MUTED)).child(text)));
            }

        div()
            .id("info-panel")
            .relative()
            .w(px(WINDOW_WIDTH as f32))
            .h(px(WINDOW_HEIGHT as f32))
            .bg(solid(BG_PRIMARY))
            .font(font("Ioskeley Mono"))
            .text_color(solid(TEXT_PRIMARY))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .top(px(20.0))
                    .text_size(px(13.0))
                    .child("HELP"),
            )
            .child(
                utility_button(
                    "close-panel",
                    "Close",
                    &self.close_focus,
                    window,
                    cx.listener(|view, _, window, cx| view.close_panel(window, cx)),
                )
                .absolute()
                .left(px(142.0))
                .top(px(16.0))
                .w(px(46.0))
                .h(px(24.0)),
            )
            .child(body)
    }

    fn numeric_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((gain, start, value, dragging)) = self.numeric_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let delta = event.position - start;
        let dy = f32::from(delta.y);
        if !dragging && (dy.abs() <= 3.0 || dy.abs() <= f32::from(delta.x).abs()) {
            return;
        }
        self.numeric_drag = Some((gain, start, value, true));
        let next = value - dy * if event.modifiers.shift { 0.01 } else { 0.1 };
        if gain {
            self.controller.activate_manual();
            self.controller.set_manual_gain_db(next);
            let next = self.controller.manual_gain_db();
            self.gain_input.update(cx, |input, cx| {
                input.set_editing(false, cx);
                input.set_value(next, cx);
                input.set_text(format_gain_text(next), cx);
            });
        } else {
            self.controller.set_target_db(next);
            self.target_input.update(cx, |input, cx| {
                input.set_editing(false, cx);
            });
            self.set_target_text_force(self.controller.target_text(), cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn numeric_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.numeric_drag = None;
    }

    fn remember_mode_choice(&self, rms: bool) {
        // Standalone screenshot fixtures have no host endpoints and must not
        // alter the user's preferences while exercising their controls.
        if self.controller.remember_mode_choices {
            crate::preferences::remember_rms_mode(rms);
        }
    }

    fn select_peak_mode(
        &mut self,
        event: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_space_click(event) {
            return;
        }
        self.controller.toggle_value(PARAM_RMS_MODE, false);
        self.remember_mode_choice(false);
        cx.notify();
    }

    fn select_rms_mode(
        &mut self,
        event: &gpui::ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_space_click(event) {
            return;
        }
        self.controller.toggle_value(PARAM_RMS_MODE, true);
        self.remember_mode_choice(true);
        cx.notify();
    }

    fn toggle_match(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.controller
            .toggle_value(PARAM_MATCH, !self.controller.params.match_requested());
        cx.notify();
    }

    fn restart_matching(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.controller.restart_matching();
        cx.notify();
    }

    #[allow(dead_code)]
    fn state_changed(&self) -> bool {
        DisplaySnapshot::capture(&self.controller.params, &self.controller.status)
            != self.last_display_snapshot
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let meter_changed = self.controller.advance_meter_at(now);
        let pulse_changed = self.controller.advance_pulse_at(now);
        let snapshot = DisplaySnapshot::capture(&self.controller.params, &self.controller.status);
        if snapshot != self.last_display_snapshot {
            self.last_display_snapshot = snapshot;
            let gain_db = self.controller.manual_gain_db();
            self.gain_input
                .update(cx, |input, cx| input.set_value(gain_db, cx));
            if let Some(text) = self
                .controller
                .sync_target_text(self.controller.target_db())
            {
                self.set_target_text(text, cx);
            }
            cx.notify();
        }
        if meter_changed || pulse_changed {
            cx.notify();
        }
        // The host can begin processing after the first editor paint. Keep
        // sampling telemetry while the editor is open, even after silence.
        window.request_animation_frame();
    }
}

fn utility_button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    focus: &FocusHandle,
    window: &Window,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let on_down = focus.clone();
    let text = text.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .track_focus(focus)
        .bg(vertical_gradient(BUTTON_IDLE_TOP, BUTTON_IDLE_BOTTOM))
        .border_1()
        .border_color(solid(if focus.is_focused(window) {
            0xc8e4d1
        } else {
            BORDER_EMPHASIS
        }))
        .when(focus.is_focused(window), |element| element.border_2())
        .rounded(px(3.0))
        .font(font("Ioskeley Mono"))
        .text_size(px(10.0))
        .line_height(px(13.0))
        .text_color(solid(TEXT_PRIMARY))
        .role(gpui::Role::Button)
        .aria_label(if text.as_ref() == "?" {
            gpui::SharedString::from("Help and shortcuts")
        } else {
            text.clone()
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&on_down, cx)
        })
        .on_click(move |event, window, cx| {
            if !is_space_click(event) {
                listener(event, window, cx);
            }
        })
        .child(text)
}

fn hardware_button_element(
    id: &'static str,
    active: bool,
    enabled: bool,
    bounds: (f32, f32, f32, f32),
    active_alpha: u8,
    focus_handle: &FocusHandle,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let focus_on_mouse = focus_handle.clone();
    let focus_for_paint = focus_handle.clone();
    let (left, top, width, height) = bounds;
    div()
        .id(id)
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .text_size(px(9.0))
        .line_height(px(10.0))
        .text_color(solid(TEXT_PRIMARY))
        .font(font("Ioskeley Mono"))
        .role(gpui::Role::Button)
        .aria_label(if id == "match-now" {
            MATCH_AUTOMATION_DESCRIPTION
        } else {
            RESTART_AUTOMATION_DESCRIPTION
        })
        .track_focus(focus_handle)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if enabled {
                window.focus(&focus_on_mouse, cx);
            }
        })
        .on_click(move |event, window, cx| {
            if is_space_click(event) {
                return;
            }
            if enabled {
                listener(event, window, cx);
            }
        })
        .when(!enabled, |element| element.opacity(0.45))
        .child(
            canvas(
                move |_, _, _| {},
                move |paint_bounds, _, window, cx| {
                    paint_hardware_button(
                        paint_bounds,
                        id == "match-now",
                        active,
                        focus_for_paint.is_focused(window),
                        active_alpha,
                        window,
                        cx,
                    );
                },
            )
            .size_full(),
        )
}

fn paint_device_frame(bounds: Bounds<Pixels>, window: &mut Window) {
    let x = f32::from(bounds.left());
    let y = f32::from(bounds.top());
    let shell_points = [
        (x + 1.0, y + 1.0),
        (x + 199.0, y + 1.0),
        (x + 199.0, y + 423.0),
        (x + 1.0, y + 423.0),
    ];
    window.paint_quad(fill(
        Bounds::from_corners(
            point(px(x + 100.0), px(y + 100.0)),
            point(px(x + 200.0), px(y + 330.0)),
        ),
        vertical_gradient(RECESS_TOP, RECESS_BOTTOM),
    ));
    if let Some(body) = rounded_polygon_path(&shell_points, 9.0, 0.0) {
        window.paint_path(body, vertical_gradient(BODY_TOP, BG_PRIMARY));
    }
    if let Some(outline) = rounded_polygon_path(&shell_points, 9.0, 2.0) {
        window.paint_path(outline, solid(OUTLINE));
    }
    let inner_rim = [
        (x + 4.0, y + 4.0),
        (x + 196.0, y + 4.0),
        (x + 196.0, y + 420.0),
        (x + 4.0, y + 420.0),
    ];
    if let Some(rim) = rounded_polygon_path(&inner_rim, 7.0, 1.0) {
        window.paint_path(rim, solid(0x343d36));
    }
    let display = [
        (x + 10.0, y + 44.0),
        (x + 190.0, y + 44.0),
        (x + 190.0, y + 366.0),
        (x + 10.0, y + 366.0),
    ];
    if let Some(panel) = rounded_polygon_path(&display, 3.0, 0.0) {
        window.paint_path(panel, vertical_gradient(RECESS_TOP, RECESS_BOTTOM));
    }
    if let Some(panel_outline) = rounded_polygon_path(&display, 3.0, 1.0) {
        window.paint_path(panel_outline, solid(BORDER_EMPHASIS));
    }

    // The metal shoulder plates are intentionally painted through the window
    // edges so the inserts meet the plugin's logical bounds without a seam.
    let top_insert = [
        (x + 51.0, y),
        (x + 149.0, y),
        (x + 149.0, y + 4.0),
        (x + 140.0, y + 10.0),
        (x + 135.0, y + 12.0),
        (x + 65.0, y + 12.0),
        (x + 60.0, y + 10.0),
        (x + 51.0, y + 4.0),
    ];
    if let Some(insert) = rounded_polygon_path(&top_insert, 1.0, 0.0) {
        window.paint_path(insert, vertical_gradient(0x414b42, 0x303831));
    }
    if let Some(edge) = rounded_polygon_path(&top_insert, 1.0, 1.0) {
        window.paint_path(edge, solid(0x424d44));
    }
    let bottom_insert = [
        (x + 51.0, y + 424.0),
        (x + 149.0, y + 424.0),
        (x + 149.0, y + 422.0),
        (x + 140.0, y + 418.0),
        (x + 135.0, y + 416.0),
        (x + 65.0, y + 416.0),
        (x + 60.0, y + 418.0),
        (x + 51.0, y + 422.0),
    ];
    if let Some(insert) = rounded_polygon_path(&bottom_insert, 1.0, 0.0) {
        window.paint_path(insert, vertical_gradient(0x3b453c, 0x2c342d));
    }
    if let Some(edge) = rounded_polygon_path(&bottom_insert, 1.0, 1.0) {
        window.paint_path(edge, solid(0x424d44));
    }
}

fn rounded_polygon_path(
    points: &[(f32, f32)],
    radius: f32,
    stroke: f32,
) -> Option<gpui::Path<Pixels>> {
    rounded_polygon_path_with_radii(points, &vec![radius; points.len()], stroke)
}

fn rounded_polygon_path_with_radii(
    points: &[(f32, f32)],
    radii: &[f32],
    stroke: f32,
) -> Option<gpui::Path<Pixels>> {
    if points.len() < 3 {
        return None;
    }
    let mut path = if stroke > 0.0 {
        gpui::PathBuilder::stroke(px(stroke))
    } else {
        gpui::PathBuilder::fill()
    };
    for index in 0..points.len() {
        let previous = points[(index + points.len() - 1) % points.len()];
        let corner = points[index];
        let next = points[(index + 1) % points.len()];
        let previous_length =
            ((previous.0 - corner.0).powi(2) + (previous.1 - corner.1).powi(2)).sqrt();
        let next_length = ((next.0 - corner.0).powi(2) + (next.1 - corner.1).powi(2)).sqrt();
        if previous_length <= f32::EPSILON || next_length <= f32::EPSILON {
            continue;
        }
        let radius = radii.get(index).copied().unwrap_or(0.0);
        let to_previous = (previous.0 - corner.0, previous.1 - corner.1);
        let to_next = (next.0 - corner.0, next.1 - corner.1);
        let cosine = ((to_previous.0 * to_next.0 + to_previous.1 * to_next.1)
            / (previous_length * next_length))
            .clamp(-1.0, 1.0);
        let corner_angle = cosine.acos();
        let turn = (corner.0 - previous.0) * (next.1 - corner.1)
            - (corner.1 - previous.1) * (next.0 - corner.0);
        let interior_angle = if turn < 0.0 {
            std::f32::consts::TAU - corner_angle
        } else {
            corner_angle
        };
        let tangent = (interior_angle * 0.5).tan().abs().max(f32::EPSILON);
        let inset = (radius / tangent)
            .min(previous_length * 0.45)
            .min(next_length * 0.45);
        let before = (
            corner.0 + (previous.0 - corner.0) * inset / previous_length,
            corner.1 + (previous.1 - corner.1) * inset / previous_length,
        );
        let after = (
            corner.0 + (next.0 - corner.0) * inset / next_length,
            corner.1 + (next.1 - corner.1) * inset / next_length,
        );
        if index == 0 {
            path.move_to(point(px(before.0), px(before.1)));
        } else {
            path.line_to(point(px(before.0), px(before.1)));
        }
        path.curve_to(
            point(px(after.0), px(after.1)),
            point(px(corner.0), px(corner.1)),
        );
    }
    path.close();
    path.build().ok()
}

fn paint_hardware_button(
    bounds: Bounds<Pixels>,
    match_button: bool,
    active: bool,
    focused: bool,
    active_alpha: u8,
    window: &mut Window,
    cx: &mut App,
) {
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let cut = 5.0;
    let points = vec![
        (left, top),
        (left + width, top),
        (left + width, top + height - cut),
        (left + width - cut, top + height),
        (left, top + height),
    ];
    let corner_radii = vec![2.0, 2.0, 0.0, 0.0, 2.0];
    let shadow_points: Vec<_> = points.iter().map(|(x, y)| (*x, *y + 1.5)).collect();
    if let Some(shadow) = rounded_polygon_path_with_radii(&shadow_points, &corner_radii, 0.0) {
        window.paint_path(shadow, rgba(0x090d0d64));
    }
    if let Some(path) = rounded_polygon_path_with_radii(&points, &corner_radii, 0.0) {
        window.paint_path(
            path,
            vertical_gradient_alpha(
                if active {
                    BUTTON_ACTIVE_TOP
                } else {
                    BUTTON_IDLE_TOP
                },
                if active {
                    BUTTON_ACTIVE_BOTTOM
                } else {
                    BUTTON_IDLE_BOTTOM
                },
                if active { active_alpha } else { 255 },
            ),
        );
    }
    if let Some(path) = rounded_polygon_path_with_radii(&points, &corner_radii, 1.5) {
        window.paint_path(
            path,
            solid(if focused {
                TEXT_PRIMARY
            } else if active {
                ACCENT
            } else {
                BORDER_EMPHASIS
            }),
        );
    }
    if match_button {
        paint_label_text(
            "MATCH",
            point(px(left + width * 0.5), px(top + height * 0.5)),
            9.0,
            if active { 0xffd3ac } else { TEXT_PRIMARY },
            window,
            cx,
        );
    } else {
        let mut arrow = gpui::PathBuilder::stroke(px(1.1));
        let center_x = left + width * 0.5;
        let center_y = top + height * 0.5;
        let radius = 5.5;
        for step in 0..=32 {
            let angle = -0.9 + (4.7 + 0.9) * step as f32 / 32.0;
            let position = point(
                px(center_x + radius * angle.cos()),
                px(center_y + radius * angle.sin()),
            );
            if step == 0 {
                arrow.move_to(position);
            } else {
                arrow.line_to(position);
            }
        }
        arrow.move_to(point(px(center_x - 1.0), px(center_y - radius - 2.5)));
        arrow.line_to(point(px(center_x + 2.0), px(center_y - radius - 0.25)));
        arrow.line_to(point(px(center_x - 1.0), px(center_y - radius + 2.0)));
        if let Ok(arrow) = arrow.build() {
            window.paint_path(arrow, solid(TEXT_PRIMARY));
        }
    }
}

fn paint_readout_card(
    bounds: Bounds<Pixels>,
    rms: bool,
    rms_selected: bool,
    focused: bool,
    window: &mut Window,
) {
    let x = f32::from(bounds.left());
    let y = f32::from(bounds.top());
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let points = [
        (x, y),
        (x + w, y),
        (x + w, y + h - 4.0),
        (x + w - 4.0, y + h),
        (x, y + h),
    ];
    let selected = rms == rms_selected;
    let (top, bottom) = if selected && rms {
        (0x332f3f, 0x282531)
    } else if selected {
        (0x3b302a, 0x29231f)
    } else {
        (0x222923, 0x1b201d)
    };
    if let Some(path) = rounded_polygon_path_with_radii(&points, &[2.0, 2.0, 0.0, 0.0, 2.0], 0.0) {
        window.paint_path(path, vertical_gradient(top, bottom));
    }
    let border = if focused {
        TEXT_PRIMARY
    } else if selected && rms {
        0x6d6188
    } else if selected {
        0x967258
    } else {
        BORDER_EMPHASIS
    };
    if let Some(path) = rounded_polygon_path_with_radii(&points, &[2.0, 2.0, 0.0, 0.0, 2.0], 1.0) {
        window.paint_path(path, solid(border));
    }
}

fn paint_label_text(
    value: &str,
    center: Point<Pixels>,
    size: f32,
    color: u32,
    window: &mut Window,
    cx: &mut App,
) {
    let text: gpui::SharedString = value.into();
    let run = TextRun {
        len: text.len(),
        font: font("Ioskeley Mono"),
        color: solid(color).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text, px(size), &[run], None);
    let _ = line.paint(
        point(center.x - line.width * 0.5, center.y - px(size * 0.5)),
        px(size),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

fn paint_circle(cx: f32, cy: f32, radius: f32, stroke: f32, color: u32, window: &mut Window) {
    let k = radius * 0.5522848;
    let mut path = gpui::PathBuilder::stroke(px(stroke));
    path.move_to(point(px(cx + radius), px(cy)));
    path.cubic_bezier_to(
        point(px(cx), px(cy + radius)),
        point(px(cx + radius), px(cy + k)),
        point(px(cx + k), px(cy + radius)),
    );
    path.cubic_bezier_to(
        point(px(cx - radius), px(cy)),
        point(px(cx - k), px(cy + radius)),
        point(px(cx - radius), px(cy + k)),
    );
    path.cubic_bezier_to(
        point(px(cx), px(cy - radius)),
        point(px(cx - radius), px(cy - k)),
        point(px(cx - k), px(cy - radius)),
    );
    path.cubic_bezier_to(
        point(px(cx + radius), px(cy)),
        point(px(cx + k), px(cy - radius)),
        point(px(cx + radius), px(cy - k)),
    );
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, solid(color));
    }
}

fn circle_path(cx: f32, cy: f32, radius: f32, stroke: f32) -> Option<gpui::Path<Pixels>> {
    let k = radius * 0.5522848;
    let mut path = if stroke > 0.0 {
        gpui::PathBuilder::stroke(px(stroke))
    } else {
        gpui::PathBuilder::fill()
    };
    path.move_to(point(px(cx + radius), px(cy)));
    path.cubic_bezier_to(
        point(px(cx), px(cy + radius)),
        point(px(cx + radius), px(cy + k)),
        point(px(cx + k), px(cy + radius)),
    );
    path.cubic_bezier_to(
        point(px(cx - radius), px(cy)),
        point(px(cx - k), px(cy + radius)),
        point(px(cx - radius), px(cy + k)),
    );
    path.cubic_bezier_to(
        point(px(cx), px(cy - radius)),
        point(px(cx - radius), px(cy - k)),
        point(px(cx - k), px(cy - radius)),
    );
    path.cubic_bezier_to(
        point(px(cx + radius), px(cy)),
        point(px(cx + k), px(cy - radius)),
        point(px(cx + radius), px(cy - k)),
    );
    path.close();
    path.build().ok()
}

fn paint_knob_control(bounds: Bounds<Pixels>, gain_db: f32, focused: bool, window: &mut Window) {
    let center_x = (f32::from(bounds.left()) + f32::from(bounds.right())) * 0.5;
    let center_y = (f32::from(bounds.top()) + f32::from(bounds.bottom())) * 0.5;
    if let Some(path) = circle_path(center_x, center_y, 21.5, 1.2) {
        window.paint_path(path, vertical_gradient(KNOB_OUTER_TOP, KNOB_OUTER_BOTTOM));
    }
    if let Some(path) = circle_path(center_x, center_y, 16.5, 2.5) {
        window.paint_path(path, vertical_gradient(KNOB_RING_TOP, KNOB_RING_BOTTOM));
    }
    if let Some(path) = circle_path(center_x, center_y, 11.5, 0.0) {
        window.paint_path(path, vertical_gradient(KNOB_CAP_TOP, KNOB_CAP_BOTTOM));
    }
    if focused {
        paint_circle(center_x, center_y, 21.5, 1.5, 0xc8e4d1, window);
    }
    paint_knob_marker(bounds, gain_db, window);
}

impl Render for GainSnapEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        #[cfg(target_os = "macos")]
        crate::host_space::install(window);
        self.tick(window, cx);
        if self.panel.is_some() {
            return self.render_panel(window, cx);
        }
        let meter_bounds = Rc::clone(&self.meter_bounds);
        let target_db = self.controller.target_db();
        let output_peak_db = self.controller.output_peak_db;
        let output_rms_db = self.controller.output_rms_db;
        let output_peak_hold = self.controller.output_peak_hold.level_db;
        let output_rms_hold = self.controller.output_rms_hold.level_db;
        let target_focused = self.target_input.read(cx).focus_handle().is_focused(window);
        let meter_focused = self.meter_focus_handle.is_focused(window);
        let knob_focused = self.knob_focus_handle.is_focused(window);
        let gain_marker_selected = self.gain_marker_selected;
        let mode_rms = self.controller.params.rms_mode();
        let match_requested = self.controller.params.match_requested();
        let manual_gain_db = self.controller.manual_gain_db();
        let pulse_alpha = self.controller.pulse_alpha;
        let match_activity = self.controller.status.activity();
        let meter_paint = MeterPaintState {
            target_db,
            output_peak_db,
            output_rms_db,
            output_peak_hold_db: output_peak_hold,
            output_rms_hold_db: output_rms_hold,
            gain_db: manual_gain_db,
            focused: meter_focused,
            gain_selected: gain_marker_selected,
        };

        let meter = div()
            .id("target-meter")
            .absolute()
            .left(px(METER_LEFT))
            .top(px(METER_TOP))
            .w(px(TARGET_CONTROL_WIDTH))
            .h(px(TARGET_METER_HEIGHT))
            .track_focus(&self.meter_focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::meter_mouse_down))
            .child(
                canvas(
                    move |bounds, _, _| {
                        *meter_bounds.borrow_mut() = Some(bounds);
                    },
                    move |bounds, _, window, cx| {
                        paint_meter(bounds, meter_paint, window, cx);
                    },
                )
                .size_full(),
            );

        let target_entry = div()
            .id("target-entry")
            .absolute()
            .left(px(TARGET_ENTRY_LEFT))
            .top(px(TARGET_ENTRY_TOP))
            .w(px(TARGET_ENTRY_WIDTH))
            .h(px(TARGET_ENTRY_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .bg(vertical_gradient(FIELD_TOP, FIELD_BOTTOM))
            .border_1()
            .border_color(if target_focused {
                solid(ACCENT)
            } else {
                solid(BORDER_EMPHASIS)
            })
            .rounded(px(3.0))
            .text_color(solid(TEXT_PRIMARY))
            .font(font("Ioskeley Mono"))
            // Radiant's compact 28px text input uses a 13px mono face. Keep
            // the same scale when the field is painted by GPUI.
            .text_size(px(12.0))
            .line_height(px(14.0))
            .tooltip(|_, cx| cx.new(|_| ControlHint("Target in dBFS. Click and type or drag vertically. Double-click resets to −12 dBFS.")).into())
            .when(target_focused, |element| element.border_2())
            .aria_label(TARGET_ENTRY_AUTOMATION_LABEL)
            .child(self.target_input.clone());
        let target_entry = selectable_numeric_entry(
            target_entry,
            self.target_entry_pointer_start.clone(),
            cx.listener(Self::target_entry_mouse_down),
        );

        let match_button = hardware_button_element(
            "match-now",
            match_requested,
            true,
            (MATCH_LEFT, MATCH_TOP, MATCH_WIDTH, MATCH_HEIGHT),
            pulse_alpha,
            &self.match_focus_handle,
            cx.listener(Self::toggle_match),
        );
        let restart = hardware_button_element(
            "restart-match",
            false,
            match_requested,
            (RESTART_LEFT, RESTART_TOP, RESTART_WIDTH, RESTART_HEIGHT),
            255,
            &self.restart_focus_handle,
            cx.listener(Self::restart_matching),
        )
        .aria_label(RESTART_AUTOMATION_DESCRIPTION);
        let knob = div()
            .id("manual-gain-knob")
            .absolute()
            .left(px(KNOB_LEFT))
            .top(px(KNOB_TOP))
            .w(px(KNOB_SIZE))
            .h(px(KNOB_SIZE))
            .rounded_full()
            .bg(rgba(0))
            .tooltip(|_, cx| {
                cx.new(|_| {
                    ControlHint("Gain in dB. Drag or use arrows. Double-click resets to 0 dB.")
                })
                .into()
            })
            .aria_label("Coarse manual gain, drag vertically or use arrow keys")
            .track_focus(&self.knob_focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::knob_mouse_down))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_knob_control(bounds, manual_gain_db, knob_focused, window);
                    },
                )
                .size_full(),
            );
        let gain_entry = div()
            .id("manual-gain-entry")
            .absolute()
            .left(px(GAIN_ENTRY_LEFT))
            .top(px(GAIN_ENTRY_TOP))
            .w(px(GAIN_ENTRY_WIDTH))
            .h(px(TARGET_ENTRY_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .bg(vertical_gradient(FIELD_TOP, FIELD_BOTTOM))
            .border_1()
            .border_color(
                if self.gain_input.read(cx).focus_handle().is_focused(window) {
                    solid(ACCENT)
                } else {
                    solid(BORDER_EMPHASIS)
                },
            )
            .rounded(px(3.0))
            .text_color(solid(TEXT_PRIMARY))
            .text_size(px(12.0))
            .line_height(px(14.0))
            .when(self.gain_input.read(cx).focus_handle().is_focused(window), |element| element.border_2())
            .tooltip(|_, cx| cx.new(|_| ControlHint("Gain in dB. Click and type or drag vertically. Double-click resets to 0 dB.")).into())
            .child(self.gain_input.clone());
        let gain_entry = selectable_numeric_entry(
            gain_entry,
            self.gain_entry_pointer_start.clone(),
            cx.listener(Self::gain_entry_mouse_down),
        );
        let active_stage = activity_stage(match_activity);
        let segment_width = 50.0;
        let mut activity_rail = div()
            .id("activity-rail")
            .absolute()
            .bottom(px(ACTIVITY_RAIL_BOTTOM_INSET))
            .left(px(18.0))
            .w(px(164.0))
            .h(px(ACTIVITY_RAIL_HEIGHT))
            .flex()
            .items_center()
            .gap(px(ACTIVITY_RAIL_GAP));
        for index in 0..ACTIVITY_SEGMENT_COUNT {
            let active = active_stage == Some(index);
            let color = if active {
                rgba(ACCENT << 8 | u32::from(pulse_alpha))
            } else {
                solid(BORDER_EMPHASIS)
            };
            activity_rail = activity_rail.child(
                div()
                    .w(px(segment_width))
                    .h(px(ACTIVITY_SEGMENT_HEIGHT))
                    .bg(color),
            );
        }
        let activity_label = div()
            .id("activity-label")
            .absolute()
            .left(px(INFO_LEFT))
            .top(px(138.0))
            .w(px(72.0))
            .h(px(ACTIVITY_LABEL_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .text_color(activity_label_color(match_activity))
            .font(font("Ioskeley Mono"))
            .text_size(px(8.0))
            .line_height(px(12.0))
            .child(activity_label_text(match_activity));
        let show_activity_label = matches!(
            match_activity,
            MatchActivity::Listening
                | MatchActivity::Adjusting
                | MatchActivity::Matched
                | MatchActivity::BelowTarget
                | MatchActivity::NoSignal
        );
        let peak_focused = self.peak_focus_handle.is_focused(window);
        let rms_focused = self.rms_focus_handle.is_focused(window);
        let peak_focus = self.peak_focus_handle.clone();
        let peak_readout = div()
            .id("peak-mode")
            .absolute()
            .left(px(INFO_LEFT))
            .top(px(66.0))
            .w(px(72.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_readout_card(bounds, false, mode_rms, peak_focused, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .role(gpui::Role::Button)
            .aria_label("Use Peak matching")
            .track_focus(&self.peak_focus_handle)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&peak_focus, cx)
            })
            .on_click(cx.listener(Self::select_peak_mode))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(solid(ACCENT))
                    .child(format!("PK {}", format_meter_readout(output_peak_db))),
            );
        let rms_focus = self.rms_focus_handle.clone();
        let rms_readout = div()
            .id("rms-mode")
            .absolute()
            .left(px(INFO_LEFT))
            .top(px(102.0))
            .w(px(72.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        paint_readout_card(bounds, true, mode_rms, rms_focused, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .role(gpui::Role::Button)
            .aria_label("Use RMS matching")
            .track_focus(&self.rms_focus_handle)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&rms_focus, cx)
            })
            .on_click(cx.listener(Self::select_rms_mode))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(solid(RMS_COLOR))
                    .child(format!("RMS {}", format_meter_readout(output_rms_db))),
            );
        let shortfall = self.controller.status.target_shortfall_db();
        let deficit = div().id("target-shortfall").absolute().left(px(104.0)).top(px(212.0)).w(px(80.0))
            .flex().flex_col().items_center().gap(px(3.0))
            .text_size(px(8.0)).line_height(px(11.0)).text_color(solid(ACCENT))
            .tooltip(|_, cx| cx.new(|_| ControlHint("RMS shortfall uses the retained measurement, not the quieter current beat. The gain range limits correction. Peaks above 0 dBFS pass through to the host.")).into())
            .child(format!("{shortfall:.1} dB SHORT"))
            .child(div().text_color(solid(TEXT_MUTED)).child("GAIN LIMIT"));
        let normalize = utility_button(
            "normalize",
            "N",
            &self.normalize_focus,
            window,
            cx.listener(Self::normalize),
        )
        .absolute()
        .left(px(8.0))
        .top(px(MATCH_TOP))
        .w(px(22.0))
        .h(px(MATCH_HEIGHT))
        .aria_label("Normalize: toggle Peak matching at 0 dBFS")
        .tooltip(|_, cx| {
            cx.new(|_| ControlHint("Normalize: toggle Peak matching at 0 dBFS."))
                .into()
        })
        .when(
            match_requested
                && !self.controller.params.rms_mode()
                && self.controller.params.target_db() == 0.0,
            |element| {
                element
                    .bg(vertical_gradient_alpha(
                        BUTTON_ACTIVE_TOP,
                        BUTTON_ACTIVE_BOTTOM,
                        pulse_alpha,
                    ))
                    .text_color(solid(0xffd3ac))
            },
        );
        let help = utility_button(
            "help",
            "?",
            &self.help_focus,
            window,
            cx.listener(|view, _, window, cx| view.open_panel(InfoPanel::Help, window, cx)),
        )
        .absolute()
        .left(px(170.0))
        .top(px(44.0))
        .w(px(18.0))
        .h(px(18.0));
        let gain_label = div()
            .absolute()
            .left(px(104.0))
            .top(px(272.0))
            .w(px(80.0))
            .flex()
            .justify_center()
            .text_size(px(8.0))
            .line_height(px(10.0))
            .text_color(solid(TEXT_MUTED))
            .child("GAIN dB");
        let meter_move = cx.listener(Self::meter_mouse_move);
        let meter_up = cx.listener(Self::meter_mouse_up);
        let numeric_move = cx.listener(Self::numeric_mouse_move);
        let numeric_up = cx.listener(Self::numeric_mouse_up);
        let knob_move = cx.listener(Self::knob_mouse_move);
        let knob_up = cx.listener(Self::knob_mouse_up);
        let drag_events = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        meter_move(event, window, cx);
                        knob_move(event, window, cx);
                        numeric_move(event, window, cx);
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                        meter_up(event, window, cx);
                        knob_up(event, window, cx);
                        numeric_up(event, window, cx);
                    }
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .id("gainsnap-editor")
            .relative()
            .w(px(WINDOW_WIDTH as f32))
            .h(px(WINDOW_HEIGHT as f32))
            .bg(solid(BG_PRIMARY))
            .text_color(solid(TEXT_PRIMARY))
            .font(font("Ioskeley Mono"))
            .text_size(px(13.0))
            .line_height(px(14.0))
            .track_focus(&self.meter_focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| paint_device_frame(bounds, window),
                )
                .size_full(),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    |_, _, window, cx| {
                        paint_label_text(
                            "PORTALSURFER",
                            point(px(66.0), px(30.0)),
                            9.0,
                            TEXT_MUTED,
                            window,
                            cx,
                        );
                        paint_label_text(
                            "/",
                            point(px(111.0), px(30.0)),
                            9.0,
                            TEXT_MUTED,
                            window,
                            cx,
                        );
                        paint_label_text(
                            "GAINSNAP",
                            point(px(144.0), px(30.0)),
                            9.0,
                            ACCENT,
                            window,
                            cx,
                        );
                        paint_label_text(
                            concat!("v", env!("CARGO_PKG_VERSION")),
                            point(px(172.0), px(9.0)),
                            6.0,
                            0x6c776f,
                            window,
                            cx,
                        );
                        paint_label_text(
                            "OUT",
                            point(px(62.5), px(55.0)),
                            9.0,
                            TEXT_MUTED,
                            window,
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(meter)
            .child(target_entry)
            .child(match_button)
            .child(restart)
            .child(knob)
            .child(gain_entry)
            .child(peak_readout)
            .child(rms_readout)
            .when(show_activity_label, |element| element.child(activity_label))
            .child(activity_rail)
            .when(
                match_activity == MatchActivity::BelowTarget && shortfall > 0.0,
                |element| element.child(deficit),
            )
            .child(normalize)
            .child(help)
            .child(gain_label)
            .child(drag_events)
    }
}

impl GainSnapEditor {
    /// Change visibility state while preserving the last measured gain.
    #[allow(dead_code)]
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.controller.set_visible(visible);
    }

    /// Return whether a fresh GPUI frame is needed for telemetry or pulse.
    #[allow(dead_code)]
    pub(crate) fn needs_realtime_redraw(&self) -> bool {
        self.state_changed()
            || self.controller.meter_needs_realtime_redraw()
            || self.controller.params.match_requested()
    }
}

fn selectable_numeric_entry(
    entry: gpui::Stateful<gpui::Div>,
    pointer_start: Rc<RefCell<Option<Point<Pixels>>>>,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let down_start = pointer_start.clone();
    let up_start = pointer_start.clone();
    entry
        .capture_any_mouse_down(move |event, window, cx| {
            if event.button == MouseButton::Left {
                down_start.replace(Some(event.position));
                on_down(event, window, cx);
            }
        })
        .on_mouse_up(MouseButton::Left, move |event, window, cx| {
            let Some(start) = up_start.borrow_mut().take() else {
                return;
            };
            let delta = event.position - start;
            if f32::from(delta.x).hypot(f32::from(delta.y)) > 3.0 {
                return;
            }
            // Preserve drag selection. For a click, use the pinned
            // NumericInput's select-all keyboard handler after pointer dispatch.
            window.defer(cx, |window, cx| {
                window.dispatch_keystroke(gpui::Keystroke::parse("ctrl-a").unwrap(), cx);
            });
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
            pointer_start.replace(None);
        })
}

fn is_space_click(event: &gpui::ClickEvent) -> bool {
    matches!(event, gpui::ClickEvent::Keyboard(key) if key.button == gpui::KeyboardButton::Space)
}

const ACTIVITY_SEGMENT_COUNT: usize = 3;

fn activity_stage(activity: MatchActivity) -> Option<usize> {
    match activity {
        MatchActivity::Listening => Some(0),
        MatchActivity::Adjusting => Some(1),
        MatchActivity::Matched | MatchActivity::BelowTarget => Some(2),
        MatchActivity::Ready | MatchActivity::NoSignal | MatchActivity::Held => None,
    }
}

fn activity_label_text(activity: MatchActivity) -> &'static str {
    match activity {
        MatchActivity::Ready => "",
        MatchActivity::NoSignal => "NO SIGNAL",
        MatchActivity::Held => "HELD",
        MatchActivity::Listening => "LISTENING",
        MatchActivity::Adjusting => "ADJUSTING",
        MatchActivity::Matched => "MATCHED",
        MatchActivity::BelowTarget => "BELOW TARGET",
    }
}

fn activity_label_color(activity: MatchActivity) -> gpui::Rgba {
    match activity {
        MatchActivity::NoSignal => solid(ACCENT_DANGER),
        MatchActivity::Ready | MatchActivity::Held => solid(TEXT_MUTED),
        MatchActivity::BelowTarget => solid(ACCENT),
        MatchActivity::Listening | MatchActivity::Adjusting | MatchActivity::Matched => {
            solid(TEXT_PRIMARY)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MarkerGeometry {
    track: Bounds<Pixels>,
    top_center_y: f32,
    bottom_center_y: f32,
    travel: f32,
    marker_height: f32,
}

fn target_meter_track(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let track_width = TARGET_METER_TRACK_WIDTH.min(f32::from(bounds.size.width));
    let x = f32::from(bounds.left()) + 40.0;
    let height = f32::from(bounds.size.height);
    let inset = TARGET_METER_VERTICAL_INSET.min(height * 0.5);
    Bounds::from_corners(
        point(px(x), bounds.top() + px(inset)),
        point(px(x + track_width), bounds.bottom() - px(inset)),
    )
}

fn target_marker_geometry(bounds: Bounds<Pixels>) -> Option<MarkerGeometry> {
    let track = target_meter_track(bounds);
    let height = f32::from(track.size.height);
    if height <= 0.0 {
        return None;
    }
    let marker_height = TARGET_MARKER_HEIGHT.min(height);
    let top_center_y = f32::from(track.top());
    let bottom_center_y = f32::from(track.bottom());
    Some(MarkerGeometry {
        track,
        top_center_y,
        bottom_center_y,
        travel: (bottom_center_y - top_center_y).max(0.0),
        marker_height,
    })
}

fn target_marker_center_y(bounds: Bounds<Pixels>, target_db: f32) -> Option<f32> {
    let geometry = target_marker_geometry(bounds)?;
    Some(geometry.bottom_center_y - target_level_fraction(target_db) * geometry.travel)
}

fn target_level_fraction(db: f32) -> f32 {
    if db.is_finite() {
        FULL_SCALE_RANGE.normalize(db)
    } else {
        0.0
    }
}

// Place unity gain at -12 dB on the meter, with room to move the handle
// in either direction across the full manual gain range.
fn gain_marker_level_db(gain_db: f32) -> f32 {
    let gain_db = gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
    if gain_db > GAIN_MARKER_HIGH_DB {
        GAIN_MARKER_HIGH_LEVEL_DB
            + (gain_db - GAIN_MARKER_HIGH_DB) * (TARGET_MAX_DB - GAIN_MARKER_HIGH_LEVEL_DB)
                / (GAIN_MAX_DB - GAIN_MARKER_HIGH_DB)
    } else if gain_db >= 0.0 {
        GAIN_MARKER_NEUTRAL_DB
            + gain_db * (GAIN_MARKER_HIGH_LEVEL_DB - GAIN_MARKER_NEUTRAL_DB) / GAIN_MARKER_HIGH_DB
    } else {
        GAIN_MARKER_NEUTRAL_DB
            + gain_db * (GAIN_MARKER_NEUTRAL_DB - FULL_SCALE_RANGE.min) / -GAIN_MIN_DB
    }
}

fn gain_from_marker_level_db(level_db: f32) -> f32 {
    let level_db = level_db.clamp(FULL_SCALE_RANGE.min, FULL_SCALE_RANGE.max);
    if level_db > GAIN_MARKER_HIGH_LEVEL_DB {
        GAIN_MARKER_HIGH_DB
            + (level_db - GAIN_MARKER_HIGH_LEVEL_DB) * (GAIN_MAX_DB - GAIN_MARKER_HIGH_DB)
                / (TARGET_MAX_DB - GAIN_MARKER_HIGH_LEVEL_DB)
    } else if level_db >= GAIN_MARKER_NEUTRAL_DB {
        (level_db - GAIN_MARKER_NEUTRAL_DB) * GAIN_MARKER_HIGH_DB
            / (GAIN_MARKER_HIGH_LEVEL_DB - GAIN_MARKER_NEUTRAL_DB)
    } else {
        (level_db - GAIN_MARKER_NEUTRAL_DB) * -GAIN_MIN_DB
            / (GAIN_MARKER_NEUTRAL_DB - FULL_SCALE_RANGE.min)
    }
}

#[derive(Clone, Copy)]
struct MeterPaintState {
    target_db: f32,
    output_peak_db: f32,
    output_rms_db: f32,
    output_peak_hold_db: f32,
    output_rms_hold_db: f32,
    gain_db: f32,
    focused: bool,
    gain_selected: bool,
}

fn paint_knob_marker(bounds: Bounds<Pixels>, gain_db: f32, window: &mut Window) {
    let fraction = ((gain_db.clamp(MANUAL_GAIN_MIN_DB, MANUAL_GAIN_MAX_DB) - MANUAL_GAIN_MIN_DB)
        / (MANUAL_GAIN_MAX_DB - MANUAL_GAIN_MIN_DB))
        .clamp(0.0, 1.0);
    let angle = (-135.0 + fraction * 270.0_f32).to_radians();
    let center_x = (f32::from(bounds.left()) + f32::from(bounds.right())) * 0.5;
    let center_y = (f32::from(bounds.top()) + f32::from(bounds.bottom())) * 0.5;
    let mut marker = gpui::PathBuilder::stroke(px(1.5));
    for (index, radius) in [2.0_f32, 9.0].into_iter().enumerate() {
        let position = point(
            px(center_x + angle.sin() * radius),
            px(center_y - angle.cos() * radius),
        );
        if index == 0 {
            marker.move_to(position);
        } else {
            marker.line_to(position);
        }
    }
    if let Ok(path) = marker.build() {
        window.paint_path(path, solid(0x9b9e99));
    }
}

fn paint_meter(bounds: Bounds<Pixels>, state: MeterPaintState, window: &mut Window, cx: &mut App) {
    let Some(geometry) = target_marker_geometry(bounds) else {
        return;
    };
    let track = geometry.track;
    window.paint_quad(fill(track, solid(TRACK_INSET)));
    let half_width = f32::from(track.size.width) * 0.5;
    for (level_db, left, color) in [
        (state.output_peak_db, track.left(), ACCENT),
        (
            state.output_rms_db,
            track.left() + px(half_width),
            RMS_COLOR,
        ),
    ] {
        let level = target_level_fraction(level_db);
        if level > 0.0 {
            let top_y =
                target_marker_center_y(bounds, level_db).unwrap_or(f32::from(track.bottom()));
            let (bar_left, bar_right) = if left == track.left() {
                (track.left() + px(4.0), track.left() + px(11.0))
            } else {
                (track.left() + px(14.0), track.left() + px(21.0))
            };
            window.paint_quad(fill(
                Bounds::from_corners(point(bar_left, px(top_y)), point(bar_right, track.bottom())),
                solid(color),
            ));
        }
    }

    for (level_db, left, right, color) in [
        (
            state.output_peak_hold_db,
            track.left(),
            track.left() + px(half_width),
            0xffa38e,
        ),
        (
            state.output_rms_hold_db,
            track.left() + px(half_width),
            track.right(),
            0xc4a3ed,
        ),
    ] {
        if level_db <= TARGET_MIN_DB {
            continue;
        }
        // A distinct cap stays legible above the live bar at the compact size.
        // Keep the two-pixel stripe inside the track at both scale endpoints.
        let y = px(target_marker_center_y(bounds, level_db)
            .unwrap_or(f32::from(track.bottom()))
            .clamp(
                f32::from(track.top()) + 1.0,
                f32::from(track.bottom()) - 3.0,
            ));
        window.paint_quad(fill(
            Bounds::from_corners(
                point(left + px(1.0), y),
                point(right - px(1.0), y + px(2.0)),
            ),
            solid(color),
        ));
    }
    let border_color = solid(BORDER);
    window.paint_quad(fill(
        Bounds::from_corners(
            point(track.left(), track.top()),
            point(track.right(), track.top() + px(1.0)),
        ),
        border_color,
    ));
    window.paint_quad(fill(
        Bounds::from_corners(
            point(track.left(), track.bottom() - px(1.0)),
            track.bottom_right(),
        ),
        border_color,
    ));
    window.paint_quad(fill(
        Bounds::from_corners(
            point(track.left(), track.top()),
            point(track.left() + px(1.0), track.bottom()),
        ),
        border_color,
    ));
    window.paint_quad(fill(
        Bounds::from_corners(
            point(track.right() - px(1.0), track.top()),
            track.bottom_right(),
        ),
        border_color,
    ));

    for index in 0..TARGET_METER_TICK_COUNT {
        let fraction = index as f32 / (TARGET_METER_TICK_COUNT - 1) as f32;
        let y = f32::from(track.top()) + (1.0 - fraction) * f32::from(track.size.height);
        let tick_width = if index % 2 == 0 {
            TARGET_METER_TICK_WIDTH
        } else {
            TARGET_METER_TICK_WIDTH - 1.0
        };
        let x = f32::from(bounds.left()) + 22.0;
        window.paint_quad(fill(
            Bounds::from_corners(
                point(
                    px(x),
                    px((y - TARGET_METER_TICK_HEIGHT * 0.5).max(f32::from(track.top()))),
                ),
                point(px(x + tick_width), px(y + TARGET_METER_TICK_HEIGHT * 0.5)),
            ),
            solid(BORDER),
        ));
    }

    for &(label, db) in &TARGET_METER_LABELS {
        let center_y =
            f32::from(track.bottom()) - target_level_fraction(db) * f32::from(track.size.height);
        let y = (center_y - TARGET_METER_LABEL_FONT_SIZE * 0.5).clamp(
            f32::from(bounds.top()),
            f32::from(bounds.bottom()) - TARGET_METER_LABEL_FONT_SIZE,
        );
        let x = f32::from(bounds.left()) + TARGET_METER_LABEL_GAP;
        let text: gpui::SharedString = label.into();
        let run = TextRun {
            len: text.len(),
            font: font("Ioskeley Mono"),
            color: rgba((TEXT_MUTED << 8) | u32::from(TARGET_METER_LABEL_ALPHA)).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line =
            window
                .text_system()
                .shape_line(text, px(TARGET_METER_LABEL_FONT_SIZE), &[run], None);
        let label_width = f32::from(line.width);
        let _ = line.paint(
            point(px(x + TARGET_METER_LABEL_WIDTH - label_width), px(y)),
            px(TARGET_METER_LABEL_FONT_SIZE),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    let fraction = target_level_fraction(state.target_db);
    let center_y = if fraction >= 1.0 {
        geometry.top_center_y
    } else if fraction <= 0.0 {
        geometry.bottom_center_y
    } else {
        geometry.bottom_center_y - fraction * geometry.travel
    };
    // A full-width rule keeps the selected target legible against both meter colors.
    let line_y = px(center_y);
    window.paint_quad(fill(
        Bounds::from_corners(
            point(track.left() - px(4.0), line_y),
            point(track.right() + px(3.0), line_y + px(1.0)),
        ),
        rgba(0xd6b0a3dd),
    ));
    let marker_left = f32::from(track.left()) - TARGET_MARKER_GAP - TARGET_MARKER_WIDTH;
    let mut marker = gpui::PathBuilder::fill();
    marker.move_to(point(px(marker_left + TARGET_MARKER_WIDTH), px(center_y)));
    marker.line_to(point(
        px(marker_left),
        px(center_y - geometry.marker_height * 0.5),
    ));
    marker.line_to(point(
        px(marker_left),
        px(center_y + geometry.marker_height * 0.5),
    ));
    marker.close();
    if let Ok(marker) = marker.build() {
        window.paint_path(
            marker,
            if state.focused && !state.gain_selected {
                rgba(0xe95843ff)
            } else {
                rgba(0xd6b0a3ff)
            },
        );
    }
    if state.focused && !state.gain_selected {
        let mut emphasis = gpui::PathBuilder::stroke(px(1.0));
        emphasis.move_to(point(px(marker_left + TARGET_MARKER_WIDTH), px(center_y)));
        emphasis.line_to(point(
            px(marker_left),
            px(center_y - geometry.marker_height * 0.5),
        ));
        emphasis.line_to(point(
            px(marker_left),
            px(center_y + geometry.marker_height * 0.5),
        ));
        emphasis.close();
        if let Ok(emphasis) = emphasis.build() {
            window.paint_path(emphasis, rgba(0xe95843ff));
        }
    }
    let gain_y =
        target_marker_center_y(bounds, gain_marker_level_db(state.gain_db)).unwrap_or(center_y);
    let gain_x = f32::from(track.right()) + GAIN_MARKER_GAP;
    let mut gain_marker = gpui::PathBuilder::fill();
    gain_marker.move_to(point(px(gain_x), px(gain_y)));
    gain_marker.line_to(point(
        px(gain_x + TARGET_MARKER_WIDTH),
        px(gain_y - TARGET_MARKER_HEIGHT * 0.5),
    ));
    gain_marker.line_to(point(
        px(gain_x + TARGET_MARKER_WIDTH),
        px(gain_y + TARGET_MARKER_HEIGHT * 0.5),
    ));
    gain_marker.close();
    if let Ok(path) = gain_marker.build() {
        window.paint_path(
            path,
            if state.focused && state.gain_selected {
                solid(ACCENT)
            } else {
                solid(TEXT_PRIMARY)
            },
        );
    }
}

/// Construct the CLAP-hosted GainSnap editor.
pub(crate) fn new_gui(
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
) -> toybox::gpui_gui::GpuiHostedGui {
    new_hosted_gui(
        "GainSnapGpuiClapEditorView",
        params,
        automation_queue,
        status,
        param_requester,
        None,
        true,
    )
}

/// Build the deterministic fixture used by the GPUI screenshot runner.
/// The `screenshot_renders_initial_ui` integration test launches that runner
/// on the process main thread so captures use the live native renderer.
#[cfg(feature = "screenshot-test")]
#[doc(hidden)]
pub fn new_screenshot_gui(matching: bool, rms: bool) -> toybox::gpui_gui::GpuiHostedGui {
    new_screenshot_gui_with_params(matching, rms).0
}

/// Configure live-looking telemetry for the named native screenshot state.
#[cfg(feature = "screenshot-test")]
#[doc(hidden)]
pub fn configure_screenshot_state(
    params: &Arc<crate::params::GainSnapParams>,
    status: &Arc<GuiStatus>,
    name: &str,
) {
    let _ = status.take_meter_maxima();
    let (peak_db, rms_db, gain_db, state, activity, matching) = match name {
        "below-target" => (
            0.0,
            -14.4,
            6.0,
            MatchState::Measuring,
            MatchActivity::BelowTarget,
            true,
        ),
        "matched" => (
            -3.0,
            -12.0,
            6.0,
            MatchState::Locked,
            MatchActivity::Matched,
            true,
        ),
        "held" => (
            -3.0,
            -12.0,
            6.0,
            MatchState::Locked,
            MatchActivity::Held,
            false,
        ),
        "no-signal" => (
            -120.0,
            -120.0,
            0.0,
            MatchState::NoSignal,
            MatchActivity::NoSignal,
            false,
        ),
        "listening" => (
            -9.0,
            -18.0,
            0.0,
            MatchState::Measuring,
            MatchActivity::Listening,
            true,
        ),
        _ => (
            -9.0,
            -18.0,
            0.0,
            MatchState::Ready,
            MatchActivity::Ready,
            false,
        ),
    };
    params.set_param(PARAM_MATCH, f32::from(matching));
    params.set_param(PARAM_MANUAL_MODE, 1.0);
    params.set_param(PARAM_MANUAL_GAIN_DB, gain_db);
    params.set_param(PARAM_LOCKED_GAIN_DB, gain_db);
    status.update_correction_feedback(
        gain_db,
        if activity == MatchActivity::BelowTarget {
            2.4
        } else {
            0.0
        },
    );
    status.update_with_activity(peak_db - gain_db, peak_db, gain_db, 0.0, state, activity);
    status.update_rms(rms_db);
}

/// Construct the deterministic screenshot fixture and expose its parameter
/// store for the native AppKit input regression. The GUI is still driven only
/// through the hosted surface; the returned Arc lets the harness assert the
/// same shared state that the editor updates.
#[cfg(feature = "screenshot-test")]
#[doc(hidden)]
pub fn new_screenshot_gui_with_params(
    matching: bool,
    rms: bool,
) -> (
    toybox::gpui_gui::GpuiHostedGui,
    Arc<crate::params::GainSnapParams>,
    Arc<GuiStatus>,
) {
    let params = Arc::new(crate::params::GainSnapParams::new());
    params.set_param(PARAM_MATCH, f32::from(matching));
    params.set_param(PARAM_RMS_MODE, f32::from(rms));

    let status = Arc::new(GuiStatus::default());
    let state = if matching {
        MatchState::Measuring
    } else {
        MatchState::Ready
    };
    let level_db = if matching { -12.0 } else { -120.0 };
    status.update(level_db, level_db, 0.0, 0.0, state);
    status.update_rms(level_db);

    let gui = new_hosted_gui(
        "GainSnapGpuiScreenshotView",
        Arc::clone(&params),
        Arc::new(AutomationQueue::default()),
        Arc::clone(&status),
        None,
        None,
        false,
    );
    (gui, params, status)
}

/// Construct the GPUI host facade with a format-specific parameter sink.
#[cfg(feature = "vst3")]
pub(crate) fn new_gui_with_edit_sink(
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    status: Arc<GuiStatus>,
    edit_sink: Arc<dyn HostParamEditSink>,
) -> toybox::gpui_gui::GpuiHostedGui {
    new_hosted_gui(
        "GainSnapGpuiVst3EditorView",
        params,
        automation_queue,
        status,
        None,
        Some(edit_sink),
        true,
    )
}

fn new_hosted_gui(
    class_name: &'static str,
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
    edit_sink: Option<Arc<dyn HostParamEditSink>>,
    remember_mode_choices: bool,
) -> toybox::gpui_gui::GpuiHostedGui {
    let mut controller = EditorController::new(
        Arc::clone(&params),
        Arc::clone(&automation_queue),
        Arc::clone(&status),
        param_requester,
        edit_sink,
    );
    controller.remember_mode_choices = remember_mode_choices && !cfg!(test);
    let factory_controller = controller.clone();
    let visibility_controller = controller;
    toybox::gpui_gui::GpuiHostedGui::new(
        class_name,
        move |_window, cx| {
            cx.new(|cx| GainSnapEditor::new(factory_controller.clone(), cx))
                .into()
        },
        WINDOW_WIDTH,
        WINDOW_HEIGHT,
    )
    .with_size_contract(
        (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT),
        (WINDOW_WIDTH, WINDOW_HEIGHT),
        (MAX_WINDOW_WIDTH, MAX_WINDOW_HEIGHT),
    )
    .with_visibility_callback(move |visible| {
        let mut controller = visibility_controller.clone();
        controller.set_visible(visible);
    })
}

/// Return the initial size used by host GUI negotiation.
pub(crate) const fn preferred_window_size() -> (u32, u32) {
    (WINDOW_WIDTH, WINDOW_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_text_parser_accepts_units_and_clamps() {
        assert_eq!(parse_target_text("-12.0"), Some(-12.0));
        assert_eq!(parse_target_text("-12 dBFS"), Some(-12.0));
        assert_eq!(parse_target_text("-12 dB"), Some(-12.0));
        assert_eq!(parse_target_text("8"), Some(0.0));
        assert_eq!(parse_target_text("-80"), Some(-36.0));
        assert_eq!(parse_target_text("--"), None);
    }

    #[test]
    fn target_steps_use_one_db_and_shift_tenth_db() {
        assert_eq!(step_target_db(-12.0, TargetStepDirection::Up, false), -11.0);
        assert_eq!(
            step_target_db(-12.0, TargetStepDirection::Down, false),
            -13.0
        );
        assert_eq!(step_target_db(-12.0, TargetStepDirection::Up, true), -11.9);
        assert_eq!(
            step_target_db(TARGET_MAX_DB, TargetStepDirection::Up, false),
            TARGET_MAX_DB
        );
        assert_eq!(
            step_target_db(TARGET_MIN_DB, TargetStepDirection::Down, false),
            TARGET_MIN_DB
        );
    }

    #[test]
    fn meter_uses_full_scale_while_target_editing_keeps_its_parameter_range() {
        assert_eq!(target_level_fraction(-60.0), 0.0);
        assert!((target_level_fraction(-36.0) - 0.4).abs() < 0.0001);
        assert!((target_level_fraction(-12.0) - 0.8).abs() < 0.0001);
        assert_eq!(target_level_fraction(0.0), 1.0);
        assert!((FULL_SCALE_RANGE.denormalize(0.4) - TARGET_RANGE.min).abs() < 0.0001);
        assert_eq!(
            FULL_SCALE_RANGE.denormalize(0.0).max(TARGET_RANGE.min),
            TARGET_RANGE.min
        );
        assert_eq!(format_meter_readout(-120.0), "−∞");
        assert_eq!(format_meter_readout(-9.0), "−9.0");
    }

    #[test]
    fn peak_hold_promotes_independent_lanes_holds_then_releases_to_live_bar() {
        let start = Instant::now();
        let mut peak = PeakHold::new(-20.0, start);
        let mut rms = PeakHold::new(-30.0, start);
        peak.advance_at(-6.0, -20.0, start);
        rms.advance_at(-12.0, -30.0, start);
        assert_eq!(peak.level_db, -6.0);
        assert_eq!(rms.level_db, -12.0);

        let quiet = start + Duration::from_secs(1);
        peak.advance_at(-24.0, -18.0, quiet);
        rms.advance_at(-30.0, -25.0, quiet);
        assert_eq!(peak.level_db, -6.0, "the peak remains held for two seconds");
        assert_eq!(rms.level_db, -12.0);

        let release = start + Duration::from_millis(2500);
        peak.advance_at(-24.0, -18.0, release);
        rms.advance_at(-30.0, -25.0, release);
        assert!((peak.level_db - (-6.75)).abs() < 0.001);
        assert!((rms.level_db - (-12.75)).abs() < 0.001);
        assert!(peak.level_db >= -18.0 && rms.level_db >= -25.0);
    }

    #[test]
    fn peak_hold_release_is_cadence_independent_and_handles_long_gaps() {
        let start = Instant::now();
        let mut direct = PeakHold::new(-8.0, start);
        let mut frequent = direct;
        direct.advance_at(-8.0, -8.0, start);
        for milliseconds in [500, 1000, 1500, 2000, 2250, 2500] {
            frequent.advance_at(-24.0, -24.0, start + Duration::from_millis(milliseconds));
        }
        direct.advance_at(-24.0, -24.0, start + Duration::from_millis(2500));
        assert!((direct.level_db - frequent.level_db).abs() < 0.001);

        let mut long_gap = PeakHold::new(-8.0, start);
        long_gap.advance_at(-24.0, -24.0, start + Duration::from_secs(20));
        assert_eq!(
            long_gap.level_db, -24.0,
            "the marker never falls below live"
        );
        assert_eq!(
            sanitize_meter_level_db(f32::NAN),
            -120.0,
            "nonfinite telemetry is treated as silence"
        );
    }

    #[test]
    fn repeated_equal_peak_refreshes_hold_deadline() {
        let start = Instant::now();
        let mut hold = PeakHold::new(-5.0, start);
        hold.advance_at(-5.0, -5.0, start + Duration::from_secs(1));
        hold.advance_at(-18.0, -18.0, start + Duration::from_millis(2500));
        assert_eq!(hold.level_db, -5.0);
        hold.advance_at(-18.0, -18.0, start + Duration::from_millis(3500));
        assert!((hold.level_db - (-5.75)).abs() < 0.001);
    }

    #[test]
    fn shift_drag_snaps_to_whole_db_and_clamps_to_control_range() {
        assert_eq!(
            snap_drag_db(-11.6, true, TARGET_MIN_DB, TARGET_MAX_DB),
            -12.0
        );
        assert_eq!(
            snap_drag_db(-11.4, true, TARGET_MIN_DB, TARGET_MAX_DB),
            -11.0
        );
        assert_eq!(
            snap_drag_db(-11.6, false, TARGET_MIN_DB, TARGET_MAX_DB),
            -11.6
        );
        assert_eq!(
            snap_drag_db(-36.4, true, TARGET_MIN_DB, TARGET_MAX_DB),
            TARGET_MIN_DB
        );
        assert_eq!(
            snap_drag_db(120.6, true, GAIN_MIN_DB, GAIN_MAX_DB),
            GAIN_MAX_DB
        );
    }

    #[test]
    fn gain_marker_starts_at_minus_twelve_and_covers_manual_range() {
        assert!((gain_marker_level_db(6.0) - (-10.5)).abs() < 0.001);
        for (gain_db, marker_db) in [
            (GAIN_MIN_DB, FULL_SCALE_RANGE.min),
            (-18.0, -36.0),
            (0.0, GAIN_MARKER_NEUTRAL_DB),
            (18.0, -7.5),
            (GAIN_MARKER_HIGH_DB, GAIN_MARKER_HIGH_LEVEL_DB),
            (GAIN_MAX_DB, TARGET_MAX_DB),
        ] {
            assert!((gain_marker_level_db(gain_db) - marker_db).abs() < 0.001);
            assert!((gain_from_marker_level_db(marker_db) - gain_db).abs() < 0.001);
        }
    }

    #[test]
    fn controller_visibility_disables_active_matching() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_MATCH, 1.0);
        let mut controller = EditorController::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );

        controller.set_visible(false);

        assert!(!params.match_requested());
    }

    #[test]
    fn restart_control_requests_a_new_session_only_while_matching() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let controller = EditorController::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );

        controller.restart_matching();
        assert_eq!(params.restart_generation(), 0);

        params.set_param(PARAM_MATCH, 1.0);
        controller.restart_matching();
        assert_eq!(params.restart_generation(), 1);
    }
}
