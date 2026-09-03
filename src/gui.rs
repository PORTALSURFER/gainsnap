//! Radiant editor for the macOS CLAP and VST3 GainSnap views.
//!
//! The editor owns only retained UI state. Audio-thread telemetry is published
//! through atomics, and parameter gestures use the CLAP queue or the VST3
//! component-handler sink supplied by the respective host adapter.

use std::sync::Arc;

use radiant::gui::automation::AutomationRole;
use radiant::gui::types::{Point, Rect, Vector2};
use radiant::layout::CrossAlign;
use radiant::prelude::{
    column, custom_widget_mapped, row, text, text_input, toggle, IntoView, Widget, WidgetCommon,
    WidgetInput, WidgetKey, WidgetOutput, WidgetSizing,
};
use radiant::runtime::{DeclarativeSurfaceRuntime, Event, SurfacePaintPlan, UiSurface};
use radiant::runtime::{PaintFillRect, PaintPrimitive, PaintStrokeRect};
use radiant::theme::ThemeTokens;
use radiant::widgets::{
    FocusBehavior, PaintBounds, PointerButton, SliderMessage, TextInputMessage, WidgetCapabilities,
    WidgetSemantics,
};
use toybox::clack_plugin::utils::ClapId;
use toybox::clap::automation::{AutomationConfig, AutomationQueue};

use crate::clap_plugin::HostParamRequester;
use crate::params::{PARAM_MATCH, PARAM_TARGET_DB, TARGET_MAX_DB, TARGET_MIN_DB};
use crate::status::{GuiStatus, MatchState};

/// Preferred logical editor width for the compact toggle control surface.
pub const WINDOW_WIDTH: u32 = 300;
/// Preferred logical editor height for the compact toggle control surface.
pub const WINDOW_HEIGHT: u32 = 320;
/// Minimum logical editor width.
pub const MIN_WINDOW_WIDTH: u32 = 300;
/// Minimum logical editor height.
pub const MIN_WINDOW_HEIGHT: u32 = 320;
/// Maximum logical editor width.
pub const MAX_WINDOW_WIDTH: u32 = 480;
/// Maximum logical editor height.
pub const MAX_WINDOW_HEIGHT: u32 = 520;

const TARGET_TEXT_SYNC_EPSILON: f32 = 0.0001;
const VERTICAL_SLIDER_TRACK_WIDTH: f32 = 8.0;
const VERTICAL_SLIDER_KEYBOARD_STEP_DB: f32 = 1.0;
const VERTICAL_SLIDER_FINE_KEYBOARD_STEP_DB: f32 = 0.1;

/// Format-neutral host automation sink used by the VST3 editor.
pub(crate) trait HostParamEditSink: Send + Sync {
    /// Begin a host gesture for a parameter.
    fn gesture_started(&self, config: &AutomationConfig, param_id: ClapId);
    /// Send one plain parameter value during a host gesture.
    fn gesture_value(&self, config: &AutomationConfig, param_id: ClapId, value: f64);
    /// End a host gesture for a parameter.
    fn gesture_ended(&self, config: &AutomationConfig, param_id: ClapId);
}

/// Compact vertical target control used because the pinned Radiant slider is
/// intentionally horizontal. It keeps the built-in slider's normalized value,
/// pointer, keyboard, focus, and automation behavior while painting a narrow
/// vertical bar for the small GainSnap editor.
#[derive(Clone, Debug, PartialEq)]
struct VerticalSlider {
    common: WidgetCommon,
    value: f32,
    shift_held: bool,
}

impl VerticalSlider {
    fn new(value: f32) -> Self {
        let mut common = WidgetCommon::new(0, WidgetSizing::fixed(Vector2::new(28.0, 140.0)));
        common.focus = FocusBehavior::Keyboard;
        common.paint.bounds = PaintBounds::ClipToRect;
        Self {
            common,
            value: clamp_fraction(value),
            shift_held: false,
        }
    }

    fn with_shift_held(mut self, shift_held: bool) -> Self {
        self.shift_held = shift_held;
        self
    }

