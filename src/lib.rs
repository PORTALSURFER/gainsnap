//! GainSnap AudioDev plug-in.
//!
//! This generated crate is intentionally small. Keep plug-in-specific DSP,
//! parameters, state, and declarative UI here; shared host/framework behavior
//! belongs in Toybox.

#![deny(missing_docs, warnings)]

mod clap_plugin;
mod dsp;
mod params;
mod state;
mod status;

#[cfg(all(target_os = "macos", feature = "radiant-gui"))]
pub mod gui;

#[cfg(feature = "vst3")]
mod vst3;

toybox::clap_plugin_entry!(clap_plugin::GainSnapPlugin);
