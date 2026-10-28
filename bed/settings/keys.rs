//! The Keyboard Shortcuts section: every registered command and its keys in
//! the layer being edited. Click a command's keys, then press the new ones.
use super::{CONTROL_WIDTH, columns, overridden_marker, reset_button};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{
    ActionId, ActionRegistry, KeyChord, Keybindings, Modifiers, keys_at, keys_in,
    keys_overridden_above, persistence::StoreId, set_keys_in,
};
use bevy::prelude::*;

pub(super) fn section(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    layer: StoreId,
) {
    let commands: Vec<_> = world
        .resource::<ActionRegistry>()
        .commands()
        .map(|(command, label)| (command.clone(), label))
        .collect();
    let recording_id = ui.id().with("recording-keys");
    let mut recording: Option<ActionId> = ui.data(|data| data.get_temp(recording_id));
    for (command, label) in commands {
        let set_here = keys_in(world, &command, layer).is_some();
        let overridden = keys_overridden_above(world, &command, layer);
        let shared = shared_with(world, &command);
        let mut reset = false;
        columns(
            ui,
            label,
            |ui| overridden_marker(ui, context, overridden),
            |ui| {
                let size = egui::vec2(CONTROL_WIDTH, 0.0);
                if recording.as_ref() == Some(&command) {
                    let cancel = ui
                        .add(egui::Button::new("Press keys…").min_size(size))
                        .on_hover_text("Escape or click to cancel")
                        .clicked();
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
                    // The layer's own view, without the layers above it.
                    let keys = keys_at(world, &command, layer);
                    let text = if keys.is_empty() {
                        egui::RichText::new("Not set").weak()
                    } else {
                        let labels: Vec<_> = keys.iter().map(|chord| chord.label()).collect();
                        egui::RichText::new(labels.join(", "))
                    };
                    let response = ui
                        .add(egui::Button::new(text).min_size(size))
                        .on_hover_text("Click, then press the new keys");
                    if response.clicked() {
                        recording = Some(command.clone());
                    }
                }
            },
            |ui| {
                reset = reset_button(ui, context, layer, set_here);
                if let Some(note) = shared {
                    ui.weak(note);
                }
            },
        );
        if reset {
            let _ = set_keys_in(world, &command, layer, None);
        }
    }
    ui.data_mut(|data| match recording {
        Some(command) => {
            data.insert_temp(recording_id, command);
        }
        None => data.remove::<ActionId>(recording_id),
    });
}

/// "Also bound to Find" when another command shares one of these keys. Not an
/// error: which one runs depends on the focused panel.
fn shared_with(world: &World, command: &ActionId) -> Option<String> {
    let bindings = world.resource::<Keybindings>();
    let registry = world.resource::<ActionRegistry>();
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
