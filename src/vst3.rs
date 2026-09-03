//! VST3 adapter sharing GainSnap's parameters, state, DSP, and editor.

use std::ffi::c_void;

use toybox::vst3::prelude::Steinberg::*;
use toybox::vst3::prelude::*;

use crate::state::{
    apply_snapshot, decode_payload, StateSnapshot, ACCEPTED_STATE_VERSIONS, STATE_MAGIC,
    STATE_PAYLOAD_BYTES, STATE_VERSION,
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

#[cfg(target_os = "windows")]
pub(super) const fn vst3_bus_flag(flag: i32) -> u32 {
    flag as u32
}

#[cfg(not(target_os = "windows"))]
pub(super) const fn vst3_bus_flag(flag: u32) -> u32 {
    flag
}

/// Build a VST3 state payload from the shared atomics.
unsafe fn write_vst3_state(stream: *mut IBStream, shared: &GainSnapVst3Shared) -> tresult {
    let payload = crate::state::encode_payload(&shared.params);
    match unsafe { write_versioned_payload(stream, STATE_MAGIC, STATE_VERSION, &payload) } {
        Ok(()) => kResultOk,
        Err(_) => kResultFalse,
    }
}

fn decode_vst3_state_payload(version: u32, payload: &[u8]) -> Option<StateSnapshot> {
    if payload.len() != STATE_PAYLOAD_BYTES {
        return None;
    }
    decode_payload(version, payload)
}

/// Decode and apply a VST3 state payload only after the full payload validates.
unsafe fn read_vst3_state(stream: *mut IBStream, shared: &GainSnapVst3Shared) -> tresult {
    let Ok(versioned) =
        (unsafe { read_versioned_payload(stream, STATE_MAGIC, ACCEPTED_STATE_VERSIONS) })
    else {
        return kInvalidArgument;
    };
    let Some(snapshot) = decode_vst3_state_payload(versioned.version, &versioned.payload) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{GainSnapParams, PARAM_LOCKED_GAIN_DB, PARAM_MATCH, PARAM_TARGET_DB};
    use crate::state::{encode_payload, LEGACY_STATE_VERSION};

    #[test]
    fn vst3_v1_state_migrates_match_now_to_off_and_preserves_gain() {
        let params = GainSnapParams::new();
        params.set_param(PARAM_TARGET_DB, -7.5);
        params.set_param(PARAM_MATCH, 1.0);
        params.set_param(PARAM_LOCKED_GAIN_DB, 5.25);
        let payload = encode_payload(&params);
        let encoded = try_encode_versioned_payload(STATE_MAGIC, LEGACY_STATE_VERSION, &payload)
            .expect("legacy VST3 state should encode");
        let versioned = decode_versioned_payload(&encoded, STATE_MAGIC, ACCEPTED_STATE_VERSIONS)
            .expect("legacy VST3 state should be accepted");

        let shared = GainSnapVst3Shared::new();
        let snapshot = decode_vst3_state_payload(versioned.version, &versioned.payload)
            .expect("legacy VST3 state should migrate");
        apply_snapshot(&shared.params, snapshot);

        assert_eq!(shared.params.target_db(), -7.5);
        assert!(!shared.params.match_requested());
        assert_eq!(shared.params.locked_gain_db(), 5.25);
    }
}
