//! Native-shaped interaction regression checks, active only in workbench smoke.
//! These exercise chrome hit testing, not just direct layout mutations.
use bedtime_ui2::{UiFrame, egui};
use bedtime_workbench2::*;
use bevy::{prelude::*, window::PrimaryWindow};

#[derive(Resource)]
struct Interaction {
    layout: Entity,
    original: Entity,
    area: Entity,
    tabs: [Entity; 3],
    start: Vec2,
    target: Vec2,
    before: Vec<(Entity, AreaRect)>,
    empty: Option<Entity>,
    right: Option<Entity>,
    received: u32,
    widget: Rect,
    count: usize,
}
fn areas(world: &World, layout: Entity) -> Vec<(Entity, AreaRect)> {
    world
        .get::<LayoutAreas>(layout)
        .unwrap()
        .entities()
        .iter()
        .map(|&e| (e, world.get::<Area>(e).unwrap().rect()))
        .collect()
}
fn panel_area(world: &World, panel: Entity) -> Entity {
    world.get::<InArea>(panel).unwrap().0
}
fn area_target(world: &mut World, entity: Entity) -> Rect {
    let bounds = world
        .query::<(&WorkbenchHost, &WorkbenchPresentation)>()
        .iter(world)
        .find_map(|(host, presentation)| host.panels_visible.then_some(presentation.panels))
        .unwrap();
    let r = world.get::<Area>(entity).unwrap().rect();
    Rect::from_corners(
        bounds.min
            + Vec2::new(r.min[0] as f32, r.min[1] as f32) * bounds.size() / LAYOUT_EXTENT as f32,
        bounds.min
            + Vec2::new(r.max[0] as f32, r.max[1] as f32) * bounds.size() / LAYOUT_EXTENT as f32,
    )
}
fn save_drag(world: &mut World, start: Vec2, target: Vec2) {
    let mut s = world.resource_mut::<Interaction>();
    s.start = start;
    s.target = target;
}
fn drag(world: &mut World, frame: u32, base: u32) {
    let age = frame.saturating_sub(base);
    if !(base..=base + 8).contains(&frame) {
        return;
    }
    let s = world.resource::<Interaction>();
    let p = if age < 4 { s.start } else { s.target };
    pointer(
        world,
        p,
        if age == 2 {
            Some((MouseButton::Left, true))
        } else if age == 6 {
            Some((MouseButton::Left, false))
        } else {
            None
        },
    );
}
#[derive(Resource, Default)]
pub(crate) struct ScriptedPointer {
    pending: Vec<(Vec2, Option<(MouseButton, bool)>)>,
    last: Option<Vec2>,
    cursor_emitted: bool,
}

pub(crate) fn pointer(world: &mut World, position: Vec2, button: Option<(MouseButton, bool)>) {
    world
        .resource_mut::<ScriptedPointer>()
        .pending
        .push((position, button));
}

// Inject after real OS events and before egui reads them. Observations and test
// assertions stay in Update, after presentation has been populated for the frame.
pub(crate) fn flush_pointer(world: &mut World) {
    world.resource_mut::<ScriptedPointer>().cursor_emitted = false;
    let events = std::mem::take(&mut world.resource_mut::<ScriptedPointer>().pending);
    for (position, button) in events {
        emit_pointer(world, position, button);
    }
}

