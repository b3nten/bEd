//! Portable appearance data, UI helpers and GPU canvas contracts.
//! Attribution: workspace LICENSE and NOTICE; bundled fonts retain their OFL license.
//!
//! Default features are empty: color, appearance values and texture identities
//! require no native UI, GPU, editor, or application configuration. Enable `imgui`
//! for widgets, icons and atlas helpers, and `gpu` for canvas rendering contracts.
//! `UiRuntime` prepares fonts, theme and motion for an ImGui context before a
//! frame; widgets read its immutable `UiFrameContext` while drawing.
#[cfg(feature = "imgui")]
mod animation;
pub mod appearance;
#[cfg(feature = "imgui")]
pub mod context;
#[cfg(feature = "imgui")]
pub use context::{UiFrameContext, UiRuntime};
pub mod color;
#[cfg(feature = "imgui")]
pub mod components;
#[cfg(feature = "imgui")]
pub mod font;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod texture;
pub use texture::TextureHandle;
#[cfg(feature = "imgui")]
pub mod icons;
#[cfg(feature = "imgui")]
pub mod presentation;
#[cfg(feature = "imgui")]
pub mod util;
#[cfg(all(test, feature = "imgui"))]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "documentation")]
pub mod documentation;
#[cfg(feature = "documentation")]
pub mod documentation_markdown;

#[cfg(all(test, feature = "imgui"))]
mod checkbox_tests;
