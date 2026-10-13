//! Settings sections contributed by application features.
use dear_imgui_rs::Ui;
use serde_json::Value;

/// Application-owned preferences, readable without document or workspace services.
#[derive(bevy::prelude::Resource, Clone)]
pub struct SettingsSnapshot {
    pub settings: Value,
    pub default_viewers: Value,
}

impl Default for SettingsSnapshot {
    fn default() -> Self {
        Self {
            settings: serde_json::json!({}),
            default_viewers: serde_json::json!({}),
        }
    }
}

#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct SettingsRequest {
    pub plugin: String,
    pub key: String,
    pub value: Value,
    pub sequence: u64,
}
impl SettingsRequest {
    pub fn new(plugin: impl Into<String>, key: impl Into<String>, value: Value) -> Self {
        Self {
            plugin: plugin.into(),
            key: key.into(),
            value,
            sequence: crate::features::next_request_sequence(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SettingsSection {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
    pub draw: Option<fn(&Ui, &mut Value) -> bool>,
}
