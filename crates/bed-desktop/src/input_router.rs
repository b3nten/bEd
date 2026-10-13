//! Gameplay input ownership for Bevy's native window.
//! Winit normalizes input; the desktop routes it before Bevy's input systems run.
use super::DesktopCapture;
use bevy::prelude::Resource;
use bevy::window::{CursorMoved, Ime, WindowEvent as BevyWindowEvent};
use bevy::{
    ecs::{
        entity::Entity,
        message::{Message, MessageCursor, Messages},
        world::World,
    },
    input::{
        ButtonInput, ButtonState,
        keyboard::{KeyCode, KeyboardFocusLost, KeyboardInput},
        mouse::{MouseButton, MouseButtonInput, MouseMotion, MouseWheel},
    },
};
use winit::event::WindowEvent;

enum PendingInput {
    Keyboard(KeyboardInput, bool),
    Button(MouseButtonInput, bool),
    Motion(MouseMotion),
    Wheel(MouseWheel),
    Cursor(CursorMoved, bool),
    Ime(Ime, bool),
    Focus(bool),
    Window(BevyWindowEvent),
}

#[derive(Resource)]
pub(super) struct InputBridge {
    pending: Vec<PendingInput>,
    events: MessageCursor<BevyWindowEvent>,
    blocked_motion: bool,
    blocked_wheel: bool,
    collected: bool,
    observed: usize,
    window: Entity,
    focused: bool,
    capture: DesktopCapture,
    keys: std::collections::HashMap<KeyCode, KeyboardInput>,
    native_keys: std::collections::HashSet<KeyCode>,
    injected_keys: std::collections::HashSet<KeyCode>,
    native_buttons: std::collections::HashSet<MouseButton>,
    injected_buttons: std::collections::HashSet<MouseButton>,
    injected_modifiers: crate::DesktopModifiers,
    pointer: Option<bevy::math::Vec2>,
}

impl Default for InputBridge {
    fn default() -> Self {
        Self {
            pending: Vec::new(),
            events: MessageCursor::default(),
            blocked_motion: false,
            blocked_wheel: false,
            collected: false,
            observed: 0,
            window: Entity::PLACEHOLDER,
            focused: true,
            capture: DesktopCapture::default(),
            keys: std::collections::HashMap::new(),
            native_keys: Default::default(),
            injected_keys: Default::default(),
            native_buttons: Default::default(),
            injected_buttons: Default::default(),
            injected_modifiers: Default::default(),
            pointer: None,
        }
    }
}

impl InputBridge {
    /// Save this native batch before the UI establishes input capture. The
    /// aggregate preserves ordering between focus, keys, buttons and motion.
    pub(super) fn collect(&mut self, world: &World) {
        self.collected = true;
        self.observed = 0;
        self.blocked_motion = false;
        self.blocked_wheel = false;
        for event in self
            .events
            .read(world.resource::<Messages<BevyWindowEvent>>())
        {
            match event {
                BevyWindowEvent::KeyboardInput(input) => {
                    if input.state.is_pressed() {
                        self.native_keys.insert(input.key_code);
                    } else {
                        self.native_keys.remove(&input.key_code);
                    }
                    if self.injected_keys.contains(&input.key_code) && !input.repeat {
                        continue;
                    }
                    self.window = input.window;
                    self.pending
                        .push(PendingInput::Keyboard(input.clone(), false));
                }
                BevyWindowEvent::MouseButtonInput(input) => {
                    if input.state.is_pressed() {
                        self.native_buttons.insert(input.button);
                    } else {
                        self.native_buttons.remove(&input.button);
                    }
                    if self.injected_buttons.contains(&input.button) {
                        continue;
                    }
                    self.window = input.window;
                    self.pending.push(PendingInput::Button(*input, false));
                }
                BevyWindowEvent::MouseMotion(input) => {
                    self.pending.push(PendingInput::Motion(*input))
                }
                BevyWindowEvent::MouseWheel(input) => {
                    self.pending.push(PendingInput::Wheel(*input))
                }
                BevyWindowEvent::CursorMoved(input) => {
                    self.pointer = Some(input.position);
                    self.window = input.window;
                    self.pending
                        .push(PendingInput::Cursor(input.clone(), false));
                }
                BevyWindowEvent::Ime(input) => {
                    self.pending.push(PendingInput::Ime(input.clone(), false))
                }
                BevyWindowEvent::WindowFocused(input) => {
                    self.window = input.window;
                    self.pending.push(PendingInput::Window(event.clone()));
                    self.pending.push(PendingInput::Focus(input.focused));
                    if !input.focused {
                        self.native_keys.clear();
                        self.injected_keys.clear();
                        self.native_buttons.clear();
                        self.injected_buttons.clear();
                    }
                }
                BevyWindowEvent::KeyboardFocusLost(_) => {
                    self.native_keys.clear();
                    self.injected_keys.clear();
                    self.native_buttons.clear();
                    self.injected_buttons.clear();
                    self.pending.push(PendingInput::Focus(false))
                }
                _ => self.pending.push(PendingInput::Window(event.clone())),
            }
        }
    }

