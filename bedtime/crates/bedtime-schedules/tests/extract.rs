//! Headless tests of the extraction layer.

use bedtime_schedules::{
    AppSnapshot, ScheduleExtractPlugin,
    snapshot::{OrderKind, ScheduleSnapshot},
};
use bevy::prelude::*;

#[derive(Component)]
struct Foo;
#[derive(Component)]
struct Bar;
#[derive(Resource, Default)]
struct Baz(u32);
#[derive(Resource, Default)]
struct Counter(u32);
#[derive(Resource, Default)]
struct ChangeCount(u32);

fn writer(_q: Query<&mut Foo>) {}
fn reader(_q: Query<&Foo, With<Bar>>, _r: Res<Baz>) {}
fn spawner(mut commands: Commands) {
    commands.spawn((Foo, Bar));
}
fn consumer(_q: Query<&Foo>) {}
fn extra(_q: Query<&Bar>) {}
fn counter(world: &mut World, mut local: Local<u32>) {
    *local += 1;
    world.resource_mut::<Counter>().0 = *local;
}
fn observe_change(mut count: ResMut<ChangeCount>) {
    count.0 += 1;
}

fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ScheduleExtractPlugin::default())
        .init_resource::<Baz>()
        .init_resource::<Counter>()
        .init_resource::<ChangeCount>();
    app.add_systems(Update, (writer, reader).chain());
    app.add_systems(Update, counter);
    app.add_systems(Update, observe_change.run_if(resource_changed::<Baz>));
    app.add_systems(PostUpdate, (spawner, consumer).chain());
    app.finish();
    app.cleanup();
    app
}

fn snapshot(app: &App) -> &AppSnapshot {
    app.world().resource::<AppSnapshot>()
}

fn component(snapshot: &AppSnapshot, short_name: &str) -> u32 {
    snapshot
        .components
        .iter()
        .position(|c| c.short_name == short_name)
        .unwrap_or_else(|| panic!("component {short_name} not in snapshot")) as u32
}

fn system(schedule: &ScheduleSnapshot, name: &str) -> u32 {
    schedule
        .system_by_name(name)
        .unwrap_or_else(|| panic!("system {name} not in {}", schedule.label))
        .0
}

#[test]
fn captures_accesses_ordering_and_sync_points() {
    let mut app = make_app();
    app.update();
    let snap = snapshot(&app);
    assert_eq!(snap.generation, 1);

    let update = snap.schedule("Update").expect("Update captured");
    assert!(snap.schedule("PostUpdate").is_some());
    let main = snap
        .schedule("Main")
        .expect("Main captured through the pass");
    assert!(!main.conflicts_available);
    assert!(update.conflicts_available);
    assert!(
        main.systems
            .iter()
            .any(|s| s.short_name == "Main::run_main"),
        "Main systems: {:?}",
        main.systems
            .iter()
            .map(|s| (&s.name, &s.short_name))
            .collect::<Vec<_>>()
    );

    let foo = component(snap, "Foo");
    let bar = component(snap, "Bar");
    let baz = component(snap, "Baz");
    assert!(snap.components[baz as usize].is_resource);
    assert!(!snap.components[foo as usize].is_resource);

    let writer_ix = system(update, "writer");
    let reader_ix = system(update, "reader");
    let writer_s = &update.systems[writer_ix as usize];
    let reader_s = &update.systems[reader_ix as usize];
    assert!(writer_s.flags.access_available);
    assert_eq!(writer_s.queries.len(), 1);
    assert_eq!(writer_s.queries[0].writes, vec![foo]);
    assert!(writer_s.queries[0].reads.is_empty());
    assert!(writer_s.queries[0].filters.is_empty());

    assert_eq!(reader_s.queries.len(), 1);
    assert_eq!(reader_s.queries[0].reads, vec![foo]);
    assert_eq!(reader_s.queries[0].filters.len(), 1);
    assert_eq!(reader_s.queries[0].filters[0].with, vec![bar]);
    assert!(reader_s.queries[0].filters[0].without.is_empty());
    assert_eq!(reader_s.resources.len(), 1);
    assert_eq!(reader_s.resources[0].comp, baz);
    assert!(!reader_s.resources[0].write);

    assert!(
        update
            .flat_edges
            .iter()
            .any(|e| e.from == writer_ix && e.to == reader_ix && e.kind == OrderKind::Explicit),
        "writer -> reader edge missing: {:?}",
        update.flat_edges
    );
    let writer_pos = update.exec_order.iter().position(|s| *s == writer_ix);
    let reader_pos = update.exec_order.iter().position(|s| *s == reader_ix);
    assert!(
        writer_pos < reader_pos,
        "executor order must respect the chain"
    );

    let counter_s = &update.systems[system(update, "counter") as usize];
    assert!(counter_s.flags.exclusive);
    assert!(!counter_s.flags.access_available);

    let observe_s = &update.systems[system(update, "observe_change") as usize];
    assert_eq!(observe_s.conditions.len(), 1);
    assert_eq!(observe_s.conditions[0].resources[0].comp, baz);

    let post = snap.schedule("PostUpdate").unwrap();
    let spawner_ix = system(post, "spawner");
    let consumer_ix = system(post, "consumer");
    assert!(post.systems[spawner_ix as usize].flags.deferred);
    let sync = post
        .systems
        .iter()
        .position(|s| s.flags.apply_deferred)
        .expect("an ApplyDeferred sync point between spawner and consumer") as u32;
    assert!(
        post.flat_edges
            .iter()
            .any(|e| e.from == spawner_ix && e.to == sync && e.kind == OrderKind::SyncPoint)
    );
    assert!(
        post.flat_edges
            .iter()
            .any(|e| e.from == sync && e.to == consumer_ix && e.kind == OrderKind::SyncPoint)
    );
}

#[test]
fn runtime_rebuilds_are_recaptured() {
    let mut app = make_app();
    app.update();
    assert_eq!(snapshot(&app).generation, 1);
    app.update();
    assert_eq!(snapshot(&app).generation, 1, "no rebuild, no new snapshot");

    app.add_systems(Update, extra);
    app.update();
    let snap = snapshot(&app);
    assert_eq!(snap.generation, 2);
    let update = snap.schedule("Update").unwrap();
    let extra_s = &update.systems[system(update, "extra") as usize];
    assert_eq!(extra_s.queries[0].reads, vec![component(snap, "Bar")]);
    assert_eq!(update.build_count, 2);
}

#[test]
fn capture_does_not_disturb_locals_or_change_detection() {
    let mut app = make_app();
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(app.world().resource::<Counter>().0, 3);
    // `resource_changed` is true on the first run only.
    assert_eq!(app.world().resource::<ChangeCount>().0, 1);

    // Force a rebuild of Update (recaptures every system).
    app.add_systems(Update, extra);
    app.update();
    assert_eq!(
        app.world().resource::<Counter>().0,
        4,
        "Local of the exclusive system survived"
    );
    assert_eq!(
        app.world().resource::<ChangeCount>().0,
        1,
        "re-initialisation must not spuriously trigger change detection"
    );

    app.world_mut().resource_mut::<Baz>().0 += 1;
    app.update();
    assert_eq!(app.world().resource::<ChangeCount>().0, 2);
}