    fn value_for_position(bounds: Rect, position: Point) -> f32 {
        if !bounds.has_finite_positive_area() {
            return 0.0;
        }
        clamp_fraction(1.0 - (position.y - bounds.min.y) / bounds.height())
    }

    fn set_value(&mut self, value: f32) -> Option<SliderMessage> {
        let value = clamp_fraction(value);
        if (self.value - value).abs() <= f32::EPSILON {
            return None;
        }
        self.value = value;
        Some(SliderMessage::ValueChanged { value })
    }

    fn keyboard_step_db(&self) -> f32 {
        if self.shift_held {
            VERTICAL_SLIDER_FINE_KEYBOARD_STEP_DB
        } else {
            VERTICAL_SLIDER_KEYBOARD_STEP_DB
        }
    }

    fn step_value(&mut self, direction: f32) -> Option<SliderMessage> {
        let target_db = TARGET_RANGE.denormalize(self.value);
        let next_target_db = (target_db + direction * self.keyboard_step_db())
            .clamp(TARGET_RANGE.min, TARGET_RANGE.max);
        self.set_value(TARGET_RANGE.normalize(next_target_db))
    }

    fn pointer_value(&mut self, bounds: Rect, position: Point) -> Option<WidgetOutput> {
        self.set_value(Self::value_for_position(bounds, position))
            .map(WidgetOutput::typed)
    }
}

impl WidgetSemantics for VerticalSlider {
    fn automation_role(&self) -> AutomationRole {
        AutomationRole::Slider
    }

    fn automation_label(&self) -> Option<String> {
        Some(String::from("Target Peak"))
    }

    fn automation_description(&self) -> Option<String> {
        Some(String::from("Target peak in dBFS"))
    }

    fn automation_value_text(&self) -> Option<String> {
        Some(format!("{:.1} dBFS", TARGET_RANGE.denormalize(self.value)))
    }
}

impl Widget for VerticalSlider {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        if self.common.state.disabled || self.common.state.read_only {
            return None;
        }
        match input {
            WidgetInput::PointerMove { position } => {
                self.common.state.hovered = bounds.contains(position);
                self.common
                    .state
                    .pressed
                    .then(|| self.pointer_value(bounds, position))
                    .flatten()
            }
            WidgetInput::PointerPress {
                position,
                button: PointerButton::Primary,
                ..
            } if bounds.contains(position) => {
                self.common.state.hovered = true;
                self.common.state.pressed = true;
                self.common.state.focused = true;
                self.pointer_value(bounds, position)
            }
            WidgetInput::PointerRelease {
                position,
                button: PointerButton::Primary,
                ..
            }
            | WidgetInput::PointerDrop {
                position,
                button: PointerButton::Primary,
                ..
            } => {
                let was_pressed = self.common.state.pressed;
                self.common.state.pressed = false;
                was_pressed
                    .then(|| self.pointer_value(bounds, position))
                    .flatten()
            }
            WidgetInput::FocusChanged(focused) => {
                self.common.state.focused = focused;
                None
            }
            WidgetInput::PointerModifiersChanged { modifiers } => {
                self.shift_held = modifiers.shift;
                None
            }
            WidgetInput::KeyPress(key) if self.common.state.focused => match key {
                WidgetKey::ArrowUp | WidgetKey::ArrowRight => {
                    self.step_value(1.0).map(WidgetOutput::typed)
                }
                WidgetKey::ArrowDown | WidgetKey::ArrowLeft => {
                    self.step_value(-1.0).map(WidgetOutput::typed)
                }
                WidgetKey::Home => self.set_value(0.0).map(WidgetOutput::typed),
                WidgetKey::End => self.set_value(1.0).map(WidgetOutput::typed),
                _ => None,
            },
            _ => None,
        }
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.common.state = previous.common.state;
        }
    }

    fn capabilities(&self) -> WidgetCapabilities<'_> {
        WidgetCapabilities::new().semantics(self)
    }

    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        _layout: &radiant::layout::LayoutOutput,
        theme: &ThemeTokens,
    ) {
        if !bounds.has_finite_positive_area() {
            return;
        }
        let track_width = VERTICAL_SLIDER_TRACK_WIDTH.min(bounds.width());
        let track_x = bounds.min.x + (bounds.width() - track_width) * 0.5;
        let track = Rect::from_min_max(
            Point::new(track_x, bounds.min.y),
            Point::new(track_x + track_width, bounds.max.y),
        );
        let tokens = radiant::widgets::resolve_widget_visual_tokens(
            theme,
            self.common.style,
            self.common.state,
        );
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: track,
            color: theme.bg_tertiary,
        }));
        let fill_height = track.height() * self.value.clamp(0.0, 1.0);
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: Rect::from_min_max(
                Point::new(track.min.x, track.max.y - fill_height),
                track.max,
            ),
            color: tokens.emphasis,
        }));
        let thumb_height = 4.0_f32.min(track.height());
        let thumb_y = track.max.y
            - self.value.clamp(0.0, 1.0) * (track.height() - thumb_height)
            - thumb_height;
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: Rect::from_min_max(
                Point::new(track.min.x - 4.0, thumb_y),
                Point::new(track.max.x + 4.0, thumb_y + thumb_height),
            ),
            color: theme.text_primary,
        }));
        primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
            widget_id: self.common.id,
            rect: track,
            color: theme.border_emphasis,
            width: 1.0,
        }));
        if self.common.state.focused && self.common.paint.paints_focus {
            primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
                widget_id: self.common.id,
                rect: bounds,
                color: tokens.emphasis,
                width: 1.0,
            }));
        }
    }
}

