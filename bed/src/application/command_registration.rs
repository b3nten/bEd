//! bEd's numbered layout and panel-navigation policy. Feature and editing
//! commands register themselves in their owning plugins.
use super::runtime::{self, LayoutCommand, LayoutSlot};
use bedtime_api::workbench::{CommandInvocation, CommandResult, WorkbenchAction};
use bevy::prelude::*;

pub fn register(world: &mut World) {
    bedtime_api::workbench::register(world, "bed.app", |registrar| {
        for (id, label) in [
            ("bed.app.SetCurrentLayout", "Set Current Layout"),
            ("bed.app.ResetLayout", "Reset Layout"),
            ("bed.app.SaveDefaultLayout", "Save Default Layout"),
            ("bed.app.ResetDefaultLayout", "Reset Default Layout"),
            ("bed.app.set_layout_startup", "Set Startup Layout"),
            ("bed.app.restore_layout_startup", "Restore Startup Layout"),
            ("bed.app.set_layout_1", "Set Layout 1"),
            ("bed.app.restore_layout_1", "Restore Layout 1"),
            ("bed.app.set_layout_2", "Set Layout 2"),
            ("bed.app.restore_layout_2", "Restore Layout 2"),
            ("bed.app.set_layout_3", "Set Layout 3"),
            ("bed.app.restore_layout_3", "Restore Layout 3"),
            ("bed.app.set_layout_4", "Set Layout 4"),
            ("bed.app.restore_layout_4", "Restore Layout 4"),
            ("bed.app.set_layout_5", "Set Layout 5"),
            ("bed.app.restore_layout_5", "Restore Layout 5"),
            ("bed.app.set_layout_6", "Set Layout 6"),
            ("bed.app.restore_layout_6", "Restore Layout 6"),
            ("bed.app.set_layout_7", "Set Layout 7"),
            ("bed.app.restore_layout_7", "Restore Layout 7"),
            ("bed.app.set_layout_8", "Set Layout 8"),
            ("bed.app.restore_layout_8", "Restore Layout 8"),
            ("bed.app.set_layout_9", "Set Layout 9"),
            ("bed.app.restore_layout_9", "Restore Layout 9"),
        ] {
            registrar.command(id, label, command);
        }
        registrar.command(
            "bed.app.layout_startup",
            "Switch to Startup Layout",
            command,
        );
        for (id, label, shortcut) in [
            ("bed.app.layout_1", "Switch to Layout 1", "Primary+1"),
            ("bed.app.layout_2", "Switch to Layout 2", "Primary+2"),
            ("bed.app.layout_3", "Switch to Layout 3", "Primary+3"),
            ("bed.app.layout_4", "Switch to Layout 4", "Primary+4"),
            ("bed.app.layout_5", "Switch to Layout 5", "Primary+5"),
            ("bed.app.layout_6", "Switch to Layout 6", "Primary+6"),
            ("bed.app.layout_7", "Switch to Layout 7", "Primary+7"),
            ("bed.app.layout_8", "Switch to Layout 8", "Primary+8"),
            ("bed.app.layout_9", "Switch to Layout 9", "Primary+9"),
        ] {
            registrar.command(id, label, command);
            registrar.global_shortcut(id, &[shortcut]);
        }
        for (id, label, shortcut) in [
            ("bed.app.focus_1", "Focus Panel 1", "Primary+Shift+1"),
            ("bed.app.focus_2", "Focus Panel 2", "Primary+Shift+2"),
            ("bed.app.focus_3", "Focus Panel 3", "Primary+Shift+3"),
            ("bed.app.focus_4", "Focus Panel 4", "Primary+Shift+4"),
            ("bed.app.focus_5", "Focus Panel 5", "Primary+Shift+5"),
            ("bed.app.focus_6", "Focus Panel 6", "Primary+Shift+6"),
            ("bed.app.focus_7", "Focus Panel 7", "Primary+Shift+7"),
            ("bed.app.focus_8", "Focus Panel 8", "Primary+Shift+8"),
            ("bed.app.focus_9", "Focus Panel 9", "Primary+Shift+9"),
        ] {
            registrar.command(id, label, focus);
            registrar.global_shortcut(id, &[shortcut]);
        }
    })
    .expect("bEd layout policy is registered once");
}

fn command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let command = match invocation.command.as_str() {
        "bed.app.SetCurrentLayout" => LayoutCommand::SetCurrentLayout,
        "bed.app.ResetLayout" => LayoutCommand::ResetLayout,
        "bed.app.SaveDefaultLayout" => LayoutCommand::SaveDefaultLayout,
        "bed.app.ResetDefaultLayout" => LayoutCommand::ResetDefaultLayout,
        id if id.starts_with("bed.app.layout_") => {
            LayoutCommand::UseLayout(LayoutSlot::from_key(&id[15..]).ok_or("Invalid layout slot")?)
        }
        id if id.starts_with("bed.app.set_layout_") => LayoutCommand::SetLayoutDefault(
            LayoutSlot::from_key(
                id.strip_prefix("bed.app.set_layout_")
                    .expect("layout command prefix matched"),
            )
            .ok_or("Invalid layout slot")?,
        ),
        id if id.starts_with("bed.app.restore_layout_") => LayoutCommand::RestoreBuiltinLayout(
            LayoutSlot::from_key(
                id.strip_prefix("bed.app.restore_layout_")
                    .expect("layout command prefix matched"),
            )
            .ok_or("Invalid layout slot")?,
        ),
        _ => return Err("Unknown bEd layout command".into()),
    };
    runtime::apply_layout_command(world, command).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

fn focus(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let index = invocation
        .command
        .strip_prefix("bed.app.focus_")
        .and_then(|index| index.parse::<usize>().ok())
        .ok_or("Invalid panel focus command")?;
    Ok(bedtime_core::active_panel_entities(world)
        .get(index - 1)
        .map(|panel| WorkbenchAction::FocusPanel(*panel))
        .into_iter()
        .collect())
}
