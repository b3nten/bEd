use crate::*;
use bedtime_persistence::{
    MemoryStore, PersistenceAppExt, PersistencePlugin, Store, StoreBackend, StoreId,
};
use bevy::prelude::*;
use std::time::Duration;

struct GlobalStore;
impl Store for GlobalStore {}
struct WorkbenchStore;
impl Store for WorkbenchStore {}

#[derive(Resource, Default)]
struct Applied(Vec<f32>);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Shade {
    Dark,
    Light,
}

crate::settings! {
    /// A test setting with an apply hook.
    Indent: f32 {
        default: 16.0,
        key: "test.indent",
        apply: |world, indent| world.resource_mut::<Applied>().0.push(*indent),
    }
    RestoreLast: bool { default: true, key: "test.restore", only_in: [GlobalStore] }
    ShadeSetting: Shade { default: Shade::Dark, key: "test.shade" }
}

/// Saving runs about once a second, and always when the app exits.
fn save_now(app: &mut App) {
    app.world_mut().write_message(AppExit::Success);
    app.update();
    app.world_mut().resource_mut::<Messages<AppExit>>().clear();
}

fn global() -> StoreId {
    StoreId::of::<GlobalStore>()
}

fn workspace() -> StoreId {
    StoreId::of::<WorkbenchStore>()
}

/// Global settings under per-workbench ones, each in its own backend.
fn app(global: &MemoryStore, workbench: &MemoryStore) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, PersistencePlugin, SettingsPlugin))
        .init_resource::<Applied>()
        .add_store::<GlobalStore>(global.clone())
        .add_store::<WorkbenchStore>(workbench.clone())
        .add_settings_layer::<GlobalStore>()
        .add_settings_layer::<WorkbenchStore>()
        .register_setting::<Indent>()
        .register_setting::<RestoreLast>()
        .register_setting::<ShadeSetting>();
    app.update();
    app
}

fn applied(app: &App) -> Vec<f32> {
    app.world().resource::<Applied>().0.clone()
}

#[test]
fn later_layers_override_earlier_ones_and_clearing_inherits() {
    let mut app = app(&MemoryStore::default(), &MemoryStore::default());
    assert_eq!(applied(&app), [16.0], "apply runs once at startup");
    let world = app.world_mut();

    world.settings_mut().set::<Indent>(global(), 18.0).unwrap();
    world
        .settings_mut()
        .set::<Indent>(workspace(), 12.0)
        .unwrap();
    // The workspace still overrides it, so nothing changes.
    world.settings_mut().set::<Indent>(global(), 20.0).unwrap();
    let settings = world.resource::<Settings>();
    assert_eq!(settings.get::<Indent>(), 12.0);
    assert_eq!(settings.source::<Indent>(), Some(workspace()));
    assert_eq!(settings.get_in::<Indent>(global()), Some(20.0));

    world.settings_mut().clear::<Indent>(workspace()).unwrap();
    assert_eq!(world.resource::<Settings>().get::<Indent>(), 20.0);
    assert_eq!(applied(&app), [16.0, 18.0, 12.0, 20.0]);
}

#[test]
fn each_layer_has_its_own_view_and_knows_what_overrides_it() {
    let mut app = app(&MemoryStore::default(), &MemoryStore::default());
    let world = app.world_mut();
    let settings = world.resource::<Settings>();
    assert_eq!(settings.get_at::<Indent>(global()), 16.0);
    assert_eq!(settings.overridden_above::<Indent>(global()), None);

    world.settings_mut().set::<Indent>(global(), 18.0).unwrap();
    let settings = world.resource::<Settings>();
    // The workspace inherits the global value.
    assert_eq!(settings.get_at::<Indent>(workspace()), 18.0);

    world
        .settings_mut()
        .set::<Indent>(workspace(), 12.0)
        .unwrap();
    let settings = world.resource::<Settings>();
    assert_eq!(settings.get::<Indent>(), 12.0);
    // Editing the global layer starts from its own value, not the workspace's.
    assert_eq!(settings.get_at::<Indent>(global()), 18.0);
    assert_eq!(
        settings.overridden_above::<Indent>(global()),
        Some(workspace())
    );
    assert_eq!(settings.overridden_above::<Indent>(workspace()), None);
}

