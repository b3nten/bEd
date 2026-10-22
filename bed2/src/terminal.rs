//! Application-owned shell commands and optional terminal observations.
use bedtime_terminal2::{
    ShellCommandsAppExt, ShellInvocation, ShellReply, TerminalPanel, TerminalSystems,
};
use bedtime_workbench2::{InWorkspace, Workbench, WorkspaceTask, WorkspaceWork};
use bevy::prelude::*;
use std::{ffi::OsString, path::PathBuf, sync::mpsc};

pub fn install(app: &mut App) {
    app.init_resource::<ShellOpens>()
        .add_shell_command(["+open", "+o"], open_from_shell)
        .expect("Application shell aliases")
        .add_systems(
            Update,
            poll_opens
                .after(TerminalSystems::Update)
                .after(crate::files::FilesSystems::Update)
                .before(crate::close::CloseSystems::Update),
        );
    #[cfg(feature = "automation")]
    {
        use bedtime_automation2::AutomationAppExt;
        app.register_automation_command("app.inspect_terminal", inspect);
    }
}

enum OpenTargets {
    Folder(PathBuf),
    Files(Vec<PathBuf>),
}

enum OpenStage {
    Resolve {
        layout: Entity,
        task: WorkspaceTask<Result<OpenTargets, String>>,
    },
    Folder(crate::opening::FolderOpenResult),
    Files(Vec<crate::opening::OpenedFile>),
}

struct ShellOpen {
    panel: Entity,
    workspace: Entity,
    stage: OpenStage,
    reply: ShellReply,
}

#[derive(Resource, Default)]
struct ShellOpens(Vec<ShellOpen>);

fn resolve_targets(directory: PathBuf, arguments: Vec<OsString>) -> Result<OpenTargets, String> {
    let mut files = Vec::with_capacity(arguments.len());
    for argument in &arguments {
        if argument.is_empty() {
            return Err("File path must not be empty".into());
        }
        let path = directory.join(argument);
        let path = path
            .canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let metadata = path
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if metadata.is_dir() {
            return if arguments.len() == 1 {
                Ok(OpenTargets::Folder(path))
            } else {
                Err("Open a directory on its own, or supply only file paths".into())
            };
        }
        files.push(path);
    }
    Ok(OpenTargets::Files(files))
}

fn open_from_shell(world: &mut World, invocation: ShellInvocation) {
    let ShellInvocation {
        arguments,
        directory,
        panel,
        workspace,
        reply,
        ..
    } = invocation;
    let result = (|| {
        if !crate::close::accepts_user_actions(world) {
            return Err("Cancel quit before opening files or a workspace".into());
        }
        if arguments.is_empty() {
            return crate::opening::open_folder(world, directory).map(OpenStage::Folder);
        }
        if !crate::storage::ready(world, workspace) {
            return Err(
                "The originating workspace is still loading or has a settings error".into(),
            );
        }
        let layout = Workbench::for_entity(world, panel)?.panel_layout(panel)?;
        let task = world
            .get_mut::<WorkspaceWork>(workspace)
            .ok_or("Workspace work is unavailable")?
            .spawn_task(Some(panel), "Resolve shell paths", move |cancel| {
                if cancel.is_cancelled() {
                    return Err("File opening cancelled".into());
                }
                resolve_targets(directory, arguments)
            })?;
        Ok(OpenStage::Resolve { layout, task })
    })();
    match result {
        Ok(stage) => world.resource_mut::<ShellOpens>().0.push(ShellOpen {
            panel,
            workspace,
            stage,
            reply,
        }),
        Err(error) => reply.complete(Err(error)),
    }
}

