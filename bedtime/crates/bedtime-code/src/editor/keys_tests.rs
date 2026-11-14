//! Keys reach editor commands through the workbench (its claim pass, before
//! panels draw), with the keymap an app chose.
use super::*;
use bedtime_ui::{UiCamera, UiCapture};
use bedtime_workbench::{
    ActionAppExt, ActionsWorldExt, LayoutsWorldExt, PathOptions, Root, WorkbenchCorePlugins,
    WorkbenchWorldExt,
};
use bevy::prelude::*;
use bevy_egui::EguiInput;

/// An app showing a file in an editor, its window focused, with VS Code's
/// keys.
fn app() -> (App, Entity, Entity) {
    let root = std::env::temp_dir().join(format!("bedtime-code-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.rs"), "a\nb\n").unwrap();
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, CodePlugin))
        .add_keymap(crate::keymap::VSCODE)
        .insert_resource(Root(root.clone()));
    let world = app.world_mut();
    let layout = world.layouts_mut().create("Test").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let panel = world
        .workbench_mut()
        .open_path(&root.join("main.rs"), PathOptions::default())
        .unwrap();
    let surface = world
        .spawn((
            Window {
                focused: true,
                ..default()
            },
            UiCapture::default(),
        ))
        .id();
    let camera = world.spawn((UiCamera(surface), EguiInput::default())).id();
    app.update();
    (app, panel, camera)
}

fn press(app: &mut App, camera: Entity, key: egui::Key, modifiers: egui::Modifiers) {
    app.world_mut().get_mut::<EguiInput>(camera).unwrap().events = vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }];
    app.update();
}

fn queued(app: &App, panel: Entity) -> Vec<EditorCommand> {
    app.world()
        .get::<EditorPanel>(panel)
        .unwrap()
        .view
        .queued
        .iter()
        .filter_map(|queued| match queued {
            Queued::Command(command) => Some(*command),
            Queued::Event(_) => None,
        })
        .collect()
}

#[test]
fn keys_reach_the_focused_editors_commands_but_not_from_its_find_field() {
    let (mut app, panel, camera) = app();
    assert_eq!(
        app.world()
            .actions()
            .entity(bedtime_workbench::InvocationContext::Focused),
        Some(panel)
    );
    let set_keyboard = |app: &mut App, has: bool| {
        app.world_mut()
            .get_mut::<EditorPanel>(panel)
            .unwrap()
            .view
            .focused = has;
    };
    let waiting = |app: &mut App| app.world().get::<EguiInput>(camera).unwrap().events.len();

    // The text has the keyboard: Alt+Up (a text key) is Move Line Up,
    // taken before the editor would see it.
    set_keyboard(&mut app, true);
    press(&mut app, camera, egui::Key::ArrowUp, egui::Modifiers::ALT);
    assert_eq!(queued(&app, panel), [EditorCommand::MoveLineUp]);
    assert_eq!(waiting(&mut app), 0);
    press(&mut app, camera, egui::Key::Slash, egui::Modifiers::COMMAND);
    assert_eq!(
        queued(&app, panel),
        [EditorCommand::MoveLineUp, EditorCommand::ToggleLineComment]
    );

    // The Find field has it: the keys stay for the field.
    app.world_mut()
        .get_mut::<EditorPanel>(panel)
        .unwrap()
        .view
        .queued
        .clear();
    set_keyboard(&mut app, false);
    press(&mut app, camera, egui::Key::ArrowUp, egui::Modifiers::ALT);
    assert!(queued(&app, panel).is_empty());
    assert_eq!(waiting(&mut app), 1);
}

#[test]
fn editor_actions_have_no_keys_without_a_keymap() {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, CodePlugin));
    let keys = app.world().resource::<bedtime_workbench::Keybindings>();
    use bedtime_workbench::ActionId;
    for action in [
        ActionId::of::<MoveLineUp>(),
        ActionId::of::<ToggleLineComment>(),
        ActionId::of::<ToggleWordWrap>(),
    ] {
        assert!(keys.keys(&action).is_empty(), "{}", action.as_str());
    }
}
