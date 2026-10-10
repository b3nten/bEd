//! Panel and window backgrounds contribute one layer at each visible pixel.
use super::tests::{frame, workspace};
use super::*;
use crate::test_support::TempDir;

/// Composite the emitted solid triangles at a quiet pixel. This catches an
/// opaque underlying surface and two translucent background layers alike.
fn solid_rgba_at(context: &Context, point: [f32; 2]) -> [f32; 4] {
    context.binding().with_bound_context(|| unsafe {
        let data = &*sys::igGetDrawData();
        let mut result = [0.0; 4];
        for list_index in 0..data.CmdLists.Size as usize {
            let list = &**data.CmdLists.Data.add(list_index);
            for command_index in 0..list.CmdBuffer.Size as usize {
                let command = &*list.CmdBuffer.Data.add(command_index);
                let clip = command.ClipRect;
                if command.UserCallback.is_some()
                    || point[0] < clip.x
                    || point[0] >= clip.z
                    || point[1] < clip.y
                    || point[1] >= clip.w
                {
                    continue;
                }
                for offset in (0..command.ElemCount as usize).step_by(3) {
                    let vertices = std::array::from_fn::<_, 3, _>(|index| {
                        let vertex_index = *list
                            .IdxBuffer
                            .Data
                            .add(command.IdxOffset as usize + offset + index)
                            as usize;
                        *list
                            .VtxBuffer
                            .Data
                            .add(command.VtxOffset as usize + vertex_index)
                    });
                    // Font glyphs and images have varying UVs. Solid fills use
                    // one atlas texel, so their vertex alpha is their coverage.
                    if vertices[0].uv != vertices[1].uv || vertices[0].uv != vertices[2].uv {
                        continue;
                    }
                    let [a, b, c] = vertices.map(|vertex| [vertex.pos.x, vertex.pos.y]);
                    let cross = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
                        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
                    };
                    let area = cross(a, b, c);
                    if area.abs() <= f32::EPSILON {
                        continue;
                    }
                    let weights = [cross(point, b, c), cross(a, point, c), cross(a, b, point)]
                        .map(|weight| weight / area);
                    if weights.iter().any(|weight| *weight <= 0.0) {
                        continue;
                    }
                    let source: [f32; 4] = std::array::from_fn(|channel| {
                        vertices
                            .iter()
                            .zip(weights)
                            .map(|(vertex, weight)| {
                                ((vertex.col >> (channel * 8)) & 255) as f32 / 255.0 * weight
                            })
                            .sum()
                    });
                    for channel in 0..3 {
                        result[channel] =
                            source[channel] * source[3] + result[channel] * (1.0 - source[3]);
                    }
                    result[3] = source[3] + result[3] * (1.0 - source[3]);
                }
            }
        }
        result
    })
}

fn solid_alpha_at(context: &Context, point: [f32; 2]) -> f32 {
    solid_rgba_at(context, point)[3]
}

fn assert_background_at(context: &Context, point: [f32; 2], color: [f32; 4], opacity: f32) {
    let actual = solid_rgba_at(context, point);
    let expected = [
        color[0] * opacity,
        color[1] * opacity,
        color[2] * opacity,
        opacity,
    ];
    for channel in 0..4 {
        let tolerance = if channel == 3 { 1.0 } else { 2.0 } / 255.0;
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "background at {point:?}: premultiplied RGBA {actual:?}, expected {expected:?}"
        );
    }
}

