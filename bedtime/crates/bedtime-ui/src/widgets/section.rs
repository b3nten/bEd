use crate::{Theme, ambient, theme, tokens::*};

/// A titled group that opens and closes.
///
/// ```ignore
/// Section::new("Edges").show(ui, |ui| {
///     ui.add(Checkbox::new(&mut ordering, "Ordering"));
/// });
/// ```
#[must_use = "a section does nothing until it's shown"]
pub struct Section {
    title: String,
    id: Option<egui::Id>,
    open: bool,
}

impl Section {
    /// Closed until it's opened.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            id: None,
            open: false,
        }
    }

    /// Open until it's closed.
    pub fn open(mut self) -> Self {
        self.open = true;
        self
    }

    /// What it's known by between frames, when its title changes (a count
    /// in it, say).
    pub fn id(mut self, id: impl egui::AsId) -> Self {
        self.id = Some(egui::Id::new(id));
        self
    }

    /// The header, and under it `content` while it's open. `None` when closed.
    pub fn show<R>(self, ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
        let services = ambient::services(ui.ctx());
        let id = ui.make_persistent_id(self.id.unwrap_or_else(|| egui::Id::new(&self.title)));
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            self.open,
        );
        let font_size = Size::Md.font(ui);
        let height = Size::Md.height(ui);
        let galley = ui.painter().layout_no_wrap(
            self.title.clone(),
            egui::FontId::proportional(font_size),
            egui::Color32::PLACEHOLDER,
        );
        let chevron = font_size;
        let gap = (font_size * 0.3).round();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(
                (chevron + gap + galley.size().x).max(ui.available_width()),
                height,
            ),
            egui::Sense::click(),
        );
        if response.clicked() {
            state.toggle(ui);
        }
        let (enabled, open) = (ui.is_enabled(), state.is_open());
        response.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, &self.title);
            info.selected = Some(open);
            info
        });
        let openness = state.openness(ui.ctx());
        if ui.is_rect_visible(rect) {
            let p = &services.theme.palette;
            let lit = response.hovered() || response.has_focus();
            let ink = if lit {
                p.text
            } else {
                theme::mix(p.muted, p.text, 0.6)
            };
            crate::paint_chevron(
                ui.painter(),
                egui::pos2(rect.left() + chevron / 2.0, rect.center().y),
                openness,
                theme::color(p.muted),
            );
            let at = egui::pos2(
                rect.left() + chevron + gap,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(at, galley, theme::color(ink));
        }
        let indent = chevron + gap;
        let body = state.show_body_unindented(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: indent as i8,
                    ..Default::default()
                })
                .show(ui, content)
                .inner
        });
        body.map(|body| body.inner)
    }
}

/// A line between groups: across in a column, upright in a row.
///
/// ```ignore
/// ui.add(Separator::new());
/// ```
#[must_use = "a separator does nothing until it's added to a Ui"]
#[derive(Clone, Copy, Default)]
pub struct Separator {
    space: Space,
}

impl Separator {
    /// `Space::Sm` of room for it.
    pub fn new() -> Self {
        Self::default()
    }
    /// How much room it takes along the layout (the line is in the middle).
    pub fn space(mut self, space: Space) -> Self {
        self.space = space;
        self
    }
}

impl egui::Widget for Separator {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let border = theme::color(Theme::of(ui).palette.border);
        let space = self.space.px(ui).max(1.0);
        let across = !ui.layout().main_dir().is_horizontal();
        // While what it's in is being measured (a menu), it takes no length
        // of its own: it's as long as the rest makes room for.
        let measuring = ui.is_sizing_pass();
        let size = match (across, measuring) {
            (true, false) => egui::vec2(ui.available_width(), space),
            (true, true) => egui::vec2(0.0, space),
            (false, false) => egui::vec2(space, ui.available_size_before_wrap().y),
            (false, true) => egui::vec2(space, 0.0),
        };
        let (rect, response) = ui.allocate_at_least(size, egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            let stroke = egui::Stroke::new(1.0, border);
            let painter = ui.painter();
            if across {
                painter.hline(rect.x_range(), rect.center().y, stroke);
            } else {
                painter.vline(rect.center().x, rect.y_range(), stroke);
            }
        }
        response
    }
}
