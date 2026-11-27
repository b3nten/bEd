//! Chat panel status bar item (`add_panel_status::<AcpPanel>`): shows the
//! connection's state, the thread's model, approval mode (when auto-approve),
//! context usage, and streaming / pending permission state.
use crate::{
    connection::{AcpConnection, ConnectionStatus},
    panel::{AcpRuntime, format_usage},
    thread::AcpThread,
    worker::ApprovalMode,
};
use bedtime_workbench::status::{PanelStatus, Tone};
use bevy::prelude::*;

/// The status bar item for a focused `AcpPanel`.
pub fn item(world: &World, panel: Entity, status: &mut PanelStatus) {
    let Some(thread) = world
        .get::<AcpRuntime>(panel)
        .and_then(|runtime| runtime.thread)
        .and_then(|thread| world.get::<AcpThread>(thread))
    else {
        return;
    };
    let Some(connection) = world.get::<AcpConnection>(thread.connection) else {
        return;
    };

    for (tone, text, tooltip) in summarize(thread, connection) {
        let piece = status.add(tone, text);
        if let Some(tip) = tooltip {
            piece.tooltip(tip);
        }
    }
}

fn summarize(
    thread: &AcpThread,
    connection: &AcpConnection,
) -> Vec<(Tone, String, Option<String>)> {
    let mut pieces = Vec::new();

    match &connection.status {
        ConnectionStatus::Connecting => {
            pieces.push((Tone::Weak, "connecting…".to_owned(), None));
        }
        ConnectionStatus::Error(err) => {
            pieces.push((Tone::Error, "disconnected".to_owned(), Some(err.clone())));
        }
        ConnectionStatus::Connected => {
            if let Some(model) = thread.model_name() {
                pieces.push((Tone::Weak, model, None));
            } else if let Some(info) = &connection.agent_info {
                pieces.push((Tone::Weak, info.clone(), None));
            }
        }
        ConnectionStatus::Idle => {}
    }

    if thread.approval_mode == ApprovalMode::AutoApprove {
        pieces.push((
            Tone::Warning,
            "auto-approve".to_owned(),
            Some("Tool calls are automatically approved".to_owned()),
        ));
    }

    if let Some(usage) = &thread.usage {
        pieces.push((Tone::Weak, format_usage(usage), None));
    }

    let pending_perms = thread.pending_permissions.len();
    if pending_perms > 0 {
        let label = if pending_perms == 1 {
            "permission needed".to_owned()
        } else {
            format!("{pending_perms} permissions needed")
        };
        pieces.push((Tone::Warning, label, None));
    } else if thread.streaming {
        pieces.push((Tone::Normal, "working…".to_owned(), None));
    }

    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_and_connecting_show_in_status() {
        let connection = AcpConnection::connecting_for_test("Claude");
        let mut thread = AcpThread::new(Entity::PLACEHOLDER, "Chat".into());
        thread.approval_mode = ApprovalMode::AutoApprove;

        let pieces = summarize(&thread, &connection);
        let texts: Vec<&str> = pieces.iter().map(|(_, t, _)| t.as_str()).collect();
        assert_eq!(texts, ["connecting…", "auto-approve"]);
        assert_eq!(pieces[1].0, Tone::Warning);
    }
}
