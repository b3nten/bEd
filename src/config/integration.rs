//! bEd submits its namespaced settings draft to the shared preference owner.
use super::Settings;
use bed_api::settings::{SettingDefinition, SettingsRegistry, SettingsResults, SettingsSnapshot};
use bevy::prelude::*;
use serde_json::Value;
use std::{collections::BTreeMap, io};

#[derive(Resource)]
struct AppSettingsRegistered;

const APPLICATION_KEYS: &[&str] = &[
    "agent_model",
    "ai_autocomplete",
    "completion_model",
    "fps_target",
    "fps_target_unfocused",
    "fps_toggle",
    "mac_blur_enabled",
    "sidebar_visible",
    "terminal_visible",
    "splitPos",
];

pub fn register_app_settings(world: &mut World) -> io::Result<()> {
    if world.contains_resource::<AppSettingsRegistered>() {
        return Ok(());
    }
    bed_core::settings::install_settings(world);
    let defaults = world.non_send::<Settings>().application_defaults();
    let missing = APPLICATION_KEYS
        .iter()
        .filter(|key| {
            world
                .resource::<SettingsRegistry>()
                .definition("bed.application", key)
                .is_none()
        })
        .map(|key| SettingDefinition::new(key, key, defaults["bed.application"][*key].clone()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bed_api::settings::register(world, "bed.application", missing).map_err(io::Error::other)?;
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
        bed_core::settings::set_application_defaults(world, &owner, defaults)?;
    }
    world.insert_resource(AppSettingsRegistered);
    Ok(())
}

/// Standalone test/application hosts have the same setup order as the launcher.
pub fn ensure_settings_view(world: &mut World) -> io::Result<()> {
    if !world.contains_resource::<AppSettingsRegistered>() {
        let path = world.non_send::<Settings>().settings_path.clone();
        bed_core::settings::initialize_settings(world, Some(path))?;
        let resources_root = world.non_send::<Settings>().resources_root.clone();
        crate::themes::install_theme_services(world, resources_root);
        register_app_settings(world)?;
        bed_core::settings::finalize_settings(world)?;
    }
    let resources_root = world.non_send::<Settings>().resources_root.clone();
    crate::themes::install_theme_services(world, resources_root);
    sync_view(world)?;
    crate::themes::configure_themes(world);
    Ok(())
}

pub fn sync_view(world: &mut World) -> io::Result<()> {
    let ids = world.non_send::<Settings>().request_ids();
    let results = ids
        .into_iter()
        .filter_map(|id| {
            world
                .resource_mut::<SettingsResults>()
                .take(id)
                .map(|result| (id, result))
        })
        .collect();
    let snapshot = world.resource::<SettingsSnapshot>().clone();
    world
        .non_send_mut::<Settings>()
        .synchronize(&snapshot, results);
    Ok(())
}

pub fn flush_settings_outbox(world: &mut World) {
    let requests = world.non_send_mut::<Settings>().take_requests();
    for request in requests {
        world.write_message(request);
    }
}

#[cfg(test)]
pub(crate) fn commit_test_draft(view: &mut Settings) -> io::Result<()> {
    use bed_api::settings::{SettingDefinition, SettingsRequest};
    let placeholder = Settings::with_paths(view.config_dir.clone(), view.resources_root.clone())?;
    let owned = std::mem::replace(view, placeholder);
    let mut app = App::new();
    #[cfg(feature = "editor")]
    app.add_plugins(bed_plugin_editor::EditorPlugin);
    let world = app.world_mut();
    #[cfg(feature = "effects")]
    bed_plugin_effects::settings::register_preferences(world);
    #[cfg(feature = "idle")]
    bed_plugin_idle::register_preferences(world);
    let paths =
        bed_api::launch::ApplicationPaths::new("bed", Some(owned.config_dir.clone()), None)?;
    world.insert_resource(paths);
    world.insert_non_send(owned);
    bed_core::settings::install_settings(world);
    #[cfg(not(feature = "editor"))]
    bed_api::settings::register(
        world,
        "bed.editor",
        vec![
            SettingDefinition::new("word_wrap", "Wrap", true),
            SettingDefinition::new("minimap", "Minimap", true),
            SettingDefinition::new("rainbow", "Rainbow", true),
            SettingDefinition::new("cursor_smear", "Animate cursor movement", true),
            SettingDefinition::new("lsp_autocomplete", "Autocomplete", true),
            SettingDefinition::new("lsp_completion_delay_ms", "Delay", 150.0),
            SettingDefinition::new("lsp_format_on_save", "Format", true),
            SettingDefinition::new("treesitter", "Syntax", true),
            SettingDefinition::new("git_changed_lines", "Git", true),
        ],
    )
    .map_err(io::Error::other)?;
    for (owner, definitions) in [
        (
            "bed.application",
            vec![SettingDefinition::new("test_close", "Close probe", "")],
        ),
        (
            "image",
            vec![
                SettingDefinition::new("fit", "Fit", false),
                SettingDefinition::new("zoom", "Zoom", 1.0),
            ],
        ),
        (
            "unavailable",
            vec![SettingDefinition::new("retained", "Retain", false)],
        ),
        (
            "fixture",
            vec![SettingDefinition::new("enabled", "Enabled", false)],
        ),
    ] {
        bed_api::settings::register(world, owner, definitions).map_err(io::Error::other)?;
    }
    let path = world.non_send::<Settings>().settings_path.clone();
    let result = (|| {
        bed_core::settings::initialize_settings(world, Some(path.clone()))?;
        let resources_root = world.non_send::<Settings>().resources_root.clone();
        crate::themes::install_theme_services(world, resources_root);
        register_app_settings(world)?;
        bed_core::settings::finalize_settings(world)?;
        // Fixture setup explicitly asks the core owner for an empty profile.
        // The Settings constructor itself creates no configuration files.
        if !path.exists() {
            bed_core::settings::apply_settings_request(world, &SettingsRequest::batch(Vec::new()))
                .map_err(io::Error::other)?;
        }
        let requests = world.non_send_mut::<Settings>().take_requests();
        let mut failure = None;
        for request in requests {
            if let Err(error) = bed_core::settings::apply_settings_request(world, &request) {
                failure.get_or_insert(error);
            }
        }
        sync_view(world)?;
        failure.map_or(Ok(()), |error| Err(io::Error::other(error)))
    })();
    *view = world.remove_non_send::<Settings>().unwrap();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::TempDir;
    use serde_json::json;
    fn view(temp: &TempDir) -> Settings {
        Settings::with_paths(
            temp.path("config"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap()
    }

    #[test]
    fn constructing_the_view_creates_no_preference_or_keybinding_files() {
        let temp = TempDir::new();
        let view = view(&temp);
        assert_eq!(view.font_size(), 20.0);
        assert!(!temp.path("config").exists());
    }

    #[cfg(all(feature = "editor", feature = "effects", feature = "idle"))]
    #[test]
    fn fresh_profile_applies_application_defaults_after_plugin_definitions() {
        let temp = TempDir::new();
        let view = view(&temp);
        let expected = view.settings.clone();
        let mut app = App::new();
        app.add_plugins(bed_plugin_editor::EditorPlugin);
        bed_plugin_effects::settings::register_preferences(app.world_mut());
        bed_plugin_idle::register_preferences(app.world_mut());
        app.insert_resource(
            bed_api::launch::ApplicationPaths::new("bed", Some(temp.path("config")), None).unwrap(),
        );
        app.insert_non_send(view);
        crate::themes::install_theme_services(
            app.world_mut(),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        );
        bed_core::settings::initialize_settings(
            app.world_mut(),
            Some(temp.path("config/settings.json")),
        )
        .unwrap();
        register_app_settings(app.world_mut()).unwrap();
        bed_core::settings::finalize_settings(app.world_mut()).unwrap();
        sync_view(app.world_mut()).unwrap();
        let snapshot = app.world().resource::<SettingsSnapshot>();
        assert_eq!(
            snapshot.get("bed.idle", "bedtime_delay_seconds"),
            Some(&json!(60))
        );
        assert_eq!(
            snapshot.get("bed.effects", "vignet_intensity"),
            Some(&expected["bed.effects"]["vignet_intensity"])
        );
        assert_eq!(
            snapshot.get("bed.editor", "cursor_smear"),
            Some(&json!(true))
        );
        assert_eq!(
            app.world().non_send::<Settings>().settings["bed.idle"]["bedtime_delay_seconds"],
            60
        );
        assert!(
            app.world()
                .non_send::<Settings>()
                .settings
                .get("plugins")
                .is_none()
        );
        assert!(!temp.path("config").exists());
    }

    #[test]
    fn draft_batches_reach_the_core_owner_and_keybindings_share_the_profile() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        current.settings["bed.ui"]["fontSize"] = json!(25.0);
        current.settings["bed.documents"]["autosave"] = json!(false);
        let mut keybindings = current.keybindings();
        keybindings
            .actions
            .insert("bed.edit.copy".into(), Vec::new());
        current.set_keybindings(keybindings);
        current.request_save();
        assert_eq!(current.request_ids().len(), 1);
        commit_test_draft(&mut current).unwrap();
        assert!(current.request_ids().is_empty());
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&current.settings_path).unwrap()).unwrap();
        assert_eq!(saved["overrides"]["bed.ui"]["fontSize"], 25.0);
        assert_eq!(saved["overrides"]["bed.documents"]["autosave"], false);
        assert_eq!(
            saved["overrides"]["bed.workbench"]["keybindings"]["actions"]["bed.edit.copy"],
            json!([])
        );
        assert!(!temp.path("config/keybindings.json").exists());
        let mut restarted = view(&temp);
        commit_test_draft(&mut restarted).unwrap();
        assert_eq!(restarted.font_size(), 25.0);
        assert!(!restarted.autosave_enabled());
    }

    #[test]
    fn plugin_drafts_save_in_their_own_namespace_and_keep_absent_plugins() {
        let temp = TempDir::new();
        let absent = json!({"state": {"future": [1, "retained", true]}});
        temp.write(
            "config/settings.json",
            &serde_json::to_vec(&json!({
                "version": 1,
                "overrides": {"third.party": absent},
            }))
            .unwrap(),
        );
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        assert_eq!(current.settings["third.party"], absent);
        current.settings["fixture"]["enabled"] = json!(true);
        current.settings["bed.ui"]["fontSize"] = json!(25.0);
        current.request_save();
        commit_test_draft(&mut current).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&current.settings_path).unwrap()).unwrap();
        assert_eq!(saved["overrides"]["fixture"]["enabled"], true);
        assert_eq!(saved["overrides"]["third.party"], absent);
        assert!(
            saved["overrides"]["bed.application"]
                .get("plugins")
                .is_none()
        );
        let mut restarted = view(&temp);
        commit_test_draft(&mut restarted).unwrap();
        assert_eq!(restarted.settings["fixture"]["enabled"], true);
        assert_eq!(restarted.settings["third.party"], absent);
        assert_eq!(restarted.font_size(), 25.0);
    }

    #[test]
    fn pending_batch_keeps_save_sequence_and_later_unsaved_draft() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        current.settings["bed.ui"]["fontSize"] = json!(24.0);
        current.request_save();
        let sequence = current.request_ids()[0].0;
        let later = bed_api::features::next_request_sequence();
        assert!(sequence < later);
        current.settings["bed.ui"]["fontSize"] = json!(28.0);
        commit_test_draft(&mut current).unwrap();
        assert_eq!(current.font_size(), 28.0);
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&current.settings_path).unwrap()).unwrap();
        assert_eq!(saved["overrides"]["bed.ui"]["fontSize"], 24.0);
        current.request_save();
        commit_test_draft(&mut current).unwrap();
        assert_eq!(current.font_size(), 28.0);
    }

    #[test]
    fn a_rejected_draft_reports_the_receipt_and_can_be_saved_again() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        current.settings["bed.ui"]["fontSize"] = json!("invalid");
        current.request_save();
        assert!(commit_test_draft(&mut current).is_err());
        assert!(current.error.is_some());
        assert!(current.request_ids().is_empty());
        assert_eq!(current.settings["bed.ui"]["fontSize"], json!(20.0));

        current.settings["bed.ui"]["fontSize"] = json!(26.0);
        current.request_save();
        commit_test_draft(&mut current).unwrap();
        assert!(current.error.is_none());
        let mut restarted = view(&temp);
        commit_test_draft(&mut restarted).unwrap();
        assert_eq!(restarted.font_size(), 26.0);
    }
}
