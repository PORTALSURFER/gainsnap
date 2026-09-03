//! VST3 adapter sharing GainSnap's parameters, state, DSP, and editor.

use std::ffi::c_void;

use toybox::vst3::prelude::Steinberg::*;
use toybox::vst3::prelude::*;

use crate::state::{
    apply_snapshot, decode_payload, STATE_MAGIC, STATE_PAYLOAD_BYTES, STATE_VERSION,
};

mod controller;
mod factory;
#[cfg(target_os = "macos")]
mod gui_adapter;
mod param_bridge;
mod processor;
mod shared_state;

use controller::GainSnapVst3Controller;
use factory::GainSnapVst3Factory;
use processor::GainSnapVst3Processor;
use shared_state::GainSnapVst3Shared;

/// Stable processor class identifier.
pub(super) const PROCESSOR_CID: TUID = uid(0x52C9D8A1, 0x4B273FEE, 0xA61D8952, 0x0BB70C43);
/// Stable edit-controller class identifier.
pub(super) const CONTROLLER_CID: TUID = uid(0x81C45E72, 0xAE0B3D9C, 0xB256F014, 0x6DB88935);

/// Build a VST3 state payload from the shared atomics.
unsafe fn write_vst3_state(stream: *mut IBStream, shared: &GainSnapVst3Shared) -> tresult {
    let payload = crate::state::encode_payload(&shared.params);
    match unsafe { write_versioned_payload(stream, STATE_MAGIC, STATE_VERSION, &payload) } {
        Ok(()) => kResultOk,
        Err(_) => kResultFalse,
    }
}

/// Decode and apply a VST3 state payload only after the full payload validates.
unsafe fn read_vst3_state(stream: *mut IBStream, shared: &GainSnapVst3Shared) -> tresult {
    let Ok(versioned) = (unsafe { read_versioned_payload(stream, STATE_MAGIC, &[STATE_VERSION]) })
    else {
        return kInvalidArgument;
    };
    if versioned.payload.len() != STATE_PAYLOAD_BYTES {
        return kInvalidArgument;
    }
    let Some(snapshot) = decode_payload(&versioned.payload) else {
        return kInvalidArgument;
    };
    apply_snapshot(&shared.params, snapshot);
    kResultOk
}

/// Create a VST3 processor instance for the class factory.
pub(super) fn create_processor() -> Option<ComPtr<FUnknown>> {
    ComWrapper::new(GainSnapVst3Processor::new()).to_com_ptr::<FUnknown>()
}

/// Create a VST3 controller instance for the class factory.
pub(super) fn create_controller() -> Option<ComPtr<FUnknown>> {
    ComWrapper::new(GainSnapVst3Controller::new()).to_com_ptr::<FUnknown>()
}

/// Query a class instance for the requested VST3 interface.
pub(super) unsafe fn query_instance(
    instance: ComPtr<FUnknown>,
    iid: FIDString,
    object: *mut *mut c_void,
) -> tresult {
    let pointer = instance.as_ptr();
    unsafe { ((*(*pointer).vtbl).queryInterface)(pointer, iid as *mut TUID, object) }
}

toybox::vst3_plugin_entry!(GainSnapVst3Factory);
