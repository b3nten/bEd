//! One controlled input transition after UI and before ContextCommand lookup.
use crate::*;
use bedtime_ui2::{
    UiSystems,
    input::{InputOwner, InputRouter, RoutedInput},
};
use bevy::{
    ecs::system::{SystemId, SystemInput},
    prelude::*,
    window::WindowEvent,
};
use std::collections::HashSet;

#[derive(Component)]
struct PanelInputSystem;

#[derive(Component)]
pub(crate) struct HasPanelInput;

/// Eligible state for one panel. Claims and captures are applied by the router.
pub struct PanelInput<'a> {
    pub panel: Entity,
    pub input: &'a RoutedInput,
    pub content_focused: bool,
    consumed: &'a mut HashSet<u64>,
    keyboard: &'a mut bool,
    pointer: &'a mut bool,
}

impl SystemInput for PanelInput<'_> {
    type Param<'i> = PanelInput<'i>;
    type Inner<'i> = PanelInput<'i>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}

impl PanelInput<'_> {
    pub fn consume(&mut self, event: u64) {
        self.consumed.insert(event);
    }

    pub fn capture_keyboard(&mut self) {
        *self.keyboard = true;
    }

    pub fn capture_pointer(&mut self) {
        *self.pointer = true;
    }

    pub fn release_keyboard(&mut self) {
        *self.keyboard = false;
    }

    pub fn release_pointer(&mut self) {
        *self.pointer = false;
    }

    pub fn consume_all(&mut self) {
        self.consumed.extend(self.input.events.iter().map(|e| e.id));
    }
}
pub fn register_panel_input<M>(
    world: &mut World,
    panel: Entity,
    system: impl IntoSystem<PanelInput<'static>, (), M> + 'static,
) -> Result<Entity, String> {
    let world = crate::workspaces::selected_world(world, None)?;
    if world.get::<Panel>(panel).is_none() {
        return Err("Panel is unavailable".into());
    }
    if world.get::<HasPanelInput>(panel).is_some() {
        return Err("Panel already has an input system".into());
    }
    let id = world.register_system(system).entity();
    world
        .entity_mut(id)
        .insert((PanelInputSystem, InPanel(panel)));
    world.entity_mut(panel).insert(HasPanelInput);
    Ok(id)
}

pub(crate) fn install(app: &mut App) {
    app.configure_sets(
        PreUpdate,
        ContextCommandSystems::PanelInput.after(UiSystems::Draw),
    )
    .configure_sets(
        PreUpdate,
        ContextCommandSystems::PublishInput
            .after(WorkbenchSystems::Apply)
            .before(UiSystems::PublishInput),
    )
    .add_systems(
        PreUpdate,
        crate::layout::apply_focus
            .after(UiSystems::Draw)
            .before(ContextCommandSystems::PanelInput),
    )
    .add_systems(PreUpdate, route.in_set(ContextCommandSystems::PanelInput))
    .add_systems(
        PreUpdate,
        publish.in_set(ContextCommandSystems::PublishInput),
    );
}

