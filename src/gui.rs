//! Radiant editor for the macOS CLAP and VST3 GainSnap views.
//!
//! The editor owns only retained UI state. Audio-thread telemetry is published
//! through atomics, and parameter gestures use the CLAP queue or the VST3
//! component-handler sink supplied by the respective host adapter.

use std::sync::Arc;
use std::time::{Duration, Instant};

use radiant::gui::automation::AutomationRole;
use radiant::gui::types::{Point, Rect, Rgba8, Vector2};
use radiant::layout::{CrossAlign, MainAlign};
use radiant::prelude::{
    column, custom_widget, custom_widget_mapped, row, text_input, IntoView, Widget, WidgetCommon,
    WidgetInput, WidgetKey, WidgetOutput, WidgetSizing,
};
use radiant::runtime::{DeclarativeSurfaceRuntime, Event, SurfacePaintPlan, UiSurface};
use radiant::runtime::{
    PaintFillPolygon, PaintFillRect, PaintPrimitive, PaintStrokePolygon, PaintStrokeRect,
    PaintText, PaintTextAlign, PaintTextRun,
};
use radiant::theme::ThemeTokens;
use radiant::widgets::{
    ButtonMessage, ButtonWidget, FocusBehavior, PaintBounds, PointerButton, PointerCapturePolicy,
    SliderMessage, TextInputMessage, TextWrap, ToggleMessage, ToggleWidget, WidgetCapabilities,
    WidgetId, WidgetSemantics, WidgetStyle, WidgetTone,
};
use toybox::clack_plugin::utils::ClapId;
use toybox::clap::automation::{AutomationConfig, AutomationQueue};

use crate::clap_plugin::HostParamRequester;
use crate::params::{PARAM_MATCH, PARAM_TARGET_DB, TARGET_MAX_DB, TARGET_MIN_DB};
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
// Compact text inputs reserve horizontal insets for their chrome and focused
// caret. Keep enough room for the longest formatted target (for example,
// `-12.0`) to remain visible while it is being edited.
const TARGET_CONTROL_WIDTH: f32 = 68.0;
const TARGET_METER_HEIGHT: f32 = 148.0;
const TARGET_ENTRY_HEIGHT: f32 = 28.0;
const TARGET_CONTROL_SPACING: f32 = 6.0;
const ACTION_CONTROL_WIDTH: f32 = 96.0;
const ACTION_CONTROL_HEIGHT: f32 =
    TARGET_METER_HEIGHT + TARGET_CONTROL_SPACING + TARGET_ENTRY_HEIGHT;
const MATCHING_CONTROL_HEIGHT: f32 = 62.0;
const MATCH_BUTTON_WIDTH: f32 = ACTION_CONTROL_WIDTH;
const MATCH_BUTTON_HEIGHT: f32 = 32.0;
const NORMALIZE_BUTTON_WIDTH: f32 = ACTION_CONTROL_WIDTH;
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
const SURFACE_COLUMN_GAP: f32 = 12.0;
const MATCH_PULSE_SECONDS: f32 = 1.2;

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
    active: bool,
    pulse_alpha: u8,
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
        Self {
            common,
            state,
            active: false,
            pulse_alpha: 255,
        }
    }

    fn with_pulse(mut self, active: bool, alpha: u8) -> Self {
        self.active = active;
        self.pulse_alpha = alpha;
        self
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
        if self.active {
            return theme.accent_mint.with_alpha(self.pulse_alpha);
        }
        match self.state {
            MatchState::Ready => theme.border_emphasis,
            MatchState::Measuring => theme.accent_mint,
            MatchState::Locked => theme.highlight_cyan,
            MatchState::NoSignal => theme.accent_danger,
        }
    }
}

/// Keep Radiant's toggle input, focus, and automation behavior; only its active
/// accent is animated using the same opacity as the status indicator.
#[derive(Clone, Debug, PartialEq)]
struct MatchButtonWidget {
    toggle: ToggleWidget,
    pulse_alpha: u8,
}

impl MatchButtonWidget {
    fn new(checked: bool, pulse_alpha: u8) -> Self {
        Self {
            toggle: ToggleWidget::new(
                0,
                "MATCH",
                WidgetSizing::fixed(Vector2::new(MATCH_BUTTON_WIDTH, MATCH_BUTTON_HEIGHT)),
            )
            .with_checked(checked),
            pulse_alpha,
        }
    }
}

