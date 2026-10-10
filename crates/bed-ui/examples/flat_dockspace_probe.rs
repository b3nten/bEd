//! Headless experiment for application-owned rectangles with native ImGui tabs.
//!
//! Run: cargo run --offline -p bed-ui --example flat_dockspace_probe
//! The mouse tests submit real events over complete ImGui frames. The split/join
//! tests deliberately use DockBuilder only to move contents, never to split.

use dear_imgui_rs::{Condition, ConfigFlags, Context, MouseButton, StyleVar, WindowFlags};
use dear_imgui_sys as sys;
use std::ffi::CString;

unsafe extern "C" {
    fn bed_imgui_dock_node_tab_bar(node: *const sys::ImGuiDockNode) -> *mut sys::ImGuiTabBar;
}

#[derive(Clone, Copy)]
struct Area {
    id: u32,
    pos: [f32; 2],
    size: [f32; 2],
}

#[derive(Default)]
struct TilingPolicy {
    // This is drag history, not a second source of current tab membership.
    last_area: Vec<(&'static str, u32)>,
    pending: Vec<(&'static str, u32)>,
    rehome_count: usize,
}

fn frame(context: &mut Context, areas: &[Area], panels: &[&str]) {
    frame_with_policy(context, areas, panels, None);
}

fn frame_with_policy(
    context: &mut Context,
    areas: &[Area],
    panels: &[&str],
    mut policy: Option<&mut TilingPolicy>,
) {
    let binding = context.binding();
    let ui = context.frame();
    binding.with_bound_context(|| {
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        ui.window("##flat_probe_workspace")
            .position([0.0; 2], Condition::Always)
            .size([1200.0, 800.0], Condition::Always)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_DOCKING
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
                    | WindowFlags::NO_NAV_FOCUS
                    | WindowFlags::NO_SAVED_SETTINGS,
            )
            .build(|| {
                for area in areas {
                    ui.set_cursor_screen_pos(area.pos);
                    unsafe {
                        sys::igDockSpace(
                            area.id,
                            area.size.into(),
                            sys::ImGuiDockNodeFlags_NoDockingSplit,
                            std::ptr::null(),
                        );
                    }
                }
            });
        drop(_border);
        drop(_rounding);
        drop(_padding);
        let pending = policy
            .as_mut()
            .map(|policy| std::mem::take(&mut policy.pending))
            .unwrap_or_default();
        for &title in panels {
            if let Some((_, home)) = pending.iter().find(|(panel, _)| *panel == title) {
                unsafe {
                    sys::igSetNextWindowDockID(*home, sys::ImGuiCond_Always);
                }
                policy.as_mut().unwrap().rehome_count += 1;
            }
            ui.window(title)
                .flags(WindowFlags::NO_COLLAPSE)
                .size([300.0, 220.0], Condition::FirstUseEver)
                .build(|| ui.text(format!("Contents of {title}")));
        }
        if let Some(policy) = policy.as_mut() {
            unsafe {
                let native = &*sys::igGetCurrentContext();
                // Valid docking is queued on the release frame and applies in
                // the following NewFrame. DragDropActive can already be false
                // on release, so explicitly wait until the next frame as well.
                let finished = !native.IO.MouseDown[0]
                    && !native.IO.MouseReleased[0]
                    && !native.DragDropActive;
                for (title, last_area) in &mut policy.last_area {
                    let name = CString::new(*title).unwrap();
                    let window = &*sys::igFindWindowByName(name.as_ptr());
                    if areas.iter().any(|area| area.id == window.DockId) {
                        *last_area = window.DockId;
                    } else if finished {
                        policy.pending.push((*title, *last_area));
                    }
                }
            }
        }
    });
    drop(context.render_legacy());
}

fn native_drag_state(context: &Context) -> (bool, bool, bool, i32) {
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        (
            native.IO.MouseDown[0],
            native.DragDropActive,
            !native.MovingWindow.is_null(),
            native.DockContext.Requests.Size,
        )
    })
}

fn settle(context: &mut Context, areas: &[Area], panels: &[&str]) {
    for _ in 0..4 {
        frame(context, areas, panels);
    }
}

