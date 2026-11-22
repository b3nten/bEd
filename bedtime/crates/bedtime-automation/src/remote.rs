//! The remote methods and the systems that carry them out.
use bedtime_ui::{AccessibilityTree, egui::accesskit};
use bedtime_workbench::{ActionRegistry, ActionRequest, ActionRequests, InvocationContext};
use bevy::{
    ecs::message::MessageCursor,
    input::{
        ButtonState,
        keyboard::{Key, KeyboardFocusLost, KeyboardInput, NativeKey, NativeKeyCode},
        mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel},
        touch::TouchPhase,
    },
    prelude::*,
    remote::{BrpError, BrpResult, RemotePlugin, error_codes, http::RemoteHttpPlugin},
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    window::{CursorEntered, CursorLeft, CursorMoved, PrimaryWindow, WindowEvent},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf};

/// One input step; a script's steps run one per frame, so each is seen by the
/// UI before the next. As JSON: `{"move": {"x": 10, "y": 20}}` (logical
/// pixels from the window's top left), `{"press": "Left"}`, `{"release":
/// "Left"}`, `{"key_down": "KeyA"}`, `{"key_up": "SuperLeft"}` (Bevy
/// `KeyCode` names), `{"text": "typed"}`, `{"scroll": {"x": 0, "y": -40}}`
/// (pixels), `{"wait": 3}` (frames), `{"drop": {"x": 10, "y": 20, "paths":
/// ["/a.txt"]}}` (files dropped from outside the app, there).
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
    Drop { x: f32, y: f32, paths: Vec<PathBuf> },
}

