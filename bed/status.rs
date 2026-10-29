//! The status bar along the bottom of the window: the latest notification for
//! a few seconds, with the recent ones in its tooltip, and the workspace's name.
use crate::workspace::Workspace;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::{Notification, NotificationLevel, Notifications, WorkbenchWorldExt};
use bevy::prelude::*;
use std::time::Duration;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How many recent notifications the tooltip lists.
const RECENT: usize = 10;

pub struct StatusBarPlugin;

impl Plugin for StatusBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, |world: &mut World| {
            world.workbench_mut().register_status_bar(draw);
        });
    }
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
