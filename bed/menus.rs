//! bEd's own menu items: quitting, folders (Open, Open Recent, Close) and
//! new windows.
use crate::workspace::{RecentWorkspaces, Workspace, switch};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, CloseRequested, Handler, InMenu, Invokes,
    KeyChord, Menu, MenuEntries, MenuItem, MenusAppExt, StandardMenu,
};
use bevy::prelude::*;
use std::{future::Future, path::PathBuf, pin::Pin};

pub struct Quit;
impl Action for Quit {
    const KEY: &'static str = "app.quit";
    const LABEL: &'static str = "Quit bEd";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyQ)];
}

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

/// Another window, in its own process, starting as bed does from the command line.
pub struct NewWindow;
impl Action for NewWindow {
    const KEY: &'static str = "app.new_window";
    const LABEL: &'static str = "New Window";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        bedtime_workbench::Modifiers::PRIMARY.union(bedtime_workbench::Modifiers::SHIFT),
        KeyCode::KeyN,
    )];
}

/// On an Open Recent item: the folder it opens.
#[derive(Component)]
struct RecentPath(PathBuf);

/// The Open Recent submenu, refilled when the list changes.
#[derive(Component)]
struct RecentMenu;

pub struct MenusPlugin;

impl Plugin for MenusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecentWorkspaces>()
            .init_non_send::<FolderDialog>()
            .add_action_handler::<Quit>(Handler::new(quit))
            .add_action_handler::<NewWindow>(Handler::new(new_window))
            .add_action_handler::<CloseFolder>(
                Handler::new(close_folder)
                    .available_when(|world, _| !world.resource::<Workspace>().default),
            )
            .add_action_handler::<OpenFolder>(
                Handler::new(open_folder)
                    .available_when(|world, _| world.non_send::<FolderDialog>().0.is_none()),
            )
            .add_action_handler::<OpenRecent>(
                Handler::new(open_recent).available_when(|world, _| other_recent(world).is_some()),
            )
            .add_menu_item::<Quit>(StandardMenu::App, "quit")
            .add_menu_item::<NewWindow>(StandardMenu::File, "new")
            .add_menu_item::<bedtime_diagram::NewDiagram>(StandardMenu::File, "new")
            .add_menu_item::<OpenFolder>(StandardMenu::File, "open")
            .add_menu_entry(StandardMenu::File, "open", recent_menu)
            .add_menu_item::<CloseFolder>(StandardMenu::File, "open")
            .add_systems(Update, (poll_folder_dialog, fill_recent_menu));
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

fn quit(world: &mut World, _: ActionInvocation) -> ActionResult {
    world.write_message(CloseRequested { window: None });
    Ok(())
}

/// The folder picker, while it's open. AppKit owns the dialog, so its future is
/// polled on the main thread; nothing blocks while it's open.
#[derive(Default)]
struct FolderDialog(Option<Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>>);

fn open_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Open Folder")
        .pick_folder();
    world.non_send_mut::<FolderDialog>().0 = Some(Box::pin(dialog));
    Ok(())
}

fn poll_folder_dialog(world: &mut World) {
    let mut dialog = world.non_send_mut::<FolderDialog>();
    let Some(future) = dialog.0.as_mut() else {
        return;
    };
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(chosen) = future.as_mut().poll(&mut context) else {
        return;
    };
    dialog.0 = None;
    if let Some(folder) = chosen {
        switch(world, Some(folder.path().to_owned()));
    }
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

/// Start another bEd, as from the command line.
pub fn spawn_window() -> Result<(), String> {
    std::env::current_exe()
        .and_then(|bed| std::process::Command::new(bed).spawn())
        .map(|_| ())
        .map_err(|error| format!("Couldn't start another bEd: {error}"))
}