fn poll_opens(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<ShellOpens>().0);
    for mut request in pending {
        let result = (|| -> Result<bool, String> {
            if !crate::close::accepts_user_actions(world) {
                return Err("File opening cancelled while quitting".into());
            }
            if world.get::<TerminalPanel>(request.panel).is_none()
                || world
                    .get::<InWorkspace>(request.panel)
                    .is_none_or(|owner| owner.0 != request.workspace)
                || world
                    .get::<WorkspaceWork>(request.workspace)
                    .is_none_or(WorkspaceWork::is_closing)
            {
                return Err("The originating terminal or workspace has closed".into());
            }
            match &mut request.stage {
                OpenStage::Resolve { layout, task } => {
                    let targets = match task.try_recv() {
                        Ok(result) => result?,
                        Err(mpsc::TryRecvError::Empty) => return Ok(false),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err("Shell path validation was cancelled".into());
                        }
                    };
                    request.stage = match targets {
                        OpenTargets::Folder(root) => {
                            OpenStage::Folder(crate::opening::open_folder(world, root)?)
                        }
                        OpenTargets::Files(paths) => {
                            if world
                                .get::<InWorkspace>(*layout)
                                .is_none_or(|owner| owner.0 != request.workspace)
                            {
                                return Err("The originating layout has closed".into());
                            }
                            let files = paths
                                .into_iter()
                                .map(|path| {
                                    crate::opening::open_file(
                                        world,
                                        *layout,
                                        path,
                                        crate::opening::FileOpenMode::Focus,
                                    )
                                })
                                .collect::<Result<_, _>>()?;
                            OpenStage::Files(files)
                        }
                    };
                    Ok(false)
                }
                OpenStage::Folder(result) => {
                    match result.get_mut().expect("Folder result lock").try_recv() {
                        Ok(result) => result.map(|_| true),
                        Err(mpsc::TryRecvError::Empty) => Ok(false),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            Err("Workspace opening was cancelled".into())
                        }
                    }
                }
                OpenStage::Files(files) => {
                    let mut finished = true;
                    for file in files {
                        match file.result(world) {
                            Some(result) => result?,
                            None => finished = false,
                        }
                    }
                    Ok(finished)
                }
            }
        })();
        match result {
            Ok(false) => world.resource_mut::<ShellOpens>().0.push(request),
            Ok(true) => request.reply.complete(Ok(())),
            Err(error) => request.reply.complete(Err(error)),
        }
    }
}

