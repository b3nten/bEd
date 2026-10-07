use dear_imgui_rs::{Key, TextureId};
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
