//! Language servers in the status bar: each server in the global status
//! (whether it's running, and the errors and warnings it reports for open
//! files), the focused editor's file's errors and warnings among its
//! status, and what servers are working on, reported as status activity.
use super::{DocumentServer, LanguageServer, ServerStatus};
use crate::editor::EditorPanel;
use bedtime_workbench::status::{GlobalStatus, PanelStatus, StatusActivity, Tone};
use bevy::prelude::*;
use std::time::Duration;

/// A global status item (`app.add_global_status`): an icon, coloured by the
/// worst a server reports, with each server on hover: whether it's running,
/// and the errors and warnings it reports for the files open.
pub fn global_item(world: &World, status: &mut GlobalStatus) {
    let mut servers: Vec<(Entity, &LanguageServer)> = world
        .try_query::<(Entity, &LanguageServer)>()
        .map(|mut query| query.iter(world).collect())
        .unwrap_or_default();
    if servers.is_empty() {
        return;
    }
    servers.sort_by(|a, b| a.1.name().cmp(b.1.name()));
    // Errors and warnings per server, over its open documents.
    let mut counts = std::collections::HashMap::<Entity, [usize; 2]>::new();
    if let Some(mut documents) = world.try_query::<&DocumentServer>() {
        for connection in documents.iter(world) {
            if let Some(report) = &connection.diagnostics {
                let [errors, warnings, ..] = report.counts();
                let count = counts.entry(connection.server).or_default();
                count[0] += errors;
                count[1] += warnings;
            }
        }
    }
    status.icon(bedtime_ui::BuiltinIcon::Diagnostics);
    status.heading("Language servers");
    for (entity, server) in servers {
        let [errors, warnings] = counts.get(&entity).copied().unwrap_or_default();
        let problems = problems(errors, warnings);
        let name = server.name();
        match server.status() {
            ServerStatus::Starting => {
                status.row(name, "starting").tone(Tone::Weak).busy();
                status.refresh_after(Duration::from_millis(250));
            }
            ServerStatus::Ready => {
                let (value, tone) = match problems {
                    Some((text, tone)) => (format!("ready, {text} in open files"), tone),
                    None => ("ready".to_owned(), Tone::Normal),
                };
                status.row(name, value).tone(tone);
            }
            ServerStatus::Failed(error) => {
                status
                    .row(name, format!("failed: {error}"))
                    .tone(Tone::Error);
            }
            ServerStatus::Stopped => {
                status.row(name, "stopped").tone(Tone::Weak);
            }
        }
    }
}

/// A panel status item for editors (`add_panel_status::<EditorPanel>`): the
/// errors and warnings its server reports for the file, and a problem with
/// the file's connection.
pub fn file_item(world: &World, panel: Entity, status: &mut PanelStatus) {
    if world.get::<EditorPanel>(panel).is_none() {
        return;
    }
    let Some(connection) = world
        .get::<bedtime_workbench::DocumentReference>(panel)
        .and_then(|reference| world.get::<DocumentServer>(reference.document().entity()))
    else {
        return;
    };
    if let Some(report) = &connection.diagnostics {
        let [errors, warnings, ..] = report.counts();
        if let Some((text, tone)) = problems(errors, warnings) {
            status.add(tone, text);
        }
    }
    if let Some(error) = &connection.error {
        let server = world
            .get::<LanguageServer>(connection.server)
            .map_or("language server", |server| server.name());
        status
            .warning(format!("{server} error"))
            .tooltip(error.clone());
    }
}

/// "2 errors, 1 warning", in the error colour if there are errors; nothing
/// for none.
fn problems(errors: usize, warnings: usize) -> Option<(String, Tone)> {
    let count = |n: usize, what: &str| match n {
        0 => None,
        1 => Some(format!("1 {what}")),
        n => Some(format!("{n} {what}s")),
    };
    let counts: Vec<String> = [count(errors, "error"), count(warnings, "warning")]
        .into_iter()
        .flatten()
        .collect();
    if counts.is_empty() {
        return None;
    }
    let tone = if errors > 0 {
        Tone::Error
    } else {
        Tone::Warning
    };
    Some((counts.join(", "), tone))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_count_errors_and_warnings() {
        assert_eq!(problems(0, 0), None);
        assert_eq!(problems(0, 1), Some(("1 warning".into(), Tone::Warning)));
        assert_eq!(
            problems(2, 3),
            Some(("2 errors, 3 warnings".into(), Tone::Error))
        );
    }
}