// Upstream input also receives real OS cursor events while this native fixture
// runs. Admit only the last scripted move, and no moves on stationary frames,
// so live mouse movement cannot redirect a drag or reset a tooltip's delay.
pub(crate) fn isolate_pointer(
    scripted: Res<ScriptedPointer>,
    mut contexts: Query<&mut bevy_egui::EguiInput>,
) {
    if scripted.last.is_none() {
        return;
    }
    for mut input in &mut contexts {
        let keep = scripted
            .cursor_emitted
            .then(|| {
                input
                    .events
                    .iter()
                    .rposition(|event| matches!(event, egui::Event::PointerMoved(_)))
            })
            .flatten();
        let mut index = 0;
        input.events.retain(|event| {
            let current = index;
            index += 1;
            match event {
                egui::Event::PointerMoved(_) => keep == Some(current),
                egui::Event::PointerGone | egui::Event::MouseMoved(_) => false,
                _ => true,
            }
        });
    }
}
fn emit_pointer(world: &mut World, position: Vec2, button: Option<(MouseButton, bool)>) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    let moved = world.resource::<ScriptedPointer>().last != Some(position);
    world.resource_mut::<ScriptedPointer>().last = Some(position);
    // Real OS movement may arrive between scripted frames. Reassert the target
    // on button events while retaining stationary time for tooltip checks.
    if moved || button.is_some() {
        world.resource_mut::<ScriptedPointer>().cursor_emitted = true;
        let moved = bevy::window::CursorMoved {
            window,
            position,
            delta: None,
        };
        world.write_message(moved.clone());
        world.write_message(bevy::window::WindowEvent::CursorMoved(moved));
    }
    if let Some((button, pressed)) = button {
        let event = bevy::input::mouse::MouseButtonInput {
            window,
            button,
            state: if pressed {
                bevy::input::ButtonState::Pressed
            } else {
                bevy::input::ButtonState::Released
            },
        };
        world.write_message(event);
        world.write_message(bevy::window::WindowEvent::MouseButtonInput(event));
    }
}

