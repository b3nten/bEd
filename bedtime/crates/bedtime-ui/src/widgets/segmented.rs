use super::Row;
use crate::{ambient, motion, theme, tokens::*};

/// One of a few options, all in view: a row of segments, the chosen one
/// filled. For views and modes; a `Select` is for longer lists.
///
/// ```ignore
/// ui.add(Segmented::new(&mut view, &View::ALL));
/// ```
///
/// When the segments don't fit in a row they wrap, as separate pills.
#[must_use = "a segmented control does nothing until it's added to a Ui"]
pub struct Segmented<'a, T> {
    value: &'a mut T,
    options: &'a [T],
    labels: Vec<String>,
    hover: Vec<String>,
    size: Size,
}

impl<'a, T: PartialEq + Clone> Segmented<'a, T> {
    /// The options, named by how they `Display`.
    pub fn new(value: &'a mut T, options: &'a [T]) -> Self
    where
        T: std::fmt::Display,
    {
        Self::with(value, options, T::to_string)
    }

    /// The options, named by `label`.
    pub fn with(value: &'a mut T, options: &'a [T], label: impl Fn(&T) -> String) -> Self {
        Self {
            value,
            options,
            labels: options.iter().map(label).collect(),
            hover: Vec::new(),
            size: Size::default(),
        }
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// What each option says when it's pointed at.
    pub fn hover(mut self, text: impl Fn(&T) -> String) -> Self {
        self.hover = self.options.iter().map(text).collect();
        self
    }
}

impl<T: PartialEq + Clone> egui::Widget for Segmented<'_, T> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let services = ambient::services(ui.ctx());
        let font = egui::FontId::proportional(self.size.font(ui));
        let padding = self.size.padding(ui);
        let height = self.size.height(ui);
        let galleys: Vec<_> = self
            .labels
            .iter()
            .map(|label| {
                ui.painter()
                    .layout_no_wrap(label.clone(), font.clone(), egui::Color32::PLACEHOLDER)
            })
            .collect();
        let widths: Vec<f32> = galleys
            .iter()
            .map(|galley| galley.size().x + 2.0 * padding.x)
            .collect();
        // The track is a control's height; the segments sit a little inside it.
        const INSET: f32 = 2.0;
        let total = widths.iter().sum::<f32>() + 2.0 * INSET;
        let fits = total <= ui.available_width();
        let enabled = ui.is_enabled();
        let radius = Radius::Md.px(ui);
        let p = services.theme.palette.clone();
        let surface = theme::distinct(p.surface, p.panel_background, p.text);
        let chosen = self.options.iter().position(|option| option == self.value);
        let mut clicked = None;

        let mut segment = |ui: &mut egui::Ui, index: usize, rect: egui::Rect, id: egui::Id| {
            let mut response = ui.interact(rect, id, egui::Sense::click());
            if let Some(text) = self.hover.get(index).filter(|text| !text.is_empty()) {
                response = response.on_hover_text(text);
            }
            let selected = chosen == Some(index);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::RadioButton,
                    enabled,
                    selected,
                    &self.labels[index],
                )
            });
            if response.clicked() {
                clicked = Some(index);
            }
            let animate = |name: &str, on: bool| {
                let target = if on { 1.0 } else { 0.0 };
                motion::scalar(ui.ctx(), id.with(name), target, target, services.motion)
            };
            let hover = animate("hover", response.hovered() || response.has_focus());
            let on = animate("on", selected);
            let mut fill = theme::mix(theme::mix(surface, p.accent, 0.2), p.selection, on);
            fill[3] *= hover.max(on);
            let painter = ui.painter();
            painter.rect_filled(rect, (radius - 1.0).max(0.0), theme::color(fill));
            if response.has_focus() {
                painter.rect_stroke(
                    rect,
                    (radius - 1.0).max(0.0),
                    egui::Stroke::new(1.5, theme::color(p.accent)),
                    egui::StrokeKind::Inside,
                );
            }
            let ink = theme::mix(p.muted, p.text, hover.max(on));
            let galley = galleys[index].clone();
            let at = rect.center() - galley.size() / 2.0;
            painter.galley(at, galley, theme::color(ink));
            response
        };

        let mut response = if fits {
            let (track, response) =
                ui.allocate_exact_size(egui::vec2(total, height), egui::Sense::hover());
            ui.painter().rect(
                track,
                radius,
                theme::color(theme::mix(p.panel_background, surface, 0.5)),
                egui::Stroke::new(1.0, theme::color(p.border)),
                egui::StrokeKind::Inside,
            );
            let mut left = track.left() + INSET;
            for (index, width) in widths.iter().enumerate() {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, track.top() + INSET),
                    egui::pos2(left + width, track.bottom() - INSET),
                );
                segment(ui, index, rect, response.id.with(index));
                left += width;
            }
            response
        } else {
            Row::new()
                .gap(Space::Xs)
                .wrap()
                .show(ui, |ui| {
                    for (index, width) in widths.iter().enumerate() {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(*width, height - 2.0 * INSET),
                            egui::Sense::hover(),
                        );
                        segment(ui, index, rect, response.id.with("segment"));
                    }
                })
                .response
        };
        if let Some(index) = clicked
            && chosen != Some(index)
        {
            *self.value = self.options[index].clone();
            response.mark_changed();
        }
        response
    }
}
