//! Shared contracts for bEd foundations and optional Bevy plugins.
//! Plugins are linked Rust crates; this is not a dynamic-library ABI.
#[cfg(feature = "documents")]
mod document_frame;
#[cfg(feature = "documents")]
pub mod documents;
pub mod features;
pub mod files;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod presentation;
pub mod settings;
pub mod status;
pub mod storage;
pub mod workbench;
