use crate::*;
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardFocusLost, KeyboardInput, NativeKey, NativeKeyCode},
        mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel},
    },
    window::{CursorMoved, WindowEvent, WindowFocused},
};
use serde::Deserialize;
use std::collections::HashSet;

pub(crate) const KINDS: &[&str] = &[
    "move",
    "motion",
    "button",
    "key",
    "text",
    "scroll",
    "wait",
    "click",
    "click_text",
    "drag",
];
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    Move {
        position: [f32; 2],
    },
    Motion {
        delta: [f32; 2],
    },
    Button {
        button: MouseButton,
        pressed: bool,
    },
    Key {
        code: KeyCode,
        pressed: bool,
    },
    Text {
        text: String,
    },
    Scroll {
        delta: [f32; 2],
        #[serde(default)]
        pixels: bool,
    },
    Wait {
        frames: u32,
    },
    Click {
        position: [f32; 2],
        #[serde(default = "left")]
        button: MouseButton,
    },
    ClickText {
        text: String,
        #[serde(default)]
        index: usize,
    },
    Drag {
        from: [f32; 2],
        to: [f32; 2],
        #[serde(default = "drag_frames")]
        frames: u32,
    },
}
fn left() -> MouseButton {
    MouseButton::Left
}
fn drag_frames() -> u32 {
    12
}
#[derive(Clone)]
pub(crate) enum Event {
    Move(Vec2),
    Motion(Vec2),
    Button(MouseButton, bool),
    Key(KeyCode, bool),
    Text(String),
    Scroll(Vec2, bool),
}
#[derive(Default)]
pub(crate) struct Held {
    window: Option<Entity>,
    keys: HashSet<KeyCode>,
    buttons: HashSet<MouseButton>,
}

pub(crate) fn prepare(
    world: &mut World,
    args: &Value,
    _state: &State,
) -> Result<VecDeque<Vec<Event>>, String> {
    let steps: Vec<Step> = required(args, "steps")?;
    if steps.len() > 1024 {
        return Err("At most 1024 input steps".into());
    }
    let mut frames = VecDeque::new();
    let valid = |p: [f32; 2]| -> Result<Vec2, String> {
        if p.into_iter().all(|v| v.is_finite() && v.abs() <= 100000.0) {
            Ok(Vec2::from_array(p))
        } else {
            Err("Invalid input coordinates".into())
        }
    };
    for step in steps {
        match step {
            Step::Move { position } => frames.push_back(vec![Event::Move(valid(position)?)]),
            Step::Motion { delta } => frames.push_back(vec![Event::Motion(valid(delta)?)]),
            Step::Button { button, pressed } => {
                frames.push_back(vec![Event::Button(button, pressed)])
            }
            Step::Key { code, pressed } => frames.push_back(vec![Event::Key(code, pressed)]),
            Step::Text { text } => frames.push_back(vec![Event::Text(text)]),
            Step::Scroll { delta, pixels } => {
                frames.push_back(vec![Event::Scroll(valid(delta)?, pixels)])
            }
            Step::Wait { frames: count } => {
                if count > 10000 {
                    return Err("Input wait is at most 10000 frames".into());
                }
                frames.extend((0..count).map(|_| vec![]));
            }
            Step::Click { position, button } => {
                frames.push_back(vec![Event::Move(valid(position)?)]);
                frames.push_back(vec![Event::Button(button, true)]);
                frames.push_back(vec![Event::Button(button, false)]);
            }
            Step::ClickText { text, index } => {
                #[cfg(feature = "ui")]
                {
                    let window = crate::window(world, args)?;
                    let matches: Vec<_> = _state
                        .ui
                        .texts
                        .iter()
                        .filter(|t| t.window == window && t.text == text)
                        .collect();
                    let target = matches.get(index).ok_or_else(|| {
                        format!("No visible text {text:?} at index {index}; inspect ui.texts")
                    })?;
                    frames.push_back(vec![Event::Move(target.rect.center())]);
                    frames.push_back(vec![Event::Button(MouseButton::Left, true)]);
                    frames.push_back(vec![Event::Button(MouseButton::Left, false)]);
                }
                #[cfg(not(feature = "ui"))]
                {
                    let _ = (world, text, index);
                    return Err("click_text requires the ui feature".into());
                }
            }
            Step::Drag {
                from,
                to,
                frames: count,
            } => {
                if !(1..=1000).contains(&count) {
                    return Err("Drag frames must be in 1..=1000".into());
                }
                let from = valid(from)?;
                let to = valid(to)?;
                frames.push_back(vec![Event::Move(from)]);
                frames.push_back(vec![Event::Button(MouseButton::Left, true)]);
                for n in 1..=count {
                    frames.push_back(vec![Event::Move(from.lerp(to, n as f32 / count as f32))]);
                }
                frames.push_back(vec![Event::Button(MouseButton::Left, false)]);
            }
        }
        if frames.len() > 10000 {
            return Err("Input sequence exceeds 10000 frames".into());
        }
    }
    Ok(frames)
}

