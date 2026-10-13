//! Settings sections contributed by application features.
use dear_imgui_rs::Ui;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct SettingsSection {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
    pub draw: Option<fn(&Ui, &mut Value) -> bool>,
}