#[derive(Resource, Default)]
struct Automation {
    /// How many times a workspace has opened (at startup, and each switch).
    workspaces: u64,
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

pub(crate) fn install(app: &mut App, port: u16) {
    app.add_plugins(
        RemotePlugin::default()
            .with_method_main("bedtime.status", status)
            .with_method_main("bedtime.input", input)
            .with_method_main("bedtime.ui", ui)
            .with_method_main("bedtime.screenshot", screenshot)
            .with_method_main("bedtime.invoke", invoke)
            .with_method_main("bedtime.exit", exit),
    );
    app.add_plugins(RemoteHttpPlugin::default().with_port(port))
        // Scripts wait for this to change when they switch workspace.
        .add_systems(
            bedtime_workbench::OnWorkspaceOpen,
            |mut automation: ResMut<Automation>| automation.workspaces += 1,
        )
        .init_resource::<Automation>()
        .init_resource::<AccessibilityTree>()
        .add_systems(
            First,
            (pretend_focus, clicks_take_as_long_as_they_take, run_step),
        )
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
        "workspaces": automation.workspaces,
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

/// A script's press and release are a frame apart, however long the frame
/// takes: while the app starts, or the computer is busy, that can be longer
/// than egui lets a click be (0.8 s), and it would be taken for a drag that
/// went nowhere. A script's click is a click.
fn clicks_take_as_long_as_they_take(
    mut contexts: Query<&mut bevy_egui::EguiContext, Added<bevy_egui::EguiContext>>,
) {
    for mut context in &mut contexts {
        context
            .get_mut()
            .options_mut(|options| options.input_options.max_click_duration = f64::INFINITY);
    }
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
fn run_step(world: &mut World, mut seen: Local<MessageCursor<WindowEvent>>) {
    drop_real_input(world, &mut seen);
    step(world);
    // What the step wrote is the script's: next frame, what's new is the
    // window's own.
    seen.clear(world.resource::<Messages<WindowEvent>>());
}

/// The pointer and the keyboard are the script's. The window is on screen,
/// beside whatever someone is working in: the real pointer crosses it, and a
/// real move between a script's press and release would put the release
/// somewhere else; the window is covered or uncovered as they work, which
/// tells it the keyboard's focus is lost, and that releases every held key,
/// a script's modifier before its letter among them. What the real pointer,
/// keyboard and focus did since the last frame is dropped, before this
/// frame's step is written. (Passing the pointer through the window,
/// `CursorOptions::hit_test`, doesn't stop its moves arriving on macOS.)
///
/// `seen` is at the end of what the last step wrote, so what it hasn't read
/// came from the window.
fn drop_real_input(world: &mut World, seen: &mut MessageCursor<WindowEvent>) {
    fn clear<M: Message>(world: &mut World) {
        if let Some(mut messages) = world.get_resource_mut::<Messages<M>>() {
            messages.clear();
        }
    }
    // Only input writes these. Last frame's are the script's, and every
    // system has read them.
    clear::<CursorMoved>(world);
    clear::<CursorEntered>(world);
    clear::<CursorLeft>(world);
    clear::<MouseButtonInput>(world);
    clear::<MouseWheel>(world);
    clear::<MouseMotion>(world);
    clear::<KeyboardInput>(world);
    clear::<KeyboardFocusLost>(world);

    let input = |event: &WindowEvent| {
        matches!(
            event,
            WindowEvent::CursorMoved(_)
                | WindowEvent::CursorEntered(_)
                | WindowEvent::CursorLeft(_)
                | WindowEvent::MouseButtonInput(_)
                | WindowEvent::MouseMotion(_)
                | WindowEvent::MouseWheel(_)
                | WindowEvent::KeyboardInput(_)
                | WindowEvent::KeyboardFocusLost(_)
                | WindowEvent::WindowFocused(_)
        )
    };
    let mut messages = world.resource_mut::<Messages<WindowEvent>>();
    let new: Vec<WindowEvent> = seen.read(&messages).cloned().collect();
    if new.iter().any(input) {
        // Window events hold more than input: keep the rest of the new ones.
        // (The older ones have been read, like the input's own.)
        messages.clear();
        for event in new.into_iter().filter(|event| !input(event)) {
            messages.write(event);
        }
    }
}

fn step(world: &mut World) {
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
        Step::Drop { x, y, paths } => {
            let position = Vec2::new(x, y);
            let message = CursorMoved {
                window,
                position,
                delta: Some(position - automation.cursor),
            };
            automation.cursor = position;
            world.write_message(WindowEvent::CursorMoved(message.clone()));
            world.write_message(message);
            for path_buf in paths {
                let drop = bevy::window::FileDragAndDrop::DroppedFile { window, path_buf };
                world.write_message(WindowEvent::FileDragAndDrop(drop.clone()));
                world.write_message(drop);
            }
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

/// Run an action by key, for what's focused, as its keybinding would. The
/// parameter is still called `command`, which scripts use.
fn invoke(In(request): In<Option<Value>>, world: &mut World) -> BrpResult {
    let Invoke { command } = params(request)?;
    let id = world
        .resource::<ActionRegistry>()
        .actions()
        .find(|(id, _)| id.as_str() == command)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| invalid(format!("No action {command}")))?;
    world
        .resource_mut::<ActionRequests>()
        .submit(ActionRequest {
            action: id,
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

    /// The real pointer moves over the window between a script's press and
    /// release: its moves are dropped, the script's input and the window's
    /// other events are kept.
    #[test]
    fn real_input_is_dropped_and_the_rest_kept() {
        let mut world = World::new();
        world.init_resource::<Messages<WindowEvent>>();
        world.init_resource::<Messages<CursorMoved>>();
        world.init_resource::<Messages<MouseButtonInput>>();
        let window = world.spawn_empty().id();
        let moved = |x| CursorMoved {
            window,
            position: Vec2::new(x, 5.0),
            delta: None,
        };
        let mut seen = MessageCursor::<WindowEvent>::default();

        // A frame: nothing from the window, then the script's press.
        drop_real_input(&mut world, &mut seen);
        let press = MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window,
        };
        world.write_message(WindowEvent::MouseButtonInput(press));
        world.write_message(press);
        seen.clear(world.resource::<Messages<WindowEvent>>());
        assert_eq!(world.resource::<Messages<MouseButtonInput>>().len(), 1);

        // Before the next frame the real pointer crosses the window, and the
        // window is resized.
        world.write_message(WindowEvent::CursorMoved(moved(700.0)));
        world.write_message(moved(700.0));
        // And it's covered: the keyboard's focus is lost, which would
        // release a script's held modifier.
        world.init_resource::<Messages<KeyboardFocusLost>>();
        world.write_message(WindowEvent::KeyboardFocusLost(KeyboardFocusLost));
        world.write_message(KeyboardFocusLost);
        world.write_message(WindowEvent::WindowResized(bevy::window::WindowResized {
            window,
            width: 800.0,
            height: 600.0,
        }));
        drop_real_input(&mut world, &mut seen);

        assert!(world.resource::<Messages<CursorMoved>>().is_empty());
        assert!(world.resource::<Messages<KeyboardFocusLost>>().is_empty());
        let mut all = MessageCursor::<WindowEvent>::default();
        let left: Vec<_> = all
            .read(world.resource::<Messages<WindowEvent>>())
            .cloned()
            .collect();
        assert!(
            matches!(left.as_slice(), [WindowEvent::WindowResized(_)]),
            "{left:?}"
        );
    }

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