fn clamp_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
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
struct DisplaySnapshot {
    target_db: u32,
    match_requested: bool,
    input_peak_db: u32,
    output_peak_db: u32,
    locked_gain_db: u32,
    progress: u32,
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
            match_requested: params.get_param(PARAM_MATCH).unwrap_or(0.0) >= 0.5,
            input_peak_db: status.input_peak_db().to_bits(),
            output_peak_db: status.output_peak_db().to_bits(),
            locked_gain_db: status.locked_gain_db().to_bits(),
            progress: status.progress().to_bits(),
            state: status.state(),
        }
    }
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

#[derive(Clone, Debug)]
enum EditorMessage {
    TargetChanged(f32),
    TargetTextChanged(TextInputMessage),
    Toggle { id: ClapId, checked: bool },
}

#[derive(Clone)]
struct EditorState {
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    automation_config: AutomationConfig,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
    edit_sink: Option<Arc<dyn HostParamEditSink>>,
    target_text: String,
    target_text_param: f32,
    shift_held: bool,
}

impl EditorState {
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
        Self {
            params,
            automation_queue,
            automation_config: AutomationConfig::default(),
            status,
            param_requester,
            edit_sink,
            target_text: format_target_text(target_db),
            target_text_param: target_db,
            shift_held: false,
        }
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

    fn sync_target_text(&mut self, target_db: f32) {
        if (target_db - self.target_text_param).abs() > TARGET_TEXT_SYNC_EPSILON {
            self.target_text = format_target_text(target_db);
            self.target_text_param = target_db;
        }
    }

    fn set_target_db(&mut self, value: f32) {
        self.begin(PARAM_TARGET_DB);
        self.value(PARAM_TARGET_DB, value);
        self.end(PARAM_TARGET_DB);
        self.target_text_param = self.parameter_value(PARAM_TARGET_DB, TARGET_RANGE);
    }
}

type EditorRuntime = DeclarativeSurfaceRuntime<
    EditorState,
    EditorMessage,
    fn(&mut EditorState) -> Arc<UiSurface<EditorMessage>>,
    fn(&mut EditorState, EditorMessage),
>;

/// Retained editor hosted by Toybox's macOS GUI bridge.
pub(crate) struct GainSnapEditor {
    runtime: EditorRuntime,
    theme: ThemeTokens,
    paint_plan: SurfacePaintPlan,
    viewport: Vector2,
    last_display_snapshot: DisplaySnapshot,
}

