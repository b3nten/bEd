use crate::{IconSource, ambient, motion, theme, tokens::*};

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
///
/// A name can have an icon before it (`.icon`), and `show_with` hands each
/// one's `Response` over as it's drawn, to hang a menu on.
#[must_use = "breadcrumbs do nothing until they're shown"]
pub struct Breadcrumbs {
    names: Vec<String>,
    icons: Vec<(usize, IconSource, egui::Color32)>,
    plain: Vec<usize>,
    size: Size,
}

const SEPARATOR: &str = " › ";

impl Breadcrumbs {
    pub fn new<N: Into<String>>(names: impl IntoIterator<Item = N>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
            icons: Vec::new(),
            plain: Vec::new(),
            size: Size::Sm,
        }
    }

    /// These names instead (when they're gathered after the icons are).
    pub fn names<N: Into<String>>(mut self, names: impl IntoIterator<Item = N>) -> Self {
        self.names = names.into_iter().map(Into::into).collect();
        self
    }
    /// The size of their text (`Size::Sm`, unless set).
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// The name at `index` is only said: it can't be clicked, and it's a
    /// label to scripts and screen readers (what all this is in, say).
    pub fn plain(mut self, index: usize) -> Self {
        self.plain.push(index);
        self
    }
    /// An icon before the name at `index`, in `tint`.
    pub fn icon(mut self, index: usize, icon: impl Into<IconSource>, tint: egui::Color32) -> Self {
        self.icons.push((index, icon.into(), tint));
        self
    }

    /// Draw them; the index (into the names given) of one that was clicked.
    pub fn show(self, ui: &mut egui::Ui) -> Option<usize> {
        self.show_with(ui, |_, _, _| {})
    }

    /// Draw them, and hand each one's index and `Response` to `each` as
    /// it's drawn (for a menu of what else is there, say).
    pub fn show_with(
        self,
        ui: &mut egui::Ui,
        mut each: impl FnMut(&mut egui::Ui, usize, &egui::Response),
    ) -> Option<usize> {
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
        // An icon is as tall as the text, with a gap after it.
        let side = font.size;
        let gap = (font.size * 0.3).round();
        let icon_of = |index: usize| self.icons.iter().find(|(at, ..)| *at == index);
        let widths: Vec<f32> = galleys
            .iter()
            .enumerate()
            .map(|(index, galley)| galley.size().x + icon_of(index).map_or(0.0, |_| side + gap))
            .collect();
        // What doesn't fit goes, from the first on; the last always stays.
        let room = ui.available_width();
        let mut needed: f32 =
            widths.iter().sum::<f32>() + separator.size().x * (galleys.len() - 1) as f32;
        let mut first = 0;
        let ellipsis = layout(ui, "…");
        while needed > room && first + 1 < galleys.len() {
            needed -= widths[first] + separator.size().x;
            if first == 0 {
                needed += ellipsis.size().x + separator.size().x;
            }
            first += 1;
        }
        let height = (font.size * theme::LINE_HEIGHT).ceil() + 2.0;
        let (rect, strip) = ui.allocate_exact_size(
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
                egui::vec2(widths[index], rect.height()),
            );
            let plain = self.plain.contains(&index);
            let (sense, kind) = if plain {
                (egui::Sense::hover(), egui::WidgetType::Label)
            } else {
                (egui::Sense::click(), egui::WidgetType::Button)
            };
            // Named from their own strip: two trails in one place don't clash.
            let response = ui.interact(at, strip.id.with(("crumb", index)), sense);
            response.widget_info(|| egui::WidgetInfo::labeled(kind, enabled, &self.names[index]));
            if response.clicked() {
                clicked = Some(index);
            }
            let pointed = !plain && (response.hovered() || response.has_focus());
            let target = if pointed || index == last { 1.0 } else { 0.0 };
            let lit = motion::scalar(
                ui.ctx(),
                response.id.with("lit"),
                target,
                target,
                services.motion,
            );
            if let Some((_, icon, tint)) = icon_of(index) {
                let pixels = (side * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
                let texture = services.icons.lock().unwrap().texture(icon, pixels);
                if let Some(texture) = texture {
                    painter.image(
                        texture,
                        egui::Rect::from_min_size(
                            egui::pos2(x, rect.center().y - side / 2.0),
                            egui::Vec2::splat(side),
                        ),
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        *tint,
                    );
                }
                x += side + gap;
            }
            x = place(x, galley, theme::color(theme::mix(p.muted, p.text, lit)));
            each(ui, index, &response);
            if index < last {
                x = place(x, separator.clone(), muted);
            }
        }
        clicked
    }
}
