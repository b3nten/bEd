//! Portable shortcut data, resolution and frame input ownership. No settings I/O.
use dear_imgui_rs::{Key, Ui};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    fmt,
    rc::Rc,
    str::FromStr,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Other,
}
impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub primary: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_key: bool,
}
impl Modifiers {
    pub fn resolved(mut self, platform: Platform) -> Self {
        if self.primary {
            if platform == Platform::Mac {
                self.super_key = true;
            } else {
                self.ctrl = true;
            }
        }
        self.primary = false;
        self
    }
    pub fn from_ui(ui: &Ui) -> Self {
        let io = ui.io();
        // ImGui swaps physical Command and Control when Mac behaviors are enabled.
        let (ctrl, super_key) = if io.config_macosx_behaviors() {
            (io.key_super(), io.key_ctrl())
        } else {
            (io.key_ctrl(), io.key_super())
        };
        Self {
            ctrl,
            super_key,
            shift: io.key_shift(),
            alt: io.key_alt(),
            primary: false,
        }
    }
    pub fn modified(self) -> bool {
        self.ctrl || self.alt || self.super_key || self.primary
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stroke {
    pub key: Key,
    pub modifiers: Modifiers,
}
impl Stroke {
    pub fn resolved(self, platform: Platform) -> Self {
        Self {
            modifiers: self.modifiers.resolved(platform),
            ..self
        }
    }
    pub fn label(self, platform: Platform) -> String {
        self.resolved(platform).to_string()
    }
}
impl fmt::Display for Stroke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (set, name) in [
            (self.modifiers.primary, "Primary"),
            (self.modifiers.ctrl, "Ctrl"),
            (self.modifiers.super_key, "Command"),
            (self.modifiers.alt, "Alt"),
            (self.modifiers.shift, "Shift"),
        ] {
            if set {
                write!(f, "{name}+")?;
            }
        }
        write!(
            f,
            "{}",
            key_name(self.key).expect("a stroke contains a bindable key")
        )
    }
}
impl FromStr for Stroke {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parts = text.split('+').map(str::trim).collect::<Vec<_>>();
        let key = parse_key(parts.pop().unwrap_or_default())
            .ok_or_else(|| format!("Unknown shortcut key: {text}"))?;
        let mut modifiers = Modifiers::default();
        for part in parts {
            let value = match part.to_ascii_lowercase().as_str() {
                "primary" => &mut modifiers.primary,
                "ctrl" | "control" => &mut modifiers.ctrl,
                "shift" => &mut modifiers.shift,
                "alt" | "option" => &mut modifiers.alt,
                "super" | "cmd" | "command" | "win" => &mut modifiers.super_key,
                _ => return Err(format!("Unknown shortcut modifier: {part}")),
            };
            if *value {
                return Err(format!("Repeated shortcut modifier: {part}"));
            }
            *value = true;
        }
        Ok(Self { key, modifiers })
    }
}
impl Serialize for Stroke {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Stroke {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

/// Editor is a reusable widget scope, independent of its containing panel type.
pub const EDITOR_SCOPE: &str = "bed.editor.text-panel";
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    /// A common operation handled by the focused widget or panel.
    Focused,
    Panel(String),
}
impl Scope {
    pub fn panel(id: impl Into<String>) -> Self {
        Self::Panel(id.into())
    }
}
#[derive(Clone, Debug)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub owner: String,
    pub scope: Scope,
    pub defaults: Vec<Stroke>,
    pub repeat: bool,
}
impl Action {
    pub fn new(id: &str, label: &str, owner: &str, scope: Scope, defaults: &[&str]) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            owner: owner.into(),
            scope,
            defaults: defaults
                .iter()
                .map(|s| s.parse().expect("valid built-in shortcut"))
                .collect(),
            repeat: false,
        }
    }
    pub fn repeating(mut self) -> Self {
        self.repeat = true;
        self
    }
}