impl GainSnapEditor {
    pub(crate) fn new(
        params: Arc<crate::params::GainSnapParams>,
        automation_queue: Arc<AutomationQueue>,
        status: Arc<GuiStatus>,
        param_requester: Option<HostParamRequester>,
        edit_sink: Option<Arc<dyn HostParamEditSink>>,
    ) -> Self {
        let theme = ThemeTokens::default();
        let viewport = Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32);
        let last_display_snapshot = DisplaySnapshot::capture(&params, &status);
        Self {
            runtime: EditorRuntime::new_declarative(
                EditorState::new(params, automation_queue, status, param_requester, edit_sink),
                viewport,
                project_surface,
                reduce_message,
            ),
            paint_plan: SurfacePaintPlan::empty(&theme),
            theme,
            viewport,
            last_display_snapshot,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.viewport = Vector2::new(width.max(1) as f32, height.max(1) as f32);
        self.runtime.dispatch_event(Event::resize(self.viewport));
    }

    fn dispatch_event(&mut self, event: Event) {
        let shift_held = match event {
            Event::PointerModifiersChanged { modifiers } => Some(modifiers.shift),
            _ => None,
        };
        let _ = self.runtime.dispatch_event(event);
        if let Some(shift_held) = shift_held {
            let shift_changed = self.runtime.bridge().state().shift_held != shift_held;
            if shift_changed {
                self.runtime.bridge_mut().state_mut().shift_held = shift_held;
                self.runtime.refresh();
            }
        }
    }

    fn paint_plan(&mut self) -> &SurfacePaintPlan {
        let display_snapshot = self.display_snapshot();
        if display_snapshot != self.last_display_snapshot {
            self.last_display_snapshot = display_snapshot;
            self.runtime.refresh();
        }
        let _ = self
            .runtime
            .borrowed_frame_into(&self.theme, &mut self.paint_plan);
        &self.paint_plan
    }

    fn display_snapshot(&self) -> DisplaySnapshot {
        let state = self.runtime.bridge().state();
        DisplaySnapshot::capture(&state.params, &state.status)
    }

    fn needs_realtime_redraw(&self) -> bool {
        self.display_snapshot() != self.last_display_snapshot
    }

    fn dispatch_key_press(&mut self, key: WidgetKey) -> bool {
        self.runtime.dispatch_event(Event::key_press(key)).is_some()
    }

    fn dispatch_character(&mut self, character: char) -> bool {
        self.runtime
            .dispatch_event(Event::character(character))
            .is_some()
    }
}

impl toybox::radiant_gui::RadiantEditor for GainSnapEditor {
    fn resize(&mut self, width: u32, height: u32) {
        Self::resize(self, width, height);
    }

    fn dispatch_event(&mut self, event: Event) {
        Self::dispatch_event(self, event);
    }

    fn paint_plan(&mut self) -> &SurfacePaintPlan {
        Self::paint_plan(self)
    }

    fn needs_realtime_redraw(&self) -> bool {
        Self::needs_realtime_redraw(self)
    }

    fn dispatch_key_press(&mut self, key: WidgetKey) -> bool {
        Self::dispatch_key_press(self, key)
    }

    fn dispatch_character(&mut self, character: char) -> bool {
        Self::dispatch_character(self, character)
    }

    fn cancel_text_entry(&mut self) -> bool {
        false
    }
}

/// Construct the CLAP-hosted GainSnap editor.
pub(crate) fn new_gui(
    params: Arc<crate::params::GainSnapParams>,
    automation_queue: Arc<AutomationQueue>,
    status: Arc<GuiStatus>,
    param_requester: Option<HostParamRequester>,
) -> toybox::radiant_gui::RadiantHostedGui {
    toybox::radiant_gui::RadiantHostedGui::new(
        "GainSnapRadiantClapEditorView",
        GainSnapEditor::new(params, automation_queue, status, param_requester, None),
        WINDOW_WIDTH,
        WINDOW_HEIGHT,
    )
    .with_size_contract(
        (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT),
        (WINDOW_WIDTH, WINDOW_HEIGHT),
        (MAX_WINDOW_WIDTH, MAX_WINDOW_HEIGHT),
    )
}

