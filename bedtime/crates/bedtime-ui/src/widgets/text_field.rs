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
    plain: bool,
    muted: bool,
    id: Option<egui::Id>,
    char_limit: usize,
    event_filter: Option<egui::EventFilter>,
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
            plain: false,
            muted: false,
            id: None,
            char_limit: usize::MAX,
            event_filter: None,
        }
    }

    /// The most characters it takes.
    pub fn char_limit(mut self, limit: usize) -> Self {
        self.char_limit = limit;
        self
    }
    /// The keys it keeps while it has the keyboard, which would otherwise
    /// move focus or be left for something else (Escape, the arrows, Tab).
    pub fn event_filter(mut self, filter: egui::EventFilter) -> Self {
        self.event_filter = Some(filter);
        self
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
    /// No box around it until it's pointed at or has the keyboard: a title
    /// that can be edited where it stands.
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
    /// Its text in the muted colour: what it holds isn't in use.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }
    /// The id of its text, to ask after its focus from elsewhere
    /// (`memory.has_focus(id)`).
    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = Some(id);
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
            super::fill_width(ui, ui.spacing().text_edit_width)
        } else {
            self.width
                .unwrap_or(ui.spacing().text_edit_width)
                .min(available)
        }
        .max(height * 2.0);
        let (rect, frame) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let id = self.id.unwrap_or(frame.id.with("edit"));
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
        // A plain field's box fades in when it's pointed at or being edited.
        let boxed = if !self.plain || focused || self.invalid {
            1.0
        } else {
            hover
        };
        if boxed > 0.0 {
            ui.painter().rect(
                rect,
                Radius::Md.px(ui),
                theme::color(p.panel_background).gamma_multiply(boxed),
                egui::Stroke::new(emphasis, theme::color(border).gamma_multiply(boxed)),
                egui::StrokeKind::Inside,
            );
        }

        let gap = (font_size * 0.4).round();
        // A plain field's text starts where the field does.
        let inset = if self.plain { 0.0 } else { padding.x };
        let mut inner = rect.shrink2(egui::vec2(inset, 0.0));
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
        let mut edit = egui::TextEdit::singleline(&mut *self.text).char_limit(self.char_limit);
        if let Some(filter) = self.event_filter {
            edit = edit.event_filter(filter);
        }
        let output = edit
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(0.0)
            .background_color(egui::Color32::TRANSPARENT)
            .font(egui::FontId::proportional(font_size))
            .text_color(theme::color(if self.muted { p.muted } else { p.text }))
            .desired_width(inner.width())
            .hint_text(hint)
            .password(self.password)
            .vertical_align(egui::Align::Center)
            .show(&mut child);
        let mut response = output.response.response;
        // Named, it's still a text field with its text: scripts and screen
        // readers get both.
        if !self.label.is_empty() {
            let enabled = ui.is_enabled();
            response.widget_info(|| {
                let mut info =
                    egui::WidgetInfo::text_edit(enabled, &*self.text, &*self.text, &self.hint);
                info.label = Some(self.label.clone());
                info
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
