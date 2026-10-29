use super::*;
use crate::WorkbenchWorldExt;
use crate::{
    WorkbenchCorePlugin,
    persistence::{
        StoreAppExt, StoreBackend, WorkbenchStore,
        tests::{MemoryBackend, save_now},
    },
};

struct GlobalStore;
impl Store for GlobalStore {}

#[derive(Resource, Default)]
struct Applied(Vec<f32>);

crate::setting! {
    /// A test setting with an apply hook.
    Indent: f32 = 16.0, key = "test.indent",
    apply = |world, indent| world.resource_mut::<Applied>().0.push(*indent),
}

crate::setting! {
    RestoreLast: bool = true, key = "test.restore", only_in = [GlobalStore],
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Shade {
    Dark,
    Light,
}

crate::setting! {
    ShadeSetting: Shade = Shade::Dark, key = "test.shade",
}

fn global() -> StoreId {
    StoreId::of::<GlobalStore>()
}

fn workspace() -> StoreId {
    StoreId::of::<WorkbenchStore>()
}

/// Global settings under per-workbench ones, each in its own backend.
fn app(global: &MemoryBackend, workbench: &MemoryBackend) -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
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
    let mut app = app(&MemoryBackend::default(), &MemoryBackend::default());
    assert_eq!(applied(&app), [16.0], "apply runs once at startup");
    let world = app.world_mut();

    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 18.0)
        .unwrap();
    world
        .workbench_mut()
        .set_setting::<Indent>(workspace(), 12.0)
        .unwrap();
    // The workspace still overrides it, so nothing changes.
    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 20.0)
        .unwrap();
    let settings = world.resource::<Settings>();
    assert_eq!(settings.get::<Indent>(), 12.0);
    assert_eq!(settings.source::<Indent>(), Some(workspace()));
    assert_eq!(settings.get_in::<Indent>(global()), Some(20.0));

    world
        .workbench_mut()
        .clear_setting::<Indent>(workspace())
        .unwrap();
    assert_eq!(world.resource::<Settings>().get::<Indent>(), 20.0);
    assert_eq!(applied(&app), [16.0, 18.0, 12.0, 20.0]);
}

#[test]
fn each_layer_has_its_own_view_and_knows_what_overrides_it() {
    let mut app = app(&MemoryBackend::default(), &MemoryBackend::default());
    let world = app.world_mut();
    let settings = world.resource::<Settings>();
    assert_eq!(settings.get_at::<Indent>(global()), 16.0);
    assert_eq!(settings.overridden_above::<Indent>(global()), None);

    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 18.0)
        .unwrap();
    let settings = world.resource::<Settings>();
    // The workspace inherits the global value.
    assert_eq!(settings.get_at::<Indent>(workspace()), 18.0);

    world
        .workbench_mut()
        .set_setting::<Indent>(workspace(), 12.0)
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
    let mut app = app(&MemoryBackend::default(), &MemoryBackend::default());
    let world = app.world_mut();
    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 18.0)
        .unwrap();
    world
        .workbench_mut()
        .set_setting::<Indent>(workspace(), 12.0)
        .unwrap();
    // Set back to what the global layer gives: the workspace stops holding a
    // value, so it follows later global changes.
    world
        .workbench_mut()
        .set_setting::<Indent>(workspace(), 18.0)
        .unwrap();
    assert_eq!(
        world.resource::<Settings>().get_in::<Indent>(workspace()),
        None
    );
    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 20.0)
        .unwrap();
    assert_eq!(world.resource::<Settings>().get::<Indent>(), 20.0);
    // The global layer at the default holds nothing either.
    world
        .workbench_mut()
        .set_setting::<Indent>(global(), 16.0)
        .unwrap();
    assert_eq!(
        world.resource::<Settings>().get_in::<Indent>(global()),
        None
    );
}

#[test]
fn settings_can_be_limited_to_some_layers() {
    let mut app = app(&MemoryBackend::default(), &MemoryBackend::default());
    let world = app.world_mut();
    assert!(
        world
            .workbench_mut()
            .set_setting::<RestoreLast>(workspace(), false)
            .is_err()
    );
    world
        .workbench_mut()
        .set_setting::<RestoreLast>(global(), false)
        .unwrap();
    assert!(!world.resource::<Settings>().get::<RestoreLast>());
}

#[test]
fn layers_load_at_startup_save_changes_and_keep_unknown_keys() {
    let global_backend = MemoryBackend::default();
    global_backend
        .write(
            "settings",
            br#"{"test.indent": 18.0, "some.plugin": "kept"}"#,
        )
        .unwrap();
    let mut app = app(&global_backend, &MemoryBackend::default());
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
        .workbench_mut()
        .set_setting::<Indent>(global(), 19.0)
        .unwrap();
    save_now(&mut app);
    let saved = global_backend.text("settings").unwrap();
    assert!(saved.contains("19.0") && saved.contains("some.plugin"));
}

#[test]
fn invalid_values_fall_through_and_are_reported() {
    let global_backend = MemoryBackend::default();
    global_backend
        .write(
            "settings",
            br#"{"test.indent": "wide", "test.restore": false}"#,
        )
        .unwrap();
    let workbench_backend = MemoryBackend::default();
    // Not allowed in this layer, so it's ignored too.
    workbench_backend
        .write("settings", br#"{"test.restore": true}"#)
        .unwrap();
    let app = app(&global_backend, &workbench_backend);
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
    let global_backend = MemoryBackend::default();
    global_backend.write("settings", b"not settings").unwrap();
    let mut app = app(&global_backend, &MemoryBackend::default());
    assert!(app.world().resource::<Settings>().error(global()).is_some());
    app.world_mut()
        .workbench_mut()
        .set_setting::<Indent>(global(), 30.0)
        .unwrap();
    save_now(&mut app);
    assert_eq!(global_backend.text("settings").unwrap(), "not settings");
}

#[test]
fn a_layer_changed_outside_the_app_is_reloaded() {
    let global_backend = MemoryBackend::default();
    let mut app = app(&global_backend, &MemoryBackend::default());
    global_backend
        .write("settings", br#"{"test.indent": 24.0}"#)
        .unwrap();
    std::thread::sleep(CHECK_INTERVAL + Duration::from_millis(50));
    app.update();
    assert_eq!(app.world().resource::<Settings>().get::<Indent>(), 24.0);
    assert_eq!(applied(&app), [16.0, 24.0]);
}

#[test]
fn enum_values_round_trip_through_a_saved_layer() {
    let global_backend = MemoryBackend::default();
    let mut first = app(&global_backend, &MemoryBackend::default());
    first
        .world_mut()
        .workbench_mut()
        .set_setting::<ShadeSetting>(global(), Shade::Light)
        .unwrap();
    assert_eq!(
        first.world().resource::<Settings>().get::<ShadeSetting>(),
        Shade::Light
    );
    save_now(&mut first);
    let second = app(&global_backend, &MemoryBackend::default());
    assert_eq!(
        second.world().resource::<Settings>().get::<ShadeSetting>(),
        Shade::Light
    );
}

#[test]
fn a_setting_can_be_read_before_the_app_exists() {
    let backend = MemoryBackend::default();
    assert_eq!(read_setting::<RestoreLast>(&backend), None);
    backend
        .write(ENTRY, b"{\"test.restore\": false, \"test.shade\": Light}")
        .unwrap();
    assert_eq!(read_setting::<RestoreLast>(&backend), Some(false));
    assert_eq!(read_setting::<ShadeSetting>(&backend), Some(Shade::Light));
    assert_eq!(read_setting::<Indent>(&backend), None);
}
