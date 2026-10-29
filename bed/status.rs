//! The status bar along the bottom of the window: the latest notification for
//! a few seconds, with the recent ones in its tooltip, and the workspace's name.
//! It's part of the window's body (`body`), under the panels.
use crate::workspace::Workspace;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::{Notification, NotificationLevel, Notifications, WorkbenchWorldExt, view};
use bevy::prelude::*;
use std::time::Duration;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How many recent notifications the tooltip lists.
const RECENT: usize = 10;

/// The window's body: the panels, with the status bar along the bottom.
pub fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let available = ui.available_rect_before_wrap();
    // A compact strip, independent of roomy panel buttons.
    let button_padding_y = ui.spacing().button_padding.y.min(1.0);
    let height = (ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.text_style_height(&egui::TextStyle::Button) + 2.0 * button_padding_y)
        .max(ui.spacing().interact_size.y)
        + 4.0)
        .min(available.height().max(0.0));
    let bar = egui::Rect::from_min_max(
        egui::pos2(available.left(), available.bottom() - height),
        available.max,
    );
    let panels = egui::Rect::from_min_max(available.min, egui::pos2(available.right(), bar.top()));
    let mut panels_ui = ui.new_child(egui::UiBuilder::new().id_salt("panels").max_rect(panels));
    view::show(&mut panels_ui, world, context);

    let background = bedtime_ui::theme::color(context.theme.palette.background);
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().rect_filled(bar, 0.0, background);
    ui.painter()
        .line_segment([bar.left_top(), bar.right_top()], stroke);
    let content = bar.shrink2(egui::vec2(10.0, 2.0));
    if content.is_positive() {
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("status-content")
                .max_rect(content)
                .layout(egui::Layout::left_to_right(egui::Align::Center).with_cross_justify(true)),
        );
        row.spacing_mut().button_padding.y = button_padding_y;
        row.set_clip_rect(content);
        draw(&mut row, world, context);
    }
    ui.advance_cursor_after_rect(available);
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    // The workspace's name on the right, then the notification in what's left.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if let Some(workspace) = world.get_resource::<Workspace>() {
            ui.weak(workspace.name());
            ui.add_space(context.metrics.spacing[0]);
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            notification(ui, world, context);
        });
    });
}

fn notification(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let palette = &context.theme.palette;
    let notifications = world.resource::<Notifications>();
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

/// Show `result`'s error in the status bar, prefixed with what was being done.
pub fn notify_failure<T>(
    world: &mut World,
    doing: &str,
    result: Result<T, impl std::fmt::Display>,
) {
    if let Err(error) = result {
        world
            .workbench_mut()
            .notify(Notification::error(format!("{doing}: {error}")));
    }
}
