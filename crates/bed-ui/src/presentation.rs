use bed_editing::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{StyleColor, TextureId, Ui};

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
    fn file_icon_tint(&self, filename: &str, text: [f32; 4]) -> [f32; 4] {
        if self.get_for_file(filename) == self.get("default") {
            text
        } else {
            [1.0; 4]
        }
    }
}
