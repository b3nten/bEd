//! bEd's menus and titlebar toolbar, and the app-level commands they run.
//!
//! Menus and tools only name commands: labels, shortcuts and whether they're
//! enabled all come from the command and its handler.
use crate::{
    layouts::{ResetLayout, SaveLayoutAsDefault, UseLayout},
    settings::{OpenSettings, SettingsPanel},
    workspace::{self, OpenWorkspaces, RecentWorkspaces, Start, Workspace},
};
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::WorkbenchWorldExt;
use bedtime_workbench::{
    Action, ActionAppExt, ActionId, ActionInvocation, ActionRequests, ActionResult,
    ActionsWorldExt, CloseRequested, ErrorDialog, Handler, InMenu, InvocationContext, Invokes,
    KeyChord, Menu, MenuBar, MenuEntries, MenuItem, MenuSeparator, NativeMenuBar, PanelFactory,
    SaveDocument, WorkbenchSurface, draw_menu_bar, panel_drag_source,
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

/// The application menu bar.
#[derive(Component)]
struct ApplicationMenu;

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
            .add_systems(Startup, compose_menus)
            .add_systems(Update, (poll_folder_dialog, fill_recent_menu));
    }
}

fn compose_menus(mut commands: Commands) {
    let root = commands
        .spawn((MenuBar, NativeMenuBar, ApplicationMenu))
        .id();
    let mut menu = |label: &str| {
        commands
            .spawn((
                Menu {
                    label: label.into(),
                },
                InMenu(root),
            ))
            .id()
    };
    // On macOS the first menu becomes the application menu.
    let app = menu("bEd");
    let file = menu("File");
    let view = menu("View");
    let layout = menu("Layout");
    commands.spawn((
        MenuItem::new("Settings…"),
        Invokes::of::<OpenSettings>(),
        InMenu(app),
    ));
    commands.spawn((MenuSeparator, InMenu(app)));
    commands.spawn((MenuItem::action::<Quit>(), InMenu(app)));
    commands.spawn((MenuItem::action::<NewWindow>(), InMenu(file)));
    commands.spawn((MenuSeparator, InMenu(file)));
    commands.spawn((MenuItem::action::<OpenFolder>(), InMenu(file)));
    commands.spawn((
        Menu {
            label: "Open Recent".into(),
        },
        RecentMenu,
        InMenu(file),
    ));
    commands.spawn((MenuItem::action::<CloseFolder>(), InMenu(file)));
    commands.spawn((MenuSeparator, InMenu(file)));
    commands.spawn((MenuItem::action::<SaveDocument>(), InMenu(file)));
    commands.spawn((
        MenuItem::new("Settings"),
        Invokes::of::<OpenSettings>(),
        InMenu(view),
    ));
    commands.spawn((MenuItem::action::<OpenWorkspaces>(), InMenu(view)));
    commands.spawn((MenuItem::action::<UseLayout<0>>(), InMenu(layout)));
    commands.spawn((MenuItem::action::<UseLayout<1>>(), InMenu(layout)));
    commands.spawn((MenuItem::action::<UseLayout<2>>(), InMenu(layout)));
    commands.spawn((MenuItem::action::<UseLayout<3>>(), InMenu(layout)));
    commands.spawn((MenuItem::action::<UseLayout<4>>(), InMenu(layout)));
    commands.spawn((MenuSeparator, InMenu(layout)));
    commands.spawn((MenuItem::action::<SaveLayoutAsDefault>(), InMenu(layout)));
    commands.spawn((MenuItem::action::<ResetLayout>(), InMenu(layout)));
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
        switch(world, Start::Folder(folder.path().to_owned()));
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
    switch(world, Start::Folder(folder));
    Ok(())
}

fn close_folder(world: &mut World, _: ActionInvocation) -> ActionResult {
    switch(world, Start::Default);
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

/// Switch this window to `start`'s workspace, or explain why not.
pub fn switch(world: &mut World, start: Start) {
    if let Err(error) = workspace::switch(world, start) {
        world.workbench_mut().request_error_dialog(ErrorDialog {
            title: "Open Folder".into(),
            message: error,
            details: None,
        });
    }
}

fn new_window(_: &mut World, _: ActionInvocation) -> ActionResult {
    std::env::current_exe()
        .and_then(|bed| std::process::Command::new(bed).spawn())
        .map(|_| ())
        .map_err(|error| format!("Couldn't start another bEd: {error}"))
}

/// The menu bar, where the platform has no native one, then the tools.
pub fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    if let Ok(root) = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .single(world)
    {
        // Inset like the panels below it.
        let inset = world
            .get::<WorkbenchSurface>(context.surface)
            .map_or(0.0, |surface| surface.panel_gap);
        let result = draw_menu_bar(&mut context.frame(ui), world, root, inset);
        crate::status::notify_failure(world, "Menu", result);
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        ui.visuals_mut().button_frame = false;
        tool::<OpenSettings>(
            ui,
            world,
            context,
            BuiltinIcon::Settings,
            PanelFactory::of::<SettingsPanel>(),
        );
    });
}

