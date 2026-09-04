//! Radiant editor for the macOS CLAP and VST3 GainSnap views.
//!
//! The editor owns only retained UI state. Audio-thread telemetry is published
//! through atomics, and parameter gestures use the CLAP queue or the VST3
//! component-handler sink supplied by the respective host adapter.

use std::sync::Arc;

use radiant::gui::automation::AutomationRole;
use radiant::gui::types::{Point, Rect, Rgba8, Vector2};
use radiant::layout::{CrossAlign, MainAlign};
use radiant::prelude::{
    column, custom_widget, custom_widget_mapped, row, text, text_input, toggle, IntoView, Widget,
    WidgetCommon, WidgetInput, WidgetKey, WidgetOutput, WidgetSizing,
};
use radiant::runtime::{DeclarativeSurfaceRuntime, Event, SurfacePaintPlan, UiSurface};
use radiant::runtime::{PaintFillPolygon, PaintFillRect, PaintPrimitive, PaintStrokeRect};
use radiant::theme::ThemeTokens;
use radiant::widgets::{
    ButtonMessage, ButtonWidget, FocusBehavior, PaintBounds, PointerButton, SliderMessage,
    TextInputMessage, WidgetCapabilities, WidgetId, WidgetSemantics,
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
// Compact text inputs reserve horizontal insets for their chrome and focused
// caret. Keep enough room for the longest formatted target (for example,
// `-12.0`) to remain visible while it is being edited.
const TARGET_CONTROL_WIDTH: f32 = 68.0;
const TARGET_SLIDER_HEIGHT: f32 = 176.0;
const TARGET_ENTRY_HEIGHT: f32 = 28.0;
const TARGET_CONTROL_SPACING: f32 = 8.0;
const ACTION_CONTROL_WIDTH: f32 = 112.0;
const ACTION_CONTROL_HEIGHT: f32 = 212.0;
const MATCHING_CONTROL_HEIGHT: f32 = 64.0;
const MATCH_BUTTON_WIDTH: f32 = 108.0;
const MATCH_BUTTON_HEIGHT: f32 = 38.0;
const NORMALIZE_BUTTON_WIDTH: f32 = 40.0;
const NORMALIZE_BUTTON_HEIGHT: f32 = 22.0;
const STATUS_INDICATOR_SIZE: f32 = 12.0;
const VERTICAL_SLIDER_TRACK_WIDTH: f32 = 6.0;
const VERTICAL_SLIDER_THUMB_HEIGHT: f32 = 6.0;
const VERTICAL_SLIDER_MARKER_GAP: f32 = 2.0;
const VERTICAL_SLIDER_MARKER_WIDTH: f32 = 8.0;
const VERTICAL_SLIDER_MARKER_HEIGHT: f32 = 3.0;
const VERTICAL_SLIDER_TICK_COUNT: usize = 13;
const VERTICAL_SLIDER_TICK_WIDTH: f32 = 4.0;
const VERTICAL_SLIDER_TICK_HEIGHT: f32 = 1.0;
const VERTICAL_SLIDER_KEYBOARD_STEP_DB: f32 = 1.0;
const VERTICAL_SLIDER_FINE_KEYBOARD_STEP_DB: f32 = 0.1;
const SURFACE_PADDING_X: f32 = 32.0;
const SURFACE_COLUMN_GAP: f32 = 32.0;

const TARGET_ENTRY_AUTOMATION_LABEL: &str = "Target peak, dBFS";
const NORMALIZE_AUTOMATION_LABEL: &str = "Normalize";
const NORMALIZE_AUTOMATION_DESCRIPTION: &str = "Normalize to 0 dBFS and start Match";

/// Format-neutral host automation sink used by the VST3 editor.
pub(crate) trait HostParamEditSink: Send + Sync {
    /// Begin a host gesture for a parameter.
    fn gesture_started(&self, config: &AutomationConfig, param_id: ClapId);
    /// Send one plain parameter value during a host gesture.
    fn gesture_value(&self, config: &AutomationConfig, param_id: ClapId, value: f64);
    /// End a host gesture for a parameter.
    fn gesture_ended(&self, config: &AutomationConfig, param_id: ClapId);
}

/// Small passive state indicator kept visible without reintroducing a verbose
/// status readout into the compact editor.
#[derive(Clone, Debug, PartialEq)]
struct StatusIndicator {
    common: WidgetCommon,
    state: MatchState,
}

impl StatusIndicator {
    fn new(state: MatchState) -> Self {
        let mut common = WidgetCommon::new(
            0,
            WidgetSizing::fixed(Vector2::new(STATUS_INDICATOR_SIZE, STATUS_INDICATOR_SIZE)),
        );
        common.focus = FocusBehavior::None;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self { common, state }
    }

    fn label(state: MatchState) -> &'static str {
        match state {
            MatchState::Ready => "Ready",
            MatchState::Measuring => "Measuring",
            MatchState::Locked => "Locked",
            MatchState::NoSignal => "No signal",
        }
    }

    fn color(&self, theme: &ThemeTokens) -> Rgba8 {
        match self.state {
            MatchState::Ready => theme.border_emphasis,
            MatchState::Measuring => theme.accent_mint,
            MatchState::Locked => theme.highlight_cyan,
            MatchState::NoSignal => theme.accent_danger,
        }
    }
}

