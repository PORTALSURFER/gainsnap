//! Native GPUI editor for GainSnap on macOS and Windows.
//!
//! The editor keeps the compact GainSnap surface deliberately small: the
//! controller owns only retained UI state and forwards parameter gestures to
//! the CLAP queue or the VST3 component handler. GPUI owns drawing, focus,
//! pointer routing, and native text input/IME transport.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use toybox::clack_plugin::utils::ClapId;
use toybox::clap::automation::{AutomationConfig, AutomationQueue};
use toybox::gpui::{
    self as gpui, actions, canvas, div, fill, font, point, prelude::*, px, relative, rgba, size,
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    KeyDownEvent, LayoutId, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ShapedLine, Style, Subscription, TextRun, UTF16Selection,
    UnderlineStyle, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::clap_plugin::HostParamRequester;
use crate::params::{PARAM_MATCH, PARAM_RMS_MODE, PARAM_TARGET_DB, TARGET_MAX_DB, TARGET_MIN_DB};
use crate::status::{GuiStatus, MatchState};

/// Preferred logical editor width for the compact toggle control surface.
pub const WINDOW_WIDTH: u32 = 208;
/// Preferred logical editor height for the compact toggle control surface.
pub const WINDOW_HEIGHT: u32 = 212;
/// Minimum logical editor width.
pub const MIN_WINDOW_WIDTH: u32 = WINDOW_WIDTH;
/// Minimum logical editor height.
pub const MIN_WINDOW_HEIGHT: u32 = WINDOW_HEIGHT;
/// Maximum logical editor width.
pub const MAX_WINDOW_WIDTH: u32 = 480;
/// Maximum logical editor height.
pub const MAX_WINDOW_HEIGHT: u32 = 520;

const TARGET_TEXT_SYNC_EPSILON: f32 = 0.0001;
const TARGET_CONTROL_WIDTH: f32 = 68.0;
const TARGET_METER_HEIGHT: f32 = 148.0;
const TARGET_ENTRY_HEIGHT: f32 = 28.0;
const TARGET_CONTROL_SPACING: f32 = 6.0;
const ACTION_CONTROL_WIDTH: f32 = 96.0;
const ACTION_CONTROL_HEIGHT: f32 =
    TARGET_METER_HEIGHT + TARGET_CONTROL_SPACING + TARGET_ENTRY_HEIGHT;
const MATCHING_CONTROL_HEIGHT: f32 = 92.0;
const MATCH_BUTTON_HEIGHT: f32 = 32.0;
const NORMALIZE_BUTTON_HEIGHT: f32 = 24.0;
const STATUS_INDICATOR_SIZE: f32 = 12.0;
const TARGET_METER_TRACK_WIDTH: f32 = 14.0;
const TARGET_METER_VERTICAL_INSET: f32 = 2.0;
const TARGET_METER_TICK_COUNT: usize = 13;
const TARGET_METER_TICK_WIDTH: f32 = 4.0;
const TARGET_METER_TICK_HEIGHT: f32 = 1.0;
const TARGET_METER_LABELS: [(&str, f32); 7] = [
    ("0 dB", TARGET_MAX_DB),
    ("−6", -6.0),
    ("−12", -12.0),
    ("−18", -18.0),
    ("−24", -24.0),
    ("−30", -30.0),
    ("−∞", TARGET_MIN_DB),
];
const TARGET_METER_LABEL_WIDTH: f32 = 23.0;
const TARGET_METER_LABEL_GAP: f32 = 3.0;
const TARGET_METER_LABEL_FONT_SIZE: f32 = 8.0;
const TARGET_METER_LABEL_ALPHA: u8 = 190;
const TARGET_MARKER_GAP: f32 = 5.0;
const TARGET_MARKER_WIDTH: f32 = 8.0;
const TARGET_MARKER_HEIGHT: f32 = 8.0;
const METER_ATTACK_SECONDS: f32 = 0.010;
const METER_RELEASE_SECONDS: f32 = 0.300;
const METER_MAX_ELAPSED_SECONDS: f32 = 0.100;
const METER_SETTLE_EPSILON_DB: f32 = 0.01;
const TARGET_KEYBOARD_STEP_DB: f32 = 1.0;
const TARGET_FINE_KEYBOARD_STEP_DB: f32 = 0.1;
const SURFACE_PADDING_X: f32 = 16.0;
const SURFACE_PADDING_Y: f32 = 15.0;
const SURFACE_COLUMN_GAP: f32 = 12.0;
const MATCH_PULSE_SECONDS: f32 = 1.2;
const MAX_TARGET_TEXT_BYTES: usize = 64;

const TARGET_ENTRY_AUTOMATION_LABEL: &str = "Target level, dBFS";
const NORMALIZE_AUTOMATION_DESCRIPTION: &str = "Peak normalize to 0 dBFS and start Match";

// The original GainSnap dark theme values are part of the compact visual
// contract preserved by the GPUI editor.
const BG_PRIMARY: u32 = 0x1b1e1e;
const BORDER: u32 = 0x282b2b;
const BORDER_EMPHASIS: u32 = 0x404342;
const ACCENT: u32 = 0xe95843;
const ACCENT_DANGER: u32 = 0xef4c3d;
const TEXT_PRIMARY: u32 = 0xd8d7d3;
const TEXT_MUTED: u32 = 0x999b9a;
const HIGHLIGHT_CYAN: u32 = 0xaeb0ad;

fn solid(value: u32) -> gpui::Rgba {
    rgba((value << 8) | 0xff)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisplaySnapshot {
    target_db: u32,
    match_requested: bool,
    rms_mode: bool,
    output_peak_db: u32,
    state: MatchState,
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
            output_peak_db: sanitize_meter_level_db(status.output_level_db(params.rms_mode()))
                .to_bits(),
            state: status.state(),
        }
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
    shift_held: bool,
    output_peak_db: f32,
    meter_last_update: Instant,
    pulse_started: Option<Instant>,
    pulse_alpha: u8,
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
        let output_peak_db = sanitize_meter_level_db(status.output_level_db(params.rms_mode()));
        Self {
            params,
            automation_queue,
            automation_config: AutomationConfig::default(),
            status,
            param_requester,
            edit_sink,
            target_text_param: target_db,
            shift_held: false,
            output_peak_db,
            meter_last_update: Instant::now(),
            pulse_started: None,
            pulse_alpha: 255,
        }
    }

    fn target_db(&self) -> f32 {
        self.parameter_value(PARAM_TARGET_DB, TARGET_RANGE)
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

    fn set_target_from_fraction(&mut self, value: f32) {
        self.set_target_db(TARGET_RANGE.denormalize(clamp_fraction(value)));
    }

    fn set_target_from_position(&mut self, bounds: Bounds<Pixels>, position: Point<Pixels>) {
        let track = target_meter_track(bounds);
        let Some(geometry) = target_marker_geometry(track) else {
            return;
        };
        let fraction = if geometry.travel <= 0.0 {
            0.0
        } else {
            clamp_fraction((geometry.bottom_center_y - f32::from(position.y)) / geometry.travel)
        };
        self.set_target_from_fraction(fraction);
    }

    fn step_target(&mut self, direction: TargetStepDirection) -> Option<String> {
        let target_db = self.target_db();
        let next_target_db = step_target_db(target_db, direction, self.shift_held);
        if (next_target_db - target_db).abs() > TARGET_TEXT_SYNC_EPSILON {
            self.set_target_db(next_target_db);
        } else {
            self.target_text_param = target_db;
        }
        Some(format_target_text(self.target_text_param))
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

    fn normalize(&mut self) -> String {
        self.toggle_value(PARAM_RMS_MODE, false);
        self.set_target_db(TARGET_MAX_DB);
        self.toggle_value(PARAM_MATCH, true);
        format_target_text(self.target_text_param)
    }

    fn set_visible(&mut self, visible: bool) {
        if !visible && self.params.match_requested() {
            self.toggle_value(PARAM_MATCH, false);
        }
    }

    fn set_shift_held(&mut self, shift_held: bool) {
        self.shift_held = shift_held;
    }

    fn advance_meter_at(&mut self, now: Instant) -> bool {
        let target_db =
            sanitize_meter_level_db(self.status.output_level_db(self.params.rms_mode()));
        let elapsed = now.saturating_duration_since(self.meter_last_update);
        self.meter_last_update = now;
        let next_db = smooth_meter_level_db(self.output_peak_db, target_db, elapsed);
        let changed = (next_db - self.output_peak_db).abs() > f32::EPSILON;
        self.output_peak_db = next_db;
        changed
    }

    fn advance_pulse_at(&mut self, now: Instant) -> bool {
        let next = if self.params.match_requested() {
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
        let target_db =
            sanitize_meter_level_db(self.status.output_level_db(self.params.rms_mode()));
        (self.output_peak_db - target_db).abs() > METER_SETTLE_EPSILON_DB
    }
}

actions!(
    gainsnap_target_input,
    [
        /// Delete the character before the caret.
        Backspace,
        /// Delete the character after the caret.
        Delete,
        /// Move the caret left.
        Left,
        /// Move the caret right.
        Right,
        /// Extend the selection left by one grapheme.
        SelectLeft,
        /// Extend the selection right by one grapheme.
        SelectRight,
        /// Select the complete target value.
        SelectAll,
        /// Move the caret to the beginning.
        Home,
        /// Move the caret to the end.
        End,
        /// Paste clipboard text into the target field.
        Paste,
        /// Copy the selected target text.
        Copy,
        /// Cut the selected target text.
        Cut,
        /// Open the platform character palette.
        ShowCharacterPalette,
        /// Submit the target value.
        Submit,
        /// Cancel the current target draft.
        Cancel,
    ]
);

fn install_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TargetInput")),
        KeyBinding::new("delete", Delete, Some("TargetInput")),
        KeyBinding::new("left", Left, Some("TargetInput")),
        KeyBinding::new("right", Right, Some("TargetInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TargetInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TargetInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TargetInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TargetInput")),
        KeyBinding::new("cmd-v", Paste, Some("TargetInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TargetInput")),
        KeyBinding::new("cmd-c", Copy, Some("TargetInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TargetInput")),
        KeyBinding::new("cmd-x", Cut, Some("TargetInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TargetInput")),
        KeyBinding::new("home", Home, Some("TargetInput")),
        KeyBinding::new("end", End, Some("TargetInput")),
        KeyBinding::new("enter", Submit, Some("TargetInput")),
        KeyBinding::new("escape", Cancel, Some("TargetInput")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("TargetInput")),
    ]);
}

