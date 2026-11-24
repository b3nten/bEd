use crate::{IconSource, ambient, motion, theme, tokens::*};

/// What a button is for, as a look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    /// The one thing to do here.
    Primary,
    /// Anything else.
    #[default]
    Secondary,
    /// No background until it's pointed at: toolbars, rows of a list.
    Ghost,
    /// Something that destroys.
    Danger,
}

impl Variant {
    pub const ALL: [Self; 4] = [Self::Primary, Self::Secondary, Self::Ghost, Self::Danger];
}

/// A button: text, an icon, or both.
///
/// ```ignore
/// if ui.add(Button::new("Save").primary().icon(BuiltinIcon::Save)).clicked() { … }
/// ```
#[must_use = "a button does nothing until it's added to a Ui"]
pub struct Button {
    text: String,
    /// What it's called when it shows no text.
    label: String,
    icon: Option<IconSource>,
    variant: Variant,
    size: Size,
    fill_width: bool,
    align: egui::Align,
    selected: bool,
}

impl Button {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            label: String::new(),
            icon: None,
            variant: Variant::default(),
            size: Size::default(),
            fill_width: false,
            align: egui::Align::Center,
            selected: false,
        }
    }

    /// A square button of an icon alone. `label` names it for accessibility
    /// and scripts.
    pub fn icon_only(icon: impl Into<IconSource>, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: Some(icon.into()),
            ..Self::new("")
        }
    }

    /// An icon before the text.
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }
    pub fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(Size::Sm)
    }
    pub fn large(self) -> Self {
        self.size(Size::Lg)
    }

    /// As wide as the space it's in.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }

    /// Where its content sits when it's wider than that (centred, unless set).
    pub fn align(mut self, align: egui::Align) -> Self {
        self.align = align;
        self
    }

    /// Drawn as the chosen one of several (a list's current row, a tool
    /// that's on).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl egui::Widget for Button {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let padding = self.size.padding(ui);
        let height = self.size.height(ui);
        let galley = (!self.text.is_empty()).then(|| {
            ui.painter().layout_no_wrap(
                self.text.clone(),
                egui::FontId::proportional(font_size),
                egui::Color32::PLACEHOLDER,
            )
        });
        let icon_side = font_size;
        let gap = (font_size * 0.4).round();
        let content = match (&self.icon, &galley) {
            (Some(_), Some(galley)) => icon_side + gap + galley.size().x,
            (Some(_), None) => icon_side,
            (None, Some(galley)) => galley.size().x,
            (None, None) => 0.0,
        };
        let mut size = if galley.is_none() {
            egui::Vec2::splat(height)
        } else {
            egui::vec2(content + 2.0 * padding.x, height)
        };
        if self.fill_width {
            size.x = size.x.max(ui.available_width());
        }
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let enabled = ui.is_enabled();
        let name = if self.text.is_empty() {
            &self.label
        } else {
            &self.text
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
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
        let hover = animate(
            "hover",
            response.hovered() || response.has_focus() || self.selected,
        );
        let press = animate("press", response.is_pointer_button_down_on());

        let p = &services.theme.palette;
        let surface = theme::distinct(p.surface, p.panel_background, p.text);
        let mut on_fill = p.panel_background;
        on_fill[3] = 1.0;
        let filled = |fill: theme::Rgba| {
            theme::mix(theme::mix(fill, p.text, 0.2 * hover), on_fill, 0.2 * press)
        };
        let (fill, ink, border) = match self.variant {
            Variant::Primary => (filled(p.accent), on_fill, false),
            Variant::Danger => (filled(p.error), on_fill, false),
            Variant::Secondary => (
                theme::mix(surface, p.accent, 0.2 * hover + 0.15 * press),
                p.text,
                true,
            ),
            Variant::Ghost => {
                let mut fill = theme::mix(surface, p.accent, 0.2 * press);
                fill[3] *= hover.max(press);
                (fill, p.text, false)
            }
        };
        let radius = Radius::Md.px(ui);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, theme::color(fill));
        if border {
            painter.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.0, theme::color(p.border)),
                egui::StrokeKind::Inside,
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                radius + 2.0,
                egui::Stroke::new(1.5, theme::color(p.accent)),
                egui::StrokeKind::Inside,
            );
        }

        let ink = theme::color(ink);
        let mut x = match self.align {
            egui::Align::Min => rect.left() + padding.x,
            egui::Align::Center => rect.center().x - content / 2.0,
            egui::Align::Max => rect.right() - padding.x - content,
        };
        if let Some(icon) = &self.icon {
            let pixels = (icon_side * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
            let texture = services.icons.lock().unwrap().texture(icon, pixels);
            if let Some(texture) = texture {
                painter.image(
                    texture,
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.center().y - icon_side / 2.0),
                        egui::Vec2::splat(icon_side),
                    ),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    ink,
                );
            }
            x += icon_side + gap;
        }
        if let Some(galley) = galley {
            let y = rect.center().y - galley.size().y / 2.0;
            painter.galley(egui::pos2(x, y), galley, ink);
        }
        response
    }
}
