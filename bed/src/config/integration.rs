//! bEd registers its own preferences and overrides plugin defaults at startup.
use super::AppConfig;
use bedtime_api::settings::{SettingDefinition, SettingsRegistry};
use bevy::prelude::*;
use serde_json::Value;
use std::{collections::BTreeMap, io};

#[derive(Resource)]
struct AppSettingsRegistered;

const APPLICATION_SETTINGS: &[(&str, &str)] = &[("mac_blur_enabled", "Window blur")];

pub fn register_app_settings(world: &mut World) -> io::Result<()> {
    if world.contains_resource::<AppSettingsRegistered>() {
        return Ok(());
    }
    bedtime_core::settings::install_settings(world);
    let defaults = world.non_send::<AppConfig>().application_defaults();
    let missing = APPLICATION_SETTINGS
        .iter()
        .filter(|(key, _)| {
            world
                .resource::<SettingsRegistry>()
                .definition("bed.application", key)
                .is_none()
        })
        .map(|(key, label)| {
            SettingDefinition::new(key, label, defaults["bed.application"][*key].clone())
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bedtime_api::settings::register(world, "bed.application", missing)
            .map_err(io::Error::other)?;
    }
    let mut application_defaults = BTreeMap::<String, Vec<(String, Value)>>::new();
    for (owner, namespace) in defaults
        .as_object()
        .expect("Application default namespaces")
    {
        for (key, value) in namespace
            .as_object()
            .expect("Application default namespace")
        {
            if world
                .resource::<SettingsRegistry>()
                .definition(owner, key)
                .is_some()
            {
                application_defaults
                    .entry(owner.clone())
                    .or_default()
                    .push((key.clone(), value.clone()));
            }
        }
    }
    for (owner, defaults) in application_defaults {
        bedtime_core::settings::set_application_defaults(world, &owner, defaults)?;
    }
    world.insert_resource(AppSettingsRegistered);
    Ok(())
}

/// Explicit unit-fixture bootstrap; production launch owns settings finalization.
#[cfg(test)]
pub(crate) fn bootstrap_test_settings(world: &mut World) -> io::Result<()> {
    if !world.contains_resource::<AppSettingsRegistered>() {
        let path = world.non_send::<AppConfig>().settings_path.clone();
        bedtime_core::settings::initialize_settings(world, Some(path))?;
        let resources_root = world.non_send::<AppConfig>().resources_root.clone();
        crate::themes::install_theme_services(world, resources_root);
        register_app_settings(world)?;
        bedtime_core::settings::finalize_settings(world)?;
    }
    let resources_root = world.non_send::<AppConfig>().resources_root.clone();
    crate::themes::install_theme_services(world, resources_root);
    crate::themes::configure_themes(world);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bedtime_api::settings::SettingsSnapshot;
    use serde_json::json;
    #[test]
    fn constructing_configuration_creates_no_files() {
        let temp = TempDir::new();
        let settings =
            AppConfig::with_paths(temp.path("config"), env!("CARGO_MANIFEST_DIR").into()).unwrap();
        assert_eq!(
            settings.application_defaults()["bedtime.ui"]["fontSize"],
            20.0
        );
        assert!(!settings.config_dir.exists());
    }
    #[test]
    fn fresh_profile_applies_application_defaults_after_plugin_definitions() {
        let temp = TempDir::new();
        let settings =
            AppConfig::with_paths(temp.path("config"), env!("CARGO_MANIFEST_DIR").into()).unwrap();
        let expected = settings.application_defaults();
        let mut app = App::new();
        app.add_plugins(bedtime_plugin_editor::EditorPlugin);
        bedtime_plugin_effects::settings::register_preferences(app.world_mut());
        bedtime_plugin_idle::register_preferences(app.world_mut());
        app.insert_resource(
            bedtime_api::launch::ApplicationPaths::new("bed", Some(temp.path("config")), None)
                .unwrap(),
        );
        app.insert_non_send(settings);
        bootstrap_test_settings(app.world_mut()).unwrap();
        let snapshot = app.world().resource::<SettingsSnapshot>();
        for (owner, key) in [
            ("bedtime.idle", "bedtime_delay_seconds"),
            ("bedtime.effects", "vignet_intensity"),
            ("bedtime.editor", "cursor_smear"),
        ] {
            assert_eq!(snapshot.get(owner, key), Some(&expected[owner][key]));
        }
        assert_eq!(
            snapshot.get("bed.application", "mac_blur_enabled"),
            Some(&json!(true))
        );
        assert!(!temp.path("config/settings.json").exists());
    }
}
