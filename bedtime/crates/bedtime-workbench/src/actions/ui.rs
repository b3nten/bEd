//! The `ui` feature: keyboard shortcuts from egui, and the Keyboard Shortcuts
//! section of a settings panel.
//!
//! Keys reach actions in two passes. Before panels draw, `claim` gives the
//! focused panel the keys of actions it handles itself (an editor's Move
//! Line Up on Alt+Up): they're its even where a text field would otherwise
//! keep them. After panels draw, `collect` takes the shortcuts widgets left.
use crate::actions::*;
#[cfg(feature = "persistence")]
use crate::settings::{
    SettingsWorldExt,
    ui::{CONTROL_WIDTH, Marks, columns},
};
#[cfg(feature = "persistence")]
use bedtime_persistence::StoreId;
#[cfg(feature = "persistence")]
use bedtime_ui::UiContext;
use bedtime_ui::{UiCamera, UiCapture, UiSystems, egui};
use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiInput, EguiPreUpdateSet, helpers::bevy_to_egui_physical_key};
use std::{cmp::Reverse, collections::HashSet};

pub(crate) fn install(app: &mut App) {
    app.configure_sets(
        PreUpdate,
        (
            ActionSystems::Claim
                .after(EguiPreUpdateSet::ProcessInput)
                .before(UiSystems::Draw),
            ActionSystems::CollectInput.after(UiSystems::Draw),
        ),
    )
    .add_systems(
        PreUpdate,
        (
            claim.in_set(ActionSystems::Claim),
            collect.in_set(ActionSystems::CollectInput),
        ),
    );
}

/// The UI camera and surface of the focused window.
fn focused_surface(world: &mut World) -> Option<(Entity, Entity)> {
    world
        .query::<(Entity, &UiCamera)>()
        .iter(world)
        .map(|(camera, ui)| (camera, ui.0))
        .find(|&(_, surface)| world.get::<Window>(surface).is_some_and(|w| w.focused))
}

/// Before panels draw: a key bound to an action the focused entity handles,
/// and can run now, is taken out of the frame's input and runs at once (so
/// the panel draws its effect this frame). Text fields don't keep these
/// keys: the panel asked for them. One whose handler can't run now (its
/// text doesn't have the keyboard) leaves them where they'd go.
fn claim(world: &mut World) {
    let Some((camera, surface)) = focused_surface(world) else {
        return;
    };
    let Some(mut ctx) = world.get_mut::<EguiContext>(camera) else {
        return;
    };
    let ctx = ctx.get_mut().clone();
    let capture = world.get::<UiCapture>(surface).copied().unwrap_or_default();
    if capture.modal
        || ctx.any_popup_open()
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
    {
        return;
    }
    let Some(focus) = world.actions().entity(InvocationContext::Focused) else {
        return;
    };
    let Some(events) = world
        .get::<EguiInput>(camera)
        .map(|input| input.events.clone())
    else {
        return;
    };
    let mut seen = HashSet::new();
    let chords: Vec<_> = world
        .resource::<Keybindings>()
        .chords()
        .filter(|chord| seen.insert(*chord))
        .filter_map(|chord| Some((chord, bevy_to_egui_physical_key(&chord.key)?)))
        .collect();
    let mut claimed = Vec::new();
    for (index, event) in events.iter().enumerate() {
        for &(chord, key) in &chords {
            let Some((repeat, ..)) = press(chord.modifiers, key, event) else {
                continue;
            };
            let Ok(Some(action)) = chord_target(world, chord, Some(focus)) else {
                continue;
            };
            let registry = world.resource::<ActionRegistry>();
            if registry.entity_handler(world, &action, focus).is_none()
                || !world
                    .actions()
                    .available(&action, InvocationContext::Entity(focus))
            {
                continue;
            }
            claimed.push(index);
            // Held keys are the panel's either way; they run again only
            // for actions that repeat.
            let repeats = registry.get(&action).is_some_and(|r| r.repeats);
            if !repeat || repeats {
                world
                    .resource_mut::<ActionRequests>()
                    .submit(ActionRequest {
                        action,
                        context: InvocationContext::Entity(focus),
                        source: None,
                    });
            }
            break;
        }
    }
    if claimed.is_empty() {
        return;
    }
    if let Some(mut input) = world.get_mut::<EguiInput>(camera) {
        let mut index = 0;
        input.events.retain(|_| {
            let keep = !claimed.contains(&index);
            index += 1;
            keep
        });
    }
    super::dispatch(world);
}