impl WidgetSemantics for StatusIndicator {
    fn automation_role(&self) -> AutomationRole {
        AutomationRole::Readout
    }

    fn automation_label(&self) -> Option<String> {
        Some(String::from("Match status"))
    }

    fn automation_description(&self) -> Option<String> {
        Some(String::from("Current GainSnap matching status"))
    }

    fn automation_value_text(&self) -> Option<String> {
        Some(Self::label(self.state).to_owned())
    }
}

impl Widget for StatusIndicator {
    fn common(&self) -> &WidgetCommon {
        &self.common
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        &mut self.common
    }

    fn handle_input(&mut self, _bounds: Rect, _input: WidgetInput) -> Option<WidgetOutput> {
        None
    }

    fn needs_state_synchronization(&self) -> bool {
        false
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
        let diameter = bounds.width().min(bounds.height()).min(8.0);
        if diameter <= 0.0 {
            return;
        }
        let center = Point::new(
            bounds.min.x + bounds.width() * 0.5,
            bounds.min.y + bounds.height() * 0.5,
        );
        let radius = diameter * 0.5;
        let points = (0..16)
            .map(|index| {
                let angle = std::f32::consts::TAU * index as f32 / 16.0;
                Point::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                )
            })
            .collect::<Vec<_>>();
        primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
            widget_id: self.common.id,
            points: Arc::from(points),
            color: self.color(theme),
        }));
    }
}

/// Compact normalize action whose accessible name describes the full action
/// while retaining the short visible button label.
#[derive(Clone, Debug, PartialEq)]
struct NormalizeButtonWidget {
    button: ButtonWidget,
}

impl NormalizeButtonWidget {
    fn new() -> Self {
        Self {
            button: ButtonWidget::new(
                0,
                "0 dB",
                WidgetSizing::fixed(Vector2::new(
                    NORMALIZE_BUTTON_WIDTH,
                    NORMALIZE_BUTTON_HEIGHT,
                )),
            ),
        }
    }
}

impl WidgetSemantics for NormalizeButtonWidget {
    fn automation_role(&self) -> AutomationRole {
        AutomationRole::Button
    }

    fn automation_label(&self) -> Option<String> {
        Some(NORMALIZE_AUTOMATION_LABEL.to_owned())
    }

    fn automation_description(&self) -> Option<String> {
        Some(NORMALIZE_AUTOMATION_DESCRIPTION.to_owned())
    }
}

impl Widget for NormalizeButtonWidget {
    fn common(&self) -> &WidgetCommon {
        self.button.common()
    }