/// Event emitted after the target text changes through GPUI input or IME.
pub(crate) struct TargetTextChanged;

/// Event emitted when the user submits the target text with Enter.
pub(crate) struct TargetTextSubmitted;

/// Event emitted when the user cancels a target draft with Escape.
pub(crate) struct TargetTextCanceled;

/// Native GPUI target-level text field.
pub(crate) struct TargetInput {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<TargetTextChanged> for TargetInput {}
impl EventEmitter<TargetTextSubmitted> for TargetInput {}
impl EventEmitter<TargetTextCanceled> for TargetInput {}

impl TargetInput {
    fn new(content: String, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
        }
    }

    fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn content(&self) -> &str {
        &self.content
    }

    fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content;
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace('\r', "").replace('\n', " ");
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_owned(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TargetTextSubmitted);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TargetTextCanceled);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        for (index, ch) in self.content.char_indices() {
            if utf16_offset >= offset {
                return index;
            }
            utf16_offset += ch.len_utf16();
            if utf16_offset > offset {
                return index + ch.len_utf8();
            }
        }
        self.content.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].chars().map(char::len_utf16).sum()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for TargetInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        if range.start > range.end
            || range.end > self.content.len()
            || !self.content.is_char_boundary(range.start)
            || !self.content.is_char_boundary(range.end)
        {
            return;
        }
        let resulting_len = self
            .content
            .len()
            .saturating_sub(range.end - range.start)
            .saturating_add(new_text.len());
        if resulting_len > MAX_TARGET_TEXT_BYTES {
            return;
        }
        self.content.replace_range(range.clone(), new_text);
        let offset = range.start + new_text.len();
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.emit(TargetTextChanged);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        if range.start > range.end
            || range.end > self.content.len()
            || !self.content.is_char_boundary(range.start)
            || !self.content.is_char_boundary(range.end)
        {
            return;
        }
        let resulting_len = self
            .content
            .len()
            .saturating_sub(range.end - range.start)
            .saturating_add(new_text.len());
        if resulting_len > MAX_TARGET_TEXT_BYTES {
            return;
        }
        self.content.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range =
            marked_selection_range(range.start, new_text, new_selected_range_utf16.as_ref());
        self.selection_reversed = false;
        cx.emit(TargetTextChanged);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        Some(self.offset_to_utf16(line.closest_index_for_x(point.x - bounds.left())))
    }
}