fn collect(world: &mut World) {
    let Some((camera, surface)) = focused_surface(world) else {
        return;
    };
    let Some(mut ctx) = world.get_mut::<EguiContext>(camera) else {
        return;
    };
    let ctx = ctx.get_mut().clone();
    let capture = world.get::<UiCapture>(surface).copied().unwrap_or_default();
    if !ctx.input(|input| input.focused)
        || capture.modal
        || ctx.any_popup_open()
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
    {
        return;
    }
    let focus = world.actions().entity(InvocationContext::Focused);
    let text = capture.text || ctx.text_edit_focused();
    let mut seen = HashSet::new();
    let mut chords: Vec<_> = world
        .resource::<Keybindings>()
        .chords()
        .filter_map(|chord| {
            let key = bevy_to_egui_physical_key(&chord.key)?;
            if !seen.insert(chord) || text && conflicts_with_text(key, chord.modifiers) {
                return None;
            }
            Some((chord, key))
        })
        .collect();
    // egui permits extra Shift/Alt when consuming a shortcut. Check exact
    // modifiers and consume specific combinations first to preserve our bindings.
    chords.sort_by_key(|(chord, _)| {
        Reverse(
            u8::from(chord.modifiers.contains(Modifiers::SHIFT))
                + u8::from(chord.modifiers.contains(Modifiers::ALT)),
        )
    });
    let mut presses = Vec::new();
    ctx.input_mut(|input| {
        let events = input.events.clone();
        for (chord, key) in chords {
            let matching: Vec<_> = events
                .iter()
                .enumerate()
                .filter_map(|(index, event)| {
                    let (repeat, modifiers, logical_key) = press(chord.modifiers, key, event)?;
                    Some((index, repeat, modifiers, logical_key))
                })
                .collect();
            let Some((_, _, modifiers, logical_key)) = matching.first() else {
                continue;
            };
            if input.consume_shortcut(&egui::KeyboardShortcut::new(*modifiers, *logical_key)) {
                presses.extend(matching.into_iter().map(|(index, repeat, _, _)| {
                    (
                        index,
                        BindingPress {
                            chord,
                            repeat,
                            context: focus
                                .map_or(InvocationContext::Global, InvocationContext::Entity),
                        },
                    )
                }));
            }
        }
    });
    // Preserve command order when several shortcuts arrive in the same frame.
    presses.sort_by_key(|(index, _)| *index);
    world
        .resource_mut::<BindingInput>()
        .0
        .extend(presses.into_iter().map(|(_, press)| press));
}

fn press(
    chord: Modifiers,
    key: egui::Key,
    event: &egui::Event,
) -> Option<(bool, egui::Modifiers, egui::Key)> {
    let egui::Event::Key {
        key: event_key,
        physical_key,
        pressed: true,
        repeat,
        modifiers,
    } = event
    else {
        return None;
    };
    let mut flags = Modifiers::default();
    for (pressed, flag) in [
        (
            modifiers.ctrl || modifiers.command && !cfg!(target_os = "macos"),
            Modifiers::CONTROL,
        ),
        (modifiers.shift, Modifiers::SHIFT),
        (modifiers.alt, Modifiers::ALT),
        (
            modifiers.mac_cmd || modifiers.command && cfg!(target_os = "macos"),
            Modifiers::META,
        ),
    ] {
        if pressed {
            flags = flags | flag;
        }
    }
    (physical_key.unwrap_or(*event_key) == key && flags == chord)
        .then_some((*repeat, *modifiers, *event_key))
}