impl Widget for MatchButtonWidget {
    fn common(&self) -> &WidgetCommon {
        self.toggle.common()
    }
    fn common_mut(&mut self) -> &mut WidgetCommon {
        self.toggle.common_mut()
    }
    fn handle_input(&mut self, bounds: Rect, input: WidgetInput) -> Option<WidgetOutput> {
        self.toggle
            .handle_input(bounds, input)
            .map(WidgetOutput::typed)
    }
    fn accepts_pointer_move(&self) -> bool {
        false
    }
    fn capabilities(&self) -> WidgetCapabilities<'_> {
        self.toggle.capabilities()
    }
    fn synchronize_from_previous(&mut self, previous: &dyn Widget) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.toggle.synchronize_from_previous(&previous.toggle);
        }
    }
    fn append_paint(
        &self,
        primitives: &mut Vec<PaintPrimitive>,
        bounds: Rect,
        layout: &radiant::layout::LayoutOutput,
        theme: &ThemeTokens,
    ) {
        let mut theme = *theme;
        if self.toggle.state.checked {
            theme.accent_mint = theme.accent_mint.with_alpha(self.pulse_alpha);
        }
        self.toggle.append_paint(primitives, bounds, layout, &theme);
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
                "Normalize",
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

/// Compact target control that combines a realtime output meter with a draggable
/// target marker. The full widget bounds are an intentionally wider invisible
/// rail, so the small marker remains easy to grab without changing the visual
/// proportions of the editor.
#[derive(Clone, Debug, PartialEq)]
struct TargetMeter {
    common: WidgetCommon,
    value: f32,
    shift_held: bool,
    output_peak_db: f32,
}

impl TargetMeter {
    fn new(value: f32) -> Self {
        let mut common = WidgetCommon::new(
            0,
            WidgetSizing::fixed(Vector2::new(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT)),
        );
        common.focus = FocusBehavior::Keyboard;
        common.paint.bounds = PaintBounds::ClipToRect;
        common.paint.paints_focus = false;
        common.paint.paints_state_layers = false;
        Self {
            common,
            value: clamp_fraction(value),
            shift_held: false,
            output_peak_db: -120.0,
        }
    }

    fn with_shift_held(mut self, shift_held: bool) -> Self {
        self.shift_held = shift_held;
        self
    }

    fn with_output_peak_db(mut self, output_peak_db: f32) -> Self {
        self.output_peak_db = output_peak_db;
        self
    }

    fn value_for_position(bounds: Rect, position: Point) -> f32 {
        target_value_for_position(bounds, position)
    }

    fn set_value(&mut self, value: f32) -> Option<SliderMessage> {
        let value = clamp_fraction(value);
        if (self.value - value).abs() <= f32::EPSILON {
            return None;
        }
        self.value = value;
        Some(SliderMessage::ValueChanged { value })
    }

    fn step_value(&mut self, direction: TargetStepDirection) -> Option<SliderMessage> {
        let target_db = TARGET_RANGE.denormalize(self.value);
        let next_target_db = step_target_db(target_db, direction, self.shift_held);
        self.set_value(TARGET_RANGE.normalize(next_target_db))
    }

    fn pointer_value(&mut self, bounds: Rect, position: Point) -> Option<WidgetOutput> {
        self.set_value(Self::value_for_position(bounds, position))
            .map(WidgetOutput::typed)
    }
}

impl WidgetSemantics for TargetMeter {
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

impl Widget for TargetMeter {
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
                if was_pressed {
                    self.common.state.hovered = bounds.contains(position);
                    self.pointer_value(bounds, position)
                } else {
                    None
                }
            }
            WidgetInput::FocusChanged(focused) => {
                self.common.state.focused = focused;
                if !focused {
                    self.common.state.pressed = false;
                }
                None
            }
            WidgetInput::PointerModifiersChanged { modifiers } => {
                self.shift_held = modifiers.shift;
                None
            }
            WidgetInput::KeyPress(key) if self.common.state.focused => match key {
                WidgetKey::ArrowUp | WidgetKey::ArrowRight => self
                    .step_value(TargetStepDirection::Up)
                    .map(WidgetOutput::typed),
                WidgetKey::ArrowDown | WidgetKey::ArrowLeft => self
                    .step_value(TargetStepDirection::Down)
                    .map(WidgetOutput::typed),
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

    fn accepts_pointer_move(&self) -> bool {
        true
    }

    fn pointer_capture_policy(&self) -> PointerCapturePolicy {
        PointerCapturePolicy::Exclusive
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
        let track = target_meter_track(bounds);
        let tokens = radiant::widgets::resolve_widget_visual_tokens(
            theme,
            self.common.style,
            self.common.state,
        );

        // The meter itself is deliberately narrow, while the widget bounds
        // remain wide enough to act as an invisible target-selection rail.
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id: self.common.id,
            rect: track,
            color: theme.bg_primary,
        }));

        push_meter_labels(
            primitives,
            self.common.id,
            track,
            theme.text_muted.with_alpha(TARGET_METER_LABEL_ALPHA),
        );

        let tick_color = theme.grid_strong;
        for index in 0..TARGET_METER_TICK_COUNT {
            let fraction = index as f32 / (TARGET_METER_TICK_COUNT - 1) as f32;
            let y = track.min.y + (1.0 - fraction) * track.height();
            let tick_y = (y - TARGET_METER_TICK_HEIGHT * 0.5)
                .clamp(track.min.y, track.max.y - TARGET_METER_TICK_HEIGHT);
            let tick_width = if index % 2 == 0 {
                TARGET_METER_TICK_WIDTH
            } else {
                TARGET_METER_TICK_WIDTH - 1.0
            };
            primitives.push(PaintPrimitive::FillRect(PaintFillRect {
                widget_id: self.common.id,
                rect: Rect::from_min_max(
                    Point::new(track.min.x - TARGET_MARKER_GAP - tick_width, tick_y),
                    Point::new(
                        track.min.x - TARGET_MARKER_GAP,
                        tick_y + TARGET_METER_TICK_HEIGHT,
                    ),
                ),
                color: tick_color,
            }));
        }

        push_meter_level(
            primitives,
            self.common.id,
            track,
            self.output_peak_db,
            theme.highlight_orange,
        );
        primitives.push(PaintPrimitive::StrokeRect(PaintStrokeRect {
            widget_id: self.common.id,
            rect: track,
            color: theme.border,
            width: 1.0,
        }));

        if let Some(points) = target_marker_points(track, TARGET_RANGE.denormalize(self.value)) {
            primitives.push(PaintPrimitive::FillPolygon(PaintFillPolygon {
                widget_id: self.common.id,
                points: points.clone(),
                color: theme.text_primary,
            }));
            if self.common.state.focused {
                primitives.push(PaintPrimitive::StrokePolygon(PaintStrokePolygon {
                    widget_id: self.common.id,
                    points,
                    color: tokens.emphasis,
                    width: 1.0,
                }));
            }
        }
    }
}

fn target_meter_track(bounds: Rect) -> Rect {
    let track_width = TARGET_METER_TRACK_WIDTH.min(bounds.width());
    let track_x = bounds.min.x + (bounds.width() - track_width) * 0.5;
    let vertical_inset = TARGET_METER_VERTICAL_INSET.min(bounds.height() * 0.5);
    Rect::from_min_max(
        Point::new(track_x, bounds.min.y + vertical_inset),
        Point::new(track_x + track_width, bounds.max.y - vertical_inset),
    )
}

fn target_level_fraction(db: f32) -> f32 {
    if db.is_finite() {
        TARGET_RANGE.normalize(db)
    } else {
        0.0
    }
}

fn target_value_for_position(bounds: Rect, position: Point) -> f32 {
    let track = target_meter_track(bounds);
    let Some(geometry) = target_marker_geometry(track) else {
        return 0.0;
    };
    if geometry.travel <= 0.0 {
        return 0.0;
    }
    clamp_fraction((geometry.bottom_center_y - position.y) / geometry.travel)
}

fn meter_level_rect(track: Rect, db: f32) -> Option<Rect> {
    if !track.has_finite_positive_area() {
        return None;
    }
    let height = track.height() * target_level_fraction(db);
    (height > 0.0)
        .then(|| Rect::from_min_max(Point::new(track.min.x, track.max.y - height), track.max))
}

fn push_meter_level(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: WidgetId,
    track: Rect,
    db: f32,
    color: Rgba8,
) {
    if let Some(rect) = meter_level_rect(track, db) {
        primitives.push(PaintPrimitive::FillRect(PaintFillRect {
            widget_id,
            rect,
            color,
        }));
    }
}

fn meter_label_rect(track: Rect, db: f32) -> Option<Rect> {
    if !track.has_finite_positive_area() {
        return None;
    }
    let center_y = track.max.y - target_level_fraction(db) * track.height();
    let max_label_y = (track.max.y - TARGET_METER_LABEL_FONT_SIZE).max(track.min.y);
    let label_y = (center_y - TARGET_METER_LABEL_FONT_SIZE * 0.5).clamp(track.min.y, max_label_y);
    let label_x = track.min.x - TARGET_METER_LABEL_GAP - TARGET_METER_LABEL_WIDTH;
    let rect = Rect::from_xy_size(
        label_x,
        label_y,
        TARGET_METER_LABEL_WIDTH,
        TARGET_METER_LABEL_FONT_SIZE,
    );
    rect.has_finite_positive_area().then_some(rect)
}

