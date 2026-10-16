//! Ordered input and explicit claims. Raw Bevy messages remain available for platform integration.
use bevy::{
    input::{
        ButtonState,
        keyboard::{KeyCode, KeyboardInput},
    },
    prelude::*,
    window::WindowEvent,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputModifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Debug)]
pub struct InputEvent {
    pub id: u64,
    pub event: WindowEvent,
    pub modifiers: InputModifiers,
}

impl InputEvent {
    pub fn key(&self) -> Option<&KeyboardInput> {
        if let WindowEvent::KeyboardInput(k) = &self.event {
            Some(k)
        } else {
            None
        }
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct RoutedInput {
    pub events: Vec<InputEvent>,
    pub keys: ButtonInput<KeyCode>,
    pub buttons: ButtonInput<MouseButton>,
    pub pointer: Vec2,
    pub wheel: Vec2,
}

impl RoutedInput {
    pub fn begin_frame(&mut self) {
        self.events.clear();
        self.keys.clear();
        self.buttons.clear();
        self.wheel = Vec2::ZERO;
    }

    pub fn admit(&mut self, event: InputEvent) {
        match &event.event {
            WindowEvent::KeyboardInput(k) => match k.state {
                ButtonState::Pressed => self.keys.press(k.key_code),
                ButtonState::Released => self.keys.release(k.key_code),
            },
            WindowEvent::MouseButtonInput(b) => match b.state {
                ButtonState::Pressed => self.buttons.press(b.button),
                ButtonState::Released => self.buttons.release(b.button),
            },
            WindowEvent::CursorMoved(p) => self.pointer = p.position,
            WindowEvent::MouseWheel(w) => self.wheel += Vec2::new(w.x, w.y),
            WindowEvent::WindowFocused(f) if !f.focused => self.release_all(),
            _ => {}
        }
        self.events.push(event);
    }

    pub fn release_all(&mut self) {
        self.keys.release_all();
        self.buttons.release_all();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InputRegion {
    pub owner: Entity,
    pub id: egui::Id,
    pub rect: egui::Rect,
}

#[derive(Default)]
pub struct UiInputFrame {
    pub events: Vec<InputEvent>,
    pub claims: HashMap<u64, InputOwner>,
    pub regions: Vec<InputRegion>,
    pub text_widgets: HashSet<egui::Id>,
    pub focused_content: Option<Entity>,
    pub modal: bool,
    pub text_active: bool,
}

impl UiInputFrame {
    pub fn consume(&mut self, event: u64, owner: Entity) {
        let _ = owner;
        self.claims.entry(event).or_insert(InputOwner::Ui);
    }
    /// Egui widgets own pointer gestures; explicitly delegated image regions do not.
    pub(crate) fn claim_widgets(&mut self, ctx: &egui::Context) {
        let (active, candidates) = ctx.interaction_snapshot(|i| {
            (
                [i.clicked, i.dragged, i.drag_stopped],
                i.contains_pointer.iter().copied().collect::<Vec<_>>(),
            )
        });
        let delegated = |id| self.regions.iter().any(|r| r.id == id);
        let used = active.into_iter().flatten().any(|id| !delegated(id))
            || candidates.into_iter().any(|id| {
                !delegated(id)
                    && ctx.read_response(id).is_some_and(|r| {
                        r.sense.interactive() && (r.hovered() || r.is_pointer_button_down_on())
                    })
            });
        if used {
            let ids = self
                .events
                .iter()
                .filter(|e| {
                    matches!(
                        e.event,
                        WindowEvent::CursorMoved(_)
                            | WindowEvent::MouseButtonInput(_)
                            | WindowEvent::MouseWheel(_)
                    )
                })
                .map(|e| e.id)
                .collect::<Vec<_>>();
            for id in ids {
                self.claims.entry(id).or_insert(InputOwner::Ui);
            }
        }
    }
    pub fn claim_text(&mut self, owner: Entity, id: egui::Id) {
        self.text_widgets.insert(id);
        let ids: Vec<_> = self
            .events
            .iter()
            .filter(|e| match &e.event {
                WindowEvent::Ime(_) => true,
                WindowEvent::KeyboardInput(k) => editing_key(k, e.modifiers),
                _ => false,
            })
            .map(|e| e.id)
            .collect();
        for id in ids {
            self.consume(id, owner);
        }
    }
    pub fn region(&mut self, owner: Entity, id: egui::Id, rect: egui::Rect, clicked: bool) {
        self.regions.push(InputRegion { owner, id, rect });
        if clicked {
            self.focused_content = Some(owner);
        }
    }
}

/// Conservatively reserve editing gestures, including no-op edits and AltGr text.
fn editing_key(k: &KeyboardInput, m: InputModifiers) -> bool {
    use KeyCode::*;
    if !m.meta && (!m.control || m.alt) && k.text.as_ref().is_some_and(|t| !t.is_empty()) {
        return true;
    }
    if matches!(
        k.key_code,
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
            | NumpadEnter
            | Tab
            | Escape
    ) {
        return true;
    }
    let primary = if cfg!(target_os = "macos") {
        m.meta
    } else {
        m.control
    };
    (primary && matches!(k.key_code, KeyA | KeyC | KeyX | KeyV | KeyZ | KeyY))
        || (m.control
            && matches!(
                k.key_code,
                KeyA | KeyE | KeyB | KeyF | KeyP | KeyN | KeyH | KeyK | KeyU | KeyW
            ))
}

#[derive(Resource, Default)]
pub struct InputRouter {
    pub frame: std::sync::Mutex<UiInputFrame>,
    pub pointer_position: Vec2,
    pub keyboard_capture: Option<Entity>,
    pub pointer_capture: Option<Entity>,
    pub panel_states: HashMap<Entity, RoutedInput>,
    /// Panels awaiting a released-state notification at the next publication.
    pub cancelled_panels: HashSet<Entity>,
    pub key_owners: HashMap<KeyCode, InputOwner>,
    pub button_owners: HashMap<MouseButton, InputOwner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOwner {
    Ui,
    Panel(Entity),
    Command,
    Application,
}

impl InputRouter {
    pub fn cancel_panel(&mut self, panel: Entity) {
        let needs_notification = self.keyboard_capture == Some(panel)
            || self.pointer_capture == Some(panel)
            || self.panel_states.get(&panel).is_some_and(|s| {
                s.keys.get_pressed().next().is_some() || s.buttons.get_pressed().next().is_some()
            });
        if self.frame.get_mut().unwrap().focused_content == Some(panel) {
            self.frame.get_mut().unwrap().focused_content = None;
        }
        if self.keyboard_capture == Some(panel) {
            self.keyboard_capture = None;
        }
        if self.pointer_capture == Some(panel) {
            self.pointer_capture = None;
        }
        if needs_notification {
            self.cancelled_panels.insert(panel);
        }
        for owner in self
            .key_owners
            .values_mut()
            .chain(self.button_owners.values_mut())
        {
            if *owner == InputOwner::Panel(panel) {
                *owner = InputOwner::Ui;
            }
        }
    }
    /// Commit ownership once, after UI, panel handlers and shortcut resolution.
    pub fn publish(&mut self, application: &mut RoutedInput) {
        application.begin_frame();
        for state in self.panel_states.values_mut() {
            state.begin_frame();
        }
        for panel in self.cancelled_panels.drain() {
            if let Some(state) = self.panel_states.get_mut(&panel) {
                state.release_all();
            }
        }
        let events = self.frame.get_mut().unwrap().events.clone();
        let claims = self.frame.get_mut().unwrap().claims.clone();
        for event in events {
            let requested = claims
                .get(&event.id)
                .copied()
                .unwrap_or(InputOwner::Application);
            let owner = match &event.event {
                WindowEvent::WindowFocused(f) if !f.focused => {
                    application.release_all();
                    for state in self.panel_states.values_mut() {
                        state.release_all();
                    }
                    self.key_owners.clear();
                    self.button_owners.clear();
                    self.keyboard_capture = None;
                    self.pointer_capture = None;
                    InputOwner::Application
                }
                WindowEvent::KeyboardInput(k) => {
                    if k.state == ButtonState::Released {
                        self.key_owners
                            .remove(&k.key_code)
                            .unwrap_or(InputOwner::Ui)
                    } else {
                        let previous = self.key_owners.get(&k.key_code).copied();
                        let owner = if let Some(previous) = previous {
                            if requested == InputOwner::Ui && previous != InputOwner::Ui {
                                match previous {
                                    InputOwner::Application => application.keys.release(k.key_code),
                                    InputOwner::Panel(p) => {
                                        if let Some(s) = self.panel_states.get_mut(&p) {
                                            s.keys.release(k.key_code);
                                        }
                                    }
                                    _ => {}
                                }
                                InputOwner::Ui
                            } else {
                                previous
                            }
                        } else {
                            requested
                        };
                        self.key_owners.insert(k.key_code, owner);
                        owner
                    }
                }
                WindowEvent::MouseButtonInput(b) => {
                    if b.state == ButtonState::Released {
                        self.button_owners
                            .remove(&b.button)
                            .unwrap_or(InputOwner::Ui)
                    } else {
                        *self.button_owners.entry(b.button).or_insert(requested)
                    }
                }
                _ => requested,
            };
            match owner {
                InputOwner::Application => application.admit(event),
                InputOwner::Panel(panel) => {
                    self.panel_states.entry(panel).or_default().admit(event)
                }
                InputOwner::Ui | InputOwner::Command => {}
            }
        }
    }
}

pub(crate) fn publish(world: &mut World) {
    let mut router = world.remove_resource::<InputRouter>().unwrap();
    let mut application = world.remove_resource::<RoutedInput>().unwrap();
    router.publish(&mut application);
    world.insert_resource(application);
    world.insert_resource(router);
}