fn dock(context: &Context, title: &str, area: u32) {
    let title = CString::new(title).unwrap();
    context.binding().with_bound_context(|| unsafe {
        sys::igDockBuilderDockWindow(title.as_ptr(), area);
        sys::igDockBuilderFinish(area);
    });
}

fn dock_id(context: &Context, title: &str) -> u32 {
    let title = CString::new(title).unwrap();
    context
        .binding()
        .with_bound_context(|| unsafe { (*sys::igFindWindowByName(title.as_ptr())).DockId })
}

fn tab_center(context: &Context, title: &str) -> [f32; 2] {
    let title = CString::new(title).unwrap();
    context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(title.as_ptr());
        let bar = bed_imgui_dock_node_tab_bar((*window).DockNode)
            .as_ref()
            .unwrap();
        let tabs = std::slice::from_raw_parts(bar.Tabs.Data, bar.Tabs.Size as usize);
        let tab = tabs.iter().find(|tab| tab.Window == window).unwrap();
        [
            bar.BarRect.Min.x + tab.Offset - bar.ScrollingAnim + tab.Width * 0.45,
            (bar.BarRect.Min.y + bar.BarRect.Max.y) * 0.5,
        ]
    })
}

fn drag_tab(context: &mut Context, areas: &[Area], panels: &[&str], title: &str, target: [f32; 2]) {
    let start = tab_center(context, title);
    context.io_mut().add_mouse_pos_event(start);
    frame(context, areas, panels);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(context, areas, panels);
    // Leaving the tab strip crosses ImGui's undock threshold before seeking a target.
    context
        .io_mut()
        .add_mouse_pos_event([start[0], start[1] + 80.0]);
    settle(context, areas, panels);
    context.io_mut().add_mouse_pos_event(target);
    settle(context, areas, panels);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    settle(context, areas, panels);
}

fn assert_flat_geometry(context: &Context, area: Area) {
    context.binding().with_bound_context(|| unsafe {
        let node = sys::igDockBuilderGetNode(area.id);
        assert!(
            !node.is_null(),
            "empty areas must keep their dockspace node"
        );
        assert!(sys::ImGuiDockNode_IsDockSpace(node));
        assert!(sys::ImGuiDockNode_IsRootNode(node));
        assert!(!sys::ImGuiDockNode_IsSplitNode(node));
        let rect = sys::ImGuiDockNode_Rect(node);
        assert_eq!([rect.Min.x, rect.Min.y], area.pos);
        assert_eq!(
            [rect.Max.x - rect.Min.x, rect.Max.y - rect.Min.y],
            area.size
        );
    });
}

