use super::Spinner;
use crate::{IconSource, ambient, motion, theme, tokens::*};

/// One of a list's rows, to choose: its text cut to fit, with what can come
/// before it (an icon, a short tag) and after it (a count, a spinner).
///
/// ```ignore
/// if ui.add(ListRow::new(name).selected(chosen == id).tag("GET", green)).clicked() { … }
/// ```
#[must_use = "a row does nothing until it's added to a Ui"]
pub struct ListRow {
    text: String,
    selected: bool,
    icon: Option<IconSource>,
    tag: Option<(String, egui::Color32, f32)>,
    trailing: String,
    busy: bool,
    italic: bool,
    strikethrough: bool,
    size: Size,
}

impl ListRow {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            selected: false,
            icon: None,
            tag: None,
            trailing: String::new(),
            busy: false,
            italic: false,
            strikethrough: false,
            size: Size::default(),
        }
    }

    /// The chosen one.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    /// An icon before the text.
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    /// A few small letters before the text, in a colour of the caller's (a
    /// kind: GET, POST). `widest` is the longest tag in the list, so the
    /// rows' texts line up.
    pub fn tag(mut self, tag: impl Into<String>, color: egui::Color32, widest: &str) -> Self {
        self.tag = Some((tag.into(), color, widest.chars().count() as f32));
        self
    }
    /// Muted text at its end (a count, a size).
    pub fn trailing(mut self, trailing: impl Into<String>) -> Self {
        self.trailing = trailing.into();
        self
    }
    /// A spinner at its end: what it stands for is working.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }
    /// Its text struck through: what it stands for is gone, hidden or done.
    pub fn strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
}

impl egui::Widget for ListRow {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let height = self.size.height(ui);
        let padding = self.size.padding(ui).x;
        let gap = (font_size * 0.4).round();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                enabled,
                self.selected,
                &self.text,
            )
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let animate = |name: &str, on: bool| {
            let target = if on { 1.0 } else { 0.0 };
            motion::scalar(
                ui.ctx(),
                response.id.with(name),
                target,
                target,
                services.motion,
            )
        };
        let hover = animate("hover", response.hovered() || response.has_focus());
        let chosen = animate("selected", self.selected);
        let p = &services.theme.palette;
        let mut fill = theme::mix(
            theme::mix(p.panel_background, p.text, 0.08),
            p.selection,
            chosen,
        );
        fill[3] *= hover.max(chosen);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, Radius::Md.px(ui), theme::color(fill));
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                Radius::Md.px(ui),
                egui::Stroke::new(1.5, theme::color(p.accent)),
                egui::StrokeKind::Inside,
            );
        }

        let ink = theme::color(p.text);
        let mut left = rect.left() + padding;
        let mut right = rect.right() - padding;
        if let Some(icon) = &self.icon {
            let pixels = (font_size * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
            if let Some(texture) = services.icons.lock().unwrap().texture(icon, pixels) {
                painter.image(
                    texture,
                    egui::Rect::from_min_size(
                        egui::pos2(left, rect.center().y - font_size / 2.0),
                        egui::Vec2::splat(font_size),
                    ),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    ink,
                );
            }
            left += font_size + gap;
        }
        if let Some((tag, color, widest)) = &self.tag {
            let font = egui::FontId::monospace(font_size * 0.72);
            let galley = painter.layout_no_wrap(tag.clone(), font.clone(), *color);
            // Every tag's room is the widest one's (the font is monospace).
            let room = galley.size().x / tag.chars().count().max(1) as f32 * widest.max(1.0);
            painter.galley(
                egui::pos2(left, rect.center().y - galley.size().y / 2.0),
                galley,
                *color,
            );
            left += room + gap;
        }
        if self.busy {
            let side = font_size * 0.8;
            Spinner::new().size(self.size).paint_at(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(right - side / 2.0, rect.center().y),
                    egui::Vec2::splat(side),
                ),
            );
            right -= side + gap;
        }
        if !self.trailing.is_empty() {
            let galley = painter.layout_no_wrap(
                self.trailing.clone(),
                egui::FontId::proportional(font_size * 0.85),
                theme::color(p.muted),
            );
            right -= galley.size().x;
            painter.galley(
                egui::pos2(right, rect.center().y - galley.size().y / 2.0),
                galley,
                theme::color(p.muted),
            );
            right -= gap;
        }
        let line = if self.strikethrough {
            egui::Stroke::new(1.0, ink)
        } else {
            egui::Stroke::NONE
        };
        let mut job = egui::text::LayoutJob::single_section(
            self.text.clone(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(font_size),
                color: ink,
                italics: self.italic,
                strikethrough: line,
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: (right - left).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
        response
    }
}