    fn common_mut(&mut self) -> &mut WidgetCommon {
        self.button.common_mut()
    }

    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        self.button
            .handle_input(bounds, input)
            .map(WidgetOutput::typed)
    }

    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.button.synchronize_from_previous(&previous.button);
    }

    fn accepts_pointer_move(&self) -> bool {
        self.button.accepts_pointer_move()
    }

    fn capabilities(&self) -> WidgetCapabilities<'_> {
        WidgetCapabilities::new().semantics(self)
    }

    fn set_text_align(&mut self, align: radiant::widgets::TextAlign) -> bool {
        self.button.set_text_align(align)
    }

    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        layout: &radiant::layout::LayoutOutput,
        theme: &ThemeTokens,
    ) {
        self.button.append_paint(primitives, bounds, layout, theme);
    }
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
    input_peak_db: f32,
    output_peak_db: f32,
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
            input_peak_db: -120.0,
            output_peak_db: -120.0,
        }
    }

    fn with_shift_held(mut self, shift_held: bool) -> Self {
        self.shift_held = shift_held;
        self
    }

    fn with_peak_levels(mut self, input_peak_db: f32, output_peak_db: f32) -> Self {
        self.input_peak_db = input_peak_db;
        self.output_peak_db = output_peak_db;
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
        let track = vertical_slider_track(bounds);
        let tokens = radiant::widgets::resolve_widget_visual_tokens(
            theme,
            self.common.style,
            self.common.state,
        );
        let frame = Rect::from_min_max(
            Point::new(bounds.min.x + 0.5, bounds.min.y + 0.5),
            Point::new(bounds.max.x - 0.5, bounds.max.y - 0.5),
        );
        primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
            widget_id: self.common.id,
            rect: frame,
            color: theme.border,
            width: 1.0,
        }));
        let tick_color = theme.grid_strong;
        for index in 0..VERTICAL_SLIDER_TICK_COUNT {
            let fraction = index as f32 / (VERTICAL_SLIDER_TICK_COUNT - 1) as f32;
            let y = bounds.min.y + (1.0 - fraction) * bounds.height();
            let tick_y = (y - VERTICAL_SLIDER_TICK_HEIGHT * 0.5)
                .clamp(bounds.min.y, bounds.max.y - VERTICAL_SLIDER_TICK_HEIGHT);
            let tick_width = if index % 2 == 0 {
                VERTICAL_SLIDER_TICK_WIDTH
            } else {
                VERTICAL_SLIDER_TICK_WIDTH - 1.0
            };
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: Rect::from_min_max(
                    Point::new(bounds.min.x + 5.0, tick_y),
                    Point::new(
                        bounds.min.x + 5.0 + tick_width,
                        tick_y + VERTICAL_SLIDER_TICK_HEIGHT,
                    ),
                ),
                color: tick_color,
            }));
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: Rect::from_min_max(
                    Point::new(bounds.max.x - 5.0 - tick_width, tick_y),
                    Point::new(bounds.max.x - 5.0, tick_y + VERTICAL_SLIDER_TICK_HEIGHT),
                ),
                color: tick_color,
            }));
        }
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: track,
            color: theme.bg_primary,
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
        push_peak_marker(
            primitives,
            self.common.id,
            bounds,
            track,
            self.input_peak_db,
            PeakMarkerSide::Left,
            theme.highlight_orange,
        );
        push_peak_marker(
            primitives,
            self.common.id,
            bounds,
            track,
            self.output_peak_db,
            PeakMarkerSide::Right,
            theme.highlight_cyan,
        );
        let thumb_height = VERTICAL_SLIDER_THUMB_HEIGHT.min(track.height());
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
            color: theme.grid_strong,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeakMarkerSide {
    Left,
    Right,
}

fn vertical_slider_track(bounds: Rect) -> Rect {
    let track_width = VERTICAL_SLIDER_TRACK_WIDTH.min(bounds.width());
    let track_x = bounds.min.x + (bounds.width() - track_width) * 0.5;
    Rect::from_min_max(
        Point::new(track_x, bounds.min.y),
        Point::new(track_x + track_width, bounds.max.y),
    )
}

fn target_level_fraction(db: f32) -> f32 {
    if db.is_finite() {
        TARGET_RANGE.normalize(db)
    } else {
        0.0
    }
}

fn marker_center_y(track: Rect, db: f32) -> f32 {
    let thumb_height = VERTICAL_SLIDER_THUMB_HEIGHT.min(track.height());
    let travel = (track.height() - thumb_height).max(0.0);
    track.max.y - (thumb_height * 0.5) - (target_level_fraction(db) * travel)
}

