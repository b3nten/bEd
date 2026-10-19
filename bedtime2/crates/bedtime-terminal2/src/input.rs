//! egui events -> semantic terminal input. The PTY worker, not the canvas, owns
//! keyboard modes and calls bedtime-terminal's keyboard/text/paste encoders.
//!
//! egui does not retain numpad identity, lock states, or an unmodified Unicode
//! keysym. We preserve the information it does provide (including physical base
//! keys and left/right modifiers), rather than pretending to reconstruct those
//! missing native keyboard fields.
use bedtime_terminal::{
    terminal_input::{KeyLocation, KeyState, TerminalKey, TerminalKeyEvent, TerminalModifiers},
    terminal_pty::TerminalCommand,
};
use bedtime_ui2::egui::{Event, ImeEvent, Key, KeyboardShortcut, Modifiers};
use std::collections::BTreeMap;

pub(crate) enum TerminalInput {
    Command(TerminalCommand),
    Bytes(Vec<u8>),
}

pub(crate) enum KeyboardAction {
    Input(TerminalInput),
    Copy,
    SelectAll,
    ScrollPage(i32),
    ScrollEdge { top: bool },
}

#[derive(Default)]
pub(crate) struct MappedEvent {
    pub consumed: bool,
    pub actions: Vec<KeyboardAction>,
}

impl MappedEvent {
    fn consume(&mut self) {
        self.consumed = true;
    }

    fn action(&mut self, action: KeyboardAction) {
        self.consume();
        self.actions.push(action);
    }

    fn command(&mut self, command: TerminalCommand) {
        self.action(KeyboardAction::Input(TerminalInput::Command(command)));
    }
}

/// A physical key can acquire a different repeat route when modifiers change,
/// but that must not erase an earlier terminal press's release obligation.
#[derive(Default)]
struct HeldKey {
    terminal_release: Option<TerminalKeyEvent>,
    reserve_release: bool,
}

#[derive(Clone, Copy)]
enum ClipboardOwner {
    Terminal,
    Reserved,
    Discard,
}

/// One mapper per canvas. Releases are reported only for keys this canvas
/// previously sent, regardless of where modifier-changing repeats were routed.
#[derive(Default)]
pub(crate) struct InputMapper {
    held: BTreeMap<Key, HeldKey>,
    composing: bool,
    pub preedit: String,
}

impl InputMapper {
    /// Finish terminal-owned keys when focus/input is suspended. No events from
    /// the newly focused widget are consumed by this cleanup.
    pub fn suspend(&mut self, send_releases: bool) -> Vec<TerminalInput> {
        let releases = std::mem::take(&mut self.held)
            .into_values()
            .filter_map(|held| {
                let mut event = held.terminal_release.filter(|_| send_releases)?;
                event.state = KeyState::Release;
                event.text = None;
                event.modifiers = TerminalModifiers::default();
                Some(TerminalInput::Command(TerminalCommand::Key(event)))
            })
            .collect();
        self.composing = false;
        self.preedit.clear();
        releases
    }

