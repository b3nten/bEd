//! Terminal input ownership is resolved before ImGui and gameplay receive events.
use bed_desktop::{
    DesktopContext, DesktopInjectedEvent, DesktopInjectedInputEvents, DesktopInputEvents,
    DesktopSystems,
};
use bed_terminal_core::{
    bed_terminal_core::BedTerminal,
    terminal_input::{KeyLocation, KeyState, TerminalKey, TerminalKeyEvent, TerminalModifiers},
};
use bed_workbench::WorkbenchLayout;
use bed_workbench_api::PanelFrontend;
use bevy::prelude::*;
use std::collections::HashMap;
use winit::{
    event::{ElementState, Ime, KeyEvent, WindowEvent},
    keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
};
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalInputSystems {
    Intercept,
}
#[derive(Resource, Default)]
struct InputState {
    modifiers: TerminalModifiers,
    reserved: HashMap<PhysicalKey, KeyRoute>,
    composing: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyRoute {
    Application,
    Terminal,
}
pub(crate) fn install(app: &mut App) {
    app.init_resource::<InputState>()
        .init_resource::<TerminalInputPolicy>()
        .init_resource::<FontSelection>()
        .add_systems(
            PreUpdate,
            (fonts, prepare_terminal)
                .chain()
                .after(DesktopSystems::Prepare)
                .after(bed_terminal_core::TerminalCoreSystems::Poll)
                .before(DesktopSystems::Ui),
        )
        .add_systems(
            PreUpdate,
            intercept
                .in_set(DesktopSystems::InterceptInput)
                .in_set(TerminalInputSystems::Intercept),
        );
}
/// Application chord keys reserved before terminal input. Hosts may replace
/// this policy without depending on a configuration storage implementation.
#[derive(Resource, Clone, Debug)]
pub struct TerminalInputPolicy {
    pub application_shortcuts: Vec<bed_keybindings::Stroke>,
    pub copy: Vec<bed_keybindings::Stroke>,
    pub paste: Vec<bed_keybindings::Stroke>,
    pub select_all: Vec<bed_keybindings::Stroke>,
    pub recording: bool,
}
impl Default for TerminalInputPolicy {
    fn default() -> Self {
        let bindings = bed_keybindings::Bindings::new(
            crate::feature::shortcut_actions(),
            &Default::default(),
            bed_keybindings::Platform::current(),
        );
        Self {
            application_shortcuts: Vec::new(),
            copy: bindings
                .for_action("bed.terminal.copy")
                .iter()
                .map(|b| b.shortcut)
                .collect(),
            paste: bindings
                .for_action("bed.terminal.paste")
                .iter()
                .map(|b| b.shortcut)
                .collect(),
            select_all: bindings
                .for_action("bed.terminal.select_all")
                .iter()
                .map(|b| b.shortcut)
                .collect(),
            recording: false,
        }
    }
}
fn stroke(key: &TerminalKeyEvent) -> Option<bed_keybindings::Stroke> {
    use TerminalKey::*;
    let name = match &key.key {
        Character(value) => {
            let value = key.unshifted_key.as_deref().unwrap_or(value);
            let physical = key.base_layout_key.map(|c| c.to_string());
            let value = if physical
                .as_ref()
                .is_some_and(|s| !s.chars().next().unwrap().is_alphanumeric())
            {
                physical.as_deref().unwrap()
            } else {
                value
            };
            let code = bed_keybindings::parse_key(value)?;
            return Some(bed_keybindings::Stroke {
                key: code,
                modifiers: bed_keybindings::Modifiers {
                    ctrl: key.modifiers.control,
                    alt: key.modifiers.alt,
                    shift: key.modifiers.shift,
                    super_key: key.modifiers.super_key,
                    primary: false,
                },
            });
        }
        F(n) => format!("F{n}"),
        Enter if key.location == KeyLocation::Numpad => "KeypadEnter".into(),
        Enter => "Enter".into(),
        Tab => "Tab".into(),
        Backspace => "Backspace".into(),
        Escape => "Escape".into(),
        Up => "Up".into(),
        Down => "Down".into(),
        Left => "Left".into(),
        Right => "Right".into(),
        Home => "Home".into(),
        End => "End".into(),
        Insert => "Insert".into(),
        Delete => "Delete".into(),
        PageUp => "PageUp".into(),
        PageDown => "PageDown".into(),
        _ => return None,
    };
    Some(bed_keybindings::Stroke {
        key: bed_keybindings::parse_key(&name)?,
        modifiers: bed_keybindings::Modifiers {
            ctrl: key.modifiers.control,
            alt: key.modifiers.alt,
            shift: key.modifiers.shift,
            super_key: key.modifiers.super_key,
            primary: false,
        },
    })
}
fn application_shortcut(key: &TerminalKeyEvent, policy: &TerminalInputPolicy) -> bool {
    policy.recording
        || stroke(key).is_some_and(|stroke| policy.application_shortcuts.contains(&stroke))
}
fn clipboard_key(key: &TerminalKeyEvent, policy: &TerminalInputPolicy) -> Option<&'static str> {
    let stroke = stroke(key)?;
    if policy.copy.contains(&stroke) {
        Some("copy")
    } else if policy.paste.contains(&stroke) {
        Some("paste")
    } else if policy.select_all.contains(&stroke) {
        Some("select")
    } else {
        None
    }
}