fn main() {
    let mut context = Context::create();
    context.set_ini_filename(None::<String>).unwrap();
    context
        .io_mut()
        .set_config_flags(ConfigFlags::DOCKING_ENABLE);
    context.io_mut().set_display_size([1200.0, 800.0]);
    context.io_mut().set_delta_time(1.0 / 60.0);
    context.io_mut().set_config_docking_always_tab_bar(true);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();

    let mut areas = vec![
        Area {
            id: 0xbed001,
            pos: [8.0, 8.0],
            size: [580.0, 784.0],
        },
        Area {
            id: 0xbed002,
            pos: [596.0, 8.0],
            size: [596.0, 784.0],
        },
    ];
    let mut panels = vec![
        "Editor###probe_editor",
        "Debugger###probe_debugger",
        "Settings###probe_settings",
    ];
    settle(&mut context, &areas, &panels);
    dock(&context, panels[0], areas[0].id);
    dock(&context, panels[1], areas[0].id);
    dock(&context, panels[2], areas[1].id);
    settle(&mut context, &areas, &panels);
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    assert_eq!(dock_id(&context, panels[0]), areas[0].id);
    assert_eq!(dock_id(&context, panels[1]), areas[0].id);
    assert_eq!(dock_id(&context, panels[2]), areas[1].id);
    println!("PASS: independent flat dockspaces host native tab groups");

    context.binding().with_bound_context(|| unsafe {
        let source = CString::new(panels[0]).unwrap();
        let target = CString::new(panels[2]).unwrap();
        let source = sys::igFindWindowByName(source.as_ptr());
        let target = sys::igFindWindowByName(target.as_ptr());
        let node = sys::igDockBuilderGetNode(areas[1].id);
        let mut position = [0.0; 2].into();
        assert!(
            !sys::igDockContextCalcDropPosForDocking(
                target,
                node,
                source,
                std::ptr::null_mut(),
                sys::ImGuiDir_Left,
                true,
                &mut position,
            ),
            "NoDockingSplit removes the native side target"
        );
        assert!(
            sys::igDockContextCalcDropPosForDocking(
                target,
                node,
                source,
                std::ptr::null_mut(),
                sys::ImGuiDir_None,
                false,
                &mut position,
            ),
            "native center/tab docking remains available"
        );
    });
    println!("PASS: native docking preview offers a center target and no side targets");

    drag_tab(&mut context, &areas, &panels, panels[1], [894.0, 400.0]);
    assert_eq!(
        dock_id(&context, panels[1]),
        areas[1].id,
        "native mouse tab transfer"
    );
    assert_eq!(dock_id(&context, panels[0]), areas[0].id);
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    println!("PASS: real mouse drag transfers one native tab between areas");

    drag_tab(&mut context, &areas, &panels, panels[0], [894.0, 400.0]);
    assert_eq!(dock_id(&context, panels[0]), areas[1].id);
    assert_flat_geometry(&context, areas[0]);
    context.binding().with_bound_context(|| unsafe {
        assert!(sys::ImGuiDockNode_IsEmpty(sys::igDockBuilderGetNode(
            areas[0].id
        )));
    });
    println!("PASS: removing the final tab leaves its empty area alive");

    // A side drop would ordinarily create a native split. With NoDockingSplit,
    // it remains floating; bed must choose whether to keep it or rehome it.
    drag_tab(
        &mut context,
        &areas,
        &panels,
        panels[1],
        [areas[0].pos[0] + 14.0, 400.0],
    );
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    let floating_id = dock_id(&context, panels[1]);
    assert!(areas.iter().all(|area| area.id != floating_id));
    context.binding().with_bound_context(|| unsafe {
        assert!(sys::ImGuiDockNode_IsFloatingNode(
            sys::igDockBuilderGetNode(floating_id)
        ));
    });
    dock(&context, panels[1], areas[1].id);
    settle(&mut context, &areas, &panels);
    println!("PASS: NoDockingSplit blocks native side splits; rehoming restores a floating tab");

    areas[0].pos = [20.0, 40.0];
    areas[0].size = [420.0, 740.0];
    areas[1].pos = [448.0, 40.0];
    areas[1].size = [732.0, 740.0];
    settle(&mut context, &areas, &panels);
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    println!("PASS: dockspace geometry follows application rectangles");

    // Splitting changes only application geometry and creates a new dockspace.
    // A duplicated editor has a distinct view identity but could share a document.
    areas[1].size[1] = 360.0;
    areas.push(Area {
        id: 0xbed003,
        pos: [448.0, 408.0],
        size: [732.0, 372.0],
    });
    panels.push("Editor duplicate###probe_editor_duplicate");
    settle(&mut context, &areas, &panels);
    dock(&context, panels[3], areas[2].id);
    settle(&mut context, &areas, &panels);
    assert_eq!(dock_id(&context, panels[3]), areas[2].id);
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    println!("PASS: application split creates an area with an independent duplicate view");

    // Rehome all live windows before deleting the removed area's native node.
    let removed = areas.pop().unwrap();
    for &title in &panels {
        if dock_id(&context, title) == removed.id {
            dock(&context, title, areas[1].id);
        }
    }
    context.binding().with_bound_context(|| unsafe {
        sys::igDockBuilderRemoveNode(removed.id);
    });
    areas[1].size[1] = 740.0;
    settle(&mut context, &areas, &panels);
    for &title in &panels {
        assert_eq!(dock_id(&context, title), areas[1].id);
    }
    for &area in &areas {
        assert_flat_geometry(&context, area);
    }
    println!("PASS: application join preserves every panel as a survivor tab");

    let mut ini = String::new();
    context.save_ini_settings(&mut ini);
    assert!(ini.contains("[Docking][Data]"));
    assert!(!ini.contains("Split="));
    assert!(!ini.contains("0x00BED003"));
    println!("PASS: persisted native layout contains flat area roots and no removed root");

    // Native ini data restores tab membership; application geometry remains the
    // source of truth when the same stable workspace/root IDs are submitted.
    drop(context);
    let mut restored = Context::create();
    restored.set_ini_filename(None::<String>).unwrap();
    restored
        .io_mut()
        .set_config_flags(ConfigFlags::DOCKING_ENABLE);
    restored.io_mut().set_display_size([1200.0, 800.0]);
    restored.io_mut().set_delta_time(1.0 / 60.0);
    restored.io_mut().set_config_docking_always_tab_bar(true);
    restored
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    restored.load_ini_settings(&ini);
    areas[1].pos = [448.0, 56.0];
    areas[1].size[1] = 720.0;
    settle(&mut restored, &areas, &panels);
    for &title in &panels {
        assert_eq!(dock_id(&restored, title), areas[1].id);
    }
    for &area in &areas {
        assert_flat_geometry(&restored, area);
    }
    println!("PASS: ini round trip restores tab membership while application geometry wins");

    // Strict tiling allows the native drag preview, then returns an unsuccessful
    // drop to its most recent area. Successful cross-area drops stay native.
    dock(&restored, panels[0], areas[0].id);
    settle(&mut restored, &areas, &panels);
    let mut policy = TilingPolicy {
        last_area: panels
            .iter()
            .map(|&title| (title, dock_id(&restored, title)))
            .collect(),
        ..Default::default()
    };
    let gestures = [
        ("valid transfer", [814.0, 416.0], Some(areas[1].id)),
        ("valid return", [230.0, 410.0], Some(areas[0].id)),
        ("blocked side", [462.0, 416.0], None),
        ("outside every area", [1195.0, 795.0], None),
    ];
    for (gesture, target, destination) in gestures {
        let title = panels[0];
        let home = dock_id(&restored, title);
        let previous_rehomes = policy.rehome_count;
        let start = tab_center(&restored, title);
        restored.io_mut().add_mouse_pos_event(start);
        frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        restored
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        restored
            .io_mut()
            .add_mouse_pos_event([start[0], start[1] + 80.0]);
        for _ in 0..4 {
            frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        }
        restored.io_mut().add_mouse_pos_event(target);
        for _ in 0..4 {
            frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        }
        let floating_id = dock_id(&restored, title);
        assert!(areas.iter().all(|area| area.id != floating_id));
        let held = native_drag_state(&restored);
        assert!(
            held.0 && held.1 && held.2,
            "native drag must remain active until release"
        );
        assert!(
            policy.pending.is_empty(),
            "held drags must not schedule rehoming"
        );
        assert_eq!(policy.rehome_count, previous_rehomes);

        restored
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        let released = native_drag_state(&restored);
        assert!(!released.0 && !released.2);
        assert!(
            policy.pending.is_empty(),
            "release-frame native requests must resolve first"
        );
        assert_eq!(
            released.3 > 0,
            destination.is_some(),
            "only valid drops queue native docking"
        );

        if let Some(destination) = destination {
            frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
            assert_eq!(dock_id(&restored, title), destination);
            assert!(policy.pending.is_empty());
            assert_eq!(policy.rehome_count, previous_rehomes);
        } else {
            // An outside drop can keep its payload alive beyond release. Wait
            // for native drag cleanup before scheduling the one-shot request.
            for _ in 0..4 {
                frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
                if !policy.pending.is_empty() {
                    break;
                }
            }
            assert_eq!(policy.pending, vec![(title, home)]);
            frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
            assert_eq!(dock_id(&restored, title), home);
            assert_eq!(policy.rehome_count, previous_rehomes + 1);
            assert!(policy.pending.is_empty());
        }
        for _ in 0..4 {
            frame_with_policy(&mut restored, &areas, &panels, Some(&mut policy));
        }
        assert!(policy.pending.is_empty(), "rehoming must be one-shot");
        for &area in &areas {
            assert_flat_geometry(&restored, area);
        }
        restored.binding().with_bound_context(|| unsafe {
            assert!(
                sys::igDockBuilderGetNode(floating_id).is_null(),
                "native docking cleans up the empty floating root"
            );
        });
        println!(
            "PASS: strict tiling {gesture}; release state={released:?}; floating root cleaned automatically"
        );
    }
}
