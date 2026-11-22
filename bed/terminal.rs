//! Terminals, and bEd's shell commands: `+open` (which carries the default
//! workspace's terminals into the folder it opens) and `+web`.
//!
//! Carrying: leaving the default workspace for a folder with `+open` takes
//! the active layout's shells along. They keep running in the folder's
//! workspace, in the places of its terminals (in tab order; any left over
//! go beside them, else in the largest area), and the one that ran `+open`
//! is focused. The default workspace still saves its terminals (as panels),
//! so it starts fresh shells there next time.
use crate::workspace::Workspace;
use bedtime::terminal::{
    ShellCommandsAppExt, ShellInvocation, TerminalPanel, TerminalPlugin, TerminalSession,
};
use bedtime::workbench::{
    Area, BedtimeWorldExt, ErrorDialog, InArea, ManagedWork, Notification, OnWorkspaceClosing,
    OnWorkspaceOpen, PathOptions, Place,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

pub struct TerminalsPlugin;

impl Plugin for TerminalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TerminalPlugin)
            // Carrying terminals into the folder `+open` opens.
            .add_systems(Update, forget_cancelled)
            .add_systems(OnWorkspaceClosing, take)
            .add_systems(
                OnWorkspaceOpen,
                resume.after(bedtime::workbench::layout_slots::OpenFirstLayout),
            )
            .add_shell_command(["+open", "+o"], open_from_shell)
            .expect("bed's shell commands are unique")
            .add_shell_command(["+web"], web_from_shell)
            .expect("bed's shell commands are unique")
            // Web pages open in bEd's own browser; other links (mail,
            // and pages where it has no browser) as the system does.
            .insert_resource(bedtime::ui::ExternalOpener(|world, url, from| {
                let page = url.starts_with("https://") || url.starts_with("http://");
                if page && bedtime::web::open_url(world, url, from).is_ok() {
                    return Ok(());
                }
                bedtime::ui::open_with_system(url)
            }));
    }
}

