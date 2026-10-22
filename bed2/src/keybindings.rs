//! Fixed application shortcuts until the keybinding settings UI is introduced.
use bedtime_workbench2::*;
use bevy::prelude::*;

pub fn install(app: &mut App) {
    let world = app.world_mut();
    world.spawn(Binding::new::<SaveDocument>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyS,
    )));
    world.spawn(Binding::new::<crate::menus::OpenFolder>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyO,
    )));
    world.spawn(Binding::new::<crate::menus::Quit>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyQ,
    )));
}