fn marked_selection_range(
    insertion_start: usize,
    text: &str,
    selection_utf16: Option<&Range<usize>>,
) -> Range<usize> {
    let Some(selection) = selection_utf16 else {
        let end = insertion_start + text.len();
        return end..end;
    };
    let start = offset_from_utf16(text, selection.start);
    let end = offset_from_utf16(text, selection.end).max(start);
    insertion_start + start..insertion_start + end
}

fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    for (index, ch) in text.char_indices() {
        if offset <= utf16_offset {
            return index;
        }
        let next_utf16_offset = utf16_offset + ch.len_utf16();
        if offset < next_utf16_offset {
            // A UTF-16 offset can land between the surrogate halves of a
            // non-BMP scalar. GPUI's byte ranges cannot represent that
            // position, so keep it at the scalar's leading byte.
            return index;
        }
        utf16_offset = next_utf16_offset;
    }
    text.len()
}

impl Focusable for TargetInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct TargetTextElement {
    input: Entity<TargetInput>,
}

struct TargetTextPrepaint {
    line: Option<ShapedLine>,
    cursor: Option<gpui::PaintQuad>,
    selection: Option<gpui::PaintQuad>,
}

impl gpui::IntoElement for TargetTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TargetTextElement {
    type RequestLayoutState = ();
    type PrepaintState = TargetTextPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let text = input.content.clone();
        let run = TextRun {
            len: text.len(),
            font: font("Ioskeley Mono"),
            color: style.color,
            background_color: None,
            underline: input.marked_range.as_ref().map(|_| UnderlineStyle {
                color: Some(style.color),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text.into(), px(13.0), &[run], None);
        let cursor_position = line.x_for_index(input.cursor_offset());
        let (selection, cursor) = if input.selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_position, bounds.top()),
                        size(px(1.0), bounds.size.height),
                    ),
                    rgba(0xd8d7d3ff),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(input.selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(input.selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0xe9584340),
                )),
                None,
            )
        };
        TargetTextPrepaint {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("target text line");
        let _ = line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TargetInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .key_context("TargetInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, |_event, _window, _cx| {})
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            // Match Radiant's compact text-input content inset.
            .px(px(8.0))
            .child(TargetTextElement { input: cx.entity() })
    }
}

