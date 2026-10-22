use super::*;

#[test]
fn curvature_events_match_display_sampling_and_preserve_event_semantics() {
    let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1000.0, 600.0));
    let raw = egui::pos2(910.0, 140.0); // normalized (0.9, 0.2)
    // center=(0.4,-0.3), squared radius=.25, scale=1+.8*.25*.25=1.05.
    let expected = egui::pos2(930.0, 131.0);
    let modifiers = egui::Modifiers {
        shift: true,
        ..default()
    };
    let mut input = egui::RawInput {
        events: vec![
            egui::Event::PointerMoved(raw),
            egui::Event::PointerButton {
                pos: raw,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 20.0),
                phase: egui::TouchPhase::Move,
                modifiers,
            },
        ],
        ..default()
    };
    let mut pointer = PointerState::default();
    map_events(&mut input, rect, 0.8, &mut pointer);
    let egui::Event::PointerMoved(position) = input.events[0] else {
        panic!()
    };
    assert!(position.distance(expected) < 0.001);
    let egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: actual,
    } = input.events[1]
    else {
        panic!()
    };
    assert_eq!(pos, position);
    assert_eq!(button, egui::PointerButton::Primary);
    assert!(pressed && actual.shift);
    assert!(matches!(input.events[2], egui::Event::MouseWheel { delta, .. } if delta.y == 20.0));
    assert_eq!(source_position(rect.center(), rect, 0.8), rect.center());
    assert_eq!(source_position(raw, rect, 0.0), raw);
}

#[test]
fn stationary_pointer_reprojects_on_settings_and_resize_and_leaves_cleanly() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
    let raw = egui::pos2(900.0, 120.0);
    let mut pointer = PointerState::default();
    let mut input = egui::RawInput {
        events: vec![egui::Event::PointerMoved(raw)],
        ..default()
    };
    map_events(&mut input, rect, 1.0, &mut pointer);
    input.events.clear();
    map_events(&mut input, rect, 0.0, &mut pointer);
    assert!(matches!(input.events[0], egui::Event::PointerMoved(p) if p == raw));
    input.events.clear();
    let resized = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    map_events(&mut input, resized, 1.0, &mut pointer);
    assert!(
        matches!(input.events[0], egui::Event::PointerMoved(p) if p == source_position(raw, resized, 1.0))
    );
    input.events = vec![egui::Event::PointerGone];
    map_events(&mut input, resized, 1.0, &mut pointer);
    input.events.clear();
    map_events(&mut input, rect, 0.0, &mut pointer);
    assert!(input.events.is_empty());
}

#[test]
fn pointer_clamps_to_the_same_source_border_as_the_shader() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
    let corner = source_position(egui::Pos2::ZERO, rect, 1.0);
    assert!(corner.distance(egui::pos2(1.0, 0.6)) < 0.001);
    let opposite = source_position(rect.max, rect, 1.0);
    assert!(opposite.distance(egui::pos2(999.0, 599.4)) < 0.001);
}

#[test]
fn egui_clicks_the_widget_at_the_warped_visual_position() {
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
    // A small widget near the edge: uncorrected input misses it entirely.
    let displayed = egui::pos2(900.0, 120.0);
    let widget = egui::Rect::from_center_size(egui::pos2(925.0, 108.75), egui::vec2(8.0, 8.0));
    // For curvature=1, scale=1+.25*.25=1.0625; source=(925,108.75).
    assert!(!widget.contains(displayed));
    let mut pointer = PointerState::default();
    let mut clicked = false;
    for pressed in [None, Some(true), Some(false)] {
        let mut input = egui::RawInput {
            screen_rect: Some(rect),
            events: vec![egui::Event::PointerMoved(displayed)],
            ..default()
        };
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos: displayed,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: default(),
            });
        }
        map_events(&mut input, rect, 1.0, &mut pointer);
        let mut output = ctx.run_ui(input, |ui| {
            clicked |= ui
                .interact(widget, egui::Id::new("edge-button"), egui::Sense::click())
                .clicked();
        });
        output.textures_delta.clear();
    }
    assert!(clicked);
}
