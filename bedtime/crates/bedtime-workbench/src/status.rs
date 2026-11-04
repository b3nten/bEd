//! The status bar (`ui` feature): a window body (`SurfaceBody(status::body)`)
//! drawing the panels with a bar along the bottom.
//!
//! The left side is the workbench's: what's going on (`StatusActivity`,
//! which any plugin can report to, with a spinner) and the latest
//! notification (for a few seconds; hover for recent ones). The right side is
//! the app's: the items it adds with `add_status_item`, the first added
//! rightmost. Plugins export items for it (workspaces' name, the code
//! plugin's language server) and the app chooses which to show.
use crate::*;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bevy::prelude::*;
use std::{collections::BTreeMap, time::Duration};

/// Room above and below the status bar's text.
const STATUS_PADDING: f32 = 5.0;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How many recent notifications the tooltip lists.
const RECENT: usize = 10;

/// An item on the right of the status bar: it draws itself into the bar's
/// row (right to left), reading the world.
pub type StatusItem = fn(&mut egui::Ui, &World, &UiContext<'_>);

/// The right side's items, the first added rightmost.
#[derive(Resource, Default)]
pub struct StatusItems(Vec<StatusItem>);

/// What's going on, by who's doing it ("rust-analyzer": "indexing"): shown
/// on the left of the status bar with a spinner, before the latest
/// notification. A plugin sets its entry while working and clears it after.
#[derive(Resource, Default, Debug)]
pub struct StatusActivity(BTreeMap<String, String>);

impl StatusActivity {
    /// Say what `source` is doing; `None` says it's done.
    pub fn set(&mut self, source: impl Into<String>, doing: Option<String>) {
        let source = source.into();
        match doing {
            Some(doing) => {
                if self.0.get(&source) != Some(&doing) {
                    self.0.insert(source, doing);
                }
            }
            None => {
                self.0.remove(&source);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(source, doing)| (source.as_str(), doing.as_str()))
    }
}

pub trait StatusAppExt {
    /// Show `item` on the right of the status bar, left of those added before.
    fn add_status_item(&mut self, item: StatusItem) -> &mut Self;
}

impl StatusAppExt for App {
    fn add_status_item(&mut self, item: StatusItem) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<StatusItems>()
            .0
            .push(item);
        self
    }
}

/// The window's body: the panels, with the status bar along the bottom.
pub fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let available = ui.available_rect_before_wrap();
    // A compact strip: a line of text, with the same room above and below.
    // The panels already leave their gap above the bar, so that counts
    // towards the room above the text.
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let above = (STATUS_PADDING - panel_gap(world, context)).max(0.0);
    let height = (line + above + STATUS_PADDING).min(available.height().max(0.0));
    let bar = egui::Rect::from_min_max(
        egui::pos2(available.left(), available.bottom() - height),
        available.max,
    );
    let panels = egui::Rect::from_min_max(available.min, egui::pos2(available.right(), bar.top()));
    let mut panels_ui = ui.new_child(egui::UiBuilder::new().id_salt("panels").max_rect(panels));
    view::show(&mut panels_ui, world, context);

    let background = bedtime_ui::theme::color(context.theme.palette.background);
    ui.painter().rect_filled(bar, 0.0, background);
    let content = egui::Rect::from_min_max(
        egui::pos2(bar.left() + 10.0, bar.top() + above),
        egui::pos2(bar.right() - 10.0, bar.bottom() - STATUS_PADDING),
    );
    if content.is_positive() {
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("status-content")
                .max_rect(content)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // Rows centre on `interact_size.y`: the content's height, so text
        // sits in its middle.
        row.spacing_mut().interact_size.y = content.height();
        row.spacing_mut().button_padding.y = 0.0;
        row.set_clip_rect(content);
        draw(&mut row, world, context);
    }
    ui.advance_cursor_after_rect(available);
}

/// The gap the panels leave between themselves and the bar: none when one
/// area fills the window edge to edge (as the layout view draws it).
fn panel_gap(world: &World, context: &UiContext<'_>) -> f32 {
    let Some(surface) = world.get::<LayoutSurface>(context.surface) else {
        return 0.0;
    };
    let single = world
        .layouts()
        .active()
        .and_then(|layout| world.get::<LayoutAreas>(layout))
        .is_some_and(|areas| areas.entities().len() == 1);
    if surface.single_area_edge_to_edge && single {
        0.0
    } else {
        surface.panel_gap
    }
}

fn draw(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    // The app's items on the right, then what's going on and the
    // notification in what's left.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if let Some(items) = world.get_resource::<StatusItems>() {
            for item in &items.0 {
                item(ui, world, context);
            }
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            activity(ui, world, context);
            notification(ui, world, context);
        });
    });
}

fn activity(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let Some(activity) = world.get_resource::<StatusActivity>() else {
        return;
    };
    for (source, doing) in activity.iter() {
        ui.spinner();
        ui.weak(format!("{source}: {doing}"));
        ui.add_space(context.metrics.spacing[0]);
    }
}

fn notification(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let palette = &context.theme.palette;
    let Some(notifications) = world.get_resource::<Notifications>() else {
        return;
    };
    let Some(latest) = notifications.latest() else {
        return;
    };
    let Some(left) = SHOWN_FOR.checked_sub(latest.at.elapsed()) else {
        return;
    };
    let tint = match latest.level {
        NotificationLevel::Info => palette.muted,
        NotificationLevel::Warning => palette.warning,
        NotificationLevel::Error => palette.error,
    };
    let recent: Vec<_> = notifications
        .iter()
        .rev()
        .take(RECENT)
        .map(|notification| notification.text.as_str())
        .collect();
    let mut frame = context.frame(ui);
    let side = context.metrics.font_size;
    let icon = frame.icon(
        BuiltinIcon::CircleAlert,
        side,
        bedtime_ui::theme::color(tint),
    );
    let text = frame.ui.add(egui::Label::new(&latest.text).truncate());
    frame.tooltip(&icon.union(text), recent.join("\n"));
    ui.ctx().request_repaint_after(left);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_set_and_cleared_by_source() {
        let mut activity = StatusActivity::default();
        activity.set("rust-analyzer", Some("indexing".into()));
        activity.set("git", Some("fetching".into()));
        activity.set("rust-analyzer", Some("checking".into()));
        assert_eq!(
            activity.iter().collect::<Vec<_>>(),
            [("git", "fetching"), ("rust-analyzer", "checking")]
        );
        activity.set("git", None);
        assert_eq!(activity.iter().count(), 1);
    }
}