/// GPUI retained view for the compact GainSnap editor.
pub(crate) struct GainSnapEditor {
    controller: EditorController,
    target_input: Entity<TargetInput>,
    #[allow(dead_code)]
    target_text_subscription: Subscription,
    #[allow(dead_code)]
    target_submit_subscription: Subscription,
    #[allow(dead_code)]
    target_cancel_subscription: Subscription,
    meter_focus_handle: FocusHandle,
    mode_focus_handle: FocusHandle,
    match_focus_handle: FocusHandle,
    normalize_focus_handle: FocusHandle,
    meter_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    target_dragging: bool,
    last_display_snapshot: DisplaySnapshot,
}

impl GainSnapEditor {
    fn new(controller: EditorController, cx: &mut Context<Self>) -> Self {
        let target_input = cx.new(|cx| TargetInput::new(controller.target_text(), cx));
        let target_text_subscription =
            cx.subscribe(&target_input, |view, input, _: &TargetTextChanged, cx| {
                let text = input.read(cx).content().to_owned();
                view.apply_target_text(text, false, cx);
            });
        let target_submit_subscription =
            cx.subscribe(&target_input, |view, input, _: &TargetTextSubmitted, cx| {
                let text = input.read(cx).content().to_owned();
                view.apply_target_text(text, true, cx);
            });
        let target_cancel_subscription =
            cx.subscribe(&target_input, |view, input, _: &TargetTextCanceled, cx| {
                let text = view.controller.target_text();
                input.update(cx, |input, cx| input.set_content(text, cx));
                cx.notify();
            });
        let last_display_snapshot =
            DisplaySnapshot::capture(&controller.params, &controller.status);
        Self {
            controller,
            target_input,
            target_text_subscription,
            target_submit_subscription,
            target_cancel_subscription,
            meter_focus_handle: cx.focus_handle(),
            mode_focus_handle: cx.focus_handle(),
            match_focus_handle: cx.focus_handle(),
            normalize_focus_handle: cx.focus_handle(),
            meter_bounds: Rc::new(RefCell::new(None)),
            target_dragging: false,
            last_display_snapshot,
        }
    }

