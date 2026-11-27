//! What a chat panel's drawing and actions ask for (`AcpIntent`), and doing
//! it, in `AcpSystems`: the panel's own (attachments, links) and the
//! thread's (prompts, permissions, options).
use crate::{
    attachments::{PromptAttachment, open_texts},
    connection::{AcpConnection, reconnect, send},
    panel::{AcpPanel, AcpRuntime},
    thread::{AcpThread, QueuedPrompt, SessionState, dispatch_prompt, new_thread},
    worker::{ApprovalMode, WorkerCommand},
};
use agent_client_protocol::v1::schema::{
    SessionConfigId, SessionConfigKind, SessionConfigOptionValue, SessionModeId,
};
use bedtime_workbench::{ActionResult, BedtimeWorldExt as _, Clipboard, PathOptions, Placement};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

/// What the panel's drawing asks for, done in `AcpSystems`.
pub(crate) enum AcpIntent {
    NewChat,
    Reconnect,
    SubmitPrompt {
        interrupt: bool,
    },
    CancelPrompt,
    SendQueuedNow(usize),
    RemoveQueued(usize),
    SetConfigOption(SessionConfigId, SessionConfigOptionValue),
    SetMode(SessionModeId),
    RespondPermission {
        request_id: u64,
        option_id: Option<String>,
    },
    SetApprovalMode(ApprovalMode),
    PasteClipboardImage,
    AttachPaths(Vec<PathBuf>),
    OpenLink(String),
    OpenFile {
        path: PathBuf,
        line: Option<u32>,
    },
}

/// Do what the chat panels' drawing (and their actions) asked for.
pub(crate) fn apply_intents(world: &mut World, cwd: &Path) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<AcpRuntime>>()
        .iter(world)
        .collect();
    for panel in panels {
        let Some(mut rt) = world.get_mut::<AcpRuntime>(panel) else {
            continue;
        };
        let target = rt.thread;
        for intent in std::mem::take(&mut rt.intents) {
            apply_intent(world, panel, target, cwd, intent);
        }
    }
}

