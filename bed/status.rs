//! The status bar along the bottom of the window: the latest notification for
//! a few seconds, with the recent ones in its tooltip; the language server of
//! the editor you're in, and its problems; and the workspace's name.
//! It's part of the window's body (`body`), under the panels.
use crate::workspace::Workspace;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::{Notification, NotificationLevel, Notifications, WorkbenchWorldExt, view};
use bevy::prelude::*;
use std::time::Duration;

/// Room above and below the status bar's text.
const STATUS_PADDING: f32 = 5.0;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How many recent notifications the tooltip lists.
const RECENT: usize = 10;

/// The window's body: the panels, with the status bar along the bottom.
pub fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let available = ui.available_rect_before_wrap();
    // A compact strip: a line of text, with the same room above and below.
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let height = (line + 2.0 * STATUS_PADDING).min(available.height().max(0.0));
    let bar = egui::Rect::from_min_max(
        egui::pos2(available.left(), available.bottom() - height),
        available.max,
    );
    let panels = egui::Rect::from_min_max(available.min, egui::pos2(available.right(), bar.top()));
    let mut panels_ui = ui.new_child(egui::UiBuilder::new().id_salt("panels").max_rect(panels));
    view::show(&mut panels_ui, world, context);

    let background = bedtime_ui::theme::color(context.theme.palette.background);
    ui.painter().rect_filled(bar, 0.0, background);
    let content = bar.shrink2(egui::vec2(10.0, 0.0));
    if content.is_positive() {
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("status-content")
                .max_rect(content)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // Rows centre on `interact_size.y`: the bar's height, so text sits
        // in its middle.
        row.spacing_mut().interact_size.y = content.height();
        row.spacing_mut().button_padding.y = 0.0;
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
        language_server(ui, world, context);
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

/// The language server of the focused editor (or the last one): what it's
/// doing, and how many errors and warnings it reports for the file.
fn language_server(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    use bedtime_code::lsp::{DocumentServer, LanguageServer, ServerStatus};
    let Some(editor) = world
        .resource::<bedtime_code::structure::StructureSource>()
        .0
    else {
        return;
    };
    let Some(document) = world
        .get::<bedtime_workbench::DocumentReference>(editor)
        .map(|reference| reference.document().entity())
    else {
        return;
    };
    let Some(connection) = world.get::<DocumentServer>(document) else {
        return;
    };
    let Some(server) = world.get::<LanguageServer>(connection.server) else {
        return;
    };
    let palette = &context.theme.palette;
    let activity = server.activity();
    let program = server.name();
    let (text, tint, detail) = match server.status() {
        ServerStatus::Starting => (format!("{program} starting"), palette.muted, None),
        ServerStatus::Ready => match activity.progress.first() {
            Some(work) => (format!("{program}: {}", work.label()), palette.muted, None),
            // It answers nothing while loading the project.
            None if activity
                .project_status
                .as_ref()
                .is_some_and(|status| !status.quiescent) =>
            {
                (format!("{program}: loading"), palette.muted, None)
            }
            None => (program.to_owned(), palette.muted, None),
        },
        ServerStatus::Failed(error) => (
            format!("{program} failed"),
            palette.error,
            Some(format!("{error}\n{}", activity.stderr)),
        ),
        ServerStatus::Stopped => (format!("{program} stopped"), palette.muted, None),
    };
    if let Some(report) = &connection.diagnostics {
        let [errors, warnings, ..] = report.counts();
        let count = |n: usize, what: &str| match n {
            0 => None,
            1 => Some(format!("1 {what}")),
            n => Some(format!("{n} {what}s")),
        };
        let counts: Vec<String> = [count(errors, "error"), count(warnings, "warning")]
            .into_iter()
            .flatten()
            .collect();
        if !counts.is_empty() {
            let tint = if errors > 0 {
                palette.error
            } else {
                palette.warning
            };
            ui.colored_label(bedtime_ui::theme::color(tint), counts.join(", "));
            ui.add_space(context.metrics.spacing[0]);
        }
    }
    let label = ui.colored_label(bedtime_ui::theme::color(tint), text);
    if let Some(detail) = detail.or_else(|| connection.error.clone()) {
        label.on_hover_text(detail);
    }
    ui.add_space(context.metrics.spacing[0]);
    // The server's progress doesn't wake the UI by itself.
    let loading = activity
        .project_status
        .as_ref()
        .is_some_and(|status| !status.quiescent);
    if server.status() == ServerStatus::Starting || !activity.progress.is_empty() || loading {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
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