/// A titlebar button running command `C`. Dragging it out creates a new panel
/// from `factory` where it's dropped.
fn tool<C: Action>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    icon: BuiltinIcon,
    factory: PanelFactory,
) {
    let command = ActionId::of::<C>();
    let enabled = world
        .actions()
        .available(&command, InvocationContext::Focused);
    let tooltip = match world
        .actions()
        .shortcut_for(&command, InvocationContext::Focused)
    {
        Some(chord) => format!("{} ({})", C::LABEL, chord.label()),
        None => C::LABEL.to_owned(),
    };
    ui.add_enabled_ui(enabled, |ui| {
        let response = context
            .frame(ui)
            .icon_button_sized(icon, 16.0)
            .interact(egui::Sense::click_and_drag());
        context.frame(ui).tooltip(&response, tooltip);
        panel_drag_source(&response, factory);
        if response.clicked() {
            world.resource_mut::<ActionRequests>().invoke::<C>();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::{MenuNode, MenusWorldExt, WorkbenchCorePlugin};

    fn labels(nodes: &[MenuNode]) -> Vec<(String, bool)> {
        nodes
            .iter()
            .filter_map(|node| match node {
                MenuNode::Menu { label, .. } => Some((label.clone(), true)),
                MenuNode::Item { label, enabled, .. } => Some((label.clone(), *enabled)),
                MenuNode::Separator(_) => None,
            })
            .collect()
    }

    fn menu<'a>(nodes: &'a [MenuNode], name: &str) -> &'a [MenuNode] {
        nodes
            .iter()
            .find_map(|node| match node {
                MenuNode::Menu { label, entries, .. } if label == name => Some(&entries[..]),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn open_recent_lists_other_folders_and_items_follow_their_commands() {
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugin,
            crate::settings::BedSettingsPlugin,
            crate::layouts::LayoutsPlugin,
            MenusPlugin,
        ))
        .insert_resource(RecentWorkspaces(vec!["/a".into(), "/b".into()]))
        .insert_resource(Workspace {
            root: "/a".into(),
            default: false,
        });
        app.update();
        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<ApplicationMenu>>()
            .single(world)
            .unwrap();
        let bar = world
            .menus()
            .snapshot(root, InvocationContext::Focused)
            .unwrap();
        assert_eq!(
            labels(&bar.entries),
            [
                ("bEd".into(), true),
                ("File".into(), true),
                ("View".into(), true),
                ("Layout".into(), true)
            ]
        );
        let file = menu(&bar.entries, "File");
        // Save is greyed out: no document is focused.
        assert_eq!(
            labels(file),
            [
                ("New Window".into(), true),
                ("Open Folder…".into(), true),
                ("Open Recent".into(), true),
                ("Close Folder".into(), true),
                ("Save".into(), false)
            ]
        );
        assert_eq!(labels(menu(file, "Open Recent")), [("/b".into(), true)]);

        // Only this folder is recent: nothing else to open.
        world.resource_mut::<RecentWorkspaces>().0 = vec!["/a".into()];
        app.update();
        let world = app.world_mut();
        let bar = world
            .menus()
            .snapshot(root, InvocationContext::Focused)
            .unwrap();
        let recent = menu(menu(&bar.entries, "File"), "Open Recent");
        assert_eq!(labels(recent), [("No Recent Folders".into(), false)]);

        let open_recent = ActionId::of::<OpenRecent>();
        assert!(
            !world
                .actions()
                .available(&open_recent, InvocationContext::Focused)
        );

        // In the default workspace there's no folder to close, and every
        // recent folder can be opened.
        *world.resource_mut::<Workspace>() = Workspace {
            root: "/a".into(),
            default: true,
        };
        // An App's workspace never changes; refill as a new App would.
        world.resource_mut::<RecentWorkspaces>().set_changed();
        app.update();
        let world = app.world_mut();
        let bar = world
            .menus()
            .snapshot(root, InvocationContext::Focused)
            .unwrap();
        let file = menu(&bar.entries, "File");
        assert!(labels(file).contains(&("Close Folder".into(), false)));
        assert_eq!(labels(menu(file, "Open Recent")), [("/a".into(), true)]);
    }
}
