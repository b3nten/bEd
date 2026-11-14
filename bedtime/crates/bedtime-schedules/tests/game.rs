//! Headless checks of the frame model against the example arena game.

#[path = "shared/game.rs"]
mod game;

use std::collections::HashSet;

use bedtime_schedules::{
    ScheduleExtractPlugin,
    frame::{FrameOptions, MarkKind, frame_sequence},
    snapshot_now,
};
use bevy::{input::InputPlugin, prelude::*, state::app::StatesPlugin};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin, StatesPlugin, TransformPlugin))
        .add_plugins(game::GamePlugin::default())
        .add_plugins(ScheduleExtractPlugin::default());
    app.finish();
    app.cleanup();
    app.update();
    app
}

#[test]
fn frame_sequence_follows_main_order_and_hands_off_data() {
    let mut app = app();
    let snapshot = snapshot_now(app.world_mut());
    let seq = frame_sequence(&snapshot, &FrameOptions::default());
    // Update comes before PostUpdate, and FixedUpdate is nested after RunFixedMainLoop.
    let band = |label: &str| seq.bands.iter().position(|b| b.label == label).unwrap();
    assert!(band("Update") < band("PostUpdate"));
    assert!(band("RunFixedMainLoop") < band("FixedUpdate") && band("FixedUpdate") < band("Update"));
    assert!(
        seq.bands.iter().any(|b| b.label.starts_with("OnEnter(")),
        "state transition schedules are nested"
    );

    let column = |name: &str| {
        seq.systems
            .iter()
            .find(|fs| {
                snapshot.schedules[fs.schedule].systems[fs.system as usize].short_name == name
            })
            .map(|fs| fs.column)
            .unwrap_or_else(|| panic!("{name} not in frame"))
    };
    let comp = |name: &str| {
        snapshot
            .components
            .iter()
            .position(|c| c.short_name == name)
            .unwrap() as u32
    };

    // DamageMessage: collide_projectiles writes (MessageWriter) -> apply_damage reads.
    let damage = comp("Messages<DamageMessage>");
    let handoff = seq
        .handoffs
        .iter()
        .find(|h| h.comp == damage && h.from == column("collide_projectiles"))
        .expect("damage message hand-off");
    assert!(handoff.to.contains(&column("apply_damage")));

    // Transform: written in FixedUpdate (integrate_velocity, then clamp_player_to_arena), read
    // later in Update by collide_projectiles. Each write feeds the reads up to the next write.
    let transform = comp("Transform");
    let integrate = column("integrate_velocity");
    let clamp = column("clamp_player_to_arena");
    assert!(
        seq.handoffs
            .iter()
            .any(|h| h.comp == transform && h.from == integrate && h.to.contains(&clamp))
    );
    assert!(seq.handoffs.iter().any(|h| h.comp == transform
        && h.from == clamp
        && h.to.contains(&column("collide_projectiles"))));

    // Access that only happens in a run condition is marked as such.
    assert!(
        seq.marks
            .iter()
            .any(|m| m.column == column("start_on_enter")
                && m.comp == comp("State<GameState>")
                && m.kind == MarkKind::ConditionRead)
    );
    // Filter-only access shows up as a filter mark.
    assert!(
        seq.marks
            .iter()
            .any(|m| m.column == column("read_player_input")
                && m.comp == comp("Player")
                && m.kind == MarkKind::Filter)
    );
    // Lanes are ordered by first access: Messages<DamageMessage> is written before it is read.
    assert!(seq.lanes.contains(&damage));
}

#[test]
fn lane_filtering_can_narrow_the_frame_to_the_systems_that_touch_it() {
    let mut app = app();
    let snapshot = snapshot_now(app.world_mut());
    let mut seq = frame_sequence(&snapshot, &FrameOptions::default());
    let comp = |name: &str| {
        snapshot
            .components
            .iter()
            .position(|c| c.short_name == name)
            .unwrap() as u32
    };
    let name_at = |seq: &bedtime_schedules::frame::FrameSequence, column: usize| {
        let fs = seq.systems[column];
        snapshot.schedules[fs.schedule].systems[fs.system as usize]
            .short_name
            .clone()
    };

    // Keep one lane, as a search for "Health" would.
    let health = comp("Health");
    seq.lanes.retain(|c| *c == health);
    let before = seq.systems.len();
    let touching: HashSet<String> = seq
        .marks
        .iter()
        .filter(|m| m.comp == health)
        .map(|m| name_at(&seq, m.column))
        .collect();
    seq.retain_columns_touching_lanes();

    assert!(seq.systems.len() < before, "columns were dropped");
    assert_eq!(seq.systems.len(), touching.len());
    let kept: HashSet<String> = (0..seq.systems.len()).map(|c| name_at(&seq, c)).collect();
    assert_eq!(kept, touching);
    assert!(kept.contains("apply_damage"), "a system that writes Health");
    assert!(!kept.contains("wave_timer"), "a system that does not");

    // Columns stay consistent: every mark, hand-off and band boundary is renumbered.
    assert!(seq.systems.iter().enumerate().all(|(i, fs)| fs.column == i));
    assert!(seq.marks.iter().all(|m| m.column < seq.systems.len()));
    assert!(
        seq.handoffs
            .iter()
            .all(|h| h.from < seq.systems.len() && h.to.iter().all(|c| *c < seq.systems.len()))
    );
    assert!(
        seq.bands
            .iter()
            .all(|b| b.first <= b.last && b.last <= seq.systems.len())
    );
    // Hidden lanes keep their marks on the surviving columns, so tooltips stay complete.
    assert!(seq.marks.iter().any(|m| m.comp != health));
}
