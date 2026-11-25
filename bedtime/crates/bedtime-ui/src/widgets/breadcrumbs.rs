use crate::{ambient, motion, theme, tokens::*};

/// Where something is: the names down to it, each one to click.
///
/// ```ignore
/// if let Some(index) = Breadcrumbs::new(["bed", "src", "view"]).show(ui) {
///     go_to(index);
/// }
/// ```
///
/// When they don't fit in the width there is, the first ones go, and an
/// ellipsis stands for them. The last is where you are, in the text colour;
/// the others are muted until they're pointed at.
#[must_use = "breadcrumbs do nothing until they're shown"]
pub struct Breadcrumbs {
    names: Vec<String>,
    size: Size,
}

const SEPARATOR: &str = " › ";

impl Breadcrumbs {
    pub fn new<N: Into<String>>(names: impl IntoIterator<Item = N>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
            size: Size::Sm,
        }
    }

    /// The size of their text (`Size::Sm`, unless set).
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Draw them; the index (into the names given) of one that was clicked.
    pub fn show(self, ui: &mut egui::Ui) -> Option<usize> {
        if self.names.is_empty() {
            return None;
        }
        let services = ambient::services(ui.ctx());
        let p = &services.theme.palette;
        let font = egui::FontId::proportional(self.size.font(ui));
        let layout = |ui: &egui::Ui, text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
        };
        let separator = layout(ui, SEPARATOR);
        let galleys: Vec<_> = self.names.iter().map(|name| layout(ui, name)).collect();
        // What doesn't fit goes, from the first on; the last always stays.
        let room = ui.available_width();
        let mut needed: f32 = galleys.iter().map(|galley| galley.size().x).sum::<f32>()
            + separator.size().x * (galleys.len() - 1) as f32;
        let mut first = 0;
        let ellipsis = layout(ui, "…");
        while needed > room && first + 1 < galleys.len() {
            needed -= galleys[first].size().x + separator.size().x;
            if first == 0 {
                needed += ellipsis.size().x + separator.size().x;
            }
            first += 1;
        }
        let height = (font.size * theme::LINE_HEIGHT).ceil() + 2.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(needed.min(room.max(0.0)), height),
            egui::Sense::hover(),
        );
        let muted = theme::color(p.muted);
        let painter = ui.painter_at(rect);
        let mut x = rect.left();
        // Paint `galley` at `x`, and say where the next thing starts.
        let place = |x: f32, galley: std::sync::Arc<egui::Galley>, ink: egui::Color32| {
            let width = galley.size().x;
            let at = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
            painter.galley(at, galley, ink);
            x + width
        };
        if first > 0 {
            x = place(x, ellipsis, muted);
            x = place(x, separator.clone(), muted);
        }
        let mut clicked = None;
        let last = galleys.len() - 1;
        let enabled = ui.is_enabled();
        for (index, galley) in galleys.into_iter().enumerate().skip(first) {
            let at = egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(galley.size().x, rect.height()),
            );
            let response = ui.interact(at, ui.id().with(("crumb", index)), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &self.names[index])
            });
            if response.clicked() {
                clicked = Some(index);
            }
            let target = if response.hovered() || response.has_focus() || index == last {
                1.0
            } else {
                0.0
            };
            let lit = motion::scalar(
                ui.ctx(),
                response.id.with("lit"),
                target,
                target,
                services.motion,
            );
            x = place(x, galley, theme::color(theme::mix(p.muted, p.text, lit)));
            if index < last {
                x = place(x, separator.clone(), muted);
            }
        }
        clicked
    }
}
