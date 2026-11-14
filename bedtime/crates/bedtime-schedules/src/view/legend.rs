//! Legend overlay for the frame-wide views: what the symbols and colours mean.
//!
//! The entries are painted with the same helpers the views use, so a symbol here is exactly the
//! symbol on the canvas.

use bedtime_ui::egui::{
    self, Color32, CornerRadius, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke,
    epaint::QuadraticBezierShape, pos2, vec2,
};

use super::{Mode, frame_view::mark_shape, style};
use crate::frame::MarkKind;

const WIDTH: f32 = 330.0;
const ICON: egui::Vec2 = egui::vec2(26.0, 18.0);
/// Roughly the height of the longest entry list; the window opens this tall and scrolls beyond it.
const CONTENT_H: f32 = 620.0;

/// A stand-in for "the colour this component happens to get".
fn sample(alpha: u8) -> Color32 {
    style::hue_color("Transform", 0.7, 1.0, alpha)
}

/// Draws the legend for `mode` as a window over the canvas, pinned to the top-right of `area`
/// until the user drags it. `show` is the viewer's `show_legend` flag, which the window's close
/// button clears.
pub(super) fn overlay(ui: &mut egui::Ui, mode: Mode, area: Rect, show: &mut bool) {
    let ctx = ui.ctx().clone();
    let id = ui.id().with("schedules_legend");
    if !*show {
        egui::Area::new(id.with("button"))
            .order(egui::Order::Middle)
            .movable(false)
            .fixed_pos(pos2(area.max.x - 76.0, area.min.y + 8.0))
            .constrain_to(area)
            .show(&ctx, |ui| {
                if ui
                    .small_button("legend")
                    .on_hover_text("what the symbols and colours mean")
                    .clicked()
                {
                    *show = true;
                }
            });
        return;
    }
    egui::Window::new("Legend")
        .id(id)
        .order(egui::Order::Middle)
        .open(show)
        .default_pos(pos2(area.max.x - WIDTH - 40.0, area.min.y + 8.0))
        .default_width(WIDTH)
        .max_width(WIDTH)
        .default_height((area.height() - 48.0).min(CONTENT_H))
        .resizable([false, true])
        .collapsible(true)
        .vscroll(true)
        .constrain_to(area)
        .show(&ctx, |ui| match mode {
            Mode::Arcs => arcs(ui),
            _ => lanes(ui),
        });
}

// ------------------------------------------------------------------------------------------------
// Entries
// ------------------------------------------------------------------------------------------------

fn lanes(ui: &mut egui::Ui) {
    heading(ui, "Marks — one access, at a system column");
    row(ui, "write", |p, c| {
        mark_shape(p, c, MarkKind::Write, sample(255), false)
    });
    row(ui, "read", |p, c| {
        mark_shape(p, c, MarkKind::Read, style::read(), false)
    });
    row(ui, "write from a run condition", |p, c| {
        mark_shape(p, c, MarkKind::ConditionWrite, sample(255), false)
    });
    row(ui, "read from a run condition", |p, c| {
        mark_shape(p, c, MarkKind::ConditionRead, style::read(), false)
    });
    row(ui, "used only as a With/Without filter", |p, c| {
        mark_shape(p, c, MarkKind::Filter, style::filter(), false)
    });
    row(ui, "archetypal access (Has<T>)", |p, c| {
        mark_shape(p, c, MarkKind::Archetypal, style::filter(), false)
    });
    note(
        ui,
        "A write is drawn in the lane's own colour, a hash of the type name — it groups a \
         component's marks and arcs, it doesn't rank them.",
    );

    heading(ui, "Lines");
    row(
        ui,
        "hand-off: this write feeds those later reads",
        |p, c| {
            let color = sample(255);
            p.line_segment(
                [pos2(c.x - 10.0, c.y), pos2(c.x + 10.0, c.y)],
                Stroke::new(2.2, color),
            );
            arrow_head(p, pos2(c.x + 8.0, c.y), vec2(1.0, 0.0), color, 4.0);
        },
    );
    row(ui, "schedule boundary", |p, c| {
        p.line_segment(
            [pos2(c.x, c.y - 7.0), pos2(c.x, c.y + 7.0)],
            Stroke::new(1.0, style::rule()),
        );
    });

    heading(ui, "Lane labels");
    text_row(ui, "Time ⚙", style::read(), "resource");
    text_row(ui, "Transform", style::node_text(), "component");

    focus_entries(ui, "click a label to focus that lane, a column for details");
    header_entries(ui);
}