fn push_meter_labels(
    primitives: &mut Vec<PaintPrimitive>,
    widget_id: WidgetId,
    track: Rect,
    color: Rgba8,
) {
    for &(label, db) in &TARGET_METER_LABELS {
        let Some(rect) = meter_label_rect(track, db) else {
            continue;
        };
        primitives.push(PaintPrimitive::Text(PaintTextRun {
            widget_id,
            text: PaintText::from_static(label),
            rect,
            font_size: TARGET_METER_LABEL_FONT_SIZE,
            baseline: Some(TARGET_METER_LABEL_FONT_SIZE),
            color,
            align: PaintTextAlign::Right,
            wrap: TextWrap::None,
        }));
    }
}

#[derive(Clone, Copy, Debug)]
struct TargetMarkerGeometry {
    height: f32,
    top_center_y: f32,
    bottom_center_y: f32,
    travel: f32,
}

fn target_marker_geometry(track: Rect) -> Option<TargetMarkerGeometry> {
    if !track.has_finite_positive_area() {
        return None;
    }
    let height = TARGET_MARKER_HEIGHT.min(track.height());
    if !height.is_finite() || height <= 0.0 {
        return None;
    }
    let half_height = height * 0.5;
    let top_center_y = track.min.y + half_height;
    let bottom_center_y = track.max.y - half_height;
    let travel = (bottom_center_y - top_center_y).max(0.0);
    (top_center_y.is_finite() && bottom_center_y.is_finite() && travel.is_finite()).then_some(
        TargetMarkerGeometry {
            height,
            top_center_y,
            bottom_center_y,
            travel,
        },
    )
}

fn target_marker_rect(track: Rect, db: f32) -> Option<Rect> {
    let geometry = target_marker_geometry(track)?;
    let marker_width = TARGET_MARKER_WIDTH;
    let fraction = target_level_fraction(db);
    let center_y = if fraction >= 1.0 {
        geometry.top_center_y
    } else if fraction <= 0.0 {
        geometry.bottom_center_y
    } else {
        geometry.bottom_center_y - fraction * geometry.travel
    };
    let marker_x = track.max.x + TARGET_MARKER_GAP;
    let rect = Rect::from_min_max(
        Point::new(marker_x, center_y - geometry.height * 0.5),
        Point::new(marker_x + marker_width, center_y + geometry.height * 0.5),
    );
    rect.has_finite_positive_area().then_some(rect)
}

