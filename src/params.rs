//! GainSnap parameter definitions and atomic state.

use std::ffi::CStr;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use toybox::clack_extensions::params::{ParamDisplayWriter, ParamInfoWriter};
use toybox::clack_plugin::prelude::ClapId;
use toybox::clap::params::{ParamBuilder, ParamSpec};

/// Target peak parameter identifier.
pub const PARAM_TARGET_DB: ClapId = ClapId::new(1);
/// One-shot matcher arm/trigger parameter identifier.
pub const PARAM_MATCH: ClapId = ClapId::new(2);
/// Read-only calculated gain parameter identifier.
pub const PARAM_LOCKED_GAIN_DB: ClapId = ClapId::new(3);

/// Lowest supported target peak in dBFS.
pub const TARGET_MIN_DB: f32 = -36.0;
/// Highest supported target peak in dBFS, leaving a small true-peak safety margin.
pub const TARGET_MAX_DB: f32 = -0.1;
/// Default target peak in dBFS.
pub const DEFAULT_TARGET_DB: f32 = -12.0;
/// Lowest gain correction GainSnap can apply.
pub const GAIN_MIN_DB: f32 = -24.0;
/// Highest gain correction GainSnap can apply.
pub const GAIN_MAX_DB: f32 = 24.0;
/// Default applied gain before the first successful match.
pub const DEFAULT_LOCKED_GAIN_DB: f32 = 0.0;

/// Stable metadata for a single exposed parameter.
#[derive(Clone, Copy, Debug)]
pub struct ParamDef {
    /// Stable CLAP/VST3 identifier.
    pub id: ClapId,
    /// Host-facing display title.
    pub name: &'static [u8],
    /// Host-facing module label.
    pub module: &'static [u8],
    /// Plain-value lower bound.
    pub min: f64,
    /// Plain-value upper bound.
    pub max: f64,
    /// Plain-value default.
    pub default: f64,
    /// Whether hosts should expose this value for automation.
    pub automatable: bool,
    /// Whether hosts should present this value as a stepped control.
    pub stepped: bool,
}

impl ParamDef {
    /// Convert this definition to the shared CLAP metadata type.
    pub fn to_spec(self) -> ParamSpec<'static> {
        let mut builder = ParamBuilder::new(self.id, self.name, self.module)
            .range(self.min, self.max)
            .default(self.default);
        if self.automatable {
            builder = builder.automatable();
        }
        if self.stepped {
            builder = builder.stepped().enumerated();
        }
        builder.build()
    }
}

/// Parameters in stable host-visible order.
pub const PARAM_DEFS: [ParamDef; 3] = [
    ParamDef {
        id: PARAM_TARGET_DB,
        name: b"Target Peak",
        module: b"Match",
        min: TARGET_MIN_DB as f64,
        max: TARGET_MAX_DB as f64,
        default: DEFAULT_TARGET_DB as f64,
        automatable: true,
        stepped: false,
    },
    ParamDef {
        id: PARAM_MATCH,
        name: b"Match Now",
        module: b"Match",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        automatable: true,
        stepped: true,
    },
    ParamDef {
        id: PARAM_LOCKED_GAIN_DB,
        name: b"Locked Gain",
        module: b"Result",
        min: GAIN_MIN_DB as f64,
        max: GAIN_MAX_DB as f64,
        default: DEFAULT_LOCKED_GAIN_DB as f64,
        automatable: false,
        stepped: false,
    },
];

/// VST3 metadata corresponding to one shared parameter.
#[cfg(feature = "vst3")]
#[derive(Clone, Copy, Debug)]
pub struct Vst3ParamInfo {
    /// VST3 parameter identifier.
    pub id: u32,
    /// Long parameter title.
    pub title: &'static str,
    /// Short parameter title.
    pub short_title: &'static str,
    /// Unit label.
    pub units: &'static str,
    /// VST3 step count (`0` for continuous controls).
    pub step_count: i32,
    /// Default normalized value.
    pub default_normalized: f64,
    /// Whether VST3 should expose host automation for this parameter.
    pub automatable: bool,
}

