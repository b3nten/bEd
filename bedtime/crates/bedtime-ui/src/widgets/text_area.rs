use crate::{ambient, motion, theme, tokens::*};

/// Several lines of text to edit.
///
/// ```ignore
/// ui.add(TextArea::new(&mut body).hint("Request body").rows(8).fill_width());
/// ```
///
/// The editing itself is egui's `TextEdit`; for code with highlighting, the
/// editor and `CodeView` are the places.
#[must_use = "a text area does nothing until it's added to a Ui"]
pub struct TextArea<'a> {
    text: &'a mut String,
    hint: String,
    label: String,
    rows: usize,
    width: Option<f32>,
    fill_width: bool,
    fill_height: bool,
    invalid: bool,
    id: Option<egui::Id>,
}

impl<'a> TextArea<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            hint: String::new(),
            label: String::new(),
            rows: 4,
            width: None,
            fill_width: false,
            fill_height: false,
            invalid: false,
            id: None,
        }
    }

    /// Shown while it's empty.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    /// Its name, for accessibility and scripts (which otherwise know it by
    /// its text).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    /// How many lines tall it starts (it grows with what's in it).
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }
    /// This wide, in points (egui's text edit width, unless set).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    /// As wide as the space it's in.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }
    /// At least as tall as the space it's in.
    pub fn fill_height(mut self) -> Self {
        self.fill_height = true;
        self
    }
    /// The id of its text, to ask after its focus from elsewhere
    /// (`memory.has_focus(id)`).
    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = Some(id);
        self
    }
    /// What's in it is wrong: outlined in the error colour.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }
}

impl egui::Widget for TextArea<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let p = &services.theme.palette;
        let font_size = Size::Md.font(ui);
        let padding = Size::Md.padding(ui);
        let available = ui.available_width();
        let width = if self.fill_width {
            available
        } else {
            self.width
                .unwrap_or(ui.spacing().text_edit_width)
                .min(available)
        };
        let id = self
            .id
            .unwrap_or_else(|| ui.next_auto_id().with("text-area"));
        let margin_y = padding.y + 2.0;
        let min_height = if self.fill_height {
            // The space, less the padding and the border around it.
            (ui.available_height() - 2.0 * margin_y - 2.0).max(0.0)
        } else {
            0.0
        };
        let focused = ui.memory(|memory| memory.has_focus(id));
        let hovered = ui
            .ctx()
            .read_response(id)
            .is_some_and(|response| response.hovered());
        let target = if hovered { 1.0 } else { 0.0 };
        let hover = motion::scalar(ui.ctx(), id.with("hover"), target, target, services.motion);
        let border = if self.invalid {
            p.error
        } else if focused {
            p.accent
        } else {
            theme::mix(p.border, p.text, 0.35 * hover)
        };
        let emphasis = if focused || self.invalid { 1.5 } else { 1.0 };
        let hint = egui::RichText::new(&self.hint)
            .size(font_size)
            .color(theme::color(p.muted));
        let frame = egui::Frame::NONE
            .fill(theme::color(p.panel_background))
            // Always one point, so focus doesn't move what's around it; the
            // emphasis is painted inside it, below.
            .stroke(egui::Stroke::new(1.0, theme::color(border)))
            .corner_radius(Radius::Md.px(ui))
            .inner_margin(egui::Margin::symmetric(padding.x as i8, margin_y as i8));
        let shown = frame.show(ui, |ui| {
            egui::TextEdit::multiline(&mut *self.text)
                .id(id)
                .frame(egui::Frame::NONE)
                .margin(0.0)
                .background_color(egui::Color32::TRANSPARENT)
                .font(egui::FontId::monospace(font_size))
                .text_color(theme::color(p.text))
                .hint_text(hint)
                .desired_rows(self.rows)
                .min_size(egui::vec2(0.0, min_height))
                .desired_width(width - 2.0 * padding.x)
                .show(ui)
                .response
                .response
        });
        // The whole box is the control, not only the text inside it.
        let mut response = shown.inner;
        response.rect = shown.response.rect;
        if emphasis > 1.0 {
            ui.painter().rect_stroke(
                response.rect,
                Radius::Md.px(ui),
                egui::Stroke::new(emphasis, theme::color(border)),
                egui::StrokeKind::Inside,
            );
        }
        if !self.label.is_empty() {
            let enabled = ui.is_enabled();
            response.widget_info(|| {
                let mut info =
                    egui::WidgetInfo::text_edit(enabled, &*self.text, &*self.text, &self.hint);
                info.label = Some(self.label.clone());
                info
            });
        }
        response.id = id;
        response
    }
}