    /// The caller invokes this only when this canvas owns keyboard/IME focus.
    /// `allow_input` gates process input, but still permits read-only selection
    /// and copying from an exited terminal.
    ///
    /// Results have exactly the same indices as `events`; the view interleaves
    /// these actions with pointer actions before returning ordered worker input.
    pub fn map_events(
        &mut self,
        events: &[Event],
        allow_input: bool,
        reserved: &[KeyboardShortcut],
        mac: bool,
    ) -> Vec<MappedEvent> {
        let mut mapped: Vec<_> = (0..events.len()).map(|_| MappedEvent::default()).collect();
        let mut text_owner = vec![None; events.len()];
        let mut key_text = vec![None; events.len()];
        // bevy_egui emits Text then Key; injected events and other integrations
        // can use Key then Text. Greedily pair adjacent, non-overlapping events
        // in either order. Searching across other input would reorder actions.
        let mut index = 0;
        while index + 1 < events.len() {
            let pair = match (&events[index], &events[index + 1]) {
                (
                    Event::Key {
                        pressed: true, key, ..
                    },
                    Event::Text(_),
                ) if character(*key, false).is_some() => Some((index, index + 1)),
                (
                    Event::Text(text),
                    Event::Key {
                        pressed: true, key, ..
                    },
                ) if character(*key, false).is_some()
                    && !matches!(index.checked_sub(1).and_then(|previous| events.get(previous)), Some(Event::Ime(ImeEvent::Commit(committed))) if committed == text) =>
                {
                    Some((index + 1, index))
                }
                _ => None,
            };
            if let Some((key, text)) = pair {
                text_owner[text] = Some(key);
                key_text[key] = Some(text);
                index += 2;
            } else {
                index += 1;
            }
        }

        // UI2 appends one Copy/Cut/Paste event for each Primary+C/X/V press,
        // including Ctrl+C/V on non-macOS. They are companions, not independent
        // terminal shortcuts. In particular Ctrl+C stays SIGINT, Ctrl+V stays
        // literal-next, and Ctrl+Shift+V sends exactly one Paste command.
        let mut copies = Vec::new();
        let mut cuts = Vec::new();
        let mut pastes = Vec::new();
        for (index, event) in events.iter().enumerate() {
            if let Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } = event
                && modifiers.command
            {
                let owner = if is_reserved(physical_key.unwrap_or(*key), *modifiers, reserved) {
                    ClipboardOwner::Reserved
                } else if local_shortcut(*key, *modifiers, mac).is_some() {
                    ClipboardOwner::Terminal
                } else {
                    ClipboardOwner::Discard
                };
                match key {
                    Key::C => copies.push((index, owner)),
                    Key::X => cuts.push((index, owner)),
                    Key::V => pastes.push((index, owner)),
                    _ => {}
                }
            }
        }

        // Match from the suffix because UI2 appends synthesized events. A
        // standalone OS/menu Paste earlier in the stream must retain its own
        // position rather than being mistaken for a later chord's companion.
        let mut companions = vec![None; events.len()];
        for (index, event) in events.iter().enumerate().rev() {
            let queue = match event {
                Event::Copy => &mut copies,
                Event::Cut => &mut cuts,
                Event::Paste(_) => &mut pastes,
                _ => continue,
            };
            if queue.last().is_some_and(|(press, _)| *press < index) {
                companions[index] = queue.pop();
            }
        }