    fn apply_target_text(&mut self, text: String, submitted: bool, cx: &mut Context<Self>) {
        if let Some(formatted) = self.controller.target_text_changed(&text, submitted) {
            self.target_input
                .update(cx, |input, cx| input.set_content(formatted, cx));
        }
        cx.notify();
    }

    fn set_target_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.target_input
            .update(cx, |input, cx| input.set_content(text, cx));
    }

    fn handle_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.controller.set_shift_held(event.modifiers.shift);
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let button_focused = self.mode_focus_handle.is_focused(window)
            || self.match_focus_handle.is_focused(window)
            || self.normalize_focus_handle.is_focused(window);
        if button_focused {
            // GPUI's interactive button turns a clean Space or Enter
            // keydown/keyup pair into its ClickEvent. Consume the keydown at
            // the editor boundary so VST3 hosts treat the activation as
            // handled and do not also use Space for transport. Modified
            // variants remain available to the host and cannot activate the
            // button.
            if matches!(event.keystroke.key.as_str(), "space" | "enter")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
            }
            return;
        }
        let input_focused = self.target_input.read(cx).focus_handle().is_focused(window);
        let meter_focused = self.meter_focus_handle.is_focused(window);
        if !(input_focused || meter_focused) {
            return;
        }
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.function
        {
            return;
        }
        self.controller
            .set_shift_held(event.keystroke.modifiers.shift);
        let direction = if input_focused {
            match event.keystroke.key.as_str() {
                "up" => Some(TargetStepDirection::Up),
                "down" => Some(TargetStepDirection::Down),
                _ => None,
            }
        } else {
            match event.keystroke.key.as_str() {
                "up" | "right" => Some(TargetStepDirection::Up),
                "down" | "left" => Some(TargetStepDirection::Down),
                _ => None,
            }
        };
        if let Some(direction) = direction {
            if let Some(text) = self.controller.step_target(direction) {
                self.set_target_text(text, cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if meter_focused {
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
                self.set_target_text(text, cx);
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
        self.target_dragging = true;
        let bounds = self.meter_bounds.borrow().as_ref().copied();
        if let Some(bounds) = bounds {
            self.controller
                .set_target_from_position(bounds, event.position);
            self.set_target_text(self.controller.target_text(), cx);
        }
        cx.notify();
    }

    fn meter_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.target_dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let bounds = self.meter_bounds.borrow().as_ref().copied();
        if let Some(bounds) = bounds {
            self.controller
                .set_target_from_position(bounds, event.position);
            self.set_target_text(self.controller.target_text(), cx);
            cx.notify();
        }
    }

    fn meter_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.target_dragging = false;
    }

    fn toggle_mode(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.controller
            .toggle_value(PARAM_RMS_MODE, !self.controller.params.rms_mode());
        cx.notify();
    }

    fn toggle_match(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.controller
            .toggle_value(PARAM_MATCH, !self.controller.params.match_requested());
        cx.notify();
    }

    fn normalize(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.controller.normalize();
        self.set_target_text(text, cx);
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
            if let Some(text) = self
                .controller
                .sync_target_text(self.controller.target_db())
            {
                self.set_target_text(text, cx);
            }
            cx.notify();
        }
        if meter_changed
            || pulse_changed
            || self.controller.meter_needs_realtime_redraw()
            || self.controller.params.match_requested()
        {
            window.request_animation_frame();
        }
    }
}

