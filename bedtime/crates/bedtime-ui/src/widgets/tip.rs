use super::Text;

/// What something is, shown beside it when it's pointed at.
///
/// ```ignore
/// ui.add(Button::icon_only(BuiltinIcon::Pin, "Pin")).tip("Keep showing this editor");
/// ```
///
/// On the `Response` of anything: a widget needs no surface or panel in
/// hand to explain itself.
pub trait Tip: Sized {
    /// A line of text.
    fn tip(self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.tip_ui(|ui| {
            ui.add(Text::new(text));
        })
    }
    /// Anything drawn.
    fn tip_ui(self, draw: impl FnOnce(&mut egui::Ui)) -> Self;
}

impl Tip for egui::Response {
    fn tip_ui(self, draw: impl FnOnce(&mut egui::Ui)) -> Self {
        let style = self.ctx.global_style();
        // Padded by the text's size, so it grows with the zoom.
        let font = egui::TextStyle::Body.resolve(&style).size;
        let padding =
            egui::Margin::symmetric((font * 0.75).round() as i8, (font * 0.6).round() as i8);
        let mut tooltip = egui::Tooltip::for_enabled(&self).gap((font * 0.25).round());
        tooltip.popup = tooltip
            .popup
            .frame(egui::Frame::popup(&style).inner_margin(padding));
        tooltip.show(draw);
        self
    }
}

/// The same on a borrowed `Response`, for one that's still needed after:
/// `(&response).tip("…")`, or `response.tip("…")` where it's already a
/// reference.
impl Tip for &egui::Response {
    fn tip_ui(self, draw: impl FnOnce(&mut egui::Ui)) -> Self {
        let _ = self.clone().tip_ui(draw);
        self
    }
}