        for (index, event) in events.iter().enumerate() {
            match event {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } => {
                    let identity = physical_key.unwrap_or(*key);
                    if !pressed {
                        match self.held.remove(&identity) {
                            Some(HeldKey {
                                terminal_release: Some(mut key_event),
                                reserve_release,
                            }) if allow_input => {
                                key_event.state = KeyState::Release;
                                key_event.text = None;
                                key_event.modifiers = terminal_modifiers(*modifiers, mac);
                                mapped[index].command(TerminalCommand::Key(key_event));
                                // The terminal's earlier press still needs its
                                // release even if Workbench owned a later repeat.
                                mapped[index].consumed = !reserve_release;
                            }
                            Some(HeldKey {
                                reserve_release: true,
                                ..
                            }) => {}
                            _ => mapped[index].consume(),
                        }
                        continue;
                    }
                    if is_reserved(identity, *modifiers, reserved) {
                        self.held.entry(identity).or_default().reserve_release = true;
                        continue;
                    }
                    if let Some(shortcut) = local_shortcut(*key, *modifiers, mac) {
                        self.held.entry(identity).or_default().reserve_release = false;
                        match shortcut {
                            LocalShortcut::Copy => mapped[index].action(KeyboardAction::Copy),
                            // The clipboard bridge supplies the text separately.
                            LocalShortcut::Paste => mapped[index].consume(),
                            LocalShortcut::SelectAll => {
                                mapped[index].action(KeyboardAction::SelectAll)
                            }
                            LocalShortcut::Page(lines) => {
                                mapped[index].action(KeyboardAction::ScrollPage(lines))
                            }
                            LocalShortcut::Edge(top) => {
                                mapped[index].action(KeyboardAction::ScrollEdge { top })
                            }
                        }
                        continue;
                    }
                    // A conforming backend omits IME-processed keys. Also
                    // suppress a physical confirmation key immediately before
                    // an IME update, without suppressing later ordinary input
                    // in the same frame after a Commit.
                    let following = index
                        + if key_text[index] == Some(index + 1) {
                            2
                        } else {
                            1
                        };
                    let ime_confirmation = matches!(
                        events.get(following),
                        Some(Event::Ime(ImeEvent::Commit(_)))
                    ) || matches!(events.get(following), Some(Event::Ime(ImeEvent::Preedit { text, .. })) if !text.is_empty());
                    if !allow_input || (self.composing || ime_confirmation) && !is_modifier(*key) {
                        self.held.entry(identity).or_default().reserve_release = false;
                        mapped[index].consume();
                        continue;
                    }
                    let paired_text = match key_text[index].and_then(|text| events.get(text)) {
                        Some(Event::Text(text)) => Some(text.as_str()),
                        _ => None,
                    };
                    // bevy_egui permits text under Ctrl+Alt on non-macOS to
                    // represent AltGr. Actual printable text, including ASCII
                    // @/{/backslash, identifies text input rather than a Ctrl+
                    // Alt chord. Without text, preserve the real control chord.
                    if !mac
                        && modifiers.ctrl
                        && modifiers.alt
                        && paired_text.is_some_and(|text| {
                            !text.is_empty() && !text.chars().any(char::is_control)
                        })
                    {
                        mapped[index]
                            .command(TerminalCommand::Text(paired_text.unwrap().to_owned()));
                        self.held.entry(identity).or_default().reserve_release = false;
                        continue;
                    }
                    let key_event = terminal_key_event(
                        *key,
                        *physical_key,
                        *modifiers,
                        *repeat,
                        paired_text,
                        mac,
                    );
                    let held = self.held.entry(identity).or_default();
                    held.reserve_release = false;
                    held.terminal_release
                        .get_or_insert_with(|| key_event.clone());
                    mapped[index].command(TerminalCommand::Key(key_event));
                }
                Event::Text(text) => {
                    if text_owner[index].is_some() {
                        // Ownership is resolved after keys have been mapped,
                        // including a paired key later in a Text-first stream.
                    } else if allow_input
                        && !self.composing
                        && !text.is_empty()
                        && !matches!(index.checked_sub(1).and_then(|previous| events.get(previous)), Some(Event::Ime(ImeEvent::Commit(committed))) if committed == text)
                    {
                        mapped[index].command(TerminalCommand::Text(text.clone()));
                    } else {
                        mapped[index].consume();
                    }
                }
                Event::Copy | Event::Cut => {
                    match companions[index] {
                        Some((_, ClipboardOwner::Reserved)) => {}
                        // Copy was already handled at the key's original index;
                        // a Cut chord is terminal input, never destructive cut.
                        Some(_) => mapped[index].consume(),
                        None => mapped[index].action(KeyboardAction::Copy),
                    }
                }
                Event::Paste(text) => match companions[index] {
                    Some((_, ClipboardOwner::Reserved)) => {}
                    Some((_, ClipboardOwner::Discard)) => mapped[index].consume(),
                    Some((press, ClipboardOwner::Terminal)) => {
                        // UI2 appends synthesized clipboard events. Resolve the
                        // paste at its originating key, before a later Enter
                        // or pointer action, rather than at the end of frame.
                        mapped[index].consume();
                        if allow_input {
                            mapped[press].command(TerminalCommand::Paste(text.clone()));
                        }
                    }
                    None if allow_input => {
                        mapped[index].command(TerminalCommand::Paste(text.clone()))
                    }
                    None => mapped[index].consume(),
                },
                Event::Ime(ime) => {
                    mapped[index].consume();
                    match ime {
                        ImeEvent::Preedit { text, .. } if allow_input => {
                            self.composing = !text.is_empty();
                            self.preedit.clone_from(text);
                        }
                        ImeEvent::Commit(text) => {
                            self.composing = false;
                            self.preedit.clear();
                            if allow_input && !text.is_empty() {
                                mapped[index].command(TerminalCommand::Text(text.clone()));
                            }
                        }
                        // A terminal has no editable surrounding-text model.
                        // Do not turn DeleteSurrounding into guessed backspaces.
                        ImeEvent::DeleteSurrounding { .. } => {}
                        _ => {
                            self.composing = false;
                            self.preedit.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        for (text, owner) in text_owner.into_iter().enumerate() {
            if let Some(key) = owner {
                // A reserved key and its paired text remain for Workbench;
                // terminal/local keys consume their text exactly once.
                mapped[text].consumed = mapped[key].consumed;
            }
        }
        mapped
    }
}

// Workbench supplies physical chord identities in KeyboardShortcut's key field.
// Logical egui keys remain authoritative for terminal text/clipboard conventions.
fn is_reserved(key: Key, modifiers: Modifiers, reserved: &[KeyboardShortcut]) -> bool {
    reserved
        .iter()
        .any(|shortcut| shortcut.logical_key == key && modifiers.matches_exact(shortcut.modifiers))
}

enum LocalShortcut {
    Copy,
    Paste,
    SelectAll,
    Page(i32),
    Edge(bool),
}

fn local_shortcut(key: Key, modifiers: Modifiers, mac: bool) -> Option<LocalShortcut> {
    let primary = if mac {
        (modifiers.mac_cmd || modifiers.command)
            && !modifiers.ctrl
            && !modifiers.alt
            && !modifiers.shift
    } else {
        (modifiers.ctrl || modifiers.command)
            && modifiers.shift
            && !modifiers.alt
            && !modifiers.mac_cmd
    };
    if primary {
        match key {
            Key::C => return Some(LocalShortcut::Copy),
            Key::V => return Some(LocalShortcut::Paste),
            Key::A => return Some(LocalShortcut::SelectAll),
            _ => {}
        }
    }
    if modifiers.shift
        && !modifiers.ctrl
        && !modifiers.alt
        && !modifiers.mac_cmd
        && !modifiers.command
    {
        return match key {
            Key::PageUp => Some(LocalShortcut::Page(1)),
            Key::PageDown => Some(LocalShortcut::Page(-1)),
            Key::Home => Some(LocalShortcut::Edge(true)),
            Key::End => Some(LocalShortcut::Edge(false)),
            _ => None,
        };
    }
    match key {
        Key::Copy => Some(LocalShortcut::Copy),
        Key::Paste => Some(LocalShortcut::Paste),
        _ => None,
    }
}

fn terminal_modifiers(modifiers: Modifiers, mac: bool) -> TerminalModifiers {
    TerminalModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.ctrl || modifiers.command && !mac,
        super_key: modifiers.mac_cmd || modifiers.command && mac,
        ..TerminalModifiers::default()
    }
}

fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

fn terminal_key_event(
    key: Key,
    physical_key: Option<Key>,
    modifiers: Modifiers,
    repeat: bool,
    text: Option<&str>,
    mac: bool,
) -> TerminalKeyEvent {
    // egui falls back to a physical Latin key for logical non-Latin text.
    // Do not claim that fallback is the layout's unshifted Unicode keysym.
    // The encoder can use the actual Character identity when this is absent.
    let unshifted = if text.is_some_and(|text| !text.is_ascii() && text.chars().count() == 1) {
        None
    } else {
        character(key, false)
    };
    let resulting = text
        .filter(|text| text.chars().count() == 1)
        .map(str::to_owned)
        .or_else(|| character(key, modifiers.shift));
    let terminal_key = match key {
        Key::ArrowUp => TerminalKey::Up,
        Key::ArrowDown => TerminalKey::Down,
        Key::ArrowLeft => TerminalKey::Left,
        Key::ArrowRight => TerminalKey::Right,
        Key::Enter => TerminalKey::Enter,
        Key::Tab => TerminalKey::Tab,
        Key::Backspace => TerminalKey::Backspace,
        Key::Escape => TerminalKey::Escape,
        Key::Insert => TerminalKey::Insert,
        Key::Delete => TerminalKey::Delete,
        Key::Home => TerminalKey::Home,
        Key::End => TerminalKey::End,
        Key::PageUp => TerminalKey::PageUp,
        Key::PageDown => TerminalKey::PageDown,
        Key::ShiftLeft | Key::ShiftRight => TerminalKey::Shift,
        Key::ControlLeft | Key::ControlRight => TerminalKey::Control,
        Key::AltLeft | Key::AltRight => TerminalKey::Alt,
        Key::SuperLeft | Key::SuperRight => TerminalKey::Super,
        _ => function_number(key)
            .map(TerminalKey::F)
            .or_else(|| resulting.clone().map(TerminalKey::Character))
            .unwrap_or(TerminalKey::Unknown),
    };
    let location = match key {
        Key::ShiftLeft | Key::ControlLeft | Key::AltLeft | Key::SuperLeft => KeyLocation::Left,
        Key::ShiftRight | Key::ControlRight | Key::AltRight | Key::SuperRight => KeyLocation::Right,
        _ => KeyLocation::Standard,
    };
    TerminalKeyEvent {
        key: terminal_key,
        unshifted_key: unshifted,
        shifted_key: modifiers.shift.then_some(resulting).flatten(),
        base_layout_key: physical_key
            .and_then(|key| character(key, false))
            .and_then(|text| text.chars().next()),
        text: text.map(str::to_owned),
        location,
        modifiers: terminal_modifiers(modifiers, mac),
        state: if repeat {
            KeyState::Repeat
        } else {
            KeyState::Press
        },
    }
}

fn function_number(key: Key) -> Option<u8> {
    const KEYS: [Key; 35] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
        Key::F21,
        Key::F22,
        Key::F23,
        Key::F24,
        Key::F25,
        Key::F26,
        Key::F27,
        Key::F28,
        Key::F29,
        Key::F30,
        Key::F31,
        Key::F32,
        Key::F33,
        Key::F34,
        Key::F35,
    ];
    KEYS.iter()
        .position(|candidate| *candidate == key)
        .map(|index| index as u8 + 1)
}

/// Logical egui keys already include punctuation where it is available. The
/// shifted fallback is only used when the backend did not supply a Text event.
fn character(key: Key, shift: bool) -> Option<String> {
    let (plain, shifted) = match key {
        Key::A => ('a', 'A'),
        Key::B => ('b', 'B'),
        Key::C => ('c', 'C'),
        Key::D => ('d', 'D'),
        Key::E => ('e', 'E'),
        Key::F => ('f', 'F'),
        Key::G => ('g', 'G'),
        Key::H => ('h', 'H'),
        Key::I => ('i', 'I'),
        Key::J => ('j', 'J'),
        Key::K => ('k', 'K'),
        Key::L => ('l', 'L'),
        Key::M => ('m', 'M'),
        Key::N => ('n', 'N'),
        Key::O => ('o', 'O'),
        Key::P => ('p', 'P'),
        Key::Q => ('q', 'Q'),
        Key::R => ('r', 'R'),
        Key::S => ('s', 'S'),
        Key::T => ('t', 'T'),
        Key::U => ('u', 'U'),
        Key::V => ('v', 'V'),
        Key::W => ('w', 'W'),
        Key::X => ('x', 'X'),
        Key::Y => ('y', 'Y'),
        Key::Z => ('z', 'Z'),
        Key::Num0 => ('0', ')'),
        Key::Num1 => ('1', '!'),
        Key::Num2 => ('2', '@'),
        Key::Num3 => ('3', '#'),
        Key::Num4 => ('4', '$'),
        Key::Num5 => ('5', '%'),
        Key::Num6 => ('6', '^'),
        Key::Num7 => ('7', '&'),
        Key::Num8 => ('8', '*'),
        Key::Num9 => ('9', '('),
        Key::Space => (' ', ' '),
        Key::Colon => (':', ':'),
        Key::Comma => (',', '<'),
        Key::Backslash | Key::IntlBackslash => ('\\', '|'),
        Key::Slash => ('/', '?'),
        Key::Pipe => ('|', '|'),
        Key::Questionmark => ('?', '?'),
        Key::Exclamationmark => ('!', '!'),
        Key::OpenBracket => ('[', '{'),
        Key::CloseBracket => (']', '}'),
        Key::OpenCurlyBracket => ('{', '{'),
        Key::CloseCurlyBracket => ('}', '}'),
        Key::Backtick => ('`', '~'),
        Key::Minus => ('-', '_'),
        Key::Period => ('.', '>'),
        Key::Plus => ('+', '+'),
        Key::Equals => ('=', '+'),
        Key::Semicolon => (';', ':'),
        Key::Quote => ('\'', '"'),
        _ => return None,
    };
    Some(if shift { shifted } else { plain }.to_string())
}

pub(crate) fn mouse_modifiers(modifiers: Modifiers) -> u8 {
    u8::from(modifiers.shift) * 4
        + u8::from(modifiers.alt) * 8
        + u8::from(modifiers.ctrl || modifiers.mac_cmd) * 16
}

/// Xterm mouse reporting is independent of keyboard encoding. SGR preserves
/// release button identity and large coordinates; the legacy byte protocol uses
/// button 3 for release and can represent coordinates only through 223.
pub(crate) fn mouse_packet(
    button: u8,
    col: usize,
    row: usize,
    modifiers: u8,
    release: bool,
    sgr: bool,
) -> Vec<u8> {
    if sgr {
        return format!(
            "\x1b[<{};{};{}{}",
            button + modifiers,
            col.saturating_add(1),
            row.saturating_add(1),
            if release { 'm' } else { 'M' }
        )
        .into_bytes();
    }
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push(32 + if release { 3 } else { button } + modifiers);
    bytes.extend([col, row].map(|value| 32 + value.saturating_add(1).min(223) as u8));
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_terminal::terminal_input::{KeyboardModes, encode_key, encode_paste, encode_text};

    fn key(key: Key, modifiers: Modifiers, pressed: bool, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            modifiers,
            pressed,
            repeat,
        }
    }

