//! Cached repository status outlives the Git panel that displays its controls.
use crate::{GitState, REFRESH_COMMAND, SHOW_COMMAND};
use bed_git_core::RepositoryOperation;
use bed_workbench_api::{
    Action, StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

const QUIET: &str = "bed.git.repository-status";
const ACTIVITY: &str = "bed.git.operation-status";

pub(crate) fn install(app: &mut App) {
    app.world_mut().spawn(StatusItems(
        [(QUIET, "Repository", 80), (ACTIVITY, "Git activity", 150)]
            .map(|(id, label, priority)| StatusItem {
                id,
                label,
                priority,
                icon: "git-branch",
                prefer_icon: false,
                measure,
                draw,
                tooltip,
            })
            .to_vec(),
    ));
}

fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    presentation(world, context).map(|status| status.measure(ui))
}

fn draw(world: &mut World, ui: &Ui, context: StatusDrawContext, actions: &mut Vec<Action>) {
    if let Some(status) = presentation(world, context.context) {
        status.draw(ui, context, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(status) = presentation(world, context) {
        status.tooltip(ui);
    }
}

fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    let controller = &world.get_non_send::<GitState>()?.0;
    // Without a workspace or explicit Git request, no discovery is underway.
    if controller.root.is_empty() && controller.status.is_none() && controller.error.is_none() {
        return None;
    }
    let progress = controller.progress_label();
    let active = controller.error.is_some() || progress.is_some();
    if (context.item_id == ACTIVITY) != active {
        return None;
    }
    let status = controller
        .status
        .as_ref()
        .filter(|status| !status.root.is_empty());
    let mut details = Vec::new();
    if let Some(status) = status {
        details.push(format!(
            "Repository: {}\nBranch: {}",
            status.root, status.branch
        ));
        details.push(format!(
            "{}\n{} commits ahead, {} behind\n{} changed files, {} conflicts",
            status.upstream.as_deref().unwrap_or("No upstream"),
            status.ahead,
            status.behind,
            status.entries.len(),
            status
                .entries
                .iter()
                .filter(|entry| entry.conflicted)
                .count(),
        ));
        let operation = match status.operation {
            RepositoryOperation::None => None,
            RepositoryOperation::Merge => Some("Merge in progress"),
            RepositoryOperation::Rebase => Some("Rebase in progress"),
            RepositoryOperation::CherryPick => Some("Cherry-pick in progress"),
            RepositoryOperation::Revert => Some("Revert in progress"),
        };
        if let Some(operation) = operation {
            details.push(operation.into());
        }
    }
    if let Some(message) = progress {
        details.push(message.into());
    }
    if let Some(error) = &controller.error {
        details.push(error.clone());
    }
    let summary = if controller.error.is_some() {
        "Git error".into()
    } else if let Some(progress) = progress {
        progress.into()
    } else {
        let status = status?;
        let conflicts = status
            .entries
            .iter()
            .filter(|entry| entry.conflicted)
            .count();
        let mut summary = status.branch.clone();
        if status.ahead > 0 || status.behind > 0 {
            summary.push_str(&format!("  ↑{} ↓{}", status.ahead, status.behind));
        }
        if !status.entries.is_empty() {
            summary.push_str(&format!("  {} changed", status.entries.len()));
        }
        if conflicts > 0 {
            summary.push_str(&format!("  {conflicts} conflicts"));
        }
        summary
    };
    let mut text = StatusText {
        error: controller.error.is_some(),
        ..StatusText::new(summary, details.join("\n\n"))
            .command("Open Git", SHOW_COMMAND)
            .command("Refresh", REFRESH_COMMAND)
    };
    text.commands[1].enabled = !controller.busy();
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_git_core::{Status, StatusEntry};
    use serde_json::Value;

    fn context(publisher: Entity, item_id: &'static str) -> StatusContext {
        StatusContext {
            publisher,
            item_id,
            focused_panel: None,
            focused_frontend: None,
            source: None,
            area: 9,
        }
    }

    #[test]
    fn cached_git_state_contributes_without_a_panel_or_additional_reads() {
        let mut app = App::new();
        app.add_plugins(crate::GitPlugin);
        let publisher = app
            .world_mut()
            .query::<(Entity, &StatusItems)>()
            .single(app.world())
            .unwrap()
            .0;
        assert!(presentation(app.world(), context(publisher, QUIET)).is_none());
        assert!(presentation(app.world(), context(publisher, ACTIVITY)).is_none());
        {
            let mut controller = app.world_mut().non_send_mut::<GitState>();
            controller.0.root = "/workspace/repo".into();
            controller.0.status = Some(Status {
                root: "/workspace/repo".into(),
                branch: "main".into(),
                upstream: Some("origin/main".into()),
                ahead: 2,
                behind: 1,
                entries: vec![StatusEntry {
                    path: "a.rs".into(),
                    old_path: None,
                    index_status: 'U',
                    worktree_status: 'U',
                    conflicted: true,
                }],
                ..Default::default()
            });
        }
        let text = presentation(app.world(), context(publisher, QUIET)).unwrap();
        assert_eq!(text.summary, "main  ↑2 ↓1  1 changed  1 conflicts");
        assert!(text.details.contains("origin/main"));
        assert!(presentation(app.world(), context(publisher, ACTIVITY)).is_none());
        assert!(
            !app.world().non_send::<GitState>().0.busy(),
            "status presentation must not start a worker"
        );
        app.world_mut().non_send_mut::<GitState>().0.error = Some("Push rejected".into());
        assert!(presentation(app.world(), context(publisher, QUIET)).is_none());
        let error = presentation(app.world(), context(publisher, ACTIVITY)).unwrap();
        assert!(error.error);
        assert!(error.details.contains("Push rejected"));
        assert_eq!(error.commands[0].command, SHOW_COMMAND);
        let Action::Invoke(command) =
            context(publisher, ACTIVITY).invoke(error.commands[0].command, Value::Null)
        else {
            unreachable!()
        };
        assert_eq!(command.source, None);
        assert_eq!(command.area, 9);
    }
}
