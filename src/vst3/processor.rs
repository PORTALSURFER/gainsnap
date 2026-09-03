//! VST3 processor and hardened stereo buffer boundary.

use std::cell::UnsafeCell;
use std::mem::{align_of, size_of};
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use toybox::vst3::prelude::Steinberg::*;
use toybox::vst3::prelude::*;

use super::param_bridge;
use super::shared_state::{
    GainSnapVst3Runtime, GainSnapVst3Shared, ParamEvent, PARAMETER_COUNT, PARAM_EVENT_CAPACITY,
};
use super::CONTROLLER_CID;
use crate::params::GainSnapParams;

/// Exclusive audio-side borrow of Toybox's VST3 runtime owner.
struct RealtimeRuntime {
    inner: UnsafeCell<AudioRuntime<GainSnapVst3Runtime>>,
    in_process: AtomicBool,
}

// SAFETY: the atomic flag grants at most one mutable runtime borrow. Lifecycle
// callbacks publish replacements and never access the audio-owned runtime.
unsafe impl Sync for RealtimeRuntime {}
unsafe impl Send for RealtimeRuntime {}

impl RealtimeRuntime {
    fn new(runtime: AudioRuntime<GainSnapVst3Runtime>) -> Self {
        Self {
            inner: UnsafeCell::new(runtime),
            in_process: AtomicBool::new(false),
        }
    }

    fn try_acquire(&self) -> Option<RealtimeRuntimeGuard<'_>> {
        self.in_process
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| RealtimeRuntimeGuard { owner: self })
    }
}

struct RealtimeRuntimeGuard<'a> {
    owner: &'a RealtimeRuntime,
}

impl Deref for RealtimeRuntimeGuard<'_> {
    type Target = AudioRuntime<GainSnapVst3Runtime>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: successful guard acquisition gives this guard exclusive access.
        unsafe { &*self.owner.inner.get() }
    }
}

impl DerefMut for RealtimeRuntimeGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: see the `Deref` implementation above.
        unsafe { &mut *self.owner.inner.get() }
    }
}

impl Drop for RealtimeRuntimeGuard<'_> {
    fn drop(&mut self) {
        self.owner.in_process.store(false, Ordering::Release);
    }
}

/// Validated stereo f32 pointers for one VST3 process block.
#[derive(Clone, Copy)]
struct RawStereoBuffers {
    frames: usize,
    input_left: *const f32,
    input_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    input_silence_flags: uint64,
}

fn address_range<T>(pointer: *const T, count: usize) -> Option<(usize, usize)> {
    if pointer.is_null() || !(pointer as usize).is_multiple_of(align_of::<T>()) {
        return None;
    }
    let bytes = count.checked_mul(size_of::<T>())?;
    let start = pointer as usize;
    Some((start, start.checked_add(bytes)?))
}

fn ranges_overlap(first: (usize, usize), second: (usize, usize)) -> bool {
    first.0 < second.1 && second.0 < first.1
}

fn validate_stereo_aliases(
    input_left: *const f32,
    input_right: *const f32,
    output_left: *mut f32,
    output_right: *mut f32,
    frames: usize,
) -> bool {
    let Some(input_left_range) = address_range(input_left, frames) else {
        return false;
    };
    let Some(input_right_range) = address_range(input_right, frames) else {
        return false;
    };
    let Some(output_left_range) = address_range(output_left, frames) else {
        return false;
    };
    let Some(output_right_range) = address_range(output_right, frames) else {
        return false;
    };

    if ranges_overlap(input_left_range, input_right_range)
        || ranges_overlap(output_left_range, output_right_range)
        || ranges_overlap(output_left_range, input_right_range)
        || ranges_overlap(output_right_range, input_left_range)
    {
        return false;
    }

    let left_overlap = ranges_overlap(output_left_range, input_left_range);
    let right_overlap = ranges_overlap(output_right_range, input_right_range);
    (!left_overlap || ptr::eq(output_left as *const f32, input_left))
        && (!right_overlap || ptr::eq(output_right as *const f32, input_right))
}

