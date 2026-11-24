use super::Button;
use crate::{BuiltinIcon, IconSource, ambient, motion, theme, tokens::*};

/// A line of text to edit.
///
/// ```ignore
/// ui.add(TextField::new(&mut query).hint("Search").icon(BuiltinIcon::Search).clearable());
/// ```
///
/// The editing itself (the caret, selection, clipboard, input methods) is
/// egui's `TextEdit`; the field is its look and what's around it.
#[must_use = "a text field does nothing until it's added to a Ui"]
pub struct TextField<'a> {
    text: &'a mut String,
    hint: String,
    label: String,
    icon: Option<IconSource>,
    size: Size,
    width: Option<f32>,
    fill_width: bool,
    clearable: bool,
    invalid: bool,
    password: bool,
    focus: bool,
}

impl<'a> TextField<'a> {
    pub fn new(text: &'a mut String) -> Self {
        Self {
            text,
            hint: String::new(),
            label: String::new(),
            icon: None,
            size: Size::default(),
            width: None,
            fill_width: false,
            clearable: false,
            invalid: false,
            password: false,
            focus: false,
        }
    }

    /// Shown while it's empty; its name too, unless `label` gives one.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
    /// Its name, for accessibility and scripts.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    /// An icon before the text.
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
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
    /// A button at its end, while there's text, that empties it.
    pub fn clearable(mut self) -> Self {
        self.clearable = true;
        self
    }
    /// What's in it is wrong: outlined in the error colour.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }
    /// Show dots, not the text.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }
    /// Take the keyboard now (a field that's just appeared, say).
    pub fn focus(mut self, focus: bool) -> Self {
        self.focus = focus;
        self
    }
}

impl egui::Widget for TextField<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let padding = self.size.padding(ui);
        let height = self.size.height(ui);
        let available = ui.available_width();
        let width = if self.fill_width {
            available
        } else {
            self.width
                .unwrap_or(ui.spacing().text_edit_width)
                .min(available)
        }
        .max(height * 2.0);
        let (rect, frame) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let id = frame.id.with("edit");
        if self.focus {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        let focused = ui.memory(|memory| memory.has_focus(id));

        let p = &services.theme.palette;
        let hover_target = if frame.hovered() { 1.0 } else { 0.0 };
        let hover = motion::scalar(
            ui.ctx(),
            id.with("hover"),
            hover_target,
            hover_target,
            services.motion,
        );
        let border = if self.invalid {
            p.error
        } else if focused {
            p.accent
        } else {
            theme::mix(p.border, p.text, 0.35 * hover)
        };
        let emphasis = if focused || self.invalid { 1.5 } else { 1.0 };
        ui.painter().rect(
            rect,
            Radius::Md.px(ui),
            theme::color(p.panel_background),
            egui::Stroke::new(emphasis, theme::color(border)),
            egui::StrokeKind::Inside,
        );

        let gap = (font_size * 0.4).round();
        let mut inner = rect.shrink2(egui::vec2(padding.x, 0.0));
        if let Some(icon) = &self.icon {
            let side = font_size;
            let pixels = (side * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
            let texture = services.icons.lock().unwrap().texture(icon, pixels);
            if let Some(texture) = texture {
                ui.painter().image(
                    texture,
                    egui::Rect::from_min_size(
                        egui::pos2(inner.left(), rect.center().y - side / 2.0),
                        egui::Vec2::splat(side),
                    ),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    theme::color(p.muted),
                );
            }
            inner.min.x += side + gap;
        }
        let clear = (self.clearable && !self.text.is_empty()).then(|| {
            let side = Size::Sm.height(ui).min(height - 4.0);
            let at = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 2.0 - side / 2.0, rect.center().y),
                egui::Vec2::splat(side),
            );
            inner.max.x = at.left() - 2.0;
            at
        });

        let hint = egui::RichText::new(&self.hint)
            .size(font_size)
            .color(theme::color(p.muted));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let output = egui::TextEdit::singleline(&mut *self.text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(0.0)
            .background_color(egui::Color32::TRANSPARENT)
            .font(egui::FontId::proportional(font_size))
            .text_color(theme::color(p.text))
            .desired_width(inner.width())
            .hint_text(hint)
            .password(self.password)
            .vertical_align(egui::Align::Center)
            .show(&mut child);
        let mut response = output.response.response;
        // A hint isn't a name: say what the field is.
        let name = if self.label.is_empty() {
            &self.hint
        } else {
            &self.label
        };
        if !name.is_empty() {
            let enabled = ui.is_enabled();
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, enabled, name)
            });
        }
        if let Some(at) = clear {
            let button = Button::icon_only(BuiltinIcon::Close, "Clear")
                .size(Size::Sm)
                .ghost();
            if ui.put(at, button).clicked() {
                self.text.clear();
                response.mark_changed();
                ui.memory_mut(|memory| memory.request_focus(id));
            }
        }
        response
    }
}