fn deliver_key(
    state: &mut InputState,
    terminal: &mut BedTerminal,
    desktop: Option<&mut DesktopContext>,
    physical: PhysicalKey,
    key: TerminalKeyEvent,
    policy: &TerminalInputPolicy,
) -> Result<bool, String> {
    if key.state == KeyState::Release
        && let Some(route) = state.reserved.remove(&physical)
    {
        return Ok(route == KeyRoute::Terminal);
    }
    if let Some(route) = state.reserved.get(&physical) {
        return Ok(*route == KeyRoute::Terminal);
    }
    if state.composing {
        return Ok(true);
    }
    if policy.recording {
        if key.state == KeyState::Press {
            state.reserved.insert(physical, KeyRoute::Application);
        }
        return Ok(false);
    }
    if let Some(action) = clipboard_key(&key, policy) {
        if key.state == KeyState::Press {
            state.reserved.insert(physical, KeyRoute::Terminal);
            match action {
                "copy" => {
                    if let Some(text) = terminal.native_copy()
                        && let Some(desktop) = desktop
                    {
                        desktop.context.set_clipboard_text(text);
                    }
                }
                "select" => terminal
                    .native_select_all()
                    .map_err(|error| error.to_string())?,
                _ => {
                    if let Some(text) = desktop.and_then(|desktop| desktop.context.clipboard_text())
                    {
                        terminal
                            .native_paste(text)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        return Ok(true);
    }
    if application_shortcut(&key, policy) {
        if key.state == KeyState::Press {
            state.reserved.insert(physical, KeyRoute::Application);
        }
        return Ok(false);
    }
    terminal.queue_key(key).map_err(|error| error.to_string())?;
    Ok(true)
}
#[expect(
    clippy::too_many_arguments,
    reason = "Bevy system parameters name native observations and canonical focus/input owners"
)]
fn intercept(
    mut state: ResMut<InputState>,
    mut terminal: NonSendMut<BedTerminal>,
    mut events: Option<ResMut<DesktopInputEvents>>,
    mut injected: Option<ResMut<DesktopInjectedInputEvents>>,
    mut desktop: Option<NonSendMut<DesktopContext>>,
    layout: Option<Res<WorkbenchLayout>>,
    frontends: Query<&PanelFrontend>,
    panels: Query<&crate::TerminalPanelState>,
    policy: Res<TerminalInputPolicy>,
    mut intents: ResMut<bed_editor_api::EditorIntents>,
) {
    let focused = layout
        .as_ref()
        .and_then(|layout| layout.focused)
        .and_then(|entity| {
            frontends
                .get(entity)
                .ok()
                .map(|frontend| frontend.0)
                .or(Some(entity))
        })
        .is_some_and(|entity| panels.contains(entity));
    let mut active = focused && terminal.is_focused();
    let mut errors = Vec::new();
    if let Some(events) = &mut events {
        for event in &mut events.0 {
            if event.consumed {
                continue;
            }
            match &event.event.event {
                WindowEvent::ModifiersChanged(modifiers) => {
                    let modifiers = modifiers.state();
                    state.modifiers.shift = modifiers.contains(ModifiersState::SHIFT);
                    state.modifiers.alt = modifiers.contains(ModifiersState::ALT);
                    state.modifiers.control = modifiers.contains(ModifiersState::CONTROL);
                    state.modifiers.super_key = modifiers.contains(ModifiersState::SUPER);
                }
                WindowEvent::Focused(window_focused) => {
                    if let Err(error) = terminal.set_window_focused(*window_focused) {
                        errors.push(error.to_string());
                    }
                    active = focused && terminal.is_focused();
                    if !window_focused {
                        state.modifiers = TerminalModifiers {
                            caps_lock: state.modifiers.caps_lock,
                            num_lock: state.modifiers.num_lock,
                            ..default()
                        };
                        state.reserved.clear();
                        state.composing = false;
                        terminal.queue_ime_preedit(String::new());
                    }
                }
                WindowEvent::KeyboardInput {
                    event: key,
                    is_synthetic,
                    ..
                } => {
                    if key.state == ElementState::Pressed && !key.repeat {
                        match key.logical_key {
                            WinitKey::Named(NamedKey::CapsLock) => {
                                state.modifiers.caps_lock = !state.modifiers.caps_lock
                            }
                            WinitKey::Named(NamedKey::NumLock) => {
                                state.modifiers.num_lock = !state.modifiers.num_lock
                            }
                            _ => {}
                        }
                    }
                    if key.state == ElementState::Released
                        && let Some(route) = state.reserved.remove(&key.physical_key)
                    {
                        event.consumed = route == KeyRoute::Terminal;
                        event.gameplay_consumed = true;
                        continue;
                    }
                    if !active || *is_synthetic {
                        continue;
                    }
                    event.gameplay_consumed = true;
                    if matches!(key.logical_key, WinitKey::Dead(_)) {
                        event.consumed = true;
                        continue;
                    }
                    let translated = terminal_key_event(key, state.modifiers);
                    match deliver_key(
                        &mut state,
                        &mut terminal,
                        desktop.as_deref_mut(),
                        key.physical_key,
                        translated,
                        &policy,
                    ) {
                        Ok(consumed) => event.consumed = consumed,
                        Err(error) => {
                            event.consumed = true;
                            errors.push(error);
                        }
                    }
                }
                WindowEvent::Ime(ime) if active => {
                    event.consumed = true;
                    if policy.recording {
                        event.gameplay_consumed = true;
                        continue;
                    }
                    event.gameplay_consumed = true;
                    match ime {
                        Ime::Preedit(text, _) => {
                            state.composing = !text.is_empty();
                            terminal.queue_ime_preedit(text.clone());
                        }
                        Ime::Commit(text) => {
                            state.composing = false;
                            if let Err(error) = terminal.queue_ime_commit(text.clone()) {
                                errors.push(error.to_string());
                            }
                        }
                        Ime::Disabled => {
                            state.composing = false;
                            terminal.queue_ime_preedit(String::new());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(events) = &mut injected {
        for event in &mut events.0 {
            if event.consumed {
                continue;
            }
            match &event.event {
                DesktopInjectedEvent::Modifiers(modifiers) => {
                    state.modifiers.shift = modifiers.shift;
                    state.modifiers.control = modifiers.ctrl;
                    state.modifiers.alt = modifiers.alt;
                    state.modifiers.super_key = modifiers.super_key;
                }
                DesktopInjectedEvent::Key {
                    physical,
                    logical,
                    pressed,
                } => {
                    let physical = PhysicalKey::Code(*physical);
                    if !pressed && let Some(route) = state.reserved.remove(&physical) {
                        event.consumed = route == KeyRoute::Terminal;
                        event.gameplay_consumed = true;
                        continue;
                    }
                    if !active {
                        continue;
                    }
                    event.gameplay_consumed = true;
                    let key = TerminalKeyEvent {
                        key: match logical {
                            WinitKey::Character(text) => TerminalKey::Character(text.to_string()),
                            WinitKey::Named(named) => terminal_named_key(*named),
                            _ => TerminalKey::Unknown,
                        },
                        unshifted_key: match logical {
                            WinitKey::Character(text) => Some(text.to_string()),
                            _ => None,
                        },
                        base_layout_key: match physical {
                            PhysicalKey::Code(code) => terminal_base_key(code),
                            _ => None,
                        },
                        state: if *pressed {
                            KeyState::Press
                        } else {
                            KeyState::Release
                        },
                        modifiers: state.modifiers,
                        ..default()
                    };
                    // The injection API supplies character text separately. Physical
                    // unmodified character presses cannot also emit that text.
                    if matches!(key.key, TerminalKey::Character(_))
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && !key.modifiers.super_key
                        && !application_shortcut(&key, &policy)
                        && clipboard_key(&key, &policy).is_none()
                    {
                        event.consumed = true;
                        continue;
                    }
                    match deliver_key(
                        &mut state,
                        &mut terminal,
                        desktop.as_deref_mut(),
                        physical,
                        key,
                        &policy,
                    ) {
                        Ok(consumed) => event.consumed = consumed,
                        Err(error) => {
                            event.consumed = true;
                            errors.push(error);
                        }
                    }
                }
                DesktopInjectedEvent::Text(text) if active => {
                    event.gameplay_consumed = true;
                    if policy.recording
                        || state
                            .reserved
                            .values()
                            .any(|route| *route == KeyRoute::Application)
                    {
                        // The UI owns both the key and its character event.
                        event.consumed = false;
                        continue;
                    }
                    event.consumed = true;
                    if state.reserved.iter().any(|(physical,route)| *route==KeyRoute::Terminal && matches!(physical,PhysicalKey::Code(code) if terminal_base_key(*code).is_some())) {continue;}
                    if let Err(error) = terminal.queue_ime_commit(text.clone()) {
                        errors.push(error.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    for message in errors {
        intents.push(None, bed_editor_api::HostRequest::Notify { message });
    }
}
fn terminal_key_event(event: &KeyEvent, modifiers: TerminalModifiers) -> TerminalKeyEvent {
    let key = match &event.logical_key {
        WinitKey::Character(text) => TerminalKey::Character(text.to_string()),
        WinitKey::Named(named) => terminal_named_key(*named),
        _ => TerminalKey::Unknown,
    };
    let unshifted_key = match event.key_without_modifiers() {
        WinitKey::Character(text) => Some(text.to_string()),
        _ => None,
    };
    let shifted_key = modifiers
        .shift
        .then(|| {
            if let WinitKey::Character(text) = &event.logical_key {
                Some(text.to_string())
            } else {
                None
            }
        })
        .flatten();
    TerminalKeyEvent {
        key,
        unshifted_key,
        shifted_key,
        base_layout_key: match event.physical_key {
            PhysicalKey::Code(code) => terminal_base_key(code),
            _ => None,
        },
        text: event.text.as_ref().map(ToString::to_string),
        location: match event.location {
            winit::keyboard::KeyLocation::Standard => KeyLocation::Standard,
            winit::keyboard::KeyLocation::Left => KeyLocation::Left,
            winit::keyboard::KeyLocation::Right => KeyLocation::Right,
            winit::keyboard::KeyLocation::Numpad => KeyLocation::Numpad,
        },
        modifiers,
        state: if event.state == ElementState::Released {
            KeyState::Release
        } else if event.repeat {
            KeyState::Repeat
        } else {
            KeyState::Press
        },
    }
}

fn terminal_named_key(key: NamedKey) -> TerminalKey {
    match key {
        NamedKey::Enter => TerminalKey::Enter,
        NamedKey::Tab => TerminalKey::Tab,
        NamedKey::Backspace => TerminalKey::Backspace,
        NamedKey::Escape => TerminalKey::Escape,
        NamedKey::ArrowUp => TerminalKey::Up,
        NamedKey::ArrowDown => TerminalKey::Down,
        NamedKey::ArrowLeft => TerminalKey::Left,
        NamedKey::ArrowRight => TerminalKey::Right,
        NamedKey::Home => TerminalKey::Home,
        NamedKey::End => TerminalKey::End,
        NamedKey::Insert => TerminalKey::Insert,
        NamedKey::Delete => TerminalKey::Delete,
        NamedKey::PageUp => TerminalKey::PageUp,
        NamedKey::PageDown => TerminalKey::PageDown,
        NamedKey::CapsLock => TerminalKey::CapsLock,
        NamedKey::NumLock => TerminalKey::NumLock,
        NamedKey::ScrollLock => TerminalKey::ScrollLock,
        NamedKey::PrintScreen => TerminalKey::PrintScreen,
        NamedKey::Pause => TerminalKey::Pause,
        NamedKey::ContextMenu => TerminalKey::Menu,
        NamedKey::Shift => TerminalKey::Shift,
        NamedKey::Control => TerminalKey::Control,
        NamedKey::Alt => TerminalKey::Alt,
        NamedKey::Super => TerminalKey::Super,
        NamedKey::Space => TerminalKey::Character(" ".into()),
        NamedKey::F1 => TerminalKey::F(1),
        NamedKey::F2 => TerminalKey::F(2),
        NamedKey::F3 => TerminalKey::F(3),
        NamedKey::F4 => TerminalKey::F(4),
        NamedKey::F5 => TerminalKey::F(5),
        NamedKey::F6 => TerminalKey::F(6),
        NamedKey::F7 => TerminalKey::F(7),
        NamedKey::F8 => TerminalKey::F(8),
        NamedKey::F9 => TerminalKey::F(9),
        NamedKey::F10 => TerminalKey::F(10),
        NamedKey::F11 => TerminalKey::F(11),
        NamedKey::F12 => TerminalKey::F(12),
        NamedKey::F13 => TerminalKey::F(13),
        NamedKey::F14 => TerminalKey::F(14),
        NamedKey::F15 => TerminalKey::F(15),
        NamedKey::F16 => TerminalKey::F(16),
        NamedKey::F17 => TerminalKey::F(17),
        NamedKey::F18 => TerminalKey::F(18),
        NamedKey::F19 => TerminalKey::F(19),
        NamedKey::F20 => TerminalKey::F(20),
        NamedKey::F21 => TerminalKey::F(21),
        NamedKey::F22 => TerminalKey::F(22),
        NamedKey::F23 => TerminalKey::F(23),
        NamedKey::F24 => TerminalKey::F(24),
        NamedKey::F25 => TerminalKey::F(25),
        NamedKey::F26 => TerminalKey::F(26),
        NamedKey::F27 => TerminalKey::F(27),
        NamedKey::F28 => TerminalKey::F(28),
        NamedKey::F29 => TerminalKey::F(29),
        NamedKey::F30 => TerminalKey::F(30),
        NamedKey::F31 => TerminalKey::F(31),
        NamedKey::F32 => TerminalKey::F(32),
        NamedKey::F33 => TerminalKey::F(33),
        NamedKey::F34 => TerminalKey::F(34),
        NamedKey::F35 => TerminalKey::F(35),
        _ => TerminalKey::Unknown,
    }
}

fn terminal_base_key(key: KeyCode) -> Option<char> {
    Some(match key {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Space => ' ',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

#[derive(Resource, Default)]
struct FontSelection(Option<u64>);
fn fonts(
    mut selection: ResMut<FontSelection>,
    mut fonts: NonSendMut<bed_terminal_core::terminal_font::TerminalFonts>,
    mut terminal: NonSendMut<BedTerminal>,
    appearance: Res<crate::TerminalAppearance>,
    ui_fonts: Option<NonSend<bed_desktop::DesktopFonts>>,
    desktop: Option<NonSendMut<DesktopContext>>,
    mut intents: ResMut<bed_editor_api::EditorIntents>,
) {
    terminal.set_theme(
        appearance.background,
        appearance.foreground,
        appearance.ansi,
    );
    let (Some(ui_fonts), Some(mut desktop)) = (ui_fonts, desktop) else {
        return;
    };
    let Some(resolved) = &ui_fonts.resolved else {
        return;
    };
    if selection.0 == Some(ui_fonts.generation) {
        return;
    }
    let size = ui_fonts.size();
    match fonts.reload_resolved(&mut desktop.context, size, resolved) {
        Ok(()) => {
            selection.0 = Some(ui_fonts.generation);
            terminal.reload_terminal_fonts(size);
        }
        Err(error) => intents.push(
            None,
            bed_editor_api::HostRequest::Notify {
                message: error.to_string(),
            },
        ),
    }
}

/// Prepare retained terminal glyphs and images between UI frames. The desktop
/// renderer owns GPU copies and reuploads these managed textures on recovery;
/// this owner keeps their CPU handles and shaping caches across device changes.
fn prepare_terminal(
    mut terminal: NonSendMut<BedTerminal>,
    fonts: NonSend<bed_terminal_core::terminal_font::TerminalFonts>,
    desktop: Option<NonSendMut<DesktopContext>>,
    windows: Query<&bevy::window::Window, With<bevy::window::PrimaryWindow>>,
    mut intents: ResMut<bed_editor_api::EditorIntents>,
) {
    let Some(mut desktop) = desktop else {
        return;
    };
    if terminal.consume_needs_font_resync() {
        terminal.reload_terminal_fonts(fonts.size);
    }
    let scale = windows
        .iter()
        .next()
        .map_or(1.0, |window| window.resolution.scale_factor());
    if let Err(error) = terminal.prepare_frame(&mut desktop.context, &fonts, scale) {
        intents.push(
            None,
            bed_editor_api::HostRequest::Notify {
                message: error.to_string(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IMGUI_TEST_LOCK as UI_LOCK;
    use bed_desktop::{DesktopInjectedInputEvent, DesktopInputEvent};
    use bed_terminal_core::terminal_pty::{PtyOptions, TerminalShell};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn event(event: DesktopInjectedEvent) -> DesktopInjectedInputEvent {
        DesktopInjectedInputEvent {
            event,
            consumed: false,
            gameplay_consumed: false,
        }
    }
    fn focused_terminal(capture: &std::path::Path) -> (App, Context, Entity) {
        let mut app = App::new();
        app.add_plugins(crate::TerminalPlugin);
        let (session, _) = app.world_mut().non_send_mut::<BedTerminal>().new_command_session(
            PtyOptions {
                shell: Some(TerminalShell::new("/bin/sh", vec!["-c".into(), "stty -echo; read line; printf '%s' \"$line\" > \"$BED_INPUT_CAPTURE\"; sleep 5".into()])),
                env: [("BED_INPUT_CAPTURE".into(), capture.to_string_lossy().into_owned())].into(),
                ..default()
            }, "input fixture").unwrap();
        let entity = app
            .world_mut()
            .spawn(crate::TerminalPanelState {
                session,
                command: true,
            })
            .id();
        app.world_mut()
            .resource_mut::<crate::TerminalRetentions>()
            .0
            .insert(session);
        let mut layout = WorkbenchLayout::default();
        layout.focused = Some(entity);
        app.insert_resource(layout);
        app.init_resource::<DesktopInjectedInputEvents>();
        app.init_resource::<DesktopInputEvents>();
        let mut context = Context::create();
        context.set_ini_filename(None::<String>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..8 {
            let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
            // Font references are immutable external data for this native widget.
            let fonts = bed_terminal_core::terminal_font::TerminalFonts::default();
            terminal.prepare_frame(&mut context, &fonts, 1.0).unwrap();
            context.prepare_frame(FramePrepareOptions::new([640.0, 360.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Terminal input fixture")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 360.0], Condition::Always)
                .build(|| {
                    terminal.render_session(ui, &fonts, session).unwrap();
                });
            drop(context.render_legacy());
            if terminal.is_focused() {
                break;
            }
        }
        assert!(app.world().non_send::<BedTerminal>().is_focused());
        (app, context, entity)
    }
    #[test]
    fn host_supplied_shortcuts_are_reserved_without_configuration_storage() {
        let modifiers = TerminalModifiers {
            super_key: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            ..Default::default()
        };
        let key = TerminalKeyEvent {
            key: TerminalKey::Character("k".into()),
            unshifted_key: Some("k".into()),
            modifiers,
            state: KeyState::Press,
            ..Default::default()
        };
        assert!(!application_shortcut(&key, &TerminalInputPolicy::default()));
        let policy = TerminalInputPolicy {
            application_shortcuts: vec![
                "Primary+K"
                    .parse::<bed_keybindings::Stroke>()
                    .unwrap()
                    .resolved(bed_keybindings::Platform::current()),
            ],
            ..Default::default()
        };
        assert!(application_shortcut(&key, &policy));
        let mut alt = key;
        alt.modifiers.alt = true;
        assert!(!application_shortcut(&alt, &policy));
    }

    #[test]
    fn injected_text_reaches_the_focused_terminal_once_and_app_exit_releases_sessions() {
        let _lock = UI_LOCK.lock().unwrap();
        let temp = crate::test_support::TempDir::new();
        assert!(temp.root().is_dir());
        temp.write("fixture", b"terminal");
        let capture = temp.path("capture");
        let (mut app, _context, _) = focused_terminal(&capture);
        app.world_mut()
            .resource_mut::<TerminalInputPolicy>()
            .application_shortcuts = vec!["Q".parse().unwrap()];
        app.world_mut()
            .resource_mut::<DesktopInjectedInputEvents>()
            .0 = vec![
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::KeyQ,
                logical: WinitKey::Character("q".into()),
                pressed: true,
            }),
            event(DesktopInjectedEvent::Text("q".into())),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::KeyQ,
                logical: WinitKey::Character("q".into()),
                pressed: false,
            }),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::KeyX,
                logical: WinitKey::Character("x".into()),
                pressed: true,
            }),
            event(DesktopInjectedEvent::Text("x".into())),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::KeyX,
                logical: WinitKey::Character("x".into()),
                pressed: false,
            }),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Enter,
                logical: WinitKey::Named(NamedKey::Enter),
                pressed: true,
            }),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Enter,
                logical: WinitKey::Named(NamedKey::Enter),
                pressed: false,
            }),
        ];
        app.update();
        let events = &app.world().resource::<DesktopInjectedInputEvents>().0;
        assert!(
            events[..3]
                .iter()
                .all(|event| !event.consumed && event.gameplay_consumed),
            "reserved printable shortcuts and their text stay on the UI route"
        );
        assert!(
            events[3..]
                .iter()
                .all(|event| event.consumed && event.gameplay_consumed)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !capture.exists() && Instant::now() < deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(fs::read(&capture).unwrap(), b"x");
        app.world_mut().write_message(bevy::app::AppExit::Success);
        app.update();
        assert_eq!(app.world().non_send::<BedTerminal>().session_count(), 0);
    }
    #[test]
    fn native_focus_loss_prevents_terminal_capture_and_resets_modifier_state() {
        let _lock = UI_LOCK.lock().unwrap();
        let temp = crate::test_support::TempDir::new();
        let capture = temp.path("capture");
        let (mut app, _context, _) = focused_terminal(&capture);
        app.world_mut()
            .resource_mut::<InputState>()
            .modifiers
            .control = true;
        app.world_mut()
            .resource_mut::<DesktopInputEvents>()
            .0
            .push(DesktopInputEvent {
                event: bevy::winit::RawWinitWindowEvent {
                    window_id: winit::window::WindowId::dummy(),
                    event: WindowEvent::Focused(false),
                },
                consumed: false,
                gameplay_consumed: false,
            });
        app.world_mut()
            .resource_mut::<DesktopInjectedInputEvents>()
            .0
            .push(event(DesktopInjectedEvent::Text("outside".into())));
        app.update();
        assert!(!app.world().non_send::<BedTerminal>().is_focused());
        assert_eq!(
            app.world().resource::<InputState>().modifiers,
            TerminalModifiers::default()
        );
        let observed = &app.world().resource::<DesktopInjectedInputEvents>().0[0];
        assert!(!observed.consumed && !observed.gameplay_consumed);
        app.world_mut().resource_mut::<DesktopInputEvents>().0 = vec![DesktopInputEvent {
            event: bevy::winit::RawWinitWindowEvent {
                window_id: winit::window::WindowId::dummy(),
                event: WindowEvent::Focused(true),
            },
            consumed: false,
            gameplay_consumed: false,
        }];
        app.world_mut()
            .resource_mut::<DesktopInjectedInputEvents>()
            .0 = vec![
            event(DesktopInjectedEvent::Text("restored".into())),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Enter,
                logical: WinitKey::Named(NamedKey::Enter),
                pressed: true,
            }),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Enter,
                logical: WinitKey::Named(NamedKey::Enter),
                pressed: false,
            }),
        ];
        app.update();
        assert!(app.world().non_send::<BedTerminal>().is_focused());
        assert!(
            app.world()
                .resource::<DesktopInjectedInputEvents>()
                .0
                .iter()
                .all(|event| event.consumed && event.gameplay_consumed)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !capture.exists() && Instant::now() < deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(fs::read(&capture).unwrap(), b"restored");
        app.world_mut().write_message(bevy::app::AppExit::Success);
        app.update();
    }
    #[test]
    fn application_shortcut_release_follows_its_ui_route_after_modifiers_and_focus_change() {
        let _lock = UI_LOCK.lock().unwrap();
        let temp = crate::test_support::TempDir::new();
        let (mut app, _context, _) = focused_terminal(&temp.path("capture"));
        app.world_mut()
            .resource_mut::<TerminalInputPolicy>()
            .application_shortcuts = vec![
            "Primary+Slash"
                .parse::<bed_keybindings::Stroke>()
                .unwrap()
                .resolved(bed_keybindings::Platform::current()),
        ];
        app.world_mut()
            .resource_mut::<DesktopInjectedInputEvents>()
            .0 = vec![
            event(DesktopInjectedEvent::Modifiers(
                bed_desktop::DesktopModifiers {
                    ctrl: !cfg!(target_os = "macos"),
                    super_key: cfg!(target_os = "macos"),
                    ..Default::default()
                },
            )),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Slash,
                logical: WinitKey::Character("/".into()),
                pressed: true,
            }),
        ];
        app.update();
        let press = &app.world().resource::<DesktopInjectedInputEvents>().0[1];
        assert!(
            !press.consumed && press.gameplay_consumed,
            "application shortcuts reach ImGui while gameplay stays blocked"
        );
        app.world_mut().resource_mut::<WorkbenchLayout>().focused = None;
        app.world_mut()
            .resource_mut::<DesktopInjectedInputEvents>()
            .0 = vec![
            event(DesktopInjectedEvent::Modifiers(Default::default())),
            event(DesktopInjectedEvent::Key {
                physical: KeyCode::Slash,
                logical: WinitKey::Character("/".into()),
                pressed: false,
            }),
        ];
        app.update();
        let release = &app.world().resource::<DesktopInjectedInputEvents>().0[1];
        assert!(
            !release.consumed && release.gameplay_consumed,
            "ImGui must see the release for its forwarded key press"
        );
        assert!(app.world().resource::<InputState>().reserved.is_empty());
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
}