impl Render for GainSnapEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.tick(window, cx);
        let meter_bounds = Rc::clone(&self.meter_bounds);
        let target_db = self.controller.target_db();
        let output_peak_db = self.controller.output_peak_db;
        let target_focused = self.target_input.read(cx).focus_handle().is_focused(window);
        let meter_focused = self.meter_focus_handle.is_focused(window);
        let mode_rms = self.controller.params.rms_mode();
        let match_requested = self.controller.params.match_requested();
        let pulse_alpha = self.controller.pulse_alpha;
        let match_state = self.controller.status.state();
        let target_input = self.target_input.clone();

        let meter = div()
            .id("target-meter")
            .relative()
            .w(px(TARGET_CONTROL_WIDTH))
            .h(px(TARGET_METER_HEIGHT))
            .track_focus(&self.meter_focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::meter_mouse_down))
            .on_mouse_move(cx.listener(Self::meter_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::meter_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::meter_mouse_up))
            .child(
                canvas(
                    move |bounds, _, _| {
                        *meter_bounds.borrow_mut() = Some(bounds);
                    },
                    move |bounds, _, window, cx| {
                        paint_meter(
                            bounds,
                            target_db,
                            output_peak_db,
                            target_focused || meter_focused,
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            );

        let target_entry = div()
            .id("target-entry")
            .w(px(TARGET_CONTROL_WIDTH))
            .h(px(TARGET_ENTRY_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .bg(solid(BG_PRIMARY))
            .border_1()
            .border_color(solid(BORDER))
            .text_color(solid(TEXT_PRIMARY))
            .font(font("Ioskeley Mono"))
            // Radiant's compact 28px text input uses a 13px mono face. Keep
            // the same scale when the field is painted by GPUI.
            .text_size(px(13.0))
            .line_height(px(14.0))
            .aria_label(TARGET_ENTRY_AUTOMATION_LABEL)
            .child(target_input);

        let target_control = div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(TARGET_CONTROL_WIDTH))
            .h(px(ACTION_CONTROL_HEIGHT))
            .gap(px(TARGET_CONTROL_SPACING))
            .children([meter, target_entry]);

        let mode = button_element(
            "level-mode",
            if mode_rms { "RMS" } else { "PEAK" },
            // Radiant's mode toggle uses the subtle control chrome for both
            // choices; its label changes without becoming a filled action.
            false,
            ACTION_CONTROL_WIDTH,
            24.0,
            &self.mode_focus_handle,
            cx.listener(Self::toggle_mode),
        );
        let match_button = button_element(
            "match-now",
            "MATCH",
            match_requested,
            ACTION_CONTROL_WIDTH,
            MATCH_BUTTON_HEIGHT,
            &self.match_focus_handle,
            cx.listener(Self::toggle_match),
        )
        .when(match_requested, |element| {
            element.bg(rgba(ACCENT << 8 | u32::from(pulse_alpha)))
        });
        let normalize = div()
            .id("normalize")
            .w(px(ACTION_CONTROL_WIDTH))
            .h(px(NORMALIZE_BUTTON_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(solid(BORDER))
            .bg(solid(BG_PRIMARY))
            .text_color(solid(TEXT_MUTED))
            .font(font("Ioskeley Mono"))
            .text_size(px(13.0))
            .line_height(px(14.0))
            .role(gpui::Role::Button)
            .aria_label(NORMALIZE_AUTOMATION_DESCRIPTION)
            .track_focus(&self.normalize_focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let focus_handle = self.normalize_focus_handle.clone();
                move |_, window, cx| window.focus(&focus_handle, cx)
            })
            .on_click(cx.listener(Self::normalize))
            .child("Normalize");

        let matching = div()
            .flex()
            .flex_col()
            .items_end()
            .w(px(ACTION_CONTROL_WIDTH))
            .h(px(MATCHING_CONTROL_HEIGHT))
            .gap(px(6.0))
            .children([mode, match_button, normalize]);

        let dot_color = match_status_color(match_state, match_requested, pulse_alpha);
        let status_dot = div()
            .id("status-indicator")
            .w(px(STATUS_INDICATOR_SIZE))
            .h(px(STATUS_INDICATOR_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(dot_color));

        let action_control = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .w(px(ACTION_CONTROL_WIDTH))
            .h(px(ACTION_CONTROL_HEIGHT))
            .gap(px(10.0))
            .child(matching)
            .child(status_dot);

        div()
            .id("gainsnap-editor")
            .w(px(WINDOW_WIDTH as f32))
            .h(px(WINDOW_HEIGHT as f32))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(SURFACE_COLUMN_GAP))
            .px(px(SURFACE_PADDING_X))
            .py(px(SURFACE_PADDING_Y))
            .bg(solid(BG_PRIMARY))
            .text_color(solid(TEXT_PRIMARY))
            .font(font("Ioskeley Mono"))
            .text_size(px(13.0))
            .line_height(px(14.0))
            .track_focus(&self.meter_focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .child(target_control)
            .child(action_control)
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

fn button_element(
    id: &'static str,
    label: &'static str,
    active: bool,
    width: f32,
    height: f32,
    focus_handle: &FocusHandle,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let focus_handle_for_mouse = focus_handle.clone();
    let (background, foreground) = if active {
        // Radiant keeps the active label on the primary text color while the
        // coral fill pulses underneath it.
        (solid(ACCENT), solid(TEXT_PRIMARY))
    } else {
        // MATCH is an accent action even while idle; PEAK/RMS stays muted.
        (
            solid(BG_PRIMARY),
            solid(if id == "match-now" {
                ACCENT
            } else {
                TEXT_MUTED
            }),
        )
    };
    div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex()
        .items_center()
        .justify_start()
        .px(px(8.0))
        .border_1()
        .border_color(if active { solid(ACCENT) } else { solid(BORDER) })
        .bg(background)
        .text_color(foreground)
        .font(font("Ioskeley Mono"))
        .text_size(px(13.0))
        .line_height(px(14.0))
        .role(gpui::Role::Button)
        .track_focus(focus_handle)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus_handle_for_mouse, cx);
        })
        .on_click(listener)
        .child(label)
}

fn match_status_color(state: MatchState, active: bool, pulse_alpha: u8) -> gpui::Rgba {
    if active {
        return rgba(ACCENT << 8 | u32::from(pulse_alpha));
    }
    match state {
        MatchState::Ready => solid(BORDER_EMPHASIS),
        MatchState::Measuring => solid(ACCENT),
        MatchState::Locked => solid(HIGHLIGHT_CYAN),
        MatchState::NoSignal => solid(ACCENT_DANGER),
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
    let width = f32::from(bounds.size.width);
    let track_width = TARGET_METER_TRACK_WIDTH.min(width);
    let x = f32::from(bounds.left()) + (width - track_width) * 0.5;
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
    let half = marker_height * 0.5;
    let top_center_y = f32::from(track.top()) + half;
    let bottom_center_y = f32::from(track.bottom()) - half;
    Some(MarkerGeometry {
        track,
        top_center_y,
        bottom_center_y,
        travel: (bottom_center_y - top_center_y).max(0.0),
        marker_height,
    })
}

fn target_level_fraction(db: f32) -> f32 {
    if db.is_finite() {
        TARGET_RANGE.normalize(db)
    } else {
        0.0
    }
}

fn paint_meter(
    bounds: Bounds<Pixels>,
    target_db: f32,
    output_peak_db: f32,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(geometry) = target_marker_geometry(bounds) else {
        return;
    };
    let track = geometry.track;
    window.paint_quad(fill(track, solid(BG_PRIMARY)));
    let level = target_level_fraction(output_peak_db);
    if level > 0.0 {
        let level_height = f32::from(track.size.height) * level;
        window.paint_quad(fill(
            Bounds::from_corners(
                point(track.left(), track.bottom() - px(level_height)),
                track.bottom_right(),
            ),
            solid(ACCENT),
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
        let x = f32::from(track.left()) - TARGET_MARKER_GAP - tick_width;
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
        let x = f32::from(track.left()) - TARGET_METER_LABEL_GAP - TARGET_METER_LABEL_WIDTH;
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

    let fraction = target_level_fraction(target_db);
    let center_y = if fraction >= 1.0 {
        geometry.top_center_y
    } else if fraction <= 0.0 {
        geometry.bottom_center_y
    } else {
        geometry.bottom_center_y - fraction * geometry.travel
    };
    let marker_left = f32::from(track.right()) + TARGET_MARKER_GAP;
    let mut marker = gpui::PathBuilder::fill();
    marker.move_to(point(px(marker_left), px(center_y)));
    marker.line_to(point(
        px(marker_left + TARGET_MARKER_WIDTH),
        px(center_y - geometry.marker_height * 0.5),
    ));
    marker.line_to(point(
        px(marker_left + TARGET_MARKER_WIDTH),
        px(center_y + geometry.marker_height * 0.5),
    ));
    marker.close();
    if let Ok(marker) = marker.build() {
        window.paint_path(marker, rgba(0xd8d7d3ff));
    }
    if focused {
        let mut emphasis = gpui::PathBuilder::stroke(px(1.0));
        emphasis.move_to(point(px(marker_left), px(center_y)));
        emphasis.line_to(point(
            px(marker_left + TARGET_MARKER_WIDTH),
            px(center_y - geometry.marker_height * 0.5),
        ));
        emphasis.line_to(point(
            px(marker_left + TARGET_MARKER_WIDTH),
            px(center_y + geometry.marker_height * 0.5),
        ));
        emphasis.close();
        if let Ok(emphasis) = emphasis.build() {
            window.paint_path(emphasis, rgba(0xe95843ff));
        }
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
        status,
        None,
        None,
    );
    (gui, params)
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
    )
}

fn new_hosted_gui(
    class_name: &'static str,
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
    edit_sink: Option<Arc<dyn HostParamEditSink>>,
) -> toybox::gpui_gui::GpuiHostedGui {
    let controller = EditorController::new(
        Arc::clone(&params),
        Arc::clone(&automation_queue),
        Arc::clone(&status),
        param_requester,
        edit_sink,
    );
    let factory_controller = controller.clone();
    let visibility_controller = controller;
    toybox::gpui_gui::GpuiHostedGui::new(
        class_name,
        move |_window, cx| {
            install_keybindings(cx);
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
    fn marked_ime_selection_is_relative_to_inserted_text() {
        assert_eq!(marked_selection_range(7, "é😀a", Some(&(1..3))), 9..13);
        assert_eq!(marked_selection_range(7, "é😀a", Some(&(3..3))), 13..13);
        assert_eq!(marked_selection_range(4, "a", Some(&(0..99))), 4..5);
        assert_eq!(marked_selection_range(4, "", Some(&(0..0))), 4..4);
    }

    #[test]
    fn utf16_offsets_preserve_non_ascii_boundaries() {
        let text = "é😀a";
        assert_eq!(offset_from_utf16(text, 0), 0);
        assert_eq!(offset_from_utf16(text, 1), 2);
        assert_eq!(offset_from_utf16(text, 2), 2);
        assert_eq!(offset_from_utf16(text, 3), 6);
        assert_eq!(offset_from_utf16(text, 4), 7);
        assert_eq!(offset_from_utf16(text, 5), 7);
        assert_eq!(offset_from_utf16(text, 99), text.len());
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
    fn normalize_forces_peak_zero_db_and_matching() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_RMS_MODE, 1.0);
        let mut controller = EditorController::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );

        assert_eq!(controller.normalize(), "0.0");
        assert!(!params.rms_mode());
        assert_eq!(params.target_db(), TARGET_MAX_DB);
        assert!(params.match_requested());
    }
}
