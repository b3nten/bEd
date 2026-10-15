//! Native-shaped interaction regression checks, active only in workbench smoke.
//! These exercise chrome hit testing, not just direct layout mutations.
use bedtime_ui2::{UiFrame, imgui};
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
            Some((imgui::MouseButton::Left, true))
        } else if age == 6 {
            Some((imgui::MouseButton::Left, false))
        } else {
            None
        },
    );
}
fn pointer(world: &mut World, position: Vec2, button: Option<(imgui::MouseButton, bool)>) {
    use bevy::winit::{RawWinitWindowEvent, WINIT_WINDOWS};
    let surface = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let window = windows.get_window(surface).unwrap();
        world.write_message(RawWinitWindowEvent {
            window_id: window.id(),
            event: winit::event::WindowEvent::CursorMoved {
                device_id: winit::event::DeviceId::dummy(),
                position: winit::dpi::PhysicalPosition::new(
                    position.x as f64 * window.scale_factor(),
                    position.y as f64 * window.scale_factor(),
                ),
            },
        });
        if let Some((button, pressed)) = button {
            world.write_message(RawWinitWindowEvent {
                window_id: window.id(),
                event: winit::event::WindowEvent::MouseInput {
                    device_id: winit::event::DeviceId::dummy(),
                    button: match button {
                        imgui::MouseButton::Left => winit::event::MouseButton::Left,
                        imgui::MouseButton::Right => winit::event::MouseButton::Right,
                        _ => winit::event::MouseButton::Middle,
                    },
                    state: if pressed {
                        winit::event::ElementState::Pressed
                    } else {
                        winit::event::ElementState::Released
                    },
                },
            });
        }
    });
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
    // A corner split previews without changing ECS until release.
    if frame == 1420 {
        let r = world.get::<AreaPresentation>(area).unwrap().surface;
        save_drag(
            world,
            r.max - Vec2::splat(3.0),
            Vec2::new(r.min.x + r.width() * 0.55, r.max.y - 3.0),
        );
    }
    if (1420..=1428).contains(&frame) {
        drag(world, frame, 1420);
    }
    if frame == 1440 {
        let other = areas(world, layout)
            .into_iter()
            .find(|(e, _)| *e != area)
            .unwrap()
            .0;
        assert!(panels_in_area(world, other).is_empty());
        world.resource_mut::<Interaction>().empty = Some(other);
        info!("Panel smoke: native corner split passed");
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
                1462 => Some((imgui::MouseButton::Left, true)),
                1465 => Some((imgui::MouseButton::Right, true)),
                1466 => Some((imgui::MouseButton::Right, false)),
                1468 => Some((imgui::MouseButton::Left, false)),
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
}

fn receiver(frame: UiFrame, mut state: ResMut<Interaction>) {
    frame
        .ui
        .button_with_size("Drop a Notes panel here", [220.0, 100.0]);
    state.widget = Rect::from_corners(
        Vec2::from_array(frame.ui.item_rect_min()),
        Vec2::from_array(frame.ui.item_rect_max()),
    );
    if let Some(target) = frame.ui.drag_drop_target() {
        // SAFETY: inspect only delivery metadata; no pointer or payload bytes escape.
        if let Some(payload) = unsafe {
            target.accept_payload_unchecked(PANEL_TYPE_PAYLOAD, imgui::DragDropTargetFlags::NONE)
        } {
            if payload.delivery {
                state.received += 1;
            }
        }
    }
}
