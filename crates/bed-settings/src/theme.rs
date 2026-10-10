//! Color-only desktop themes. Theme data never enters the preferences object.
use bed_highlight::{capture_map::THEME_KEYS, tree_sitter::ThemeColors};
use serde_json::{Value, json};
use std::{fs, io, path::Path};

pub const DEFAULT_THEME: &str = "tokyo";
pub const BUILTIN_THEMES: &[&str] = &[
    "tokyo",
    "solarized-light",
    "carbon",
    "slash",
    "catppuccin-latte",
    "catppuccin-frappe",
    "catppuccin-macchiato",
    "catppuccin-mocha",
    "rose-pine",
    "rose-pine-moon",
    "rose-pine-dawn",
    "synthwave-84",
    "everforest-dark-hard",
    "everforest-dark-medium",
    "everforest-dark-soft",
    "everforest-light-hard",
    "everforest-light-medium",
    "everforest-light-soft",
    "oxocarbon-dark",
    "oxocarbon-light",
];

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub light: bool,
    pub background: [f32; 4],
    pub window_background: [f32; 4],
    pub tab_bar: [f32; 4],
    pub surface: [f32; 4],
    pub foreground: [f32; 4],
    pub accent: [f32; 4],
    pub selection: [f32; 4],
    pub syntax: ThemeColors,
    pub terminal: [[f32; 4]; 16],
}

/// An unsaved theme, owned by each settings view. The service retains one for
/// callers of the legacy settings UI that do not supply per-view state.
#[derive(Default)]
pub struct ThemeDraft {
    pub editing: bool,
    pub source: String,
    pub destination: Option<String>,
    pub palette: Option<Theme>,
    pub json_input: String,
    pub error: Option<String>,
    pub message: Option<String>,
    pub dirty: bool,
    pub separate_tab_bar: bool,
}

impl ThemeDraft {
    pub fn load(&mut self, theme: Theme, destination: Option<String>) {
        self.editing = true;
        self.separate_tab_bar = theme.tab_bar != theme.background;
        self.palette = Some(theme);
        self.destination = destination;
        self.error = None;
        self.message = None;
        self.dirty = self.destination.is_none();
    }