    /// Synthetic input joins the same normalized pending batch as native input.
    /// Ownership sets keep cleanup releases from releasing a held native key.
    pub(super) fn inject(
        &mut self,
        event: &crate::DesktopInjectedEvent,
        window: Entity,
        consumed: bool,
    ) {
        use crate::DesktopInjectedEvent;
        use bevy::input::mouse::MouseScrollUnit;
        use winit::keyboard::{NamedKey, PhysicalKey};
        self.window = window;
        match event {
            DesktopInjectedEvent::Key {
                physical,
                logical,
                pressed,
            } => {
                let key_code = bevy::winit::converters::convert_physical_key_code(
                    PhysicalKey::Code(*physical),
                );
                if *pressed {
                    self.injected_keys.insert(key_code);
                } else {
                    self.injected_keys.remove(&key_code);
                }
                if !self.native_keys.contains(&key_code) {
                    self.pending.push(PendingInput::Keyboard(
                        KeyboardInput {
                            key_code,
                            logical_key: bevy::winit::converters::convert_logical_key(logical),
                            state: if *pressed {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Released
                            },
                            text: None,
                            repeat: false,
                            window,
                        },
                        consumed,
                    ));
                }
            }
            DesktopInjectedEvent::PointerButton { button, pressed } => {
                let button = bevy::winit::converters::convert_mouse_button(*button);
                if *pressed {
                    self.injected_buttons.insert(button);
                } else {
                    self.injected_buttons.remove(&button);
                }
                if !self.native_buttons.contains(&button) {
                    self.pending.push(PendingInput::Button(
                        MouseButtonInput {
                            button,
                            state: if *pressed {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Released
                            },
                            window,
                        },
                        consumed,
                    ));
                }
            }
            DesktopInjectedEvent::PointerMoved(position) => {
                let position = bevy::math::Vec2::from_array(*position);
                let delta = self.pointer.map(|previous| position - previous);
                self.pointer = Some(position);
                self.pending.push(PendingInput::Cursor(
                    CursorMoved {
                        window,
                        position,
                        delta,
                    },
                    consumed,
                ));
            }
            DesktopInjectedEvent::Text(text) => self.pending.push(PendingInput::Ime(
                Ime::Commit {
                    window,
                    value: text.clone(),
                },
                consumed,
            )),
            DesktopInjectedEvent::Scroll(delta) => {
                if !consumed {
                    self.pending.push(PendingInput::Wheel(MouseWheel {
                        phase: bevy::input::touch::TouchPhase::Moved,
                        unit: MouseScrollUnit::Line,
                        x: delta[0],
                        y: delta[1],
                        window,
                    }));
                }
            }
            DesktopInjectedEvent::Modifiers(modifiers) => {
                let before = self.injected_modifiers;
                self.injected_modifiers = *modifiers;
                for (physical, named, was, pressed) in [
                    (
                        winit::keyboard::KeyCode::ShiftLeft,
                        NamedKey::Shift,
                        before.shift,
                        modifiers.shift,
                    ),
                    (
                        winit::keyboard::KeyCode::ControlLeft,
                        NamedKey::Control,
                        before.ctrl,
                        modifiers.ctrl,
                    ),
                    (
                        winit::keyboard::KeyCode::AltLeft,
                        NamedKey::Alt,
                        before.alt,
                        modifiers.alt,
                    ),
                    (
                        winit::keyboard::KeyCode::SuperLeft,
                        NamedKey::Super,
                        before.super_key,
                        modifiers.super_key,
                    ),
                ] {
                    if was != pressed {
                        self.inject(
                            &DesktopInjectedEvent::Key {
                                physical,
                                logical: winit::keyboard::Key::Named(named),
                                pressed,
                            },
                            window,
                            consumed,
                        );
                    }
                }
            }
        }
    }

