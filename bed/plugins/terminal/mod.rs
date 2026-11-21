//! Terminals, and bEd's shell commands: `+open` (which carries the default
//! workspace's terminals into the folder it opens) and `+web`.
mod carry;

use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation, TerminalPlugin};
use bedtime_workbench::{PathOptions, WorkbenchWorldExt};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

pub struct TerminalsPlugin;

impl Plugin for TerminalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((TerminalPlugin, carry::CarryTerminalsPlugin))
            .add_shell_command(["+open", "+o"], open_from_shell)
            .expect("bed's shell commands are unique")
            .add_shell_command(["+web"], web_from_shell)
            .expect("bed's shell commands are unique")
            // Web pages open in bEd's own browser; other links (mail,
            // and pages where it has no browser) as the system does.
            .insert_resource(bedtime_ui::ExternalOpener(|world, url, from| {
                let page = url.starts_with("https://") || url.starts_with("http://");
                if page && bedtime_web::open_url(world, url, from).is_ok() {
                    return Ok(());
                }
                bedtime_ui::open_with_system(url)
            }));
    }
}

/// `+web bevy.org localhost:3000`: each address in a web browser (as
/// links open, `open_externally_from` the terminal); `+web` alone opens
/// a blank one. Unlike `+open`, whatever it's given is an address.
fn web_from_shell(world: &mut World, invocation: ShellInvocation) {
    use bedtime_ui::LinksWorldExt;
    use bedtime_workbench::PanelsWorldExt;
    let mut result = Ok(());
    if invocation.arguments.is_empty() {
        result = world
            .open_panel_at(
                bedtime_web::WebPanel::default(),
                bedtime_workbench::Placement::From(invocation.panel),
            )
            .map(|_| ());
    }
    for argument in &invocation.arguments {
        let url = bedtime_web::with_scheme(&argument.to_string_lossy());
        if let Err(error) = world.open_externally_from(&url, invocation.panel) {
            result = Err(error);
        }
    }
    invocation.reply.complete(result);
}

/// `+open a.rs b.rs:12 c.rs:3:5`, relative to the shell's directory;
/// `+open` or `+open folder` opens the folder as the workspace. A web
/// address (`https://…`, `www.…`, `localhost:…`) that isn't a file opens
/// as links do.
fn open_from_shell(world: &mut World, invocation: ShellInvocation) {
    let folder = match invocation.arguments.as_slice() {
        [] => Some(invocation.directory.clone()),
        [only] => Some(invocation.directory.join(only)).filter(|path| path.is_dir()),
        _ => None,
    };
    if let Some(folder) = folder {
        invocation.reply.complete(Ok(()));
        // After this frame.
        let origin = invocation.panel;
        world.commands().queue(move |world: &mut World| {
            carry::switch_carrying_terminals(world, folder, origin);
        });
        return;
    }
    let mut result = Ok(());
    for argument in &invocation.arguments {
        if invocation.directory.join(argument).is_dir() {
            result = Err(format!(
                "{}: open a folder on its own",
                argument.to_string_lossy()
            ));
            continue;
        }
        let text = argument.to_string_lossy();
        if !invocation.directory.join(argument).exists()
            && let Some(url) = bedtime_web::web_address(&text)
        {
            if let Err(error) =
                bedtime_ui::LinksWorldExt::open_externally_from(world, &url, invocation.panel)
            {
                result = Err(error);
            }
            continue;
        }
        let (path, line) = location(&invocation.directory, &text);
        // Somewhere other than the terminal it was typed in.
        let options = PathOptions {
            line,
            place: Some(bedtime_workbench::Placement::From(invocation.panel)),
            ..default()
        };
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
