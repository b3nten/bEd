//! Translated from ned util/keybinds.{h,cpp}; see LICENSE and NOTICE.
use dear_imgui_rs::Key;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::settings::{read_json, write_json};

pub struct KeybindsManager {
    pub keybinds: Value,
    keys: BTreeMap<String, Key>,
    path: PathBuf,
    bundled: PathBuf,
    disk_time: Option<SystemTime>,
}

impl KeybindsManager {
    pub fn new(config_dir: &Path, resources_root: &Path) -> Self {
        Self {
            keybinds: Value::Object(Default::default()),
            keys: BTreeMap::new(),
            path: config_dir.join("keybinds.json"),
            bundled: resources_root.join("resources/config/keybinds.json"),
            disk_time: None,
        }
    }
    pub fn load_keybinds(&mut self) -> io::Result<bool> {
        if !self.path.exists() {
            if let Some(dir) = self.path.parent() {
                fs::create_dir_all(dir)?;
            }
            if fs::copy(&self.bundled, &self.path).is_err() {
                write_json(&self.path, &serde_json::json!({"toggle_file_finder":"p"}))?;
            }
        }
        let value = read_json(&self.path)
            .or_else(|_| read_json(&self.path.with_file_name("default-keybinds.json")));
        match value {
            Ok(value) => {
                self.keybinds = value;
                self.rebuild_map();
                self.disk_time = modified(&self.path);
                Ok(true)
            }
            Err(_) => {
                self.keybinds = Value::Object(Default::default());
                self.rebuild_map();
                Ok(false)
            }
        }
    }
    fn rebuild_map(&mut self) {
        self.keys.clear();
        if let Some(object) = self.keybinds.as_object() {
            for (action, value) in object {
                if let Some(key) = value.as_str().and_then(string_to_imgui_key) {
                    self.keys.insert(action.clone(), key);
                }
            }
        }
    }
    pub fn check_keybinds_file(&mut self) -> io::Result<bool> {
        if !self.path.exists() {
            return self.load_keybinds();
        }
        let time = modified(&self.path);
        if time.is_none() || time <= self.disk_time {
            return Ok(false);
        }
        if let Ok(value) = read_json(&self.path) {
            self.keybinds = value;
            self.rebuild_map();
            self.disk_time = time;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn get_action_key(&self, action: &str) -> Option<Key> {
        self.keys.get(action).copied()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

pub fn string_to_imgui_key(value: &str) -> Option<Key> {
    use Key::*;
    let lower = value.to_ascii_lowercase();
    Some(match lower.as_str() {
        "a" => A,
        "b" => B,
        "c" => C,
        "d" => D,
        "e" => E,
        "f" => F,
        "g" => G,
        "h" => H,
        "i" => I,
        "j" => J,
        "k" => K,
        "l" => L,
        "m" => M,
        "n" => N,
        "o" => O,
        "p" => P,
        "q" => Q,
        "r" => R,
        "s" => S,
        "t" => T,
        "u" => U,
        "v" => V,
        "w" => W,
        "x" => X,
        "y" => Y,
        "z" => Z,
        "0" => Key0,
        "1" => Key1,
        "2" => Key2,
        "3" => Key3,
        "4" => Key4,
        "5" => Key5,
        "6" => Key6,
        "7" => Key7,
        "8" => Key8,
        "9" => Key9,
        "space" | "spacebar" => Space,
        "enter" | "return" => Enter,
        "escape" | "esc" => Escape,
        "tab" => Tab,
        "backspace" => Backspace,
        "delete" | "del" => Delete,
        "insert" | "ins" => Insert,
        "up" => UpArrow,
        "down" => DownArrow,
        "left" => LeftArrow,
        "right" => RightArrow,
        "home" => Home,
        "end" => End,
        "pageup" | "pgup" => PageUp,
        "pagedown" | "pgdn" => PageDown,
        "leftctrl" | "lctrl" => LeftCtrl,
        "rightctrl" | "rctrl" => RightCtrl,
        "leftshift" | "lshift" => LeftShift,
        "rightshift" | "rshift" => RightShift,
        "leftalt" | "lalt" => LeftAlt,
        "rightalt" | "ralt" => RightAlt,
        "leftsuper" | "lsuper" | "cmd" | "command" | "win" | "windows" => LeftSuper,
        "rightsuper" | "rsuper" => RightSuper,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        "apostrophe" | "'" => Apostrophe,
        "comma" | "," => Comma,
        "minus" | "-" => Minus,
        "period" | "." => Period,
        "slash" | "/" => Slash,
        "semicolon" | ";" => Semicolon,
        "equal" | "=" => Equal,
        "leftbracket" | "[" => LeftBracket,
        "backslash" | "\\" => Backslash,
        "rightbracket" | "]" => RightBracket,
        "graveaccent" | "`" => GraveAccent,
        _ => return Option::None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_unknowns_follow_upstream() {
        for (name, expected) in [
            ("CMD", Key::LeftSuper),
            ("spacebar", Key::Space),
            ("pgdn", Key::PageDown),
            ("9", Key::Key9),
            (";", Key::Semicolon),
        ] {
            assert_eq!(string_to_imgui_key(name), Some(expected));
        }
        assert_eq!(string_to_imgui_key("cmd+p"), None);
        assert_eq!(string_to_imgui_key(" f1"), None);
    }
}
