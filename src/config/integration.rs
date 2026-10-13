//! bEd maps its private settings form into the shared preference owner's transactions.
use super::Settings;
use bed_api::settings::{SettingDefinition, SettingsRegistry, SettingsResults, SettingsSnapshot};
use bevy::prelude::*;
use serde_json::Value;
use std::{collections::BTreeMap, io};

#[derive(Resource)]
struct AppSettingsRegistered;

pub fn settings_owner(key: &str) -> &'static str {
    match key {
        "font" | "fontSize" | "background_opacity" | "ui_animations" => "bed.ui",
        "autosave" | "autosave_delay_ms" | "default_viewers" => "bed.documents",
        "tab_bar_add_button" => "bed.workbench",
        "word_wrap"
        | "minimap"
        | "rainbow"
        | "lsp_autocomplete"
        | "lsp_completion_delay_ms"
        | "lsp_format_on_save"
        | "git_changed_lines"
        | "treesitter" => "bed.editor",
        _ => "bed.application",
    }
}

const APPLICATION_KEYS: &[&str] = &[
    "agent_model",
    "ai_autocomplete",
    "bedtime_enabled",
    "bedtime_delay_seconds",
    "completion_model",
    "fps_target",
    "fps_target_unfocused",
    "fps_toggle",
    "mac_blur_enabled",
    "sidebar_visible",
    "terminal_visible",
    "splitPos",
    "theme",
    "effect_preset",
    "cursor_smear",
    "bloom_intensity",
    "burnin_intensity",
    "colorshift_intensity",
    "curvature_intensity",
    "jitter_intensity",
    "pixel_width",
    "pixelation_intensity",
    "scanline_intensity",
    "shader_toggle",
    "static_intensity",
    "vignet_intensity",
    "pulse_intensity",
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
        register_app_settings(world)?;
        bed_core::settings::finalize_settings(world)?;
    }
    sync_view(world)
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
    let syntax = world.non_send::<Settings>().highlight_colors();
    if world
        .get_resource::<bed_core::SyntaxAppearance>()
        .is_none_or(|appearance| appearance.0 != syntax)
    {
        world.insert_resource(bed_core::SyntaxAppearance(syntax));
    }
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

    #[test]
    fn draft_batches_reach_the_core_owner_and_keybindings_share_the_profile() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        current.settings["fontSize"] = json!(25.0);
        current.settings["autosave"] = json!(false);
        current.keybinds.clear("bed.edit.copy").unwrap();
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
    fn pending_batch_keeps_save_sequence_and_later_unsaved_draft() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        current.settings["fontSize"] = json!(24.0);
        current.request_save();
        let sequence = current.request_ids()[0].0;
        let later = bed_api::features::next_request_sequence();
        assert!(sequence < later);
        current.settings["fontSize"] = json!(28.0);
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
        current.settings["fontSize"] = json!("invalid");
        current.request_save();
        assert!(commit_test_draft(&mut current).is_err());
        assert!(current.error.is_some());
        assert!(current.request_ids().is_empty());
        assert_eq!(current.settings["fontSize"], json!(20.0));

        current.settings["fontSize"] = json!(26.0);
        current.request_save();
        commit_test_draft(&mut current).unwrap();
        assert!(current.error.is_none());
        let mut restarted = view(&temp);
        commit_test_draft(&mut restarted).unwrap();
        assert_eq!(restarted.font_size(), 26.0);
    }

    #[test]
    fn theme_files_remain_color_only_and_preference_changes_use_core() {
        let temp = TempDir::new();
        let mut current = view(&temp);
        commit_test_draft(&mut current).unwrap();
        let mut theme = current.theme.to_json();
        theme["name"] = json!("Custom theme");
        theme["fontSize"] = json!(40);
        let selection = current.save_custom_theme(None, &theme).unwrap();
        current.select_theme(&selection).unwrap();
        commit_test_draft(&mut current).unwrap();
        let theme: Value = serde_json::from_slice(
            &std::fs::read(current.theme_path(&selection).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(theme.get("fontSize").is_none());
        assert_eq!(current.font_size(), 20.0);
        let mut restarted = view(&temp);
        commit_test_draft(&mut restarted).unwrap();
        assert_eq!(restarted.theme.name, "Custom theme");
    }
}