#[test]
fn empty_and_tabbed_areas_composite_the_background_opacity_once() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let mut workbench = workspace(&directory);
    while !workbench.tabs.is_empty() {
        workbench.close_tab(0).unwrap();
    }
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.settings.settings["minimap"] = json!(false);
    let mut context = Context::create();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    // A point off the fills' diagonal avoids shared triangle-edge ambiguity.
    for tab_count in 0..=2 {
        if tab_count > 0 {
            let file = directory.write(&format!("blank_{tab_count}.txt"), b"");
            workbench.open_or_focus(&file).unwrap();
        }
        for opacity in [0.0, 0.5, 1.0] {
            workbench.settings.settings["background_opacity"] = json!(opacity);
            workbench.settings.request_apply();
            workbench.apply_settings(&mut context).unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            for _ in 0..6 {
                frame(&mut context, &mut workbench);
            }
            let point = [0, 1].map(|axis| {
                workbench.tiling_ui.origin[axis]
                    + workbench.tiling_ui.size[axis] * [0.73, 0.61][axis]
            });
            let actual = solid_alpha_at(&context, point);
            assert!(
                (actual - opacity).abs() <= 1.0 / 255.0,
                "{tab_count} tabs, opacity {opacity}: content alpha was {actual}"
            );
            if tab_count > 0 {
                let tab = workbench.tabs.last().unwrap();
                let name = CString::new(workbench.title(tab)).unwrap();
                let menu_point = context.binding().with_bound_context(|| unsafe {
                    let window = &*sys::igFindWindowByName(name.as_ptr());
                    (window.MenuBarHeight > 0.0).then_some([
                        window.Pos.x + window.Size.x * 0.73,
                        window.Pos.y + window.TitleBarHeight + window.MenuBarHeight * 0.41,
                    ])
                });
                if let Some(point) = menu_point {
                    let actual = solid_alpha_at(&context, point);
                    assert!(
                        (actual - opacity).abs() <= 1.0 / 255.0,
                        "{tab_count} tabs, opacity {opacity}: menu alpha was {actual}"
                    );
                }
                if tab_count > 1 {
                    let area = workbench.tiling.area_for(tab.id).unwrap();
                    let (min, max) = workbench.tiling_ui.bar_rects[&area];
                    let point =
                        [0, 1].map(|axis| min[axis] + (max[axis] - min[axis]) * [0.73, 0.41][axis]);
                    assert!(
                        workbench.tiling_ui.tab_rects.values().all(|(min, max)| {
                            point[0] < min[0]
                                || point[0] >= max[0]
                                || point[1] < min[1]
                                || point[1] >= max[1]
                        }),
                        "sample the tab strip's quiet background, outside the themed tab fills"
                    );
                    let actual = solid_alpha_at(&context, point);
                    assert!(
                        (actual - opacity).abs() <= 1.0 / 255.0,
                        "{tab_count} tabs, opacity {opacity}: tab bar alpha was {actual}"
                    );
                }
            }
        }
    }
    workbench.cleanup().unwrap();
}

