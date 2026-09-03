//! Shared VST3 endpoint state and bounded automation storage.

use std::sync::{Arc, Mutex};

use toybox::clack_plugin::utils::ClapId;
use toybox::vst3::prelude::ComPtr;
use toybox::vst3::prelude::Steinberg::Vst::IComponentHandler;
use toybox::vst3::prelude::Steinberg::{kResultFalse, kResultOk, tresult};

use crate::dsp::GainSnapEngine;
use crate::params::GainSnapParams;
use crate::status::GuiStatus;

/// Maximum parameter points retained by one VST3 process block.
pub(super) const PARAM_EVENT_CAPACITY: usize = 256;
/// Number of stable parameters used by overflow convergence storage.
pub(super) const PARAMETER_COUNT: usize = 3;

/// One normalized-to-plain parameter point scheduled by a VST3 host.
#[derive(Clone, Copy, Debug)]
pub(super) struct ParamEvent {
    /// Sample offset relative to the current process block.
    pub(super) offset: usize,
    /// Source order used to make equal-offset points deterministic.
    pub(super) sequence: usize,
    /// Shared CLAP/VST3 parameter identity.
    pub(super) param_id: ClapId,
    /// Canonical plain parameter value.
    pub(super) value: f32,
}

/// Fixed-capacity, allocation-free process-block parameter timeline.
pub(super) struct ParamTimeline {
    events: Vec<ParamEvent>,
    overflow_final: [Option<ParamEvent>; PARAMETER_COUNT],
    block_frames: usize,
    next_event: usize,
    next_sequence: usize,
}

impl ParamTimeline {
    /// Construct all process-time storage before audio starts.
    pub(super) fn new() -> Self {
        Self {
            events: Vec::with_capacity(PARAM_EVENT_CAPACITY),
            overflow_final: [None; PARAMETER_COUNT],
            block_frames: 0,
            next_event: 0,
            next_sequence: 0,
        }
    }

    /// Start collecting one process block.
    pub(super) fn begin_block(&mut self, block_frames: usize) {
        self.events.clear();
        self.overflow_final = [None; PARAMETER_COUNT];
        self.block_frames = block_frames;
        self.next_event = 0;
        self.next_sequence = 0;
    }

    /// Add one point without growing process-time storage.
    pub(super) fn push(&mut self, offset: i32, mut event: ParamEvent) {
        let Some(index) = event
            .param_id
            .get()
            .checked_sub(1)
            .and_then(|id| usize::try_from(id).ok())
            .filter(|index| *index < PARAMETER_COUNT)
        else {
            return;
        };
        event.offset = usize::try_from(offset.max(0))
            .unwrap_or(self.block_frames)
            .min(self.block_frames);
        event.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        if self.events.len() < PARAM_EVENT_CAPACITY {
            self.events.push(event);
        } else {
            self.overflow_final[index] = Some(event);
        }
    }

    /// Sort retained points into chronological order.
    pub(super) fn prepare(&mut self) {
        self.events
            .sort_unstable_by_key(|event| (event.offset, event.sequence));
        self.next_event = 0;
    }

    /// Return one retained event by index.
    #[cfg(test)]
    pub(super) fn get(&self, index: usize) -> Option<ParamEvent> {
        self.events.get(index).copied()
    }

    /// Return the number of retained events.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.events.len()
    }

    /// Apply all points at or before a sample offset.
    pub(super) fn apply_through(&mut self, offset: usize, params: &GainSnapParams) {
        while let Some(event) = self.events.get(self.next_event).copied() {
            if event.offset > offset {
                break;
            }
            params.set_param(event.param_id, event.value);
            self.next_event = self.next_event.saturating_add(1);
        }
    }

    /// Apply all remaining points and the bounded overflow convergence values.
    pub(super) fn apply_remaining(&mut self, params: &GainSnapParams) {
        self.apply_through(self.block_frames, params);
        self.apply_overflow(params);
    }

    /// Apply overflow final values so the next block starts converged.
    pub(super) fn apply_overflow(&self, params: &GainSnapParams) {
        for event in self.overflow_final.iter().flatten() {
            params.set_param(event.param_id, event.value);
        }
    }
}

/// State shared by one VST3 processor/controller pair and its editor.
pub(super) struct GainSnapVst3Shared {
    /// Atomic parameters read by the realtime engine.
    pub(super) params: Arc<GainSnapParams>,
    /// Realtime metering consumed by the editor.
    pub(super) status: Arc<GuiStatus>,
}

