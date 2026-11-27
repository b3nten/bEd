use crate::{ambient, theme, tokens::*};

/// A bordered box that sets something apart: a message, a notice, an
/// attachment.
///
/// ```ignore
/// Card::new().tone(Tone::Warning).fill_width().show(ui, |ui| {
///     ui.add(Text::title("Permission requested"));
///     ui.add(Button::new("Allow").primary().small());
/// });
/// ```
#[must_use = "a card does nothing until it's shown"]
#[derive(Clone, Copy, Default)]
pub struct Card {
    tone: Tone,
    small: bool,
    fill_width: bool,
}

impl Card {
    pub fn new() -> Self {
        Self::default()
    }

    /// The border's colour: a warning's, an error's. Neutral is the theme's
    /// border.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Tighter, for a chip or a line in a list.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// As wide as there's room for, whatever is in it.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }

    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        content: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let services = ambient::services(ui.ctx());
        let p = &services.theme.palette;
        let border = match self.tone {
            Tone::Neutral => p.border,
            tone => tone.rgba(p),
        };
        let (across, down, radius) = if self.small {
            (Space::Sm, Space::Xs, Radius::Sm)
        } else {
            (Space::Md, Space::Sm, Radius::Md)
        };
        let fill_width = self.fill_width && !ui.is_sizing_pass();
        egui::Frame::new()
            .fill(theme::color(p.surface))
            .stroke(egui::Stroke::new(1.0, theme::color(border)))
            .corner_radius(radius.px(ui))
            .inner_margin(egui::Margin::symmetric(
                across.px(ui) as i8,
                down.px(ui) as i8,
            ))
            .show(ui, |ui| {
                if fill_width {
                    ui.set_width(ui.available_width());
                }
                content(ui)
            })
    }
}