pub(crate) fn isolate(world: &mut World) {
    if let Some(mut messages) = world.get_resource_mut::<Messages<WindowEvent>>() {
        // Messages has no retain API. Suppressed input becomes a harmless
        // redraw notification, preserving lifecycle message IDs and aging.
        // Draining/requeuing the retained history would replay resize/close
        // events and keep them alive indefinitely.
        let mut cursor = messages.get_cursor();
        for event in cursor.read_mut(&mut messages) {
            if matches!(
                event,
                WindowEvent::CursorMoved(_)
                    | WindowEvent::CursorEntered(_)
                    | WindowEvent::CursorLeft(_)
                    | WindowEvent::KeyboardInput(_)
                    | WindowEvent::MouseButtonInput(_)
                    | WindowEvent::MouseWheel(_)
                    | WindowEvent::MouseMotion(_)
                    | WindowEvent::Ime(_)
                    | WindowEvent::WindowFocused(_)
                    | WindowEvent::KeyboardFocusLost(_)
            ) {
                *event = WindowEvent::RequestRedraw(bevy::window::RequestRedraw);
            }
        }
    }
    macro_rules! clear { ($($t:ty),*) => { $(if let Some(mut m) = world.get_resource_mut::<Messages<$t>>() { m.clear(); })* }; }
    clear!(
        KeyboardInput,
        MouseButtonInput,
        MouseWheel,
        MouseMotion,
        CursorMoved,
        KeyboardFocusLost
    );
    #[cfg(feature = "ui")]
    if let Some(mut events) = world.get_resource_mut::<Messages<bevy_egui::input::EguiInputEvent>>()
    {
        events.clear();
    }
}
pub(crate) fn focus(world: &mut World, window: Entity) {
    if let Some(mut w) = world.get_mut::<Window>(window) {
        w.focused = true;
    }
    let event = WindowFocused {
        window,
        focused: true,
    };
    if let Some(mut messages) = world.get_resource_mut::<Messages<WindowFocused>>() {
        messages.write(event.clone());
    }
    if let Some(mut messages) = world.get_resource_mut::<Messages<WindowEvent>>() {
        messages.write(WindowEvent::WindowFocused(event));
    }
}
fn keyboard(window: Entity, code: KeyCode, pressed: bool, text: Option<String>) -> KeyboardInput {
    let logical_key = if let Some(text) = &text {
        Key::Character(text.clone().into())
    } else {
        match code {
            KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
            KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
            KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
            KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
            KeyCode::Enter => Key::Enter,
            KeyCode::Tab => Key::Tab,
            KeyCode::Space => Key::Unidentified(NativeKey::Unidentified),
            KeyCode::Escape => Key::Escape,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::ArrowLeft => Key::ArrowLeft,
            KeyCode::ArrowRight => Key::ArrowRight,
            KeyCode::ArrowUp => Key::ArrowUp,
            KeyCode::ArrowDown => Key::ArrowDown,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            _ => Key::Unidentified(NativeKey::Unidentified),
        }
    };
    KeyboardInput {
        window,
        key_code: code,
        logical_key,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
        text: text.map(Into::into),
        repeat: false,
    }
}
pub(crate) fn emit(world: &mut World, window: Entity, events: Vec<Event>, held: &mut Held) {
    held.window = Some(window);
    for event in events {
        let native = match event {
            Event::Motion(delta) => {
                let event = MouseMotion { delta };
                world.write_message(event);
                WindowEvent::MouseMotion(event)
            }
            Event::Move(position) => {
                let e = CursorMoved {
                    window,
                    position,
                    delta: None,
                };
                world.write_message(e.clone());
                WindowEvent::CursorMoved(e)
            }
            Event::Button(button, pressed) => {
                if pressed {
                    held.buttons.insert(button);
                } else {
                    held.buttons.remove(&button);
                }
                let e = MouseButtonInput {
                    window,
                    button,
                    state: if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                };
                world.write_message(e);
                WindowEvent::MouseButtonInput(e)
            }
            Event::Key(code, pressed) => {
                if pressed {
                    held.keys.insert(code);
                } else {
                    held.keys.remove(&code);
                }
                let e = keyboard(window, code, pressed, None);
                world.write_message(e.clone());
                WindowEvent::KeyboardInput(e)
            }
            Event::Text(text) => {
                let code = KeyCode::Unidentified(NativeKeyCode::Unidentified);
                let e = keyboard(window, code, true, Some(text));
                world.write_message(e.clone());
                world.write_message(WindowEvent::KeyboardInput(e));
                let e = keyboard(window, code, false, None);
                world.write_message(e.clone());
                WindowEvent::KeyboardInput(e)
            }
            Event::Scroll(delta, pixels) => {
                let e = MouseWheel {
                    window,
                    x: delta.x,
                    y: delta.y,
                    unit: if pixels {
                        MouseScrollUnit::Pixel
                    } else {
                        MouseScrollUnit::Line
                    },
                    phase: bevy::input::touch::TouchPhase::Moved,
                };
                world.write_message(e);
                WindowEvent::MouseWheel(e)
            }
        };
        world.write_message(native);
    }
}
pub(crate) fn release(world: &mut World, held: &mut Held) {
    if let Some(window) = held.window {
        let events = held
            .keys
            .drain()
            .map(|k| Event::Key(k, false))
            .chain(held.buttons.drain().map(|b| Event::Button(b, false)))
            .collect();
        emit(world, window, events, held);
    }
}