#[test]
fn setting_the_inherited_value_clears_the_layer() {
    let mut app = app(&MemoryStore::default(), &MemoryStore::default());
    let world = app.world_mut();
    world.settings_mut().set::<Indent>(global(), 18.0).unwrap();
    world
        .settings_mut()
        .set::<Indent>(workspace(), 12.0)
        .unwrap();
    // Set back to what the global layer gives: the workspace stops holding a
    // value, so it follows later global changes.
    world
        .settings_mut()
        .set::<Indent>(workspace(), 18.0)
        .unwrap();
    assert_eq!(
        world.resource::<Settings>().get_in::<Indent>(workspace()),
        None
    );
    world.settings_mut().set::<Indent>(global(), 20.0).unwrap();
    assert_eq!(world.resource::<Settings>().get::<Indent>(), 20.0);
    // The global layer at the default holds nothing either.
    world.settings_mut().set::<Indent>(global(), 16.0).unwrap();
    assert_eq!(
        world.resource::<Settings>().get_in::<Indent>(global()),
        None
    );
}

#[test]
fn settings_can_be_limited_to_some_layers() {
    let mut app = app(&MemoryStore::default(), &MemoryStore::default());
    let world = app.world_mut();
    assert!(
        world
            .settings_mut()
            .set::<RestoreLast>(workspace(), false)
            .is_err()
    );
    world
        .settings_mut()
        .set::<RestoreLast>(global(), false)
        .unwrap();
    assert!(!world.resource::<Settings>().get::<RestoreLast>());
}

#[test]
fn layers_load_at_startup_save_changes_and_keep_unknown_keys() {
    let global_backend = MemoryStore::default();
    global_backend
        .write(
            "settings",
            br#"{"test.indent": 18.0, "some.plugin": "kept"}"#,
        )
        .unwrap();
    let mut app = app(&global_backend, &MemoryStore::default());
    assert_eq!(app.world().resource::<Settings>().get::<Indent>(), 18.0);
    assert_eq!(applied(&app), [18.0]);

    let writes = global_backend.writes();
    save_now(&mut app);
    assert_eq!(
        global_backend.writes(),
        writes,
        "unchanged layers aren't rewritten"
    );

    app.world_mut()
        .settings_mut()
        .set::<Indent>(global(), 19.0)
        .unwrap();
    save_now(&mut app);
    let saved = global_backend.text("settings").unwrap();
    assert!(saved.contains("19.0") && saved.contains("some.plugin"));
}

#[test]
fn invalid_values_fall_through_and_are_reported() {
    let global_backend = MemoryStore::default();
    global_backend
        .write(
            "settings",
            br#"{"test.indent": "wide", "test.restore": false}"#,
        )
        .unwrap();
    let workbench_backend = MemoryStore::default();
    // Not allowed in this layer, so it's ignored too.
    workbench_backend
        .write("settings", br#"{"test.restore": true}"#)
        .unwrap();
    let mut app = app(&global_backend, &workbench_backend);
    let reported: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SettingsError>>()
        .drain()
        .collect();
    assert_eq!(reported.len(), 1);
    assert!(reported[0].0.contains("test.indent in"));
    let settings = app.world().resource::<Settings>();
    assert_eq!(settings.get::<Indent>(), 16.0);
    assert!(!settings.get::<RestoreLast>());
    assert!(settings.error(global()).unwrap().contains("test.indent"));
    assert!(
        settings
            .error(workspace())
            .unwrap()
            .contains("test.restore")
    );
}

#[test]
fn an_unreadable_layer_is_never_saved_over() {
    let global_backend = MemoryStore::default();
    global_backend.write("settings", b"not settings").unwrap();
    let mut app = app(&global_backend, &MemoryStore::default());
    assert!(app.world().resource::<Settings>().error(global()).is_some());
    let reported: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<bedtime_persistence::StoreError>>()
        .drain()
        .collect();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].entry, SETTINGS_ENTRY);
    app.world_mut()
        .settings_mut()
        .set::<Indent>(global(), 30.0)
        .unwrap();
    save_now(&mut app);
    assert_eq!(global_backend.text("settings").unwrap(), "not settings");
}