/// Read exactly one stereo f32 input and output bus after validating pointers.
unsafe fn raw_stereo_buffers(data: &ProcessData) -> Option<RawStereoBuffers> {
    if data.numInputs != 1
        || data.numOutputs != 1
        || data.inputs.is_null()
        || data.outputs.is_null()
        || address_range(data.inputs.cast_const(), 1).is_none()
        || address_range(data.outputs.cast_const(), 1).is_none()
    {
        return None;
    }
    let input = unsafe { &*data.inputs };
    let output = unsafe { &*data.outputs };
    let input_channels_ptr = unsafe { input.__field0.channelBuffers32 };
    let output_channels_ptr = unsafe { output.__field0.channelBuffers32 };
    if input.numChannels != 2
        || output.numChannels != 2
        || input_channels_ptr.is_null()
        || output_channels_ptr.is_null()
        || address_range(input_channels_ptr.cast_const(), 2).is_none()
        || address_range(output_channels_ptr.cast_const(), 2).is_none()
    {
        return None;
    }

    let input_channels = unsafe { slice::from_raw_parts(input_channels_ptr, 2) };
    let output_channels = unsafe { slice::from_raw_parts(output_channels_ptr, 2) };
    if input_channels.iter().any(|channel| channel.is_null())
        || output_channels.iter().any(|channel| channel.is_null())
    {
        return None;
    }
    let frames = usize::try_from(data.numSamples).ok()?;
    if !validate_stereo_aliases(
        input_channels[0],
        input_channels[1],
        output_channels[0],
        output_channels[1],
        frames,
    ) {
        return None;
    }
    Some(RawStereoBuffers {
        frames,
        input_left: input_channels[0],
        input_right: input_channels[1],
        output_left: output_channels[0],
        output_right: output_channels[1],
        input_silence_flags: input.silenceFlags,
    })
}

/// Silence a structurally valid stereo output when the input descriptor is bad.
unsafe fn silence_valid_stereo_output(data: &ProcessData) -> tresult {
    if data.numSamples < 0 {
        return kInvalidArgument;
    }
    if data.numSamples == 0 {
        return process_ok();
    }
    if data.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as int32
        || data.numOutputs != 1
        || data.outputs.is_null()
        || address_range(data.outputs.cast_const(), 1).is_none()
    {
        return kInvalidArgument;
    }

    let output = unsafe { &mut *data.outputs };
    let channels_ptr = unsafe { output.__field0.channelBuffers32 };
    if output.numChannels != 2
        || channels_ptr.is_null()
        || address_range(channels_ptr.cast_const(), 2).is_none()
    {
        return kInvalidArgument;
    }
    let channels = unsafe { slice::from_raw_parts(channels_ptr, 2) };
    if channels.iter().any(|channel| channel.is_null()) {
        return kInvalidArgument;
    }
    let frames = data.numSamples as usize;
    let Some(left_range) = address_range(channels[0], frames) else {
        return kInvalidArgument;
    };
    let Some(right_range) = address_range(channels[1], frames) else {
        return kInvalidArgument;
    };
    if ranges_overlap(left_range, right_range) {
        return kInvalidArgument;
    }
    unsafe { slice::from_raw_parts_mut(channels[0], frames).fill(0.0) };
    unsafe { slice::from_raw_parts_mut(channels[1], frames).fill(0.0) };
    output.silenceFlags = 0b11;
    process_ok()
}

/// VST3 processor component for GainSnap.
pub(super) struct GainSnapVst3Processor {
    connection: InstanceConnection<GainSnapVst3Shared>,
    /// Processor-owned canonical state; process never resolves it through the connection lock.
    shared: Arc<GainSnapVst3Shared>,
    runtime: RealtimeRuntime,
    publisher: RuntimePublisher<GainSnapVst3Runtime>,
    processing_reset_requested: AtomicBool,
}

impl GainSnapVst3Processor {
    /// Create a processor with a fully initialized 48 kHz runtime.
    pub(super) fn new() -> Self {
        let shared = GainSnapVst3Shared::new();
        let (publisher, audio_runtime) =
            RuntimePublisher::new(GainSnapVst3Runtime::new(&shared.params, 48_000.0));
        Self {
            connection: InstanceConnection::new(
                InstanceConnectionRole::Processor,
                Arc::clone(&shared),
            ),
            shared,
            runtime: RealtimeRuntime::new(audio_runtime),
            publisher,
            processing_reset_requested: AtomicBool::new(false),
        }
    }

