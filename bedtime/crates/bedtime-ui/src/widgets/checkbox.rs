use crate::{ambient, motion, theme, tokens::*};

/// A box that's ticked or not, with its label.
///
/// ```ignore
/// ui.add(Checkbox::new(&mut wrap, "Wrap long lines"));
/// ```
#[must_use = "a checkbox does nothing until it's added to a Ui"]
pub struct Checkbox<'a> {
    value: &'a mut bool,
    label: String,
    size: Size,
    indeterminate: bool,
}

impl<'a> Checkbox<'a> {
    pub fn new(value: &'a mut bool, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            size: Size::default(),
            indeterminate: false,
        }
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    /// Drawn as neither: some of what it stands for is on, some isn't.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }
}

impl egui::Widget for Checkbox<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let height = self.size.height(ui);
        let side = (font_size * 1.05).round();
        let gap = (font_size * 0.5).round();
        let galley = (!self.label.is_empty()).then(|| {
            ui.painter().layout_no_wrap(
                self.label.clone(),
                egui::FontId::proportional(font_size),
                egui::Color32::PLACEHOLDER,
            )
        });
        let width = side + galley.as_ref().map_or(0.0, |galley| gap + galley.size().x);
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        if response.clicked() {
            *self.value = !*self.value;
            response.mark_changed();
        }
        let (enabled, on) = (ui.is_enabled(), *self.value);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, &self.label)
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
        let filled = animate("on", on || self.indeterminate);
        let hover = animate("hover", response.hovered() || response.has_focus());
        let p = &services.theme.palette;
        let surface = theme::distinct(p.surface, p.panel_background, p.text);
        let mut mark = p.panel_background;
        mark[3] = 1.0;
        let square = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - side / 2.0),
            egui::Vec2::splat(side),
        );
        let fill = theme::mix(
            theme::mix(surface, p.accent, 0.2 * hover),
            theme::mix(p.accent, p.text, 0.2 * hover),
            filled,
        );
        let border = theme::mix(theme::mix(p.border, p.text, 0.4 * hover), p.accent, filled);
        let radius = Radius::Sm.px(ui).max(2.0);
        let painter = ui.painter();
        painter.rect(
            square,
            radius,
            theme::color(fill),
            egui::Stroke::new(1.0, theme::color(border)),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            painter.rect_stroke(
                square.expand(2.0),
                radius + 2.0,
                egui::Stroke::new(1.5, theme::color(p.accent)),
                egui::StrokeKind::Inside,
            );
        }
        if filled > 0.0 {
            let mut ink = mark;
            ink[3] = filled;
            let stroke = egui::Stroke::new((side / 9.0).max(1.5), theme::color(ink));
            let at = |x: f32, y: f32| square.min + egui::vec2(x, y) * side;
            if self.indeterminate && !on {
                painter.line_segment([at(0.26, 0.5), at(0.74, 0.5)], stroke);
            } else {
                painter.line(vec![at(0.24, 0.52), at(0.43, 0.7), at(0.77, 0.32)], stroke);
            }
        }
        if let Some(galley) = galley {
            let at = egui::pos2(
                square.right() + gap,
                rect.center().y - galley.size().y / 2.0,
            );
            painter.galley(at, galley, theme::color(p.text));
        }
        response
    }
}

/// A switch that's on or off, with its label: for what takes effect at once
/// (a `Checkbox` is for a choice among others in a form).
///
/// ```ignore
/// ui.add(Toggle::new(&mut autosave, "Autosave"));
/// ```
#[must_use = "a toggle does nothing until it's added to a Ui"]
pub struct Toggle<'a> {
    value: &'a mut bool,
    label: String,
    size: Size,
}

impl<'a> Toggle<'a> {
    pub fn new(value: &'a mut bool, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            size: Size::default(),
        }
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
}

impl egui::Widget for Toggle<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font_size = self.size.font(ui);
        let height = self.size.height(ui);
        // A track a little inside the control's height.
        let track_size = egui::vec2((height * 1.6).round(), height - 6.0);
        let gap = (font_size * 0.6).round();
        let galley = (!self.label.is_empty()).then(|| {
            ui.painter().layout_no_wrap(
                self.label.clone(),
                egui::FontId::proportional(font_size),
                egui::Color32::PLACEHOLDER,
            )
        });
        let width = track_size.x + galley.as_ref().map_or(0.0, |galley| gap + galley.size().x);
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        if response.clicked() {
            *self.value = !*self.value;
            response.mark_changed();
        }
        let (enabled, on) = (ui.is_enabled(), *self.value);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, &self.label)
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
        let travel = animate("on", on);
        let hover = animate("hover", response.hovered() || response.has_focus());
        let p = &services.theme.palette;
        let track = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - track_size.y / 2.0),
            track_size,
        );
        let fill = theme::mix(theme::mix(p.border, p.accent, travel), p.text, 0.12 * hover);
        let painter = ui.painter();
        painter.rect_filled(track, track_size.y / 2.0, theme::color(fill));
        if response.has_focus() {
            painter.rect_stroke(
                track.expand(2.0),
                track_size.y / 2.0 + 2.0,
                egui::Stroke::new(1.5, theme::color(p.accent)),
                egui::StrokeKind::Inside,
            );
        }
        let mut knob = p.panel_background;
        knob[3] = 1.0;
        painter.circle_filled(
            egui::pos2(
                track.left() + track_size.y / 2.0 + travel * (track_size.x - track_size.y),
                track.center().y,
            ),
            track_size.y / 2.0 - 3.0,
            theme::color(theme::mix(p.text, knob, travel)),
        );
        if let Some(galley) = galley {
            let at = egui::pos2(track.right() + gap, rect.center().y - galley.size().y / 2.0);
            painter.galley(at, galley, theme::color(p.text));
        }
        response
    }
}
