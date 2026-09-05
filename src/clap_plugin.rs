//! CLAP entry point for the GainSnap toggle gain matcher.

use std::sync::Arc;

use toybox::clack_common::plugin::features as plugin_features;
use toybox::clack_extensions::audio_ports::*;
#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
use toybox::clack_extensions::gui::{PluginGui, PluginGuiImpl};
use toybox::clack_extensions::params::*;
use toybox::clack_extensions::state::{PluginState, PluginStateImpl};
use toybox::clack_plugin::events::UnknownEvent;
use toybox::clack_plugin::stream::{InputStream, OutputStream};
use toybox::clap::automation::{AutomationDrainBuffer, AutomationQueue};
use toybox::clap::params::apply_param_events;
use toybox::clap::prelude::*;
use toybox::clap::process::{min_len, split_channel};
use toybox::clap::state::{read_versioned_payload, write_versioned_payload};
use toybox::events::{BlockEvent, BlockEventTimeline};

use crate::dsp::GainSnapEngine;
use crate::params::{
    param_count, text_to_value, value_to_text, write_param_info, GainSnapParams,
    PARAM_LOCKED_GAIN_DB, PARAM_MATCH, PARAM_TARGET_DB,
};
use crate::state::{
    apply_snapshot, decode_payload, encode_payload, StateSnapshot, ACCEPTED_STATE_VERSIONS,
    STATE_MAGIC, STATE_VERSION,
};
use crate::status::GuiStatus;

/// Maximum number of CLAP parameter events retained for one process block.
const CLAP_PARAMETER_EVENT_CAPACITY: usize = 256;
/// Number of stable CLAP parameters with bounded overflow convergence slots.
const CLAP_PARAMETER_COUNT: usize = 3;

fn classify_clap_parameter(event: &UnknownEvent) -> Option<BlockEvent<(ClapId, f32), ()>> {
    match event.as_core_event() {
        Some(event_spaces::CoreEventSpace::ParamValue(param)) => {
            let param_id = param.param_id()?;
            clap_parameter_index(param_id)?;
            Some(BlockEvent::Parameter((param_id, param.value() as f32)))
        }
        _ => None,
    }
}

fn clap_parameter_index(param_id: ClapId) -> Option<usize> {
    match param_id {
        PARAM_TARGET_DB => Some(0),
        PARAM_MATCH => Some(1),
        PARAM_LOCKED_GAIN_DB => Some(2),
        _ => None,
    }
}

/// Collect CLAP parameter events into the reusable timeline and retain the
/// final value for each parameter whose event overflowed the timeline.
fn collect_clap_timeline_with_overflow(
    input: &InputEvents<'_>,
    frame_count: usize,
    timeline: &mut BlockEventTimeline<(ClapId, f32), ()>,
    overflow_final: &mut [Option<(ClapId, f32)>; CLAP_PARAMETER_COUNT],
) {
    timeline.begin_block(frame_count);
    *overflow_final = [None; CLAP_PARAMETER_COUNT];
    for event in input {
        let Some(payload) = classify_clap_parameter(event) else {
            continue;
        };
        let status = timeline.push(i64::from(event.header().time()), payload);
        if status.overflowed() {
            let BlockEvent::Parameter((param_id, value)) = payload else {
                continue;
            };
            if let Some(index) = clap_parameter_index(param_id) {
                overflow_final[index] = Some((param_id, value));
            }
        }
    }
    timeline.prepare();
}

fn decode_clap_state_payload(version: u32, payload: &[u8]) -> Result<StateSnapshot, PluginError> {
    decode_payload(version, payload).ok_or(PluginError::Message("Invalid GainSnap state payload"))
}

/// CLAP plug-in type for GainSnap.
pub struct GainSnapPlugin;

impl Plugin for GainSnapPlugin {
    type AudioProcessor<'a> = GainSnapAudioProcessor<'a>;
    type Shared<'a> = GainSnapShared;
    type MainThread<'a> = GainSnapMainThread<'a>;

    fn declare_extensions(
        builder: &mut PluginExtensions<Self>,
        _shared: Option<&Self::Shared<'_>>,
    ) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginParams>()
            .register::<PluginState>();
        #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
        builder.register::<PluginGui>();
    }
}

