//! Application-owned creation discovery: concrete panels, no workbench registry.
use bedtime_ui::egui;
use bedtime_workbench::*;

#[cfg(test)]
pub struct NewCode;
#[cfg(test)]
impl ContextCommand for NewCode {
    const KEY: &'static str = "bed.document.new";
    const LABEL: &'static str = "New document";
}

pub struct OpenFolder;
impl ContextCommand for OpenFolder {
    const KEY: &'static str = "bed.folder.open";
    const LABEL: &'static str = "Open Folder…";
}

macro_rules! edit_command {
    ($name:ident, $key:literal, $label:literal) => {
        pub struct $name;
        impl ContextCommand for $name {
            const KEY: &'static str = $key;
            const LABEL: &'static str = $label;
        }
    };
}

edit_command!(UndoEdit, "bed.edit.undo", "Undo");
edit_command!(RedoEdit, "bed.edit.redo", "Redo");
edit_command!(Cut, "bed.edit.cut", "Cut");
edit_command!(Copy, "bed.edit.copy", "Copy");
edit_command!(Paste, "bed.edit.paste", "Paste");

/// Contributions are installed by features; persistence never consults this list.
#[derive(Resource, Default)]
struct PanelMenus(Vec<PanelMenu>);

#[derive(Clone)]
pub(crate) struct PanelMenu {
    pub key: &'static str,
    pub factory: PanelFactory,
    command: ContextCommandId,
    order: Option<u32>,
    pub(crate) available: fn(&World) -> bool,
}

struct OpenPanel<P>(std::marker::PhantomData<P>);
impl<P: Panel + Send + Sync + 'static> ContextCommand for OpenPanel<P> {
    const KEY: &'static str = P::KEY;
    const LABEL: &'static str = P::LABEL;
}

/// Register a feature's creation action and optional View entry once at setup.
/// Menu composition runs after plugin setup and preserves explicit contribution order.
pub(crate) fn register_panel<P: Panel + Send + Sync + 'static>(
    app: &mut App,
    order: Option<u32>,
    factory: PanelFactory,
    available: fn(&World) -> bool,
) {
    app.init_resource::<PanelMenus>();
    let world = app.world_mut();
    register_context_command::<OpenPanel<P>>(world).unwrap();
    install_global_handler::<OpenPanel<P>, _>(
        world,
        |_invocation: In<ContextCommandInvocation>, world: &mut World| {
            let layout = active_layout(world).ok_or("Select a layout first")?;
            open_panel(world, layout, P::KEY).map(|_| ())
        },
    )
    .unwrap();
    world.resource_mut::<PanelMenus>().0.push(PanelMenu {
        key: P::KEY,
        factory,
        command: ContextCommandId::of::<OpenPanel<P>>(),
        order,
        available,
    });
}

pub(crate) fn panel_menu(world: &World, key: &str) -> Option<PanelMenu> {
    world
        .get_resource::<PanelMenus>()?
        .0
        .iter()
        .find(|entry| entry.key == key)
        .cloned()
}

fn view_panels(world: &World) -> Vec<PanelMenu> {
    let mut entries = world
        .get_resource::<PanelMenus>()
        .map(|r| r.0.clone())
        .unwrap_or_default();
    entries.retain(|entry| entry.order.is_some());
    entries.sort_by_key(|entry| (entry.order, entry.key));
    entries
}

pub struct Quit;
impl ContextCommand for Quit {
    const KEY: &'static str = "bed.application.quit";
    const LABEL: &'static str = "Quit";
}

#[derive(Resource, Default)]
struct CommandResults(bevy::ecs::message::MessageCursor<ContextCommandFinished>);
fn report_command_failures(world: &mut World) {
    world.resource_scope(|world, mut results: Mut<CommandResults>| {
        let failures: Vec<_> = results
            .0
            .read(world.resource::<Messages<ContextCommandFinished>>())
            .filter_map(|finished| finished.result.as_ref().err().cloned())
            .collect();
        for error in failures {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Application command".into(),
                    message: "The requested operation couldn't be completed.".into(),
                    details: Some(error),
                },
            );
        }
    });
}