fn peak_marker_rect(bounds: Rect, track: Rect, db: f32, side: PeakMarkerSide) -> Option<Rect> {
    let marker_width = (((bounds.width() - track.width()) * 0.5) - VERTICAL_SLIDER_MARKER_GAP)
        .clamp(0.0, VERTICAL_SLIDER_MARKER_WIDTH);
    let marker_height = VERTICAL_SLIDER_MARKER_HEIGHT
        .min(bounds.height())
        .min(track.height());
    if marker_width <= 0.0 || marker_height <= 0.0 {
        return None;
    }

    let y = (marker_center_y(track, db) - marker_height * 0.5)
        .clamp(bounds.min.y, bounds.max.y - marker_height);
    let (min_x, max_x) = match side {
        PeakMarkerSide::Left => (
            track.min.x - VERTICAL_SLIDER_MARKER_GAP - marker_width,
            track.min.x - VERTICAL_SLIDER_MARKER_GAP,
        ),
        PeakMarkerSide::Right => (
            track.max.x + VERTICAL_SLIDER_MARKER_GAP,
            track.max.x + VERTICAL_SLIDER_MARKER_GAP + marker_width,
        ),
    };
    let rect = Rect::from_min_max(Point::new(min_x, y), Point::new(max_x, y + marker_height));
    rect.has_finite_positive_area().then_some(rect)
}

