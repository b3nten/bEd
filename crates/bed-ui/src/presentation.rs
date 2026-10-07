use bed_core::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{Key, StyleColor, TextureId, Ui};

/// Readable secondary text for both light and dark host palettes.
pub fn muted_text_color(ui: &Ui) -> [f32; 4] {
    let background = ui.style_color(StyleColor::WindowBg);
    let text = ui.style_color(StyleColor::Text);
    ensure_contrast(blend(text, background, 0.6), background, 4.5)
}

/// Keep semantic hues while making their labels legible on the current surface.
pub fn readable_color(ui: &Ui, color: [f32; 4]) -> [f32; 4] {
    ensure_contrast(color, ui.style_color(StyleColor::WindowBg), 4.5)
}
pub trait FileIcons {
    fn get(&self, name: &str) -> Option<TextureId>;
    fn get_for_file(&self, filename: &str) -> Option<TextureId>;
}
/// Host-selected appearance and LSP shortcuts; no settings files or seeding.
#[derive(Clone, Debug)]
pub struct LspPresentationOptions {
    pub background_color: [f32; 4],
    pub embedded: bool,
    pub symbol_key: Option<Key>,
    pub definition_key: Option<Key>,
    pub references_key: Option<Key>,
}
impl Default for LspPresentationOptions {
    fn default() -> Self {
        Self {
            background_color: [0.0, 0.0, 0.0, 1.0],
            embedded: true,
            symbol_key: Some(Key::I),
            definition_key: Some(Key::D),
            references_key: Some(Key::R),
        }
    }
}
impl LspPresentationOptions {
    pub fn background_color(&self) -> [f32; 4] {
        self.background_color
    }
    pub fn action_key(&self, action: &str) -> Option<Key> {
        match action {
            "lsp_symbol_info" => self.symbol_key,
            "lsp_find_def" => self.definition_key,
            "lsp_find_ref" => self.references_key,
            _ => None,
        }
    }
}
