use crate::{ambient, caret, motion, theme, tokens::*};

/// Several lines of text to edit.
///
/// ```ignore
/// ui.add(TextArea::new(&mut body).hint("Request body").rows(8).fill_width());
/// ```
///
/// The editing itself is egui's `TextEdit`, under the editor's caret; for
/// code with highlighting, the editor and `CodeView` are the places.
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
    plain: bool,
    font: Option<egui::FontId>,
    color: Option<egui::Color32>,
    align: egui::Align,
    unclipped: bool,
    keeps_tab: bool,
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
            plain: false,
            font: None,
            color: None,
            align: egui::Align::Min,
            unclipped: false,
            keeps_tab: false,
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
    /// No box or padding around it: text edited where it stands (a note's
    /// body, a label on a canvas).
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
    /// Its text's font (the monospace one at the body size, unless set).
    pub fn font(mut self, font: egui::FontId) -> Self {
        self.font = Some(font);
        self
    }
    /// Its text's colour (the theme's text colour, unless set).
    pub fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }
    /// Where its lines sit across it (at its start, unless set).
    pub fn align(mut self, align: egui::Align) -> Self {
        self.align = align;
        self
    }
    /// Let text wider than it show past its edges.
    pub fn unclipped(mut self) -> Self {
        self.unclipped = true;
        self
    }
    /// Tab types a tab, where it would otherwise move to the next control.
    pub fn keeps_tab(mut self) -> Self {
        self.keeps_tab = true;
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
            super::fill_width(ui, ui.spacing().text_edit_width)
        } else {
            self.width
                .unwrap_or(ui.spacing().text_edit_width)
                .min(available)
        };
        let id = self
            .id
            .unwrap_or_else(|| ui.next_auto_id().with("text-area"));
        let margin_y = padding.y + 2.0;
        // What's around the text, each way: the padding and the border.
        let around = if self.plain {
            egui::Vec2::ZERO
        } else {
            egui::vec2(padding.x, margin_y + 1.0)
        };
        let min_height = if self.fill_height {
            (ui.available_height() - 2.0 * around.y).max(0.0)
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
        let font = self
            .font
            .clone()
            .unwrap_or(egui::FontId::monospace(font_size));
        // In the text's font when that's been chosen.
        let hint = match &self.font {
            Some(font) => egui::RichText::new(&self.hint).font(font.clone()),
            None => egui::RichText::new(&self.hint).size(font_size),
        }
        .color(theme::color(p.muted));
        let color = self.color.unwrap_or(theme::color(p.text));
        let (rows, align, unclipped, keeps_tab) =
            (self.rows, self.align, self.unclipped, self.keeps_tab);
        let text = &mut *self.text;
        let edit = |ui: &mut egui::Ui| {
            caret::text_edit(ui, id, &font, services.motion, |ui| {
                egui::TextEdit::multiline(text)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(0.0)
                    .background_color(egui::Color32::TRANSPARENT)
                    .font(font.clone())
                    .text_color(color)
                    .hint_text(hint)
                    .desired_rows(rows)
                    .min_size(egui::vec2(0.0, min_height))
                    .desired_width(width - 2.0 * around.x)
                    .horizontal_align(align)
                    .clip_text(!unclipped)
                    .lock_focus(keeps_tab)
                    .show(ui)
            })
            .response
            .response
        };
        let mut response = if self.plain {
            edit(ui)
        } else {
            let frame = egui::Frame::NONE
                .fill(theme::color(p.panel_background))
                // Always one point, so focus doesn't move what's around it;
                // the emphasis is painted inside it, below.
                .stroke(egui::Stroke::new(1.0, theme::color(border)))
                .corner_radius(Radius::Md.px(ui))
                .inner_margin(egui::Margin::symmetric(padding.x as i8, margin_y as i8));
            let shown = frame.show(ui, edit);
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
            response
        };
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