    pub(super) fn observe(&mut self, event: &WindowEvent, consumed: bool) {
        match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                let code = bevy::winit::converters::convert_physical_key_code(event.physical_key);
                let state = if event.state.is_pressed() {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                if let Some((index, PendingInput::Keyboard(_, blocked))) = self.pending
                    .iter_mut().enumerate().skip(self.observed)
                    .find(|(_, pending)| matches!(pending, PendingInput::Keyboard(input, _) if input.key_code == code && input.state == state && input.repeat == event.repeat)) {
                    *blocked = consumed;
                    self.observed = index + 1;
                }
            }
            WindowEvent::Focused(focused) if !self.collected => {
                self.pending.push(PendingInput::Focus(*focused))
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = bevy::winit::converters::convert_mouse_button(*button);
                let state = if state.is_pressed() {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                if let Some((index, PendingInput::Button(_, blocked))) = self.pending.iter_mut().enumerate()
                    .skip(self.observed).find(|(_, pending)| matches!(pending, PendingInput::Button(input, _) if input.button == button && input.state == state)) {
                    *blocked = consumed;
                    self.observed = index + 1;
                }
            }
            WindowEvent::CursorMoved { .. } => {
                self.blocked_motion |= consumed;
                if let Some((index, PendingInput::Cursor(_, blocked))) = self
                    .pending
                    .iter_mut()
                    .enumerate()
                    .skip(self.observed)
                    .find(|(_, pending)| matches!(pending, PendingInput::Cursor(_, _)))
                {
                    *blocked = consumed;
                    self.observed = index + 1;
                }
            }
            WindowEvent::Ime(_) => {
                if let Some((index, PendingInput::Ime(_, blocked))) = self
                    .pending
                    .iter_mut()
                    .enumerate()
                    .skip(self.observed)
                    .find(|(_, pending)| matches!(pending, PendingInput::Ime(_, _)))
                {
                    *blocked = consumed;
                    self.observed = index + 1;
                }
            }
            WindowEvent::MouseWheel { .. } if consumed => self.blocked_wheel = true,
            _ => {}
        }
    }

