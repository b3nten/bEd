//! Tree rows shared by Files, Git and Structure. Callers own rows, identity,
//! viewport culling and actions; this module never reads a filesystem.
use crate::{BuiltinIcon, MotionPolicy, UiContext, egui};

pub const INSET: f32 = 8.0;
pub const INDENT: f32 = 16.0;

pub fn row_height(context: &UiContext<'_>) -> f32 {
    (context.metrics.font_size + 10.0).max(24.0)
}

/// A row's artwork. `label: None` reserves the name for an inline editor.
pub struct Row<'a> {
    pub label: Option<&'a str>,
    pub depth: usize,
    /// Drawn before the name; `None` puts the name there (an outline).
    pub icon: Option<BuiltinIcon>,
    pub expanded: Option<bool>,
    pub selected: bool,
    pub ink: egui::Color32,
    pub icon_ink: crate::theme::Rgba,
}

/// Paint inside an existing hit rectangle; selection fits the icon and name.
pub fn paint_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    row: Row<'_>,
    rect: egui::Rect,
    response: Option<&egui::Response>,
    alpha: f32,
    indent: f32,
) {
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(alpha.clamp(0.0, 1.0));
    let x = rect.left() + INSET + row.depth as f32 * indent;
    let side = context.metrics.font_size.min(18.0);
    // Where the name starts: after the icon, if there's one.
    let name_x = x + 18.0 + row.icon.map_or(0.0, |_| side + 5.0);
    if let Some(open) = row.expanded {
        // The caller's stable row ID also scopes its disclosure animation.
        let id = response.map_or_else(|| ui.id(), |response| response.id);
        let progress = crate::animate_expansion(ui.ctx(), id.with("arrow"), open, context.motion);
        crate::paint_chevron(
            &painter,
            egui::pos2(x + 7.0, rect.center().y),
            progress,
            row.ink,
        );
    }
    if let Some(label) = row.label {
        let ink = if row.selected {
            crate::rainbow_ink(ui, context.motion)
        } else {
            row.ink
        };
        let font = egui::FontId::proportional(context.metrics.font_size);
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, ink);
        job.wrap.max_width = (rect.right() - x - 42.0).max(0.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        if let Some(response) = response {
            let visuals = ui.style().interact_selectable(response, row.selected);
            let fill = ui.ctx().animate_color_with_time(
                response.id.with("row-fill"),
                visuals.bg_fill,
                if context.motion.enabled {
                    context.motion.seconds
                } else {
                    0.0
                },
            );
            if row.selected || response.hovered() {
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(x + 14.0, rect.top() + 1.0),
                    egui::pos2(name_x + galley.size().x + 4.0, rect.bottom() - 1.0),
                );
                painter.rect_filled(
                    label_rect.intersect(rect.shrink(1.0)),
                    visuals.corner_radius,
                    fill,
                );
            }
        }
        painter.galley(
            egui::pos2(name_x, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
    }
    let Some(icon) = row.icon else {
        return;
    };
    // A painter must not advance the caller's cursor: Git places a staging
    // checkbox after this rectangle. `scope` would add another item spacing.
    let mut icons_ui = ui.new_child(egui::UiBuilder::new().id_salt("tree-icon").max_rect(rect));
    icons_ui.multiply_opacity(alpha);
    let mut theme = context.theme.clone();
    theme.palette.text = row.icon_ink;
    let mut frame = context.frame(&mut icons_ui);
    frame.theme = &theme;
    frame.icon_at(icon, [x + 18.0, rect.center().y - side / 2.0], side);
}

/// The Files tree's timing (`ease`), also used for disappearing descendants.
pub fn animation_fraction(now: f64, started: f64, policy: MotionPolicy) -> f32 {
    if !policy.enabled {
        return 1.0;
    }
    crate::ease((now - started) as f32 / policy.seconds.max(0.001))
}

/// One visible row's movement/fade. Retargeting preserves its displayed values.
#[derive(Clone)]
pub struct RowMotion {
    from: f32,
    target: f32,
    alpha_from: f32,
    alpha_target: f32,
    started: f64,
}
impl RowMotion {
    pub fn new(from: f32, target: f32, alpha: f32, now: f64) -> Self {
        Self {
            from,
            target,
            alpha_from: alpha,
            alpha_target: 1.0,
            started: now,
        }
    }
    pub fn sample(&self, now: f64, policy: MotionPolicy) -> (f32, f32) {
        let t = animation_fraction(now, self.started, policy);
        (
            self.from + (self.target - self.from) * t,
            self.alpha_from + (self.alpha_target - self.alpha_from) * t,
        )
    }
    pub fn retarget(&mut self, target: f32, alpha: f32, now: f64, policy: MotionPolicy) {
        if self.target != target || self.alpha_target != alpha {
            (self.from, self.alpha_from) = self.sample(now, policy);
            self.target = target;
            self.alpha_target = alpha;
            self.started = now;
        }
    }
    pub fn shift(&mut self, delta: f32) {
        self.from += delta;
        self.target += delta;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_highlights_fit_the_name_and_fading_respects_parent_opacity() {
        fn draw(opacity: f32, alpha: f32) -> (f32, u8) {
            let ctx = egui::Context::default();
            let icons = std::sync::Mutex::new(crate::icon_cache::IconCache::default());
            let theme = crate::Theme::dark();
            let context = UiContext {
                view: bevy::prelude::Entity::PLACEHOLDER,
                surface: bevy::prelude::Entity::PLACEHOLDER,
                theme: &theme,
                metrics: &crate::Metrics::default(),
                motion: MotionPolicy {
                    enabled: false,
                    ..Default::default()
                },
                icons: &icons,
            };
            let rect = egui::Rect::from_min_size(
                egui::pos2(10.0, 10.0),
                egui::vec2(600.0, row_height(&context)),
            );
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_opacity(opacity);
                let response = ui.interact(rect, egui::Id::new("row"), egui::Sense::click());
                let cursor = ui.cursor();
                paint_row(
                    ui,
                    &context,
                    Row {
                        label: Some("file.txt"),
                        depth: 0,
                        icon: Some(BuiltinIcon::Text),
                        expanded: None,
                        selected: true,
                        ink: egui::Color32::WHITE,
                        icon_ink: theme.palette.text,
                    },
                    rect,
                    Some(&response),
                    alpha,
                    INDENT,
                );
                assert_eq!(
                    ui.cursor(),
                    cursor,
                    "painting must not move adjacent controls"
                );
            });
            output.textures_delta.clear();
            let highlight = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(shape) if shape.rect.top() == rect.top() + 1.0 => {
                        Some(shape.rect.width())
                    }
                    _ => None,
                })
                .expect("selected row highlight");
            let max_alpha = ctx
                .tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .filter_map(|primitive| match &primitive.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => {
                        mesh.vertices.iter().map(|vertex| vertex.color.a()).max()
                    }
                    _ => None,
                })
                .max()
                .unwrap();
            (highlight, max_alpha)
        }
        let (highlight, opaque) = draw(1.0, 1.0);
        let (faded_highlight, faded) = draw(0.5, 0.5);
        assert!(
            highlight > 40.0 && highlight < 200.0,
            "highlight must fit the icon/name, not the 600-point row"
        );
        assert_eq!(highlight, faded_highlight);
        assert!((faded as f32 - opaque as f32 * 0.25).abs() <= 1.0);
    }

    #[test]
    fn collapse_and_reopen_preserve_position_and_opacity_then_settle() {
        let policy = MotionPolicy::default();
        let mut row = RowMotion::new(120.0, 120.0, 1.0, 0.0);
        row.retarget(24.0, 0.0, 1.0, policy);
        let halfway = row.sample(1.08, policy);
        let eased = crate::ease(0.5);
        assert!((halfway.0 - (120.0 - 96.0 * eased)).abs() < 0.001);
        assert!((halfway.1 - (1.0 - eased)).abs() < 0.001);
        row.retarget(144.0, 1.0, 1.08, policy);
        assert_eq!(row.sample(1.08, policy), halfway);
        row.shift(48.0);
        let shifted = row.sample(1.08, policy);
        assert!((shifted.0 - halfway.0 - 48.0).abs() < 0.001);
        assert_eq!(shifted.1, halfway.1);
        assert_eq!(row.sample(2.0, policy), (192.0, 1.0));
    }

    #[test]
    fn disabling_motion_settles_both_entering_and_retiring_rows() {
        let policy = MotionPolicy {
            enabled: false,
            ..Default::default()
        };
        let mut row = RowMotion::new(29.0, 24.0, 0.0, 1.0);
        assert_eq!(row.sample(1.0, policy), (24.0, 1.0));
        row.retarget(0.0, 0.0, 1.0, policy);
        assert_eq!(row.sample(1.0, policy), (0.0, 0.0));
    }
}