/// Atomically shared parameter storage.
pub struct GainSnapParams {
    target_db: AtomicF32,
    match_request: AtomicU32,
    locked_gain_db: AtomicF32,
}

impl Default for GainSnapParams {
    fn default() -> Self {
        Self::new()
    }
}

impl GainSnapParams {
    /// Construct parameters with GainSnap's conservative defaults.
    pub fn new() -> Self {
        Self {
            target_db: AtomicF32::new(DEFAULT_TARGET_DB),
            match_request: AtomicU32::new(0),
            locked_gain_db: AtomicF32::new(DEFAULT_LOCKED_GAIN_DB),
        }
    }

    /// Read the selected target peak in dBFS.
    pub fn target_db(&self) -> f32 {
        sanitize_target(self.target_db.load(Ordering::Relaxed))
    }

    /// Read whether a one-shot match is currently requested.
    pub fn match_requested(&self) -> bool {
        self.match_request.load(Ordering::Relaxed) != 0
    }

    /// Read the last calculated gain correction in decibels.
    pub fn locked_gain_db(&self) -> f32 {
        sanitize_gain(self.locked_gain_db.load(Ordering::Relaxed))
    }

    /// Apply a canonical plain parameter value from a host or editor.
    pub fn set_param(&self, id: ClapId, value: f32) {
        match id {
            PARAM_TARGET_DB => self
                .target_db
                .store(sanitize_target(value), Ordering::Relaxed),
            PARAM_MATCH => self.match_request.store(
                u32::from(value.is_finite() && value >= 0.5),
                Ordering::Relaxed,
            ),
            PARAM_LOCKED_GAIN_DB => self
                .locked_gain_db
                .store(sanitize_gain(value), Ordering::Relaxed),
            _ => {}
        }
    }

    /// Return the current plain value for a known parameter.
    pub fn get_param(&self, id: ClapId) -> Option<f32> {
        match id {
            PARAM_TARGET_DB => Some(self.target_db()),
            PARAM_MATCH => Some(f32::from(self.match_requested())),
            PARAM_LOCKED_GAIN_DB => Some(self.locked_gain_db()),
            _ => None,
        }
    }
}

/// Return the number of exposed parameters.
pub const fn param_count() -> u32 {
    PARAM_DEFS.len() as u32
}

/// Write metadata for a parameter index.
pub fn write_param_info(index: u32, writer: &mut ParamInfoWriter) {
    if let Some(def) = PARAM_DEFS.get(index as usize) {
        def.to_spec().write(writer);
    }
}

/// Format a parameter in its host-facing plain-value units.
pub fn value_to_text(id: ClapId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
    match id {
        PARAM_TARGET_DB => write!(writer, "{:.1} dB", sanitize_target(value as f32)),
        PARAM_MATCH => write!(writer, "{}", if value >= 0.5 { "Ready" } else { "Off" }),
        PARAM_LOCKED_GAIN_DB => write!(writer, "{:+.2} dB", sanitize_gain(value as f32)),
        _ => Ok(()),
    }
}

/// Parse a host-facing parameter string into a plain value.
pub fn text_to_value(id: ClapId, text: &CStr) -> Option<f64> {
    let raw = text.to_str().ok()?.trim();
    match id {
        PARAM_TARGET_DB | PARAM_LOCKED_GAIN_DB => raw
            .trim_end_matches("dB")
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| {
                if id == PARAM_TARGET_DB {
                    sanitize_target(value) as f64
                } else {
                    sanitize_gain(value) as f64
                }
            }),
        PARAM_MATCH
            if raw.eq_ignore_ascii_case("ready")
                || raw.eq_ignore_ascii_case("on")
                || raw == "1" =>
        {
            Some(1.0)
        }
        PARAM_MATCH if raw.eq_ignore_ascii_case("off") || raw == "0" => Some(0.0),
        _ => None,
    }
}

