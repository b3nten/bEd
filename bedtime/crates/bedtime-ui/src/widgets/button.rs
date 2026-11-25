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
    italic: bool,
    strikethrough: bool,
    min_width: f32,
    muted: bool,
    width: Option<f32>,
    dropdown: bool,
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
            italic: false,
            strikethrough: false,
            min_width: 0.0,
            muted: false,
            width: None,
            dropdown: false,
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

    /// At least this wide, in points: buttons that take each other's place
    /// (Send, Cancel) at one width.
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    /// Exactly this wide, in points: its text is cut short with an ellipsis
    /// when it doesn't fit (a button sharing a row that mustn't grow).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// A small arrow at its end: it opens a menu or a list (see `MenuButton`).
    pub fn dropdown(mut self) -> Self {
        self.dropdown = true;
        self
    }

    /// The width it would have in `ui`, to leave room for it beside
    /// something that takes the rest.
    pub fn width_in(&self, ui: &egui::Ui) -> f32 {
        let font_size = self.size.font(ui);
        let text = if self.text.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(
                    self.text.clone(),
                    egui::FontId::proportional(font_size),
                    egui::Color32::PLACEHOLDER,
                )
                .size()
                .x
        };
        let width = match (&self.icon, text > 0.0) {
            (_, false) => self.size.height(ui),
            (Some(_), true) => {
                font_size + (font_size * 0.4).round() + text + 2.0 * self.size.padding(ui).x
            }
            (None, true) => text + 2.0 * self.size.padding(ui).x,
        };
        width.max(self.min_width)
    }

    /// Where its content sits when it's wider than that (centred, unless set).
    pub fn align(mut self, align: egui::Align) -> Self {
        self.align = align;
        self
    }

    /// Its text in the muted colour: it stands for nothing yet ("Not set").
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
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
        let icon_side = font_size;
        let gap = (font_size * 0.4).round();
        let arrow = if self.dropdown {
            (font_size * 0.7).round() + gap
        } else {
            0.0
        };
        let galley = (!self.text.is_empty()).then(|| {
            let line = if self.strikethrough {
                egui::Stroke::new(1.0, ui.visuals().text_color())
            } else {
                egui::Stroke::NONE
            };
            let mut job = egui::text::LayoutJob::single_section(
                self.text.clone(),
                egui::TextFormat {
                    font_id: egui::FontId::proportional(font_size),
                    color: egui::Color32::PLACEHOLDER,
                    italics: self.italic,
                    strikethrough: line,
                    ..Default::default()
                },
            );
            // At a set width, what doesn't fit is cut short.
            if let Some(width) = self.width {
                let beside = if self.icon.is_some() {
                    icon_side + gap
                } else {
                    0.0
                };
                let room = (width - 2.0 * padding.x - beside - arrow).max(0.0);
                job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            }
            ui.painter().layout_job(job)
        });
        let content = match (&self.icon, &galley) {
            (Some(_), Some(galley)) => icon_side + gap + galley.size().x,
            (Some(_), None) => icon_side,
            (None, Some(galley)) => galley.size().x,
            (None, None) => 0.0,
        };
        let mut size = if galley.is_none() && !self.dropdown {
            egui::Vec2::splat(height)
        } else if galley.is_none() {
            // An arrow alone, or an icon and an arrow.
            egui::vec2(height.max(content + arrow - gap + 2.0 * padding.x), height)
        } else {
            egui::vec2(content + arrow + 2.0 * padding.x, height)
        };
        if let Some(width) = self.width {
            size.x = width;
        }
        size.x = size.x.max(self.min_width);
        if self.fill_width {
            size.x = size.x.max(super::fill_width(ui, 0.0));
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
        let hover = animate("hover", response.hovered() || response.has_focus());
        let chosen = animate("selected", self.selected);
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
            // Selected, these are filled with the selection colour, as a
            // selected row is.
            Variant::Secondary => (
                theme::mix(
                    theme::mix(surface, p.accent, 0.2 * hover + 0.15 * press),
                    p.selection,
                    chosen,
                ),
                p.text,
                true,
            ),
            Variant::Ghost => {
                let mut fill = theme::mix(
                    theme::mix(surface, p.accent, 0.2 * press),
                    p.selection,
                    chosen,
                );
                fill[3] *= hover.max(press).max(chosen);
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

        let ink = theme::color(if self.muted { p.muted } else { ink });
        let align = if self.dropdown && content > 0.0 {
            egui::Align::Min
        } else {
            self.align
        };
        let mut x = match align {
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
        if self.dropdown {
            let side = (font_size * 0.7).round();
            let center = if content > 0.0 {
                egui::pos2(rect.right() - padding.x - side / 2.0, rect.center().y)
            } else {
                rect.center()
            };
            crate::paint_chevron(painter, center, 1.0, ink.gamma_multiply(0.8));
        }
        response
    }
}
