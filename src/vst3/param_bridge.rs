//! VST3 parameter conversion over GainSnap's shared parameter model.

use toybox::clack_plugin::utils::ClapId;

use crate::params::{
    apply_normalized_param_value, clap_id_from_vst3_param_id, normalized_from_plain_value,
    plain_from_normalized_value, vst3_param_info_for_index, GainSnapParams, Vst3ParamInfo,
};

/// Return metadata for a VST3 parameter index.
pub(super) fn info(index: i32) -> Option<Vst3ParamInfo> {
    vst3_param_info_for_index(index)
}

/// Map a VST3 parameter id to the shared CLAP id.
pub(super) fn clap_id(param_id: u32) -> Option<ClapId> {
    clap_id_from_vst3_param_id(param_id)
}

/// Convert a plain value to VST3 normalized space.
pub(super) fn to_normalized(param_id: u32, value: f64) -> Option<f64> {
    normalized_from_plain_value(clap_id(param_id)?, value)
}

/// Convert a normalized VST3 value to a plain value.
pub(super) fn from_normalized(param_id: u32, value: f64) -> Option<f64> {
    plain_from_normalized_value(clap_id(param_id)?, value)
}

/// Read one shared parameter in plain units.
pub(super) fn read_plain(params: &GainSnapParams, param_id: u32) -> Option<f64> {
    params.get_param(clap_id(param_id)?).map(f64::from)
}

/// Apply one normalized VST3 parameter value.
pub(super) fn apply_normalized(params: &GainSnapParams, param_id: u32, value: f64) -> bool {
    let Some(clap_id) = clap_id(param_id) else {
        return false;
    };
    apply_normalized_param_value(params, clap_id, value)
}

/// Format one VST3 parameter in host-facing units.
pub(super) fn format_value(param_id: u32, value: f64) -> Option<String> {
    let id = clap_id(param_id)?;
    crate::params::format_value_text(id, value)
}