#[derive(Resource, Default)]
struct NativeErrors(bevy::ecs::message::MessageCursor<NativeError>);
fn report_native_failures(world: &mut World) {
    if !world.contains_resource::<Messages<NativeError>>() {
        return;
    }
    world.resource_scope(|world, mut errors: Mut<NativeErrors>| {
        let failures: Vec<_> = errors.0.read(world.resource::<Messages<NativeError>>()).map(|error| error.0.clone()).collect();
        for error in failures {
            request_error_dialog(world, ErrorDialog { title: "Application menu".into(), message: "The native menu couldn't be installed. An in-window menu remains available.".into(), details: Some(error) });
        }
    });
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation::AutomationResult {
    use serde_json::json;
    fn node(entry: &MenuNode) -> serde_json::Value {
        match entry {
            MenuNode::Menu { label, entries, .. } => {
                json!({ "type": "menu", "label": label, "entries": entries.iter().map(node).collect::<Vec<_>>() })
            }
            MenuNode::Item {
                label,
                enabled,
                target,
                shortcut,
                ..
            } => {
                json!({ "type": "item", "label": label, "enabled": enabled, "command": target.as_ref().map(|command| command.as_str()), "shortcut": shortcut.map(|chord| format!("{chord:?}")) })
            }
            MenuNode::Separator(_) => json!({ "type": "separator" }),
        }
    }
    let root = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .single(world)
        .map_err(|error| error.to_string())?;
    let menu = menu_snapshot(world, root, InvocationContext::Focused)?;
    Ok(
        json!({ "native_installed": native_menu_installed(world, root), "entries": menu.entries.iter().map(node).collect::<Vec<_>>(), "view_choices": view_panels(world).iter().map(|entry| entry.factory.label).collect::<Vec<_>>() }),
    )
}

pub fn install(app: &mut App) {
    #[cfg(feature = "automation")]
    {
        use bedtime_automation::AutomationAppExt;
        app.register_automation_command("app.inspect_menus", inspect)
            .register_automation_command("app.activate_menu", activate_menu);
    }
    app.init_resource::<crate::layouts::LayoutDefaults>()
        .init_resource::<CommandResults>()
        .init_resource::<NativeErrors>()
        .init_resource::<PendingEdits>()
        .init_non_send::<FolderDialog>()
        .add_systems(
            Update,
            (
                poll_folder_dialog,
                report_command_failures,
                report_native_failures,
            ),
        )
        .add_systems(
            PreUpdate,
            sync_menu_availability
                .after(SessionSystems::Prepare)
                .before(NativeSystems::Poll),
        )
        .add_systems(
            PreUpdate,
            inject_edit_events
                .after(bevy_egui::EguiPreUpdateSet::ProcessInput)
                .before(bevy_egui::EguiPreUpdateSet::BeginPass)
                .before(bedtime_ui::UiSystems::Draw),
        );
    let world = app.world_mut();
    register_context_command::<OpenFolder>(world).unwrap();
    register_context_command::<Quit>(world).unwrap();
    register_context_command::<UndoEdit>(world).unwrap();
    register_context_command::<RedoEdit>(world).unwrap();
    register_context_command::<Cut>(world).unwrap();
    register_context_command::<Copy>(world).unwrap();
    register_context_command::<Paste>(world).unwrap();
    install_global_handler::<OpenFolder, _>(world, open_folder).unwrap();
    install_global_handler::<Quit, _>(world, quit).unwrap();
    install_global_handler::<UndoEdit, _>(
        world,
        |i: In<ContextCommandInvocation>, w: &mut World| edit(w, &i, EditAction::Undo),
    )
    .unwrap();
    install_global_handler::<RedoEdit, _>(
        world,
        |i: In<ContextCommandInvocation>, w: &mut World| edit(w, &i, EditAction::Redo),
    )
    .unwrap();
    install_global_handler::<Cut, _>(world, |i: In<ContextCommandInvocation>, w: &mut World| {
        edit(w, &i, EditAction::Cut)
    })
    .unwrap();
    install_global_handler::<Copy, _>(world, |i: In<ContextCommandInvocation>, w: &mut World| {
        edit(w, &i, EditAction::Copy)
    })
    .unwrap();
    install_global_handler::<Paste, _>(world, |i: In<ContextCommandInvocation>, w: &mut World| {
        edit(w, &i, EditAction::Paste)
    })
    .unwrap();
    // These fixed editing chords also supply native accelerator labels. They
    // aren't configurable application shortcuts: text widgets keep first refusal
    // in the general shortcut collector, and native clicks use the same actions.
    world.spawn(Binding::new::<UndoEdit>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyZ,
    )));
    world.spawn(Binding::new::<RedoEdit>(KeyChord::new(
        Modifiers::PRIMARY | Modifiers::SHIFT,
        KeyCode::KeyZ,
    )));
    world.spawn(Binding::new::<Cut>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyX,
    )));
    world.spawn(Binding::new::<Copy>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyC,
    )));
    world.spawn(Binding::new::<Paste>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyV,
    )));
    compose_menu(world);
}