/// One keymap for operations whose meaning follows the current focus.
pub fn common_actions() -> Vec<Action> {
    [
        ("find", "Find", vec!["Primary+F"]),
        ("copy", "Copy", vec!["Primary+C"]),
        ("cut", "Cut", vec!["Primary+X"]),
        ("paste", "Paste", vec!["Primary+V"]),
        ("select_all", "Select All", vec!["Primary+A"]),
        ("undo", "Undo", vec!["Primary+Z"]),
        (
            "redo",
            "Redo",
            if cfg!(target_os = "macos") {
                vec!["Primary+Shift+Z"]
            } else {
                vec!["Primary+Shift+Z", "Ctrl+Y"]
            },
        ),
    ]
    .into_iter()
    .map(|(name, label, mut keys)| {
        if !cfg!(target_os = "macos") {
            match name {
                "copy" => keys.push("Ctrl+Shift+C"),
                "paste" => keys.push("Ctrl+Shift+V"),
                "select_all" => keys.push("Ctrl+Shift+A"),
                _ => {}
            }
        }
        Action::new(
            &format!("bed.edit.{name}"),
            label,
            "bed.edit",
            Scope::Focused,
            &keys,
        )
    })
    .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub shortcut: Stroke,
    pub order: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    pub version: u32,
    pub actions: BTreeMap<String, Vec<Assignment>>,
}
impl Default for Overrides {
    fn default() -> Self {
        Self {
            version: 1,
            actions: BTreeMap::new(),
        }
    }
}
impl Overrides {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if value.version != 1 {
            return Err(format!(
                "Unsupported keybindings version: {}",
                value.version
            ));
        }
        if value.actions.keys().any(|id| !valid_id(id)) {
            return Err("Invalid shortcut action ID".into());
        }
        if value
            .actions
            .values()
            .flatten()
            .any(|a| a.order == u64::MAX)
        {
            return Err("Assignment order must be below u64::MAX".into());
        }
        for entries in value.actions.values_mut() {
            deduplicate(entries);
        }
        Ok(value)
    }
    pub fn replace(
        &mut self,
        action: &Action,
        index: Option<usize>,
        shortcut: Stroke,
    ) -> Result<(), String> {
        let order = self
            .actions
            .values()
            .flatten()
            .map(|a| a.order)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|order| *order < u64::MAX)
            .ok_or("Shortcut assignment order exhausted")?;
        let entries = self.actions.entry(action.id.clone()).or_insert_with(|| {
            action
                .defaults
                .iter()
                .map(|&shortcut| Assignment { shortcut, order: 0 })
                .collect()
        });
        if let Some(index) = index {
            let entry = entries
                .get_mut(index)
                .ok_or("This binding was changed externally; select it again")?;
            *entry = Assignment { shortcut, order };
        } else {
            entries.push(Assignment { shortcut, order });
        }
        deduplicate(entries);
        Ok(())
    }
    pub fn remove(&mut self, action: &Action, index: usize) -> Result<(), String> {
        let entries = self.actions.entry(action.id.clone()).or_insert_with(|| {
            action
                .defaults
                .iter()
                .map(|&shortcut| Assignment { shortcut, order: 0 })
                .collect()
        });
        if index >= entries.len() {
            return Err("This binding was changed externally; select it again".into());
        }
        entries.remove(index);
        Ok(())
    }
}
fn deduplicate(entries: &mut Vec<Assignment>) {
    let mut unique: Vec<Assignment> = Vec::new();
    for entry in entries.drain(..) {
        if let Some(existing) = unique.iter_mut().find(|a| {
            a.shortcut.resolved(Platform::current()) == entry.shortcut.resolved(Platform::current())
        }) {
            if entry.order >= existing.order {
                *existing = entry;
            }
        } else {
            unique.push(entry);
        }
    }
    *entries = unique;
}
fn valid_id(id: &str) -> bool {
    id.split('.').all(|s| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    })
}
#[derive(Clone, Debug)]
pub struct Binding {
    pub action: String,
    pub scope: Scope,
    pub shortcut: Stroke,
    pub order: u64,
    pub repeat: bool,
}
#[derive(Clone, Debug)]
pub struct Bindings {
    pub actions: Vec<Action>,
    pub entries: Vec<Binding>,
    pub platform: Platform,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionKind {
    Conflict,
    Overlap,
    Reuse,
}
#[derive(Clone, Debug)]
pub struct Collision {
    pub shortcut: Stroke,
    pub first: String,
    pub second: String,
    pub kind: CollisionKind,
    pub winner: Option<String>,
}
impl Bindings {
    pub fn new(actions: Vec<Action>, overrides: &Overrides, platform: Platform) -> Self {
        let mut entries = Vec::new();
        for action in &actions {
            let assignments = overrides
                .actions
                .get(&action.id)
                .cloned()
                .unwrap_or_else(|| {
                    action
                        .defaults
                        .iter()
                        .map(|&shortcut| Assignment { shortcut, order: 0 })
                        .collect()
                });
            // Normalize aliases and deduplicate alternatives, keeping the newest assignment.
            for assignment in assignments {
                let shortcut = assignment.shortcut.resolved(platform);
                if let Some(entry) = entries
                    .iter_mut()
                    .find(|b: &&mut Binding| b.action == action.id && b.shortcut == shortcut)
                {
                    entry.order = entry.order.max(assignment.order);
                    continue;
                }
                entries.push(Binding {
                    action: action.id.clone(),
                    scope: action.scope.clone(),
                    shortcut,
                    order: assignment.order,
                    repeat: action.repeat,
                });
            }
        }
        Self {
            actions,
            entries,
            platform,
        }
    }
    pub fn winner(&self, scope: &Scope, shortcut: Stroke) -> Option<&Binding> {
        let shortcut = shortcut.resolved(self.platform);
        self.entries
            .iter()
            .filter(|b| &b.scope == scope && b.shortcut == shortcut)
            .max_by(|a, b| (a.order, &a.action).cmp(&(b.order, &b.action)))
    }
    pub fn for_action(&self, id: &str) -> Vec<&Binding> {
        self.entries.iter().filter(|b| b.action == id).collect()
    }
    pub fn active_winner(&self, scope: &Scope, shortcut: Stroke) -> Option<&Binding> {
        let shortcut = shortcut.resolved(self.platform);
        let global = self.winner(&Scope::Global, shortcut);
        if *scope == Scope::Global || shortcut.modifiers.modified() && global.is_some() {
            return global;
        }
        self.entries
            .iter()
            .filter(|b| (b.scope == *scope || b.scope == Scope::Focused) && b.shortcut == shortcut)
            .max_by(|a, b| (a.order, &a.action).cmp(&(b.order, &b.action)))
    }
    pub fn collisions(&self) -> Vec<Collision> {
        let mut result = Vec::new();
        for (index, a) in self.entries.iter().enumerate() {
            for b in &self.entries[index + 1..] {
                if a.action == b.action || a.shortcut != b.shortcut {
                    continue;
                }
                let kind = if a.scope == b.scope {
                    CollisionKind::Conflict
                } else if a.scope == Scope::Global
                    || b.scope == Scope::Global
                    || a.scope == Scope::Focused
                    || b.scope == Scope::Focused
                    || a.scope == Scope::panel(EDITOR_SCOPE)
                    || b.scope == Scope::panel(EDITOR_SCOPE)
                {
                    CollisionKind::Overlap
                } else {
                    CollisionKind::Reuse
                };
                let winner = (kind == CollisionKind::Conflict)
                    .then(|| self.winner(&a.scope, a.shortcut).unwrap().action.clone());
                result.push(Collision {
                    shortcut: a.shortcut,
                    first: a.action.clone(),
                    second: b.action.clone(),
                    kind,
                    winner,
                });
            }
        }
        result
    }
}
impl Default for Bindings {
    fn default() -> Self {
        Self::new(Vec::new(), &Overrides::default(), Platform::current())
    }
}
#[derive(Debug, Default)]
struct Frame {
    consumed: HashSet<Stroke>,
    suppressed: bool,
    frame: Option<usize>,
}
/// Scoped view of one frame's input. Clones share consumption, never a second dispatcher.
#[derive(Clone, Debug)]
pub struct ShortcutInput {
    pub bindings: Rc<Bindings>,
    pub scope: Scope,
    frame: Rc<RefCell<Frame>>,
}
impl Default for ShortcutInput {
    fn default() -> Self {
        Self::new(Bindings::default())
    }
}
impl ShortcutInput {
    pub fn new(bindings: Bindings) -> Self {
        Self {
            bindings: Rc::new(bindings),
            scope: Scope::Global,
            frame: Default::default(),
        }
    }
    pub fn with_scope(&self, scope: Scope) -> Self {
        Self {
            scope,
            ..self.clone()
        }
    }
    pub fn begin_frame(&self, ui: &Ui) {
        let mut frame = self.frame.borrow_mut();
        if frame.frame != Some(ui.frame_count()) {
            *frame = Frame {
                frame: Some(ui.frame_count()),
                ..Default::default()
            };
        }
    }
    pub fn suppress(&self, ui: &Ui) {
        self.begin_frame(ui);
        self.frame.borrow_mut().suppressed = true;
    }
    pub fn suppressed(&self, ui: &Ui) -> bool {
        self.begin_frame(ui);
        self.frame.borrow().suppressed
    }
    pub fn take(&self, ui: &Ui, action: &str) -> bool {
        self.begin_frame(ui);
        if self.frame.borrow().suppressed {
            return false;
        }
        let modifiers = Modifiers::from_ui(ui);
        for binding in self.bindings.for_action(action) {
            if (binding.scope != self.scope
                && !(binding.scope == Scope::Focused && self.scope != Scope::Global))
                || binding.shortcut.modifiers != modifiers
                || self.frame.borrow().consumed.contains(&binding.shortcut)
            {
                continue;
            }
            let winner = self.bindings.active_winner(&self.scope, binding.shortcut);
            if winner.is_none_or(|b| b.action != action) {
                continue;
            }
            if self.scope == Scope::Global {
                if ui.io().want_text_input() && !modifiers.modified() {
                    continue;
                }
                let owned = ui.with_bound_context(|| unsafe {
                    let owner = dear_imgui_rs::sys::igGetKeyOwner(binding.shortcut.key as _);
                    owner != 0 && owner != u32::MAX
                });
                if owned {
                    continue;
                }
            }
            if ui.is_key_pressed_with_repeat(binding.shortcut.key, binding.repeat) {
                self.frame.borrow_mut().consumed.insert(binding.shortcut);
                return true;
            }
        }
        false
    }
    /// Modified application shortcuts must not turn into text in custom widgets.
    pub fn global_pending(&self, ui: &Ui) -> bool {
        let modifiers = Modifiers::from_ui(ui);
        modifiers.modified()
            && self.bindings.entries.iter().any(|b| {
                b.scope == Scope::Global
                    && b.shortcut.modifiers == modifiers
                    && ui.is_key_pressed_with_repeat(b.shortcut.key, b.repeat)
            })
    }
    /// Custom widgets claim ordinary typing before panel/global action dispatch.
    pub fn claim_text(&self, ui: &Ui) {
        self.begin_frame(ui);
        let modifiers = Modifiers::from_ui(ui);
        let mut frame = self.frame.borrow_mut();
        for (key, _) in KEY_NAMES {
            if modifiers.modified()
                && self
                    .bindings
                    .winner(
                        &Scope::Global,
                        Stroke {
                            key: *key,
                            modifiers,
                        },
                    )
                    .is_some()
            {
                continue;
            }
            if ui.is_key_pressed_with_repeat(*key, true) {
                frame.consumed.insert(Stroke {
                    key: *key,
                    modifiers,
                });
            }
        }
    }
    pub fn held(&self, ui: &Ui, action: &str) -> bool {
        !self.suppressed(ui)
            && self.bindings.for_action(action).iter().any(|b| {
                (b.scope == self.scope || b.scope == Scope::Focused && self.scope != Scope::Global)
                    && b.shortcut.modifiers == Modifiers::from_ui(ui)
                    && ui.is_key_down(b.shortcut.key)
                    && self
                        .bindings
                        .active_winner(&self.scope, b.shortcut)
                        .is_some_and(|winner| winner.action == action)
            })
    }
}

pub fn parse_key(text: &str) -> Option<Key> {
    let name = text.to_ascii_lowercase();
    let alias = match name.as_str() {
        "up" => "uparrow",
        "down" => "downarrow",
        "left" => "leftarrow",
        "right" => "rightarrow",
        "return" => "enter",
        "esc" => "escape",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "spacebar" => "space",
        "," => "comma",
        "." => "period",
        "/" => "slash",
        ";" => "semicolon",
        "'" => "apostrophe",
        "-" => "minus",
        "=" => "equal",
        "[" => "leftbracket",
        "]" => "rightbracket",
        "\\" => "backslash",
        "`" | "graveaccent" => "grave",
        _ => &name,
    };
    KEY_NAMES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(alias))
        .map(|(key, _)| *key)
}
pub fn key_name(key: Key) -> Option<&'static str> {
    KEY_NAMES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, name)| *name)
}
pub fn record(ui: &Ui) -> Option<Stroke> {
    KEY_NAMES
        .iter()
        .find(|(key, _)| ui.is_key_pressed_with_repeat(*key, false))
        .map(|(key, _)| Stroke {
            key: *key,
            modifiers: Modifiers::from_ui(ui),
        })
}

