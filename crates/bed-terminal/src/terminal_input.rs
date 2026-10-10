//! Keyboard and paste encodings at the terminal's input boundary.
//!
//! Keep physical modifiers here: ImGui remaps Command to Control on macOS for
//! editor shortcuts, while a terminal must distinguish them. The encodings follow
//! https://sw.kovidgoyal.net/kitty/keyboard-protocol/ and xterm's control sequences.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TerminalKey {
    Character(String),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    F(u8),
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,
    Menu,
    Shift,
    Control,
    Alt,
    Super,
    /// A composing/unknown key can still carry committed text.
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyLocation {
    #[default]
    Standard,
    Left,
    Right,
    Numpad,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyState {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

impl TerminalModifiers {
    fn parameter(self, locks: bool) -> u16 {
        1 + u16::from(self.shift)
            + 2 * u16::from(self.alt)
            + 4 * u16::from(self.control)
            + 8 * u16::from(self.super_key)
            + 64 * u16::from(locks && self.caps_lock)
            + 128 * u16::from(locks && self.num_lock)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalKeyEvent {
    pub key: TerminalKey,
    /// Key in the current layout with modifiers removed.
    pub unshifted_key: Option<String>,
    pub shifted_key: Option<String>,
    /// Physical key in the standard PC layout, for Kitty alternate reporting.
    pub base_layout_key: Option<char>,
    pub text: Option<String>,
    pub location: KeyLocation,
    pub modifiers: TerminalModifiers,
    pub state: KeyState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardModes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub newline: bool,
    pub kitty: u8,
    pub modify_other_keys: u8,
}

pub const KITTY_DISAMBIGUATE: u8 = 1;
pub const KITTY_REPORT_EVENTS: u8 = 2;
pub const KITTY_REPORT_ALTERNATE_KEYS: u8 = 4;
pub const KITTY_REPORT_ALL_KEYS: u8 = 8;
pub const KITTY_REPORT_TEXT: u8 = 16;

/// Encode one physical key event. An empty result means the mode does not report
/// that event (for example, a release at a shell prompt).
pub fn encode_key(event: &TerminalKeyEvent, modes: KeyboardModes) -> Vec<u8> {
    if matches!(event.key, TerminalKey::Unknown) && event.text.as_deref().is_none_or(str::is_empty)
    {
        return Vec::new();
    }
    let all = modes.kitty & KITTY_REPORT_ALL_KEYS != 0;
    let report_events = modes.kitty & KITTY_REPORT_EVENTS != 0;
    if event.state == KeyState::Release && !report_events {
        return Vec::new();
    }
    let modifiers = event.modifiers;
    let modified = modifiers.parameter(false) != 1;
    let text_key = matches!(event.key, TerminalKey::Character(_) | TerminalKey::Unknown);
    let recovery_key = matches!(
        event.key,
        TerminalKey::Enter | TerminalKey::Tab | TerminalKey::Backspace
    ) && !modified
        && event.location != KeyLocation::Numpad;
    // Unmodified Enter, Tab and Backspace retain their legacy encodings unless
    // the application explicitly requests all keys. This also keeps `reset`
    // usable after an application exits without restoring keyboard modes.
    let enhanced = all
        || (!recovery_key
            && (modes.kitty & KITTY_DISAMBIGUATE != 0
                && (!text_key || modifiers.alt || modifiers.control || modifiers.super_key)
                || report_events && !text_key));
    if enhanced {
        return encode_csi(event, modes);
    }
    if event.state == KeyState::Release {
        return Vec::new();
    }
    if is_modifier(&event.key) {
        return Vec::new();
    }
    if modes.kitty == 0 && modes.modify_other_keys != 0 {
        let encode_other = match modes.modify_other_keys {
            1 => modifiers.alt || modifiers.super_key,
            2 => modified,
            _ => true,
        };
        if encode_other && let Some(code) = other_key_code(event) {
            return format!("\x1b[27;{};{code}~", modifiers.parameter(false)).into_bytes();
        }
    }
    encode_legacy(event, modes)
}

/// Commit text from an IME without re-sending its composing physical key events.
pub fn encode_text(text: &str, modes: KeyboardModes) -> Vec<u8> {
    if modes.kitty & (KITTY_REPORT_ALL_KEYS | KITTY_REPORT_TEXT)
        == KITTY_REPORT_ALL_KEYS | KITTY_REPORT_TEXT
    {
        let points = text_codepoints(text);
        if points.is_empty() {
            return Vec::new();
        }
        format!("\x1b[0;1;{points}u").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

/// CRLF and LF paste as a single carriage return, as terminal input expects.
/// Strip escape bytes so pasted content cannot terminate bracketed paste early.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + if bracketed { 12 } else { 0 });
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                bytes.push(b'\r');
            }
            '\n' => bytes.push(b'\r'),
            '\x1b' if bracketed => {}
            _ => {
                let mut encoded = [0; 4];
                bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    bytes
}

fn scalar(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let character = chars.next()?;
    chars.next().is_none().then_some(character)
}

fn character_code(event: &TerminalKeyEvent) -> Option<char> {
    event.unshifted_key.as_deref().and_then(scalar).or_else(|| {
        if let TerminalKey::Character(text) = &event.key {
            scalar(text).and_then(|character| scalar(&character.to_lowercase().collect::<String>()))
        } else {
            None
        }
    })
}

fn other_key_code(event: &TerminalKeyEvent) -> Option<u32> {
    // Xterm reports the layout's resulting keysym, including Shift and Caps
    // Lock. Kitty's unshifted key identity would turn ':' into ';' here.
    match &event.key {
        TerminalKey::Enter => Some(13),
        TerminalKey::Tab => Some(9),
        TerminalKey::Backspace => Some(127),
        TerminalKey::Escape => Some(27),
        TerminalKey::Character(text) => scalar(text).map(u32::from),
        _ => None,
    }
}

fn is_modifier(key: &TerminalKey) -> bool {
    matches!(
        key,
        TerminalKey::Shift | TerminalKey::Control | TerminalKey::Alt | TerminalKey::Super
    )
}

/// Key number and terminator are a single representation used for both extended
/// key events and the familiar xterm navigation/function sequences.
fn key_code(event: &TerminalKeyEvent, keypad: bool) -> Option<(u32, char)> {
    if keypad && event.location == KeyLocation::Numpad {
        let code = match &event.key {
            TerminalKey::Character(text) => match text.as_str() {
                "0" => 57399,
                "1" => 57400,
                "2" => 57401,
                "3" => 57402,
                "4" => 57403,
                "5" => 57404,
                "6" => 57405,
                "7" => 57406,
                "8" => 57407,
                "9" => 57408,
                "." => 57409,
                "/" => 57410,
                "*" => 57411,
                "-" => 57412,
                "+" => 57413,
                "=" => 57415,
                "," => 57416,
                _ => return None,
            },
            TerminalKey::Enter => 57414,
            TerminalKey::Left => 57417,
            TerminalKey::Right => 57418,
            TerminalKey::Up => 57419,
            TerminalKey::Down => 57420,
            TerminalKey::PageUp => 57421,
            TerminalKey::PageDown => 57422,
            TerminalKey::Home => 57423,
            TerminalKey::End => 57424,
            TerminalKey::Insert => 57425,
            TerminalKey::Delete => 57426,
            _ => return None,
        };
        return Some((code, 'u'));
    }
    Some(match &event.key {
        TerminalKey::Character(_) => (u32::from(character_code(event)?), 'u'),
        TerminalKey::Unknown => (0, 'u'),
        TerminalKey::Enter => (13, 'u'),
        TerminalKey::Tab => (9, 'u'),
        TerminalKey::Backspace => (127, 'u'),
        TerminalKey::Escape => (27, 'u'),
        TerminalKey::Up => (1, 'A'),
        TerminalKey::Down => (1, 'B'),
        TerminalKey::Right => (1, 'C'),
        TerminalKey::Left => (1, 'D'),
        TerminalKey::Home => (1, 'H'),
        TerminalKey::End => (1, 'F'),
        TerminalKey::Insert => (2, '~'),
        TerminalKey::Delete => (3, '~'),
        TerminalKey::PageUp => (5, '~'),
        TerminalKey::PageDown => (6, '~'),
        TerminalKey::F(1) => (1, 'P'),
        TerminalKey::F(2) => (1, 'Q'),
        TerminalKey::F(3) => (13, '~'),
        TerminalKey::F(4) => (1, 'S'),
        TerminalKey::F(5) => (15, '~'),
        TerminalKey::F(6) => (17, '~'),
        TerminalKey::F(7) => (18, '~'),
        TerminalKey::F(8) => (19, '~'),
        TerminalKey::F(9) => (20, '~'),
        TerminalKey::F(10) => (21, '~'),
        TerminalKey::F(11) => (23, '~'),
        TerminalKey::F(12) => (24, '~'),
        TerminalKey::F(number @ 13..=35) => (57376 + u32::from(*number) - 13, 'u'),
        TerminalKey::F(_) => return None,
        TerminalKey::CapsLock => (57358, 'u'),
        TerminalKey::ScrollLock => (57359, 'u'),
        TerminalKey::NumLock => (57360, 'u'),
        TerminalKey::PrintScreen => (57361, 'u'),
        TerminalKey::Pause => (57362, 'u'),
        TerminalKey::Menu => (57363, 'u'),
        TerminalKey::Shift => (
            if event.location == KeyLocation::Right {
                57447
            } else {
                57441
            },
            'u',
        ),
        TerminalKey::Control => (
            if event.location == KeyLocation::Right {
                57448
            } else {
                57442
            },
            'u',
        ),
        TerminalKey::Alt => (
            if event.location == KeyLocation::Right {
                57449
            } else {
                57443
            },
            'u',
        ),
        TerminalKey::Super => (
            if event.location == KeyLocation::Right {
                57450
            } else {
                57444
            },
            'u',
        ),
    })
}

fn encode_csi(event: &TerminalKeyEvent, modes: KeyboardModes) -> Vec<u8> {
    if is_modifier(&event.key) && modes.kitty & KITTY_REPORT_ALL_KEYS == 0 {
        return Vec::new();
    }
    let Some((code, terminator)) = key_code(event, true) else {
        return encode_text(event.text.as_deref().unwrap_or_default(), modes);
    };
    let mut encoded = format!("\x1b[{code}");
    if modes.kitty & KITTY_REPORT_ALTERNATE_KEYS != 0
        && matches!(event.key, TerminalKey::Character(_))
    {
        let shifted = event
            .modifiers
            .shift
            .then(|| event.shifted_key.as_deref().and_then(scalar))
            .flatten();
        let base = event
            .base_layout_key
            .filter(|base| u32::from(*base) != code);
        if shifted.is_some() || base.is_some() {
            encoded.push(':');
            if let Some(shifted) = shifted {
                encoded.push_str(&u32::from(shifted).to_string());
            }
            if let Some(base) = base {
                encoded.push(':');
                encoded.push_str(&u32::from(base).to_string());
            }
        }
    }
    let report_text = modes.kitty & (KITTY_REPORT_ALL_KEYS | KITTY_REPORT_TEXT)
        == KITTY_REPORT_ALL_KEYS | KITTY_REPORT_TEXT;
    let points = if report_text && event.state != KeyState::Release {
        text_codepoints(event.text.as_deref().unwrap_or_default())
    } else {
        String::new()
    };
    let locks = modes.kitty & KITTY_REPORT_ALL_KEYS != 0
        || !matches!(event.key, TerminalKey::Character(_) | TerminalKey::Unknown);
    let modifiers = event.modifiers.parameter(locks);
    let event_type = if modes.kitty & KITTY_REPORT_EVENTS != 0 {
        match event.state {
            KeyState::Press => None,
            KeyState::Repeat => Some(2),
            KeyState::Release => Some(3),
        }
    } else {
        None
    };
    if modifiers != 1 || event_type.is_some() || !points.is_empty() {
        encoded.push(';');
        encoded.push_str(&modifiers.to_string());
        if let Some(event_type) = event_type {
            encoded.push(':');
            encoded.push_str(&event_type.to_string());
        }
    }
    if !points.is_empty() {
        encoded.push(';');
        encoded.push_str(&points);
    }
    // Navigation keys omit their default argument when no modifiers are present.
    if code == 1 && terminator != 'u' && encoded == "\x1b[1" {
        encoded.pop();
    }
    encoded.push(terminator);
    encoded.into_bytes()
}

fn text_codepoints(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .map(|character| u32::from(character).to_string())
        .collect::<Vec<_>>()
        .join(":")
}

fn encode_legacy(event: &TerminalKeyEvent, modes: KeyboardModes) -> Vec<u8> {
    let modifiers = event.modifiers;
    if modifiers.super_key {
        // Unhandled Command/Super chords must not accidentally become shell text.
        return Vec::new();
    }
    if event.location == KeyLocation::Numpad
        && modes.application_keypad
        && modifiers.parameter(false) == 1
    {
        let suffix = match &event.key {
            TerminalKey::Character(text) => match text.as_str() {
                "0" => Some('p'),
                "1" => Some('q'),
                "2" => Some('r'),
                "3" => Some('s'),
                "4" => Some('t'),
                "5" => Some('u'),
                "6" => Some('v'),
                "7" => Some('w'),
                "8" => Some('x'),
                "9" => Some('y'),
                "." => Some('n'),
                "/" => Some('o'),
                "*" => Some('j'),
                "-" => Some('m'),
                "+" => Some('k'),
                "=" => Some('X'),
                "," => Some('l'),
                _ => None,
            },
            TerminalKey::Enter => Some('M'),
            _ => None,
        };
        if let Some(suffix) = suffix {
            return format!("\x1bO{suffix}").into_bytes();
        }
    }
    let plain = match &event.key {
        TerminalKey::Enter => {
            if modes.newline {
                b"\r\n".to_vec()
            } else {
                vec![b'\r']
            }
        }
        TerminalKey::Tab => {
            if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            }
        }
        TerminalKey::Backspace => vec![if modifiers.control { 8 } else { 127 }],
        TerminalKey::Escape => vec![27],
        TerminalKey::Character(text) => {
            if modifiers.control {
                let character = character_code(event).or_else(|| scalar(text));
                match character.and_then(control_byte) {
                    Some(byte) => vec![byte],
                    None => text.as_bytes().to_vec(),
                }
            } else {
                event.text.as_deref().unwrap_or(text).as_bytes().to_vec()
            }
        }
        TerminalKey::Unknown => event
            .text
            .as_deref()
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
        _ => {
            let Some((code, terminator)) = key_code(event, false) else {
                return Vec::new();
            };
            let parameter = modifiers.parameter(false);
            if parameter == 1 {
                if matches!(event.key, TerminalKey::F(1..=4)) {
                    let suffix = match event.key {
                        TerminalKey::F(1) => 'P',
                        TerminalKey::F(2) => 'Q',
                        TerminalKey::F(3) => 'R',
                        _ => 'S',
                    };
                    return format!("\x1bO{suffix}").into_bytes();
                }
                if code == 1 && terminator != 'u' {
                    return format!(
                        "\x1b{}{terminator}",
                        if modes.application_cursor { 'O' } else { '[' }
                    )
                    .into_bytes();
                }
                return format!("\x1b[{code}{terminator}").into_bytes();
            }
            return format!("\x1b[{code};{parameter}{terminator}").into_bytes();
        }
    };
    if modifiers.alt {
        let mut prefixed = Vec::with_capacity(plain.len() + 1);
        prefixed.push(27);
        prefixed.extend_from_slice(&plain);
        prefixed
    } else {
        plain
    }
}

fn control_byte(character: char) -> Option<u8> {
    Some(match character.to_ascii_lowercase() {
        'a'..='z' => character.to_ascii_lowercase() as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 27,
        '\\' | '4' => 28,
        ']' | '5' => 29,
        '^' | '~' | '6' => 30,
        '_' | '/' | '7' => 31,
        '?' | '8' => 127,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: TerminalKey) -> TerminalKeyEvent {
        TerminalKeyEvent {
            key,
            ..Default::default()
        }
    }

    #[test]
    fn shift_enter_distinguishes_enhanced_apps_and_keeps_shell_enter() {
        let mut event = key(TerminalKey::Enter);
        event.modifiers.shift = true;
        assert_eq!(encode_key(&event, KeyboardModes::default()), b"\r");
        let modes = KeyboardModes {
            kitty: KITTY_DISAMBIGUATE,
            ..Default::default()
        };
        assert_eq!(encode_key(&event, modes), b"\x1b[13;2u");
        event.modifiers.shift = false;
        assert_eq!(encode_key(&event, modes), b"\r");
        assert_eq!(
            encode_key(
                &event,
                KeyboardModes {
                    kitty: KITTY_REPORT_ALL_KEYS,
                    ..modes
                }
            ),
            b"\x1b[13u"
        );
    }

    #[test]
    fn physical_control_keeps_interrupt_and_quote_at_a_shell_prompt() {
        for (character, expected) in [("c", 3), ("v", 22), ("[", 27), ("/", 31), ("2", 0)] {
            let mut event = key(TerminalKey::Character(character.into()));
            event.modifiers.control = true;
            assert_eq!(encode_key(&event, KeyboardModes::default()), [expected]);
        }
        let mut command = key(TerminalKey::Character("c".into()));
        command.modifiers.super_key = true;
        assert!(encode_key(&command, KeyboardModes::default()).is_empty());
        let mut control = command;
        control.modifiers.super_key = false;
        control.modifiers.control = true;
        assert_eq!(
            encode_key(
                &control,
                KeyboardModes {
                    kitty: KITTY_DISAMBIGUATE,
                    ..Default::default()
                }
            ),
            b"\x1b[99;5u"
        );
    }

    #[test]
    fn navigation_and_function_keys_honor_modifiers_and_cursor_mode() {
        let mut event = key(TerminalKey::Home);
        assert_eq!(encode_key(&event, KeyboardModes::default()), b"\x1b[H");
        let modes = KeyboardModes {
            application_cursor: true,
            ..Default::default()
        };
        assert_eq!(encode_key(&event, modes), b"\x1bOH");
        event.modifiers.control = true;
        assert_eq!(encode_key(&event, modes), b"\x1b[1;5H");
        event.key = TerminalKey::F(3);
        assert_eq!(
            encode_key(
                &event,
                KeyboardModes {
                    kitty: KITTY_DISAMBIGUATE,
                    ..modes
                }
            ),
            b"\x1b[13;5~"
        );
        event.modifiers.control = false;
        assert_eq!(encode_key(&event, modes), b"\x1bOR");
    }

    #[test]
    fn repeats_releases_and_recovery_keys_follow_negotiation() {
        let mut event = key(TerminalKey::Left);
        event.state = KeyState::Release;
        assert!(encode_key(&event, KeyboardModes::default()).is_empty());
        let modes = KeyboardModes {
            kitty: KITTY_REPORT_EVENTS | KITTY_DISAMBIGUATE,
            ..Default::default()
        };
        assert_eq!(encode_key(&event, modes), b"\x1b[1;1:3D");
        event.state = KeyState::Repeat;
        assert_eq!(encode_key(&event, modes), b"\x1b[1;1:2D");
        event.key = TerminalKey::Enter;
        assert_eq!(encode_key(&event, modes), b"\r");
        event.state = KeyState::Release;
        assert!(encode_key(&event, modes).is_empty());
    }

    #[test]
    fn alternate_layout_and_associated_unicode_text_preserve_identity() {
        let mut event = key(TerminalKey::Character("Я".into()));
        event.unshifted_key = Some("я".into());
        event.shifted_key = Some("Я".into());
        event.base_layout_key = Some('z');
        event.modifiers.shift = true;
        event.text = Some("Я\u{301}".into());
        let modes = KeyboardModes {
            kitty: KITTY_REPORT_ALL_KEYS | KITTY_REPORT_ALTERNATE_KEYS | KITTY_REPORT_TEXT,
            ..Default::default()
        };
        assert_eq!(
            encode_key(&event, modes),
            "\x1b[1103:1071:122;2;1071:769u".as_bytes()
        );
        assert_eq!(encode_text("日本語", modes), b"\x1b[0;1;26085:26412:35486u");
        assert_eq!(
            encode_text("日本語", KeyboardModes::default()),
            "日本語".as_bytes()
        );
    }

    #[test]
    fn keypad_and_modifier_keys_have_distinct_extended_codes() {
        let mut event = key(TerminalKey::Enter);
        event.location = KeyLocation::Numpad;
        assert_eq!(
            encode_key(
                &event,
                KeyboardModes {
                    application_keypad: true,
                    ..Default::default()
                }
            ),
            b"\x1bOM"
        );
        let modes = KeyboardModes {
            kitty: KITTY_DISAMBIGUATE,
            ..Default::default()
        };
        assert_eq!(encode_key(&event, modes), b"\x1b[57414u");
        event.key = TerminalKey::Control;
        event.location = KeyLocation::Right;
        event.modifiers.control = true;
        assert!(encode_key(&event, modes).is_empty());
        assert_eq!(
            encode_key(
                &event,
                KeyboardModes {
                    kitty: KITTY_REPORT_ALL_KEYS,
                    ..modes
                }
            ),
            b"\x1b[57448;5u"
        );
    }

    #[test]
    fn modify_other_keys_and_paste_match_xterm_input() {
        let mut event = key(TerminalKey::Enter);
        event.modifiers.shift = true;
        assert_eq!(
            encode_key(
                &event,
                KeyboardModes {
                    modify_other_keys: 2,
                    ..Default::default()
                }
            ),
            b"\x1b[27;2;13~"
        );
        assert_eq!(
            encode_paste("one\r\ntwo\n日本語\r", false),
            "one\rtwo\r日本語\r".as_bytes()
        );
        assert_eq!(
            encode_paste("line\n\x1b[201~", true),
            b"\x1b[200~line\r[201~\x1b[201~"
        );
    }

    #[test]
    fn xterm_modified_keys_preserve_shifted_punctuation_and_letter_case() {
        let modes = KeyboardModes {
            modify_other_keys: 2,
            ..Default::default()
        };
        for (text, unshifted, expected) in [
            (":", ";", "\x1b[27;2;58~"),
            ("!", "1", "\x1b[27;2;33~"),
            ("A", "a", "\x1b[27;2;65~"),
        ] {
            let event = TerminalKeyEvent {
                key: TerminalKey::Character(text.into()),
                unshifted_key: Some(unshifted.into()),
                shifted_key: Some(text.into()),
                text: Some(text.into()),
                modifiers: TerminalModifiers {
                    shift: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert_eq!(encode_key(&event, modes), expected.as_bytes());
            let kitty = encode_key(
                &event,
                KeyboardModes {
                    kitty: KITTY_REPORT_ALL_KEYS,
                    ..Default::default()
                },
            );
            assert_eq!(
                kitty,
                format!("\x1b[{};2u", unshifted.as_bytes()[0]).as_bytes()
            );
        }
    }
}
