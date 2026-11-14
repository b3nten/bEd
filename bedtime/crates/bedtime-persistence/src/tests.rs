use crate::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

struct MainStore;
impl Store for MainStore {}
struct GlobalStore;
impl Store for GlobalStore {}

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Persist)]
struct Note(String);

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Persist)]
struct Pinned(#[entities] Entity);

#[derive(Resource, Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Resource, Persist)]
struct Counter(u32);

#[derive(Resource, Serialize, Deserialize, Persisted, Default, Clone, Debug, PartialEq)]
#[persisted(key = "recent")]
#[serde(transparent)]
struct Recent(Vec<String>);

fn app(main: &MemoryStore, global: &MemoryStore) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, PersistencePlugin::default()))
        .register_type::<Note>()
        .register_type::<Pinned>()
        .register_type::<Counter>()
        .init_resource::<Counter>()
        .init_resource::<Recent>()
        .add_store::<MainStore>(main.clone())
        .add_store::<GlobalStore>(global.clone())
        .persist_world::<MainStore>()
        .persist::<Recent, GlobalStore>();
    app
}

/// Saving runs about once a second, and always when the app exits.
fn save_now(app: &mut App) {
    app.world_mut().write_message(AppExit::Success);
    app.update();
    app.world_mut().resource_mut::<Messages<AppExit>>().clear();
}

fn errors(app: &mut App) -> Vec<StoreError> {
    app.world_mut()
        .resource_mut::<Messages<StoreError>>()
        .drain()
        .collect()
}

#[test]
fn the_world_and_persisted_resources_round_trip_through_their_stores() {
    let (main, global) = (MemoryStore::default(), MemoryStore::default());
    let mut first = app(&main, &global);
    first.update();
    let note = first.world_mut().spawn(Note("kept".into())).id();
    first.world_mut().spawn(Pinned(note));
    first.world_mut().spawn(Transform::default()); // not persisted
    first.world_mut().resource_mut::<Counter>().0 = 3;
    first.world_mut().resource_mut::<Recent>().0 = vec!["/a".into()];
    save_now(&mut first);
    assert!(main.text("world").unwrap().contains("kept"));
    assert_eq!(global.text("recent").unwrap(), "[\n    \"/a\",\n]\n");
    assert!(
        global.text("world").is_none(),
        "only one store holds the world"
    );
    let writes = (main.writes(), global.writes());
    save_now(&mut first);
    assert_eq!(
        (main.writes(), global.writes()),
        writes,
        "unchanged data isn't rewritten"
    );

    let mut second = app(&main, &global);
    second.update();
    let world = second.world_mut();
    let (note, Note(text)) = world
        .query::<(Entity, &Note)>()
        .single(world)
        .map(|(e, n)| (e, n.clone()))
        .unwrap();
    assert_eq!(text, "kept");
    let pinned = world.query::<&Pinned>().single(world).unwrap();
    assert_eq!(pinned.0, note, "references are mapped to the new entities");
    assert_eq!(world.resource::<Counter>().0, 3);
    assert_eq!(world.resource::<Recent>().0, ["/a"]);
}

#[test]
fn saved_data_that_cant_be_read_is_reported_and_never_overwritten() {
    let (main, global) = (MemoryStore::default(), MemoryStore::default());
    main.write("world", b"not a world").unwrap();
    global.write("recent", b"(not a list").unwrap();
    let mut app = app(&main, &global);
    app.update();
    let errors = errors(&mut app);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().all(|e| e.kind == StoreErrorKind::Load));
    assert!(
        app.world()
            .resource::<Stores>()
            .error::<MainStore>()
            .is_some()
    );
    app.world_mut().spawn(Note("new".into()));
    app.world_mut().resource_mut::<Recent>().0 = vec!["/b".into()];
    save_now(&mut app);
    assert_eq!(main.text("world").unwrap(), "not a world");
    assert_eq!(global.text("recent").unwrap(), "(not a list");
}

