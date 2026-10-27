//! The Keyboard Shortcuts section: every registered command and its keys in
//! the layer being edited. Click a command's keys, then press the new ones.
use super::layer_state;
use bedtime_ui::egui;
use bedtime_workbench::{
    CommandRegistry, ContextCommandId, KeyChord, Keybindings, Modifiers, keys_in,
    persistence::StoreId, set_keys_in,
};
use bevy::prelude::*;

pub(super) fn section(ui: &mut egui::Ui, world: &mut World, layer: StoreId) {
    ui.heading("Keyboard Shortcuts");
    let commands: Vec<_> = world
        .resource::<CommandRegistry>()
        .commands()
        .map(|(command, label)| (command.clone(), label))
        .collect();
    let recording_id = ui.id().with("recording-keys");
    let mut recording: Option<ContextCommandId> = ui.data(|data| data.get_temp(recording_id));
    egui::Grid::new("keyboard-shortcuts")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (command, label) in commands {
                ui.label(label);
                ui.horizontal(|ui| {
                    if recording.as_ref() == Some(&command) {
                        let cancel = ui.button("Press keys… (Esc cancels)").clicked();
                        match pressed_chord(world) {
                            Some(Ok(chord)) => {
                                let _ = set_keys_in(world, &command, layer, Some(vec![chord]));
                                recording = None;
                            }
                            Some(Err(())) => recording = None,
                            None if cancel => recording = None,
                            None => {}
                        }
                    } else {
                        let keys = world.resource::<Keybindings>().keys(&command);
                        let text = if keys.is_empty() {
                            "None".to_owned()
                        } else {
                            keys.iter()
                                .map(|chord| chord.label())
                                .collect::<Vec<_>>()
                                .join(", ")
                        };
                        let response = ui.button(text).on_hover_text("Click to change");
                        if response.clicked() {
                            recording = Some(command.clone());
                        }
                    }
                    let set_here = keys_in(world, &command, layer).is_some();
                    if layer_state(ui, layer, set_here) {
                        let _ = set_keys_in(world, &command, layer, None);
                    }
                    if let Some(note) = shared_with(world, &command) {
                        ui.weak(note);
                    }
                });
                ui.end_row();
            }
        });
    ui.data_mut(|data| match recording {
        Some(command) => {
            data.insert_temp(recording_id, command);
        }
        None => data.remove::<ContextCommandId>(recording_id),
    });
}

/// "Also bound to Find" when another command shares one of these keys. Not an
/// error: which one runs depends on the focused panel.
fn shared_with(world: &World, command: &ContextCommandId) -> Option<String> {
    let bindings = world.resource::<Keybindings>();
    let registry = world.resource::<CommandRegistry>();
    let mut others: Vec<_> = bindings
        .keys(command)
        .iter()
        .flat_map(|&chord| bindings.commands_for(chord))
        .filter(|other| *other != command)
        .filter_map(|other| registry.label(other))
        .collect();
    others.dedup();
    (!others.is_empty()).then(|| format!("Also bound to {}", others.join(", ")))
}

/// A key pressed this frame with its modifiers; `Err` for Escape. The
/// platform's main modifier is recorded as `Primary`, so the binding carries
/// over to other platforms.
fn pressed_chord(world: &World) -> Option<Result<KeyChord, ()>> {
    let input = world.get_resource::<ButtonInput<KeyCode>>()?;
    let key = input
        .get_just_pressed()
        .copied()
        .find(|key| !is_modifier(*key))?;
    if key == KeyCode::Escape {
        return Some(Err(()));
    }
    let held = |left, right| input.any_pressed([left, right]);
    let (primary, other) = if cfg!(target_os = "macos") {
        (
            held(KeyCode::SuperLeft, KeyCode::SuperRight),
            held(KeyCode::ControlLeft, KeyCode::ControlRight),
        )
    } else {
        (
            held(KeyCode::ControlLeft, KeyCode::ControlRight),
            held(KeyCode::SuperLeft, KeyCode::SuperRight),
        )
    };
    let other_flag = if cfg!(target_os = "macos") {
        Modifiers::CONTROL
    } else {
        Modifiers::META
    };
    let mut modifiers = Modifiers::default();
    for (pressed, flag) in [
        (primary, Modifiers::PRIMARY),
        (other, other_flag),
        (held(KeyCode::AltLeft, KeyCode::AltRight), Modifiers::ALT),
        (
            held(KeyCode::ShiftLeft, KeyCode::ShiftRight),
            Modifiers::SHIFT,
        ),
    ] {
        if pressed {
            modifiers = modifiers | flag;
        }
    }
    Some(Ok(KeyChord::new(modifiers, key)))
}

fn is_modifier(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
            | KeyCode::Fn
    )
}
