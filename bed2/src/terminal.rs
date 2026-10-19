//! Application-only terminal observations. Shell input uses the normal UI path.
use bevy::prelude::App;

pub fn install(_app: &mut App) {
    #[cfg(feature = "automation")]
    {
        use bedtime_automation2::AutomationAppExt;
        _app.register_automation_command("app.inspect_terminal", inspect);
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