fn arcs(ui: &mut egui::Ui) {
    heading(ui, "Above the axis — ordering");
    row(ui, "runs before (arrow at the later system)", |p, c| {
        mini_arc(p, c, true, style::filter().gamma_multiply(0.75), 1.2);
    });
    row(ui, "runs before the focused system", |p, c| {
        mini_arc(p, c, true, style::edge_in(), 2.0)
    });
    row(ui, "runs after it", |p, c| {
        mini_arc(p, c, true, style::edge_out(), 2.0)
    });

    heading(ui, "Below the axis — data flow");
    row(ui, "a write feeding a later read", |p, c| {
        mini_arc(p, c, false, sample(255), 2.0)
    });
    note(
        ui,
        "Hand-off arcs take the component's colour, a hash of the type name. Faint arcs are the \
         rest of the frame; focusing a system or a component picks out its own.",
    );

    heading(ui, "On the axis — one dot per system");
    row(ui, "writes something", |p, c| {
        p.circle_filled(c, 4.0, style::write());
    });
    row(ui, "read-only", |p, c| {
        p.circle_filled(c, 4.0, style::read());
    });
    row(ui, "exclusive system (&mut World)", |p, c| {
        p.circle_filled(c, 4.0, style::write());
        p.circle_stroke(c, 6.0, Stroke::new(1.0, style::exclusive()));
    });
    row(ui, "dimmed: does not match the search", |p, c| {
        p.circle_filled(c, 4.0, style::write().gamma_multiply(0.4));
    });
    row(ui, "schedule boundary", |p, c| {
        p.line_segment(
            [pos2(c.x, c.y - 7.0), pos2(c.x, c.y + 7.0)],
            Stroke::new(1.0, style::rule()),
        );
    });

    focus_entries(ui, "click a column for details");
    header_entries(ui);
}

/// Highlight colours, shared by both views.
fn focus_entries(ui: &mut egui::Ui, hint: &str) {
    heading(ui, "Highlight");
    row(ui, "hovered or selected column", |p, c| {
        p.rect_filled(
            Rect::from_center_size(c, vec2(9.0, 15.0)),
            CornerRadius::ZERO,
            style::selected().gamma_multiply(0.35),
        );
    });
    text_row(
        ui,
        "Abc",
        style::selected(),
        "its system name in the header",
    );
    note(ui, hint);
}

/// The column header, shared by both views.
fn header_entries(ui: &mut egui::Ui) {
    heading(ui, "Column header");
    row(ui, "a schedule of the frame", |p, c| {
        p.rect_filled(
            Rect::from_center_size(c, vec2(20.0, 10.0)),
            CornerRadius::same(3),
            style::hue_color("Update", 0.45, 0.6, 140),
        );
    });
    text_row(ui, "⏸", style::node_text(), "sync point (ApplyDeferred)");
    row(ui, "wave boundary", |p, c| {
        p.line_segment(
            [pos2(c.x, c.y - 6.0), pos2(c.x, c.y + 6.0)],
            Stroke::new(1.0, style::rule()),
        );
    });
    note(
        ui,
        "Columns are the frame's systems in execution order. Systems between two wave ticks are \
         one topological level: they may run in parallel — their names alternate between two \
         greys wave by wave.",
    );
}

// ------------------------------------------------------------------------------------------------
// Building blocks
// ------------------------------------------------------------------------------------------------

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().color(style::filter()));
}

fn row(ui: &mut egui::Ui, text: &str, draw: impl FnOnce(&Painter, Pos2)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(ICON, Sense::hover());
        if ui.is_rect_visible(rect) {
            draw(ui.painter(), rect.center());
        }
        ui.add(egui::Label::new(text).wrap());
    });
}

/// A row whose "symbol" is itself a piece of coloured text.
fn text_row(ui: &mut egui::Ui, symbol: &str, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.set_min_width(ICON.x);
            ui.colored_label(color, symbol);
        });
        ui.add(egui::Label::new(text).wrap());
    });
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(style::node_text_dim())).wrap());
}

fn arrow_head(p: &Painter, tip: Pos2, dir: egui::Vec2, color: Color32, size: f32) {
    let normal = vec2(-dir.y, dir.x);
    let base = tip - dir * size;
    p.add(Shape::convex_polygon(
        vec![tip, base + normal * size * 0.8, base - normal * size * 0.8],
        color,
        Stroke::NONE,
    ));
}

/// A small stand-in for the view's arcs: same shape, sized for a legend row.
fn mini_arc(p: &Painter, c: Pos2, up: bool, color: Color32, width: f32) {
    let (x0, x1) = (c.x - 10.0, c.x + 10.0);
    let y = if up { c.y + 6.0 } else { c.y - 6.0 };
    let ctrl = pos2(c.x, if up { y - 17.0 } else { y + 17.0 });
    p.add(QuadraticBezierShape::from_points_stroke(
        [pos2(x0, y), ctrl, pos2(x1, y)],
        false,
        Color32::TRANSPARENT,
        Stroke::new(width, color),
    ));
    let dir = (pos2(x1, y) - ctrl).normalized();
    arrow_head(p, pos2(x1, y), dir, color, 4.0 + width);
    p.line_segment(
        [pos2(x0 - 2.0, y), pos2(x1 + 2.0, y)],
        Stroke::new(1.0, style::rule()),
    );
}