/// Format a parameter for a host-facing VST3 string buffer.
#[cfg(feature = "vst3")]
pub fn format_value_text(id: ClapId, value: f64) -> Option<String> {
    let mut text = String::new();
    match id {
        PARAM_TARGET_DB => write!(&mut text, "{:.1} dB", sanitize_target(value as f32)).ok()?,
        PARAM_MATCH => text.push_str(if value >= 0.5 { "Ready" } else { "Off" }),
        PARAM_LOCKED_GAIN_DB => write!(&mut text, "{:+.2} dB", sanitize_gain(value as f32)).ok()?,
        _ => return None,
    }
    Some(text)
}

/// Convert a shared plain value into VST3 normalized space.
#[cfg(feature = "vst3")]
pub fn normalized_from_plain_value(id: ClapId, value: f64) -> Option<f64> {
    let def = PARAM_DEFS.iter().find(|def| def.id == id)?;
    Some(((value.clamp(def.min, def.max) - def.min) / (def.max - def.min)).clamp(0.0, 1.0))
}

/// Convert a VST3 normalized value to a shared plain value.
#[cfg(feature = "vst3")]
pub fn plain_from_normalized_value(id: ClapId, normalized: f64) -> Option<f64> {
    let def = PARAM_DEFS.iter().find(|def| def.id == id)?;
    Some(def.min + normalized.clamp(0.0, 1.0) * (def.max - def.min))
}

/// Apply one normalized VST3 value to the shared parameter store.
#[cfg(feature = "vst3")]
pub fn apply_normalized_param_value(params: &GainSnapParams, id: ClapId, normalized: f64) -> bool {
    let Some(value) = plain_from_normalized_value(id, normalized) else {
        return false;
    };
    params.set_param(id, value as f32);
    true
}

/// Map a VST3 parameter identifier to a shared CLAP identifier.
#[cfg(feature = "vst3")]
pub fn clap_id_from_vst3_param_id(id: u32) -> Option<ClapId> {
    let clap_id = ClapId::new(id);
    PARAM_DEFS
        .iter()
        .any(|def| def.id == clap_id)
        .then_some(clap_id)
}

/// Return VST3 metadata for a parameter index.
#[cfg(feature = "vst3")]
pub fn vst3_param_info_for_index(index: i32) -> Option<Vst3ParamInfo> {
    match index {
        0 => Some(Vst3ParamInfo {
            id: PARAM_TARGET_DB.get(),
            title: "Target Peak",
            short_title: "Target",
            units: "dBFS",
            step_count: 0,
            default_normalized: normalized_from_plain_value(
                PARAM_TARGET_DB,
                DEFAULT_TARGET_DB as f64,
            )?,
            automatable: true,
        }),
        1 => Some(Vst3ParamInfo {
            id: PARAM_MATCH.get(),
            title: "Match Now",
            short_title: "Match",
            units: "",
            step_count: 1,
            default_normalized: 0.0,
            automatable: true,
        }),
        2 => Some(Vst3ParamInfo {
            id: PARAM_LOCKED_GAIN_DB.get(),
            title: "Locked Gain",
            short_title: "Gain",
            units: "dB",
            step_count: 0,
            default_normalized: 0.5,
            automatable: false,
        }),
        _ => None,
    }
}

fn sanitize_target(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(TARGET_MIN_DB, TARGET_MAX_DB)
    } else {
        DEFAULT_TARGET_DB
    }
}

fn sanitize_gain(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(GAIN_MIN_DB, GAIN_MAX_DB)
    } else {
        DEFAULT_LOCKED_GAIN_DB
    }
}

/// Atomic floating-point storage using an atomic bit representation.
#[derive(Default)]
struct AtomicF32 {
    value: AtomicU32,
}

impl AtomicF32 {
    fn new(value: f32) -> Self {
        Self {
            value: AtomicU32::new(value.to_bits()),
        }
    }

    fn load(&self, ordering: Ordering) -> f32 {
        f32::from_bits(self.value.load(ordering))
    }

    fn store(&self, value: f32, ordering: Ordering) {
        self.value.store(value.to_bits(), ordering);
    }
}