impl GainSnapVst3Shared {
    /// Construct one unconnected VST3 endpoint's initial state.
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            params: Arc::new(GainSnapParams::new()),
            status: Arc::new(GuiStatus::default()),
        })
    }
}

/// Host-owned component handler retained for one VST3 controller lifetime.
///
/// This intentionally lives outside [`GainSnapVst3Shared`]. The controller
/// adopts the processor's shared state during `IConnectionPoint::connect`,
/// which can happen after the host installs its component handler.
pub(super) struct ComponentHandlerOwner {
    handler: Mutex<Option<ComPtr<IComponentHandler>>>,
}

impl Default for ComponentHandlerOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentHandlerOwner {
    /// Construct an empty handler slot.
    pub(super) fn new() -> Self {
        Self {
            handler: Mutex::new(None),
        }
    }

    /// Retain the host handler, releasing the previous one if present.
    pub(super) unsafe fn set(&self, handler: *mut IComponentHandler) -> tresult {
        let Ok(mut slot) = self.handler.lock() else {
            return kResultFalse;
        };
        if handler.is_null() {
            *slot = None;
            return kResultOk;
        }

        // SAFETY: the host owns a valid handler pointer for this synchronous
        // callback. Retain one reference before placing it in the ComPtr.
        unsafe { ((*(*handler).vtbl).base.addRef)(handler.cast()) };
        // SAFETY: the addRef above transfers one owned reference into `slot`.
        *slot = unsafe { ComPtr::from_raw(handler) };
        kResultOk
    }

    /// Clone one handler reference without holding the mutex during host code.
    pub(super) fn clone_handler(&self) -> Option<ComPtr<IComponentHandler>> {
        let guard = self.handler.lock().ok()?;
        let handler = guard.as_ref()?;
        let pointer = handler.as_ptr();
        // SAFETY: the handler is retained by `guard` until this clone owns its
        // additional reference; callers invoke it only after the lock drops.
        unsafe { ((*(*pointer).vtbl).base.addRef)(pointer.cast()) };
        // SAFETY: the addRef above transfers one owned reference to the clone.
        unsafe { ComPtr::from_raw(pointer) }
    }
}

/// Audio-owned DSP runtime published through Toybox's bounded handoff API.
pub(super) struct GainSnapVst3Runtime {
    /// Format-neutral GainSnap engine.
    pub(super) engine: GainSnapEngine,
    /// Per-block VST3 automation storage.
    pub(super) timeline: ParamTimeline,
}

impl GainSnapVst3Runtime {
    /// Build a complete runtime on a non-audio lifecycle callback.
    pub(super) fn new(params: &GainSnapParams, sample_rate: f32) -> Self {
        Self {
            engine: GainSnapEngine::new(sample_rate, params.locked_gain_db()),
            timeline: ParamTimeline::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(param_id: u32, value: f32) -> ParamEvent {
        ParamEvent {
            offset: 0,
            sequence: 0,
            param_id: ClapId::new(param_id),
            value,
        }
    }

    #[test]
    fn timeline_clamps_offsets_and_sorts_points() {
        let mut timeline = ParamTimeline::new();
        timeline.begin_block(64);
        timeline.push(64, event(1, 1.0));
        timeline.push(-1, event(2, 2.0));
        timeline.push(12, event(3, 3.0));
        timeline.prepare();

        assert_eq!(timeline.get(0).map(|item| item.offset), Some(0));
        assert_eq!(timeline.get(1).map(|item| item.offset), Some(12));
        assert_eq!(timeline.get(2).map(|item| item.offset), Some(64));
    }

    #[test]
    fn timeline_overflow_retains_the_last_value_per_parameter() {
        let mut timeline = ParamTimeline::new();
        timeline.begin_block(8);
        for value in 0..=PARAM_EVENT_CAPACITY {
            let value = if value == PARAM_EVENT_CAPACITY {
                0.75
            } else {
                0.0
            };
            timeline.push(2, event(3, value));
        }

        assert_eq!(timeline.len(), PARAM_EVENT_CAPACITY);
        let params = GainSnapParams::new();
        timeline.apply_overflow(&params);
        assert_eq!(params.get_param(ClapId::new(3)), Some(0.75));
    }
}
