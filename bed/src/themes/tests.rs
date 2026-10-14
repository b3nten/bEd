use super::*;
use crate::test_support::TempDir;
use bevy::ecs::message::MessageCursor;
use serde_json::json;

pub(crate) fn app(temp: &TempDir) -> App {
    let mut app = App::new();
    let paths =
        bedtime_api::launch::ApplicationPaths::new("bed", Some(temp.path("config")), None).unwrap();
    app.insert_resource(paths.clone());
    bedtime_core::settings::initialize_settings(
        app.world_mut(),
        Some(paths.config_dir.join("settings.json")),
    )
    .unwrap();
    install_theme_services(app.world_mut(), PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    bedtime_core::settings::finalize_settings(app.world_mut()).unwrap();
    app.insert_resource(bedtime_desktop::DesktopTheme::default());
    app
}

pub(crate) fn finish_requests(app: &mut App) {
    process_theme_requests(app);
    let requests = MessageCursor::<SettingsRequest>::default()
        .read(app.world().resource::<Messages<SettingsRequest>>())
        .cloned()
        .collect::<Vec<_>>();
    app.world_mut()
        .resource_mut::<Messages<SettingsRequest>>()
        .clear();
    for request in requests {
        let _ = bedtime_core::settings::apply_settings_request(app.world_mut(), &request);
    }
    let system = app.world_mut().register_system(finish_applies);
    app.world_mut().run_system(system).unwrap();
    app.world_mut().unregister_system(system).unwrap();
    configure_themes(app.world_mut());
}

#[test]
fn theme_resources_take_core_paths_without_creating_a_profile() {
    let temp = TempDir::new();
    let app = app(&temp);
    assert_eq!(
        app.world()
            .resource::<ThemeFiles>()
            .theme_path("themes/custom.json")
            .unwrap(),
        temp.path("config/themes/custom.json")
    );
    assert!(!temp.path("config").exists());
}

#[test]
fn save_and_apply_preserves_color_format_and_projects_all_palettes() {
    let temp = TempDir::new();
    let mut app = app(&temp);
    let mut imported = ThemeFile::bundled_default().to_json();
    imported["name"] = json!("Custom Theme");
    imported["ui"]["window_background"] = json!("#123456");
    imported["fontSize"] = json!(44);
    let file = ThemeFile::from_json(&imported).unwrap();
    let request = SaveTheme::new(None, file.clone(), true);
    let id = request.id;
    app.world_mut().write_message(request);
    assert!(
        !temp.path("config/themes").exists(),
        "UI intentions perform no writes"
    );
    finish_requests(&mut app);
    let result = app
        .world_mut()
        .resource_mut::<ThemeResults>()
        .take(id)
        .unwrap()
        .unwrap();
    assert!(result.apply_error.is_none());
    assert_eq!(result.selection, "themes/custom-theme.json");
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(temp.path("config/themes/custom-theme.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["ui"]["window_background"], "#123456");
    assert!(saved.get("fontSize").is_none());
    assert_eq!(
        app.world()
            .resource::<SettingsSnapshot>()
            .get("bedtime.ui", "fontSize"),
        Some(&json!(20.0))
    );
    let active = app.world().resource::<ActiveTheme>();
    assert_eq!(active.file.name, "Custom Theme");
    assert_eq!(
        app.world()
            .resource::<bedtime_desktop::DesktopTheme>()
            .0
            .palette,
        file.runtime_palette()
    );
    assert_eq!(
        app.world().resource::<bedtime_core::SyntaxAppearance>().0,
        file.syntax
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_api::presentation::TerminalPalette>()
            .0,
        file.terminal
    );
}

#[test]
fn headless_theme_resources_register_message_maintenance() {
    let temp = TempDir::new();
    let mut app = app(&temp);
    app.world_mut()
        .write_message(SaveTheme::new(None, ThemeFile::bundled_default(), false));
    app.world_mut()
        .write_message(SettingsRequest::new("bed.application", "theme", "tokyo"));
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().resource::<Messages<SaveTheme>>().is_empty());
    assert!(
        app.world()
            .resource::<Messages<SettingsRequest>>()
            .is_empty()
    );
    assert!(!temp.path("config").exists());
}

#[test]
fn duplicate_names_are_unique_and_invalid_reload_retains_valid_colors() {
    let temp = TempDir::new();
    let mut app = app(&temp);
    let mut file = ThemeFile::bundled_default();
    file.name = "Same name".into();
    let first = SaveTheme::new(None, file.clone(), true);
    let second = SaveTheme::new(None, file, false);
    let ids = [first.id, second.id];
    app.world_mut().write_message(first);
    app.world_mut().write_message(second);
    finish_requests(&mut app);
    let first = app
        .world_mut()
        .resource_mut::<ThemeResults>()
        .take(ids[0])
        .unwrap()
        .unwrap();
    let second = app
        .world_mut()
        .resource_mut::<ThemeResults>()
        .take(ids[1])
        .unwrap()
        .unwrap();
    assert_eq!(first.selection, "themes/same-name.json");
    assert_eq!(second.selection, "themes/same-name-2.json");
    let before = app.world().resource::<ActiveTheme>().file.clone();
    std::fs::write(temp.path("config/themes/same-name.json"), b"{invalid").unwrap();
    configure_themes(app.world_mut());
    assert!(app.world().resource::<ActiveTheme>().error.is_some());
    assert_eq!(app.world().resource::<ActiveTheme>().file, before);
    std::fs::write(
        temp.path("config/themes/same-name.json"),
        serde_json::to_vec(&before.to_json()).unwrap(),
    )
    .unwrap();
    configure_themes(app.world_mut());
    assert!(app.world().resource::<ActiveTheme>().error.is_none());
}

#[test]
fn draft_receipt_keeps_newer_edits_and_reports_failed_apply_after_file_save() {
    let temp = TempDir::new();
    let mut app = app(&temp);
    let mut draft = ThemeDraft::default();
    let mut file = ThemeFile::bundled_default();
    file.name = "Receipt".into();
    draft.load(file, None);
    draft.request_save(true);
    for request in draft.take_requests() {
        app.world_mut().write_message(request);
    }
    draft.file.as_mut().unwrap().palette.background = [0.1, 0.2, 0.3, 1.0];
    std::fs::create_dir_all(temp.path("config")).unwrap();
    std::fs::write(temp.path("config/settings.json"), b"{invalid").unwrap();
    finish_requests(&mut app);
    draft.synchronize(&mut app.world_mut().resource_mut::<ThemeResults>());
    assert_eq!(draft.destination.as_deref(), Some("themes/receipt.json"));
    assert!(draft.error.as_ref().unwrap().contains("could not apply"));
    assert!(
        draft.dirty,
        "the receipt belongs to the submitted colors, not later edits"
    );
    assert_eq!(
        draft.file.as_ref().unwrap().palette.background,
        [0.1, 0.2, 0.3, 1.0]
    );
    assert!(temp.path("config/themes/receipt.json").exists());
}

#[test]
fn theme_destinations_reject_traversal_and_builtin_overwrites() {
    let temp = TempDir::new();
    let mut app = app(&temp);
    let files = app.world().resource::<ThemeFiles>();
    assert!(files.theme_path("themes/../outside.json").is_err());
    assert!(files.theme_path("/outside.json").is_err());
    for destination in ["tokyo", "themes/missing.json"] {
        let request = SaveTheme::new(
            Some(destination.into()),
            ThemeFile::bundled_default(),
            false,
        );
        let id = request.id;
        app.world_mut().write_message(request);
        finish_requests(&mut app);
        assert!(
            app.world_mut()
                .resource_mut::<ThemeResults>()
                .take(id)
                .unwrap()
                .is_err()
        );
    }
}