fn target_marker_points(track: Rect, db: f32) -> Option<Arc<[Point]>> {
    let rect = target_marker_rect(track, db)?;
    let center_y = rect.center().y;
    Some(Arc::from(vec![
        Point::new(rect.min.x, center_y),
        Point::new(rect.max.x, rect.min.y),
        Point::new(rect.max.x, rect.max.y),
    ]))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DisplaySnapshot {
    target_db: u32,
    match_requested: bool,
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
            output_peak_db: sanitize_meter_level_db(status.output_peak_db()).to_bits(),
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
    StepTarget(TargetStepDirection),
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
    output_peak_db: f32,
    meter_last_update: Instant,
    pulse_started: Option<Instant>,
    pulse_alpha: u8,
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
        let output_peak_db = sanitize_meter_level_db(status.output_peak_db());
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
            output_peak_db,
            meter_last_update: Instant::now(),
            pulse_started: None,
            pulse_alpha: 255,
        }
    }

    fn advance_meter_at(&mut self, now: Instant) -> bool {
        let target_db = sanitize_meter_level_db(self.status.output_peak_db());
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
        let target_db = sanitize_meter_level_db(self.status.output_peak_db());
        (self.output_peak_db - target_db).abs() > METER_SETTLE_EPSILON_DB
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

    fn step_target(&mut self, direction: TargetStepDirection) {
        let target_db = self.parameter_value(PARAM_TARGET_DB, TARGET_RANGE);
        let next_target_db = step_target_db(target_db, direction, self.shift_held);
        if (next_target_db - target_db).abs() > TARGET_TEXT_SYNC_EPSILON {
            self.set_target_db(next_target_db);
        } else {
            self.target_text_param = target_db;
        }
        self.target_text = format_target_text(self.target_text_param);
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
        let now = Instant::now();
        let meter_changed = self.runtime.bridge_mut().state_mut().advance_meter_at(now);
        let pulse_changed = self.runtime.bridge_mut().state_mut().advance_pulse_at(now);
        if display_snapshot != self.last_display_snapshot || meter_changed || pulse_changed {
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
            || self.runtime.bridge().state().meter_needs_realtime_redraw()
            || self.runtime.bridge().state().params.match_requested()
    }

    fn dispatch_key_press(&mut self, key: WidgetKey) -> bool {
        let direction = match key {
            WidgetKey::ArrowUp => Some(TargetStepDirection::Up),
            WidgetKey::ArrowDown => Some(TargetStepDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            if self.runtime.focused_text_input_id().is_some() {
                self.runtime
                    .dispatch_message(EditorMessage::StepTarget(direction));
                return true;
            }
        }
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
        TargetMeter::new(TARGET_RANGE.normalize(target_db))
            .with_shift_held(state.shift_held)
            .with_output_peak_db(state.output_peak_db),
        |message: SliderMessage| match message {
            SliderMessage::ValueChanged { value } => {
                EditorMessage::TargetChanged(TARGET_RANGE.denormalize(value))
            }
        },
    )
    .primary()
    .key("target-peak")
    .width(TARGET_CONTROL_WIDTH)
    .height(TARGET_METER_HEIGHT);
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
        custom_widget_mapped(
            MatchButtonWidget::new(state.params.match_requested(), state.pulse_alpha),
            |message: ToggleMessage| {
                let ToggleMessage::ValueChanged { checked } = message;
                EditorMessage::Toggle {
                    id: PARAM_MATCH,
                    checked,
                }
            },
        )
        .style(WidgetStyle::normal(WidgetTone::Accent))
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
    .spacing(6.0)
    .align_main(MainAlign::Center)
    .align_cross(CrossAlign::End);
    let target_control = column([target, target_entry])
        .width(TARGET_CONTROL_WIDTH)
        .height(ACTION_CONTROL_HEIGHT)
        .spacing(TARGET_CONTROL_SPACING)
        .align_cross(CrossAlign::Center);
    let action_control = column([
        matching,
        custom_widget(
            StatusIndicator::new(match_state)
                .with_pulse(state.params.match_requested(), state.pulse_alpha),
            |_output| None,
        )
        .key("status-indicator")
        .size(STATUS_INDICATOR_SIZE, STATUS_INDICATOR_SIZE)
        .tooltip(StatusIndicator::label(match_state)),
    ])
    .width(ACTION_CONTROL_WIDTH)
    .height(ACTION_CONTROL_HEIGHT)
    .spacing(10.0)
    .align_main(MainAlign::Center)
    .align_cross(CrossAlign::Center);
    let view = row([target_control, action_control])
        .height(ACTION_CONTROL_HEIGHT)
        .spacing(SURFACE_COLUMN_GAP)
        .padding_x(SURFACE_PADDING_X)
        .padding_y(15.0)
        .align_main(MainAlign::Center)
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
        EditorMessage::StepTarget(direction) => state.step_target(direction),
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
    use std::sync::Mutex;

    #[derive(Clone, Debug, PartialEq)]
    enum EditEvent {
        Begin(ClapId),
        Value(ClapId, f64),
        End(ClapId),
    }

    #[derive(Default)]
    struct RecordingEditSink {
        events: Mutex<Vec<EditEvent>>,
    }

    impl RecordingEditSink {
        fn events(&self) -> Vec<EditEvent> {
            self.events
                .lock()
                .expect("edit events should not be poisoned")
                .clone()
        }
    }

    impl HostParamEditSink for RecordingEditSink {
        fn gesture_started(&self, _config: &AutomationConfig, param_id: ClapId) {
            self.events
                .lock()
                .expect("edit events should not be poisoned")
                .push(EditEvent::Begin(param_id));
        }

        fn gesture_value(&self, _config: &AutomationConfig, param_id: ClapId, value: f64) {
            self.events
                .lock()
                .expect("edit events should not be poisoned")
                .push(EditEvent::Value(param_id, value));
        }

        fn gesture_ended(&self, _config: &AutomationConfig, param_id: ClapId) {
            self.events
                .lock()
                .expect("edit events should not be poisoned")
                .push(EditEvent::End(param_id));
        }
    }

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
    fn target_meter_maps_pointer_and_keyboard_input() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let mut slider = TargetMeter::new(TARGET_RANGE.normalize(-12.0));

        let output = slider
            .handle_input(bounds, WidgetInput::primary_press(Point::new(14.0, 0.0)))
            .expect("pressing the meter rail should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged { value: 1.0 })
        );

        let output = slider
            .handle_input(
                bounds,
                WidgetInput::PointerMove {
                    position: Point::new(TARGET_CONTROL_WIDTH * 0.5, TARGET_METER_HEIGHT),
                },
            )
            .expect("dragging the meter rail should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged { value: 0.0 })
        );

        slider.set_value(1.0);
        let output = slider
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowDown))
            .expect("focused meter keyboard input should emit a value");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged {
                value: TARGET_RANGE.normalize(-1.0),
            })
        );
    }

    #[test]
    fn target_meter_keyboard_steps_use_db_units_and_shift_fine_step() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let mut slider = TargetMeter::new(TARGET_RANGE.normalize(-12.0));
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
    fn target_meter_keeps_projected_shift_modifier_when_rebuilt() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let previous = TargetMeter::new(TARGET_RANGE.normalize(-12.0));
        let mut current = TargetMeter::new(previous.value).with_shift_held(true);

        current.synchronize_from_previous(&previous);
        current.common.state.focused = true;
        current
            .handle_input(bounds, WidgetInput::KeyPress(WidgetKey::ArrowUp))
            .expect("rebuilt meter should accept keyboard input");

        let target_db = TARGET_RANGE.denormalize(current.value);
        assert!((target_db - (-11.9)).abs() < 0.0001);
    }

    #[test]
    fn target_meter_mapping_uses_target_scale_and_clamps_endpoints() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let track = target_meter_track(bounds);

        assert_eq!(target_level_fraction(-120.0), 0.0);
        assert_eq!(target_level_fraction(TARGET_MIN_DB), 0.0);
        assert!((target_level_fraction(-18.0) - 0.5).abs() < f32::EPSILON);
        assert_eq!(target_level_fraction(TARGET_MAX_DB), 1.0);
        assert_eq!(target_level_fraction(24.0), 1.0);
        assert_eq!(target_level_fraction(f32::NAN), 0.0);

        assert_eq!(
            target_value_for_position(bounds, Point::new(0.0, -10.0)),
            1.0
        );
        assert_eq!(
            target_value_for_position(bounds, Point::new(0.0, 200.0)),
            0.0
        );
        assert!((target_value_for_position(bounds, track.center()) - 0.5).abs() < f32::EPSILON);

        let bottom = target_marker_rect(track, TARGET_MIN_DB)
            .expect("the target marker should be visible at the minimum");
        let middle = target_marker_rect(track, -18.0)
            .expect("the target marker should be visible at the midpoint");
        let top = target_marker_rect(track, TARGET_MAX_DB)
            .expect("the target marker should be visible at the maximum");
        assert!(top.center().y < middle.center().y && middle.center().y < bottom.center().y);
        assert!((top.center().y - (track.min.y + TARGET_MARKER_HEIGHT * 0.5)).abs() < f32::EPSILON);
        assert!(
            (bottom.center().y - (track.max.y - TARGET_MARKER_HEIGHT * 0.5)).abs() < f32::EPSILON
        );
    }

    #[test]
    fn target_meter_round_trips_marker_centers_to_normalized_values() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let track = target_meter_track(bounds);

        for (db, expected) in [(TARGET_MIN_DB, 0.0), (-18.0, 0.5), (TARGET_MAX_DB, 1.0)] {
            let center = target_marker_rect(track, db)
                .expect("the target marker should be visible")
                .center();
            assert_eq!(target_value_for_position(bounds, center), expected);
        }
    }

    #[test]
    fn target_meter_endpoint_triangle_clicks_are_noops_and_release_clears_pressed_state() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let track = target_meter_track(bounds);

        for (db, value) in [(TARGET_MIN_DB, 0.0), (TARGET_MAX_DB, 1.0)] {
            let center = target_marker_rect(track, db)
                .expect("the target marker should be visible")
                .center();
            let mut meter = TargetMeter::new(value);

            assert!(meter
                .handle_input(bounds, WidgetInput::primary_press(center))
                .is_none());
            assert!(meter.common.state.pressed);

            assert!(meter
                .handle_input(
                    bounds,
                    WidgetInput::PointerRelease {
                        position: center,
                        button: PointerButton::Primary,
                        modifiers: PointerModifiers::default(),
                    },
                )
                .is_none());
            assert!(!meter.common.state.pressed);
        }
    }

    #[test]
    fn target_meter_paints_single_full_width_orange_level_and_scale_labels() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let track = target_meter_track(bounds);
        let meter = TargetMeter::new(TARGET_RANGE.normalize(-12.0)).with_output_peak_db(-6.0);
        let theme = ThemeTokens::default();
        let primitives = meter.paint_primitives_with_defaults(bounds);

        let output = meter_level_rect(track, -6.0).expect("output level should be visible");
        assert!(primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill)
                if fill.rect == output && fill.color == theme.highlight_orange)
        }));
        assert!(!primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill)
                if fill.color == theme.highlight_cyan)
        }));
        assert_eq!(output.width(), track.width());

        let labels = primitives
            .iter()
            .filter_map(PaintPrimitive::text_run)
            .filter(|run| run.widget_id == meter.common.id)
            .collect::<Vec<_>>();
        assert_eq!(labels.len(), TARGET_METER_LABELS.len());
        for ((expected_label, expected_db), label) in TARGET_METER_LABELS.iter().zip(labels) {
            assert_eq!(label.text.as_str(), *expected_label);
            assert_eq!(label.font_size, TARGET_METER_LABEL_FONT_SIZE);
            assert_eq!(
                label.color,
                theme.text_muted.with_alpha(TARGET_METER_LABEL_ALPHA)
            );
            assert!(label.rect.min.x >= bounds.min.x);
            assert!(label.rect.max.x <= track.min.x - TARGET_METER_LABEL_GAP);
            assert!(label.rect.min.y >= bounds.min.y);
            assert!(label.rect.max.y <= bounds.max.y);
            let expected_y = track.max.y - target_level_fraction(*expected_db) * track.height();
            assert!((label.rect.center().y - expected_y).abs() <= 4.0);
        }

        let triangle = primitives
            .iter()
            .find_map(PaintPrimitive::fill_polygon)
            .expect("the target should be painted as a triangle");
        assert_eq!(triangle.points.len(), 3);
        assert_eq!(triangle.color, theme.text_primary);
        assert!(!primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill)
                if fill.color == theme.text_primary)
        }));
    }

    #[test]
    fn target_meter_accepts_pointer_input_across_the_invisible_rail_and_captures_release() {
        let bounds = Rect::from_size(TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT);
        let mut meter = TargetMeter::new(TARGET_RANGE.normalize(-12.0));

        let output = meter
            .handle_input(
                bounds,
                WidgetInput::primary_press(Point::new(1.0, bounds.center().y)),
            )
            .expect("the expanded rail should accept a press away from the visible meter");
        let SliderMessage::ValueChanged { value } = output
            .typed_copied::<SliderMessage>()
            .expect("rail press should emit a slider value");
        assert!((TARGET_RANGE.denormalize(value) - -18.0).abs() < 0.5);
        assert!(meter.common.state.pressed);
        assert!(meter.common.state.focused);

        let output = meter
            .handle_input(
                bounds,
                WidgetInput::PointerMove {
                    position: Point::new(-20.0, bounds.min.y),
                },
            )
            .expect("captured movement should continue to update the target outside bounds");
        assert_eq!(
            output.typed_copied::<SliderMessage>(),
            Some(SliderMessage::ValueChanged { value: 1.0 })
        );
        meter.handle_input(
            bounds,
            WidgetInput::PointerRelease {
                position: Point::new(-20.0, bounds.min.y),
                button: PointerButton::Primary,
                modifiers: PointerModifiers::default(),
            },
        );
        assert!(!meter.common.state.pressed);
        assert_eq!(
            meter.pointer_capture_policy(),
            PointerCapturePolicy::Exclusive
        );
    }

    #[test]
    fn target_meter_pointer_selection_updates_the_automated_target_parameter() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let sink = Arc::new(RecordingEditSink::default());
        let edit_sink: Arc<dyn HostParamEditSink> = sink.clone();
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            Some(edit_sink),
        );
        let meter_rect = editor
            .paint_plan()
            .stroke_rects()
            .find(|stroke| (stroke.rect.width() - TARGET_METER_TRACK_WIDTH).abs() < f32::EPSILON)
            .expect("the target meter track should be painted")
            .rect;

        let press = Point::new(
            meter_rect.min.x - TARGET_MARKER_GAP - 2.0,
            meter_rect.center().y,
        );
        editor.dispatch_event(Event::primary_press(press));
        editor.dispatch_event(Event::primary_release(press));

        assert!((params.target_db() - (-18.0)).abs() < 0.5);
        assert_eq!(
            sink.events(),
            vec![
                EditEvent::Begin(PARAM_TARGET_DB),
                EditEvent::Value(PARAM_TARGET_DB, params.target_db() as f64),
                EditEvent::End(PARAM_TARGET_DB),
            ]
        );
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
    fn focused_target_entry_steps_with_arrows_and_preserves_focus() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let sink = Arc::new(RecordingEditSink::default());
        let edit_sink: Arc<dyn HostParamEditSink> = sink.clone();
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            Some(edit_sink),
        );
        let entry_center = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should be painted")
            .rect
            .center();

        editor.dispatch_event(Event::primary_press(entry_center));
        editor.dispatch_event(Event::primary_release(entry_center));
        assert!(editor.runtime.focused_text_input_id().is_some());

        assert!(editor.dispatch_key_press(WidgetKey::ArrowUp));
        assert_eq!(params.target_db(), -11.0);
        let entry = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should remain painted after stepping");
        assert!(entry.focused);
        assert_eq!(entry.state.value, "-11.0");

        editor.dispatch_event(Event::pointer_modifiers_changed(PointerModifiers {
            shift: true,
            ..PointerModifiers::default()
        }));
        assert!(editor.dispatch_key_press(WidgetKey::ArrowUp));
        assert_eq!(params.target_db(), -10.9);
        let entry = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should remain painted after a fine step");
        assert!(entry.focused);
        assert_eq!(entry.state.value, "-10.9");

        let events = sink.events();
        assert_eq!(events.len(), 6);
        assert_eq!(events[0], EditEvent::Begin(PARAM_TARGET_DB));
        assert_eq!(events[1], EditEvent::Value(PARAM_TARGET_DB, -11.0));
        assert_eq!(events[2], EditEvent::End(PARAM_TARGET_DB));
        assert_eq!(events[3], EditEvent::Begin(PARAM_TARGET_DB));
        assert!(matches!(
            events[4],
            EditEvent::Value(PARAM_TARGET_DB, value) if (value - -10.9).abs() < 0.0001
        ));
        assert_eq!(events[5], EditEvent::End(PARAM_TARGET_DB));
    }

    #[test]
    fn focused_target_entry_keeps_caret_navigation_and_consumes_endpoint_steps() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_TARGET_DB, TARGET_MAX_DB);
        let sink = Arc::new(RecordingEditSink::default());
        let edit_sink: Arc<dyn HostParamEditSink> = sink.clone();
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            Some(edit_sink),
        );
        let entry = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should be painted")
            .clone();
        editor.dispatch_event(Event::primary_press(Point::new(
            entry.rect.max.x - 1.0,
            entry.rect.center().y,
        )));
        editor.dispatch_event(Event::primary_release(Point::new(
            entry.rect.max.x - 1.0,
            entry.rect.center().y,
        )));
        assert!(editor.runtime.focused_text_input_id().is_some());

        let value_len = entry.state.value.chars().count();
        assert_eq!(
            editor
                .paint_plan()
                .first_text_input()
                .expect("the endpoint entry should remain painted")
                .state
                .caret,
            value_len
        );

        assert!(editor.dispatch_key_press(WidgetKey::ArrowUp));
        assert_eq!(params.target_db(), TARGET_MAX_DB);
        assert_eq!(sink.events(), Vec::<EditEvent>::new());
        assert_eq!(
            editor
                .paint_plan()
                .first_text_input()
                .expect("the endpoint entry should remain painted")
                .state
                .value,
            "0.0"
        );

        assert!(editor.dispatch_key_press(WidgetKey::ArrowLeft));
        assert_eq!(
            editor
                .paint_plan()
                .first_text_input()
                .expect("the target entry should remain painted")
                .state
                .caret,
            value_len.saturating_sub(1)
        );

        assert!(editor.dispatch_key_press(WidgetKey::ArrowRight));
        assert_eq!(
            editor
                .paint_plan()
                .first_text_input()
                .expect("the target entry should remain painted")
                .state
                .caret,
            value_len
        );

        assert!(editor.dispatch_key_press(WidgetKey::Home));
        assert_eq!(
            editor
                .paint_plan()
                .first_text_input()
                .expect("the target entry should remain painted")
                .state
                .caret,
            0
        );

        assert!(editor.dispatch_key_press(WidgetKey::End));
        let entry = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should remain painted");
        assert_eq!(entry.state.caret, value_len);
        assert!(entry.focused);
        assert_eq!(params.target_db(), TARGET_MAX_DB);
        assert_eq!(sink.events(), Vec::<EditEvent>::new());
    }

    #[test]
    fn stepping_invalid_target_draft_uses_last_valid_parameter_and_formats_it() {
        let mut state = editor_state();
        state.params.set_param(PARAM_TARGET_DB, -6.5);
        state.target_text = String::from("-");

        reduce_message(
            &mut state,
            EditorMessage::StepTarget(TargetStepDirection::Up),
        );

        assert_eq!(state.parameter_value(PARAM_TARGET_DB, TARGET_RANGE), -5.5);
        assert_eq!(state.target_text_param, -5.5);
        assert_eq!(state.target_text, "-5.5");
    }

    #[test]
    fn focused_target_entry_consumes_minimum_endpoint_without_a_gesture() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_TARGET_DB, TARGET_MIN_DB);
        let sink = Arc::new(RecordingEditSink::default());
        let edit_sink: Arc<dyn HostParamEditSink> = sink.clone();
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            Some(edit_sink),
        );
        let entry_center = editor
            .paint_plan()
            .first_text_input()
            .expect("the target entry should be painted")
            .rect
            .center();
        editor.dispatch_event(Event::primary_press(entry_center));
        editor.dispatch_event(Event::primary_release(entry_center));

        assert!(editor.dispatch_key_press(WidgetKey::ArrowDown));
        assert_eq!(params.target_db(), TARGET_MIN_DB);
        assert!(sink.events().is_empty());
        assert!(editor.runtime.focused_text_input_id().is_some());
    }

    #[test]
    fn typed_target_edit_updates_the_meter_parameter_and_rejects_invalid_submit() {
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

        assert_eq!(preferred_window_size(), (208, 212));
        assert!(plan.contains_text("MATCH"));
        assert!(plan.contains_text("Normalize"));
        for label in ["0 dB", "−6", "−12", "−18", "−24", "−30", "−∞"] {
            assert!(plan.contains_text(label), "missing meter label {label:?}");
        }
        assert!(plan.contains_text_input());
        assert!(!plan.contains_text("GAIN SNAP"));
        assert!(!plan.contains_text("TOGGLE PEAK MATCH"));
        assert!(!plan.contains_text("TARGET PEAK"));
        assert!(!plan.contains_text("dBFS"));
        assert!(!plan.contains_text("Ready — target"));
    }

    #[test]
    fn match_toggle_uses_distinct_off_and_on_visual_tokens() {
        let theme = ThemeTokens::default();

        let mut off_state = editor_state();
        let off_plan = project_surface(&mut off_state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &theme,
            )
            .paint_plan;
        let off_label = off_plan
            .first_text_run("MATCH")
            .expect("off match label should be painted");
        let off_fill = off_plan
            .fill_rects_for_widget(off_label.widget_id)
            .find(|fill| {
                (fill.rect.width() - MATCH_BUTTON_WIDTH).abs() < f32::EPSILON
                    && (fill.rect.height() - MATCH_BUTTON_HEIGHT).abs() < f32::EPSILON
            })
            .expect("off match button should paint its fill");
        let off_bounds = off_plan
            .first_widget_rect(off_label.widget_id)
            .expect("off match button should paint a rectangular control");

        let mut on_state = editor_state();
        on_state.params.set_param(PARAM_MATCH, 1.0);
        let on_plan = project_surface(&mut on_state)
            .frame_at_size(
                Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                &theme,
            )
            .paint_plan;
        let on_label = on_plan
            .first_text_run("MATCH")
            .expect("on match label should be painted");
        let on_fill = on_plan
            .fill_rects_for_widget(on_label.widget_id)
            .find(|fill| {
                (fill.rect.width() - MATCH_BUTTON_WIDTH).abs() < f32::EPSILON
                    && (fill.rect.height() - MATCH_BUTTON_HEIGHT).abs() < f32::EPSILON
            })
            .expect("on match button should paint its fill");
        let on_bounds = on_plan
            .first_widget_rect(on_label.widget_id)
            .expect("on match button should paint a rectangular control");

        assert_eq!(off_label.text.as_str(), "MATCH");
        assert_eq!(on_label.text.as_str(), "MATCH");
        assert_eq!(off_fill.color, theme.surface_raised);
        assert_eq!(off_label.color, theme.accent_mint);
        assert_eq!(on_fill.color, theme.accent_mint);
        assert_eq!(on_label.color, theme.text_primary);
        assert_ne!(off_fill.color, on_fill.color);
        assert_ne!(off_label.color, on_label.color);
        assert_eq!(off_bounds, on_bounds);
        assert_eq!(off_bounds.width(), MATCH_BUTTON_WIDTH);
        assert_eq!(off_bounds.height(), MATCH_BUTTON_HEIGHT);
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
            .first_text_run("Normalize")
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
                (stroke.rect.width() - TARGET_METER_TRACK_WIDTH).abs() < f32::EPSILON
                    && (stroke.rect.height()
                        - (TARGET_METER_HEIGHT - TARGET_METER_VERTICAL_INSET * 2.0))
                        .abs()
                        < f32::EPSILON
            })
            .expect("the vertical meter should have a framed track");
        let tick_count = plan
            .fill_rects_for_widget(frame.widget_id)
            .filter(|fill| {
                fill.rect.height() == TARGET_METER_TICK_HEIGHT
                    && fill.rect.width() <= TARGET_METER_TICK_WIDTH
            })
            .count();
        assert_eq!(tick_count, TARGET_METER_TICK_COUNT);

        let entry = plan
            .first_text_input()
            .expect("the target entry should remain visible below the meter");
        assert!(entry.rect.min.y >= frame.rect.max.y);
        assert!(entry.rect.width() <= TARGET_CONTROL_WIDTH);
    }

    #[test]
    fn focused_target_entry_keeps_formatted_values_and_caret_room() {
        const MIN_FOCUSED_TARGET_ENTRY_WIDTH: f32 = 68.0;
        const FOCUSED_INPUT_HORIZONTAL_INSET: f32 = 16.0;

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
                focused.rect.width() + FOCUSED_INPUT_HORIZONTAL_INSET
                    >= MIN_FOCUSED_TARGET_ENTRY_WIDTH,
                "focused target entry outer width must leave room for caret rendering: {:?}",
                focused.rect
            );
            assert!(
                focused.rect.width()
                    >= MIN_FOCUSED_TARGET_ENTRY_WIDTH - FOCUSED_INPUT_HORIZONTAL_INSET,
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
            (TARGET_CONTROL_WIDTH, TARGET_METER_HEIGHT),
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
        editor.runtime.bridge_mut().state_mut().output_peak_db = -12.0;
        assert!(!editor.needs_realtime_redraw());

        status.update(-6.0, -3.0, 3.0, 1.0, MatchState::Locked);
        assert!(editor.needs_realtime_redraw());
        let plan = editor.paint_plan();
        assert!(plan
            .fill_polygons()
            .any(|polygon| polygon.points.len() == 16));
        assert!(!plan.contains_text("Locked +3.00 dB"));
        assert!(editor.needs_realtime_redraw());
        editor.runtime.bridge_mut().state_mut().output_peak_db = -3.0;
        assert!(!editor.needs_realtime_redraw());
    }

    fn meter_level_rect_for_color(plan: &SurfacePaintPlan, color: Rgba8) -> Rect {
        plan.primitives
            .iter()
            .find_map(|primitive| match primitive {
                PaintPrimitive::FillRect(fill)
                    if fill.color == color && fill.rect.height() > 0.0 =>
                {
                    Some(fill.rect)
                }
                _ => None,
            })
            .expect("the requested meter level should be painted")
    }

    #[test]
    fn editor_repaints_when_only_output_level_moves() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        status.update(-24.0, -24.0, 0.0, 0.5, MatchState::Measuring);
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::clone(&status),
            None,
            None,
        );
        let theme = ThemeTokens::default();

        let initial_output =
            meter_level_rect_for_color(editor.paint_plan(), theme.highlight_orange);

        status.update(-24.0, -12.0, 0.0, 0.5, MatchState::Measuring);
        editor.runtime.bridge_mut().state_mut().meter_last_update =
            Instant::now() - Duration::from_millis(5);
        assert!(editor.needs_realtime_redraw());
        let plan = editor.paint_plan();
        let updated_output = meter_level_rect_for_color(plan, theme.highlight_orange);

        assert!(updated_output.min.y < initial_output.min.y);
        assert_eq!(updated_output.width(), TARGET_METER_TRACK_WIDTH);
        assert!(!plan.primitives.iter().any(|primitive| {
            matches!(primitive, PaintPrimitive::FillRect(fill)
                if fill.color == theme.highlight_cyan)
        }));
    }

    #[test]
    fn editor_ignores_input_only_telemetry_for_meter_repaint() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        status.update(-12.0, -24.0, 0.0, 0.5, MatchState::Measuring);
        let mut editor = GainSnapEditor::new(
            params,
            Arc::new(AutomationQueue::default()),
            Arc::clone(&status),
            None,
            None,
        );
        editor.paint_plan();
        assert!(!editor.needs_realtime_redraw());

        status.update(-3.0, -24.0, 0.0, 0.5, MatchState::Measuring);
        assert!(!editor.needs_realtime_redraw());
    }

    #[test]
    fn matched_audio_paints_the_output_peak_at_the_target_while_on_and_off() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_MATCH, 1.0);
        let mut engine = crate::dsp::GainSnapEngine::new(48_000.0, 0.0);
        engine.begin_block(&params);
        for _ in 0..48_000 {
            engine.process_frame(&params, 0.5, -0.25);
        }

        for matching in [true, false] {
            params.set_param(PARAM_MATCH, f32::from(matching));
            engine.begin_block(&params);
            for _ in 0..256 {
                engine.process_frame(&params, 0.5, -0.25);
            }
            let report = engine.report();
            assert!((report.output_peak_db + 12.0).abs() < 0.001);
            let status = Arc::new(GuiStatus::default());
            status.update(
                report.input_peak_db,
                report.output_peak_db,
                report.locked_gain_db,
                report.progress,
                report.state,
            );
            let mut editor = GainSnapEditor::new(
                Arc::clone(&params),
                Arc::new(AutomationQueue::default()),
                status,
                None,
                None,
            );
            let plan = editor.paint_plan();
            let track = plan
                .stroke_rects()
                .find(|stroke| stroke.rect.width() == TARGET_METER_TRACK_WIDTH)
                .expect("meter frame should be painted")
                .rect;
            let output = meter_level_rect_for_color(plan, ThemeTokens::default().highlight_orange);
            let target = meter_level_rect(track, -12.0).unwrap();
            assert!((output.min.y - target.min.y).abs() < 0.01);
            assert_eq!(output.max.y, target.max.y);
        }
    }

    #[test]
    fn meter_ballistics_are_bounded_monotonic_and_attack_faster_than_release() {
        let attack = smooth_meter_level_db(-24.0, -6.0, Duration::from_millis(5));
        let release = smooth_meter_level_db(-6.0, -24.0, Duration::from_millis(5));

        assert!(attack > -24.0 && attack < -6.0);
        assert!(release < -6.0 && release > -24.0);
        assert!(attack + 24.0 > -6.0 - release);
        assert_eq!(
            smooth_meter_level_db(-24.0, -6.0, Duration::from_millis(10)),
            -6.0
        );
        assert_eq!(
            smooth_meter_level_db(-6.0, -24.0, Duration::from_millis(1_000)),
            smooth_meter_level_db(-6.0, -24.0, Duration::from_millis(100))
        );
    }

    #[test]
    fn meter_ballistics_sanitize_finite_clamped_levels_and_settle() {
        let invalid_current = smooth_meter_level_db(f32::NAN, -6.0, Duration::from_millis(5));
        let invalid_target = smooth_meter_level_db(-6.0, f32::INFINITY, Duration::from_millis(5));
        assert!(invalid_current.is_finite());
        assert!((-120.0..=TARGET_MAX_DB).contains(&invalid_current));
        assert!(invalid_target.is_finite());
        assert!((-120.0..=TARGET_MAX_DB).contains(&invalid_target));
        assert_eq!(sanitize_meter_level_db(6.0), TARGET_MAX_DB);
        assert_eq!(sanitize_meter_level_db(-200.0), -120.0);

        let status = Arc::new(GuiStatus::default());
        status.update(-30.0, -30.0, 0.0, 0.0, MatchState::Measuring);
        let mut state = EditorState::new(
            Arc::new(crate::params::GainSnapParams::new()),
            Arc::new(AutomationQueue::default()),
            status,
            None,
            None,
        );
        state.output_peak_db = -6.0;
        let mut now = state.meter_last_update;
        for _ in 0..100 {
            now += Duration::from_millis(100);
            state.advance_meter_at(now);
        }
        assert!(!state.meter_needs_realtime_redraw());
        assert_eq!(state.output_peak_db, -30.0);
    }

    #[test]
    fn editor_retains_target_meter_focus_during_realtime_refresh() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::clone(&status),
            None,
            None,
        );
        let meter_rect = {
            let plan = editor.paint_plan();
            plan.stroke_rects()
                .find(|stroke| {
                    (stroke.rect.width() - TARGET_METER_TRACK_WIDTH).abs() < f32::EPSILON
                })
                .expect("the target meter track should be painted")
                .rect
        };
        let meter_id = editor
            .paint_plan()
            .stroke_rects()
            .find(|stroke| stroke.rect == meter_rect)
            .expect("the target meter should retain a stable widget id")
            .widget_id;

        editor.dispatch_event(Event::primary_press(meter_rect.center()));
        assert_eq!(editor.runtime.focused_widget(), Some(meter_id));

        status.update(-6.0, -12.0, 0.0, 0.5, MatchState::Measuring);
        assert!(editor.needs_realtime_redraw());
        editor.paint_plan();
        assert_eq!(editor.runtime.focused_widget(), Some(meter_id));

        editor.dispatch_event(Event::primary_release(meter_rect.center()));
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

    #[test]
    fn editor_repaints_when_host_changes_match_parameter() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        let status = Arc::new(GuiStatus::default());
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            status,
            None,
            None,
        );
        let theme = ThemeTokens::default();

        let match_fill_color = |plan: &SurfacePaintPlan| {
            let label = plan
                .first_text_run("MATCH")
                .expect("match label should be painted");
            plan.fill_rects_for_widget(label.widget_id)
                .find(|fill| {
                    (fill.rect.width() - MATCH_BUTTON_WIDTH).abs() < f32::EPSILON
                        && (fill.rect.height() - MATCH_BUTTON_HEIGHT).abs() < f32::EPSILON
                })
                .expect("match button should paint its fill")
                .color
        };

        assert!(!editor.needs_realtime_redraw());
        assert_eq!(match_fill_color(editor.paint_plan()), theme.surface_raised);
        assert!(!editor.needs_realtime_redraw());

        params.set_param(PARAM_MATCH, 1.0);
        assert!(editor.needs_realtime_redraw());
        assert_eq!(match_fill_color(editor.paint_plan()), theme.accent_mint);
        assert!(editor.needs_realtime_redraw());

        params.set_param(PARAM_MATCH, 0.0);
        assert!(editor.needs_realtime_redraw());
        assert_eq!(match_fill_color(editor.paint_plan()), theme.surface_raised);
        assert!(!editor.needs_realtime_redraw());
    }

    #[test]
    fn matching_button_and_indicator_pulse_together_and_stop_when_off() {
        let mut state = editor_state();
        state.params.set_param(PARAM_MATCH, 1.0);
        let start = Instant::now();
        let mut colors = Vec::new();
        for elapsed in [0, 600, 1_200] {
            state.advance_pulse_at(start + Duration::from_millis(elapsed));
            let plan = project_surface(&mut state)
                .frame_at_size(
                    Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                    &ThemeTokens::default(),
                )
                .paint_plan;
            let id = plan.first_text_run("MATCH").unwrap().widget_id;
            let button = plan
                .fill_rects_for_widget(id)
                .find(|fill| fill.rect.width() == MATCH_BUTTON_WIDTH)
                .unwrap()
                .color;
            let dot = plan
                .fill_polygons()
                .find(|polygon| polygon.points.len() == 16)
                .unwrap()
                .color;
            assert_eq!(button, dot);
            colors.push(button);
        }
        assert_ne!(colors[0], colors[1]);
        assert_eq!(colors[0], colors[2]);
        state.params.set_param(PARAM_MATCH, 0.0);
        state.advance_pulse_at(start + Duration::from_secs(2));
        assert_eq!(state.pulse_alpha, 255);
        assert!(state.pulse_started.is_none());
        assert!(!state.advance_pulse_at(start + Duration::from_secs(3)));
    }

    #[test]
    fn match_toggle_keeps_a_press_armed_across_pulse_refresh() {
        let params = Arc::new(crate::params::GainSnapParams::new());
        params.set_param(PARAM_MATCH, 1.0);
        let mut editor = GainSnapEditor::new(
            Arc::clone(&params),
            Arc::new(AutomationQueue::default()),
            Arc::new(GuiStatus::default()),
            None,
            None,
        );
        let plan = editor.paint_plan();
        let id = plan.first_text_run("MATCH").unwrap().widget_id;
        let bounds = plan.first_widget_rect(id).unwrap();
        editor.dispatch_event(Event::primary_press(bounds.center()));
        assert_eq!(editor.runtime.focused_widget(), Some(id));
        editor.runtime.bridge_mut().state_mut().pulse_started =
            Some(Instant::now() - Duration::from_millis(600));
        editor.paint_plan();
        assert_eq!(editor.runtime.focused_widget(), Some(id));
        editor.dispatch_event(Event::primary_release(bounds.center()));
        assert!(!params.match_requested());
        editor.dispatch_key_press(WidgetKey::Space);
        assert!(params.match_requested());
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
        let mut capture = radiant::gui_runtime::OffscreenVelloCapture::new(
            Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
            DpiScale::ONE,
        )
        .expect("Radiant offscreen capture should be available");
        let root = std::env::var_os("TOYBOX_UI_SCREENSHOT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/ui-screenshots"))
            .join("gainsnap");
        std::fs::create_dir_all(&root).expect("screenshot directory should be writable");
        for (name, matching, alpha) in [
            ("initial-ui", false, 255),
            ("matching-bright", true, 255),
            ("matching-dim", true, 166),
        ] {
            state.params.set_param(PARAM_MATCH, f32::from(matching));
            state.pulse_alpha = alpha;
            if matching {
                state
                    .status
                    .update(-6.0, -12.0, -6.0, 0.0, MatchState::Measuring);
                state.output_peak_db = -12.0;
            }
            let plan = project_surface(&mut state)
                .frame_at_size(
                    Vector2::new(WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
                    &ThemeTokens::default(),
                )
                .paint_plan;
            let pixels = capture.capture(&plan).expect("screenshot should render");
            image::save_buffer_with_format(
                root.join(format!("{name}-{WINDOW_WIDTH}x{WINDOW_HEIGHT}.png")),
                &pixels,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                ColorType::Rgba8,
                ImageFormat::Png,
            )
            .expect("screenshot should be written");
        }
    }
}