fn route(world: &mut World) {
    let Some(mut router) = world.remove_resource::<InputRouter>() else {
        return;
    };
    let focused = crate::commands::focused_panel(world);
    let stamp = Workbench::new(world).stamp();
    let stamp_changed = world
        .get_resource::<InputLayout>()
        .is_some_and(|old| old.0 != stamp);
    world.insert_resource(InputLayout(stamp));
    let visible: HashSet<_> = world
        .query::<(Entity, &PanelPresentation)>()
        .iter(world)
        .filter(|(_, p)| p.visible)
        .map(|(e, _)| e)
        .collect();
    let active: Vec<_> = router
        .panel_states
        .keys()
        .copied()
        .chain(router.keyboard_capture)
        .chain(router.pointer_capture)
        .collect();
    for panel in active {
        if stamp_changed
            || !visible.contains(&panel)
            || focused != Some(panel) && router.keyboard_capture == Some(panel)
        {
            router.cancel_panel(panel);
        }
    }
    let systems: Vec<_> = world
        .query::<(Entity, &InPanel)>()
        .iter(world)
        .filter(|(e, p)| {
            world.get::<PanelInputSystem>(*e).is_some()
                && (visible.contains(&p.0) || router.cancelled_panels.contains(&p.0))
        })
        .map(|(e, p)| (e, p.0))
        .collect();
    let events = router.frame.get_mut().unwrap().events.clone();
    let regions = router.frame.get_mut().unwrap().regions.clone();
    let content = router.frame.get_mut().unwrap().focused_content;
    let text_active = router.frame.get_mut().unwrap().text_active;
    if let Some(panel) = router.keyboard_capture {
        if text_active || regions.iter().any(|r| r.owner == panel) && content != Some(panel) {
            router.cancel_panel(panel);
        }
    }
    let modal = router.frame.get_mut().unwrap().modal;
    let focus_lost = events
        .iter()
        .any(|e| matches!(&e.event,WindowEvent::WindowFocused(f) if !f.focused));
    if focus_lost || modal {
        for (_, panel) in &systems {
            router.cancel_panel(*panel);
        }
    }
    for (system, panel) in systems {
        let rect = world.get::<PanelPresentation>(panel).unwrap().rect;
        let mut preview = router.panel_states.get(&panel).cloned().unwrap_or_default();
        preview.begin_frame();
        if router.cancelled_panels.contains(&panel) {
            preview.release_all();
        }
        let mut position = router.pointer_position;
        for event in &events {
            if let WindowEvent::CursorMoved(p) = &event.event {
                position = p.position;
            }
            let owned_release = match &event.event {
                WindowEvent::KeyboardInput(k) => {
                    !k.state.is_pressed()
                        && router.key_owners.get(&k.key_code) == Some(&InputOwner::Panel(panel))
                }
                WindowEvent::MouseButtonInput(b) => {
                    !b.state.is_pressed()
                        && router.button_owners.get(&b.button) == Some(&InputOwner::Panel(panel))
                }
                _ => false,
            };
            if router
                .frame
                .get_mut()
                .unwrap()
                .claims
                .contains_key(&event.id)
                && !owned_release
            {
                continue;
            }
            let target = match &event.event {
                WindowEvent::KeyboardInput(k) => match router.key_owners.get(&k.key_code) {
                    Some(InputOwner::Ui | InputOwner::Command) => false,
                    Some(InputOwner::Panel(p)) => *p == panel,
                    _ => router.keyboard_capture.or(focused) == Some(panel),
                },
                WindowEvent::Ime(_) => router.keyboard_capture.or(focused) == Some(panel),
                WindowEvent::CursorMoved(_)
                | WindowEvent::MouseButtonInput(_)
                | WindowEvent::MouseWheel(_) => {
                    router.pointer_capture == Some(panel)
                        || router.pointer_capture.is_none()
                            && rect.contains(position)
                            && (!regions.iter().any(|r| r.owner == panel)
                                || regions.iter().any(|r| {
                                    r.owner == panel
                                        && r.rect.contains(bedtime_ui2::egui::pos2(
                                            position.x, position.y,
                                        ))
                                }))
                }
                _ => false,
            };
            if target && visible.contains(&panel) && !focus_lost && !modal {
                preview.admit(event.clone());
            }
        }
        let mut consumed = HashSet::new();
        let mut keyboard = router.keyboard_capture == Some(panel);
        let mut pointer = router.pointer_capture == Some(panel);
        let result = world.run_system_with(
            SystemId::<PanelInput<'static>>::from_entity(system),
            PanelInput {
                panel,
                input: &preview,
                content_focused: visible.contains(&panel)
                    && content == Some(panel)
                    && !text_active
                    && !focus_lost
                    && !modal
                    && focused == Some(panel),
                consumed: &mut consumed,
                keyboard: &mut keyboard,
                pointer: &mut pointer,
            },
        );
        if let Err(error) = result {
            world.write_message(bedtime_ui2::UiError(error.to_string()));
            router.cancel_panel(panel);
            keyboard = false;
            pointer = false;
        }
        if keyboard && visible.contains(&panel) && focused == Some(panel) && !focus_lost && !modal {
            router.keyboard_capture = Some(panel);
        } else if router.keyboard_capture == Some(panel) {
            router.keyboard_capture = None;
        }
        if pointer && visible.contains(&panel) && !focus_lost && !modal {
            router.pointer_capture = Some(panel);
        } else if router.pointer_capture == Some(panel) {
            router.pointer_capture = None;
        }
        for event in &preview.events {
            let captured = match event.event {
                WindowEvent::KeyboardInput(_) | WindowEvent::Ime(_) => keyboard,
                WindowEvent::CursorMoved(_)
                | WindowEvent::MouseButtonInput(_)
                | WindowEvent::MouseWheel(_) => pointer,
                _ => false,
            };
            if consumed.contains(&event.id) || captured {
                router
                    .frame
                    .get_mut()
                    .unwrap()
                    .claims
                    .entry(event.id)
                    .or_insert(InputOwner::Panel(panel));
            }
        }
        if pointer
            && preview.buttons.get_pressed().next().is_none()
            && preview.buttons.get_just_released().next().is_some()
        {
            router.pointer_capture = None;
        }
    }
    // The shell/widgets own pointer input outside explicitly registered panel
    // content. A viewport can delegate its image region without leaking chrome.
    let mut position = router.pointer_position;
    for event in &events {
        if let WindowEvent::CursorMoved(p) = &event.event {
            position = p.position;
        }
        let pointer = matches!(
            event.event,
            WindowEvent::CursorMoved(_)
                | WindowEvent::MouseButtonInput(_)
                | WindowEvent::MouseWheel(_)
        );
        if pointer
            && router.pointer_capture.is_none()
            && !regions.iter().any(|r| {
                r.rect
                    .contains(bedtime_ui2::egui::pos2(position.x, position.y))
            })
        {
            router
                .frame
                .get_mut()
                .unwrap()
                .claims
                .entry(event.id)
                .or_insert(InputOwner::Ui);
        }
        if !router
            .frame
            .get_mut()
            .unwrap()
            .claims
            .contains_key(&event.id)
        {
            if let Some(k) = event.key() {
                if k.state.is_pressed()
                    && !matches!(
                        router.key_owners.get(&k.key_code),
                        Some(InputOwner::Ui | InputOwner::Command)
                    )
                {
                    let m = event.modifiers;
                    let mut modifiers = Modifiers::default();
                    for (active, flag) in [
                        (m.control, Modifiers::CONTROL),
                        (m.shift, Modifiers::SHIFT),
                        (m.alt, Modifiers::ALT),
                        (m.meta, Modifiers::META),
                    ] {
                        if active {
                            modifiers = modifiers | flag;
                        }
                    }
                    world.resource_mut::<BindingInput>().0.push(BindingPress {
                        event: event.id,
                        chord: KeyChord::new(modifiers, k.key_code),
                        repeat: k.repeat,
                        context: focused
                            .map_or(InvocationContext::Global, InvocationContext::Panel),
                    });
                }
            }
        }
    }
    router.pointer_position = position;
    world.insert_resource(router);
}
#[derive(Resource)]
struct InputLayout(LayoutStamp);