#[derive(Component)]
struct ApplicationMenu;

/// UI renderer for a separate fallback menu row on platforms without native
/// menus. This uses the very same abstract root; the native presenter suppresses
/// the row when available. Do not attach it to the root itself (UiView on a menu
/// root is its trailing application-controls renderer).
pub fn draw_application_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
) {
    let root = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .single(world)
        .expect("Application menu root");
    draw_menu_bar(&mut context.frame(ui), world, root).expect("Valid application menu");
}

fn compose_menu(world: &mut World) -> Entity {
    let root = world.spawn((MenuBar, NativeMenuBar, ApplicationMenu)).id();
    for (label, commands) in [
        (
            "bEd",
            vec![Some((ContextCommandId::of::<Quit>(), "Quit bEd"))],
        ),
        (
            "File",
            vec![
                Some((ContextCommandId::of::<OpenFolder>(), "Open Folder…")),
                Some((ContextCommandId::of::<SaveDocument>(), "Save")),
            ],
        ),
        (
            "Edit",
            vec![
                Some((ContextCommandId::of::<UndoEdit>(), "Undo")),
                Some((ContextCommandId::of::<RedoEdit>(), "Redo")),
                None,
                Some((ContextCommandId::of::<Cut>(), "Cut")),
                Some((ContextCommandId::of::<Copy>(), "Copy")),
                Some((ContextCommandId::of::<Paste>(), "Paste")),
            ],
        ),
    ] {
        let menu = world
            .spawn((
                Menu {
                    label: label.into(),
                },
                InMenu(root),
            ))
            .id();
        for command in commands {
            if let Some((command, label)) = command {
                let global = command == ContextCommandId::of::<OpenFolder>()
                    || command == ContextCommandId::of::<Quit>();
                let mut item = world.spawn((MenuItem::new(label), Invokes(command), InMenu(menu)));
                if global {
                    item.insert(InvocationContext::Global);
                }
            } else {
                world.spawn((MenuSeparator, InMenu(menu)));
            }
        }
    }
    let view = world
        .spawn((
            Menu {
                label: "View".into(),
            },
            InMenu(root),
        ))
        .id();
    for entry in view_panels(world) {
        world.spawn((
            MenuItem::new(entry.factory.label),
            Invokes(entry.command),
            InMenu(view),
        ));
    }
    crate::layouts::install_menu(world, view);
    root
}

// The future is created and polled on the main thread: AppKit owns its dialog.
// No thread blocks waiting for a selection, and cancel has no world effects.
#[derive(Default)]
struct FolderDialog(
    Option<std::pin::Pin<Box<dyn std::future::Future<Output = Option<rfd::FileHandle>>>>>,
);

#[cfg(feature = "automation")]
fn activate_menu(
    world: &mut World,
    arguments: serde_json::Value,
) -> bedtime_automation::AutomationResult {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_array())
        .ok_or("path must be menu labels followed by an item")?;
    if !(2..=8).contains(&path.len()) {
        return Err("Menu path must contain 2–8 labels".into());
    }
    let labels: Vec<_> = path
        .iter()
        .map(|v| v.as_str().ok_or("Menu labels must be strings"))
        .collect::<Result<_, _>>()?;
    let root = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .single(world)
        .map_err(|e| e.to_string())?;
    let snapshot = menu_snapshot(world, root, InvocationContext::Focused)?;
    fn find(entries: &[MenuNode], labels: &[&str]) -> Option<(Entity, ContextCommandId)> {
        let (name, rest) = labels.split_first()?;
        entries.iter().find_map(|node| match node {
            MenuNode::Menu { label, entries, .. } if label == name && !rest.is_empty() => {
                find(entries, rest)
            }
            MenuNode::Item {
                entity,
                label,
                target: Some(target),
                ..
            } if label == name && rest.is_empty() => Some((*entity, target.clone())),
            _ => None,
        })
    }
    let (entity, target) =
        find(&snapshot.entries, &labels).ok_or("Menu action is missing or unavailable")?;
    let request = snapshot
        .request(entity, target)
        .ok_or("Menu action is missing or unavailable")?;
    world
        .resource_mut::<ContextCommandRequests>()
        .submit(request);
    Ok(serde_json::json!({"admitted": true}))
}

fn open_folder(_: In<ContextCommandInvocation>, world: &mut World) -> ContextCommandResult {
    if !crate::close::accepts_user_actions(world) {
        return Ok(());
    }
    let mut dialog = world.non_send_mut::<FolderDialog>();
    if dialog.0.is_none() {
        dialog.0 = Some(Box::pin(
            rfd::AsyncFileDialog::new()
                .set_title("Open Folder")
                .pick_folder(),
        ));
    }
    Ok(())
}

