//! The remote methods and the systems that carry them out.
use bedtime_ui::{AccessibilityTree, egui::accesskit};
use bedtime_workbench::{ActionRegistry, ActionRequest, ActionRequests, InvocationContext};
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput, NativeKey, NativeKeyCode},
        mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel},
        touch::TouchPhase,
    },
    prelude::*,
    remote::{BrpError, BrpReceiver, BrpResult, RemotePlugin, error_codes, http::RemoteHttpPlugin},
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    window::{CursorMoved, PrimaryWindow, WindowEvent},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf};

/// One input step; a script's steps run one per frame, so each is seen by the
/// UI before the next. As JSON: `{"move": {"x": 10, "y": 20}}` (logical
/// pixels from the window's top left), `{"press": "Left"}`, `{"release":
/// "Left"}`, `{"key_down": "KeyA"}`, `{"key_up": "SuperLeft"}` (Bevy
/// `KeyCode` names), `{"text": "typed"}`, `{"scroll": {"x": 0, "y": -40}}`
/// (pixels), `{"wait": 3}` (frames).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Move { x: f32, y: f32 },
    Press(MouseButton),
    Release(MouseButton),
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Text(String),
    Scroll { x: f32, y: f32 },
    Wait(u32),
}

#[derive(Resource, Default)]
struct Automation {
    /// Which App of this process this is, from 1.
    launch: u64,
    /// Frames finished since startup.
    frame: u64,
    steps: VecDeque<Step>,
    /// Where the last `move` left the pointer.
    cursor: Vec2,
    /// The window `pretend_focus` marked focused this frame.
    pretended: Option<Entity>,
    /// Screenshots written since startup, by path.
    screenshots: Vec<PathBuf>,
    /// Failures of queued work, reported by `status`.
    errors: Vec<String>,
}

/// The request mailbox of the HTTP server this process started. The server
/// outlives an App (it runs on Bevy's global task pool), so when an App exits
/// its mailbox is kept here for the next App in the same process (after a
/// relaunch), which serves the same port.
static MAILBOX: std::sync::Mutex<Option<BrpReceiver>> = std::sync::Mutex::new(None);

/// How many Apps this process has built, so scripts can tell when a relaunch
/// has happened.
static LAUNCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn install(app: &mut App, port: u16) {
    let launch = LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    app.add_plugins(
        RemotePlugin::default()
            .with_method_main("bedtime.status", status)
            .with_method_main("bedtime.input", input)
            .with_method_main("bedtime.ui", ui)
            .with_method_main("bedtime.screenshot", screenshot)
            .with_method_main("bedtime.invoke", invoke)
            .with_method_main("bedtime.exit", exit),
    );
    if MAILBOX.lock().unwrap().is_some() {
        app.add_systems(Startup, |world: &mut World| {
            if let Some(mailbox) = MAILBOX.lock().unwrap().take() {
                world.insert_resource(mailbox);
            }
        });
    } else {
        app.add_plugins(RemoteHttpPlugin::default().with_port(port));
    }
    app.add_systems(Last, |world: &mut World| {
        if world.resource::<Messages<AppExit>>().is_empty() {
            return;
        }
        if let Some(mailbox) = world.remove_resource::<BrpReceiver>() {
            *MAILBOX.lock().unwrap() = Some(mailbox);
        }
    })
    .insert_resource(Automation {
        launch,
        ..default()
    })
    .init_resource::<AccessibilityTree>()
    .add_systems(First, (pretend_focus, run_step))
    .add_systems(PostUpdate, stop_pretending)
    .add_systems(Last, |mut automation: ResMut<Automation>| {
        automation.frame += 1
    });
}

fn params<T: for<'de> Deserialize<'de>>(params: Option<Value>) -> BrpResult<T> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|error| invalid(error.to_string()))
}

fn invalid(message: impl Into<String>) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

