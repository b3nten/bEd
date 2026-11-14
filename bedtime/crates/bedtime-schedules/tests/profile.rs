//! Headless check that Bevy's trace spans are turned into per-frame timings.
#![cfg(feature = "profile")]

#[path = "shared/game.rs"]
mod game;

use bedtime_schedules::profile::{TimingLayer, store};
use bevy::{input::InputPlugin, prelude::*, state::app::StatesPlugin};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[test]
fn records_system_and_schedule_intervals() {
    tracing_subscriber::registry()
        .with(TimingLayer::new())
        .init();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin, StatesPlugin, TransformPlugin))
        .add_plugins(game::GamePlugin::default());
    app.finish();
    app.cleanup();
    for _ in 0..3 {
        app.update();
    }
    let store = store();
    let store = store.lock().unwrap();
    let frame = store.last().expect("a frame was recorded");
    assert!(frame.duration > std::time::Duration::ZERO);
    assert!(
        frame
            .systems
            .iter()
            .any(|s| s.name.ends_with("world_stats")),
        "{:?}",
        frame.systems.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    assert!(
        frame.schedules.iter().any(|s| s.label == "Update"),
        "{:?}",
        frame.schedules
    );
    let update = frame
        .schedules
        .iter()
        .find(|s| s.label == "Update")
        .unwrap();
    let inside = frame
        .systems
        .iter()
        .find(|s| s.name.ends_with("world_stats"))
        .unwrap();
    assert!(inside.start >= update.start && inside.end <= update.end);
    assert!(store.average(&inside.name).is_some());
}