impl DefaultPluginFactory for GainSnapPlugin {
    fn get_descriptor() -> PluginDescriptor {
        PluginDescriptor::new("com.portalsurfer.gainsnap", "GainSnap")
            .with_vendor("PORTALSURFER")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_description(
                "Peak matching toggle with one-click 0 dBFS normalization and a smoothed realtime orange output meter: measure and apply gain while enabled, then hold the result when disabled",
            )
            .with_features([plugin_features::AUDIO_EFFECT, plugin_features::STEREO])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<Self::Shared<'_>, PluginError> {
        Ok(GainSnapShared {
            params: Arc::new(GainSnapParams::new()),
            status: Arc::new(GuiStatus::default()),
            automation_queue: Arc::new(AutomationQueue::default()),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a Self::Shared<'a>,
    ) -> Result<Self::MainThread<'a>, PluginError> {
        #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
        {
            let param_requester = host_param_requester(host.shared());
            Ok(GainSnapMainThread {
                shared,
                gui: crate::gui::new_gui(
                    Arc::clone(&shared.params),
                    Arc::clone(&shared.automation_queue),
                    Arc::clone(&shared.status),
                    param_requester,
                ),
                automation_drain: AutomationDrainBuffer::default(),
            })
        }
        #[cfg(not(all(target_os = "macos", feature = "radiant-gui")))]
        {
            let _ = host;
            Ok(GainSnapMainThread {
                shared,
                automation_drain: AutomationDrainBuffer::default(),
            })
        }
    }
}

/// Shared lock-free state owned by one CLAP plug-in instance.
pub struct GainSnapShared {
    /// Atomic parameter values read by the audio thread.
    pub(crate) params: Arc<GainSnapParams>,
    /// Most recent realtime metering and matcher status.
    pub(crate) status: Arc<GuiStatus>,
    /// GUI gestures waiting for the host's parameter flush callback.
    pub(crate) automation_queue: Arc<AutomationQueue>,
}

impl PluginShared<'_> for GainSnapShared {}

/// Main-thread state for CLAP host interaction and the optional editor.
pub struct GainSnapMainThread<'a> {
    shared: &'a GainSnapShared,
    #[cfg(all(target_os = "macos", feature = "radiant-gui"))]
    gui: toybox::radiant_gui::RadiantHostedGui,
    automation_drain: AutomationDrainBuffer,
}

impl<'a> PluginMainThread<'a, GainSnapShared> for GainSnapMainThread<'a> {}

impl PluginAudioPortsImpl for GainSnapMainThread<'_> {
    fn count(&mut self, _is_input: bool) -> u32 {
        1
    }

    fn get(&mut self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index != 0 {
            return;
        }
        writer.set(&AudioPortInfo {
            id: ClapId::new(0),
            name: b"main",
            channel_count: 2,
            flags: AudioPortFlags::IS_MAIN,
            port_type: Some(AudioPortType::STEREO),
            in_place_pair: None,
        });
    }
}

impl PluginMainThreadParams for GainSnapMainThread<'_> {
    fn count(&mut self) -> u32 {
        param_count()
    }

    fn get_info(&mut self, param_index: u32, writer: &mut ParamInfoWriter) {
        write_param_info(param_index, writer);
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        self.shared.params.get_param(param_id).map(f64::from)
    }

    fn value_to_text(
        &mut self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        value_to_text(param_id, value, writer)
    }

    fn text_to_value(&mut self, param_id: ClapId, text: &std::ffi::CStr) -> Option<f64> {
        text_to_value(param_id, text)
    }

    fn flush(
        &mut self,
        input_parameter_changes: &InputEvents,
        output_parameter_changes: &mut OutputEvents,
    ) {
        apply_param_events(input_parameter_changes, |param_id, value| {
            self.shared.params.set_param(param_id, value as f32);
        });
        let _ = self
            .automation_drain
            .drain(&self.shared.automation_queue, output_parameter_changes);
    }
}

impl PluginStateImpl for GainSnapMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        let payload = encode_payload(&self.shared.params);
        write_versioned_payload(output, STATE_MAGIC, STATE_VERSION, &payload)
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let versioned = read_versioned_payload(input, STATE_MAGIC, ACCEPTED_STATE_VERSIONS)?;
        let snapshot = decode_clap_state_payload(versioned.version, &versioned.payload)?;
        apply_snapshot(&self.shared.params, snapshot);
        Ok(())
    }
}

#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
impl PluginGuiImpl for GainSnapMainThread<'_> {
    toybox::radiant_clap_gui_callbacks!(
        gui = gui,
        preferred_size = crate::gui::preferred_window_size,
        show = |_main_thread| Ok(())
    );
}

