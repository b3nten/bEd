//! What an app with workspaces has in its File menu: New Window, Open
//! Folder, Open Recent and Close Folder; and a folder dropped on the window
//! opens as the workspace. Installed by `WorkspacesPlugin`.
use crate::workspaces::{RecentWorkspaces, Workspace, open_workspace};
use crate::{
    Action, ActionAppExt, ActionInvocation, ActionResult, BedtimeWorldExt, ErrorDialog, FileDrops,
    Handler, InMenu, Invokes, KeyChord, Menu, MenuEntries, MenuItem, MenusAppExt, Modifiers,
    PanelFactory, StandardMenu,
};
use bevy::prelude::*;
use std::path::PathBuf;

/// Another window, in its own process, starting as the app does from the
/// command line.
pub struct NewWindow;
impl Action for NewWindow {
    const KEY: &'static str = "app.new_window";
    const LABEL: &'static str = "New Window";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        Modifiers::PRIMARY.union(Modifiers::SHIFT),
        KeyCode::KeyN,
    )];
}

/// Ask for a folder, with the system's picker (`native` feature), and open
/// it as the workspace.
pub struct OpenFolder;
impl Action for OpenFolder {
    const KEY: &'static str = "app.open_folder";
    const LABEL: &'static str = "Open Folder…";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyO)];
}

/// From an Open Recent item, opens that item's folder; from a key, the most
/// recent folder other than this one.
pub struct OpenRecent;
impl Action for OpenRecent {
    const KEY: &'static str = "app.open_recent";
    const LABEL: &'static str = "Open Recent";
}

/// Back to the default workspace, in this window.
pub struct CloseFolder;
impl Action for CloseFolder {
    const KEY: &'static str = "app.close_folder";
    const LABEL: &'static str = "Close Folder";
}

/// On an Open Recent item: the folder it opens.
#[derive(Component)]
struct RecentPath(PathBuf);

/// The Open Recent submenu, refilled when the list changes.
#[derive(Component)]
struct RecentMenu;

/// A folder dropped on the window, to open as the workspace (in `Update`,
/// not while the layout draws).
#[derive(Resource, Default)]
struct DroppedFolder(Option<PathBuf>);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<DroppedFolder>()
        .insert_resource(FileDrops(dropped))
        .add_action_handler::<NewWindow>(Handler::new(new_window))
        .add_action_handler::<CloseFolder>(
            Handler::new(close_folder)
                .available_when(|world, _| !world.resource::<Workspace>().default),
        )
        .add_action_handler::<OpenRecent>(
            Handler::new(open_recent).available_when(|world, _| other_recent(world).is_some()),
        )
        .add_menu_item::<NewWindow>(StandardMenu::File, "new");
    #[cfg(feature = "native")]
    app.add_action_handler::<OpenFolder>(
        Handler::new(open_folder).available_when(|world, _| !world.bedtime().is_picking()),
    )
    .add_menu_item::<OpenFolder>(StandardMenu::File, "open");
    app.add_menu_entry(StandardMenu::File, "open", recent_menu)
        .add_menu_item::<CloseFolder>(StandardMenu::File, "open")
        .add_systems(Update, (open_dropped_folder, fill_recent_menu));
    #[cfg(feature = "native")]
    crate::DockMenuAppExt::add_dock_menu_item::<NewWindow>(app);
}

/// Switch this window to `folder`'s workspace (`None`: the default one), or
/// explain why not.
pub fn switch(world: &mut World, folder: Option<PathBuf>) {
    if let Err(error) = open_workspace(world, folder) {
        world.bedtime_mut().request_error_dialog(ErrorDialog {
            title: "Open Folder".into(),
            message: error,
            details: None,
        });
    }
}

/// Files dropped on a layout open where they're dropped (with documents);
/// a folder opens as the workspace.
fn dropped(world: &mut World, paths: Vec<PathBuf>) -> Option<PanelFactory> {
    if let [folder] = paths.as_slice()
        && folder.is_dir()
    {
        world.resource_mut::<DroppedFolder>().0 = Some(folder.clone());
        return None;
    }
    #[cfg(feature = "files")]
    return crate::opening::dropped_files(world, paths);
    #[cfg(not(feature = "files"))]
    None
}

fn open_dropped_folder(world: &mut World) {
    if let Some(folder) = world.resource_mut::<DroppedFolder>().0.take() {
        switch(world, Some(folder));
    }
}

/// The Open Recent submenu: filled from the recent list.
fn recent_menu(world: &mut World) -> Entity {
    world
        .spawn((
            Menu {
                label: "Open Recent".into(),
            },
            RecentMenu,
        ))
        .id()
}

/// Recent folders other than this one, in Open Recent.
fn fill_recent_menu(
    recent: Res<RecentWorkspaces>,
    workspace: Option<Res<Workspace>>,
    menus: Query<(Entity, Option<&MenuEntries>), With<RecentMenu>>,
    mut commands: Commands,
) {
    let Ok((menu, entries)) = menus.single() else {
        return;
    };
    if !recent.is_changed() && entries.is_some() {
        return;
    }
    for &entry in entries.map_or(&[][..], |entries| entries.entities()) {
        commands.entity(entry).despawn();
    }
    let current = workspace
        .filter(|workspace| !workspace.default)
        .map(|workspace| workspace.root.clone());
    let others: Vec<_> = recent
        .0
        .iter()
        .filter(|path| Some(*path) != current.as_ref())
        .collect();
    if others.is_empty() {
        // A menu needs an entry; this one is never enabled.
        commands.spawn((MenuItem::new("No Recent Folders"), InMenu(menu)));
    }
    for path in others {
        commands.spawn((
            MenuItem::new(path.display().to_string()),
            Invokes::of::<OpenRecent>(),
            RecentPath(path.clone()),
            InMenu(menu),
        ));
    }
}

#[cfg(feature = "native")]
fn open_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
    let picker = crate::PathPicker::new("Open Folder", crate::Pick::Folder);
    world.bedtime_mut().pick_path(picker, |world, chosen| {
        if let Some(folder) = chosen {
            switch(world, Some(folder));
        }
    });
    Ok(())
}

fn open_recent(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let chosen = invocation
        .source
        .and_then(|source| world.get::<RecentPath>(source))
        .map(|path| path.0.clone());
    let folder = chosen
        .or_else(|| other_recent(world))
        .ok_or("No recent folder")?;
    switch(world, Some(folder));
    Ok(())
}

fn close_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
    switch(world, None);
    Ok(())
}

/// This window's folder, if it isn't the default workspace.
fn current_folder(world: &World) -> Option<&PathBuf> {
    let workspace = world.get_resource::<Workspace>()?;
    (!workspace.default).then_some(&workspace.root)
}

/// The most recent folder other than this window's.
fn other_recent(world: &World) -> Option<PathBuf> {
    let current = current_folder(world);
    let recent = world.resource::<RecentWorkspaces>();
    recent.0.iter().find(|path| Some(*path) != current).cloned()
}

fn new_window(_: &mut World, _: ActionInvocation) -> ActionResult {
    spawn_window()
}

/// Start the app again, as from the command line: another window.
pub fn spawn_window() -> Result<(), String> {
    std::env::current_exe()
        .and_then(|app| std::process::Command::new(app).spawn())
        .map(|_| ())
        .map_err(|error| format!("Couldn't open another window: {error}"))
}