/// `+web bevy.org localhost:3000`: each address in a web browser (as
/// links open, `open_externally_from` the terminal); `+web` alone opens
/// a blank one. Unlike `+open`, whatever it's given is an address.
fn web_from_shell(world: &mut World, invocation: ShellInvocation) {
    let mut result = Ok(());
    if invocation.arguments.is_empty() {
        result = world
            .bedtime_mut()
            .open_panel_at(
                bedtime::web::WebPanel::default(),
                bedtime::workbench::Placement::From(invocation.panel),
            )
            .map(|_| ());
    }
    for argument in &invocation.arguments {
        let url = bedtime::web::with_scheme(&argument.to_string_lossy());
        if let Err(error) = world
            .bedtime_mut()
            .open_externally_from(&url, invocation.panel)
        {
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
            switch_carrying_terminals(world, folder, origin);
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
            && let Some(url) = bedtime::web::web_address(&text)
        {
            if let Err(error) = world
                .bedtime_mut()
                .open_externally_from(&url, invocation.panel)
            {
                result = Err(error);
            }
            continue;
        }
        let (path, line) = location(&invocation.directory, &text);
        // Somewhere other than the terminal it was typed in.
        let options = PathOptions {
            line,
            place: Some(bedtime::workbench::Placement::From(invocation.panel)),
            ..default()
        };
        if let Err(error) = world.bedtime_mut().open_path(&path, options) {
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

/// The switch under way carries the terminals. `origin` ran `+open`.
#[derive(Resource)]
struct CarryTerminals {
    origin: Entity,
}

/// A shell between workspaces. It isn't `InWorkspace`, so the switch
/// neither despawns it nor stops its work (it owns the shell's task).
#[derive(Component)]
struct Carried {
    session: Option<TerminalSession>,
    /// Its tab order in the old layout.
    order: usize,
    origin: bool,
}

/// `workspace::switch`, carrying the terminals when leaving the default
/// workspace.
pub fn switch_carrying_terminals(world: &mut World, folder: PathBuf, origin: Entity) {
    if world.resource::<Workspace>().default {
        world.insert_resource(CarryTerminals { origin });
    }
    if let Err(error) = crate::workspace::open_workspace(world, Some(folder)) {
        world.remove_resource::<CarryTerminals>();
        world.bedtime_mut().request_error_dialog(ErrorDialog {
            title: "Open Folder".into(),
            message: error,
            details: None,
        });
    }
}

/// A switch cancelled at the unsaved-documents prompt carries nothing
/// later.
fn forget_cancelled(world: &mut World) {
    if world.contains_resource::<CarryTerminals>() && !world.bedtime().is_exiting() {
        world.remove_resource::<CarryTerminals>();
    }
}

/// The switch is certain: take each shell out of its panel, before the
/// workspace's work stops.
fn take(world: &mut World) {
    let Some(carry) = world.remove_resource::<CarryTerminals>() else {
        return;
    };
    if !world.resource::<Workspace>().default {
        return;
    }
    let Some(layout) = world.bedtime().layouts().active() else {
        return;
    };
    for (order, panel) in world
        .bedtime()
        .layouts()
        .panels(layout)
        .into_iter()
        .enumerate()
    {
        let Some(session) = world
            .get_mut::<TerminalPanel>(panel)
            .and_then(|mut terminal| terminal.take_session())
        else {
            continue;
        };
        let work = session.work_id();
        let holder = world
            .spawn(Carried {
                session: Some(session),
                order,
                origin: panel == carry.origin,
            })
            .id();
        let _ = world
            .resource_mut::<ManagedWork>()
            .set_owner(work, Some(holder));
    }
}

/// In the new workspace, after its layout opens: each shell in a
/// terminal's place, else beside the last, else in the largest area.
fn resume(world: &mut World) {
    let mut holders: Vec<(Entity, usize)> = world
        .query::<(Entity, &Carried)>()
        .iter(world)
        .map(|(holder, carried)| (holder, carried.order))
        .collect();
    if holders.is_empty() {
        return;
    }
    holders.sort_by_key(|&(_, order)| order);
    let mut places: Vec<Entity> = match world.bedtime().layouts().active() {
        Some(layout) => world
            .bedtime()
            .layouts()
            .panels(layout)
            .into_iter()
            .filter(|&panel| world.get::<TerminalPanel>(panel).is_some())
            .collect(),
        None => Vec::new(),
    };
    places.reverse();
    let (mut area, mut focus, mut lost) = (None, None, 0);
    for (holder, _) in holders {
        let mut carried = world.get_mut::<Carried>(holder).unwrap();
        let (session, origin) = (carried.session.take().unwrap(), carried.origin);
        world.despawn(holder);
        let work = session.work_id();
        let terminal = TerminalPanel::resume(session);
        let panel = match places.pop() {
            Some(old) => replace(world, old, terminal),
            None => {
                let place = area.map_or_else(Place::largest, Place::in_area);
                let panel = world.spawn((terminal, place.focus(false))).id();
                world.flush();
                panel
            }
        };
        let Some(placed) = world.get::<InArea>(panel).map(|a| a.0) else {
            lost += 1;
            continue;
        };
        area = Some(placed);
        let _ = world
            .resource_mut::<ManagedWork>()
            .set_owner(work, Some(panel));
        if origin {
            focus = Some(panel);
        }
    }
    if let Some(panel) = focus {
        let _ = world.bedtime_mut().layouts_mut().focus_panel(panel);
    }
    if lost > 0 {
        world.bedtime_mut().notify(Notification::error(format!(
            "Couldn't place {lost} terminal(s) from the default workspace"
        )));
    }
}

/// Put `terminal` where `old` is (its tab, and selected if it was),
/// and close `old`: a terminal the workspace restored, not started yet.
fn replace(world: &mut World, old: Entity, terminal: TerminalPanel) -> Entity {
    let area = world.get::<InArea>(old).unwrap().0;
    let index = world
        .bedtime()
        .layouts()
        .panels_in_area(area)
        .iter()
        .position(|&panel| panel == old)
        .unwrap_or(0);
    let selected = world.get::<Area>(area).and_then(|a| a.selected_panel());
    let panel = world
        .spawn((terminal, Place::in_area(area).focus(false)))
        .id();
    world.flush();
    if world.get_entity(panel).is_err() {
        return panel;
    }
    let _ = world
        .bedtime_mut()
        .layouts_mut()
        .move_tab(panel, area, index);
    world.despawn(old);
    let selected = if selected == Some(old) {
        Some(panel)
    } else {
        selected
    };
    world.get_mut::<Area>(area).unwrap().selected = selected;
    world.flush();
    panel
}