fn poll_folder_dialog(world: &mut World) {
    if !crate::close::accepts_user_actions(world) {
        return;
    }
    let selected = {
        let mut dialog = world.non_send_mut::<FolderDialog>();
        let Some(future) = dialog.0.as_mut() else {
            return;
        };
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Pending => return,
            std::task::Poll::Ready(selected) => {
                dialog.0 = None;
                selected
            }
        }
    };
    if let Some(folder) = selected {
        if let Err(error) = crate::opening::open_folder(world, folder.path().to_path_buf()) {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Open Folder".into(),
                    message: "Couldn't open this folder.".into(),
                    details: Some(error),
                },
            );
        }
    }
}

fn quit(_: In<ContextCommandInvocation>, world: &mut World) -> ContextCommandResult {
    world.write_message(CloseRequested { window: None });
    Ok(())
}

#[derive(Clone, Copy)]
enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
}
#[derive(Resource, Default)]
struct PendingEdits(Vec<(Entity, egui::Id, EditAction)>);

/// Use the same clipboard route as the native Edit menu, targeting this canvas.
pub fn editor_context_menu(ui: &mut egui::Ui, world: &mut World, panel: Entity) {
    let Some(document) = world.get::<DocumentView>(panel).map(DocumentView::document) else {
        return;
    };
    let Some(editor) = world.get::<bedtime_editor::workbench::WorkbenchEditor>(panel) else {
        return;
    };
    let selected = !editor.view.selection().range().is_empty();
    let writable = !editor.view.settings.read_only
        && inspect_document(world, document).is_ok_and(|info| !info.has_drafts);
    for (label, command, enabled) in [
        ("Cut", ContextCommandId::of::<Cut>(), selected && writable),
        ("Copy", ContextCommandId::of::<Copy>(), selected),
        ("Paste", ContextCommandId::of::<Paste>(), writable),
    ] {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            ui.close();
            ui.memory_mut(|memory| {
                memory.request_focus(egui::Id::new(("bedtime.editor", panel, document)));
            });
            world
                .resource_mut::<ContextCommandRequests>()
                .submit(ContextCommandRequest {
                    command,
                    source: None,
                    context: InvocationContext::Panel(panel),
                });
        }
    }
}

pub fn has_pending_edits(world: &World) -> bool {
    world
        .get_resource::<PendingEdits>()
        .is_some_and(|edits| !edits.0.is_empty())
}

fn focused_ui(world: &mut World) -> Option<(Entity, egui::Context, egui::Id)> {
    world
        .query_filtered::<(Entity, &mut bevy_egui::EguiContext), With<bedtime_ui::UiCamera>>()
        .iter_mut(world)
        .find_map(|(camera, mut context)| {
            let context = context.get_mut().clone();
            let focused = context.memory(|memory| memory.focused())?;
            Some((camera, context, focused))
        })
}

fn edit(
    world: &mut World,
    invocation: &ContextCommandInvocation,
    action: EditAction,
) -> ContextCommandResult {
    if !crate::close::accepts_user_actions(world) {
        return Ok(());
    }
    let Some((camera, context, focused)) = focused_ui(world) else {
        return Ok(());
    };
    // The canonical editor has shared document history. Ordinary egui fields
    // (including Find/Replace) must instead receive their own widget undo event.
    if matches!(action, EditAction::Undo | EditAction::Redo)
        && invocation.panel.is_some_and(|panel| {
            bedtime_editor::workbench::editor_has_focus(world, panel, &context)
        })
    {
        if world
            .get::<bedtime_editor::workbench::WorkbenchEditor>(invocation.panel.unwrap())
            .is_some_and(|editor| editor.view.settings.read_only)
        {
            return Ok(());
        }
        world
            .resource_mut::<ContextCommandRequests>()
            .submit(ContextCommandRequest {
                command: match action {
                    EditAction::Undo => ContextCommandId::of::<UndoDocument>(),
                    _ => ContextCommandId::of::<RedoDocument>(),
                },
                source: invocation.source,
                context: InvocationContext::Panel(invocation.panel.unwrap()),
            });
    } else if !matches!(action, EditAction::Undo | EditAction::Redo) || context.text_edit_focused()
    {
        // A terminal canvas has clipboard actions, but no document/widget undo.
        world
            .resource_mut::<PendingEdits>()
            .0
            .push((camera, focused, action));
    }
    Ok(())
}

