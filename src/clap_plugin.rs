//! CLAP entry point for the GainSnap one-shot gain matcher.

use std::sync::Arc;

use toybox::clack_common::plugin::features as plugin_features;
use toybox::clack_extensions::audio_ports::*;
#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
use toybox::clack_extensions::gui::{PluginGui, PluginGuiImpl};
use toybox::clack_extensions::params::*;
use toybox::clack_extensions::state::{PluginState, PluginStateImpl};
use toybox::clack_plugin::stream::{InputStream, OutputStream};
use toybox::clap::automation::{AutomationDrainBuffer, AutomationQueue};
use toybox::clap::params::apply_param_events;
use toybox::clap::prelude::*;
use toybox::clap::process::{min_len, split_channel};
use toybox::clap::state::{read_versioned_payload, write_versioned_payload};

use crate::dsp::GainSnapEngine;
use crate::params::{param_count, text_to_value, value_to_text, write_param_info, GainSnapParams};
use crate::state::{apply_snapshot, decode_payload, encode_payload, STATE_MAGIC, STATE_VERSION};
use crate::status::GuiStatus;

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
                "One-shot peak matching: measure a track, apply gain, and hold the result",
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
        let versioned = read_versioned_payload(input, STATE_MAGIC, &[STATE_VERSION])?;
        let snapshot = decode_payload(&versioned.payload)
            .ok_or(PluginError::Message("Invalid GainSnap state payload"))?;
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
            automation_drain: AutomationDrainBuffer::default(),
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        apply_param_events(events.input, |param_id, value| {
            self.shared.params.set_param(param_id, value as f32);
        });

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

        for frame in 0..frames {
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
                self.engine
                    .process_frame(&self.shared.params, input_left, input_right);
            if let Some(buffer) = left_output.as_deref_mut() {
                buffer[frame] = output_left;
            }
            if let Some(buffer) = right_output.as_deref_mut() {
                buffer[frame] = output_right;
            }
        }
    }
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
