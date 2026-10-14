//! Rendering inputs owned by the application frontend.
use bedtime_ui::TextureHandle;
use dear_imgui_rs::TextureId;
use std::collections::HashMap;
#[derive(bevy::prelude::Resource, Clone, Default)]
pub struct PresentationFrame {
    pub textures: HashMap<TextureHandle, TextureId>,
}

/// Optional ANSI colors supplied by an application's theme owner.
#[derive(bevy::prelude::Resource, Clone, Debug, PartialEq)]
pub struct TerminalPalette(pub [[f32; 4]; 16]);