pub(crate) fn step(world: &mut World, frame: u32) {
    if frame < 960 {
        return;
    }
    if frame == 960 {
        let mut w = Workbench::new(world);
        let original = w.active_layout();
        let workspace = w.active_workspace();
        let layout = w.create_layout(workspace, "Interaction check").unwrap();
        w.activate_layout(layout).unwrap();
        let area = w.destination_area(layout).unwrap();
        let kind = world
            .query::<(Entity, &PanelType)>()
            .iter(world)
            .find_map(|(e, p)| (p.key == "bed2.notes").then_some(e))
            .unwrap();
        let mut tabs = Vec::new();
        for i in 0..3 {
            let p = Workbench::new(world).open_panel(kind, area).unwrap();
            world.get_mut::<Panel>(p).unwrap().title = format!("Notes {}", i + 1);
            tabs.push(p);
        }
        world.insert_resource(Interaction {
            layout,
            original,
            area,
            tabs: tabs.try_into().unwrap(),
            start: Vec2::ZERO,
            target: Vec2::ZERO,
            before: vec![],
            empty: None,
            right: None,
            received: 0,
            widget: Rect::default(),
            count: 0,
        });
    }
    let (layout, area, tabs) = {
        let s = world.resource::<Interaction>();
        (s.layout, s.area, s.tabs)
    };
    // Reorder the last tab before the first, using the exact displayed tab bounds.
    if frame == 980 {
        let start = world
            .get::<PanelPresentation>(tabs[2])
            .unwrap()
            .tab_rect
            .center();
        let target = world
            .get::<PanelPresentation>(tabs[0])
            .unwrap()
            .tab_rect
            .min
            + Vec2::splat(3.0);
        save_drag(world, start, target);
    }
    drag(world, frame, 980);
    if frame == 1000 {
        assert_eq!(
            panels_in_area(world, area),
            &[tabs[2], tabs[0], tabs[1]],
            "Native insertion must reorder existing instances"
        );
        info!("Panel smoke: native tab ordering passed");
    }
    // Every edge supports splitting, then regrouping the same entity.
    for (base, index, high) in [
        (1020, 0, false),
        (1120, 0, true),
        (1220, 1, false),
        (1320, 1, true),
    ] {
        if frame == base {
            let start = world
                .get::<PanelPresentation>(tabs[2])
                .unwrap()
                .tab_rect
                .center();
            let r = world.get::<AreaPresentation>(area).unwrap().surface;
            let mut target = r.center();
            if index == 0 {
                target.x = if high { r.max.x - 24.0 } else { r.min.x + 24.0 };
            } else {
                target.y = if high { r.max.y - 24.0 } else { r.min.y + 44.0 };
            }
            save_drag(world, start, target);
        }
        drag(world, frame, base);
        if frame == base + 8 {
            let destination = panel_area(world, tabs[2]);
            assert_ne!(destination, area);
            let added_target = area_target(world, destination);
            let retained_target = area_target(world, area);
            let added = world.get::<AreaPresentation>(destination).unwrap();
            let retained = world.get::<AreaPresentation>(area).unwrap();
            assert_eq!(
                added.rect, added_target,
                "New split must appear at its final bounds"
            );
            assert_ne!(
                retained.rect, retained_target,
                "Existing panel must animate its resize"
            );
            for panel in [tabs[0], tabs[2]] {
                let tab = world.get::<PanelPresentation>(panel).unwrap().tab_rect;
                let bar = world
                    .get::<AreaPresentation>(panel_area(world, panel))
                    .unwrap()
                    .tab_bar;
                assert_eq!(
                    tab.min.y,
                    bar.min.y + 1.0,
                    "Tabs must follow their area without separate interpolation"
                );
            }
        }
        if frame == base + 20 {
            let destination = panel_area(world, tabs[2]);
            assert_ne!(destination, area, "Native edge drop must split");
            assert_eq!(areas(world, layout).len(), 2);
            let r = world.get::<Area>(destination).unwrap().rect();
            assert_eq!(
                if high { r.max[index] } else { r.min[index] },
                if high { LAYOUT_EXTENT } else { 0 }
            );
        }
        if frame == base + 40 {
            let start = world
                .get::<PanelPresentation>(tabs[2])
                .unwrap()
                .tab_rect
                .center();
            let target = world
                .get::<PanelPresentation>(tabs[0])
                .unwrap()
                .tab_rect
                .center();
            save_drag(world, start, target);
        }
        drag(world, frame, base + 40);
        if frame == base + 60 {
            assert_eq!(areas(world, layout).len(), 1);
            assert_eq!(panel_area(world, tabs[2]), area);
        }
    }
    if frame == 1400 {
        info!("Panel smoke: all four native edge drops and regrouping passed");
    }
    // Deliberately cover the corner with a viewport-like interaction and a
    // scrollbar. The shell must own the drag without either content handler acting.
    if frame == 1405 {
        let panel = register_panel_in_area(world, area, "Corner overlap", corner_overlap).unwrap();
        world.entity_mut(panel).insert(CornerOverlap::default());
        Workbench::new(world).focus_panel(panel).unwrap();
    }
    // A corner split previews without changing ECS until release.
    if frame == 1420 {
        let r = world.get::<AreaPresentation>(area).unwrap().surface;
        save_drag(
            world,
            r.max - Vec2::splat(6.0),
            Vec2::new(r.min.x + r.width() * 0.55, r.max.y - 3.0),
        );
    }
    if (1420..=1428).contains(&frame) {
        drag(world, frame, 1420);
    }
    if frame == 1426 {
        assert_eq!(areas(world, layout).len(), 1, "Split waits for release");
    }
    if frame == 1425 {
        let surface = world.get::<AreaPresentation>(area).unwrap().surface;
        let output = world
            .query::<&bevy_egui::EguiFullOutput>()
            .single(world)
            .unwrap();
        let (points, clip) = output
            .0
            .as_ref()
            .unwrap()
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.width == 2.0
                        && (points[1].y - points[0].y).abs() > surface.height() * 0.9 =>
                {
                    Some((*points, shape.clip_rect))
                }
                _ => None,
            })
            .expect("Corner split must draw a preview line");
        assert_eq!(points[0].y, surface.min.y + 1.0);
        assert_eq!(points[1].y, surface.max.y - 1.0);
        assert!(clip.min.y >= surface.min.y && clip.max.y <= surface.max.y);
    }
    if frame == 1428 {
        let halves = areas(world, layout);
        assert_eq!(halves.len(), 2);
        for (entity, _) in halves {
            let target = area_target(world, entity);
            assert_eq!(
                world.get::<AreaPresentation>(entity).unwrap().rect,
                target,
                "Both halves of a corner split must appear at their final bounds"
            );
        }
    }
    if frame == 1440 {
        let (panel, overlap) = world
            .query::<(Entity, &CornerOverlap)>()
            .single(world)
            .unwrap();
        assert!(
            !overlap.content_pressed,
            "Corner press must not reach panel content"
        );
        assert_eq!(
            overlap.scroll_offset, 0.0,
            "Corner drag must not move the scrollbar"
        );
        Workbench::new(world).close_panel(panel).unwrap();
        let other = areas(world, layout)
            .into_iter()
            .find(|(e, _)| *e != area)
            .unwrap()
            .0;
        assert!(panels_in_area(world, other).is_empty());
        world.resource_mut::<Interaction>().empty = Some(other);
        info!("Panel smoke: native corner split and content overlap priority passed");
    }
    let empty = world.resource::<Interaction>().empty;
    if frame == 1460 {
        let p = world.get::<AreaPresentation>(area).unwrap().rect;
        let before = areas(world, layout);
        world.resource_mut::<Interaction>().before = before;
        save_drag(
            world,
            Vec2::new(p.max.x, p.center().y),
            Vec2::new(p.max.x + 90.0, p.center().y),
        );
    }
    // Hold a border drag, cancel with right click, then release the left button.
    if (1460..=1468).contains(&frame) {
        let s = world.resource::<Interaction>();
        let p = if frame < 1464 { s.start } else { s.target };
        pointer(
            world,
            p,
            match frame {
                1462 => Some((MouseButton::Left, true)),
                1465 => Some((MouseButton::Right, true)),
                1466 => Some((MouseButton::Right, false)),
                1468 => Some((MouseButton::Left, false)),
                _ => None,
            },
        );
    }
    if frame == 1480 {
        assert_eq!(
            areas(world, layout),
            world.resource::<Interaction>().before,
            "Cancel must leave canonical geometry intact"
        );
    }
    if frame == 1500 {
        let p = world.get::<AreaPresentation>(area).unwrap().rect;
        save_drag(
            world,
            Vec2::new(p.max.x, p.center().y),
            Vec2::new(p.max.x - 80.0, p.center().y),
        );
    }
    drag(world, frame, 1500);
    if frame == 1520 {
        assert_ne!(
            areas(world, layout),
            world.resource::<Interaction>().before,
            "Native border drag must commit"
        );
    }
    // Build a T using the public API, then move its shared border through real input.
    if frame == 1540 {
        let lower = Workbench::new(world)
            .split_area(area, SplitAxis::Vertical, 5000, [100, 100])
            .unwrap();
        world.resource_mut::<Interaction>().empty = Some(lower);
        world.resource_mut::<Interaction>().right = empty;
    }
    if frame == 1560 {
        let p = world.get::<AreaPresentation>(area).unwrap().rect;
        save_drag(
            world,
            Vec2::new(p.max.x, p.center().y),
            Vec2::new(p.max.x + 70.0, p.center().y),
        );
    }
    drag(world, frame, 1560);
    if frame == 1580 {
        let lower = world.resource::<Interaction>().empty.unwrap();
        let right = world.resource::<Interaction>().right.unwrap();
        let x = world.get::<Area>(right).unwrap().rect().min[0];
        assert_eq!(world.get::<Area>(area).unwrap().rect().max[0], x);
        assert_eq!(world.get::<Area>(lower).unwrap().rect().max[0], x);
        info!("Panel smoke: cancellation and connected T-border resize passed");
    }
    // Extend the lower source across the T while preserving the upper target tail.
    if frame == 1600 {
        let lower = world.resource::<Interaction>().empty.unwrap();
        let s = world.get::<AreaPresentation>(lower).unwrap().surface;
        save_drag(
            world,
            Vec2::new(s.max.x - 3.0, s.min.y + 3.0),
            Vec2::new(s.max.x + 30.0, s.min.y + 40.0),
        );
    }
    drag(world, frame, 1600);
    if frame == 1620 {
        let lower = world.resource::<Interaction>().empty.unwrap();
        let right = world.resource::<Interaction>().right.unwrap();
        assert_eq!(
            world.get::<Area>(lower).unwrap().rect().max[0],
            LAYOUT_EXTENT
        );
        assert_eq!(world.get::<Area>(right).unwrap().rect().max[1], 5000);
        assert_eq!(areas(world, layout).len(), 3);
        info!("Panel smoke: native corner join across a T passed");
    }
    // Relocate the occupied source into the target's center; instances survive.
    if frame == 1640 {
        let s = world.get::<AreaPresentation>(area).unwrap().surface;
        let right = world.resource::<Interaction>().right.unwrap();
        let t = world.get::<AreaPresentation>(right).unwrap().surface;
        save_drag(world, Vec2::new(s.max.x - 3.0, s.min.y + 3.0), t.center());
    }
    drag(world, frame, 1640);
    if frame == 1660 {
        let right = world.resource::<Interaction>().right.unwrap();
        assert!(world.get::<Area>(right).is_none());
        for p in tabs {
            assert_eq!(panel_area(world, p), area);
        }
        info!("Panel smoke: native area relocation preserved panels");
    }
    // An outside drop must leave ordered membership intact.
    if frame == 1680 {
        let start = world
            .get::<PanelPresentation>(tabs[2])
            .unwrap()
            .tab_rect
            .center();
        save_drag(world, start, Vec2::new(-80.0, 300.0));
    }
    drag(world, frame, 1680);
    if frame == 1700 {
        assert_eq!(panel_area(world, tabs[2]), area);
    }
    // Re-entering the same layout during a drag must not admit its old target.
    if frame == 1720 {
        let s = world
            .get::<PanelPresentation>(tabs[2])
            .unwrap()
            .tab_rect
            .center();
        let r = world.get::<AreaPresentation>(area).unwrap().surface;
        save_drag(world, s, Vec2::new(r.max.x - 20.0, r.center().y));
        world.resource_mut::<Interaction>().before = areas(world, layout);
    }
    if (1720..=1728).contains(&frame) {
        drag(world, frame, 1720);
    }
    if frame == 1725 {
        let original = world.resource::<Interaction>().original;
        let mut w = Workbench::new(world);
        w.activate_layout(original).unwrap();
        w.activate_layout(layout).unwrap();
    }
    if frame == 1740 {
        assert_eq!(areas(world, layout), world.resource::<Interaction>().before);
    }
    // The drag source now lives in an ordinary panel, not persistent chrome.
    if frame == 1750 {
        let kind = world
            .query::<(Entity, &PanelType)>()
            .iter(world)
            .find_map(|(entity, kind)| (kind.key == "bed2.controls").then_some(entity))
            .unwrap();
        Workbench::new(world).open_panel(kind, area).unwrap();
    }
    // Application panel-type payloads open a panel in an empty area.
    if frame == 1760 {
        let empty = areas(world, layout)
            .into_iter()
            .find(|(e, _)| panels_in_area(world, *e).is_empty())
            .unwrap()
            .0;
        let source = world
            .resource::<crate::workbench_demo::DemoWorkbench>()
            .drag_source
            .center();
        let target = world
            .get::<AreaPresentation>(empty)
            .unwrap()
            .surface
            .center();
        let count = Workbench::new(world).panels(layout).len();
        let mut s = world.resource_mut::<Interaction>();
        s.empty = Some(empty);
        s.count = count;
        save_drag(world, source, target);
    }
    drag(world, frame, 1760);
    if frame == 1780 {
        assert_eq!(
            Workbench::new(world).panels(layout).len(),
            world.resource::<Interaction>().count + 1,
            "Panel-type payload must open through its factory"
        );
        info!("Panel smoke: external factory drop passed");
    }
    // A smaller content drop target consumes that same payload before the host.
    if frame == 1800 {
        let empty = world.resource::<Interaction>().empty.unwrap();
        let panel = register_panel_in_area(world, empty, "Drop receiver", receiver).unwrap();
        Workbench::new(world).focus_panel(panel).unwrap();
    }
    if frame == 1820 {
        let source = world
            .resource::<crate::workbench_demo::DemoWorkbench>()
            .drag_source
            .center();
        let target = world.resource::<Interaction>().widget.center();
        let count = Workbench::new(world).panels(layout).len();
        world.resource_mut::<Interaction>().count = count;
        save_drag(world, source, target);
    }
    drag(world, frame, 1820);
    if frame == 1840 {
        assert_eq!(
            world.resource::<Interaction>().received,
            1,
            "The content widget must receive the payload"
        );
        assert_eq!(
            Workbench::new(world).panels(layout).len(),
            world.resource::<Interaction>().count,
            "The workbench must not also open a panel"
        );
        info!("Panel smoke: content drop target priority passed");
    }
    // Close a single-tab area's actual close button; keep its empty footprint.
    if frame == 1860 {
        let empty = world.resource::<Interaction>().empty.unwrap();
        let p = panels_in_area(world, empty)
            .iter()
            .copied()
            .find(|e| world.get::<Panel>(*e).unwrap().title == "Notes")
            .unwrap();
        Workbench::new(world).close_panel(p).unwrap();
    }
    if frame == 1880 {
        let empty = world.resource::<Interaction>().empty.unwrap();
        let p = panels_in_area(world, empty)[0];
        let close = world
            .get::<PanelPresentation>(p)
            .unwrap()
            .close_rect
            .center();
        save_drag(world, close, close);
    }
    drag(world, frame, 1880);
    if frame == 1900 {
        let empty = world.resource::<Interaction>().empty.unwrap();
        assert!(world.get::<Area>(empty).is_some());
        assert!(panels_in_area(world, empty).is_empty());
        info!("Panel smoke: last-tab close retained its area");
    }
    if frame == 1920 {
        let original = world.resource::<Interaction>().original;
        Workbench::new(world).activate_layout(original).unwrap();
        info!("Panel interaction smoke passed");
    }
    // Present the same action model through actual egui menus. Smoke's shared
    // memory clipboard makes the command result observable without OS side effects.
    if frame == 1930 {
        let roots = world
            .query_filtered::<Entity, With<NativeMenuBar>>()
            .iter(world)
            .collect::<Vec<_>>();
        for root in roots {
            world.entity_mut(root).remove::<NativeMenuBar>();
        }
    }
    for base in [1940, 1980, 2080, 2120] {
        if frame == base {
            let menu = world
                .query::<(Entity, &Menu)>()
                .iter(world)
                .find_map(|(e, m)| (m.label == "bEd 2").then_some(e))
                .unwrap();
            let r = world.get::<MenuPresentation>(menu).unwrap();
            assert!(r.visible);
            save_drag(world, r.rect.center(), r.rect.center());
        }
        drag(world, frame, base);
    }
    if frame == 1960 {
        let p = menu_item(world, crate::CopyGreeting::LABEL);
        save_drag(world, p, p);
    }
    drag(world, frame, 1960);
    if frame == 1975 {
        assert_eq!(
            world
                .resource_mut::<bedtime_clipboard2::Clipboard>()
                .read_text()
                .unwrap(),
            "Hello from bEd 2"
        );
    }
    for base in [2000, 2140] {
        if frame == base {
            let tools = world
                .query::<(Entity, &Menu)>()
                .iter(world)
                .find_map(|(e, m)| (m.label == "Tools").then_some(e))
                .unwrap();
            let r = world.get::<MenuPresentation>(tools).unwrap();
            assert!(r.visible);
            save_drag(world, r.rect.center(), r.rect.center());
        }
        drag(world, frame, base);
    }
    for base in [2020, 2160] {
        if frame == base {
            let p = menu_item(world, crate::ToggleGreeting::LABEL);
            save_drag(world, p, p);
        }
        drag(world, frame, base);
    }
    if frame == 2040 {
        assert!(!command_available(
            world,
            ContextCommandId::of::<crate::CopyGreeting>(),
            InvocationContext::Focused
        ));
        world
            .resource_mut::<bedtime_clipboard2::Clipboard>()
            .write_text("unchanged while disabled")
            .unwrap();
    }
    if frame == 2100 {
        let p = menu_item(world, crate::CopyGreeting::LABEL);
        save_drag(world, p, p);
    }
    drag(world, frame, 2100);
    if frame == 2115 {
        assert_eq!(
            world
                .resource_mut::<bedtime_clipboard2::Clipboard>()
                .read_text()
                .unwrap(),
            "unchanged while disabled"
        );
        // A disabled item keeps its popup open. Close before reopening the root.
        pointer(
            world,
            Vec2::new(800.0, 300.0),
            Some((MouseButton::Left, true)),
        );
    }
    if frame == 2116 {
        pointer(
            world,
            Vec2::new(800.0, 300.0),
            Some((MouseButton::Left, false)),
        );
    }
    if frame == 2180 {
        assert!(command_available(
            world,
            ContextCommandId::of::<crate::CopyGreeting>(),
            InvocationContext::Focused
        ));
        info!(
            "Menu smoke: egui action invocation, shared clipboard and handler availability passed"
        );
    }
    // Open Notes through the View menu, then invoke its panel-only menu.
    for (base, label) in [(2190, "View"), (2230, "Notes")] {
        if frame == base {
            let rect = world
                .query::<(&Menu, &MenuPresentation)>()
                .iter(world)
                .find_map(|(menu, presentation)| {
                    (menu.label == label && presentation.visible).then_some(presentation.rect)
                })
                .expect("Visible top-level menu");
            save_drag(world, rect.center(), rect.center());
        }
        drag(world, frame, base);
    }
    for (base, label) in [(2210, "Open Notes"), (2250, "Copy Note Text")] {
        if frame == base {
            let point = menu_item(world, label);
            save_drag(world, point, point);
        }
        drag(world, frame, base);
    }
    if frame == 2225 {
        let active = Workbench::new(world).active_layout();
        let panel = Workbench::new(world).focused_panel(active).unwrap();
        world
            .get_mut::<crate::workbench_demo::Notes>(panel)
            .unwrap()
            .text = "Text from the focused Notes panel".into();
    }
    if frame == 2265 {
        assert_eq!(
            world.resource_mut::<Clipboard>().read_text().unwrap(),
            "Text from the focused Notes panel"
        );
        let active = Workbench::new(world).active_layout();
        let panel = world
            .query_filtered::<(Entity, &InArea), With<crate::workbench_demo::Controls>>()
            .iter(world)
            .find_map(|(entity, owner)| {
                (world.get::<InLayout>(owner.0).unwrap().0 == active).then_some(entity)
            })
            .unwrap();
        Workbench::new(world).focus_panel(panel).unwrap();
    }
    if frame == 2280 {
        assert!(
            world
                .query::<(&Menu, &MenuPresentation)>()
                .iter(world)
                .filter(|(menu, _)| menu.label == "Notes")
                .all(|(_, presentation)| !presentation.visible)
        );
        info!(
            "Menu smoke: View action, focused Notes menu, panel handler and focus-driven hiding passed"
        );
    }
    if frame == 2290 {
        let scene = world
            .query_filtered::<Entity, With<crate::viewport_demo::DemoState>>()
            .single(world)
            .unwrap();
        Workbench::new(world).focus_panel(scene).unwrap();
    }
    if frame == 2330
        && world
            .get_resource::<NativeCapabilities>()
            .is_some_and(|c| c.native_menus)
    {
        let root = world
            .query_filtered::<Entity, With<MenuBar>>()
            .iter(world)
            .next()
            .unwrap();
        world.entity_mut(root).insert(NativeMenuBar);
    }
    if frame == 2340
        && world
            .get_resource::<NativeCapabilities>()
            .is_some_and(|c| c.native_menus)
    {
        assert!(
            world
                .query::<&MenuPresentation>()
                .iter(world)
                .all(|m| !m.visible)
        );
        info!("Menu presentation toggle passed: native, egui, native");
    }
}