pub const KEY_NAMES: &[(Key, &str)] = &[
    (Key::A, "A"),
    (Key::B, "B"),
    (Key::C, "C"),
    (Key::D, "D"),
    (Key::E, "E"),
    (Key::F, "F"),
    (Key::G, "G"),
    (Key::H, "H"),
    (Key::I, "I"),
    (Key::J, "J"),
    (Key::K, "K"),
    (Key::L, "L"),
    (Key::M, "M"),
    (Key::N, "N"),
    (Key::O, "O"),
    (Key::P, "P"),
    (Key::Q, "Q"),
    (Key::R, "R"),
    (Key::S, "S"),
    (Key::T, "T"),
    (Key::U, "U"),
    (Key::V, "V"),
    (Key::W, "W"),
    (Key::X, "X"),
    (Key::Y, "Y"),
    (Key::Z, "Z"),
    (Key::Key0, "0"),
    (Key::Key1, "1"),
    (Key::Key2, "2"),
    (Key::Key3, "3"),
    (Key::Key4, "4"),
    (Key::Key5, "5"),
    (Key::Key6, "6"),
    (Key::Key7, "7"),
    (Key::Key8, "8"),
    (Key::Key9, "9"),
    (Key::F1, "F1"),
    (Key::F2, "F2"),
    (Key::F3, "F3"),
    (Key::F4, "F4"),
    (Key::F5, "F5"),
    (Key::F6, "F6"),
    (Key::F7, "F7"),
    (Key::F8, "F8"),
    (Key::F9, "F9"),
    (Key::F10, "F10"),
    (Key::F11, "F11"),
    (Key::F12, "F12"),
    (Key::F13, "F13"),
    (Key::F14, "F14"),
    (Key::F15, "F15"),
    (Key::F16, "F16"),
    (Key::F17, "F17"),
    (Key::F18, "F18"),
    (Key::F19, "F19"),
    (Key::F20, "F20"),
    (Key::F21, "F21"),
    (Key::F22, "F22"),
    (Key::F23, "F23"),
    (Key::F24, "F24"),
    (Key::Space, "Space"),
    (Key::Enter, "Enter"),
    (Key::Escape, "Escape"),
    (Key::Tab, "Tab"),
    (Key::Backspace, "Backspace"),
    (Key::Delete, "Delete"),
    (Key::Insert, "Insert"),
    (Key::UpArrow, "UpArrow"),
    (Key::DownArrow, "DownArrow"),
    (Key::LeftArrow, "LeftArrow"),
    (Key::RightArrow, "RightArrow"),
    (Key::Home, "Home"),
    (Key::End, "End"),
    (Key::PageUp, "PageUp"),
    (Key::PageDown, "PageDown"),
    (Key::Apostrophe, "Apostrophe"),
    (Key::Comma, "Comma"),
    (Key::Minus, "Minus"),
    (Key::Period, "Period"),
    (Key::Slash, "Slash"),
    (Key::Semicolon, "Semicolon"),
    (Key::Equal, "Equal"),
    (Key::LeftBracket, "LeftBracket"),
    (Key::Backslash, "Backslash"),
    (Key::RightBracket, "RightBracket"),
    (Key::KeypadEnter, "KeypadEnter"),
    (Key::KeypadAdd, "KeypadAdd"),
    (Key::KeypadSubtract, "KeypadSubtract"),
    (Key::KeypadMultiply, "KeypadMultiply"),
    (Key::KeypadDivide, "KeypadDivide"),
    (Key::KeypadDecimal, "KeypadDecimal"),
    (Key::KeypadEqual, "KeypadEqual"),
    (Key::GraveAccent, "Grave"),
    (Key::Keypad0, "Keypad0"),
    (Key::Keypad1, "Keypad1"),
    (Key::Keypad2, "Keypad2"),
    (Key::Keypad3, "Keypad3"),
    (Key::Keypad4, "Keypad4"),
    (Key::Keypad5, "Keypad5"),
    (Key::Keypad6, "Keypad6"),
    (Key::Keypad7, "Keypad7"),
    (Key::Keypad8, "Keypad8"),
    (Key::Keypad9, "Keypad9"),
];

