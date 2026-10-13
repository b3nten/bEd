//! Bed effects components. Attribution: workspace LICENSE and NOTICE.
pub mod shader;
pub mod shader_manager;
pub mod shader_types;

#[cfg(feature = "desktop")]
pub mod desktop;
#[cfg(feature = "desktop")]
pub use desktop::EffectsPlugin;
