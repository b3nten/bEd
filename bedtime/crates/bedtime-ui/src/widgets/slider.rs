use crate::{ambient, motion, theme, tokens::*};
use egui::emath::Numeric;
use std::ops::RangeInclusive;

/// A number chosen by dragging along a track.
///
/// ```ignore
/// ui.add(Slider::new(&mut zoom, 0.25..=64.0).logarithmic().label("Time zoom"));
/// ```
///
/// The arrow keys move it while it has the keyboard.
#[must_use = "a slider does nothing until it's added to a Ui"]
pub struct Slider<'a, N: Numeric> {
    value: &'a mut N,
    range: RangeInclusive<N>,
    logarithmic: bool,
    step: Option<f64>,
    label: String,
    size: Size,
    width: Option<f32>,
    fill_width: bool,
}

impl<'a, N: Numeric> Slider<'a, N> {
    pub fn new(value: &'a mut N, range: RangeInclusive<N>) -> Self {
        Self {
            value,
            range,
            logarithmic: false,
            step: None,
            label: String::new(),
            size: Size::default(),
            width: None,
            fill_width: false,
        }
    }

    /// Even steps multiply the value (for a range over orders of magnitude;
    /// its ends must be above zero).
    pub fn logarithmic(mut self) -> Self {
        self.logarithmic = true;
        self
    }
    /// Only multiples of `step` from the range's start.
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }
    /// Its name, for accessibility and scripts.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    /// This wide, in points (egui's slider width, unless set).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    /// As wide as the space it's in.
    pub fn fill_width(mut self) -> Self {
        self.fill_width = true;
        self
    }
}

impl<N: Numeric> egui::Widget for Slider<'_, N> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let height = self.size.height(ui);
        let knob = (height * 0.3).round();
        let available = ui.available_width();
        let width = if self.fill_width {
            super::fill_width(ui, ui.spacing().slider_width)
        } else {
            self.width
                .unwrap_or(ui.spacing().slider_width)
                .min(available)
        }
        .max(knob * 4.0);
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());

        let (low, high) = (self.range.start().to_f64(), self.range.end().to_f64());
        let logarithmic = self.logarithmic && low > 0.0 && high > low;
        // Where a value is along the track, 0 to 1, and back.
        let along = |value: f64| -> f64 {
            if high <= low {
                0.0
            } else if logarithmic {
                (value.max(low).ln() - low.ln()) / (high.ln() - low.ln())
            } else {
                (value - low) / (high - low)
            }
            .clamp(0.0, 1.0)
        };
        let at = |fraction: f64| -> f64 {
            let value = if logarithmic {
                (low.ln() + fraction * (high.ln() - low.ln())).exp()
            } else {
                low + fraction * (high - low)
            };
            let value = match self.step {
                Some(step) if step > 0.0 => low + ((value - low) / step).round() * step,
                _ if N::INTEGRAL => value.round(),
                _ => value,
            };
            value.clamp(low, high)
        };

        let before = self.value.to_f64();
        let mut value = before;
        let track = rect.shrink2(egui::vec2(knob, 0.0));
        if let Some(pointer) = response.interact_pointer_pos() {
            value =
                at(f64::from((pointer.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0));
        }
        if response.has_focus() {
            let (back, forward) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowLeft)
                        || input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::ArrowRight)
                        || input.key_pressed(egui::Key::ArrowUp),
                )
            });
            if back != forward {
                let nudge = match self.step {
                    Some(step) if step > 0.0 && !logarithmic => {
                        step / (high - low).max(f64::MIN_POSITIVE)
                    }
                    _ => 0.02,
                };
                value = at(along(value) + if forward { nudge } else { -nudge });
            }
        }
        if value != before {
            *self.value = N::from_f64(value);
            response.mark_changed();
        }
        let enabled = ui.is_enabled();
        response.widget_info(|| egui::WidgetInfo::slider(enabled, value, &self.label));
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
            response.hovered() || response.dragged() || response.has_focus(),
        );
        let p = &services.theme.palette;
        let thickness = (height * 0.16).round().max(3.0);
        let rail =
            egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width() - knob, thickness));
        let x = track.left() + along(value) as f32 * track.width();
        let painter = ui.painter();
        let rail_color = theme::distinct(p.border, p.panel_background, p.text);
        painter.rect_filled(rail, thickness / 2.0, theme::color(rail_color));
        let mut filled = rail;
        filled.max.x = x;
        painter.rect_filled(filled, thickness / 2.0, theme::color(p.accent));
        let center = egui::pos2(x, rect.center().y);
        if response.has_focus() {
            painter.circle_stroke(
                center,
                knob + 2.5,
                egui::Stroke::new(1.5, theme::color(p.accent)),
            );
        }
        let mut edge = p.panel_background;
        edge[3] = 1.0;
        painter.circle(
            center,
            knob + hover,
            theme::color(theme::mix(p.text, p.accent, 0.25 * hover)),
            egui::Stroke::new(1.0, theme::color(edge)),
        );
        response
    }
}
