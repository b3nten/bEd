//! Language-service presentation consumes the shared editor keymap.
#[derive(Clone, Debug)]
pub struct LspPresentationOptions {
    pub background_color: [f32; 4],
    pub embedded: bool,
    pub shortcuts: bed_keybindings::ShortcutInput,
    /// Open completions after ordinary typing. Manual invocation always works.
    pub autocomplete: bool,
    pub completion_delay_ms: u64,
}
impl Default for LspPresentationOptions {
    fn default() -> Self {
        Self {
            background_color: [0.0, 0.0, 0.0, 1.0],
            embedded: true,
            shortcuts: bed_editor_ui::shortcuts::default_input(),
            autocomplete: true,
            completion_delay_ms: 150,
        }
    }
}
impl LspPresentationOptions {
    pub fn background_color(&self) -> [f32; 4] {
        self.background_color
    }
}
