//! Language servers in the status bar: an item for the right side (the
//! focused editor's server, and its errors and warnings for the file), and
//! what servers are working on, reported as status activity on the left.
use super::{DocumentServer, LanguageServer, ServerStatus};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::status::StatusActivity;
use bevy::prelude::*;

/// A status bar item (`app.add_status_item`): the language server of the
/// focused editor (or the last one), whether it's running, and how many
/// errors and warnings it reports for the file.
pub fn status_item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let Some(editor) = world
        .get_resource::<crate::structure::StructureSource>()
        .and_then(|source| source.0)
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
    let program = server.name();
    let (text, tint, detail) = match server.status() {
        ServerStatus::Starting => (format!("{program} starting"), palette.muted, None),
        ServerStatus::Ready => (program.to_owned(), palette.muted, None),
        ServerStatus::Failed(error) => (
            format!("{program} failed"),
            palette.error,
            Some(format!("{error}\n{}", server.activity().stderr)),
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
    if server.status() == ServerStatus::Starting {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// What each server is working on, as status activity: its first progress
/// report, or "loading" while it loads the project (it answers nothing
/// then). Nothing for a server that's idle.
pub(super) fn report_activity(
    servers: Query<&LanguageServer>,
    activity: Option<ResMut<StatusActivity>>,
    mut reported: Local<Vec<String>>,
) {
    let Some(mut activity) = activity else {
        return;
    };
    let mut now = Vec::new();
    for server in &servers {
        if server.status() != ServerStatus::Ready {
            continue;
        }
        let state = server.activity();
        let doing = match state.progress.first() {
            Some(work) => Some(work.label()),
            None if state
                .project_status
                .as_ref()
                .is_some_and(|status| !status.quiescent) =>
            {
                Some("loading".to_owned())
            }
            None => None,
        };
        if let Some(doing) = doing {
            now.push(server.name().to_owned());
            activity.set(server.name(), Some(doing));
        }
    }
    for gone in reported.iter().filter(|name| !now.contains(name)) {
        activity.set(gone.clone(), None);
    }
    *reported = now;
}
