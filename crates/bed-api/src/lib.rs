//! Shared contracts for bEd foundations and optional Bevy plugins.
//! Plugins are linked Rust crates; this is not a dynamic-library ABI.
pub mod application;
#[cfg(feature = "documents")]
pub mod documents;
pub mod features;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod settings;
pub mod status;
pub mod workbench;
