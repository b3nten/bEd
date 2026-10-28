//! bEd's menus and titlebar toolbar, and the app-level commands they run.
//!
//! Menus and tools only name commands: labels, shortcuts and whether they're
//! enabled all come from the command and its handler.
use crate::{
    settings::{OpenSettings, SettingsPanel},
    workspace::{RecentWorkspaces, Workspace},
};
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::{
    CloseRequested, CommandAppExt, ContextCommand, ContextCommandId, ContextCommandInvocation,
    ContextCommandRequests, ContextCommandResult, ErrorDialog, Handler, InMenu, InvocationContext,
    Invokes, KeyChord, Menu, MenuBar, MenuEntries, MenuItem, MenuSeparator, NativeMenuBar,
    PanelFactory, SaveDocument, command_available, draw_menu_bar, panel_drag_source,
    request_error_dialog, shortcut_for,
};
use bevy::prelude::*;
use std::{future::Future, path::PathBuf, pin::Pin};

pub struct Quit;
impl ContextCommand for Quit {
    const KEY: &'static str = "app.quit";
    const LABEL: &'static str = "Quit bEd";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyQ)];
}

pub struct OpenFolder;
impl ContextCommand for OpenFolder {
    const KEY: &'static str = "app.open_folder";
    const LABEL: &'static str = "Open Folder…";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyO)];
}

/// From an Open Recent item, opens that item's folder; from a key, the most
/// recent folder other than this one.
pub struct OpenRecent;
impl ContextCommand for OpenRecent {
    const KEY: &'static str = "app.open_recent";
    const LABEL: &'static str = "Open Recent";
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
            .add_command_handler::<Quit>(Handler::new(quit))
            .add_command_handler::<OpenFolder>(
                Handler::new(open_folder)
                    .available_when(|world, _| world.non_send::<FolderDialog>().0.is_none()),
            )
            .add_command_handler::<OpenRecent>(
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
    commands.spawn((
        MenuItem::new("Settings…"),
        Invokes::of::<OpenSettings>(),
        InMenu(app),
    ));
    commands.spawn((MenuSeparator, InMenu(app)));
    commands.spawn((MenuItem::command::<Quit>(), InMenu(app)));
    commands.spawn((MenuItem::command::<OpenFolder>(), InMenu(file)));
    commands.spawn((
        Menu {
            label: "Open Recent".into(),
        },
        RecentMenu,
        InMenu(file),
    ));
    commands.spawn((MenuSeparator, InMenu(file)));
    commands.spawn((MenuItem::command::<SaveDocument>(), InMenu(file)));
    commands.spawn((
        MenuItem::new("Settings"),
        Invokes::of::<OpenSettings>(),
        InMenu(view),
    ));
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
    let current = workspace.map(|workspace| workspace.root.clone());
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

fn quit(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
    world.write_message(CloseRequested { window: None });
    Ok(())
}

/// The folder picker, while it's open. AppKit owns the dialog, so its future is
/// polled on the main thread; nothing blocks while it's open.
#[derive(Default)]
struct FolderDialog(Option<Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>>);

fn open_folder(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
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
        launch(world, folder.path().to_owned());
    }
}

fn open_recent(world: &mut World, invocation: ContextCommandInvocation) -> ContextCommandResult {
    let chosen = invocation
        .source
        .and_then(|source| world.get::<RecentPath>(source))
        .map(|path| path.0.clone());
    let folder = chosen
        .or_else(|| other_recent(world))
        .ok_or("No recent folder")?;
    launch(world, folder);
    Ok(())
}

/// The most recent folder other than this instance's.
fn other_recent(world: &World) -> Option<PathBuf> {
    let current = world.get_resource::<Workspace>().map(|w| &w.root);
    let recent = world.resource::<RecentWorkspaces>();
    recent.0.iter().find(|path| Some(*path) != current).cloned()
}

/// Open `folder` in a new bEd instance (one workspace per process). The folder
/// already open here is left alone.
fn launch(world: &mut World, folder: PathBuf) {
    let current = world.get_resource::<Workspace>().map(|w| w.root.clone());
    if Workspace::from_path(folder.clone()).map(|w| w.root) == current && current.is_some() {
        return;
    }
    let result = std::env::current_exe()
        .and_then(|bed| std::process::Command::new(bed).arg(&folder).spawn())
        .map_err(|error| error.to_string());
    if let Err(error) = result {
        request_error_dialog(
            world,
            ErrorDialog {
                title: "Open Folder".into(),
                message: format!("Couldn't open {}.", folder.display()),
                details: Some(error),
            },
        );
    }
}

/// The menu bar, where the platform has no native one, then the tools.
pub fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    if let Ok(root) = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .single(world)
        && let Err(error) = draw_menu_bar(&mut context.frame(ui), world, root)
    {
        warn!("Menu: {error}");
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
fn tool<C: ContextCommand>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    icon: BuiltinIcon,
    factory: PanelFactory,
) {
    let command = ContextCommandId::of::<C>();
    let enabled = command_available(world, &command, InvocationContext::Focused);
    let tooltip = match shortcut_for(world, &command, InvocationContext::Focused) {
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
            world.resource_mut::<ContextCommandRequests>().invoke::<C>();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::{MenuNode, WorkbenchCorePlugin, menu_snapshot};

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
            crate::settings::SettingsPlugin,
            MenusPlugin,
        ))
        .insert_resource(RecentWorkspaces(vec!["/a".into(), "/b".into()]))
        .insert_resource(Workspace { root: "/a".into() });
        app.update();
        let world = app.world_mut();
        let root = world
            .query_filtered::<Entity, With<ApplicationMenu>>()
            .single(world)
            .unwrap();
        let bar = menu_snapshot(world, root, InvocationContext::Focused).unwrap();
        assert_eq!(
            labels(&bar.entries),
            [
                ("bEd".into(), true),
                ("File".into(), true),
                ("View".into(), true)
            ]
        );
        let file = menu(&bar.entries, "File");
        // Save is greyed out: no document is focused.
        assert_eq!(
            labels(file),
            [
                ("Open Folder…".into(), true),
                ("Open Recent".into(), true),
                ("Save".into(), false)
            ]
        );
        assert_eq!(labels(menu(file, "Open Recent")), [("/b".into(), true)]);

        // Only this folder is recent: nothing else to open.
        world.resource_mut::<RecentWorkspaces>().0 = vec!["/a".into()];
        app.update();
        let world = app.world_mut();
        let bar = menu_snapshot(world, root, InvocationContext::Focused).unwrap();
        let recent = menu(menu(&bar.entries, "File"), "Open Recent");
        assert_eq!(labels(recent), [("No Recent Folders".into(), false)]);
        let open_recent = ContextCommandId::of::<OpenRecent>();
        assert!(!command_available(
            world,
            &open_recent,
            InvocationContext::Focused
        ));
    }
}