fn push_peak_marker(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: WidgetId,
    bounds: Rect,
    track: Rect,
    db: f32,
    side: PeakMarkerSide,
    color: Rgba8,
) {
    if let Some(rect) = peak_marker_rect(bounds, track, db, side) {
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id,
            rect,
            color,
        }));
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
    Normalize,
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

    fn normalize(&mut self) {
        self.set_target_db(TARGET_MAX_DB);
        self.target_text = format_target_text(self.target_text_param);
        self.toggle_value(PARAM_MATCH, true);
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
    let target_db = state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE);
    state.sync_target_text(target_db);
    let target = custom_widget_mapped(
        VerticalSlider::new(TARGET_RANGE.normalize(target_db))
            .with_shift_held(state.shift_held)
            .with_peak_levels(state.status.input_peak_db(), state.status.output_peak_db()),
        |message: SliderMessage| match message {
            SliderMessage::ValueChanged { value } => {
                EditorMessage::TargetChanged(TARGET_RANGE.denormalize(value))
            }
        },
    )
    .primary()
    .key("target-peak")
    .width(TARGET_CONTROL_WIDTH)
    .height(TARGET_SLIDER_HEIGHT);
    // Keep the framework text input as the leaf so its native editing lifecycle
    // remains intact; the compact placeholder supplies the semantic name.
    let target_entry = text_input(state.target_text.clone())
        .placeholder(TARGET_ENTRY_AUTOMATION_LABEL)
        .message_event(EditorMessage::TargetTextChanged)
        .key("target-entry")
        .subtle()
        .width(TARGET_CONTROL_WIDTH)
        .height(TARGET_ENTRY_HEIGHT);
    let matching = column([
        toggle(
            "MATCH",
            state.params.get_param(PARAM_MATCH).unwrap_or(0.0) >= 0.5,
        )
        .primary()
        .message(|checked| EditorMessage::Toggle {
            id: PARAM_MATCH,
            checked,
        })
        .key("match-now")
        .width(MATCH_BUTTON_WIDTH)
        .height(MATCH_BUTTON_HEIGHT),
        custom_widget_mapped(NormalizeButtonWidget::new(), |_message: ButtonMessage| {
            EditorMessage::Normalize
        })
        .subtle()
        .key("normalize")
        .size(NORMALIZE_BUTTON_WIDTH, NORMALIZE_BUTTON_HEIGHT)
        .tooltip("Set target to 0 dBFS and start Match"),
    ])
    .width(ACTION_CONTROL_WIDTH)
    .height(MATCHING_CONTROL_HEIGHT)
    .spacing(4.0)
    .align_main(MainAlign::Center)
    .align_cross(CrossAlign::End);
    let target_control = column([target, target_entry])
        .width(TARGET_CONTROL_WIDTH)
        .height(ACTION_CONTROL_HEIGHT)
        .spacing(TARGET_CONTROL_SPACING)
        .align_cross(CrossAlign::Center);
    let action_control = column([
        text("").height(32.0),
        matching,
        text("").height(10.0),
        custom_widget(StatusIndicator::new(match_state), |_output| None)
            .key("status-indicator")
            .size(STATUS_INDICATOR_SIZE, STATUS_INDICATOR_SIZE)
            .tooltip(StatusIndicator::label(match_state)),
    ])
    .width(ACTION_CONTROL_WIDTH)
    .height(ACTION_CONTROL_HEIGHT)
    .spacing(4.0)
    .align_main(MainAlign::Center)
    .align_cross(CrossAlign::End);
    let view = row([target_control, action_control])
        .height(ACTION_CONTROL_HEIGHT)
        .spacing(SURFACE_COLUMN_GAP)
        .padding_x(SURFACE_PADDING_X)
        .padding_y(24.0)
        .align_main(MainAlign::Start)
        .align_cross(CrossAlign::Center)
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
        EditorMessage::Normalize => state.normalize(),
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
                value: TARGET_RANGE.normalize(-1.0),
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
    fn peak_marker_mapping_uses_target_scale_bottom_up_and_clamps_silence() {
        let bounds = Rect::from_size(28.0, 140.0);
        let track = vertical_slider_track(bounds);

        assert_eq!(target_level_fraction(-120.0), 0.0);
        assert_eq!(target_level_fraction(TARGET_MIN_DB), 0.0);
        assert!((target_level_fraction(-18.0) - 0.5).abs() < f32::EPSILON);
        assert_eq!(target_level_fraction(TARGET_MAX_DB), 1.0);
        assert_eq!(target_level_fraction(24.0), 1.0);
        assert_eq!(target_level_fraction(f32::NAN), 0.0);

        let bottom = marker_center_y(track, TARGET_MIN_DB);
        let middle = marker_center_y(track, -18.0);
        let top = marker_center_y(track, TARGET_MAX_DB);
        assert!((bottom - (track.max.y - VERTICAL_SLIDER_THUMB_HEIGHT * 0.5)).abs() < f32::EPSILON);
        assert!((middle - track.center().y).abs() < f32::EPSILON);
        assert!((top - (track.min.y + VERTICAL_SLIDER_THUMB_HEIGHT * 0.5)).abs() < f32::EPSILON);
        assert!(top < middle && middle < bottom);

        for db in [-120.0, TARGET_MIN_DB, TARGET_MAX_DB, 24.0] {
            for side in [PeakMarkerSide::Left, PeakMarkerSide::Right] {
                let marker = peak_marker_rect(bounds, track, db, side)
                    .expect("a normal slider should have room for a marker");
                assert!(marker.min.x >= bounds.min.x);
                assert!(marker.max.x <= bounds.max.x);
                assert!(marker.min.y >= bounds.min.y);
                assert!(marker.max.y <= bounds.max.y);
            }
        }
    }

    #[test]
    fn peak_markers_use_opposite_sides_and_distinct_colors_at_coincident_levels() {
        let bounds = Rect::from_size(28.0, 140.0);
        let track = vertical_slider_track(bounds);
        let left = peak_marker_rect(bounds, track, -12.0, PeakMarkerSide::Left)
            .expect("left marker should be visible");
        let right = peak_marker_rect(bounds, track, -12.0, PeakMarkerSide::Right)
            .expect("right marker should be visible");

        assert!((left.min.y - right.min.y).abs() < f32::EPSILON);
        assert!((left.max.y - right.max.y).abs() < f32::EPSILON);
        assert!(left.max.x < track.min.x);
        assert!(right.min.x > track.max.x);

        let mut primitives = Vec::new();
        let slider =
            VerticalSlider::new(TARGET_RANGE.normalize(-12.0)).with_peak_levels(-12.0, -12.0);
        let theme = ThemeTokens::default();
        slider.append_paint(
            &mut primitives,
            bounds,
            &radiant::layout::LayoutOutput::default(),
            &theme,
        );
        let marker_colors = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if (fill.rect.width() - VERTICAL_SLIDER_MARKER_WIDTH).abs() < f32::EPSILON
                        && (fill.rect.height() - VERTICAL_SLIDER_MARKER_HEIGHT).abs()
                            < f32::EPSILON =>
                {
                    Some(fill.color)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(marker_colors.contains(&theme.highlight_orange));
        assert!(marker_colors.contains(&theme.highlight_cyan));
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
    fn normalize_sets_zero_target_and_starts_matching() {
        let mut state = editor_state();

        reduce_message(
            &mut state,
            EditorMessage::TargetTextChanged(TextInputMessage::Submitted {
                value: String::from("-6.5"),
            }),
        );
        reduce_message(&mut state, EditorMessage::Normalize);

        assert_eq!(state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE), 0.0);
        assert!(state.params.match_requested());
        assert_eq!(state.target_text, "0.0");
        assert_eq!(state.target_text_param, 0.0);
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
        assert!(plan.contains_text("MATCH"));
        assert!(plan.contains_text("0 dB"));
        assert!(plan.contains_text_input());
        assert!(!plan.contains_text("GAIN SNAP"));
        assert!(!plan.contains_text("TOGGLE PEAK MATCH"));
        assert!(!plan.contains_text("TARGET PEAK"));
        assert!(!plan.contains_text("NORMALIZE"));
        assert!(!plan.contains_text("dBFS"));
        assert!(!plan.contains_text("Ready — target"));
    }

    #[test]
    fn compact_surface_keeps_action_buttons_at_requested_size() {
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

        assert!((match_bounds.width() - MATCH_BUTTON_WIDTH).abs() < f32::EPSILON);
        assert!((match_bounds.height() - MATCH_BUTTON_HEIGHT).abs() < f32::EPSILON);

        let normalize_widget_id = plan
            .first_text_run("0 dB")
            .expect("compact normalize label should be painted")
            .widget_id;
        let normalize_bounds = plan
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillPolygon(paint) if paint.widget_id == normalize_widget_id => {
                    let min_x = paint
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::INFINITY, f32::min);
                    let min_y = paint
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::INFINITY, f32::min);
                    let max_x = paint
                        .points
                        .iter()
                        .map(|point| point.x)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let max_y = paint
                        .points
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    Some(Rect::from_min_max(
                        Point::new(min_x, min_y),
                        Point::new(max_x, max_y),
                    ))
                }
                _ => None,
            })
            .expect("normalize button should paint a polygon control");

        assert!((normalize_bounds.width() - NORMALIZE_BUTTON_WIDTH).abs() < f32::EPSILON);
        assert!((normalize_bounds.height() - NORMALIZE_BUTTON_HEIGHT).abs() < f32::EPSILON);
    }

    #[test]
    fn compact_surface_places_meter_and_entry_in_a_single_vertical_control() {
        let mut state = editor_state();
        let plan = project_surface(&mut state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &ThemeTokens::default(),
            )
            .paint_plan;
        let frame = plan
            .stroke_rects()
            .find(|stroke| {
                (stroke.rect.width() - (TARGET_CONTROL_WIDTH - 1.0)).abs() < f32::EPSILON
                    && (stroke.rect.height() - (TARGET_SLIDER_HEIGHT - 1.0)).abs() < f32::EPSILON
            })
            .expect("the vertical meter should have a framed control bounds");
        let tick_count = plan
            .fill_rects_for_widget(frame.widget_id)
            .filter(|fill| {
                fill.rect.height() == VERTICAL_SLIDER_TICK_HEIGHT
                    && fill.rect.width() <= VERTICAL_SLIDER_TICK_WIDTH
            })
            .count();
        assert_eq!(tick_count, VERTICAL_SLIDER_TICK_COUNT * 2);

        let entry = plan
            .first_text_input()
            .expect("the target entry should remain visible below the meter");
        assert!(entry.rect.min.y >= frame.rect.max.y);
        assert!(entry.rect.width() <= TARGET_CONTROL_WIDTH);
    }

    #[test]
    fn focused_target_entry_keeps_formatted_values_and_caret_room() {
        const MIN_FOCUSED_TARGET_ENTRY_WIDTH: f32 = 68.0;

        for target_db in [
            TARGET_MIN_DB,
            crate::params::DEFAULT_TARGET_DB,
            TARGET_MAX_DB,
        ] {
            let params = Arc::new(crate::params::GainSnapParams::new());
            params.set_param(PARAM_TARGET_DB, target_db);
            let mut editor = GainSnapEditor::new(
                params,
                Arc::new(AutomationQueue::default()),
                Arc::new(GuiStatus::default()),
                None,
                None,
            );

            let unfocused = editor.paint_plan().clone();
            let target_entry = unfocused
                .first_text_input()
                .expect("the target entry should be painted");
            let entry_center = target_entry.rect.center();

            editor.dispatch_event(Event::primary_press(entry_center));

            let focused = editor
                .paint_plan()
                .first_text_input()
                .expect("the focused target entry should remain painted");
            assert!(focused.focused);
            assert_eq!(focused.state.value, format_target_text(target_db));
            assert!(
                TARGET_CONTROL_WIDTH >= MIN_FOCUSED_TARGET_ENTRY_WIDTH,
                "target entry outer width must leave room for focused caret rendering: {TARGET_CONTROL_WIDTH}px"
            );
            assert!(
                focused.rect.width() >= MIN_FOCUSED_TARGET_ENTRY_WIDTH - 16.0,
                "focused target entry content width is too narrow for its caret: {:?}",
                focused.rect
            );
        }
    }

    #[test]
    fn compact_controls_export_explicit_automation_semantics() {
        let normalize = NormalizeButtonWidget::new();
        assert!(normalize.capabilities().has_semantics());
        let normalize_semantics = normalize.automation_semantics();
        assert_eq!(normalize_semantics.role, AutomationRole::Button);
        assert_eq!(
            normalize_semantics.label.as_deref(),
            Some(NORMALIZE_AUTOMATION_LABEL)
        );
        assert_eq!(
            normalize_semantics.description.as_deref(),
            Some(NORMALIZE_AUTOMATION_DESCRIPTION)
        );

        fn find_node<'a>(
            node: &'a radiant::runtime::AutomationNodeSnapshot,
            label: &str,
        ) -> Option<&'a radiant::runtime::AutomationNodeSnapshot> {
            if node.label.as_deref() == Some(label) {
                return Some(node);
            }
            node.children
                .iter()
                .find_map(|child| find_node(child, label))
        }

        let editor = GainSnapEditor::new(
            Arc::new(crate::params::GainSnapParams::new()),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );
        let snapshot = editor.runtime.automation_snapshot();
        let target_node = find_node(&snapshot.root, TARGET_ENTRY_AUTOMATION_LABEL)
            .expect("target entry should export its semantic label through the surface");
        assert_eq!(target_node.role, AutomationRole::TextInput);
        assert!(target_node
            .available_actions
            .iter()
            .any(|action| action == "set_text"));

        let normalize_node = find_node(&snapshot.root, NORMALIZE_AUTOMATION_LABEL)
            .expect("normalize should export its semantic label through the surface");
        assert_eq!(
            normalize_node.semantics.description.as_deref(),
            Some(NORMALIZE_AUTOMATION_DESCRIPTION)
        );
        assert!(normalize_node
            .available_actions
            .iter()
            .any(|action| action == "press"));
    }

    #[test]
    fn compact_controls_fit_minimum_viewport_with_horizontal_margins() {
        let mut state = editor_state();
        let frame = project_surface(&mut state).frame_at_size(
            Vector2::new(MIN_WINDOW_WIDTH as f32, MIN_WINDOW_HEIGHT as f32),
            &ThemeTokens::default(),
        );
        let viewport = frame.viewport;
        let epsilon = 0.001;
        let control_sizes = [
            (TARGET_CONTROL_WIDTH, ACTION_CONTROL_HEIGHT),
            (ACTION_CONTROL_WIDTH, ACTION_CONTROL_HEIGHT),
            (ACTION_CONTROL_WIDTH, MATCHING_CONTROL_HEIGHT),
            (TARGET_CONTROL_WIDTH, TARGET_SLIDER_HEIGHT),
            (TARGET_CONTROL_WIDTH, TARGET_ENTRY_HEIGHT),
            (MATCH_BUTTON_WIDTH, MATCH_BUTTON_HEIGHT),
            (NORMALIZE_BUTTON_WIDTH, NORMALIZE_BUTTON_HEIGHT),
        ];
        let controls = frame
            .layout
            .rects
            .values()
            .copied()
            .filter(|rect| {
                control_sizes.iter().any(|(width, height)| {
                    (rect.width() - width).abs() <= epsilon
                        && (rect.height() - height).abs() <= epsilon
                })
            })
            .collect::<Vec<_>>();

        assert_eq!(controls.len(), control_sizes.len());
        for rect in controls {
            assert!(rect.min.x >= SURFACE_PADDING_X - epsilon);
            assert!(rect.max.x <= viewport.max.x - SURFACE_PADDING_X + epsilon);
            assert!(rect.min.y >= 0.0);
            assert!(rect.max.y <= viewport.max.y);
        }
    }

    #[test]
    fn status_indicator_keeps_each_match_state_visually_distinct() {
        let theme = ThemeTokens::default();
        for (state, expected_color) in [
            (MatchState::Ready, theme.border_emphasis),
            (MatchState::Measuring, theme.accent_mint),
            (MatchState::Locked, theme.highlight_cyan),
            (MatchState::NoSignal, theme.accent_danger),
        ] {
            let indicator = StatusIndicator::new(state);
            let mut primitives = Vec::new();
            indicator.append_paint(
                &mut primitives,
                Rect::from_size(STATUS_INDICATOR_SIZE, STATUS_INDICATOR_SIZE),
                &radiant::layout::LayoutOutput::default(),
                &theme,
            );
            let polygon = primitives
                .iter()
                .find_map(PaintPrimitive::fill_polygon)
                .expect("status indicator should paint a dot");
            assert_eq!(polygon.points.len(), 16);
            assert_eq!(polygon.color, expected_color);
        }
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
        let plan = editor.paint_plan();
        assert!(plan
            .fill_polygons()
            .any(|polygon| polygon.points.len() == 16));
        assert!(!plan.contains_text("Measuring… toggle off to lock"));
        assert!(!editor.needs_realtime_redraw());

        status.update(-6.0, -3.0, 3.0, 1.0, MatchState::Locked);
        assert!(editor.needs_realtime_redraw());
        let plan = editor.paint_plan();
        assert!(plan
            .fill_polygons()
            .any(|polygon| polygon.points.len() == 16));
        assert!(!plan.contains_text("Locked +3.00 dB"));
        assert!(!editor.needs_realtime_redraw());
    }

    fn marker_rect_for_color(plan: &SurfacePaintPlan, color: Rgba8) -> Rect {
        plan.primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == color
                        && (fill.rect.width() - VERTICAL_SLIDER_MARKER_WIDTH).abs()
                            < f32::EPSILON
                        && (fill.rect.height() - VERTICAL_SLIDER_MARKER_HEIGHT).abs()
                            < f32::EPSILON =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .expect("the requested peak marker should be painted")
    }

    #[test]
    fn editor_repaints_when_realtime_peak_markers_move() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::clone(&status),
            None,
            None,
        );
        let theme = ThemeTokens::default();

        let (initial_input, initial_output) = {
            let plan = editor.paint_plan();
            (
                marker_rect_for_color(plan, theme.highlight_orange),
                marker_rect_for_color(plan, theme.highlight_cyan),
            )
        };

        status.update(-6.0, -18.0, 0.0, 0.5, MatchState::Measuring);
        assert!(editor.needs_realtime_redraw());
        let plan = editor.paint_plan();
        let updated_input = marker_rect_for_color(plan, theme.highlight_orange);
        let updated_output = marker_rect_for_color(plan, theme.highlight_cyan);

        assert!(updated_input.min.y < initial_input.min.y);
        assert!(updated_output.min.y < initial_output.min.y);
        assert!(!plan.contains_text("IN  -6.0 dBFS"));
        assert!(!plan.contains_text("OUT -18.0 dBFS"));
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