/// Audio-thread processor for GainSnap.
pub struct GainSnapAudioProcessor<'a> {
    shared: &'a GainSnapShared,
    engine: GainSnapEngine,
    timeline: BlockEventTimeline<(ClapId, f32), ()>,
    overflow_final: [Option<(ClapId, f32)>; CLAP_PARAMETER_COUNT],
    automation_drain: AutomationDrainBuffer,
}

impl<'a> PluginAudioProcessor<'a, GainSnapShared, GainSnapMainThread<'a>>
    for GainSnapAudioProcessor<'a>
{
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut GainSnapMainThread<'a>,
        shared: &'a GainSnapShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            shared,
            engine: GainSnapEngine::new(
                audio_config.sample_rate as f32,
                shared.params.locked_gain_db(),
            ),
            timeline: BlockEventTimeline::with_capacity(CLAP_PARAMETER_EVENT_CAPACITY),
            overflow_final: [None; CLAP_PARAMETER_COUNT],
            automation_drain: AutomationDrainBuffer::default(),
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let frames = audio.frames_count() as usize;
        collect_clap_timeline_with_overflow(
            events.input,
            frames,
            &mut self.timeline,
            &mut self.overflow_final,
        );

        self.engine.begin_block(&self.shared.params);
        let mut processed_main = false;
        for mut port_pair in &mut audio {
            if processed_main {
                break;
            }
            let Some(mut channels) = port_pair.channels()?.into_f32() else {
                continue;
            };
            let mut channel_iter = channels.iter_mut();
            let Some(left) = channel_iter.next() else {
                continue;
            };
            let Some(right) = channel_iter.next() else {
                continue;
            };
            self.process_stereo_pair(left, right);
            processed_main = true;
        }
        self.apply_remaining_timeline();

        let report = self.engine.report();
        self.shared.status.update(
            report.input_peak_db,
            report.output_peak_db,
            report.locked_gain_db,
            report.progress,
            report.state,
        );

        let _ = self
            .automation_drain
            .drain(&self.shared.automation_queue, events.output);
        Ok(ProcessStatus::Continue)
    }
}

impl GainSnapAudioProcessor<'_> {
    fn apply_remaining_timeline(&mut self) {
        while let Some(batch) = self.timeline.next_batch() {
            for scheduled in batch.events() {
                if let BlockEvent::Parameter((param_id, value)) = scheduled.event() {
                    self.shared.params.set_param(*param_id, *value);
                }
            }
            self.engine.sync_controls(&self.shared.params);
        }
        apply_clap_overflow_final(&self.overflow_final, &self.shared.params, &mut self.engine);
    }

    fn process_stereo_pair(&mut self, left: ChannelPair<'_, f32>, right: ChannelPair<'_, f32>) {
        let (left_input, mut left_output, left_in_place) = split_channel(left);
        let (right_input, mut right_output, right_in_place) = split_channel(right);
        let frames = min_len(&[
            left_input.map(|buffer| buffer.len()),
            right_input.map(|buffer| buffer.len()),
            left_output.as_ref().map(|buffer| buffer.len()),
            right_output.as_ref().map(|buffer| buffer.len()),
        ]);
        let Some(frames) = frames else {
            return;
        };

        let params = &self.shared.params;
        let engine = &mut self.engine;
        let timeline = &mut self.timeline;
        let mut frame_start = 0;
        while let Some(batch) = timeline.next_batch() {
            let batch_end = batch.sample_offset().min(frames);
            for frame in frame_start..batch_end {
                let input_left = if left_in_place {
                    left_output
                        .as_deref()
                        .and_then(|buffer| buffer.get(frame))
                        .copied()
                        .unwrap_or(0.0)
                } else {
                    left_input
                        .and_then(|buffer| buffer.get(frame))
                        .copied()
                        .unwrap_or(0.0)
                };
                let input_right = if right_in_place {
                    right_output
                        .as_deref()
                        .and_then(|buffer| buffer.get(frame))
                        .copied()
                        .unwrap_or(0.0)
                } else {
                    right_input
                        .and_then(|buffer| buffer.get(frame))
                        .copied()
                        .unwrap_or(0.0)
                };
                let (output_left, output_right) =
                    engine.process_frame(params, input_left, input_right);
                if let Some(buffer) = left_output.as_deref_mut() {
                    buffer[frame] = output_left;
                }
                if let Some(buffer) = right_output.as_deref_mut() {
                    buffer[frame] = output_right;
                }
            }
            for scheduled in batch.events() {
                if let BlockEvent::Parameter((param_id, value)) = scheduled.event() {
                    params.set_param(*param_id, *value);
                }
            }
            engine.sync_controls(params);
            frame_start = batch_end;
        }
        for frame in frame_start..frames {
            let input_left = if left_in_place {
                left_output
                    .as_deref()
                    .and_then(|buffer| buffer.get(frame))
                    .copied()
                    .unwrap_or(0.0)
            } else {
                left_input
                    .and_then(|buffer| buffer.get(frame))
                    .copied()
                    .unwrap_or(0.0)
            };
            let input_right = if right_in_place {
                right_output
                    .as_deref()
                    .and_then(|buffer| buffer.get(frame))
                    .copied()
                    .unwrap_or(0.0)
            } else {
                right_input
                    .and_then(|buffer| buffer.get(frame))
                    .copied()
                    .unwrap_or(0.0)
            };
            let (output_left, output_right) = engine.process_frame(params, input_left, input_right);
            if let Some(buffer) = left_output.as_deref_mut() {
                buffer[frame] = output_left;
            }
            if let Some(buffer) = right_output.as_deref_mut() {
                buffer[frame] = output_right;
            }
        }
    }
}

