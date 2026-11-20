//! The repository in the status bar's global status: the branch, how far
//! it is from its upstream, the changes, conflicts, a merge or rebase under
//! way, and the operation running.
use super::GitSession;
use crate::{RepositoryOperation, Status};
use bedtime_ui::BuiltinIcon;
use bedtime_workbench::status::{GlobalStatus, Tone};
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

/// A global status item (`app.add_global_status`): the branch, with how
/// far it is from its upstream, the changes, conflicts, a merge or rebase
/// under way, and the operation running on hover. While Git's panels are open (the
/// session follows them).
pub fn global_item(world: &World, status: &mut GlobalStatus) {
    let Some(session) = world.get_resource::<GitSession>() else {
        return;
    };
    let view = session
        .view
        .as_ref()
        .filter(|view| !view.status.branch.is_empty());
    let progress = session.progress();
    if view.is_none() && progress.is_none() {
        return;
    }
    status.icon(BuiltinIcon::GitBranch);
    status.heading("Git");
    if let Some(view) = view {
        let summary = summary(&view.status, view.staged_count);
        status.text(summary.branch.clone());
        status.row("Branch", summary.branch);
        status.row("Changes", summary.changes.unwrap_or_else(|| "none".into()));
        if let Some(conflicts) = summary.conflicts {
            status.row("Conflicts", conflicts).tone(Tone::Error);
        }
        if let Some(operation) = summary.operation {
            status.row("In progress", operation).tone(Tone::Warning);
        }
    }
    if let Some(progress) = progress {
        status.row("Running", progress).busy();
        status.refresh_after(Duration::from_millis(250));
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