fn menu_item(world: &mut World, label: &str) -> Vec2 {
    let entity = world
        .query::<(Entity, &MenuItem)>()
        .iter(world)
        .find_map(|(e, item)| {
            (item.label == label && world.get::<MenuPresentation>(e).is_some_and(|p| p.visible))
                .then_some(e)
        })
        .unwrap();
    let p = world
        .get::<MenuPresentation>(entity)
        .expect("Menu item drawn");
    assert!(p.visible, "Menu item {label} is visible");
    p.rect.center()
}

fn receiver(frame: UiFrame, mut state: ResMut<Interaction>) {
    let response = frame
        .ui
        .add_sized([220.0, 100.0], egui::Button::new("Drop a Notes panel here"));
    state.widget = Rect::from_corners(
        Vec2::new(response.rect.min.x, response.rect.min.y),
        Vec2::new(response.rect.max.x, response.rect.max.y),
    );
    if response.dnd_release_payload::<PanelTypePayload>().is_some() {
        state.received += 1;
    }
}

#[derive(Component, Default)]
struct CornerOverlap {
    content_pressed: bool,
    scroll_offset: f32,
}

fn corner_overlap(frame: UiFrame, mut panels: Query<&mut CornerOverlap>) {
    let mut state = panels.get_mut(frame.view).unwrap();
    let response = frame.ui.interact(
        frame.ui.max_rect(),
        frame.ui.id().with("viewport-like-content"),
        egui::Sense::click_and_drag(),
    );
    state.content_pressed |= response.is_pointer_button_down_on() || response.clicked();
    frame.ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    let output = egui::ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(frame.ui, |ui| {
            for row in 0..200 {
                ui.label(format!("Scrollable row {row}"));
            }
        });
    state.scroll_offset = output.state.offset.y;
}
