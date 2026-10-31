//! Key chords: portable modifiers plus a key, written as "Primary+Shift+Z".
use bevy::prelude::*;
use std::{borrow::Cow, fmt, ops::BitOr};

/// Portable modifier data; no native menu or egui types occur in the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers: a key on its own (F12).
    pub const NONE: Self = Self(0);
    pub const CONTROL: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const META: Self = Self(8);
    /// Cmd on macOS, Ctrl elsewhere.
    pub const PRIMARY: Self = Self(16);
    pub const fn contains(self, rhs: Self) -> bool {
        self.0 & rhs.0 == rhs.0
    }
    pub const fn union(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
    /// With `PRIMARY` replaced by this platform's modifier.
    pub fn resolved(self) -> Self {
        let primary = if !self.contains(Self::PRIMARY) {
            0
        } else if cfg!(target_os = "macos") {
            Self::META.0
        } else {
            Self::CONTROL.0
        };
        Self((self.0 & !Self::PRIMARY.0) | primary)
    }
}

impl BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: KeyCode,
}

const MODIFIER_NAMES: [(Modifiers, &str); 5] = [
    (Modifiers::PRIMARY, "Primary"),
    (Modifiers::CONTROL, "Ctrl"),
    (Modifiers::META, "Meta"),
    (Modifiers::ALT, "Alt"),
    (Modifiers::SHIFT, "Shift"),
];

impl KeyChord {
    pub const fn new(modifiers: Modifiers, key: KeyCode) -> Self {
        Self { modifiers, key }
    }

    /// `Primary+Shift+Z`.
    pub const fn primary(key: KeyCode) -> Self {
        Self::new(Modifiers::PRIMARY, key)
    }

    pub fn resolved(self) -> Self {
        Self::new(self.modifiers.resolved(), self.key)
    }

    /// For people, on this platform: "Cmd+Shift+Z" on macOS.
    pub fn label(self) -> String {
        let modifiers = self.modifiers.resolved();
        let meta = if cfg!(target_os = "macos") {
            "Cmd+"
        } else {
            "Super+"
        };
        let mut label = String::new();
        for (flag, name) in [
            (Modifiers::CONTROL, "Ctrl+"),
            (Modifiers::META, meta),
            (Modifiers::ALT, "Alt+"),
            (Modifiers::SHIFT, "Shift+"),
        ] {
            if modifiers.contains(flag) {
                label.push_str(name);
            }
        }
        label.push_str(&key_name(self.key));
        label
    }
}

/// "S" for `KeyS`, "1" for `Digit1`, else the `KeyCode` variant's name.
fn key_name(key: KeyCode) -> String {
    let name = format!("{key:?}");
    name.strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .unwrap_or(&name)
        .to_owned()
}

/// The portable form kept in settings: "Primary+Shift+Z", "Ctrl+Enter".
impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in MODIFIER_NAMES {
            if self.modifiers.contains(flag) {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key))
    }
}

impl std::str::FromStr for KeyChord {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        use serde::{Deserialize, de::IntoDeserializer};
        let mut parts: Vec<_> = text.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|key| !key.is_empty());
        let key = key.ok_or_else(|| format!("{text:?} has no key"))?;
        let mut modifiers = Modifiers::default();
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "primary" => Modifiers::PRIMARY,
                "ctrl" | "control" => Modifiers::CONTROL,
                "meta" | "cmd" | "super" => Modifiers::META,
                "alt" | "option" => Modifiers::ALT,
                "shift" => Modifiers::SHIFT,
                _ => return Err(format!("{part:?} in {text:?} isn't a modifier")),
            };
            modifiers = modifiers | flag;
        }
        let name = match key.chars().next() {
            Some(c) if key.len() == 1 && c.is_ascii_alphabetic() => {
                format!("Key{}", c.to_ascii_uppercase())
            }
            Some(c) if key.len() == 1 && c.is_ascii_digit() => format!("Digit{c}"),
            _ => key.to_owned(),
        };
        let deserializer: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
            name.as_str().into_deserializer();
        let key = KeyCode::deserialize(deserializer)
            .map_err(|_| format!("{key:?} in {text:?} isn't a key"))?;
        Ok(Self::new(modifiers, key))
    }
}

impl serde::Serialize for KeyChord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for KeyChord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}