    /// Call before InputSystems after the frontend has resolved capture.
    pub(super) fn flush(&mut self, world: &mut World, capture: DesktopCapture) {
        if self.collected {
            // The Winit runner already wrote these queues. Replace them with
            // this batch's routed events; draining and re-emitting old buffered
            // messages would process the previous frame twice.
            world.resource_mut::<Messages<KeyboardInput>>().clear();
            world.resource_mut::<Messages<KeyboardFocusLost>>().clear();
            world.resource_mut::<Messages<MouseButtonInput>>().clear();
            world.resource_mut::<Messages<MouseMotion>>().clear();
            world.resource_mut::<Messages<MouseWheel>>().clear();
            world.resource_mut::<Messages<CursorMoved>>().clear();
            world.resource_mut::<Messages<Ime>>().clear();
            world.resource_mut::<Messages<BevyWindowEvent>>().clear();
        }
        let mut buttons: Vec<_> = world
            .resource::<ButtonInput<MouseButton>>()
            .get_pressed()
            .copied()
            .collect();
        let mut reset_keyboard = false;
        if capture.keyboard && !self.capture.keyboard {
            release_keyboard(world, &mut self.keys);
            reset_keyboard = true;
        }
        if capture.pointer && !self.capture.pointer {
            release_mouse(world, &mut buttons, self.window);
        }
        self.capture = capture;
        for input in self.pending.drain(..) {
            match input {
                PendingInput::Keyboard(mut input, consumed) => {
                    if input.state == ButtonState::Released {
                        // Releasing Shift can change the logical value attached to
                        // a held physical key. Release the value we originally sent.
                        if let Some(pressed) = self.keys.remove(&input.key_code) {
                            input.logical_key = pressed.logical_key;
                        }
                        forward(world, input);
                    } else if self.focused && !capture.keyboard && !consumed {
                        if let Some(mut previous) = self.keys.insert(input.key_code, input.clone())
                            && previous.logical_key != input.logical_key
                        {
                            previous.state = ButtonState::Released;
                            previous.text = None;
                            previous.repeat = false;
                            forward(world, previous);
                        }
                        forward(world, input);
                    }
                }
                PendingInput::Button(input, consumed) => {
                    if input.state == ButtonState::Released
                        || (self.focused && !capture.pointer && !consumed)
                    {
                        if input.state == ButtonState::Pressed {
                            if !buttons.contains(&input.button) {
                                buttons.push(input.button);
                            }
                        } else {
                            buttons.retain(|button| *button != input.button);
                        }
                        forward(world, input);
                    }
                }
                PendingInput::Motion(input)
                    if self.focused && !capture.pointer && !self.blocked_motion =>
                {
                    forward(world, input);
                }
                PendingInput::Wheel(input)
                    if self.focused && !capture.pointer && !self.blocked_wheel =>
                {
                    forward(world, input);
                }
                PendingInput::Cursor(input, consumed)
                    if self.focused && !capture.pointer && !consumed =>
                {
                    forward(world, input);
                }
                PendingInput::Ime(input, consumed)
                    if matches!(input, Ime::Enabled { .. } | Ime::Disabled { .. })
                        || (self.focused && !capture.keyboard && !consumed) =>
                {
                    forward(world, input);
                }
                PendingInput::Window(event) => {
                    world
                        .resource_mut::<Messages<BevyWindowEvent>>()
                        .write(event);
                }
                PendingInput::Focus(focused) => {
                    reset_keyboard |= self.focused && !focused;
                    self.focused = focused;
                    if !focused {
                        release_keyboard(world, &mut self.keys);
                        release_mouse(world, &mut buttons, self.window);
                    }
                }
                _ => {}
            }
        }
        // Bevy applies KeyboardFocusLost after every keyboard event in the
        // batch. An intermediate loss must not reset presses accepted after
        // focus returns; explicit releases above preserve the event order.
        if reset_keyboard && (!self.focused || capture.keyboard) {
            forward(world, KeyboardFocusLost);
        }
        if self.collected {
            self.events = world
                .resource::<Messages<BevyWindowEvent>>()
                .get_cursor_current();
            self.collected = false;
        }
    }
}

fn release_keyboard(
    world: &mut World,
    keys: &mut std::collections::HashMap<KeyCode, KeyboardInput>,
) {
    for (_, mut input) in keys.drain() {
        input.state = ButtonState::Released;
        input.text = None;
        input.repeat = false;
        forward(world, input);
    }
}

fn release_mouse(world: &mut World, buttons: &mut Vec<MouseButton>, window: Entity) {
    for button in buttons.drain(..) {
        forward(
            world,
            MouseButtonInput {
                button,
                state: ButtonState::Released,
                window,
            },
        );
    }
}

