//! A document-free color tool. One RGBA value owns all text representations.
mod color;
mod feature;
mod ui;
use bevy::prelude::Component;
pub use feature::{APPLY_COMMAND, ColorFrontend, ColorPalette, ColorPickerPlugin};
use serde_json::{Value, json};

pub const MODULE_ID: &str = "bedtime.color";
pub const PANEL_ID: &str = "bedtime.color.panel";
pub const OPEN_COMMAND: &str = "bedtime.color.open";

fn read_color(value: &Value) -> Option<[f32; 4]> {
    let values = value.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut result = [0.0; 4];
    for (dst, src) in result.iter_mut().zip(values) {
        *dst = src.as_f64()? as f32;
    }
    color::valid(result).then_some(result)
}

/// Color data belongs to the panel entity. UI text drafts stay in its frontend.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct ColorState {
    pub rgba: [f32; 4],
    pub recent: Vec<[f32; 4]>,
    pub name: String,
    pub selected: Option<usize>,
}

impl ColorState {
    fn restore(saved: &Value) -> Self {
        Self {
            rgba: read_color(&saved["rgba"]).unwrap_or([0.45, 0.65, 1.0, 1.0]),
            recent: saved["recent"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(read_color)
                .take(16)
                .collect(),
            name: String::new(),
            selected: None,
        }
    }
    fn save(&self) -> Value {
        json!({"rgba":self.rgba,"recent":self.recent})
    }
    fn remember(&mut self) {
        self.recent.retain(|color| *color != self.rgba);
        self.recent.insert(0, self.rgba);
        self.recent.truncate(16);
    }
}

struct ColorDrafts {
    fields: [String; 7],
    error: Option<&'static str>,
    copied: Option<(usize, f64)>,
    #[cfg(test)]
    copy_points: [[f32; 2]; 7],
}

impl ColorDrafts {
    fn new(rgba: [f32; 4]) -> Self {
        Self {
            fields: color::outputs(rgba),
            error: None,
            copied: None,
            #[cfg(test)]
            copy_points: [[0.0; 2]; 7],
        }
    }
    fn choose(&mut self, state: &mut ColorState, rgba: [f32; 4]) {
        state.rgba = rgba;
        self.fields = color::outputs(rgba);
        self.error = None;
        self.copied = None;
        state.remember();
    }
}

#[cfg(test)]
static COLOR_TEST_CONTEXT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