    fn publish_runtime(&self, sample_rate: f32) {
        let Ok(registration) = self.publisher.register() else {
            return;
        };
        registration.publish(GainSnapVst3Runtime::new(&self.shared.params, sample_rate));
        let _ = self.publisher.reclaim();
    }
}

impl Drop for GainSnapVst3Processor {
    fn drop(&mut self) {
        let _ = self.publisher.reclaim();
    }
}

impl Class for GainSnapVst3Processor {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IProcessContextRequirements,
        IConnectionPoint,
        IToyboxSharedState,
    );
}

toybox::impl_vst3_instance_connection!(GainSnapVst3Processor, connection);

impl IPluginBaseTrait for GainSnapVst3Processor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for GainSnapVst3Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        if class_id.is_null() {
            return kInvalidArgument;
        }
        unsafe { *class_id = CONTROLLER_CID };
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media_type: MediaType, direction: BusDirection) -> int32 {
        match media_type as MediaTypes {
            MediaTypes_::kAudio => match direction as BusDirections {
                BusDirections_::kInput | BusDirections_::kOutput => 1,
                _ => 0,
            },
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        media_type: MediaType,
        direction: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        if bus.is_null() || index != 0 || media_type as MediaTypes != MediaTypes_::kAudio {
            return kInvalidArgument;
        }
        let label = match direction as BusDirections {
            BusDirections_::kInput => "Input",
            BusDirections_::kOutput => "Output",
            _ => return kInvalidArgument,
        };
        let bus = unsafe { &mut *bus };
        bus.mediaType = MediaTypes_::kAudio as MediaType;
        bus.direction = direction;
        bus.channelCount = 2;
        copy_wstring(label, &mut bus.name);
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive;
        kResultOk
    }

    unsafe fn getRoutingInfo(
        &self,
        _input: *mut RoutingInfo,
        _output: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        media_type: MediaType,
        direction: BusDirection,
        index: int32,
        _state: TBool,
    ) -> tresult {
        if media_type as MediaTypes != MediaTypes_::kAudio || index != 0 {
            return kInvalidArgument;
        }
        match direction as BusDirections {
            BusDirections_::kInput | BusDirections_::kOutput => kResultOk,
            _ => kInvalidArgument,
        }
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        self.processing_reset_requested
            .store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let result = unsafe { super::read_vst3_state(state, &self.shared) };
        if result == kResultOk {
            self.processing_reset_requested
                .store(true, Ordering::Release);
        }
        result
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        unsafe { super::write_vst3_state(state, &self.shared) }
    }
}

