//! Rendering inputs owned by the application frontend.
use bed_ui::TextureHandle;
use dear_imgui_rs::TextureId;
use std::collections::HashMap;
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct PresentationFrame {
    pub textures: HashMap<TextureHandle, TextureId>,
}