fn inject_edit_events(world: &mut World) {
    if !crate::close::accepts_user_actions(world) {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<PendingEdits>().0);
    for (camera, focused, action) in pending {
        let still_focused =
            world
                .get_mut::<bevy_egui::EguiContext>(camera)
                .is_some_and(|mut context| {
                    context.get_mut().memory(|memory| memory.focused()) == Some(focused)
                });
        if !still_focused {
            continue;
        }
        let event = match action {
            EditAction::Copy => egui::Event::Copy,
            EditAction::Cut => egui::Event::Cut,
            EditAction::Paste => match world.resource_mut::<Clipboard>().read_text_if_present() {
                Ok(Some(text)) => egui::Event::Paste(text),
                Ok(None) => continue,
                Err(error) => {
                    request_error_dialog(
                        world,
                        ErrorDialog {
                            title: "Paste".into(),
                            message: "Couldn't read the clipboard.".into(),
                            details: Some(error.to_string()),
                        },
                    );
                    continue;
                }
            },
            EditAction::Undo | EditAction::Redo => egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    command: true,
                    mac_cmd: cfg!(target_os = "macos"),
                    ctrl: !cfg!(target_os = "macos"),
                    shift: matches!(action, EditAction::Redo),
                    ..Default::default()
                },
            },
        };
        if let Some(mut input) = world.get_mut::<bevy_egui::EguiInput>(camera) {
            input.events.push(event);
        }
    }
}

fn sync_menu_availability(world: &mut World) {
    let focus = focused_ui(world);
    let panel = active_layout(world).and_then(|layout| Workbench::new(world).focused_panel(layout));
    let document = panel
        .and_then(|panel| world.get::<DocumentView>(panel))
        .and_then(|view| inspect_document(world, view.document()).ok());
    let canonical_focus = panel
        .zip(focus.as_ref())
        .is_some_and(|(panel, (_, context, _))| {
            bedtime_editor::workbench::editor_has_focus(world, panel, context)
        });
    let widget_history = focus
        .as_ref()
        .is_some_and(|(_, context, _)| context.text_edit_focused());
    let read_only = panel
        .and_then(|panel| world.get::<bedtime_editor::workbench::WorkbenchEditor>(panel))
        .is_some_and(|editor| editor.view.settings.read_only);
    let accepting_actions = crate::close::accepts_user_actions(world);
    let layout_ready = crate::storage::ready(world);
    let has_layout = active_layout(world).is_some();
    let panel_availability: Vec<_> = world
        .get_resource::<PanelMenus>()
        .map(|r| {
            r.0.iter()
                .map(|entry| (entry.command.clone(), (entry.available)(world)))
                .collect()
        })
        .unwrap_or_default();
    let enabled = |command: &ContextCommandId| {
        if !accepting_actions {
            return false;
        }
        if command.as_str().starts_with("bed.layout.") {
            return layout_ready;
        }
        if let Some((_, available)) = panel_availability.iter().find(|(id, _)| *id == *command) {
            return has_layout && *available;
        }
        if *command == ContextCommandId::of::<SaveDocument>() {
            return document.as_ref().is_some_and(|info| {
                info.has_save_binding
                    && !info.saving
                    && !info.has_drafts
                    && !info.conflict
                    && !info.requires_source_refresh
            });
        }
        if *command == ContextCommandId::of::<UndoEdit>() {
            return if canonical_focus {
                document
                    .as_ref()
                    .is_some_and(|info| info.can_undo && !info.has_drafts && !read_only)
            } else {
                widget_history
            };
        }
        if *command == ContextCommandId::of::<RedoEdit>() {
            return if canonical_focus {
                document
                    .as_ref()
                    .is_some_and(|info| info.can_redo && !info.has_drafts && !read_only)
            } else {
                widget_history
            };
        }
        if [
            ContextCommandId::of::<Cut>(),
            ContextCommandId::of::<Copy>(),
            ContextCommandId::of::<Paste>(),
        ]
        .contains(command)
        {
            return focus.is_some();
        }
        true
    };
    // Availability belongs to this application's menu tree, including nested
    // Layout entries. Other menu owners keep their own enablement policy.
    let mut parents: Vec<_> = world
        .query_filtered::<Entity, With<ApplicationMenu>>()
        .iter(world)
        .collect();
    let mut items = Vec::new();
    while let Some(parent) = parents.pop() {
        let children = world
            .get::<MenuEntries>(parent)
            .map(|entries| entries.entities())
            .unwrap_or_default();
        for &child in children {
            if world.get::<Menu>(child).is_some() {
                parents.push(child);
            }
            if world.get::<MenuItem>(child).is_some()
                && let Some(invokes) = world.get::<Invokes>(child)
            {
                items.push((child, enabled(&invokes.0)));
            }
        }
    }
    for (entity, available) in items {
        world.get_mut::<MenuItem>(entity).unwrap().enabled = available;
    }
}