#[cfg(test)]
mod tests {
    static IMGUI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    use super::*;
    fn action(id: &str, scope: Scope, keys: &[&str]) -> Action {
        Action::new(id, id, "fixture", scope, keys)
    }
    #[test]
    fn exact_modifiers_aliases_and_primary_follow_the_platform() {
        let portable: Stroke = "Primary+Shift+F12".parse().unwrap();
        assert_eq!(
            portable.resolved(Platform::Mac),
            "Command+Shift+F12".parse().unwrap()
        );
        assert_eq!(
            portable.resolved(Platform::Other),
            "Ctrl+Shift+F12".parse().unwrap()
        );
        assert_ne!(
            "Command+G".parse::<Stroke>().unwrap(),
            "Ctrl+G".parse().unwrap()
        );
        assert_eq!(
            "Ctrl+Option+PgDn".parse::<Stroke>().unwrap(),
            "Ctrl+Alt+PageDown".parse().unwrap()
        );
        for invalid in [
            "Ctrl",
            "Ctrl++",
            "Ctrl+K Ctrl+C",
            "Ctrl+Ctrl+C",
            "Shift+Unknown",
        ] {
            assert!(invalid.parse::<Stroke>().is_err(), "{invalid}");
        }
    }
    #[test]
    fn alternatives_unbinding_restore_and_assignment_priority_are_persisted() {
        let a = action("fixture.a", Scope::Global, &["Primary+S", "F12"]);
        let b = action("fixture.b", Scope::Global, &["Primary+S"]);
        let mut overrides = Overrides::default();
        overrides
            .replace(&a, Some(1), "Ctrl+Alt+G".parse().unwrap())
            .unwrap();
        assert_eq!(
            overrides.actions[&a.id][0].order, 0,
            "editing an alternative must not promote its siblings"
        );
        let model = Bindings::new(vec![a.clone(), b.clone()], &overrides, Platform::Other);
        assert_eq!(
            model
                .winner(&Scope::Global, "Ctrl+S".parse().unwrap())
                .unwrap()
                .action,
            b.id
        );
        overrides
            .replace(&a, Some(0), "Primary+S".parse().unwrap())
            .unwrap();
        assert_eq!(
            Bindings::new(vec![a.clone(), b.clone()], &overrides, Platform::Other)
                .winner(&Scope::Global, "Ctrl+S".parse().unwrap())
                .unwrap()
                .action,
            a.id
        );
        let disk = serde_json::to_vec(&overrides).unwrap();
        assert_eq!(Overrides::parse(&disk).unwrap(), overrides);
        overrides.actions.insert(a.id.clone(), vec![]);
        assert!(
            Bindings::new(vec![a.clone()], &overrides, Platform::Other)
                .for_action(&a.id)
                .is_empty()
        );
        overrides.actions.remove(&a.id);
        assert_eq!(
            Bindings::new(vec![a.clone()], &overrides, Platform::Other)
                .for_action(&a.id)
                .len(),
            2
        );
    }
    #[test]
    fn collisions_warn_for_all_scopes_and_defaults_do_not_depend_on_registration_order() {
        let global = action("fixture.global", Scope::Global, &["Ctrl+K"]);
        let a = action("fixture.a", Scope::panel("fixture.panel"), &["Ctrl+K"]);
        let b = action("fixture.b", Scope::panel("fixture.panel"), &["Ctrl+K"]);
        let other = action(
            "fixture.other",
            Scope::panel("fixture.other-panel"),
            &["Ctrl+K"],
        );
        let model = Bindings::new(
            vec![global, a.clone(), b.clone(), other],
            &Overrides::default(),
            Platform::Other,
        );
        let warnings = model.collisions();
        assert_eq!(warnings.len(), 6);
        assert!(
            warnings
                .iter()
                .any(|c| c.kind == CollisionKind::Conflict
                    && c.winner.as_deref() == Some("fixture.b"))
        );
        assert!(warnings.iter().any(|c| c.kind == CollisionKind::Reuse));
        assert!(warnings.iter().any(|c| c.kind == CollisionKind::Overlap));
        let reverse = Bindings::new(vec![b, a], &Overrides::default(), Platform::Other);
        assert_eq!(
            reverse
                .winner(&Scope::panel("fixture.panel"), "Ctrl+K".parse().unwrap())
                .unwrap()
                .action,
            "fixture.b"
        );
    }
    #[test]
    fn unknown_plugin_overrides_survive_and_malformed_input_is_rejected() {
        let value =
            br#"{"version":1,"actions":{"unavailable.action":[{"shortcut":"Alt+F12","order":9}]}}"#;
        let overrides = Overrides::parse(value).unwrap();
        assert!(
            Bindings::new(vec![], &overrides, Platform::Other)
                .entries
                .is_empty()
        );
        assert_eq!(
            Overrides::parse(&serde_json::to_vec(&overrides).unwrap()).unwrap(),
            overrides
        );
        for text in [
            r#"{"version":2,"actions":{}}"#,
            r#"{"version":1,"actions":{"bad id":[]}}"#,
            r#"{"version":1,"actions":{"fixture.a":[{"shortcut":"Ctrl+Bad","order":1}]}}"#,
        ] {
            assert!(Overrides::parse(text.as_bytes()).is_err());
        }
    }
    #[test]
    fn global_chords_win_over_local_bindings_and_recording_suppresses_dispatch() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        use dear_imgui_rs::{Context, FramePrepareOptions};
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context.io_mut().set_config_macosx_behaviors(false);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let declarations = vec![
            action("fixture.global", Scope::Global, &["Ctrl+K"]),
            action("fixture.panel", Scope::panel("fixture.panel"), &["Ctrl+K"]),
            action(
                "fixture.widget",
                Scope::panel("fixture.widget"),
                &["Ctrl+K"],
            ),
        ];
        let input = ShortcutInput::new(Bindings::new(
            declarations,
            &Overrides::default(),
            Platform::Other,
        ));
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::K, true);
        context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
        let ui = context.frame();
        assert!(
            !input
                .with_scope(Scope::panel("fixture.widget"))
                .take(ui, "fixture.widget")
        );
        input.claim_text(ui);
        assert!(input.take(ui, "fixture.global"));
        assert!(
            !input
                .with_scope(Scope::panel("fixture.widget"))
                .take(ui, "fixture.widget")
        );
        assert!(
            !input
                .with_scope(Scope::panel("fixture.panel"))
                .take(ui, "fixture.panel")
        );
        assert!(!input.take(ui, "fixture.global"));
        drop(context.render_legacy());
        context.io_mut().add_key_event(Key::K, false);
        context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
        let _ = context.frame();
        drop(context.render_legacy());
        context.io_mut().add_key_event(Key::K, true);
        context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
        let ui = context.frame();
        input.suppress(ui);
        assert!(!input.take(ui, "fixture.global"));
        assert_eq!(record(ui).unwrap().to_string(), "Ctrl+K");
        drop(context.render_legacy());
    }
    #[test]
    fn shared_bindings_follow_focus() {
        let overrides = Overrides::parse(
            br#"{"version":1,"actions":{
            "bed.edit.copy":[{"shortcut":"Ctrl+J","order":9}]
        }}"#,
        )
        .unwrap();
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context.io_mut().set_config_macosx_behaviors(false);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let input =
            ShortcutInput::new(Bindings::new(common_actions(), &overrides, Platform::Other));
        for scope in [EDITOR_SCOPE, "bed.csv.panel", "bed.explorer.panel"] {
            context.io_mut().add_key_event(Key::J, false);
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [600.0, 400.0],
                1.0 / 60.0,
            ));
            let _ = context.frame();
            drop(context.render_legacy());
            context.io_mut().add_key_event(Key::ModCtrl, true);
            context.io_mut().add_key_event(Key::J, true);
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [600.0, 400.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            assert!(
                input
                    .with_scope(Scope::panel(scope))
                    .take(ui, "bed.edit.copy")
            );
            assert!(
                !input
                    .with_scope(Scope::panel(scope))
                    .take(ui, "bed.edit.copy")
            );
            assert!(!input.take(ui, "bed.edit.copy"));
            drop(context.render_legacy());
        }
    }
    #[test]
    fn physical_command_and_control_match_exactly_with_both_imgui_mac_modes() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        use dear_imgui_rs::{Context, FramePrepareOptions};
        for mac_mode in [false, true] {
            let mut context = Context::create();
            context
                .set_ini_filename(None::<std::path::PathBuf>)
                .unwrap();
            context.io_mut().set_config_macosx_behaviors(mac_mode);
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let input = ShortcutInput::new(Bindings::new(
                vec![
                    action("fixture.command", Scope::Global, &["Primary+Alt+Shift+F12"]),
                    action("fixture.control", Scope::Global, &["Ctrl+Alt+Shift+F12"]),
                ],
                &Overrides::default(),
                Platform::Mac,
            ));
            for (modifier, id) in [
                (Key::ModSuper, "fixture.command"),
                (Key::ModCtrl, "fixture.control"),
            ] {
                context.io_mut().add_key_event(modifier, true);
                context.io_mut().add_key_event(Key::ModAlt, true);
                context.io_mut().add_key_event(Key::ModShift, true);
                context.io_mut().add_key_event(Key::F12, true);
                context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
                let ui = context.frame();
                assert!(input.take(ui, id), "{mac_mode:?} {id}");
                assert!(!input.take(
                    ui,
                    if id == "fixture.command" {
                        "fixture.control"
                    } else {
                        "fixture.command"
                    }
                ));
                assert_eq!(
                    record(ui).unwrap().modifiers.super_key,
                    modifier == Key::ModSuper
                );
                drop(context.render_legacy());
                for key in [modifier, Key::ModAlt, Key::ModShift, Key::F12] {
                    context.io_mut().add_key_event(key, false);
                }
                context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
                let _ = context.frame();
                drop(context.render_legacy());
            }
        }
    }
}