#[test]
fn custom_window_background_fills_gaps_and_vacated_space_without_layering_under_panels() {
    use crate::workspace::{tiling::Layout, tiling_state::TilingState};

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let mut workbench = workspace(&directory);
    while !workbench.tabs.is_empty() {
        workbench.close_tab(0).unwrap();
    }
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.settings.settings["minimap"] = json!(false);
    let mut theme = workbench.settings.theme.to_json();
    theme["name"] = json!("Background composition fixture");
    theme["ui"]["background"] = json!("#214365");
    theme["ui"]["window_background"] = json!("#c47432");
    let selected_theme = workbench.settings.save_custom_theme(None, &theme).unwrap();
    workbench.settings.select_theme(&selected_theme).unwrap();
    let panel_color = [
        0x21 as f32 / 255.0,
        0x43 as f32 / 255.0,
        0x65 as f32 / 255.0,
        1.0,
    ];
    let window_color = [
        0xc4 as f32 / 255.0,
        0x74 as f32 / 255.0,
        0x32 as f32 / 255.0,
        1.0,
    ];
    workbench
        .open_or_focus(&directory.write("first_empty.txt", b""))
        .unwrap();
    let first = workbench.active_panel_id().unwrap();
    let mut context = Context::create();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut layout = Layout::default();
    layout.split(1, 0, 6000, 1).unwrap();
    workbench.tiling = TilingState::new(layout.clone());
    workbench.pending_tiling = None;
    workbench.dock_built = true;
    workbench.place_panel_in_area(first, 1);
    workbench.reset_tiling_docks();
    for opacity in [0.0, 0.5, 1.0] {
        workbench.settings.settings["background_opacity"] = json!(opacity);
        workbench.settings.request_apply();
        workbench.apply_settings(&mut context).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..6 {
            frame(&mut context, &mut workbench);
        }
        assert_eq!(workbench.tiling.layout.areas.len(), 2);
        let origin = workbench.tiling_ui.origin;
        let size = workbench.tiling_ui.size;
        for fraction in [[0.27, 0.61], [0.83, 0.61]] {
            assert_background_at(
                &context,
                [
                    origin[0] + size[0] * fraction[0],
                    origin[1] + size[1] * fraction[1],
                ],
                panel_color,
                opacity,
            );
        }
        assert_background_at(
            &context,
            [origin[0] + size[0] * 0.6 + 0.73, origin[1] + size[1] * 0.61],
            window_color,
            opacity,
        );
        assert_background_at(
            &context,
            [origin[0] + 1.3, origin[1] + size[1] * 0.47],
            window_color,
            opacity,
        );
        assert_background_at(
            &context,
            [origin[0] + 5.12, origin[1] + 5.36],
            window_color,
            opacity,
        );
    }

    workbench.tiling = TilingState::default();
    workbench.place_panel_in_area(first, 1);
    workbench.reset_tiling_docks();
    for _ in 0..6 {
        frame(&mut context, &mut workbench);
    }
    let origin = workbench.tiling_ui.origin;
    let size = workbench.tiling_ui.size;
    assert_background_at(
        &context,
        [origin[0] + size[0] * 0.73, origin[1] + size[1] * 0.61],
        panel_color,
        1.0,
    );

    workbench
        .open_or_focus(&directory.write("second_empty.txt", b""))
        .unwrap();
    let second = workbench.active_panel_id().unwrap();
    workbench.tiling = TilingState::new(layout);
    workbench.place_panel_in_area(first, 1);
    workbench.place_panel_in_area(second, 2);
    workbench.reset_tiling_docks();
    workbench.settings.settings["background_opacity"] = json!(0.5);
    workbench.settings.request_apply();
    workbench.apply_settings(&mut context).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for _ in 0..6 {
        frame(&mut context, &mut workbench);
    }
    workbench.settings.settings["ui_animations"] = json!(true);
    let (min, max) = workbench.tiling_ui.tab_rects[&first];
    context
        .io_mut()
        .add_mouse_pos_event([min[0] + (max[0] - min[0]) * 0.4, (min[1] + max[1]) * 0.5]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_pos_event([origin[0] + size[0] * 0.8, origin[1] + size[1] * 0.5]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.tiling.layout.areas.len(), 1);
    let (moving_min, moving_max) = workbench.tiling_ui.area_bounds[&2];
    assert!(moving_min[0] > origin[0] && moving_min[0] < origin[0] + size[0] * 0.6);
    let vacated = [
        origin[0] + (moving_min[0] - origin[0]) * 0.43,
        origin[1] + size[1] * 0.61,
    ];
    assert_background_at(&context, vacated, window_color, 0.5);
    assert_background_at(
        &context,
        [
            moving_min[0] + (moving_max[0] - moving_min[0]) * 0.77,
            origin[1] + size[1] * 0.61,
        ],
        panel_color,
        0.5,
    );
    for _ in 0..40 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(
        workbench.tiling_ui.area_bounds[&2],
        (origin, [origin[0] + size[0], origin[1] + size[1]])
    );
    assert_background_at(&context, vacated, panel_color, 0.5);

    // Joining the full-height left area into the upper-right area changes the
    // separation from columns to rows. Interpolating those two surviving
    // rectangles independently would overlap their translucent body fills.
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench
        .open_or_focus(&directory.write("third_empty.txt", b""))
        .unwrap();
    let third = workbench.active_panel_id().unwrap();
    let mut layout = Layout::default();
    let right = layout.split(1, 0, 5000, 1).unwrap();
    layout.split(right, 1, 5000, 1).unwrap();
    workbench.tiling = TilingState::new(layout);
    for (panel, area) in [first, second, third].into_iter().zip([1, 2, 3]) {
        workbench.place_panel_in_area(panel, area);
    }
    workbench.reset_tiling_docks();
    for _ in 0..6 {
        frame(&mut context, &mut workbench);
    }
    workbench.settings.settings["ui_animations"] = json!(true);
    context
        .io_mut()
        .add_mouse_pos_event([origin[0] + size[0] * 0.5 - 4.0, origin[1] + 4.0]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_pos_event([origin[0] + size[0] * 0.51, origin[1] + size[1] * 0.25]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.tiling.layout.areas.len(), 2);
    assert_eq!(workbench.tiling.area_for(first), Some(1));
    assert_eq!(workbench.tiling.area_for(third), Some(3));
    assert!(!workbench.tabs.iter().any(|tab| tab.id == second));
    for _ in 0..6 {
        let (top_min, top_max) = workbench.tiling_ui.area_bounds[&1];
        let (bottom_min, bottom_max) = workbench.tiling_ui.area_bounds[&3];
        assert!(
            top_max[0] <= bottom_min[0]
                || bottom_max[0] <= top_min[0]
                || top_max[1] <= bottom_min[1]
                || bottom_max[1] <= top_min[1],
            "a topology-changing join never overlaps the displayed panels"
        );
        assert_background_at(
            &context,
            [origin[0] + size[0] * 0.625, origin[1] + size[1] * 0.625],
            panel_color,
            0.5,
        );
        frame(&mut context, &mut workbench);
    }
    workbench.cleanup().unwrap();
}