use bevy::prelude::*;

pub fn open_panel(world: &mut World, layout: Entity, key: &str) -> Result<Entity, String> {
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before opening a panel".into());
    }
    let entry =
        panel_menu(world, key).ok_or_else(|| format!("Unknown application panel: {key}"))?;
    if !(entry.available)(world) {
        return Err("Project root or settings are still loading or need repair".into());
    }
    let area = Workbench::for_entity(world, layout)?.destination_area(layout)?;
    Ok((entry.factory.create)(world, area))
}
pub fn open_or_focus(world: &mut World, layout: Entity, key: &str) -> Result<Entity, String> {
    let panels = Workbench::for_entity(world, layout)?.panels(layout);
    let panel = match panels.into_iter().find(|&panel| {
        world
            .get::<PanelInstance>(panel)
            .is_some_and(|p| p.key == key)
    }) {
        Some(panel) => panel,
        None => open_panel(world, layout, key)?,
    };
    Workbench::for_entity(world, layout)?.focus_panel(panel)?;
    Ok(panel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::{PanelInstance, WorkbenchCorePlugin};

    fn menu_app() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .add_message::<CloseRequested>()
            .insert_resource(Clipboard::memory());
        register_document_handlers(app.world_mut()).unwrap();
        crate::git::register_menu(&mut app);
        crate::files::register_menu(&mut app);
        crate::code::register_menu(&mut app);
        crate::terminal::register_menu(&mut app);
        crate::mascots::register_menu(&mut app);
        crate::lsp_panel::register_menu(&mut app);
        crate::settings::register_menu(&mut app);
        install(&mut app);
        app
    }

    #[test]
    fn application_availability_does_not_mutate_other_menu_owners() {
        let mut app = menu_app();
        let other = app.world_mut().spawn(MenuBar).id();
        let item = app
            .world_mut()
            .spawn((
                MenuItem {
                    label: "Owned elsewhere".into(),
                    enabled: false,
                },
                Invokes::of::<OpenFolder>(),
                InMenu(other),
            ))
            .id();
        sync_menu_availability(app.world_mut());
        assert!(!app.world().get::<MenuItem>(item).unwrap().enabled);
    }

    #[test]
    fn native_menu_has_constrained_file_edit_and_auxiliary_view_actions_even_without_workspace() {
        let mut app = menu_app();
        sync_menu_availability(app.world_mut());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<ApplicationMenu>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<NativeMenuBar>(root).is_some());
        let snapshot = menu_snapshot(app.world_mut(), root, InvocationContext::Focused).unwrap();
        let groups: Vec<_> = snapshot
            .entries
            .iter()
            .map(|node| {
                let MenuNode::Menu { label, entries, .. } = node else {
                    panic!("Expected group")
                };
                let labels: Vec<_> = entries
                    .iter()
                    .map(|entry| match entry {
                        MenuNode::Item { label, .. } => label.as_str(),
                        MenuNode::Separator(_) => "---",
                        MenuNode::Menu { label, .. } => label.as_str(),
                    })
                    .collect();
                (label.as_str(), labels)
            })
            .collect();
        assert_eq!(
            groups,
            vec![
                ("bEd", vec!["Quit bEd"]),
                ("File", vec!["Open Folder…", "Save"]),
                ("Edit", vec!["Undo", "Redo", "---", "Cut", "Copy", "Paste"]),
                (
                    "View",
                    vec![
                        "Git",
                        "Files",
                        "Structure",
                        "Terminal",
                        "Bed",
                        "Language Servers",
                        "Settings",
                        "Layout"
                    ]
                )
            ]
        );
        let MenuNode::Menu { entries, .. } = &snapshot.entries[1] else {
            unreachable!()
        };
        assert!(
            matches!(&entries[0], MenuNode::Item { enabled: true, target: Some(id), context: InvocationContext::Global, .. } if *id == ContextCommandId::of::<OpenFolder>())
        );
        assert!(
            matches!(&entries[1], MenuNode::Item { enabled: false, target: Some(id), .. } if *id == ContextCommandId::of::<SaveDocument>())
        );
        assert!(command_available(
            app.world_mut(),
            ContextCommandId::of::<OpenFolder>(),
            InvocationContext::Global
        ));
        assert!(!command_available(
            app.world_mut(),
            ContextCommandId::of::<NewCode>(),
            InvocationContext::Global
        ));
    }

    #[test]
    fn native_view_action_composes_its_concrete_panel_in_the_selected_workspace() {
        let mut app = menu_app();
        let _workspace = crate::test_layout(app.world_mut());
        sync_menu_availability(app.world_mut());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<ApplicationMenu>>()
            .single(app.world())
            .unwrap();
        let snapshot = menu_snapshot(app.world_mut(), root, InvocationContext::Focused).unwrap();
        let MenuNode::Menu { entries, .. } = &snapshot.entries[3] else {
            panic!("View group")
        };
        let item = entries
            .iter()
            .find_map(|entry| match entry {
                MenuNode::Item { entity, label, .. } if label == "Settings" => Some(*entity),
                _ => None,
            })
            .unwrap();
        let request = snapshot
            .request(
                item,
                ContextCommandId::of::<OpenPanel<crate::settings::SettingsPanel>>(),
            )
            .unwrap();
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .submit(request);
        app.update();
        let panel = app
            .world_mut()
            .query_filtered::<Entity, With<crate::settings::SettingsPanel>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<InArea>(panel).is_some());
    }

    #[test]
    fn native_actions_are_disabled_and_rejected_while_quitting() {
        let mut app = menu_app();
        app.add_plugins(crate::close::ClosePlugin)
            .add_message::<AppExit>();
        crate::test_layout(app.world_mut());
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(!crate::close::accepts_user_actions(app.world()));
        sync_menu_availability(app.world_mut());
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<ApplicationMenu>>()
            .single(app.world())
            .unwrap();
        let snapshot = menu_snapshot(app.world_mut(), root, InvocationContext::Focused).unwrap();
        for group in snapshot.entries {
            let MenuNode::Menu { entries, .. } = group else {
                panic!("Menu group")
            };
            assert!(
                entries
                    .iter()
                    .all(|entry| !matches!(entry, MenuNode::Item { enabled: true, .. }))
            );
        }
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<OpenFolder>();
        app.update();
        assert!(app.world().non_send::<FolderDialog>().0.is_none());
    }

    #[test]
    fn cancelling_folder_chooser_changes_no_workspace_or_root() {
        let mut app = menu_app();
        let _workspace = crate::test_layout(app.world_mut());
        let root = std::path::PathBuf::from("/original");
        app.world_mut()
            .insert_resource(crate::files::ProjectRoot(root.clone()));
        app.world_mut().non_send_mut::<FolderDialog>().0 = Some(Box::pin(std::future::ready(None)));
        poll_folder_dialog(app.world_mut());
        assert!(app.world().non_send::<FolderDialog>().0.is_none());
        assert_eq!(
            app.world()
                .get_resource::<crate::files::ProjectRoot>()
                .unwrap()
                .0,
            root
        );
        assert_eq!(
            app.world_mut().query::<&Layout>().iter(app.world()).count(),
            1
        );
    }

    #[test]
    fn edit_commands_feed_focused_egui_widget_and_drop_stale_focus() {
        let mut app = menu_app();
        let mut upstream = bevy_egui::EguiContext::default();
        let context = upstream.get_mut().clone();
        let id = egui::Id::new("native-edit-field");
        let camera = app
            .world_mut()
            .spawn((
                bedtime_ui::UiCamera(Entity::PLACEHOLDER),
                upstream,
                bevy_egui::EguiInput::default(),
            ))
            .id();
        let mut text = String::new();
        context.memory_mut(|memory| memory.request_focus(id));
        let draw = |text: &mut String, input: egui::RawInput| {
            let mut output = context.run_ui(input, |ui| {
                ui.add(egui::TextEdit::singleline(text).id(id));
            });
            output.textures_delta.clear();
            output
        };
        let _ = draw(&mut text, egui::RawInput::default());
        app.world_mut()
            .resource_mut::<Clipboard>()
            .write_text("from native menu")
            .unwrap();
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<Paste>();
        app.update();
        inject_edit_events(app.world_mut());
        let input = app
            .world_mut()
            .get_mut::<bevy_egui::EguiInput>(camera)
            .unwrap()
            .take();
        let _ = draw(&mut text, input);
        assert_eq!(text, "from native menu");
        let _ = draw(
            &mut text,
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        command: true,
                        mac_cmd: cfg!(target_os = "macos"),
                        ctrl: !cfg!(target_os = "macos"),
                        ..Default::default()
                    },
                }],
                ..Default::default()
            },
        );
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<Copy>();
        app.update();
        inject_edit_events(app.world_mut());
        let input = app
            .world_mut()
            .get_mut::<bevy_egui::EguiInput>(camera)
            .unwrap()
            .take();
        let copied = draw(&mut text, input);
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(value) if value == "from native menu")));
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<Cut>();
        app.update();
        inject_edit_events(app.world_mut());
        let input = app
            .world_mut()
            .get_mut::<bevy_egui::EguiInput>(camera)
            .unwrap()
            .take();
        let cut = draw(&mut text, input);
        assert!(text.is_empty());
        assert!(cut.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(value) if value == "from native menu")));
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<Paste>();
        app.update();
        context.memory_mut(|memory| memory.request_focus(egui::Id::new("different-field")));
        inject_edit_events(app.world_mut());
        assert!(
            app.world()
                .get::<bevy_egui::EguiInput>(camera)
                .unwrap()
                .events
                .is_empty()
        );
    }

    #[test]
    fn native_history_routes_to_canonical_document_handler_not_widget_history() {
        let mut app = menu_app();
        let _workspace = crate::test_layout(app.world_mut());
        let layout = active_layout(app.world()).unwrap();
        let panel = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .create_panel(layout, "Editor")
            .unwrap();
        let document = open_memory_document(
            app.world_mut(),
            bedtime_editor::TextBuffer::from("original"),
            DocumentOptions::loaded("Text"),
        )
        .unwrap();
        bedtime_editor::workbench::bind_text_editor(app.world_mut(), panel, document).unwrap();
        edit_document(
            app.world_mut(),
            document,
            0,
            bedtime_editor::BufferEdit::new(0..0, "changed "),
            EditOptions::new("Edit"),
        )
        .unwrap();
        let mut upstream = bevy_egui::EguiContext::default();
        let context = upstream.get_mut().clone();
        context.memory_mut(|memory| {
            memory.request_focus(egui::Id::new(("bedtime.editor", panel, document.id())))
        });
        app.world_mut().spawn((
            bedtime_ui::UiCamera(Entity::PLACEHOLDER),
            upstream,
            bevy_egui::EguiInput::default(),
        ));
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<UndoEdit>();
        app.update();
        app.update();
        assert_eq!(
            read_document(app.world(), document)
                .unwrap()
                .range_text(0..8),
            "original"
        );
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<RedoEdit>();
        app.update();
        app.update();
        assert_eq!(
            read_document(app.world(), document)
                .unwrap()
                .range_text(0..16),
            "changed original"
        );
        assert!(
            app.world().resource::<PendingEdits>().0.is_empty(),
            "Canonical history never synthesizes a second widget history event"
        );
    }

    #[test]
    fn code_is_not_a_view_creation_choice_or_creation_route() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let _workspace = crate::test_layout(app.world_mut());
        let layout = active_layout(app.world()).unwrap();
        crate::settings::register_menu(&mut app);
        assert!(view_panels(app.world()).iter().all(|entry| entry.key
            != crate::files::FilePanel::KEY
            && entry.factory.label != "Code"));
        assert!(open_panel(app.world_mut(), layout, crate::files::FilePanel::KEY).is_err());
        assert_eq!(
            app.world_mut()
                .query::<&crate::files::FilePanel>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn feature_menu_contributions_create_concrete_panels() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let workspace = crate::test_layout(app.world_mut());
        let layout = Workbench::for_entity(app.world_mut(), workspace)
            .unwrap()
            .active_layout();
        crate::git::register_menu(&mut app);
        crate::files::register_menu(&mut app);
        crate::code::register_menu(&mut app);
        crate::terminal::register_menu(&mut app);
        crate::mascots::register_menu(&mut app);
        crate::lsp_panel::register_menu(&mut app);
        crate::settings::register_menu(&mut app);
        for entry in view_panels(app.world()) {
            let key = entry.key;
            let panel = open_panel(app.world_mut(), layout, key).unwrap();
            let instance = app.world().get::<PanelInstance>(panel).unwrap();
            assert_eq!(instance.key, key);
            assert!(instance.draw.is_some());
        }
        let count = app
            .world_mut()
            .query::<&PanelInstance>()
            .iter(app.world())
            .count();
        assert!(open_panel(app.world_mut(), layout, "unknown.panel").is_err());
        assert_eq!(
            app.world_mut()
                .query::<&PanelInstance>()
                .iter(app.world())
                .count(),
            count
        );
        assert!(open_panel(app.world_mut(), layout, "bed.cube").is_err());
        assert!(open_panel(app.world_mut(), layout, "bed.tasks").is_err());
        assert_eq!(
            app.world_mut()
                .query::<&bedtime_terminal::TerminalPanel>()
                .iter(app.world())
                .count(),
            1
        );
    }
}