#[allow(clippy::arc_with_non_send_sync)]
fn project_surface(state: &mut EditorState) -> Arc<UiSurface<EditorMessage>> {
    let match_state = state.status.state();
    let status_line = match match_state {
        MatchState::Ready => "Ready — target".to_string(),
        MatchState::Measuring => "Measuring… toggle off to lock".to_string(),
        MatchState::Locked => format!("Locked {:+.2} dB", state.status.locked_gain_db()),
        MatchState::NoSignal => "No signal — retry".to_string(),
    };
    let target_db = state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE);
    state.sync_target_text(target_db);
    let target = custom_widget_mapped(
        VerticalSlider::new(TARGET_RANGE.normalize(target_db)).with_shift_held(state.shift_held),
        |message: SliderMessage| match message {
            SliderMessage::ValueChanged { value } => {
                EditorMessage::TargetChanged(TARGET_RANGE.denormalize(value))
            }
        },
    )
    .primary()
    .key("target-peak")
    .width(34.0)
    .height(140.0);
    let target_entry = text_input(state.target_text.clone())
        .placeholder("-12.0")
        .message_event(EditorMessage::TargetTextChanged)
        .key("target-entry")
        .width(84.0)
        .height(30.0);
    let matching = column([toggle(
        "MATCH",
        state.params.get_param(PARAM_MATCH).unwrap_or(0.0) >= 0.5,
    )
    .primary()
    .message(|checked| EditorMessage::Toggle {
        id: PARAM_MATCH,
        checked,
    })
    .key("match-now")
    .width(96.0)
    .height(30.0)])
    .width(156.0)
    .height(30.0)
    .align_cross(CrossAlign::Start);
    let target_control = column([
        text("TARGET PEAK").key("target-label").height(18.0),
        target,
        target_entry,
        text("dBFS").key("target-units").height(16.0),
    ])
    .width(100.0)
    .spacing(4.0);
    let telemetry = column([
        text(format!("IN  {:.1} dBFS", state.status.input_peak_db()))
            .key("input-readout")
            .height(18.0),
        text(format!("OUT {:.1} dBFS", state.status.output_peak_db()))
            .key("output-readout")
            .height(18.0),
        text(format!("GAIN {:+.2} dB", state.status.locked_gain_db()))
            .key("gain-readout")
            .height(18.0),
    ])
    .height(58.0)
    .spacing(2.0);
    let action_control = column([
        matching,
        text(status_line).key("status").height(44.0),
        telemetry,
    ])
    .width(156.0)
    .spacing(8.0);
    let view = column([
        text("GAIN SNAP").key("title").height(24.0),
        text("TOGGLE PEAK MATCH").key("subtitle").height(18.0),
        row([target_control, action_control])
            .height(216.0)
            .spacing(12.0),
        text("Target • Match • Lock").key("footer").height(20.0),
    ])
    .padding(12.0)
    .spacing(6.0)
    .fill_width()
    .fill_height();
    Arc::new(view.into_surface())
}

fn reduce_message(state: &mut EditorState, message: EditorMessage) {
    match message {
        EditorMessage::TargetChanged(value) => {
            state.set_target_db(value);
            state.target_text = format_target_text(state.target_text_param);
        }
        EditorMessage::TargetTextChanged(message) => {
            let submitted = matches!(
                &message,
                TextInputMessage::Submitted { .. } | TextInputMessage::CompletionRequested { .. }
            );
            state.target_text = message.into_value();
            if let Some(value) = parse_target_text(&state.target_text) {
                state.set_target_db(value);
                if submitted {
                    state.target_text = format_target_text(state.target_text_param);
                }
            } else if submitted {
                state.target_text = format_target_text(state.target_text_param);
            }
        }
        EditorMessage::Toggle { id, checked } => state.toggle_value(id, checked),
    }
}