fn status(_: In<Option<Value>>, world: &mut World) -> BrpResult {
    let window = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .map(|window| {
            json!({
                "width": window.width(),
                "height": window.height(),
                "scale": window.scale_factor(),
            })
        });
    let automation = world.resource::<Automation>();
    Ok(json!({
        "launch": automation.launch,
        "frame": automation.frame,
        "pending": automation.steps.len(),
        "window": window,
        "screenshots": automation.screenshots,
        "errors": automation.errors,
    }))
}

#[derive(Deserialize)]
struct Input {
    steps: Vec<Step>,
}

fn input(In(request): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Input { steps } = params(request)?;
    world.resource_mut::<Automation>().steps.extend(steps);
    Ok(Value::Null)
}

/// The window opens unfocused so scripts don't take over the computer, but the
/// app (shortcuts, egui) should behave as if it had focus. Mark it focused for
/// the frame without change detection, and restore it before `Last`, where
/// winit applies window changes (focusing a window activates the app).
fn pretend_focus(
    mut automation: ResMut<Automation>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    if let Ok((entity, mut window)) = windows.single_mut()
        && !window.focused
    {
        window.bypass_change_detection().focused = true;
        automation.pretended = Some(entity);
    }
}

fn stop_pretending(mut automation: ResMut<Automation>, mut windows: Query<&mut Window>) {
    if let Some(entity) = automation.pretended.take()
        && let Ok(mut window) = windows.get_mut(entity)
    {
        window.bypass_change_detection().focused = false;
    }
}

/// Run the next input step, before the input systems read this frame's input.
fn run_step(world: &mut World) {
    let Ok(window) = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
    else {
        return;
    };
    let mut automation = world.resource_mut::<Automation>();
    let Some(step) = automation.steps.pop_front() else {
        return;
    };
    let state = |pressed| {
        if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        }
    };
    match step {
        Step::Move { x, y } => {
            let position = Vec2::new(x, y);
            let message = CursorMoved {
                window,
                position,
                delta: Some(position - automation.cursor),
            };
            automation.cursor = position;
            world.write_message(WindowEvent::CursorMoved(message.clone()));
            world.write_message(message);
        }
        Step::Press(button) | Step::Release(button) => {
            let message = MouseButtonInput {
                button,
                state: state(matches!(step, Step::Press(_))),
                window,
            };
            world.write_message(WindowEvent::MouseButtonInput(message));
            world.write_message(message);
        }
        Step::KeyDown(key_code) | Step::KeyUp(key_code) => {
            world.write_message(KeyboardInput {
                key_code,
                logical_key: logical_key(key_code),
                state: state(matches!(step, Step::KeyDown(_))),
                text: None,
                repeat: false,
                window,
            });
        }
        Step::Text(text) => {
            for pressed in [true, false] {
                world.write_message(KeyboardInput {
                    key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified),
                    logical_key: Key::Character(text.as_str().into()),
                    state: state(pressed),
                    text: pressed.then(|| text.as_str().into()),
                    repeat: false,
                    window,
                });
            }
        }
        Step::Scroll { x, y } => {
            world.write_message(MouseWheel {
                unit: MouseScrollUnit::Pixel,
                x,
                y,
                window,
                phase: TouchPhase::Moved,
            });
        }
        Step::Wait(frames) => {
            if frames > 1 {
                automation.steps.push_front(Step::Wait(frames - 1));
            }
        }
    }
}

/// The logical key a physical key gives without a layout: what egui and Bevy
/// read modifiers and named keys from. Other keys fall back to the physical key.
fn logical_key(key: KeyCode) -> Key {
    match key {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Space => Key::Space,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => Key::Unidentified(NativeKey::Unidentified),
    }
}

