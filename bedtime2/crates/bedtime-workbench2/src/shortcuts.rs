//! Keyboard shortcuts from the workbench's egui context, after widget interaction.
use crate::*;
use bedtime_ui2::{UiCamera, UiCapture, UiSystems, egui};
use bevy::prelude::*;
use bevy_egui::{EguiContext, helpers::bevy_to_egui_physical_key};
use std::{cmp::Reverse, collections::HashSet};

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PreUpdate,
        crate::layout::apply_focus
            .after(UiSystems::Draw)
            .before(ContextCommandSystems::CollectInput),
    )
    .add_systems(
        PreUpdate,
        collect
            .in_set(ContextCommandSystems::CollectInput)
            .after(UiSystems::Draw),
    );
}

fn collect(world: &mut World) {
    let Some((camera, surface)) = world
        .query_filtered::<(Entity, &UiCamera), With<WorkbenchCamera>>()
        .iter(world)
        .next()
        .map(|(camera, ui)| (camera, ui.0))
    else {
        return;
    };
    if !world
        .get::<Window>(surface)
        .is_some_and(|window| window.focused)
    {
        return;
    }
    let ctx = world
        .get_mut::<EguiContext>(camera)
        .unwrap()
        .get_mut()
        .clone();
    let capture = world.get::<UiCapture>(surface).copied().unwrap();
    if !ctx.input(|input| input.focused)
        || capture.modal
        || ctx.any_popup_open()
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
    {
        return;
    }
    let Some(workspace) = crate::workspaces::selected(world) else {
        return;
    };
    let panel = crate::commands::focused_panel(world);
    let text = capture.text || ctx.text_edit_focused();
    let mut seen = HashSet::new();
    let mut chords: Vec<_> = world
        .query::<(Entity, &Binding, Option<&InPanel>)>()
        .iter(world)
        .filter(|(entity, _, scope)| {
            crate::workspaces::owner(world, *entity).is_none_or(|owner| owner == workspace)
                && scope.is_none_or(|scope| Some(scope.0) == panel)
        })
        .filter_map(|(_, binding, _)| {
            let chord = KeyChord::new(binding.chord.modifiers.resolved(), binding.chord.key);
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
                    let egui::Event::Key {
                        key: event_key,
                        physical_key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
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
                    (physical_key.unwrap_or(*event_key) == key && flags == chord.modifiers)
                        .then_some((index, *repeat, *modifiers, *event_key))
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
                            context: panel
                                .map_or(InvocationContext::Global, InvocationContext::Panel),
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Save;
    impl ContextCommand for Save {
        const KEY: &'static str = "test.save";
        const LABEL: &'static str = "Save";
    }
    struct Other;
    impl ContextCommand for Other {
        const KEY: &'static str = "test.other";
        const LABEL: &'static str = "Other";
    }
    #[derive(Resource, Default)]
    struct Calls(Vec<ContextCommandId>);

    fn record(
        invocation: In<ContextCommandInvocation>,
        mut calls: ResMut<Calls>,
    ) -> ContextCommandResult {
        calls.0.push(invocation.command.clone());
        Ok(())
    }

    fn app() -> (App, Entity, egui::Context) {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, WorkbenchUiPlugin))
            .init_resource::<Calls>();
        Workspaces::new(app.world_mut()).open("Test");
        let mut workbench = Workbench::new(app.world_mut());
        workbench.register_handler::<Save, _>(record).unwrap();
        workbench.register_handler::<Other, _>(record).unwrap();
        let surface = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                WorkbenchSurface::default(),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((Camera2d, WorkbenchCamera, UiCamera(surface)))
            .id();
        let ctx = app
            .world_mut()
            .get_mut::<EguiContext>(camera)
            .unwrap()
            .get_mut()
            .clone();
        ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
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
    fn widget_consumption_and_binding_repeat_policy_are_respected() {
        let (mut app, surface, ctx) = app();
        let binding = app
            .world_mut()
            .spawn(Binding::new::<Save>(KeyChord::new(
                Modifiers::PRIMARY,
                KeyCode::KeyS,
            )))
            .id();
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
        app.world_mut().get_mut::<Binding>(binding).unwrap().repeat = true;
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
        app.world_mut().spawn(Binding::new::<Save>(KeyChord::new(
            Modifiers::PRIMARY,
            KeyCode::KeyS,
        )));
        let shifted = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        frame(
            &mut app,
            surface,
            &ctx,
            vec![key(egui::Key::S, shifted, false)],
            |_| {},
        );
        assert!(app.world().resource::<Calls>().0.is_empty());
        app.world_mut().spawn(Binding::new::<Other>(KeyChord::new(
            Modifiers::PRIMARY | Modifiers::SHIFT,
            KeyCode::KeyS,
        )));
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
            vec![
                ContextCommandId::of::<Save>(),
                ContextCommandId::of::<Other>()
            ]
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
            Some(&ContextCommandId::of::<Save>()),
            "Remaining egui input: {:?}",
            ctx.input(|input| input.events.clone())
        );
    }

    #[test]
    fn text_editing_reserves_noop_edits_and_enter_but_allows_save() {
        let (mut app, surface, ctx) = app();
        for chord in [
            KeyChord::new(Modifiers::default(), KeyCode::Delete),
            KeyChord::new(Modifiers::default(), KeyCode::Enter),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyC),
        ] {
            app.world_mut().spawn(Binding::new::<Other>(chord));
        }
        app.world_mut().spawn(Binding::new::<Save>(KeyChord::new(
            Modifiers::PRIMARY,
            KeyCode::KeyS,
        )));
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
            vec![ContextCommandId::of::<Save>()]
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
    fn modals_and_unfocused_workbench_windows_block_shortcuts() {
        let (mut app, surface, ctx) = app();
        app.world_mut().spawn(Binding::new::<Save>(KeyChord::new(
            Modifiers::PRIMARY,
            KeyCode::KeyS,
        )));
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
}