/// Return the initial size used by host GUI negotiation.
pub(crate) fn preferred_window_size() -> (u32, u32) {
    (WINDOW_WIDTH, WINDOW_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiant::widgets::PointerModifiers;

    fn editor_state() -> EditorState {
        EditorState::new(
            Arc::new(crate::params::GainSnapParams::new()),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        )
    }

    #[test]
    fn vertical_slider_maps_pointer_and_keyboard_input() {
        let bounds = Rect::from_size(28.0, 140.0);
        let mut slider = VerticalSlider::new(TARGET_RANGE.normalize(-12.0));

        let output = slider
            .handle_input(bounds, WidgetInput::primary_press(Point::new(14.0, 0.0)))
            .expect("pressing the slider should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged { value: 1.0 })
        );

        let output = slider
            .handle_input(
                bounds,
                WidgetInput::PointerMove {
                    position: Point::new(14.0, 140.0),
                },
            )
            .expect("dragging the slider should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged { value: 0.0 })
        );

        slider.set_value(1.0);
        let output = slider
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowDown))
            .expect("focused slider keyboard input should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged {
                value: TARGET_RANGE.normalize(-1.1),
            })
        );
    }

    #[test]
    fn vertical_slider_keyboard_steps_use_db_units_and_shift_fine_step() {
        let bounds = Rect::from_size(28.0, 140.0);
        let mut slider = VerticalSlider::new(TARGET_RANGE.normalize(-12.0));
        slider.common.state.focused = true;

        slider
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowUp))
            .expect("normal arrow input should emit a value");
        let normal_target = TARGET_RANGE.denormalize(slider.value);
        assert!((normal_target - (-11.0)).abs() < 0.0001);

        slider.handle_input(
            bounds,
            WidgetInput::PointerModifiersChanged {
                modifiers: PointerModifiers {
                    shift: true,
                    ..PointerModifiers::default()
                },
            },
        );
        slider
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowUp))
            .expect("shift arrow input should emit a value");
        let fine_target = TARGET_RANGE.denormalize(slider.value);
        assert!((fine_target - (-10.9)).abs() < 0.0001);

        slider.handle_input(
            bounds,
            WidgetInput::PointerModifiersChanged {
                modifiers: PointerModifiers::default(),
            },
        );
        slider
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowDown))
            .expect("normal arrow input after Shift should emit a value");
        let normal_down_target = TARGET_RANGE.denormalize(slider.value);
        assert!((normal_down_target - (-11.9)).abs() < 0.0001);
    }

    #[test]
    fn vertical_slider_keeps_projected_shift_modifier_when_rebuilt() {
        let bounds = Rect::from_size(28.0, 140.0);
        let previous = VerticalSlider::new(TARGET_RANGE.normalize(-12.0));
        let mut current = VerticalSlider::new(previous.value).with_shift_held(true);

        current.synchronize_from_previous(&previous);
        current.common.state.focused = true;
        current
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowUp))
            .expect("rebuilt slider should accept keyboard input");

        let target_db = TARGET_RANGE.denormalize(current.value);
        assert!((target_db - (-11.9)).abs() < 0.0001);
    }

    #[test]
    fn editor_captures_shift_before_a_focused_key_event() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let mut editor = GainSnapEditor::new(
            params,
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );

        editor.dispatch_event(Event::pointer_modifiers_changed(PointerModifiers {
            shift: true,
            ..PointerModifiers::default()
        }));
        assert!(editor.runtime.bridge().state().shift_held);

        editor.dispatch_event(Event::pointer_modifiers_changed(PointerModifiers::default()));
        assert!(!editor.runtime.bridge().state().shift_held);
    }

    #[test]
    fn typed_target_edit_updates_the_slider_parameter_and_rejects_invalid_submit() {
        let mut state = editor_state();

        reduce_message(
            &mut state,
            EditorMessage::TargetTextChanged(TextInputMessage::Changed {
                value: String::from("-6.5"),
            }),
        );
        assert_eq!(state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE), -6.5);
        assert_eq!(state.target_text, "-6.5");

        reduce_message(
            &mut state,
            EditorMessage::TargetTextChanged(TextInputMessage::Changed {
                value: String::from("not-a-number"),
            }),
        );
        assert_eq!(state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE), -6.5);

        reduce_message(
            &mut state,
            EditorMessage::TargetTextChanged(TextInputMessage::Submitted {
                value: String::from("not-a-number"),
            }),
        );
        assert_eq!(state.target_text, "-6.5");
        assert_eq!(state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE), -6.5);
    }

    #[test]
    fn compact_surface_keeps_target_entry_and_match_label_visible() {
        let mut state = editor_state();
        let plan = project_surface(&mut state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &ThemeTokens::default(),
            )
            .paint_plan;

        assert_eq!(preferred_window_size(), (300, 320));
        assert!(plan.contains_text("TARGET PEAK"));
        assert!(plan.contains_text("MATCH"));
        assert!(plan.contains_text("dBFS"));
        assert!(plan.contains_text_input());
    }

    #[test]
    fn compact_surface_keeps_match_button_at_requested_size() {
        let mut state = editor_state();
        let plan = project_surface(&mut state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &ThemeTokens::default(),
            )
            .paint_plan;
        let match_widget_id = plan
            .first_text_run("MATCH")
            .expect("match label should be painted")
            .widget_id;
        let match_bounds = plan
            .first_widget_rect(match_widget_id)
            .expect("match button should paint a rectangular control");

        assert!((match_bounds.width() - 96.0).abs() < f32::EPSILON);
        assert!((match_bounds.height() - 30.0).abs() < f32::EPSILON);
    }

    #[test]
    fn editor_repaints_when_measurement_locks() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::clone(&status),
            None,
            None,
        );

        status.update(-12.0, -12.0, 0.0, 0.5, MatchState::Measuring);
        assert!(editor.needs_realtime_redraw());
        assert!(editor
            .paint_plan()
            .text_labels()
            .any(|label| label.starts_with("Measuring…")));
        assert!(!editor.needs_realtime_redraw());

        status.update(-6.0, -3.0, 3.0, 1.0, MatchState::Locked);
        assert!(editor.needs_realtime_redraw());
        assert!(editor
            .paint_plan()
            .contains_text_after_x("Locked +3.00 dB", 0.0));
        assert!(!editor.needs_realtime_redraw());
    }

    #[test]
    fn editor_repaints_when_host_changes_target_parameter() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            status,
            None,
            None,
        );

        assert!(!editor.needs_realtime_redraw());
        params.set_param(PARAM_TARGET_DB, -6.5);
        assert!(editor.needs_realtime_redraw());
        editor.paint_plan();
        assert!(!editor.needs_realtime_redraw());
    }
}

#[cfg(all(test, feature = "screenshot-test"))]
mod screenshot_tests {
    use super::*;
    use image::{ColorType, ImageFormat};
    use radiant::theme::DpiScale;
    use std::path::PathBuf;

    #[test]
    fn screenshot_renders_initial_ui() {
        let mut state = EditorState::new(
            Arc::new(crate::params::GainSnapParams::new()),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );
        let plan = project_surface(&mut state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &ThemeTokens::default(),
            )
            .paint_plan;
        let mut capture = radiant::gui_runtime::OffscreenVelloCapture::new(
            Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
            DpiScale::ONE,
        )
        .expect("Radiant offscreen capture should be available");
        let pixels = capture.capture(&plan).expect("screenshot should render");
        let root = std::env::var_os("TOYBOX_UI_SCREENSHOT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/ui-screenshots"))
            .join("gainsnap");
        std::fs::create_dir_all(&root).expect("screenshot directory should be writable");
        image::save_buffer_with_format(
            root.join("initial-ui-300x320.png"),
            &pixels,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            ColorType::Rgba8,
            ImageFormat::Png,
        )
        .expect("screenshot should be written");
    }
}