/// The last frame's widgets in tree order, each with its parent's id. Nodes
/// the root doesn't reach (egui attaches some popups elsewhere) follow.
fn ui(_: In<Option<Value>>, world: &mut World) -> BrpResult {
    let Some(tree) = &world.resource::<AccessibilityTree>().0 else {
        return Ok(json!([]));
    };
    let nodes: std::collections::HashMap<_, _> =
        tree.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let root = tree.tree.as_ref().map(|tree| tree.root);
    let mut widgets = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut stack: Vec<(accesskit::NodeId, Option<accesskit::NodeId>)> =
        root.into_iter().map(|root| (root, None)).collect();
    let mut unreached = tree.nodes.iter().map(|(id, _)| *id).rev();
    while let Some((id, parent)) = stack.pop().or_else(|| {
        unreached
            .find(|id| !visited.contains(id))
            .map(|id| (id, None))
    }) {
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        if !visited.insert(id) {
            continue;
        }
        widgets.push(json!({
            "id": id.0,
            "parent": parent.map(|parent| parent.0),
            "role": format!("{:?}", node.role()),
            "label": node.label(),
            "value": node.value(),
            "numeric_value": node.numeric_value(),
            "rect": node.bounds().map(|r| [r.x0, r.y0, r.x1, r.y1]),
            "disabled": node.is_disabled(),
            "focused": tree.focus == id,
            "toggled": node.toggled().map(|toggled| format!("{toggled:?}")),
            "selected": node.is_selected(),
        }));
        stack.extend(node.children().iter().rev().map(|child| (*child, Some(id))));
    }
    Ok(Value::Array(widgets))
}

#[derive(Deserialize)]
struct ScreenshotParams {
    path: PathBuf,
}

/// Capture the window as rendered to `path` (PNG). The image is written under
/// a temporary name and renamed, so a listed screenshot is complete.
fn screenshot(In(request): In<Option<Value>>, world: &mut World) -> BrpResult {
    let ScreenshotParams { path } = params(request)?;
    let partial = path.with_extension("partial.png");
    let mut save = save_to_disk(partial.clone());
    world.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>, mut automation: ResMut<Automation>| {
            save(captured);
            match std::fs::rename(&partial, &path) {
                Ok(()) => automation.screenshots.push(path.clone()),
                Err(error) => automation
                    .errors
                    .push(format!("Screenshot {}: {error}", path.display())),
            }
        },
    );
    Ok(Value::Null)
}

#[derive(Deserialize)]
struct Invoke {
    command: String,
}

/// Run a command by key, for the focused panel, as its keybinding would.
fn invoke(In(request): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Invoke { command } = params(request)?;
    let id = world
        .resource::<ActionRegistry>()
        .commands()
        .find(|(id, _)| id.as_str() == command)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| invalid(format!("No command {command}")))?;
    world
        .resource_mut::<ActionRequests>()
        .submit(ActionRequest {
            command: id,
            context: InvocationContext::Focused,
            source: None,
        });
    Ok(Value::Null)
}

fn exit(_: In<Option<Value>>, world: &mut World) -> BrpResult {
    world.write_message(AppExit::Success);
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_read_as_scripts_write_them() {
        let steps: Vec<Step> = serde_json::from_value(json!([
            {"move": {"x": 10, "y": 20.5}},
            {"press": "Left"},
            {"key_down": "SuperLeft"},
            {"key_up": "Comma"},
            {"text": "hi"},
            {"scroll": {"x": 0, "y": -40}},
            {"wait": 3},
        ]))
        .unwrap();
        assert!(matches!(steps[0], Step::Move { x: 10.0, y: 20.5 }));
        assert!(matches!(steps[1], Step::Press(MouseButton::Left)));
        assert!(matches!(steps[3], Step::KeyUp(KeyCode::Comma)));
        assert!(serde_json::from_value::<Step>(json!({"tap": "Left"})).is_err());
    }

    #[test]
    fn modifiers_and_named_keys_have_logical_keys() {
        assert_eq!(logical_key(KeyCode::SuperLeft), Key::Super);
        assert_eq!(logical_key(KeyCode::Escape), Key::Escape);
        assert_eq!(
            logical_key(KeyCode::KeyA),
            Key::Unidentified(NativeKey::Unidentified)
        );
    }
}