    /// Failed imports leave the current draft intact.
    pub fn import_json(&mut self) -> io::Result<()> {
        let value = serde_json::from_str(&self.json_input).map_err(|e| invalid(e.to_string()))?;
        let theme = Theme::from_json(&value)?;
        self.load(theme, None);
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn rgb(value: &Value, field: &str) -> io::Result<[f32; 4]> {
    let s = value
        .as_str()
        .ok_or_else(|| invalid(format!("{field}: expected #RRGGBB")))?;
    let hex = s
        .strip_prefix('#')
        .filter(|s| s.len() == 6 && s.is_ascii())
        .ok_or_else(|| invalid(format!("{field}: expected #RRGGBB")))?;
    let n =
        u32::from_str_radix(hex, 16).map_err(|_| invalid(format!("{field}: invalid RGB color")))?;
    Ok([
        ((n >> 16) & 255) as f32 / 255.0,
        ((n >> 8) & 255) as f32 / 255.0,
        (n & 255) as f32 / 255.0,
        1.0,
    ])
}

impl Theme {
    pub fn load(path: &Path) -> io::Result<Self> {
        let value = serde_json::from_slice(&fs::read(path)?)
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        Self::from_json(&value).map_err(|e| invalid(format!("{}: {e}", path.display())))
    }
    pub fn from_json(value: &Value) -> io::Result<Self> {
        let name = value["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("Theme requires a name"))?
            .to_owned();
        let light = match value["appearance"].as_str() {
            Some("light") => true,
            Some("dark") => false,
            _ => return Err(invalid("appearance must be light or dark")),
        };
        // Explicitly read only color fields. Foreign settings such as fontSize
        // or shader_toggle cannot affect the application's preferences.
        let ui = &value["ui"];
        let background = rgb(&ui["background"], "ui.background")?;
        let window_background = match ui.get("window_background") {
            Some(color) => rgb(color, "ui.window_background")?,
            None => background,
        };
        let tab_bar = match ui.get("tab_bar") {
            Some(color) => rgb(color, "ui.tab_bar")?,
            None => background,
        };
        let mut syntax = ThemeColors::default();
        for (index, key) in THEME_KEYS.iter().enumerate() {
            syntax.slots[index] = rgb(&value["syntax"][key], &format!("syntax.{key}"))?;
        }
        let ansi = value["terminal"]
            .as_array()
            .filter(|a| a.len() == 16)
            .ok_or_else(|| invalid("terminal requires 16 ANSI colors"))?;
        let mut terminal = [[0.0; 4]; 16];
        for (index, color) in ansi.iter().enumerate() {
            terminal[index] = rgb(color, &format!("terminal[{index}]"))?;
        }
        Ok(Self {
            name,
            light,
            background,
            window_background,
            tab_bar,
            surface: rgb(&ui["surface"], "ui.surface")?,
            foreground: rgb(&ui["foreground"], "ui.foreground")?,
            accent: rgb(&ui["accent"], "ui.accent")?,
            selection: rgb(&ui["selection"], "ui.selection")?,
            syntax,
            terminal,
        })
    }

    /// Export only theme fields, excluding foreign preferences from imports.
    pub fn to_json(&self) -> Value {
        let hex = |color: [f32; 4]| {
            let [r, g, b, _] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            format!("#{r:02x}{g:02x}{b:02x}")
        };
        let mut value = json!({
            "name": self.name.trim(),
            "appearance": if self.light { "light" } else { "dark" },
            "ui": {
                "background": hex(self.background),
                "surface": hex(self.surface),
                "foreground": hex(self.foreground),
                "accent": hex(self.accent),
                "selection": hex(self.selection),
            },
            "syntax": {},
            "terminal": self.terminal.map(hex),
        });
        if self.tab_bar != self.background {
            value["ui"]["tab_bar"] = json!(hex(self.tab_bar));
        }
        if self.window_background != self.background {
            value["ui"]["window_background"] = json!(hex(self.window_background));
        }
        for (key, color) in THEME_KEYS.iter().zip(self.syntax.slots) {
            value["syntax"][key] = json!(hex(color));
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_json() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/themes/tokyo.json"
        )))
        .unwrap()
    }

    #[test]
    fn themes_without_a_window_background_keep_their_existing_background() {
        let mut value = theme_json();
        value["ui"]
            .as_object_mut()
            .unwrap()
            .remove("window_background");
        let theme = Theme::from_json(&value).unwrap();
        assert_eq!(theme.window_background, theme.background);
        assert_eq!(
            Theme::from_json(&theme.to_json())
                .unwrap()
                .window_background,
            theme.background
        );
    }

    #[test]
    fn window_background_roundtrips_independently_of_the_panel_background() {
        let mut value = theme_json();
        let background = value["ui"]["background"].clone();
        value["ui"]["window_background"] = json!("#123456");
        let theme = Theme::from_json(&value).unwrap();
        assert_ne!(theme.window_background, theme.background);
        let exported = theme.to_json();
        assert_eq!(exported["ui"]["background"], background);
        assert_eq!(exported["ui"]["window_background"], "#123456");
        assert_eq!(
            Theme::from_json(&exported).unwrap().window_background,
            theme.window_background
        );
    }

    #[test]
    fn malformed_window_backgrounds_are_rejected_at_the_theme_boundary() {
        for color in [Value::Null, json!(42), json!("#12345"), json!("#gggggg")] {
            let mut value = theme_json();
            value["ui"]["window_background"] = color;
            let error = Theme::from_json(&value).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("ui.window_background"));
        }
    }
}