#[test]
fn a_layer_changed_outside_the_app_is_reloaded() {
    let global_backend = MemoryStore::default();
    let mut app = app(&global_backend, &MemoryStore::default());
    global_backend
        .write("settings", br#"{"test.indent": 24.0}"#)
        .unwrap();
    std::thread::sleep(Duration::from_secs(1) + Duration::from_millis(50));
    app.update();
    assert_eq!(app.world().resource::<Settings>().get::<Indent>(), 24.0);
    assert_eq!(applied(&app), [16.0, 24.0]);
}

#[test]
fn enum_values_round_trip_through_a_saved_layer() {
    let global_backend = MemoryStore::default();
    let mut first = app(&global_backend, &MemoryStore::default());
    first
        .world_mut()
        .settings_mut()
        .set::<ShadeSetting>(global(), Shade::Light)
        .unwrap();
    assert_eq!(
        first.world().resource::<Settings>().get::<ShadeSetting>(),
        Shade::Light
    );
    save_now(&mut first);
    let second = app(&global_backend, &MemoryStore::default());
    assert_eq!(
        second.world().resource::<Settings>().get::<ShadeSetting>(),
        Shade::Light
    );
}

#[test]
fn a_setting_can_be_read_before_the_app_exists() {
    let backend = MemoryStore::default();
    assert_eq!(read_setting::<RestoreLast>(&backend), None);
    backend
        .write(
            SETTINGS_ENTRY,
            b"{\"test.restore\": false, \"test.shade\": Light}",
        )
        .unwrap();
    assert_eq!(read_setting::<RestoreLast>(&backend), Some(false));
    assert_eq!(read_setting::<ShadeSetting>(&backend), Some(Shade::Light));
    assert_eq!(read_setting::<Indent>(&backend), None);
}

#[cfg(feature = "ui")]
mod ui {
    use super::*;
    use crate::ui::{SettingUi, SettingsUi, SettingsUiAppExt, words};

    #[derive(Clone, Copy, Debug, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
    enum Tone {
        Warm,
        Cool,
    }

    fn effects_on(world: &World, layer: StoreId) -> bool {
        world.settings().get_at::<Effects>(layer)
    }

    crate::settings! {
        "Appearance" {
            FontSize: f32 {
                default: 15.5,
                key: "ui.font_size",
                ui: slider(8.0..=72.0).suffix(" pt").decimals(1),
            }
            Tint: Tone { default: Tone::Warm, key: "ui.tone", label: "Tone", description: "How warm" }
        }
        "Startup" {
            Reopen: bool { default: false, key: "ui.reopen", only_in: [GlobalStore] }
        }
        "Effects" {
            Effects: bool { default: false, key: "ui.effects" }
            Strength: f64 { default: 0.5, key: "ui.strength", visible: effects_on }
        }
        Hidden: Option<String> { default: None, key: "ui.hidden" }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PersistencePlugin, SettingsPlugin))
            .add_settings_layer::<GlobalStore>()
            .add_settings_layer::<WorkbenchStore>()
            .label_settings_layer::<WorkbenchStore>("Workspace")
            .register_setting::<FontSize>()
            .register_setting::<Tint>()
            .register_setting::<Reopen>()
            .register_setting::<Effects>()
            .register_setting::<Strength>()
            .register_setting::<Hidden>()
            .settings_sections(["Effects"]);
        app.update();
        app
    }

    #[test]
    fn editors_come_from_the_entry_or_the_value_type() {
        assert!(matches!(FontSize::ui(), Some(SettingUi::Editor(_))));
        assert!(matches!(Tint::inferred_ui(), Some(SettingUi::Editor(_))));
        assert!(matches!(Reopen::inferred_ui(), Some(SettingUi::Editor(_))));
        assert!(matches!(
            Strength::inferred_ui(),
            Some(SettingUi::Editor(_))
        ));
        assert!(Hidden::inferred_ui().is_none(), "not in a section");
        assert_eq!(FontSize::label(), "Font size");
        assert_eq!(Tint::label(), "Tone");
        assert_eq!(Tint::description(), Some("How warm"));
        assert_eq!(words("ReopenLastWorkspace"), "Reopen last workspace");
    }

    #[test]
    fn sections_list_shown_rows_in_order() {
        let mut app = app();
        let shown = |app: &App, layer| -> Vec<(&'static str, Vec<String>)> {
            let world = app.world();
            world
                .resource::<SettingsUi>()
                .sections()
                .into_iter()
                .map(|(section, rows)| {
                    let keys = rows
                        .iter()
                        .filter(|row| row.shown(world, layer))
                        .map(|row| row.key.clone())
                        .collect();
                    (section, keys)
                })
                .collect()
        };
        assert_eq!(
            shown(&app, global()),
            [
                ("Effects", vec!["ui.effects".to_owned()]),
                ("Appearance", vec!["ui.font_size".into(), "ui.tone".into()]),
                ("Startup", vec!["ui.reopen".into()]),
            ]
        );
        // Only allowed layers show a setting; visibility can follow another.
        app.world_mut()
            .settings_mut()
            .set::<Effects>(workspace(), true)
            .unwrap();
        assert_eq!(
            shown(&app, workspace()),
            [
                (
                    "Effects",
                    vec!["ui.effects".to_owned(), "ui.strength".into()]
                ),
                ("Appearance", vec!["ui.font_size".into(), "ui.tone".into()]),
                ("Startup", vec![]),
            ]
        );
        let settings = app.world().settings();
        assert_eq!(settings.label(global()), Some("Global"));
        assert_eq!(settings.label(workspace()), Some("Workspace"));
        assert_eq!(settings.below(workspace()), Some(global()));
    }
}