impl IAudioProcessorTrait for GainSnapVst3Processor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        input_count: int32,
        outputs: *mut SpeakerArrangement,
        output_count: int32,
    ) -> tresult {
        if input_count != 1 || output_count != 1 || inputs.is_null() || outputs.is_null() {
            return kResultFalse;
        }
        if unsafe { *inputs } != SpeakerArr::kStereo || unsafe { *outputs } != SpeakerArr::kStereo {
            return kResultFalse;
        }
        kResultTrue
    }

    unsafe fn getBusArrangement(
        &self,
        direction: BusDirection,
        index: int32,
        arrangement: *mut SpeakerArrangement,
    ) -> tresult {
        if arrangement.is_null() || index != 0 {
            return kInvalidArgument;
        }
        match direction as BusDirections {
            BusDirections_::kInput | BusDirections_::kOutput => {
                unsafe { *arrangement = SpeakerArr::kStereo };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn canProcessSampleSize(&self, sample_size: int32) -> tresult {
        match sample_size as SymbolicSampleSizes {
            SymbolicSampleSizes_::kSample32 => kResultOk,
            SymbolicSampleSizes_::kSample64 => kNotImplemented,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if setup.is_null() {
            return kInvalidArgument;
        }
        let sample_rate = unsafe { (*setup).sampleRate };
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return kInvalidArgument;
        }
        let sample_rate = sample_rate as f32;
        self.publish_runtime(sample_rate);
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        self.processing_reset_requested
            .store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if data.is_null() {
            return kInvalidArgument;
        }
        let data = unsafe { &*data };
        if data.numSamples < 0 {
            return kInvalidArgument;
        }
        let frames = data.numSamples as usize;
        let Some(mut guard) = self.runtime.try_acquire() else {
            return if frames == 0 {
                process_ok()
            } else {
                unsafe { silence_valid_stereo_output(data) }
            };
        };
        // Adoption is a bounded lock-free operation. Runtime candidates are
        // built by setupProcessing, never by the process callback.
        let _ = guard.try_adopt(|_, _| true);
        let runtime = guard.current_mut();
        runtime.timeline.begin_block(frames);
        // SAFETY: the host owns the parameter-change collection for this
        // synchronous callback. The collector bounds both queues and points.
        unsafe { collect_parameter_events(data.inputParameterChanges, &mut runtime.timeline) };
        runtime.timeline.prepare();

        if self
            .processing_reset_requested
            .swap(false, Ordering::AcqRel)
        {
            runtime.engine.reset(&self.shared.params);
        }
        runtime.engine.begin_block(&self.shared.params);

        if frames == 0 {
            apply_remaining_events(runtime, &self.shared.params);
            return process_ok();
        }

        if data.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as int32 {
            apply_remaining_events(runtime, &self.shared.params);
            return unsafe { silence_valid_stereo_output(data) };
        }

        // SAFETY: raw_stereo_buffers validates all host-owned pointers and
        // channel ranges before any pointer arithmetic occurs.
        let Some(buffers) = (unsafe { raw_stereo_buffers(data) }) else {
            apply_remaining_events(runtime, &self.shared.params);
            return unsafe { silence_valid_stereo_output(data) };
        };

        for frame in 0..buffers.frames {
            runtime.timeline.apply_through(frame, &self.shared.params);
            runtime.engine.sync_controls(&self.shared.params);
            // SAFETY: each pointer is valid for the complete block and the
            // alias validator permits only exact per-channel in-place writes.
            let input_left = unsafe { ptr::read(buffers.input_left.add(frame)) };
            let input_right = unsafe { ptr::read(buffers.input_right.add(frame)) };
            let (output_left, output_right) =
                runtime
                    .engine
                    .process_frame(&self.shared.params, input_left, input_right);
            unsafe {
                ptr::write(buffers.output_left.add(frame), output_left);
                ptr::write(buffers.output_right.add(frame), output_right);
            }
        }
        apply_remaining_events(runtime, &self.shared.params);
        let report = runtime.engine.report();
        self.shared.status.update(
            report.input_peak_db,
            report.output_peak_db,
            report.locked_gain_db,
            report.progress,
            report.state,
        );
        // The matcher preserves the host's silence annotation for a fully
        // silent block and clears it whenever audio is present.
        unsafe {
            (*data.outputs).silenceFlags = if buffers.input_silence_flags == 0b11 {
                0b11
            } else {
                0
            };
        }
        process_ok()
    }

    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

fn apply_remaining_events(runtime: &mut GainSnapVst3Runtime, params: &GainSnapParams) {
    runtime.timeline.apply_remaining(params);
    runtime.engine.sync_controls(params);
}

impl IProcessContextRequirementsTrait for GainSnapVst3Processor {
    unsafe fn getProcessContextRequirements(&self) -> uint32 {
        0
    }
}

/// Bound host-side queue traversal before entering the realtime event loop.
const MAX_PARAMETER_QUEUES: usize = PARAMETER_COUNT;

/// Read a bounded prefix and one final point from each known host queue.
unsafe fn collect_parameter_events(
    changes: *mut IParameterChanges,
    timeline: &mut super::shared_state::ParamTimeline,
) {
    let Some(changes) = (unsafe { ComRef::from_raw(changes) }) else {
        return;
    };
    let queue_count = unsafe { changes.getParameterCount() }
        .max(0)
        .try_into()
        .unwrap_or(usize::MAX)
        .min(MAX_PARAMETER_QUEUES);
    let mut remaining = PARAM_EVENT_CAPACITY;

    for queue_index in 0..queue_count {
        let Some(queue) =
            (unsafe { ComRef::from_raw(changes.getParameterData(queue_index as int32)) })
        else {
            continue;
        };
        let param_id = unsafe { queue.getParameterId() };
        if param_bridge::clap_id(param_id).is_none() {
            continue;
        }
        let point_count = unsafe { queue.getPointCount() }
            .max(0)
            .try_into()
            .unwrap_or(usize::MAX);
        let prefix_count = point_count.min(remaining);

        for point_index in 0..prefix_count {
            let mut offset = 0;
            let mut normalized = 0.0;
            if unsafe { queue.getPoint(point_index as int32, &mut offset, &mut normalized) }
                == kResultTrue
            {
                push_normalized_param_event(timeline, param_id, offset, normalized);
            }
        }
        remaining = remaining.saturating_sub(prefix_count);

        if prefix_count < point_count {
            let mut offset = 0;
            let mut normalized = 0.0;
            if unsafe { queue.getPoint((point_count - 1) as int32, &mut offset, &mut normalized) }
                == kResultTrue
            {
                push_normalized_param_event(timeline, param_id, offset, normalized);
            }
        }
    }
}

fn push_normalized_param_event(
    timeline: &mut super::shared_state::ParamTimeline,
    param_id: ParamID,
    offset: int32,
    normalized: ParamValue,
) {
    let Some(clap_id) = param_bridge::clap_id(param_id) else {
        return;
    };
    let Some(value) = param_bridge::from_normalized(param_id, normalized) else {
        return;
    };
    timeline.push(
        offset,
        ParamEvent {
            offset: 0,
            sequence: 0,
            param_id: clap_id,
            value: value as f32,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PARAM_MATCH;
    use crate::status::MatchState;

    #[test]
    fn stereo_alias_validation_accepts_in_place_and_rejects_cross_aliases() {
        let mut input_left = [0.0_f32; 8];
        let mut input_right = [0.0_f32; 8];
        let mut output_left = [0.0_f32; 8];
        let mut output_right = [0.0_f32; 8];

        assert!(validate_stereo_aliases(
            input_left.as_ptr(),
            input_right.as_ptr(),
            input_left.as_mut_ptr(),
            input_right.as_mut_ptr(),
            input_left.len(),
        ));
        assert!(validate_stereo_aliases(
            input_left.as_ptr(),
            input_right.as_ptr(),
            output_left.as_mut_ptr(),
            output_right.as_mut_ptr(),
            input_left.len(),
        ));
        assert!(!validate_stereo_aliases(
            input_left.as_ptr(),
            input_right.as_ptr(),
            input_right.as_mut_ptr(),
            input_left.as_mut_ptr(),
            input_left.len(),
        ));
        assert!(!validate_stereo_aliases(
            input_left.as_ptr(),
            input_left.as_ptr(),
            output_left.as_mut_ptr(),
            output_right.as_mut_ptr(),
            input_left.len(),
        ));
    }

    #[test]
    fn set_processing_only_requests_a_reset() {
        let processor = GainSnapVst3Processor::new();
        let mut setup: ProcessSetup = unsafe { std::mem::zeroed() };
        setup.sampleRate = 48_000.0;
        assert_eq!(unsafe { processor.setupProcessing(&mut setup) }, kResultOk);
        let revision = processor.publisher.latest_revision();

        assert_eq!(unsafe { processor.setProcessing(1) }, kResultOk);
        assert_eq!(processor.publisher.latest_revision(), revision);
        assert!(processor.processing_reset_requested.load(Ordering::Acquire));
    }

    #[test]
    fn block_end_match_off_is_finalized_before_vst3_return() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_MATCH, 1.0);
        let mut runtime = GainSnapVst3Runtime::new(&params, 1_000.0);
        runtime.timeline.begin_block(64);
        runtime.timeline.push(
            64,
            ParamEvent {
                offset: 0,
                sequence: 0,
                param_id: PARAM_MATCH,
                value: 0.0,
            },
        );
        runtime.timeline.prepare();
        runtime.engine.begin_block(&params);
        for _ in 0..64 {
            let _ = runtime.engine.process_frame(&params, 0.25, 0.25);
        }

        apply_remaining_events(&mut runtime, &params);

        assert_eq!(runtime.engine.report().state, MatchState::Locked);
        assert!((runtime.engine.report().locked_gain_db - 0.0412).abs() < 0.02);
    }
}