#[cfg(feature = "automation")]
fn inspect(
    world: &mut bevy::prelude::World,
    _: serde_json::Value,
) -> bedtime_automation2::AutomationResult {
    use bedtime_terminal2::{TerminalPanel, TerminalStatus};
    use bedtime_workbench2::{
        InActiveWorkspace, InRunningWorkspace, InWorkspace, PanelInstance, PanelPresentation,
        WorkspaceWork,
    };
    use bevy::prelude::{Entity, Has};
    use serde_json::json;

    let panels: Vec<_> = world
        .query::<(
            Entity,
            &TerminalPanel,
            &PanelInstance,
            &InWorkspace,
            &PanelPresentation,
            Has<InActiveWorkspace>,
            Has<InRunningWorkspace>,
        )>()
        .iter(world)
        .map(|(entity, state, instance, workspace, presentation, active, running)| {
            let snapshot = state.snapshot();
            // A cell's text is the complete grapheme cluster. Width-zero cells
            // continue a wide cluster; neither they nor `character` add text.
            let lines: Vec<String> = snapshot
                .lines
                .iter()
                .map(|row| {
                    let mut text = String::new();
                    for cell in &row.cells {
                        if cell.width > 0 {
                            text.push_str(&cell.text);
                        }
                    }
                    text.trim_end_matches(' ').to_owned()
                })
                .collect();
            // Keep ordinary polling small: blank cells are represented by the
            // row text and grid dimensions. Occupied cells expose styling and
            // column widths for ANSI/wide-character observations. Color and
            // underline identifiers are debug descriptions, not restore data.
            let cells: Vec<_> = snapshot
                .lines
                .iter()
                .enumerate()
                .flat_map(|(row, line)| {
                    line.cells.iter().enumerate().filter_map(move |(col, cell)| {
                        (cell.width > 0 && !cell.text.is_empty() && cell.text.as_ref() != " ")
                            .then(|| json!({
                                "row": row,
                                "col": col,
                                "text": cell.text.as_ref(),
                                "width": cell.width,
                                "attributes": cell.mode,
                                "foreground": format!("{:?}", cell.fg),
                                "background": format!("{:?}", cell.bg),
                                "underline": format!("{:?}", cell.underline),
                            }))
                    })
                })
                .collect();
            let (status, exit) = match state.status() {
                TerminalStatus::Pending => ("Pending", None),
                TerminalStatus::Starting => ("Starting", None),
                TerminalStatus::Running => ("Running", None),
                TerminalStatus::Stopping => ("Stopping", None),
                TerminalStatus::Exited(exit) => ("Exited", exit.as_ref()),
                TerminalStatus::Failed(_) => ("Failed", None),
            };
            let modes = snapshot.modes;
            json!({
                "entity": entity.to_bits().to_string(),
                "workspace": workspace.0.to_bits().to_string(),
                "status": status,
                "tab_title": instance.title,
                "pid": state.process_id(),
                "exit_code": exit.and_then(|exit| exit.code()),
                "success": exit.map(|exit| exit.success()),
                "error": state.error(),
                "revision": snapshot.revision,
                "cursor_revision": snapshot.cursor_revision,
                "palette": {
                    "ansi": &snapshot.palette[..16],
                    "foreground": snapshot.palette[258],
                    "background": snapshot.palette[259],
                    "cursor": snapshot.palette[256],
                },
                "grid": {
                    "cols": snapshot.cols,
                    "rows": snapshot.rows,
                    "lines": lines,
                    "cells": cells,
                    "cursor": {
                        "row": snapshot.cursor.row,
                        "col": snapshot.cursor.col,
                        "shape": snapshot.cursor.shape,
                        "blinking": snapshot.cursor.blinking,
                        "visible": snapshot.cursor.visible,
                    },
                },
                "text": lines.join("\n"),
                "modes": {
                    "app_cursor": modes.app_cursor,
                    "app_keypad": modes.app_keypad,
                    "mouse_x10": modes.mouse_x10,
                    "mouse_button": modes.mouse_button,
                    "mouse_motion": modes.mouse_motion,
                    "mouse_many": modes.mouse_many,
                    "mouse_sgr": modes.mouse_sgr,
                    "focus_reporting": modes.focus_reporting,
                    "eight_bit": modes.eight_bit,
                    "reverse": modes.reverse,
                    "keyboard_lock": modes.keyboard_lock,
                    "bracket_paste": modes.bracket_paste,
                    "num_lock": modes.num_lock,
                    "alt_screen": modes.alt_screen,
                    "local_echo": modes.local_echo,
                    "newline_mode": modes.newline_mode,
                    "kitty_keyboard": modes.kitty_keyboard,
                    "modify_other_keys": modes.modify_other_keys,
                    "grapheme_clustering": modes.grapheme_clustering,
                },
                "title": snapshot.title.as_ref(),
                "cwd": snapshot.current_directory.as_ref().map(|path| path.to_string_lossy()),
                "display_offset": snapshot.display_offset,
                "history_size": snapshot.history_size,
                "warnings": snapshot.warnings.iter().map(|warning| warning.as_ref()).collect::<Vec<_>>(),
                // These are Workbench's last observed panel bounds, in egui
                // points. A hidden panel may also retain an older viewport.
                "presentation": {
                    "visible": presentation.visible,
                    "focused": presentation.focused,
                    "rect": {
                        "min": presentation.rect.min.to_array(),
                        "max": presentation.rect.max.to_array(),
                    },
                    "tab_rect": {
                        "min": presentation.tab_rect.min.to_array(),
                        "max": presentation.tab_rect.max.to_array(),
                    },
                    "close_rect": {
                        "min": presentation.close_rect.min.to_array(),
                        "max": presentation.close_rect.max.to_array(),
                    },
                },
                "active_workspace": active,
                "running_workspace": running,
            })
        })
        .collect();
    let workspaces: Vec<_> = world
        .query::<(Entity, &WorkspaceWork)>()
        .iter(world)
        .map(|(entity, work)| {
            json!({
                "entity": entity.to_bits().to_string(),
                "jobs": work.list().into_iter().map(|job| json!({
                    "id": job.id.0,
                    "owner": job.owner.map(|owner| owner.to_bits().to_string()),
                    "label": job.label,
                    "status": work_status(&job.status),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "panels": panels, "workspaces": workspaces }))
}

#[cfg(feature = "automation")]
fn work_status(status: &bedtime_workbench2::WorkStatus) -> String {
    use bedtime_workbench2::WorkStatus;
    match status {
        WorkStatus::Running => "Running".into(),
        WorkStatus::Cancelling => "Cancelling".into(),
        WorkStatus::Cancelled => "Cancelled".into(),
        WorkStatus::Finished => "Finished".into(),
        WorkStatus::Failed(error) => format!("Failed: {error}"),
    }
}