/// Release the old workspace before local entity IDs can be reused by another.
pub(crate) fn deactivate(world: &mut World, router: &mut InputRouter) {
    let systems: Vec<_> = world
        .query::<(Entity, &InPanel)>()
        .iter(world)
        .filter(|(e, _)| world.get::<PanelInputSystem>(*e).is_some())
        .map(|(e, panel)| (e, panel.0))
        .collect();
    for (system, panel) in systems {
        router.cancel_panel(panel);
        if let Some(mut input) = router.panel_states.get(&panel).cloned() {
            input.begin_frame();
            input.release_all();
            let mut consumed = HashSet::new();
            let (mut keyboard, mut pointer) = (false, false);
            world
                .run_system_with(
                    SystemId::<PanelInput<'static>>::from_entity(system),
                    PanelInput {
                        panel,
                        input: &input,
                        content_focused: false,
                        consumed: &mut consumed,
                        keyboard: &mut keyboard,
                        pointer: &mut pointer,
                    },
                )
                .expect("Panel input system");
        }
    }
    router.panel_states.clear();
    router.cancelled_panels.clear();
    router.frame.get_mut().unwrap().focused_content = None;
    router.frame.get_mut().unwrap().regions.clear();
}

fn publish(world: &mut World) {
    let Some(mut router) = world.remove_resource::<InputRouter>() else {
        return;
    };
    for id in &world.resource::<BindingClaims>().0 {
        router
            .frame
            .get_mut()
            .unwrap()
            .claims
            .entry(*id)
            .or_insert(InputOwner::Command);
    }
    let alive: HashSet<_> = world
        .query_filtered::<Entity, With<Panel>>()
        .iter(world)
        .collect();
    for panel in router.panel_states.keys().copied().collect::<Vec<_>>() {
        if !alive.contains(&panel) {
            router.cancel_panel(panel);
        }
    }
    router.panel_states.retain(|p, _| alive.contains(p));
    world.insert_resource(router);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_ui2::input::{InputEvent, InputModifiers, InputRegion, UiInputFrame};
    use bevy::input::{
        ButtonState,
        keyboard::{Key, KeyCode, KeyboardInput},
    };
    #[derive(Resource, Default)]
    struct Calls(u32);
    #[derive(Resource, Default)]
    struct Guest(RoutedInput);
    struct Save;
    impl ContextCommand for Save {
        const KEY: &'static str = "input.save";
        const LABEL: &'static str = "Save";
    }
    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(WorkspacePlugin)
            .init_resource::<InputRouter>()
            .init_resource::<RoutedInput>()
            .init_resource::<Calls>()
            .init_resource::<Guest>();
        install(&mut app);
        app.add_systems(
            PreUpdate,
            (|mut router: ResMut<InputRouter>, mut application: ResMut<RoutedInput>| {
                router.publish(&mut application)
            })
            .in_set(UiSystems::PublishInput),
        );
        let mut w = Workbench::new(app.world_mut());
        let panel = w.create_panel(w.active_layout(), "Game").unwrap();
        w.world.get_mut::<PanelPresentation>(panel).unwrap().visible = true;
        w.world.get_mut::<PanelPresentation>(panel).unwrap().rect =
            Rect::from_corners(Vec2::ZERO, Vec2::splat(500.0));
        w.register_handler::<Save, _>(
            |_: In<ContextCommandInvocation>, mut calls: ResMut<Calls>| -> ContextCommandResult {
                calls.0 += 1;
                Ok(())
            },
        )
        .unwrap();
        w.world.spawn(Binding::new::<Save>(KeyChord::new(
            Modifiers::PRIMARY,
            KeyCode::KeyS,
        )));
        (app, panel)
    }
    fn key(id: u64, pressed: bool) -> InputEvent {
        InputEvent {
            id,
            modifiers: InputModifiers {
                meta: cfg!(target_os = "macos"),
                control: !cfg!(target_os = "macos"),
                ..default()
            },
            event: WindowEvent::KeyboardInput(KeyboardInput {
                key_code: KeyCode::KeyS,
                logical_key: Key::Character("s".into()),
                state: if pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                text: Some("s".into()),
                repeat: false,
                window: Entity::PLACEHOLDER,
            }),
        }
    }
    fn frame(app: &mut App, panel: Entity, events: Vec<InputEvent>, focused: bool) {
        *app.world_mut()
            .resource_mut::<InputRouter>()
            .frame
            .get_mut()
            .unwrap() = UiInputFrame {
            events,
            focused_content: focused.then_some(panel),
            regions: vec![InputRegion {
                owner: panel,
                id: bedtime_ui2::egui::Id::new("game"),
                rect: bedtime_ui2::egui::Rect::from_min_size(
                    bedtime_ui2::egui::Pos2::ZERO,
                    bedtime_ui2::egui::vec2(500.0, 500.0),
                ),
            }],
            ..default()
        };
    }
    fn game(mut input: PanelInput, mut guest: ResMut<Guest>) {
        guest.0 = input.input.clone();
        if input.content_focused {
            input.capture_keyboard();
        }
    }
    #[test]
    fn captured_game_keyboard_bypasses_save_and_forwards_to_a_separate_world() {
        let (mut app, panel) = app();
        register_panel_input(app.world_mut(), panel, game).unwrap();
        frame(&mut app, panel, vec![key(1, true)], true);
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 0);
        assert!(app.world().resource::<RoutedInput>().events.is_empty());
        let mut guest = World::new();
        guest.insert_resource(app.world().resource::<Guest>().0.clone());
        assert!(
            guest
                .resource::<RoutedInput>()
                .keys
                .just_pressed(KeyCode::KeyS)
        );
        frame(&mut app, panel, vec![key(2, false)], true);
        app.update();
        assert!(
            app.world()
                .resource::<Guest>()
                .0
                .keys
                .just_released(KeyCode::KeyS)
        );
        frame(&mut app, panel, vec![key(3, true)], false);
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 1);
    }
    #[test]
    fn ui_claims_have_priority_over_panels_and_focus_loss_cancels_capture() {
        let (mut app, panel) = app();
        register_panel_input(app.world_mut(), panel, game).unwrap();
        frame(&mut app, panel, vec![key(1, true)], true);
        app.world_mut()
            .resource_mut::<InputRouter>()
            .frame
            .get_mut()
            .unwrap()
            .claims
            .insert(1, InputOwner::Ui);
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 0);
        assert!(app.world().resource::<Guest>().0.events.is_empty());
        frame(&mut app, panel, vec![key(2, false)], true);
        app.update();
        frame(&mut app, panel, vec![key(3, true)], true);
        app.update();
        frame(
            &mut app,
            panel,
            vec![InputEvent {
                id: 4,
                modifiers: default(),
                event: WindowEvent::WindowFocused(bevy::window::WindowFocused {
                    window: Entity::PLACEHOLDER,
                    focused: false,
                }),
            }],
            true,
        );
        app.update();
        let router = app.world().resource::<InputRouter>();
        assert!(router.keyboard_capture.is_none());
        assert!(!router.panel_states[&panel].keys.pressed(KeyCode::KeyS));
        assert_eq!(app.world().resource::<Calls>().0, 0);
    }
    #[test]
    fn modals_block_workbench_shortcuts_and_nonediting_save_survives_text_focus() {
        let (mut app, panel) = app();
        frame(&mut app, panel, vec![key(1, true)], false);
        {
            let mut router = app.world_mut().resource_mut::<InputRouter>();
            let f = router.frame.get_mut().unwrap();
            f.modal = true;
            f.claims.insert(1, InputOwner::Ui);
        }
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 0);
        frame(&mut app, panel, vec![key(2, false)], false);
        app.update();
        frame(&mut app, panel, vec![key(3, true)], false);
        app.world_mut()
            .resource_mut::<InputRouter>()
            .frame
            .get_mut()
            .unwrap()
            .claim_text(panel, bedtime_ui2::egui::Id::new("text"));
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 1);
        assert!(app.world().resource::<RoutedInput>().events.is_empty());
    }
    fn mouse(id: u64, event: WindowEvent) -> InputEvent {
        InputEvent {
            id,
            event,
            modifiers: InputModifiers {
                shift: true,
                ..default()
            },
        }
    }
    fn drag(mut input: PanelInput, mut guest: ResMut<Guest>) {
        guest.0 = input.input.clone();
        if input.input.buttons.pressed(MouseButton::Left) {
            input.capture_pointer();
        }
        input.consume_all();
    }
    #[test]
    fn shift_drag_keeps_pointer_capture_outside_content_and_hidden_panels_receive_release() {
        let (mut app, panel) = app();
        register_panel_input(app.world_mut(), panel, drag).unwrap();
        let moved = |position| {
            WindowEvent::CursorMoved(bevy::window::CursorMoved {
                window: Entity::PLACEHOLDER,
                position,
                delta: None,
            })
        };
        let button = |state| {
            WindowEvent::MouseButtonInput(bevy::input::mouse::MouseButtonInput {
                window: Entity::PLACEHOLDER,
                button: MouseButton::Left,
                state,
            })
        };
        let mut shift = key(1, true);
        if let WindowEvent::KeyboardInput(k) = &mut shift.event {
            k.key_code = KeyCode::ShiftLeft;
            k.text = None;
        }
        frame(
            &mut app,
            panel,
            vec![
                shift,
                mouse(2, moved(Vec2::splat(100.0))),
                mouse(3, button(ButtonState::Pressed)),
            ],
            true,
        );
        app.update();
        assert_eq!(
            app.world().resource::<InputRouter>().pointer_capture,
            Some(panel)
        );
        frame(
            &mut app,
            panel,
            vec![mouse(4, moved(Vec2::splat(900.0)))],
            true,
        );
        app.update();
        let guest = &app.world().resource::<Guest>().0;
        assert_eq!(guest.pointer, Vec2::splat(900.0));
        assert!(guest.keys.pressed(KeyCode::ShiftLeft));
        assert!(guest.buttons.pressed(MouseButton::Left));
        frame(
            &mut app,
            panel,
            vec![mouse(5, button(ButtonState::Released))],
            true,
        );
        app.update();
        assert!(
            app.world()
                .resource::<Guest>()
                .0
                .buttons
                .just_released(MouseButton::Left)
        );
        assert!(
            app.world()
                .resource::<InputRouter>()
                .pointer_capture
                .is_none()
        );
        app.world_mut()
            .get_mut::<PanelPresentation>(panel)
            .unwrap()
            .visible = false;
        frame(&mut app, panel, vec![], false);
        app.update();
        assert!(
            app.world()
                .resource::<Guest>()
                .0
                .keys
                .just_released(KeyCode::ShiftLeft)
        );
        assert!(
            !app.world().resource::<InputRouter>().panel_states[&panel]
                .keys
                .pressed(KeyCode::ShiftLeft)
        );
        // A held key's repeat remains owned until physical release, even after hiding.
        let mut repeated = key(6, true);
        if let WindowEvent::KeyboardInput(k) = &mut repeated.event {
            k.key_code = KeyCode::ShiftLeft;
            k.repeat = true;
        }
        frame(&mut app, panel, vec![repeated], false);
        app.update();
        assert!(app.world().resource::<RoutedInput>().events.is_empty());
    }
}
