//! VST3 component-handler bridge for the shared Radiant editor.

use std::sync::Arc;

use toybox::clack_plugin::utils::ClapId;
use toybox::clap::automation::{AutomationConfig, AutomationQueue};
use toybox::vst3::prelude::Steinberg::Vst::IComponentHandler;
use toybox::vst3::prelude::*;

use crate::gui::HostParamEditSink;

use super::param_bridge;
use super::shared_state::{ComponentHandlerOwner, GainSnapVst3Shared};

/// Forwards Radiant gestures to the VST3 host's component handler.
pub(super) struct Vst3HostParamEditSink {
    component_handler: Arc<ComponentHandlerOwner>,
}

impl Vst3HostParamEditSink {
    fn new(component_handler: Arc<ComponentHandlerOwner>) -> Self {
        Self { component_handler }
    }

    fn handler(&self) -> Option<ComPtr<IComponentHandler>> {
        self.component_handler.clone_handler()
    }

    fn enabled(config: &AutomationConfig, param_id: ClapId) -> bool {
        config.is_enabled(param_id)
    }
}

impl HostParamEditSink for Vst3HostParamEditSink {
    fn gesture_started(&self, config: &AutomationConfig, param_id: ClapId) {
        if !Self::enabled(config, param_id) {
            return;
        }
        let Some(handler) = self.handler() else {
            return;
        };
        unsafe {
            let _ = handler.beginEdit(param_id.get());
        }
    }

    fn gesture_value(&self, config: &AutomationConfig, param_id: ClapId, value: f64) {
        if !Self::enabled(config, param_id) {
            return;
        }
        let Some(normalized) = param_bridge::to_normalized(param_id.get(), value) else {
            return;
        };
        let Some(handler) = self.handler() else {
            return;
        };
        unsafe {
            let _ = handler.performEdit(param_id.get(), normalized);
        }
    }

    fn gesture_ended(&self, config: &AutomationConfig, param_id: ClapId) {
        if !Self::enabled(config, param_id) {
            return;
        }
        let Some(handler) = self.handler() else {
            return;
        };
        unsafe {
            let _ = handler.endEdit(param_id.get());
        }
    }
}

/// Construct the shared native host directly so keyboard delivery, focus,
/// and Windows DPI conversion retain Toybox's complete VST3 contract.
pub(super) fn new_gui(
    shared: Arc<GainSnapVst3Shared>,
    component_handler: Arc<ComponentHandlerOwner>,
) -> toybox::radiant_gui::RadiantHostedGui {
    let sink = Arc::new(Vst3HostParamEditSink::new(component_handler));
    let editor = crate::gui::GainSnapEditor::new(
        Arc::clone(&shared.params),
        Arc::new(AutomationQueue::default()),
        Arc::clone(&shared.status),
        None,
        Some(sink),
    );
    toybox::radiant_gui::RadiantHostedGui::new(
        "GainSnapRadiantVst3EditorView",
        editor,
        crate::gui::WINDOW_WIDTH,
        crate::gui::WINDOW_HEIGHT,
    )
    .with_size_contract(
        (crate::gui::MIN_WINDOW_WIDTH, crate::gui::MIN_WINDOW_HEIGHT),
        (crate::gui::WINDOW_WIDTH, crate::gui::WINDOW_HEIGHT),
        (crate::gui::MAX_WINDOW_WIDTH, crate::gui::MAX_WINDOW_HEIGHT),
    )
}
