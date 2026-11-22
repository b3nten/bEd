//! Leaving the default workspace for a folder with `+open` takes the
//! active layout's shells along: they keep running in the folder's
//! workspace, in the places of its terminals (in tab order; any left over
//! go beside them, else in the largest area), and the one that ran
//! `+open` is focused. The default workspace still saves its terminals
//! (as panels), so it starts fresh shells there next time.
use crate::workspace::Workspace;
use bedtime_terminal::{TerminalPanel, TerminalSession};
use bedtime_workbench::{
    Area, BedtimeWorldExt, ErrorDialog, InArea, ManagedWork, Notification, OnWorkspaceClosing,
    OnWorkspaceOpen, Place,
};
use bevy::prelude::*;
use std::path::PathBuf;

pub struct CarryTerminalsPlugin;

impl Plugin for CarryTerminalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, forget_cancelled)
            .add_systems(OnWorkspaceClosing, take)
            .add_systems(
                OnWorkspaceOpen,
                resume.after(bedtime_workbench::layout_slots::OpenFirstLayout),
            );
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