fn forward<M: Message + Clone + Into<BevyWindowEvent>>(world: &mut World, input: M) {
    world.write_message(input.clone());
    if let Some(mut events) = world.get_resource_mut::<Messages<BevyWindowEvent>>() {
        events.write(input.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DesktopContext, DesktopInputEvents, DesktopPlatform, DesktopSystems};
    use bevy::{
        app::App,
        input::{
            keyboard::Key,
            mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
            touch::TouchPhase,
        },
        math::Vec2,
        prelude::*,
    };
    use dear_imgui_rs::Context;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        app.add_message::<BevyWindowEvent>();
        app.add_message::<CursorMoved>();
        app.add_message::<Ime>();
        app
    }

    fn key(state: ButtonState) -> PendingInput {
        PendingInput::Keyboard(
            KeyboardInput {
                key_code: KeyCode::KeyW,
                logical_key: Key::Character("w".into()),
                state,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            },
            false,
        )
    }

    fn mouse(state: ButtonState) -> PendingInput {
        PendingInput::Button(
            MouseButtonInput {
                button: MouseButton::Left,
                state,
                window: Entity::PLACEHOLDER,
            },
            false,
        )
    }

    #[derive(Resource, Default)]
    struct FrontendCapture(DesktopCapture);

    fn scheduled_app() -> App {
        let mut app = app();
        app.add_message::<bevy::winit::RawWinitWindowEvent>();
        let mut context = Context::create();
        let platform = DesktopPlatform::new(&mut context);
        app.insert_non_send(DesktopContext {
            context,
            platform,
            snapshot_context: None,
        });
        crate::install_input_routing(&mut app);
        app.init_resource::<FrontendCapture>().add_systems(
            PreUpdate,
            (|frontend: Res<FrontendCapture>, mut capture: ResMut<DesktopCapture>| {
                *capture = frontend.0;
            })
            .in_set(DesktopSystems::AfterUi),
        );
        app
    }

    fn native_buttons(world: &mut World, state: ButtonState) {
        let PendingInput::Keyboard(key, _) = key(state) else {
            unreachable!()
        };
        let PendingInput::Button(mouse, _) = mouse(state) else {
            unreachable!()
        };
        forward(world, key);
        forward(world, mouse);
    }

    #[test]
    fn scheduled_desktop_routing_applies_current_capture_and_releases_held_input() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = scheduled_app();
        app.world_mut().resource_mut::<FrontendCapture>().0 = DesktopCapture {
            keyboard: true,
            pointer: true,
        };
        native_buttons(app.world_mut(), ButtonState::Pressed);
        app.update();
        assert!(app.world().resource::<Messages<KeyboardInput>>().is_empty());
        assert!(
            app.world()
                .resource::<Messages<MouseButtonInput>>()
                .is_empty()
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );

        app.world_mut().resource_mut::<FrontendCapture>().0 = DesktopCapture::default();
        native_buttons(app.world_mut(), ButtonState::Pressed);
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );

        // Acquiring UI capture releases keys and mouse buttons without waiting
        // for physical releases, before Bevy updates gameplay input this frame.
        app.world_mut().resource_mut::<FrontendCapture>().0 = DesktopCapture {
            keyboard: true,
            pointer: true,
        };
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );

        // Physical releases still reach both typed and aggregate queues while
        // the UI captures presses; returning capture never replays old presses.
        native_buttons(app.world_mut(), ButtonState::Released);
        app.update();
        let keys = app.world().resource::<Messages<KeyboardInput>>();
        assert!(
            keys.get_cursor()
                .read(keys)
                .any(|input| input.state == ButtonState::Released)
        );
        let buttons = app.world().resource::<Messages<MouseButtonInput>>();
        assert!(
            buttons
                .get_cursor()
                .read(buttons)
                .any(|input| input.state == ButtonState::Released)
        );
        app.world_mut().resource_mut::<FrontendCapture>().0 = DesktopCapture::default();
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
    }

    #[test]
    fn scheduled_gameplay_consumption_survives_frontend_capture_changes() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = scheduled_app();
        app.add_systems(
            PreUpdate,
            (|mut events: ResMut<DesktopInputEvents>| {
                for event in &mut events.0 {
                    event.gameplay_consumed = true;
                }
            })
            .in_set(DesktopSystems::InterceptInput),
        );
        let PendingInput::Button(button, _) = mouse(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), button);
        app.world_mut()
            .write_message(bevy::winit::RawWinitWindowEvent {
                window_id: winit::window::WindowId::dummy(),
                event: WindowEvent::MouseInput {
                    device_id: winit::event::DeviceId::dummy(),
                    state: winit::event::ElementState::Pressed,
                    button: winit::event::MouseButton::Left,
                },
            });
        app.update();
        let events = &app.world().resource::<DesktopInputEvents>().0;
        assert_eq!(events.len(), 1);
        assert!(!events[0].consumed, "the event remains available to ImGui");
        assert!(events[0].gameplay_consumed);
        assert!(!app.world().resource::<DesktopCapture>().pointer);
        assert!(
            app.world()
                .resource::<Messages<MouseButtonInput>>()
                .is_empty()
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
    }

    #[test]
    fn normalized_batches_are_routed_once_before_input_systems() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        let PendingInput::Keyboard(input, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), input);
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::KeyW)
        );
        bridge.collect(app.world());
        assert!(
            bridge.pending.is_empty(),
            "routed events must not be read as a new native batch"
        );
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::KeyW)
        );
    }

    #[test]
    fn native_focus_order_releases_logical_keys_and_rejects_late_presses() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        let PendingInput::Keyboard(input, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), input.clone());
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        app.world_mut()
            .resource_mut::<Messages<BevyWindowEvent>>()
            .write(
                bevy::window::WindowFocused {
                    window: Entity::PLACEHOLDER,
                    focused: false,
                }
                .into(),
            );
        forward(app.world_mut(), input);
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
    }

    #[test]
    fn intermediate_focus_loss_preserves_presses_after_focus_returns() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        bridge
            .pending
            .extend([key(ButtonState::Pressed), mouse(ButtonState::Pressed)]);
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        bridge.pending.extend([
            PendingInput::Focus(false),
            PendingInput::Focus(true),
            key(ButtonState::Pressed),
            mouse(ButtonState::Pressed),
        ]);
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            app.world()
                .resource::<ButtonInput<MouseButton>>()
                .just_pressed(MouseButton::Left)
        );
    }

    #[test]
    fn winit_last_focus_marker_does_not_reset_the_next_refocused_batch() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        let PendingInput::Keyboard(press, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), press.clone());
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        let focus = |world: &mut World, focused| {
            world.resource_mut::<Messages<BevyWindowEvent>>().write(
                bevy::window::WindowFocused {
                    window: Entity::PLACEHOLDER,
                    focused,
                }
                .into(),
            );
        };
        focus(app.world_mut(), false);
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        // Winit's check_keyboard_focus_lost runs in Last, after InputSystems.
        // Its marker and synthetic releases precede the next native batch.
        forward(app.world_mut(), KeyboardFocusLost);
        let mut release = press.clone();
        release.state = ButtonState::Released;
        forward(app.world_mut(), release);
        focus(app.world_mut(), true);
        forward(app.world_mut(), press);
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
    }

    #[test]
    fn cursor_capture_and_consumption_agree_in_typed_and_aggregate_queues() {
        for (capture, consumed, expected) in [(true, false, 0), (false, true, 0), (false, false, 1)]
        {
            let mut app = app();
            let mut bridge = InputBridge::default();
            let cursor = CursorMoved {
                window: Entity::PLACEHOLDER,
                position: Vec2::new(24.0, 36.0),
                delta: Some(Vec2::ONE),
            };
            forward(app.world_mut(), cursor.clone());
            app.world_mut()
                .resource_mut::<Messages<BevyWindowEvent>>()
                .write(
                    bevy::window::WindowFocused {
                        window: Entity::PLACEHOLDER,
                        focused: true,
                    }
                    .into(),
                );
            bridge.collect(app.world());
            bridge.observe(
                &WindowEvent::CursorMoved {
                    device_id: winit::event::DeviceId::dummy(),
                    position: winit::dpi::PhysicalPosition::new(24.0, 36.0),
                },
                consumed,
            );
            bridge.flush(
                app.world_mut(),
                DesktopCapture {
                    pointer: capture,
                    keyboard: false,
                },
            );
            let typed = app.world().resource::<Messages<CursorMoved>>();
            assert_eq!(typed.len(), expected);
            let aggregate = app.world().resource::<Messages<BevyWindowEvent>>();
            let mut reader = aggregate.get_cursor();
            let events: Vec<_> = reader.read(aggregate).collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, BevyWindowEvent::CursorMoved(_)))
                    .count(),
                expected
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, BevyWindowEvent::WindowFocused(_)))
            );
            bridge.collect(app.world());
            bridge.flush(app.world_mut(), DesktopCapture::default());
            assert!(
                app.world().resource::<Messages<CursorMoved>>().is_empty(),
                "cursor input must not replay on the next frame"
            );
        }
    }

    #[test]
    fn consuming_one_native_cursor_does_not_discard_the_next_cursor_event() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        for x in [10.0, 20.0] {
            forward(
                app.world_mut(),
                CursorMoved {
                    window: Entity::PLACEHOLDER,
                    position: Vec2::new(x, 0.0),
                    delta: None,
                },
            );
        }
        bridge.collect(app.world());
        for (x, consumed) in [(10.0, true), (20.0, false)] {
            bridge.observe(
                &WindowEvent::CursorMoved {
                    device_id: winit::event::DeviceId::dummy(),
                    position: winit::dpi::PhysicalPosition::new(x, 0.0),
                },
                consumed,
            );
        }
        bridge.flush(app.world_mut(), DesktopCapture::default());
        let typed = app.world().resource::<Messages<CursorMoved>>();
        assert_eq!(typed.len(), 1);
        assert_eq!(
            typed.get_cursor().read(typed).next().unwrap().position.x,
            20.0
        );
    }

    #[test]
    fn ime_text_follows_keyboard_capture_and_consumption_but_lifecycle_facts_survive() {
        for (capture, consumed, text_count) in
            [(true, false, 0), (false, true, 0), (false, false, 2)]
        {
            let mut app = app();
            let mut bridge = InputBridge::default();
            let window = Entity::PLACEHOLDER;
            for event in [
                Ime::Enabled { window },
                Ime::Preedit {
                    window,
                    value: "é".into(),
                    cursor: Some((0, 2)),
                },
                Ime::Commit {
                    window,
                    value: "é".into(),
                },
                Ime::Disabled { window },
            ] {
                forward(app.world_mut(), event);
            }
            bridge.collect(app.world());
            for event in [
                winit::event::Ime::Enabled,
                winit::event::Ime::Preedit("é".into(), Some((0, 2))),
                winit::event::Ime::Commit("é".into()),
                winit::event::Ime::Disabled,
            ] {
                bridge.observe(&WindowEvent::Ime(event), consumed);
            }
            bridge.flush(
                app.world_mut(),
                DesktopCapture {
                    keyboard: capture,
                    pointer: false,
                },
            );
            let typed = app.world().resource::<Messages<Ime>>();
            assert_eq!(typed.len(), 2 + text_count);
            let aggregate = app.world().resource::<Messages<BevyWindowEvent>>();
            let mut reader = aggregate.get_cursor();
            let events: Vec<_> = reader.read(aggregate).collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        BevyWindowEvent::Ime(Ime::Preedit { .. } | Ime::Commit { .. })
                    ))
                    .count(),
                text_count
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, BevyWindowEvent::Ime(Ime::Enabled { .. })))
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, BevyWindowEvent::Ime(Ime::Disabled { .. })))
            );
        }
    }

    #[test]
    fn standard_input_updates_once_and_capture_releases_held_buttons() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        bridge.pending.extend([
            key(ButtonState::Pressed),
            mouse(ButtonState::Pressed),
            PendingInput::Motion(MouseMotion {
                delta: Vec2::new(2.0, 3.0),
            }),
        ]);
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .just_pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            app.world()
                .resource::<ButtonInput<MouseButton>>()
                .just_pressed(MouseButton::Left)
        );
        assert_eq!(
            app.world().resource::<AccumulatedMouseMotion>().delta,
            Vec2::new(2.0, 3.0)
        );
        bridge.pending.extend([
            key(ButtonState::Pressed),
            mouse(ButtonState::Pressed),
            PendingInput::Motion(MouseMotion { delta: Vec2::ONE }),
        ]);
        bridge.flush(
            app.world_mut(),
            DesktopCapture {
                keyboard: true,
                pointer: true,
            },
        );
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
        assert_eq!(
            app.world().resource::<AccumulatedMouseMotion>().delta,
            Vec2::ZERO
        );
    }

    #[test]
    fn consumed_presses_are_filtered_and_releases_pass_capture() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        let PendingInput::Keyboard(input, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        bridge.pending.push(PendingInput::Keyboard(input, true));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        bridge.pending.push(key(ButtonState::Pressed));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        bridge.pending.push(key(ButtonState::Released));
        bridge.flush(
            app.world_mut(),
            DesktopCapture {
                keyboard: true,
                pointer: false,
            },
        );
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
    }

    #[test]
    fn focus_loss_releases_keys_and_mouse_and_blocks_scroll() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        bridge
            .pending
            .extend([key(ButtonState::Pressed), mouse(ButtonState::Pressed)]);
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        bridge.observe(&WindowEvent::Focused(false), false);
        bridge.pending.push(PendingInput::Wheel(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 3.0,
            phase: TouchPhase::Moved,
            window: Entity::PLACEHOLDER,
        }));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
        assert_eq!(
            app.world().resource::<AccumulatedMouseScroll>().delta,
            Vec2::ZERO
        );
    }

    #[test]
    fn focus_loss_in_the_press_batch_does_not_leave_logical_or_mouse_buttons_held() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        bridge.pending.extend([
            key(ButtonState::Pressed),
            mouse(ButtonState::Pressed),
            PendingInput::Focus(false),
        ]);
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
    }

    #[test]
    fn injected_key_text_pointer_and_wheel_use_current_capture_without_replay() {
        use crate::DesktopInjectedEvent as E;
        for captured in [false, true] {
            let mut app = app();
            let mut bridge = InputBridge::default();
            bridge.collect(app.world());
            let event = E::Key {
                physical: winit::keyboard::KeyCode::KeyW,
                logical: winit::keyboard::Key::Character("w".into()),
                pressed: true,
            };
            bridge.inject(&event, Entity::PLACEHOLDER, false);
            bridge.inject(&E::Text("w".into()), Entity::PLACEHOLDER, false);
            bridge.inject(&E::PointerMoved([12., 24.]), Entity::PLACEHOLDER, false);
            bridge.inject(&E::Scroll([0., 2.]), Entity::PLACEHOLDER, false);
            bridge.flush(
                app.world_mut(),
                DesktopCapture {
                    keyboard: captured,
                    pointer: captured,
                },
            );
            let keys = app.world().resource::<Messages<KeyboardInput>>();
            assert!(
                keys.get_cursor()
                    .read(keys)
                    .all(|event| event.text.is_none())
            );
            assert_eq!(
                app.world().resource::<Messages<Ime>>().len(),
                usize::from(!captured)
            );
            assert_eq!(
                app.world().resource::<Messages<CursorMoved>>().len(),
                usize::from(!captured)
            );
            app.update();
            assert_eq!(
                app.world()
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::KeyW),
                !captured
            );
            bridge.collect(app.world());
            bridge.inject(
                &E::Key {
                    physical: winit::keyboard::KeyCode::KeyW,
                    logical: winit::keyboard::Key::Character("w".into()),
                    pressed: false,
                },
                Entity::PLACEHOLDER,
                true,
            );
            bridge.flush(
                app.world_mut(),
                DesktopCapture {
                    keyboard: true,
                    pointer: true,
                },
            );
            app.update();
            assert!(
                !app.world()
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::KeyW)
            );
            bridge.collect(app.world());
            bridge.flush(app.world_mut(), DesktopCapture::default());
            assert!(app.world().resource::<Messages<Ime>>().is_empty());
        }
    }

    #[test]
    fn injected_cleanup_keeps_native_key_and_button_held_until_native_release() {
        use crate::DesktopInjectedEvent as E;
        let mut app = app();
        let mut bridge = InputBridge::default();
        let PendingInput::Keyboard(native, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), native.clone());
        let PendingInput::Button(button, _) = mouse(ButtonState::Pressed) else {
            unreachable!()
        };
        forward(app.world_mut(), button);
        bridge.collect(app.world());
        bridge.inject(
            &E::Key {
                physical: winit::keyboard::KeyCode::KeyW,
                logical: winit::keyboard::Key::Character("w".into()),
                pressed: true,
            },
            Entity::PLACEHOLDER,
            false,
        );
        bridge.inject(
            &E::PointerButton {
                button: winit::event::MouseButton::Left,
                pressed: true,
            },
            Entity::PLACEHOLDER,
            false,
        );
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        bridge.collect(app.world());
        bridge.inject(
            &E::Key {
                physical: winit::keyboard::KeyCode::KeyW,
                logical: winit::keyboard::Key::Character("w".into()),
                pressed: false,
            },
            Entity::PLACEHOLDER,
            false,
        );
        bridge.inject(
            &E::PointerButton {
                button: winit::event::MouseButton::Left,
                pressed: false,
            },
            Entity::PLACEHOLDER,
            false,
        );
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
        let mut release = native;
        release.state = ButtonState::Released;
        forward(app.world_mut(), release);
        forward(
            app.world_mut(),
            MouseButtonInput {
                state: ButtonState::Released,
                ..button
            },
        );
        bridge.collect(app.world());
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            !app.world()
                .resource::<ButtonInput<MouseButton>>()
                .pressed(MouseButton::Left)
        );
    }

    #[test]
    fn modifier_changes_do_not_leave_previous_logical_values_held() {
        let mut app = app();
        let mut bridge = InputBridge::default();
        bridge.pending.push(key(ButtonState::Pressed));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        let PendingInput::Keyboard(mut repeat, _) = key(ButtonState::Pressed) else {
            unreachable!()
        };
        repeat.repeat = true;
        repeat.logical_key = Key::Character("W".into());
        bridge.pending.push(PendingInput::Keyboard(repeat, false));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("w".into()))
        );
        assert!(
            app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("W".into()))
        );
        bridge.pending.push(key(ButtonState::Released));
        bridge.flush(app.world_mut(), DesktopCapture::default());
        app.update();
        assert!(
            !app.world()
                .resource::<ButtonInput<Key>>()
                .pressed(Key::Character("W".into()))
        );
    }
}
