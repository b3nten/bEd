use crate::{Theme, theme, tokens::*};

/// Something's happening and it isn't known how long it'll take.
///
/// ```ignore
/// ui.add(Spinner::new());
/// ```
#[must_use = "a spinner does nothing until it's added to a Ui"]
#[derive(Clone, Copy, Default)]
pub struct Spinner {
    size: Size,
}

impl Spinner {
    /// As big as a line of text.
    pub fn new() -> Self {
        Self::default()
    }
    /// As big as a line of a control of this size's text.
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Draw it in `rect`, taking no room (in a row that's painted by hand).
    pub fn paint_at(self, ui: &egui::Ui, rect: egui::Rect) {
        self.inner(ui).paint_at(ui, rect);
    }

    fn inner(self, ui: &egui::Ui) -> egui::Spinner {
        egui::Spinner::new()
            .size(self.size.font(ui))
            .color(theme::color(Theme::of(ui).palette.muted))
    }
}

impl egui::Widget for Spinner {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let spinner = self.inner(ui);
        ui.add(spinner)
    }
}