#[test]
fn a_failing_save_is_reported_once_until_it_changes() {
    struct Failing;
    impl StoreBackend for Failing {
        fn read(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
        fn write(&self, _: &str, _: &[u8]) -> Result<(), String> {
            Err("disk full".into())
        }
    }
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, PersistencePlugin::default()))
        .init_resource::<Recent>()
        .add_store::<GlobalStore>(Failing)
        .persist::<Recent, GlobalStore>();
    app.update();
    save_now(&mut app);
    save_now(&mut app);
    let errors = errors(&mut app);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, StoreErrorKind::Save);
    assert_eq!(
        errors[0].to_string(),
        "Couldn't save recent; retrying (disk full)"
    );
}

#[test]
fn hooks_edit_captures_refuse_snapshots_and_undo_failed_restores() {
    let (main, global) = (MemoryStore::default(), MemoryStore::default());
    let mut source = app(&main, &global);
    source.on_capture(|_, dynamic, _| {
        dynamic
            .entities
            .retain(|e| snapshot::value::<Note>(e).is_none_or(|n| n.0 != "skip"))
    });
    source.world_mut().spawn(Note("skip".into()));
    source.world_mut().spawn(Note("keep".into()));
    let saved = source.world().persistence().capture().unwrap();
    assert!(saved.contains("keep") && !saved.contains("skip"));

    let mut refusing = app(&main, &global);
    refusing.check_restore(|_, _| Err("not now".into()));
    assert_eq!(
        refusing.world_mut().persistence_mut().restore(&saved),
        Err("not now".into())
    );

    let mut failing = app(&main, &global);
    failing.on_restore(|_, _, _| Err("invalid".into()));
    let world = failing.world_mut();
    assert!(world.persistence_mut().restore(&saved).is_err());
    assert_eq!(world.query::<&Note>().iter(world).count(), 0);
}

#[test]
fn references_to_entities_that_arent_saved_are_refused() {
    let (main, global) = (MemoryStore::default(), MemoryStore::default());
    let mut app = app(&main, &global);
    let unsaved = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().spawn(Pinned(unsaved));
    assert!(app.world().persistence().capture().is_err());
}

#[test]
fn persisted_resources_can_be_read_before_there_is_an_app() {
    let store = MemoryStore::default();
    assert_eq!(read_persisted::<Recent>(&store), Ok(None));
    write_persisted(&store, &Recent(vec!["/a".into()])).unwrap();
    assert_eq!(
        read_persisted::<Recent>(&store),
        Ok(Some(Recent(vec!["/a".into()])))
    );
}

#[test]
fn directory_stores_keep_one_file_per_entry() {
    let directory = std::env::temp_dir().join(format!("bedtime-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let store = DirectoryStore::new(directory.join("nested"));
    assert_eq!(store.read("world").unwrap(), None);
    assert_eq!(store.version("world").unwrap(), None);
    store.write("world", b"first").unwrap();
    assert_eq!(store.read("world").unwrap().as_deref(), Some(&b"first"[..]));
    assert!(directory.join("nested/world.ron").exists());
    assert!(store.version("world").unwrap().is_some());
    store.write("world", b"second").unwrap();
    assert_eq!(
        store.read("world").unwrap().as_deref(),
        Some(&b"second"[..])
    );
    // Nothing is left behind from the atomic write.
    let files = std::fs::read_dir(directory.join("nested")).unwrap().count();
    assert_eq!(files, 1);
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn the_config_directory_prefers_the_override() {
    // SAFETY: tests in this crate don't read this variable concurrently.
    unsafe { std::env::set_var("BEDTIME_TEST_CONFIG_DIR", "/tmp/override") };
    assert_eq!(
        config_directory("app", Some("BEDTIME_TEST_CONFIG_DIR")),
        Some("/tmp/override".into())
    );
    unsafe { std::env::remove_var("BEDTIME_TEST_CONFIG_DIR") };
}