fn apply_intent(
    world: &mut World,
    panel: Entity,
    target: Option<Entity>,
    cwd: &Path,
    intent: AcpIntent,
) {
    // What's the panel's own.
    match intent {
        AcpIntent::NewChat => {
            let client = world
                .get::<AcpPanel>(panel)
                .map(|acp| acp.client_name.clone())
                .unwrap_or_default();
            let result = new_thread(world, &client, Placement::Focused);
            world
                .bedtime_mut()
                .notify_failure("Couldn't start a chat", result);
            return;
        }
        AcpIntent::PasteClipboardImage => {
            let image = world
                .get_resource_mut::<Clipboard>()
                .map(|mut clipboard| clipboard.read_image_if_present());
            let Some(mut rt) = world.get_mut::<AcpRuntime>(panel) else {
                return;
            };
            match image {
                Some(Ok(Some(image))) => match PromptAttachment::from_clipboard_image(&image) {
                    Ok(attachment) => {
                        rt.attachments.push(attachment);
                        rt.attachment_error = None;
                    }
                    Err(err) => rt.attachment_error = Some(err),
                },
                Some(Err(err)) => {
                    rt.attachment_error = Some(format!("Couldn't read clipboard image: {err}"));
                }
                Some(Ok(None)) | None => {}
            }
            return;
        }
        AcpIntent::AttachPaths(paths) => {
            let Some(mut rt) = world.get_mut::<AcpRuntime>(panel) else {
                return;
            };
            for path in paths {
                match PromptAttachment::from_path(path, cwd) {
                    Ok(attachment) => {
                        if !rt.attachments.contains(&attachment) {
                            rt.attachments.push(attachment);
                        }
                        rt.attachment_error = None;
                    }
                    Err(err) => rt.attachment_error = Some(err),
                }
            }
            return;
        }
        AcpIntent::OpenLink(link) => {
            let result = if link.starts_with("http://")
                || link.starts_with("https://")
                || link.starts_with("mailto:")
            {
                world.bedtime_mut().open_externally_from(&link, panel)
            } else {
                let path = PathBuf::from(link.strip_prefix("file://").unwrap_or(&link));
                open_file(world, panel, cwd, path, None)
            };
            world
                .bedtime_mut()
                .notify_failure("Couldn't open link", result);
            return;
        }
        AcpIntent::OpenFile { path, line } => {
            let result = open_file(world, panel, cwd, path, line);
            world
                .bedtime_mut()
                .notify_failure("Couldn't open file", result);
            return;
        }
        _ => {}
    }

    // The rest is the thread's.
    let Some(target) = target else {
        return;
    };
    let Some(connection) = world.get::<AcpThread>(target).map(|t| t.connection) else {
        return;
    };
    let caps = world
        .get::<AcpConnection>(connection)
        .map(|c| c.capabilities)
        .unwrap_or_default();
    if matches!(intent, AcpIntent::Reconnect) {
        reconnect(world, connection);
        return;
    }
    // Typed into the panel, and sent to the thread.
    let prompt = match &intent {
        AcpIntent::SubmitPrompt { .. } => {
            world
                .get_mut::<AcpRuntime>(panel)
                .map(|mut rt| QueuedPrompt {
                    text: std::mem::take(&mut rt.input).trim().to_owned(),
                    attachments: std::mem::take(&mut rt.attachments),
                })
        }
        _ => None,
    };
    let open = prompt
        .as_ref()
        .map(|prompt| open_texts(world, &prompt.attachments))
        .unwrap_or_default();
    let Some(mut thread) = world.get_mut::<AcpThread>(target) else {
        return;
    };
    let session_id = thread.session_id.clone();
    let mut commands: Vec<WorkerCommand> = Vec::new();
    match intent {
        AcpIntent::SubmitPrompt { interrupt } => {
            let Some(queued) = prompt.filter(|p| !p.text.is_empty() || !p.attachments.is_empty())
            else {
                return;
            };
            if thread.streaming {
                if interrupt {
                    thread.queued_prompts.push_front(queued);
                    commands.extend(
                        session_id.map(|session_id| WorkerCommand::CancelSession { session_id }),
                    );
                } else {
                    thread.queued_prompts.push_back(queued);
                }
            } else if let Some(session_id) =
                session_id.filter(|_| thread.session == SessionState::Ready)
            {
                match dispatch_prompt(&mut thread, queued, caps, &open) {
                    Ok(blocks) => commands.push(WorkerCommand::SendPrompt { session_id, blocks }),
                    Err(err) => thread.error = Some(err),
                }
            } else {
                // Sent once its session is there.
                thread.queued_prompts.push_back(queued);
            }
            thread.scroll_to_bottom = true;
        }
        AcpIntent::CancelPrompt => {
            commands
                .extend(session_id.map(|session_id| WorkerCommand::CancelSession { session_id }));
        }
        AcpIntent::RemoveQueued(idx) => {
            if idx < thread.queued_prompts.len() {
                thread.queued_prompts.remove(idx);
            }
        }
        AcpIntent::SendQueuedNow(idx) => {
            if idx < thread.queued_prompts.len() {
                if let Some(item) = thread.queued_prompts.remove(idx) {
                    thread.queued_prompts.push_front(item);
                }
                commands.extend(
                    session_id.map(|session_id| WorkerCommand::CancelSession { session_id }),
                );
            }
        }
        AcpIntent::SetConfigOption(config_id, value) => {
            let Some(session_id) = session_id else {
                return;
            };
            if let Some(opt) = thread.config_options.iter_mut().find(|o| o.id == config_id) {
                match (&mut opt.kind, &value) {
                    (
                        SessionConfigKind::Select(sel),
                        SessionConfigOptionValue::ValueId { value: vid },
                    ) => {
                        sel.current_value = vid.clone();
                    }
                    (
                        SessionConfigKind::Boolean(b),
                        SessionConfigOptionValue::Boolean { value: bv },
                    ) => {
                        b.current_value = *bv;
                    }
                    _ => {}
                }
            }
            commands.push(WorkerCommand::SetConfigOption {
                session_id,
                config_id,
                value,
            });
        }
        AcpIntent::SetMode(mode_id) => {
            let Some(session_id) = session_id else {
                return;
            };
            if let Some(modes) = &mut thread.modes {
                modes.current_mode_id = mode_id.clone();
            }
            commands.push(WorkerCommand::SetMode {
                session_id,
                mode_id,
            });
        }
        AcpIntent::RespondPermission {
            request_id,
            option_id,
        } => {
            thread
                .pending_permissions
                .retain(|perm| perm.request_id != request_id);
            commands.push(WorkerCommand::RespondPermission {
                request_id,
                option_id,
            });
        }
        AcpIntent::SetApprovalMode(mode) => {
            thread.approval_mode = mode;
            if mode == ApprovalMode::AutoApprove {
                // What it was waiting to be asked about.
                for permission in std::mem::take(&mut thread.pending_permissions) {
                    match permission.allow_option() {
                        Some(option_id) => commands.push(WorkerCommand::RespondPermission {
                            request_id: permission.request_id,
                            option_id: Some(option_id),
                        }),
                        None => thread.pending_permissions.push(permission),
                    }
                }
            }
            if let Some(session_id) = &session_id
                && let Some(worker) = world
                    .get::<AcpConnection>(connection)
                    .and_then(|c| c.worker.as_ref())
            {
                worker.set_approval_mode(session_id, mode);
            }
        }
        AcpIntent::NewChat
        | AcpIntent::Reconnect
        | AcpIntent::PasteClipboardImage
        | AcpIntent::AttachPaths(_)
        | AcpIntent::OpenLink(_)
        | AcpIntent::OpenFile { .. } => {}
    }
    for command in commands {
        send(world, connection, command);
    }
}

/// Open `path` (in the workspace, if relative) beside `panel`.
fn open_file(
    world: &mut World,
    panel: Entity,
    cwd: &Path,
    path: PathBuf,
    line: Option<u32>,
) -> ActionResult {
    let resolved = if path.is_relative() {
        cwd.join(path)
    } else {
        path
    };
    let options = PathOptions {
        line: line.map(|l| (l.saturating_sub(1), 0)),
        place: Some(Placement::From(panel)),
        ..Default::default()
    };
    world
        .bedtime_mut()
        .open_path(&resolved, options)
        .map(|_| ())
}
