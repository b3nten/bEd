//! Rendering inputs owned by the application frontend.
use bed_ui::TextureHandle;
use dear_imgui_rs::TextureId;
use std::collections::HashMap;
#[derive(bevy::prelude::Resource, Clone)]
pub struct PresentationFrame {
    pub textures: HashMap<TextureHandle, TextureId>,
    pub animations: bool,
}
impl Default for PresentationFrame {
    fn default() -> Self {
        Self {
            textures: HashMap::new(),
            animations: true,
        }
    }
}