    fn encoded(mapped: Vec<MappedEvent>) -> Vec<Vec<u8>> {
        mapped
            .into_iter()
            .flat_map(|event| event.actions)
            .filter_map(|action| match action {
                KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(event))) => {
                    Some(encode_key(&event, KeyboardModes::default()))
                }
                KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Text(text))) => {
                    Some(encode_text(&text, KeyboardModes::default()))
                }
                KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Paste(text))) => {
                    Some(encode_paste(&text, true))
                }
                _ => None,
            })
            .filter(|bytes| !bytes.is_empty())
            .collect()
    }

    #[test]
    fn ctrl_c_and_ctrl_v_are_not_the_clipboard_bridges_synthetic_events() {
        let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        let events = vec![
            key(Key::C, ctrl, true, false),
            key(Key::V, ctrl, true, false),
            Event::Copy,
            Event::Paste("not pasted".into()),
        ];
        let mut mapper = InputMapper::default();
        let mapped = mapper.map_events(&events, true, &[], false);
        assert!(mapped.iter().all(|event| event.consumed));
        assert_eq!(encoded(mapped), [vec![3], vec![22]]);
    }

    #[test]
    fn terminal_copy_and_paste_chords_do_not_send_keys_or_duplicate_paste() {
        for (mac, modifiers) in [
            (
                false,
                Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT,
            ),
            (true, Modifiers::MAC_CMD | Modifiers::COMMAND),
        ] {
            let mut mapper = InputMapper::default();
            let mapped = mapper.map_events(
                &[
                    key(Key::C, modifiers, true, false),
                    key(Key::V, modifiers, true, false),
                    Event::Copy,
                    Event::Paste("a\nb".into()),
                ],
                true,
                &[],
                mac,
            );
            assert!(matches!(
                mapped[0].actions.as_slice(),
                [KeyboardAction::Copy]
            ));
            assert_eq!(encoded(mapped), [b"\x1b[200~a\rb\x1b[201~".to_vec()]);
        }
    }

    #[test]
    fn reserved_application_shortcuts_and_companions_are_left_unconsumed() {
        let modifiers = Modifiers::CTRL | Modifiers::COMMAND;
        let reserved = [KeyboardShortcut::new(Modifiers::COMMAND, Key::V)];
        let events = [
            key(Key::V, modifiers, true, false),
            Event::Text("v".into()),
            Event::Paste("app".into()),
            key(Key::V, Modifiers::NONE, false, false),
        ];
        let mapped = InputMapper::default().map_events(&events, true, &reserved, false);
        assert!(
            mapped
                .iter()
                .all(|event| !event.consumed && event.actions.is_empty())
        );
    }

    #[test]
    fn physical_application_reservations_win_on_non_us_logical_keys() {
        let modifiers = Modifiers::CTRL | Modifiers::COMMAND;
        let reserved = [KeyboardShortcut::new(Modifiers::COMMAND, Key::Q)];
        let events = [
            Event::Key {
                key: Key::V,
                physical_key: Some(Key::Q),
                pressed: true,
                repeat: false,
                modifiers,
            },
            Event::Text("v".into()),
            Event::Paste("application clipboard".into()),
            Event::Key {
                key: Key::V,
                physical_key: Some(Key::Q),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];
        let mapped = InputMapper::default().map_events(&events, true, &reserved, false);
        assert!(
            mapped
                .iter()
                .all(|event| !event.consumed && event.actions.is_empty())
        );
    }

    #[test]
    fn synthesized_paste_precedes_a_later_enter_in_the_original_key_stream() {
        let modifiers = Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT;
        let events = [
            key(Key::V, modifiers, true, false),
            key(Key::Enter, Modifiers::NONE, true, false),
            Event::Paste("echo pasted".into()),
        ];
        assert_eq!(
            encoded(InputMapper::default().map_events(&events, true, &[], false)),
            [b"\x1b[200~echo pasted\x1b[201~".to_vec(), vec![b'\r'],]
        );
    }

    #[test]
    fn key_text_pairs_repeat_and_release_remain_ordered_semantic_keys() {
        let events = [
            key(Key::A, Modifiers::NONE, true, false),
            Event::Text("a".into()),
            key(Key::A, Modifiers::NONE, true, true),
            Event::Text("a".into()),
            key(Key::A, Modifiers::NONE, false, false),
            key(Key::Enter, Modifiers::NONE, true, false),
        ];
        let mapped = InputMapper::default().map_events(&events, true, &[], false);
        let keys: Vec<_> = mapped
            .into_iter()
            .flat_map(|event| event.actions)
            .filter_map(|action| match action {
                KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(key))) => {
                    Some(key)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            keys.iter().map(|key| key.state).collect::<Vec<_>>(),
            [
                KeyState::Press,
                KeyState::Repeat,
                KeyState::Release,
                KeyState::Press
            ]
        );
        assert_eq!(keys[0].text.as_deref(), Some("a"));
        assert_eq!(keys[1].text.as_deref(), Some("a"));
        assert_eq!(keys[2].text, None);
        assert_eq!(keys[3].key, TerminalKey::Enter);
    }

    #[test]
    fn altgr_preserves_ascii_and_unicode_committed_text_but_not_textless_control_chords() {
        let modifiers = Modifiers::CTRL | Modifiers::ALT | Modifiers::COMMAND;
        for (text, logical_key) in [
            ("@", Key::Num2),
            ("{", Key::OpenBracket),
            ("\\", Key::Backslash),
            ("€", Key::E),
        ] {
            for text_first in [true, false] {
                let mut events = vec![
                    Event::Text(text.into()),
                    key(logical_key, modifiers, true, false),
                ];
                if !text_first {
                    events.reverse();
                }
                events.push(key(logical_key, Modifiers::NONE, false, false));
                let mapped = InputMapper::default().map_events(&events, true, &[], false);
                assert!(mapped.iter().all(|event| event.consumed));
                let actions: Vec<_> = mapped.into_iter().flat_map(|event| event.actions).collect();
                assert!(
                    matches!(actions.as_slice(), [KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Text(committed)))] if committed == text)
                );
            }
        }
        assert_eq!(
            encoded(InputMapper::default().map_events(
                &[key(Key::A, modifiers, true, false)],
                true,
                &[],
                false
            )),
            [vec![27, 1]]
        );
    }

    #[test]
    fn modifier_changing_repeats_preserve_the_original_terminal_release_obligation() {
        for reserve_repeat in [false, true] {
            let mut mapper = InputMapper::default();
            let reserved = if reserve_repeat {
                vec![KeyboardShortcut::new(Modifiers::SHIFT, Key::PageUp)]
            } else {
                Vec::new()
            };
            let press = mapper.map_events(
                &[key(Key::PageUp, Modifiers::NONE, true, false)],
                true,
                &reserved,
                false,
            );
            assert!(
                matches!(press[0].actions.as_slice(), [KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(event)))] if event.key == TerminalKey::PageUp && event.state == KeyState::Press)
            );
            let repeat = mapper.map_events(
                &[key(Key::PageUp, Modifiers::SHIFT, true, true)],
                true,
                &reserved,
                false,
            );
            assert_eq!(repeat[0].consumed, !reserve_repeat);
            if reserve_repeat {
                assert!(repeat[0].actions.is_empty());
            } else {
                assert!(matches!(
                    repeat[0].actions.as_slice(),
                    [KeyboardAction::ScrollPage(1)]
                ));
            }
            let release = mapper.map_events(
                &[key(Key::PageUp, Modifiers::NONE, false, false)],
                true,
                &reserved,
                false,
            );
            assert!(
                matches!(release[0].actions.as_slice(), [KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(event)))] if event.key == TerminalKey::PageUp && event.state == KeyState::Release && !event.modifiers.shift)
            );
            assert_eq!(release[0].consumed, !reserve_repeat);
            assert!(
                mapper.suspend(true).is_empty(),
                "the physical release fulfilled the obligation exactly once"
            );
        }
    }

    #[test]
    fn bevy_native_text_before_key_is_paired_once_including_repeats_and_unicode() {
        let events = [
            Event::Text("a".into()),
            key(Key::A, Modifiers::NONE, true, false),
            Event::Text("a".into()),
            key(Key::A, Modifiers::NONE, true, true),
            Event::Text("你".into()),
            key(Key::N, Modifiers::NONE, true, false),
        ];
        let mapped = InputMapper::default().map_events(&events, true, &[], false);
        assert_eq!(
            encoded(mapped),
            [b"a".to_vec(), b"a".to_vec(), "你".as_bytes().to_vec()]
        );
        let mapped = InputMapper::default().map_events(&events, true, &[], false);
        assert!(
            matches!(&mapped[5].actions[0], KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(event))) if event.key == TerminalKey::Character("你".into()) && event.unshifted_key.is_none())
        );
    }

    #[test]
    fn ime_commit_is_sent_once_without_its_confirmation_key_or_duplicate_text() {
        let mut mapper = InputMapper::default();
        let preedit = mapper.map_events(
            &[Event::Ime(ImeEvent::Preedit {
                text: "ni".into(),
                active_range_chars: None,
            })],
            true,
            &[],
            false,
        );
        assert!(encoded(preedit).is_empty());
        assert_eq!(mapper.preedit, "ni");
        let mapped = mapper.map_events(
            &[
                key(Key::Enter, Modifiers::NONE, true, false),
                Event::Ime(ImeEvent::Commit("你".into())),
                Event::Text("你".into()),
            ],
            true,
            &[],
            false,
        );
        assert_eq!(encoded(mapped), ["你".as_bytes().to_vec()]);
        assert!(mapper.preedit.is_empty());
    }

    #[test]
    fn ordinary_input_after_an_ime_commit_in_the_same_frame_is_not_lost() {
        let events = [
            Event::Ime(ImeEvent::Commit("你".into())),
            Event::Text("你".into()),
            key(Key::A, Modifiers::NONE, true, false),
            Event::Text("a".into()),
        ];
        assert_eq!(
            encoded(InputMapper::default().map_events(&events, true, &[], false)),
            ["你".as_bytes().to_vec(), b"a".to_vec()]
        );
    }

    #[test]
    fn suspension_releases_only_keys_previously_sent_by_this_canvas() {
        let mut mapper = InputMapper::default();
        mapper.map_events(
            &[
                key(Key::ArrowUp, Modifiers::NONE, true, false),
                key(
                    Key::C,
                    Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT,
                    true,
                    false,
                ),
            ],
            true,
            &[],
            false,
        );
        let releases = mapper.suspend(true);
        assert!(matches!(
            releases.as_slice(),
            [TerminalInput::Command(TerminalCommand::Key(
                TerminalKeyEvent {
                    key: TerminalKey::Up,
                    state: KeyState::Release,
                    ..
                }
            ))]
        ));
        assert!(mapper.suspend(true).is_empty());
    }

    #[test]
    fn exited_canvas_can_copy_but_cannot_send_process_input() {
        let mapped = InputMapper::default().map_events(
            &[
                Event::Copy,
                Event::Paste("no".into()),
                Event::Text("no".into()),
                key(Key::C, Modifiers::CTRL, true, false),
            ],
            false,
            &[],
            false,
        );
        assert!(matches!(
            mapped[0].actions.as_slice(),
            [KeyboardAction::Copy]
        ));
        assert!(encoded(mapped).is_empty());
    }

    #[test]
    fn physical_control_and_command_are_distinct_on_macos() {
        let ctrl_c = [key(Key::C, Modifiers::CTRL, true, false)];
        assert_eq!(
            encoded(InputMapper::default().map_events(&ctrl_c, true, &[], true)),
            [vec![3]]
        );
        let mapped = InputMapper::default().map_events(
            &[key(
                Key::K,
                Modifiers::MAC_CMD | Modifiers::COMMAND,
                true,
                false,
            )],
            true,
            &[],
            true,
        );
        assert!(
            matches!(&mapped[0].actions[0], KeyboardAction::Input(TerminalInput::Command(TerminalCommand::Key(event))) if event.modifiers.super_key && !event.modifiers.control)
        );
    }

    #[test]
    fn mouse_reports_preserve_sgr_release_identity_and_clamp_only_legacy_coordinates() {
        assert_eq!(
            mouse_packet(2, 300, 255, 16, true, true),
            b"\x1b[<18;301;256m"
        );
        assert_eq!(
            mouse_packet(0, 300, 255, 0, true, false),
            [27, b'[', b'M', 35, 255, 255]
        );
    }
}
