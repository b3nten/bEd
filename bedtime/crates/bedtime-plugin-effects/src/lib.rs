//! Bed effects components. Attribution: workspace LICENSE and NOTICE.
pub mod settings;
#[cfg(feature = "gpu")]
pub mod shader;
#[cfg(feature = "gpu")]
pub mod shader_manager;
#[cfg(feature = "gpu")]
pub mod shader_types;

#[cfg(feature = "desktop")]
pub mod desktop;
#[cfg(feature = "desktop")]
pub use desktop::EffectsPlugin;

#[cfg(all(test, feature = "desktop"))]
mod composition_tests;