/// One of a menu's items: it does its thing and the menu closes.
///
/// ```ignore
/// response.context_menu(|ui| {
///     if ui.add(MenuItem::new("Duplicate")).clicked() { … }
///     if ui.add(MenuItem::new("Delete…").danger()).clicked() { … }
/// });
/// ```
#[must_use = "a menu item does nothing until it's added to a Ui"]
pub struct MenuItem {
    text: String,
    icon: Option<IconSource>,
    shortcut: String,
    danger: bool,
    checked: Option<bool>,
}

impl MenuItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            icon: None,
            shortcut: String::new(),
            danger: false,
            checked: None,
        }
    }
    /// An icon before the text.
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    /// Its keys, shown at its end.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = shortcut.into();
        self
    }
    /// Something that's on or off, or one of several chosen: a tick before
    /// it when it's on. A click still only tells you; the change is yours.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
    /// It destroys something: in the error colour.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

impl egui::Widget for MenuItem {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = Size::Md.font(ui);
        let height = Size::Md.height(ui);
        // A menu's own padding, so items line up with its submenus' (which
        // are egui's).
        let padding = ui.spacing().button_padding.x;
        let gap = (font_size * 0.4).round();
        let p = &services.theme.palette;
        let ink = theme::color(if self.danger { p.error } else { p.text });
        let font = egui::FontId::proportional(font_size);
        let text = ui
            .painter()
            .layout_no_wrap(self.text.clone(), font.clone(), ink);
        let keys = (!self.shortcut.is_empty()).then(|| {
            ui.painter().layout_no_wrap(
                self.shortcut.clone(),
                egui::FontId::proportional(font_size * 0.85),
                theme::color(p.muted),
            )
        });
        let icon = if self.icon.is_some() || self.checked.is_some() {
            font_size + gap
        } else {
            0.0
        };
        let wanted = 2.0 * padding
            + icon
            + text.size().x
            + keys
                .as_ref()
                .map_or(0.0, |keys| Space::Lg.px(ui) + keys.size().x);
        // As wide as the menu, which is as wide as its widest item: while
        // the menu is being measured, only what this item needs.
        let width = if ui.is_sizing_pass() {
            wanted
        } else {
            wanted.max(ui.available_width())
        };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| match self.checked {
            Some(checked) => {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, &self.text)
            }
            None => egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &self.text),
        });
        if response.clicked() {
            ui.close();
        }
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let lit = response.hovered() || response.has_focus();
        let target = if lit { 1.0 } else { 0.0 };
        let hover = motion::scalar(
            ui.ctx(),
            response.id.with("hover"),
            target,
            target,
            services.motion,
        );
        let mut fill = theme::mix(
            p.elevated,
            if self.danger { p.error } else { p.accent },
            0.25,
        );
        fill[3] *= hover;
        let painter = ui.painter();
        painter.rect_filled(rect, Radius::Md.px(ui), theme::color(fill));
        let mut left = rect.left() + padding;
        if let Some(source) = &self.icon {
            let pixels = (font_size * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
            if let Some(texture) = services.icons.lock().unwrap().texture(source, pixels) {
                painter.image(
                    texture,
                    egui::Rect::from_min_size(
                        egui::pos2(left, rect.center().y - font_size / 2.0),
                        egui::Vec2::splat(font_size),
                    ),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    ink,
                );
            }
            left += icon;
        }
        if self.icon.is_none() && self.checked.is_some() {
            if self.checked == Some(true) {
                let side = font_size;
                let square = egui::Rect::from_min_size(
                    egui::pos2(left, rect.center().y - side / 2.0),
                    egui::Vec2::splat(side),
                );
                let at = |x: f32, y: f32| square.min + egui::vec2(x, y) * side;
                painter.line(
                    vec![at(0.18, 0.52), at(0.4, 0.74), at(0.82, 0.28)],
                    egui::Stroke::new((side / 9.0).max(1.5), ink),
                );
            }
            left += icon;
        }
        painter.galley(
            egui::pos2(left, rect.center().y - text.size().y / 2.0),
            text,
            ink,
        );
        if let Some(keys) = keys {
            painter.galley(
                egui::pos2(
                    rect.right() - padding - keys.size().x,
                    rect.center().y - keys.size().y / 2.0,
                ),
                keys,
                theme::color(p.muted),
            );
        }
        response
    }
}

/// A menu inside a menu: an item that opens more items beside it.
///
/// ```ignore
/// Submenu::new("Open With").show(ui, |ui| {
///     if ui.add(MenuItem::new("Hex Viewer")).clicked() { … }
/// });
/// ```
#[must_use = "a submenu does nothing until it's shown"]
pub struct Submenu {
    text: String,
}

impl Submenu {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn show(self, ui: &mut egui::Ui, items: impl FnOnce(&mut egui::Ui)) -> egui::Response {
        // The item, its arrow and how it opens are egui's menu's own.
        ui.menu_button(self.text, items).response
    }
}