fn conflicts_with_text(key: egui::Key, modifiers: Modifiers) -> bool {
    use egui::Key::*;
    let control = modifiers.contains(Modifiers::CONTROL);
    let alt = modifiers.contains(Modifiers::ALT);
    let meta = modifiers.contains(Modifiers::META);
    if !meta && (!control || alt) && key.symbol_or_name().chars().count() == 1 {
        return true;
    }
    let primary = if cfg!(target_os = "macos") {
        meta
    } else {
        control
    };
    matches!(
        key,
        ArrowLeft
            | ArrowRight
            | ArrowUp
            | ArrowDown
            | Home
            | End
            | PageUp
            | PageDown
            | Backspace
            | Delete
            | Enter
            | Tab
            | Escape
            | Space
            | Copy
            | Cut
            | Paste
    ) || primary && matches!(key, A | C | X | V | Z | Y)
        || control && matches!(key, A | E | B | F | P | N | H | K | U | W)
}

/// The Keyboard Shortcuts section of a settings panel: every registered action
/// and its keys in `layer`. Click an action's keys, then press the new ones.
#[cfg(feature = "persistence")]
pub fn keybindings_section(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    layer: StoreId,
) {
    let actions: Vec<_> = world
        .resource::<ActionRegistry>()
        .actions()
        .map(|(action, label)| (action.clone(), label))
        .collect();
    let recording_id = ui.id().with("recording-keys");
    let mut recording: Option<ActionId> = ui.data(|data| data.get_temp(recording_id));
    for (command, label) in actions {
        let set_here = world.actions().keys_in(&command, layer).is_some();
        let overridden = world.actions().keys_overridden_above(&command, layer);
        let marks = Marks::new(world.settings(), layer, overridden);
        let shared = world.actions().shared_with(&command);
        let mut reset = false;
        columns(
            ui,
            label,
            None,
            |ui| marks.overridden(ui, context),
            |ui| {
                let size = egui::vec2(CONTROL_WIDTH, 0.0);
                if recording.as_ref() == Some(&command) {
                    let cancel = ui
                        .add(egui::Button::new("Press keys…").min_size(size))
                        .on_hover_text("Escape or click to cancel")
                        .clicked();
                    match pressed_chord(world) {
                        Some(Ok(chord)) => {
                            let result =
                                world
                                    .actions_mut()
                                    .set_keys_in(&command, layer, Some(vec![chord]));
                            report(world, &format!("Couldn't change {label}'s keys"), result);
                            recording = None;
                        }
                        Some(Err(())) => recording = None,
                        None if cancel => recording = None,
                        None => {}
                    }
                } else {
                    // The layer's own view, without the layers above it.
                    let keys = world.actions().keys_at(&command, layer);
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
                reset = set_here && marks.reset(ui, context);
                if let Some(note) = shared {
                    ui.weak(note);
                }
            },
        );
        if reset {
            let result = world.actions_mut().set_keys_in(&command, layer, None);
            report(world, &format!("Couldn't reset {label}'s keys"), result);
        }
    }
    ui.data_mut(|data| match recording {
        Some(command) => {
            data.insert_temp(recording_id, command);
        }
        None => data.remove::<ActionId>(recording_id),
    });
}

#[cfg(feature = "persistence")]
fn report(world: &mut World, doing: &str, result: Result<(), String>) {
    if let Err(error) = result {
        world.write_message(crate::settings::SettingsError(format!("{doing}: {error}")));
    }
}

/// A key pressed this frame with its modifiers; `Err` for Escape. The
/// platform's main modifier is recorded as `Primary`, so the binding carries
/// over to other platforms.
#[cfg(feature = "persistence")]
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

#[cfg(feature = "persistence")]
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Save;
    impl Action for Save {
        const KEY: &'static str = "test.save";
        const LABEL: &'static str = "Save";
    }
    struct Other;
    impl Action for Other {
        const KEY: &'static str = "test.other";
        const LABEL: &'static str = "Other";
    }
    struct Repeating;
    impl Action for Repeating {
        const KEY: &'static str = "test.repeating";
        const LABEL: &'static str = "Repeating";
        const REPEATS: bool = true;
    }
    #[derive(Resource, Default)]
    struct Calls(Vec<ActionId>);

    fn record(world: &mut World, invocation: ActionInvocation) -> ActionResult {
        world.resource_mut::<Calls>().0.push(invocation.action);
        Ok(())
    }

    fn bind<C: Action>(app: &mut App, chord: KeyChord) {
        app.world_mut()
            .resource_mut::<Keybindings>()
            .set(ActionId::of::<C>(), vec![chord]);
    }

    fn app() -> (App, Entity, egui::Context) {
        let mut app = App::new();
        app.add_plugins(ActionsPlugin)
            .init_resource::<Calls>()
            .add_action_handler::<Save>(Handler::new(record))
            .add_action_handler::<Other>(Handler::new(record))
            .add_action_handler::<Repeating>(Handler::new(record));
        let surface = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                UiCapture::default(),
            ))
            .id();
        let camera = app.world_mut().spawn(UiCamera(surface)).id();
        let ctx = app
            .world_mut()
            .get_mut::<EguiContext>(camera)
            .unwrap()
            .get_mut()
            .clone();
        ctx.options_mut(|options| {
            options.max_passes = std::num::NonZeroUsize::new(1).unwrap();
            // egui quits on Cmd+Q by default; apps (workbench) turn that off
            // so the key can be bound.
            options.quit_shortcuts.clear();
        });
        app.update();
        (app, surface, ctx)
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat,
            modifiers,
        }
    }

    fn release(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: default(),
        }
    }

    fn frame(
        app: &mut App,
        surface: Entity,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        draw: impl FnMut(&mut egui::Ui),
    ) {
        let text = ctx.text_edit_focused();
        let modal = ctx.any_popup_open() || ctx.memory(|memory| memory.top_modal_layer().is_some());
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..default()
            },
            draw,
        );
        output.textures_delta.clear();
        *app.world_mut().get_mut::<UiCapture>(surface).unwrap() = UiCapture {
            text: text || ctx.text_edit_focused(),
            modal: modal
                || ctx.any_popup_open()
                || ctx.memory(|memory| memory.top_modal_layer().is_some()),
            ..default()
        };
        app.update();
    }

    #[test]
    fn shortcuts_run_without_anything_focused() {
        let (mut app, surface, ctx) = app();
        bind::<Save>(&mut app, KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS));
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, false)],
            |_| {},
        );
        assert_eq!(
            app.world().resource::<Calls>().0,
            vec![ActionId::of::<Save>()]
        );
    }

    #[test]
    fn widget_consumption_and_binding_repeat_policy_are_respected() {
        let (mut app, surface, ctx) = app();
        bind::<Save>(&mut app, KeyChord::primary(KeyCode::KeyS));
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, false)],
            |_| {},
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, true)],
            |_| {},
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        // Only commands that repeat run again while the keys are held.
        bind::<Save>(&mut app, KeyChord::primary(KeyCode::KeyX));
        bind::<Repeating>(&mut app, KeyChord::primary(KeyCode::KeyS));
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, true)],
            |_| {},
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 2);
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, true)],
            |ui| {
                assert!(ui.input_mut(|input| input.consume_shortcut(
                    &egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S)
                )));
            },
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 2);
    }

    #[test]
    fn exact_modifiers_and_event_order_survive_egui_shortcut_consumption() {
        let (mut app, surface, ctx) = app();
        bind::<Save>(&mut app, KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS));
        let shifted = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, shifted, false)],
            |_| {},
        );
        assert!(app.world().resource::<Calls>().0.is_empty());
        bind::<Other>(
            &mut app,
            KeyChord::new(Modifiers::PRIMARY | Modifiers::SHIFT, KeyCode::KeyS),
        );
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::S),
                key(egui::Key::S, egui::Modifiers::COMMAND, false),
                release(egui::Key::S),
                key(egui::Key::S, shifted, false),
            ],
            |_| {},
        );
        assert_eq!(
            app.world().resource::<Calls>().0,
            vec![ActionId::of::<Save>(), ActionId::of::<Other>()]
        );
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::S),
                egui::Event::Key {
                    key: egui::Key::Q,
                    physical_key: Some(egui::Key::S),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
            ],
            |_| {},
        );
        assert_eq!(
            app.world().resource::<Calls>().0.last(),
            Some(&ActionId::of::<Save>()),
            "Remaining egui input: {:?}",
            ctx.input(|input| input.events.clone())
        );
    }

    #[test]
    fn text_editing_reserves_noop_edits_and_enter_but_allows_save() {
        let (mut app, surface, ctx) = app();
        app.world_mut().resource_mut::<Keybindings>().set(
            ActionId::of::<Other>(),
            vec![
                KeyChord::new(Modifiers::default(), KeyCode::Delete),
                KeyChord::new(Modifiers::default(), KeyCode::Enter),
                KeyChord::primary(KeyCode::KeyC),
            ],
        );
        bind::<Save>(&mut app, KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS));
        let id = egui::Id::new("text");
        let mut text = String::new();
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                key(egui::Key::Delete, default(), false),
                key(egui::Key::S, egui::Modifiers::COMMAND, false),
                key(egui::Key::C, egui::Modifiers::COMMAND, false),
            ],
            |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).id(id));
            },
        );
        assert_eq!(
            app.world().resource::<Calls>().0,
            vec![ActionId::of::<Save>()]
        );
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::Enter, default(), false)],
            |ui| {
                assert!(
                    ui.add(egui::TextEdit::singleline(&mut text).id(id))
                        .lost_focus()
                );
            },
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::Enter),
                key(egui::Key::Enter, default(), false),
            ],
            |_| {},
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 2);
    }

    #[test]
    fn modals_and_unfocused_windows_block_shortcuts() {
        let (mut app, surface, ctx) = app();
        bind::<Save>(&mut app, KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS));
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, egui::Modifiers::COMMAND, false)],
            |ui| {
                egui::Modal::new(egui::Id::new("modal")).show(ui.ctx(), |ui| {
                    ui.label("Modal task");
                });
            },
        );
        assert!(app.world().resource::<Calls>().0.is_empty());
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::S),
                key(egui::Key::S, egui::Modifiers::COMMAND, false),
            ],
            |_| {},
        );
        assert!(app.world().resource::<Calls>().0.is_empty());
        app.world_mut().get_mut::<Window>(surface).unwrap().focused = false;
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::S),
                key(egui::Key::S, egui::Modifiers::COMMAND, false),
            ],
            |_| {},
        );
        assert!(app.world().resource::<Calls>().0.is_empty());
        app.world_mut().get_mut::<Window>(surface).unwrap().focused = true;
        frame(
            &mut app,
            surface,
            &ctx,
            vec![
                release(egui::Key::S),
                key(egui::Key::S, egui::Modifiers::COMMAND, false),
            ],
            |_| {},
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    }

    struct MoveLine;
    impl Action for MoveLine {
        const KEY: &'static str = "test.move_line";
        const LABEL: &'static str = "Move Line";
    }
    #[derive(Resource)]
    struct Focused(Entity);
    #[derive(Resource)]
    struct Typing(bool);

    /// Key events waiting in the focused window's input, as panels would
    /// get them.
    fn input(app: &mut App) -> Vec<egui::Event> {
        let world = app.world_mut();
        let camera = world
            .query_filtered::<Entity, With<UiCamera>>()
            .single(world)
            .unwrap();
        world.get::<EguiInput>(camera).unwrap().events.clone()
    }

    fn press_before_drawing(app: &mut App, event: egui::Event) {
        let world = app.world_mut();
        let camera = world
            .query_filtered::<Entity, With<UiCamera>>()
            .single(world)
            .unwrap();
        if world.get::<EguiInput>(camera).is_none() {
            world.entity_mut(camera).insert(EguiInput::default());
        }
        world.get_mut::<EguiInput>(camera).unwrap().events = vec![event];
        app.update();
    }

    #[test]
    fn a_focused_panel_claims_its_keys_before_drawing_even_text_keys() {
        let (mut app, _, _) = app();
        app.register_action::<MoveLine>()
            .insert_resource(Typing(true))
            .insert_resource(ActionFocus(|world| {
                world.get_resource::<Focused>().map(|focused| focused.0)
            }));
        let panel = app
            .world_mut()
            .spawn(ActionHandler::<MoveLine>::new(
                // Only while its own text has the keyboard.
                Handler::new(record).available_when(|world, _| world.resource::<Typing>().0),
            ))
            .id();
        app.insert_resource(Focused(panel));
        let alt_up = || key(egui::Key::ArrowUp, egui::Modifiers::ALT, false);
        bind::<MoveLine>(&mut app, KeyChord::new(Modifiers::ALT, KeyCode::ArrowUp));
        // Alt+Up is a text key, but the panel handles it: taken and run.
        press_before_drawing(&mut app, alt_up());
        assert_eq!(
            app.world().resource::<Calls>().0,
            vec![ActionId::of::<MoveLine>()]
        );
        assert!(input(&mut app).is_empty());
        // Held: still the panel's, but it doesn't repeat.
        press_before_drawing(
            &mut app,
            key(egui::Key::ArrowUp, egui::Modifiers::ALT, true),
        );
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        assert!(input(&mut app).is_empty());
        // Its handler can't run now (the Find field has the keyboard): the
        // key stays for whatever has focus.
        app.insert_resource(Typing(false));
        press_before_drawing(&mut app, alt_up());
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        assert_eq!(input(&mut app).len(), 1);
        // Global actions aren't claimed: they wait for `collect`.
        bind::<MoveLine>(&mut app, KeyChord::new(Modifiers::ALT, KeyCode::ArrowLeft));
        bind::<Other>(&mut app, KeyChord::new(Modifiers::ALT, KeyCode::ArrowUp));
        press_before_drawing(&mut app, alt_up());
        assert_eq!(app.world().resource::<Calls>().0.len(), 1);
        assert_eq!(input(&mut app).len(), 1);
        // Nothing focused: nothing claimed.
        app.world_mut().remove_resource::<Focused>();
        app.insert_resource(Typing(true));
        press_before_drawing(
            &mut app,
            key(egui::Key::ArrowLeft, egui::Modifiers::ALT, false),
        );
        assert_eq!(input(&mut app).len(), 1);
    }

    #[test]
    fn keymaps_give_default_keys_before_or_after_registration() {
        struct Later;
        impl Action for Later {
            const KEY: &'static str = "test.later";
            const LABEL: &'static str = "Later";
            const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyL)];
        }
        const UP: &[KeyChord] = &[KeyChord::new(Modifiers::ALT, KeyCode::ArrowUp)];
        const J: &[KeyChord] = &[KeyChord::primary(KeyCode::KeyJ)];
        let (mut app, _, _) = app();
        app.register_action::<MoveLine>();
        app.add_keymap(&[
            ("test.move_line", UP),
            ("test.later", J),
            ("test.absent", J),
        ]);
        app.register_action::<Later>();
        let keys = app.world().resource::<Keybindings>();
        assert_eq!(keys.keys(&ActionId::of::<MoveLine>()), UP);
        // The keymap beats the action's own defaults.
        assert_eq!(keys.keys(&ActionId::of::<Later>()), J);
    }
}
