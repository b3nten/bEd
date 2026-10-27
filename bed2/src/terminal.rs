//! Application-owned shell commands and optional terminal observations.
use bedtime_terminal2::{
    ShellCommandsAppExt, ShellInvocation, ShellReply, TerminalPanel, TerminalSystems,
};
use bedtime_workbench2::{ManagedTask, ManagedWork, Workbench};
use bevy::prelude::*;
use std::{ffi::OsString, path::PathBuf, sync::mpsc};

pub struct TerminalPlugin;
impl Plugin for TerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bedtime_terminal2::TerminalPlugin);
        install(app);
    }
}

fn install(app: &mut App) {
    register_menu(app);
    app.init_resource::<ShellOpens>()
        .add_shell_command(["+ducky"], duck_from_shell)
        .expect("Ducky shell alias")
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
        task: ManagedTask<Result<OpenTargets, String>>,
    },
    Folder(crate::opening::FolderOpenResult),
    Files(Vec<crate::opening::OpenedFile>),
}

struct ShellOpen {
    panel: Entity,
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
        let path = bedtime_filesystem2::FileSource::local(&path)
            .canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let metadata = path
            .metadata()
            .map_err(|error| format!("{}: {error}", path.path().display()))?;
        let path = path.path().to_owned();
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
        if !crate::storage::ready(world) {
            return Err(
                "The originating workspace is still loading or has a settings error".into(),
            );
        }
        let layout = Workbench::for_entity(world, panel)?.panel_layout(panel)?;
        let task = world
            .get_resource_mut::<ManagedWork>()
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
                    .get_resource::<ManagedWork>()
                    .is_none_or(ManagedWork::is_closing)
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
                            if world.get::<bedtime_workbench2::Layout>(*layout).is_none() {
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
    use bedtime_workbench2::{ManagedWork, PanelInstance, PanelPresentation};
    use bevy::prelude::Entity;
    use serde_json::json;

    let panels: Vec<_> = world
        .query::<(
            Entity,
            &TerminalPanel,
            &PanelInstance,
            &PanelPresentation,
        )>()
        .iter(world)
        .map(|(entity, state, instance, presentation)| {
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
            })
        })
        .collect();
    let jobs: Vec<_> = world.resource::<ManagedWork>().list().into_iter().map(|job| json!({
        "id": job.id.0, "owner": job.owner.map(|owner| owner.to_bits().to_string()), "label": job.label, "status": work_status(&job.status)
    })).collect();
    Ok(json!({ "panels": panels, "jobs": jobs }))
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

fn duck_from_shell(world: &mut World, invocation: ShellInvocation) {
    let result = show_duck(world, &invocation.arguments).map(|_| ());
    invocation.reply.complete(result);
}

fn show_duck(world: &mut World, arguments: &[OsString]) -> Result<Entity, String> {
    if !arguments.is_empty() {
        return Err("usage: +ducky".into());
    }
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before opening Ducky".into());
    }
    let layout =
        bedtime_workbench2::active_layout(world).ok_or("Workspace has no selected layout")?;
    crate::menus::open_or_focus(
        world,
        layout,
        <bedtime_mascots2::DuckPanel as bedtime_workbench2::Panel>::KEY,
    )
}

#[cfg(test)]
mod mascot_tests {
    use super::*;
    use bedtime_workbench2::{Panel, PanelInstance, WorkbenchCorePlugin};

    #[test]
    fn duck_shell_uses_origin_workspace_reuses_panel_and_rejects_arguments() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        crate::mascots::register_menu(&mut app);
        let _origin = crate::test_layout(app.world_mut());
        let other_id = crate::test_layout(app.world_mut());
        let other = other_id;
        Workbench::new(app.world_mut())
            .activate_layout(other_id)
            .unwrap();
        let duck = show_duck(app.world_mut(), &[]).unwrap();
        assert!(
            app.world()
                .get::<bedtime_workbench2::InArea>(duck)
                .is_some()
        );
        assert_eq!(show_duck(app.world_mut(), &[]).unwrap(), duck);
        assert_eq!(bedtime_workbench2::active_layout(app.world()), Some(other));
        let focused = show_duck(app.world_mut(), &[]).unwrap();
        assert_eq!(focused, duck);
        assert!(
            show_duck(app.world_mut(), &["extra".into()])
                .unwrap_err()
                .contains("usage")
        );
        assert_eq!(
            app.world_mut()
                .query::<&PanelInstance>()
                .iter(app.world())
                .filter(|p| p.key == bedtime_mascots2::DuckPanel::KEY)
                .count(),
            1
        );
        let layout = bedtime_workbench2::active_layout(app.world()).unwrap();
        app.world_mut()
            .entity_mut(layout)
            .remove::<bedtime_workbench2::ActiveLayout>();
        assert!(show_duck(app.world_mut(), &[]).is_err());
    }

    #[test]
    fn duck_shell_rejects_opening_after_quit_was_requested() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, crate::close::ClosePlugin))
            .add_message::<AppExit>();
        let _owner = crate::test_layout(app.world_mut());
        app.world_mut()
            .write_message(bedtime_workbench2::CloseRequested { window: None });
        app.update();
        assert!(
            show_duck(app.world_mut(), &[])
                .unwrap_err()
                .contains("Cancel quit")
        );
        assert_eq!(
            app.world_mut()
                .query::<&bedtime_mascots2::DuckPanel>()
                .iter(app.world())
                .count(),
            0
        );
    }
}

use bedtime_workbench2::persistence::ReflectWorkbenchPersist;

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(bedtime_terminal2::TerminalPanel)]
pub(crate) struct PersistedTerminal;

pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedTerminal>();
}
pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench2::persistence::CapturedWorld,
) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<TerminalPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        captured.insert_component(panel, PersistedTerminal);
        // Foreground process names are observations, not restored process state.
        if let Some(mut state) = captured.component::<bedtime_workbench2::PanelInstance>(panel) {
            state.title = "Terminal".into();
            captured.insert_component(panel, state);
        }
    }
}
pub(crate) fn clear_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PersistedTerminal>>()
        .iter(world)
        .collect();
    for panel in panels {
        world.entity_mut(panel).remove::<PersistedTerminal>();
    }
}

pub(crate) fn activate_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PersistedTerminal>>()
        .iter(world)
        .collect();
    let directory = crate::settings::effective_preferences(world)
        .terminal
        .unwrap_or_default()
        .project_directory
        .then(|| crate::files::project_directory(world))
        .flatten();
    for panel in panels {
        world
            .get_mut::<TerminalPanel>(panel)
            .unwrap()
            .options
            .working_directory = directory.clone();
    }
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<TerminalPanel>(
        app,
        Some(40),
        bedtime_workbench2::PanelFactory::new("Terminal", |world, area| {
            let options = bedtime_terminal2::TerminalOptions {
                working_directory: crate::files::project_directory(world),
                ..default()
            };
            world
                .spawn((
                    TerminalPanel::new(options),
                    bedtime_workbench2::InArea(area),
                ))
                .id()
        }),
        crate::storage::ready,
    );
}
