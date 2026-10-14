//! Requested runtime configuration is ordinary ECS data. Consumers prepare later.
use crate::CoreNotice;
use bevy::prelude::*;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreConfigurationSystems {
    Settings,
}

#[cfg(feature = "language-services")]
#[derive(Resource, Default, Clone, PartialEq)]
pub struct SyntaxAppearance(pub bedtime_highlight::tree_sitter::ThemeColors);

pub(crate) fn configure(world: &mut World) {
    crate::settings::refresh_settings(world);
    let snapshot = world
        .resource::<bedtime_api::settings::SettingsSnapshot>()
        .clone();
    if let Some(mut theme) = world.get_resource_mut::<bedtime_desktop::DesktopTheme>() {
        apply_ui_preferences(&mut theme.0, &snapshot);
    }
    if let Some(add_button) = world
        .resource::<bedtime_api::settings::SettingsSnapshot>()
        .get("bedtime.workbench", "tab_bar_add_button")
        .and_then(serde_json::Value::as_bool)
    {
        world
            .resource_mut::<bedtime_workbench::WorkbenchAppearance>()
            .add_button = add_button;
    }
    let tab_icons = world
        .resource::<bedtime_api::settings::SettingsSnapshot>()
        .get("bedtime.workbench", "tab_icons")
        .and_then(serde_json::Value::as_bool)
        .expect("published tab icon setting");
    world
        .resource_mut::<bedtime_workbench::WorkbenchAppearance>()
        .tab_icons = tab_icons;
    let hidden = world
        .get_resource::<bedtime_api::features::FeatureRegistry>()
        .map(|registry| {
            registry
                .panels()
                .iter()
                .filter(|panel| !panel.empty_input)
                .map(|panel| panel.id.to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    world
        .resource_mut::<bedtime_workbench::WorkbenchAppearance>()
        .hidden_picker
        .extend(hidden);
    if let Err(error) = crate::settings::configure_document_settings(world) {
        world.write_message(CoreNotice {
            source: None,
            message: error.to_string(),
        });
    }
}

#[cfg(feature = "language-services")]
pub(crate) fn configure_syntax(world: &mut World) {
    let Some(palette) = world
        .get_resource::<SyntaxAppearance>()
        .map(|value| value.0.clone())
    else {
        return;
    };
    if let Some(mut session) = world.get_non_send_mut::<bedtime_documents::DocumentSession>() {
        session.set_highlight_theme(palette);
    }
}

/// Apply validated appearance preferences to a caller-owned theme. Palette
/// selection remains independent and can be supplied by any application.
pub fn apply_ui_preferences(
    theme: &mut bedtime_ui::appearance::UiTheme,
    snapshot: &bedtime_api::settings::SettingsSnapshot,
) {
    let values = snapshot.for_owner("bedtime.ui");
    theme.fonts.family = values["font"]
        .as_str()
        .expect("Registered font family")
        .into();
    theme.fonts.size = values["fontSize"].as_f64().expect("Validated font size") as f32;
    theme.opacity = values["background_opacity"]
        .as_f64()
        .expect("Validated opacity") as f32;
    theme.motion.enabled = values["ui_animations"]
        .as_bool()
        .expect("Registered motion preference");
}
