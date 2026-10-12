//! Portable appearance data, UI helpers and GPU canvas contracts.
//! Attribution: workspace LICENSE and NOTICE; bundled fonts retain their OFL license.
//!
//! Default features are empty: color, appearance values and texture identities
//! require no native UI, GPU, editor, or application configuration. Enable `imgui`
//! for widgets, icons and atlas helpers, and `gpu` for canvas rendering contracts.
//! Embedding hosts keep their own context and call `UiFonts::apply` with embedded
//! mode before a frame, then select the returned main font while drawing.
pub mod appearance;
pub mod color;
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