fn apply_clap_overflow_final(
    overflow_final: &[Option<(ClapId, f32)>; CLAP_PARAMETER_COUNT],
    params: &GainSnapParams,
    engine: &mut GainSnapEngine,
) {
    for event in overflow_final.iter().flatten() {
        params.set_param(event.0, event.1);
    }
    engine.sync_controls(params);
}

impl PluginAudioProcessorParams for GainSnapAudioProcessor<'_> {
    fn flush(
        &mut self,
        input_parameter_changes: &InputEvents,
        output_parameter_changes: &mut OutputEvents,
    ) {
        apply_param_events(input_parameter_changes, |param_id, value| {
            self.shared.params.set_param(param_id, value as f32);
        });
        let _ = self
            .automation_drain
            .drain(&self.shared.automation_queue, output_parameter_changes);
    }
}

/// Helper for requesting host-side parameter flushes from the CLAP editor.
#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
#[derive(Clone, Copy)]
pub(crate) struct HostParamRequester {
    host: HostSharedHandle<'static>,
    params: HostParams,
}

#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
impl HostParamRequester {
    /// Ask the host to flush queued editor gestures.
    pub(crate) fn request_flush(self) {
        self.params.request_flush(&self.host);
    }
}

#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
fn host_param_requester(host: HostSharedHandle<'_>) -> Option<HostParamRequester> {
    let params = host.get_extension::<HostParams>()?;
    let host =
        unsafe { std::mem::transmute::<HostSharedHandle<'_>, HostSharedHandle<'static>>(host) };
    Some(HostParamRequester { host, params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{encode_payload, LEGACY_STATE_VERSION};
    use crate::status::MatchState;
    use std::io::Cursor;
    use toybox::clack_plugin::events::event_types::ParamValueEvent;
    use toybox::clack_plugin::events::io::InputEvents;
    use toybox::clack_plugin::events::Pckn;
    use toybox::clack_plugin::stream::{InputStream, OutputStream};
    use toybox::clack_plugin::utils::Cookie;
    use toybox::clap::events::collect_clap_timeline;
    use toybox::clap::state::{read_versioned_payload, write_versioned_payload};

    fn parameter_event(time: u32, value: f64) -> ParamValueEvent {
        ParamValueEvent::new(
            time,
            crate::params::PARAM_MATCH,
            Pckn::match_all(),
            value,
            Cookie::empty(),
        )
    }

    #[test]
    fn clap_v1_state_migrates_match_now_to_off_and_preserves_gain() {
        let params = GainSnapParams::new();
        params.set_param(crate::params::PARAM_TARGET_DB, -7.5);
        params.set_param(crate::params::PARAM_MATCH, 1.0);
        params.set_param(crate::params::PARAM_LOCKED_GAIN_DB, 5.25);
        let payload = encode_payload(&params);

        let mut encoded = Vec::new();
        {
            let mut output = OutputStream::from_writer(&mut encoded);
            write_versioned_payload(&mut output, STATE_MAGIC, LEGACY_STATE_VERSION, &payload)
                .expect("legacy CLAP state should encode");
        }
        let mut cursor = Cursor::new(encoded);
        let mut input = InputStream::from_reader(&mut cursor);
        let versioned = read_versioned_payload(&mut input, STATE_MAGIC, ACCEPTED_STATE_VERSIONS)
            .expect("legacy CLAP state should be accepted");

        let snapshot = decode_clap_state_payload(versioned.version, &versioned.payload)
            .expect("legacy CLAP state should migrate");
        apply_snapshot(&params, snapshot);

        assert_eq!(params.target_db(), -7.5);
        assert!(!params.match_requested());
        assert_eq!(params.locked_gain_db(), 5.25);
    }

    fn run_timeline(
        timeline: &mut BlockEventTimeline<(ClapId, f32), ()>,
        params: &GainSnapParams,
        engine: &mut GainSnapEngine,
        samples: &[f32],
    ) {
        engine.begin_block(params);
        let mut frame = 0;
        while let Some(batch) = timeline.next_batch() {
            let batch_frame = batch.sample_offset().min(samples.len());
            for sample in samples.iter().take(batch_frame).skip(frame) {
                let _ = engine.process_frame(params, *sample, *sample);
            }
            for scheduled in batch.events() {
                if let BlockEvent::Parameter((param_id, value)) = scheduled.event() {
                    params.set_param(*param_id, *value);
                }
            }
            engine.sync_controls(params);
            frame = batch_frame;
        }
        for sample in samples.iter().skip(frame) {
            let _ = engine.process_frame(params, *sample, *sample);
        }
    }

    #[test]
    fn clap_off_event_at_nonzero_offset_finalizes_prior_frames_only() {
        let source = [parameter_event(0, 1.0), parameter_event(512, 0.0)];
        let input = InputEvents::from_buffer(&source);
        let mut timeline = BlockEventTimeline::with_capacity(CLAP_PARAMETER_EVENT_CAPACITY);
        collect_clap_timeline(&input, 1024, &mut timeline, classify_clap_parameter);
        assert_eq!(
            timeline
                .events()
                .iter()
                .map(|event| event.sample_offset())
                .collect::<Vec<_>>(),
            vec![0, 512]
        );

        let mut samples = [0.25_f32; 1024];
        samples[512..].fill(0.9);
        let params = GainSnapParams::new();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        run_timeline(&mut timeline, &params, &mut engine, &samples);

        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
    }

    #[test]
    fn clap_on_event_at_nonzero_offset_starts_measurement_there() {
        let source = [parameter_event(512, 1.0), parameter_event(1024, 0.0)];
        let input = InputEvents::from_buffer(&source);
        let mut timeline = BlockEventTimeline::with_capacity(CLAP_PARAMETER_EVENT_CAPACITY);
        collect_clap_timeline(&input, 1024, &mut timeline, classify_clap_parameter);
        assert_eq!(
            timeline
                .events()
                .iter()
                .map(|event| event.sample_offset())
                .collect::<Vec<_>>(),
            vec![512, 1024]
        );

        let mut samples = [0.9_f32; 1024];
        samples[512..].fill(0.25);
        let params = GainSnapParams::new();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        run_timeline(&mut timeline, &params, &mut engine, &samples);

        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
    }

    #[test]
    fn clap_overflow_match_off_converges_before_next_block() {
        let mut source = Vec::with_capacity(CLAP_PARAMETER_EVENT_CAPACITY + 1);
        for _ in 0..CLAP_PARAMETER_EVENT_CAPACITY {
            source.push(parameter_event(0, 1.0));
        }
        source.push(parameter_event(1024, 0.0));
        let input = InputEvents::from_buffer(&source);
        let mut timeline = BlockEventTimeline::with_capacity(CLAP_PARAMETER_EVENT_CAPACITY);
        let mut overflow_final = [None; CLAP_PARAMETER_COUNT];
        collect_clap_timeline_with_overflow(&input, 1024, &mut timeline, &mut overflow_final);

        assert_eq!(timeline.events().len(), CLAP_PARAMETER_EVENT_CAPACITY);
        assert_eq!(
            overflow_final[clap_parameter_index(PARAM_MATCH).expect("known parameter")],
            Some((PARAM_MATCH, 0.0))
        );

        let params = GainSnapParams::new();
        let mut engine = GainSnapEngine::new(48_000.0, 0.0);
        run_timeline(&mut timeline, &params, &mut engine, &[0.25; 1024]);
        apply_clap_overflow_final(&overflow_final, &params, &mut engine);

        assert_eq!(engine.report().state, MatchState::Locked);
        assert!((engine.report().locked_gain_db - 0.0412).abs() < 0.02);
        engine.begin_block(&params);
        assert_eq!(engine.report().state, MatchState::Locked);
    }
}
