//! Terminals: New Terminal opens a shell in the workspace's folder. In it,
//! `+open path[:line[:column]]` (or `+o`) opens files in bed.
use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation, TerminalPanel, TerminalPlugin};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, InArea, KeyChord,
    LayoutsWorldExt, Modifiers, PathOptions, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

pub struct NewTerminal;
impl Action for NewTerminal {
    const KEY: &'static str = "app.new_terminal";
    const LABEL: &'static str = "New Terminal";
    const DEFAULT_KEYS: &'static [KeyChord] =
        &[KeyChord::new(Modifiers::CONTROL, KeyCode::Backquote)];
}

pub struct TerminalsPlugin;

impl Plugin for TerminalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TerminalPlugin)
            .add_action_handler::<NewTerminal>(Handler::new(new_terminal))
            .add_shell_command(["+open", "+o"], open_from_shell)
            .expect("bed's shell commands are unique");
    }
}

/// A new terminal in the current layout's largest area, focused.
fn new_terminal(world: &mut World, _: ActionInvocation) -> ActionResult {
    let layout = match world.layouts().active() {
        Some(layout) => layout,
        None => crate::layouts::switch(world, crate::layouts::LayoutSlot(0))?,
    };
    let area = world.layouts().destination_area(layout)?;
    let panel = world.spawn((TerminalPanel::default(), InArea(area))).id();
    world.flush();
    world.layouts_mut().focus_panel(panel)
}

/// `+open a.rs b.rs:12 c.rs:3:5`, relative to the shell's directory.
fn open_from_shell(world: &mut World, invocation: ShellInvocation) {
    let mut result = Ok(());
    if invocation.arguments.is_empty() {
        result = Err(format!("usage: {} path[:line[:column]]…", invocation.alias));
    }
    for argument in &invocation.arguments {
        let (path, line) = location(&invocation.directory, &argument.to_string_lossy());
        let options = PathOptions { line, ..default() };
        if let Err(error) = world.workbench_mut().open_path(&path, options) {
            result = Err(format!("{}: {error}", path.display()));
        }
    }
    invocation.reply.complete(result);
}

/// A path with an optional `:line` or `:line:column` (from 1), resolved
/// against `directory`. A path that exists as written wins (a name with a
/// colon in it).
fn location(directory: &Path, argument: &str) -> (PathBuf, Option<(u32, u32)>) {
    let resolve = |path: &str| directory.join(path);
    if resolve(argument).exists() {
        return (resolve(argument), None);
    }
    let mut parts = argument.rsplitn(3, ':');
    let numbers: Vec<&str> = parts.by_ref().take(2).collect();
    let rest = parts.next();
    match (rest, numbers.as_slice()) {
        (Some(path), [column, line]) => match (line.parse::<u32>(), column.parse::<u32>()) {
            (Ok(line), Ok(column)) => (
                resolve(path),
                Some((line.saturating_sub(1), column.saturating_sub(1))),
            ),
            _ => location_line(directory, argument),
        },
        _ => location_line(directory, argument),
    }
}

fn location_line(directory: &Path, argument: &str) -> (PathBuf, Option<(u32, u32)>) {
    match argument.rsplit_once(':') {
        Some((path, line)) if line.parse::<u32>().is_ok() => (
            directory.join(path),
            Some((line.parse::<u32>().unwrap().saturating_sub(1), 0)),
        ),
        _ => (directory.join(argument), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_locations_take_a_line_and_column() {
        let directory = Path::new("/project");
        assert_eq!(location(directory, "a.rs"), (directory.join("a.rs"), None));
        assert_eq!(
            location(directory, "src/a.rs:12"),
            (directory.join("src/a.rs"), Some((11, 0)))
        );
        assert_eq!(
            location(directory, "a.rs:3:5"),
            (directory.join("a.rs"), Some((2, 4)))
        );
        assert_eq!(
            location(directory, "/abs/b.rs:1"),
            (PathBuf::from("/abs/b.rs"), Some((0, 0)))
        );
    }
}
