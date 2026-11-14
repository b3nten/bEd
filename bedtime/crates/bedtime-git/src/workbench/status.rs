//! The Git panel in the status bar (`add_panel_status::<GitPanel>`): the
//! branch, how far it is from its upstream, the changes, conflicts, a merge
//! or rebase under way, and the operation running.
use super::GitSession;
use crate::{RepositoryOperation, Status};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::status::StatusPieces;
use bevy::prelude::*;
use std::time::Duration;

/// What the item shows, as text.
#[derive(Debug, Default, PartialEq, Eq)]
struct Summary {
    /// "main ↑2 ↓1", or the short commit when detached.
    branch: String,
    /// "5 changed (2 staged)".
    changes: Option<String>,
    /// "1 conflict".
    conflicts: Option<String>,
    /// "Merging".
    operation: Option<&'static str>,
}

fn summary(status: &Status, staged: usize) -> Summary {
    let mut branch = match (&status.head, status.branch.as_str()) {
        (Some(head), "Detached HEAD") => head.chars().take(7).collect(),
        (_, branch) => branch.to_owned(),
    };
    if status.upstream.is_some() {
        if status.ahead > 0 {
            branch.push_str(&format!(" ↑{}", status.ahead));
        }
        if status.behind > 0 {
            branch.push_str(&format!(" ↓{}", status.behind));
        }
    }
    let changed = status.entries.len();
    let changes = (changed > 0).then(|| match staged {
        0 => format!("{changed} changed"),
        staged => format!("{changed} changed ({staged} staged)"),
    });
    let conflicted = status.entries.iter().filter(|e| e.conflicted).count();
    let conflicts = match conflicted {
        0 => None,
        1 => Some("1 conflict".to_owned()),
        n => Some(format!("{n} conflicts")),
    };
    let operation = match status.operation {
        RepositoryOperation::None => None,
        RepositoryOperation::Merge => Some("Merging"),
        RepositoryOperation::Rebase => Some("Rebasing"),
        RepositoryOperation::CherryPick => Some("Cherry-picking"),
        RepositoryOperation::Revert => Some("Reverting"),
    };
    Summary {
        branch,
        changes,
        conflicts,
        operation,
    }
}

/// The status bar item for the Git panel.
pub fn item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, _panel: Entity) {
    let Some(session) = world.get_resource::<GitSession>() else {
        return;
    };
    let palette = &context.theme.palette;
    let mut pieces = StatusPieces::new(ui, context);
    // Right to left: changes, branch, conflicts, operation, progress.
    if let Some(view) = &session.view
        && !view.status.branch.is_empty()
    {
        let summary = summary(&view.status, view.staged_count);
        if let Some(changes) = summary.changes {
            pieces.weak(changes);
        }
        pieces.weak(summary.branch);
        if let Some(conflicts) = summary.conflicts {
            pieces.colored(bedtime_ui::theme::color(palette.error), conflicts);
        }
        if let Some(operation) = summary.operation {
            pieces.colored(bedtime_ui::theme::color(palette.warning), operation);
        }
    }
    if let Some(progress) = session.progress() {
        pieces.weak(progress);
        pieces
            .ui()
            .ctx()
            .request_repaint_after(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StatusEntry;

    fn entry(path: &str, conflicted: bool) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            old_path: None,
            index_status: 'M',
            worktree_status: '.',
            conflicted,
        }
    }

    #[test]
    fn a_clean_branch_shows_only_its_name() {
        let status = Status {
            branch: "main".into(),
            ..Default::default()
        };
        assert_eq!(
            summary(&status, 0),
            Summary {
                branch: "main".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn ahead_behind_changes_and_conflicts() {
        let status = Status {
            branch: "main".into(),
            upstream: Some("origin/main".into()),
            ahead: 2,
            behind: 1,
            entries: vec![entry("a", false), entry("b", true), entry("c", true)],
            operation: RepositoryOperation::Merge,
            ..Default::default()
        };
        let summary = summary(&status, 1);
        assert_eq!(summary.branch, "main ↑2 ↓1");
        assert_eq!(summary.changes.as_deref(), Some("3 changed (1 staged)"));
        assert_eq!(summary.conflicts.as_deref(), Some("2 conflicts"));
        assert_eq!(summary.operation, Some("Merging"));
    }

    #[test]
    fn only_nonzero_counts_and_only_with_an_upstream() {
        let mut status = Status {
            branch: "topic".into(),
            ahead: 3,
            entries: vec![entry("a", false)],
            ..Default::default()
        };
        let summary_without = summary(&status, 0);
        assert_eq!(summary_without.branch, "topic", "no upstream: no counts");
        assert_eq!(summary_without.changes.as_deref(), Some("1 changed"));
        status.upstream = Some("origin/topic".into());
        assert_eq!(summary(&status, 0).branch, "topic ↑3");
    }

    #[test]
    fn a_detached_head_shows_its_short_commit() {
        let status = Status {
            branch: "Detached HEAD".into(),
            head: Some("0123456789abcdef".into()),
            ..Default::default()
        };
        assert_eq!(summary(&status, 0).branch, "0123456");
    }
}
