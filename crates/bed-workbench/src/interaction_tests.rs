//! Pointer gestures exercise the real compositor and its canonical layout.
use super::*;
use dear_imgui_rs::{Context, Key, MouseButton, sys};
use std::ffi::CString;
use workspace::tiling::{EXTENT, Layout, Rect};

#[test]
fn tab_bar_tooltips_are_styled_and_animate_when_the_hover_target_changes() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    {
        let mut appearance = fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchAppearance>();
        appearance.animations = true;
        appearance.add_button = true;
    }
    fixture.settle();
    let tab_points = fixture.panels[..2]
        .iter()
        .map(|&panel| {
            let (min, max) = tab_bounds(fixture.app.world(), panel).unwrap();
            [min[0] + (max[0] - min[0]) * 0.4, (min[1] + max[1]) * 0.5]
        })
        .collect::<Vec<_>>();
    let (bar_min, bar_max) = fixture
        .app
        .world()
        .non_send::<tiling_ui::TilingFrontend>()
        .tiling_ui
        .bar_rects[&1];
    let add_point = [
        bar_min[0] + (bar_max[1] - bar_min[1]) * 0.5,
        (bar_min[1] + bar_max[1]) * 0.5,
    ];
    for point in tab_points.into_iter().chain([add_point]) {
        fixture.context.io_mut().add_mouse_pos_event(point);
        let mut alphas = Vec::new();
        for _ in 0..20 {
            fixture.frame();
            let alpha = fixture.context.binding().with_bound_context(|| unsafe {
                let context = &*sys::igGetCurrentContext();
                let window = (0..context.Windows.Size).find_map(|index| {
                    let window = &**context.Windows.Data.add(index as usize);
                    (window.Active
                        && !window.Hidden
                        && window.Flags & sys::ImGuiWindowFlags_Tooltip != 0)
                        .then_some(window)
                })?;
                assert_eq!(
                    window.WindowRounding,
                    bed_ui::util::popup_style::POPUP_ROUNDING
                );
                assert_eq!(
                    window.WindowBorderSize,
                    bed_ui::util::popup_style::POPUP_BORDER_SIZE
                );
                assert_eq!(
                    [window.WindowPadding.x, window.WindowPadding.y],
                    [12.0, 10.0]
                );
                assert_eq!(window.ViewportId, (*sys::igGetMainViewport()).ID);
                let draw = &*window.DrawList;
                (0..draw.VtxBuffer.Size as usize)
                    .map(|index| (*draw.VtxBuffer.Data.add(index)).col >> 24)
                    .max()
            });
            if let Some(alpha) = alpha {
                alphas.push(alpha);
            }
        }
        assert!(
            alphas.iter().any(|alpha| *alpha < 200),
            "opening animation missing: {alphas:?}"
        );
        assert_eq!(alphas.last(), Some(&255));
    }
}

#[test]
fn double_clicking_a_preview_tab_keeps_it_and_italic_titles_render_through_the_compositor() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    let mut fonts = bed_ui::font::UiFonts::default();
    fonts
        .apply(
            &mut fixture.context,
            &bed_ui::font::UiFontOptions::default(),
            true,
        )
        .unwrap();
    fixture
        .context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    fixture.app.world_mut().insert_non_send(fonts);
    let panel = fixture.panels[0];
    let id = fixture.id(panel);
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .areas[0]
        .preview = Some(id);
    fixture.frame();
    fixture.press_tab(panel);
    fixture.release();
    assert_eq!(
        fixture.layout().areas[0].preview,
        Some(id),
        "a single click only selects the preview"
    );
    fixture.press_tab(panel);
    fixture.release();
    assert_eq!(
        fixture.layout().areas[0].preview,
        None,
        "double click promotes the tab"
    );
    assert!(fixture.app.world().entities().contains(panel));
}

struct Pane;

impl Panel for Pane {
    fn title(&self, _: &World) -> String {
        "Pane".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        _: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        ui.text("Live panel");
    }
}

fn factory(_: In<api::PanelCreation>) -> PanelResult {
    Ok(Box::new(Pane))
}

const FADE_MARKER: u32 = 0xff4d_99e6;

fn fade_marker(ui: &Ui) {
    let point = ui.cursor_screen_pos();
    ui.get_window_draw_list()
        .add_rect(point, [point[0] + 24.0, point[1] + 18.0], FADE_MARKER)
        .filled(true)
        .build();
    ui.dummy([28.0, 22.0]);
}

struct AnimatedPane;

impl Panel for AnimatedPane {
    fn title(&self, _: &World) -> String {
        "Animated pane".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        _: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        fade_marker(ui);
        ui.child_window("fade child")
            .size([100.0; 2])
            .build(ui, || fade_marker(ui));
    }
}

fn animated_factory(_: In<api::PanelCreation>) -> PanelResult {
    Ok(Box::new(AnimatedPane))
}

struct Fixture {
    context: Context,
    app: App,
    panels: [Entity; 3],
}

impl Fixture {
    fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(WorkbenchPlugin);
        register(app.world_mut(), "gesture", |registry| {
            registry.panel("gesture.pane", "Pane", factory);
        })
        .unwrap();
        app.world_mut()
            .resource_mut::<WorkbenchAppearance>()
            .animations = false;
        let panels = std::array::from_fn(|_| {
            open_panel(
                app.world_mut(),
                json!({"panel_type":"gesture.pane"}),
                None,
                None,
            )
            .unwrap()
        });
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5000, 1).unwrap();
        let bottom = layout.split(right, 1, 5000, 1).unwrap();
        let mut tiling = TilingState::new(layout);
        for (panel, area) in panels.into_iter().zip([1, right, bottom]) {
            tiling.assign(
                app.world().get::<PanelPresentation>(panel).unwrap().id,
                area,
            );
        }
        app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
        for panel in panels {
            app.world_mut()
                .get_mut::<PanelPresentation>(panel)
                .unwrap()
                .dock_next = true;
        }
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .io_mut()
            .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut fixture = Self {
            context,
            app,
            panels,
        };
        fixture.settle();
        fixture
    }

    fn layout(&self) -> &TilingState {
        &self.app.world().resource::<WorkbenchLayout>().tiling
    }

    fn id(&self, panel: Entity) -> u64 {
        self.app.world().get::<PanelPresentation>(panel).unwrap().id
    }

    fn frame(&mut self) {
        self.context.io_mut().set_display_size([1200., 800.]);
        self.context.io_mut().set_delta_time(1. / 60.);
        render_ui(self.app.world_mut(), self.context.frame());
        drop(self.context.render_legacy());
        process_requests(self.app.world_mut());
        assert!(
            self.app
                .world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
    }

    fn settle(&mut self) {
        for _ in 0..6 {
            self.frame();
        }
    }

    fn screen(&self, point: [i32; 2]) -> [f32; 2] {
        let chrome = self.app.world().non_send::<tiling_ui::TilingFrontend>();
        [0, 1].map(|axis| {
            chrome.tiling_ui.origin[axis]
                + point[axis] as f32 * chrome.tiling_ui.size[axis] / EXTENT as f32
        })
    }

    fn press(&mut self, point: [f32; 2]) {
        self.context.io_mut().add_mouse_pos_event(point);
        self.frame();
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        self.frame();
    }

    fn press_tab(&mut self, panel: Entity) {
        let (min, max) = tab_bounds(self.app.world(), panel).unwrap();
        self.press([min[0] + (max[0] - min[0]) * 0.4, (min[1] + max[1]) * 0.5]);
    }

    fn corner(&mut self) {
        self.press(self.screen([5000, 0]).map(|value| value + 4.));
    }

    fn drag(&mut self, point: [i32; 2]) {
        let point = self.screen(point);
        self.context.io_mut().add_mouse_pos_event(point);
        self.frame();
    }

    fn release(&mut self) {
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        self.frame();
        self.settle();
    }

    fn payload_frame(&mut self, payload_type: &std::ffi::CStr, position: [f32; 2], pressed: bool) {
        self.context.io_mut().add_mouse_pos_event(position);
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, pressed);
        self.context.io_mut().set_delta_time(1. / 60.);
        let ui = self.context.frame();
        ui.with_bound_context(|| unsafe {
            assert!(sys::igBeginDragDropSource(
                sys::ImGuiDragDropFlags_SourceExtern
                    | sys::ImGuiDragDropFlags_SourceNoPreviewTooltip
            ));
            let data = b"gesture.new";
            sys::igSetDragDropPayload(
                payload_type.as_ptr(),
                data.as_ptr().cast(),
                data.len(),
                sys::ImGuiCond_Always,
            );
            sys::igEndDragDropSource();
        });
        render_ui(self.app.world_mut(), ui);
        drop(self.context.render_legacy());
        process_requests(self.app.world_mut());
    }

    fn membership(&self) {
        self.context.binding().with_bound_context(|| unsafe {
            for panel in self
                .panels
                .into_iter()
                .filter(|panel| self.app.world().entities().contains(*panel))
            {
                let name = CString::new(format!("###bed_tab_{}", self.id(panel))).unwrap();
                let window = sys::igFindWindowByName(name.as_ptr()).as_ref().unwrap();
                let area = self.layout().area_for(self.id(panel)).unwrap();
                assert_eq!(
                    window.DockId,
                    self.app
                        .world()
                        .non_send::<tiling_ui::TilingFrontend>()
                        .tiling_ui
                        .docks[&area]
                );
                assert!(sys::ImGuiDockNode_IsLeafNode(window.DockNode));
                assert!(sys::ImGuiDockNode_IsDockSpace(window.DockNode));
            }
        });
    }
}

#[test]
fn newly_opened_panels_and_custom_child_content_fade_without_moving_layout() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    register(fixture.app.world_mut(), "fade", |registry| {
        registry.panel("fade.pane", "Animated pane", animated_factory);
    })
    .unwrap();
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchAppearance>()
        .animations = true;
    let panel = open_panel(
        fixture.app.world_mut(),
        json!({"panel_type":"fade.pane"}),
        Some(1),
        None,
    )
    .unwrap();
    let name = format!("bed_tab_{}", fixture.id(panel));
    let mut samples = Vec::new();
    for _ in 0..28 {
        fixture.context.io_mut().set_display_size([1200., 800.]);
        fixture.context.io_mut().set_delta_time(1. / 60.);
        let ui = fixture.context.frame();
        ui.window("Embedding host")
            .position([1200., 0.], Condition::Always)
            .build(|| fade_marker(ui));
        render_ui(fixture.app.world_mut(), ui);
        let binding = fixture.context.binding();
        let collect = || {
            binding.with_bound_context(|| unsafe {
                let context = &*sys::igGetCurrentContext();
                (0..context.Windows.Size)
                    .filter_map(|index| {
                        let window = &**context.Windows.Data.add(index as usize);
                        if !window.Active || window.Hidden {
                            return None;
                        }
                        let window_name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                        if !window_name.contains(&name) && window_name != "Embedding host" {
                            return None;
                        }
                        let list = &*window.DrawList;
                        let alpha = (0..list.VtxBuffer.Size as usize).find_map(|index| {
                            let vertex = &*list.VtxBuffer.Data.add(index);
                            (vertex.col & 0x00ff_ffff == FADE_MARKER & 0x00ff_ffff)
                                .then_some(vertex.col >> 24)
                        })?;
                        Some((
                            window.ID,
                            [window.Pos.x, window.Pos.y],
                            alpha,
                            window_name == "Embedding host",
                        ))
                    })
                    .collect::<Vec<_>>()
            })
        };
        let before = collect();
        drop(fixture.context.render_legacy());
        let after = collect();
        assert_eq!(before.len(), after.len());
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(before.0, after.0);
            assert_eq!(before.1, after.1, "animation moved native input/layout");
            assert_eq!(before.2, 255);
            if after.3 {
                assert_eq!(after.2, 255, "workbench faded its embedding host");
            }
        }
        let alpha = after
            .iter()
            .filter(|window| !window.3)
            .map(|window| window.2)
            .collect::<Vec<_>>();
        if !alpha.is_empty() {
            assert_eq!(alpha.len(), 2, "panel and custom child must both render");
            assert_eq!(alpha[0], alpha[1], "child missed its parent's fade");
            samples.push(alpha[0]);
        }
        process_requests(fixture.app.world_mut());
    }
    assert!(samples.len() > 20);
    assert!(
        samples[0] > 0 && samples[0] < 255,
        "new panel appeared opaque"
    );
    assert!(
        samples
            .windows(2)
            .any(|pair| pair[1] > pair[0] && pair[1] < 255)
    );
    assert!(samples.windows(2).all(|pair| pair[1] >= pair[0]));
    assert_eq!(samples.last(), Some(&255));
}

#[test]
fn reserved_footer_stays_outside_panels_in_native_and_embedded_bounds() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    for (bounds, top, bottom) in [
        (None, 32.0, 28.0),
        (Some(([74.0, 96.0], [842.0, 472.0])), 14.0, 30.0),
    ] {
        *fixture.app.world_mut().resource_mut::<WorkbenchViewport>() = WorkbenchViewport {
            bounds,
            top_inset: top,
            bottom_inset: bottom,
        };
        fixture.settle();
        let (origin, size) = bounds.unwrap_or(([0.0; 2], [1200.0, 800.0]));
        let chrome = fixture.app.world().non_send::<tiling_ui::TilingFrontend>();
        assert_eq!(chrome.tiling_ui.origin, [origin[0], origin[1] + top]);
        assert_eq!(chrome.tiling_ui.size, [size[0], size[1] - top - bottom]);
        for panel in fixture.panels {
            let geometry = fixture
                .app
                .world()
                .get::<api::PanelGeometry>(panel)
                .unwrap();
            assert!(geometry.visible);
            assert!(geometry.content_min[1] >= origin[1] + top);
            assert!(geometry.content_max[1] <= origin[1] + size[1] - bottom);
        }
    }
}

#[test]
fn native_titlebar_inset_receives_the_current_theme_background() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchViewport>()
        .top_inset = 32.0;
    for background in [[0.17, 0.31, 0.57, 0.45], [0.85, 0.65, 0.35, 1.0]] {
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchAppearance>()
            .background = background;
        fixture.context.io_mut().set_display_size([1200., 800.]);
        fixture.context.io_mut().set_delta_time(1. / 60.);
        render_ui(fixture.app.world_mut(), fixture.context.frame());
        let covered = fixture.context.binding().with_bound_context(|| unsafe {
            let draw = &*sys::igGetBackgroundDrawList(sys::igGetMainViewport());
            let vertices =
                std::slice::from_raw_parts(draw.VtxBuffer.Data, draw.VtxBuffer.Size as usize);
            let color = sys::igColorConvertFloat4ToU32(background.into());
            [[0., 0.], [1200., 0.], [0., 32.], [1200., 32.]]
                .into_iter()
                .all(|point| {
                    vertices.iter().any(|vertex| {
                        // The Rust draw API and ImGui round channels differently.
                        [0, 8, 16, 24].into_iter().all(|shift| {
                            ((vertex.col >> shift) as u8).abs_diff((color >> shift) as u8) <= 1
                        }) && [vertex.pos.x, vertex.pos.y] == point
                    })
                })
        });
        drop(fixture.context.render_legacy());
        assert!(covered, "titlebar pixels did not adopt {background:?}");
        for panel in fixture.panels {
            assert!(tab_bounds(fixture.app.world(), panel).unwrap().0[1] >= 32.);
        }
    }
}

#[test]
fn right_clicking_an_inactive_tab_offers_all_group_close_actions() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for (row, closed) in [
        (0, vec![1]),
        (1, vec![0, 1, 2]),
        (2, vec![0, 2]),
        (3, vec![2]),
        (4, vec![0]),
    ] {
        let mut fixture = Fixture::new();
        let middle = fixture.panels[1];
        let id = fixture.id(middle);
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchLayout>()
            .tiling
            .move_tab(id, 1, 1);
        fixture
            .app
            .world_mut()
            .get_mut::<PanelPresentation>(middle)
            .unwrap()
            .dock_next = true;
        let last = open_panel(
            fixture.app.world_mut(),
            json!({"panel_type":"gesture.pane"}),
            Some(1),
            None,
        )
        .unwrap();
        let group = [fixture.panels[0], middle, last];
        fixture.settle();
        let before = fixture.layout().clone();
        let (min, max) = tab_bounds(fixture.app.world(), middle).unwrap();
        fixture
            .context
            .io_mut()
            .add_mouse_pos_event([min[0] + 12., (min[1] + max[1]) * 0.5]);
        fixture.frame();
        fixture
            .context
            .io_mut()
            .add_mouse_button_event(MouseButton::Right, true);
        fixture.frame();
        fixture
            .context
            .io_mut()
            .add_mouse_button_event(MouseButton::Right, false);
        fixture.frame();
        fixture.settle();
        assert_eq!(
            fixture.layout(),
            &before,
            "opening a tab menu changed membership"
        );
        let point = fixture.context.binding().with_bound_context(|| unsafe {
            let context = &*sys::igGetCurrentContext();
            assert_eq!(
                context.OpenPopupStack.Size, 1,
                "right-click did not open the menu"
            );
            let popup = &*(*context.OpenPopupStack.Data).Window;
            let height = context.FontSize * 1.4;
            [
                popup.DC.CursorStartPos.x + 15.,
                popup.DC.CursorStartPos.y + row as f32 * height + context.FontSize * 0.5,
            ]
        });
        fixture.press(point);
        fixture.release();
        for (index, panel) in group.into_iter().enumerate() {
            assert_eq!(
                fixture.app.world().entities().contains(panel),
                !closed.contains(&index),
                "menu row {row}, group tab {index}"
            );
        }
        assert!(
            fixture.app.world().entities().contains(fixture.panels[2]),
            "tab menu closed another area's panel"
        );
    }
}

#[test]
fn corner_extension_previews_then_preserves_all_contents_across_a_t() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for (start, offset) in [([5000, 0], [4., 4.]), ([5000, 5000], [3., -3.])] {
        let mut fixture = Fixture::new();
        let before = fixture.layout().clone();
        let point = fixture.screen(start);
        fixture.press([point[0] + offset[0], point[1] + offset[1]]);
        for point in [[4900, 2500], [2500, 5000], [4900, 2500]] {
            fixture.drag(point);
            assert_eq!(fixture.layout(), &before);
        }
        fixture.release();
        assert_eq!(fixture.layout().layout.areas.len(), 3);
        assert_eq!(
            fixture.layout().layout.area(2).rect,
            Rect {
                min: [0, 0],
                max: [EXTENT, 5000]
            }
        );
        assert_eq!(
            fixture.layout().layout.area(1).rect,
            Rect {
                min: [0, 5000],
                max: [5000, EXTENT]
            }
        );
        for (panel, area) in fixture.panels.into_iter().zip([1, 2, 3]) {
            assert_eq!(fixture.layout().area_for(fixture.id(panel)), Some(area));
        }
        fixture.membership();
    }
}

#[test]
fn deeper_corner_placement_closes_only_eliminated_contents_and_keeps_source_focus() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    let source = fixture.panels[1];
    focus_panel(fixture.app.world_mut(), source);
    fixture.settle();
    fixture.corner();
    fixture.drag([4900, 2500]);
    fixture.drag([2500, 5000]);
    fixture.release();
    assert!(!fixture.app.world().entities().contains(fixture.panels[0]));
    assert_eq!(fixture.layout().layout.areas.len(), 2);
    assert_eq!(
        fixture.app.world().resource::<WorkbenchLayout>().focused,
        Some(source)
    );
    assert_eq!(
        fixture.layout().layout.area(2).rect,
        Rect {
            min: [0, 0],
            max: [5000, EXTENT]
        }
    );
    fixture.membership();
}

#[test]
fn escape_cancels_corner_and_shared_border_geometry() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for border in [false, true] {
        let mut fixture = Fixture::new();
        let before = fixture.layout().clone();
        if border {
            fixture.press(fixture.screen([5000, 7500]));
            fixture.drag([6000, 7500]);
            assert_eq!(fixture.layout().layout.area(1).rect.max[0], 6000);
        } else {
            fixture.corner();
            fixture.drag([2500, 5000]);
            assert_eq!(fixture.layout(), &before);
        }
        fixture.context.io_mut().add_key_event(Key::Escape, true);
        fixture.frame();
        fixture.context.io_mut().add_key_event(Key::Escape, false);
        fixture.release();
        assert_eq!(fixture.layout(), &before);
        fixture.membership();
    }
}

#[test]
fn tab_center_drop_moves_only_one_tab_and_selects_its_existing_frontend() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    let moved = fixture.panels[0];
    let id = fixture.id(moved);
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .assign(id, 3);
    fixture
        .app
        .world_mut()
        .get_mut::<PanelPresentation>(moved)
        .unwrap()
        .dock_next = true;
    fixture.settle();
    let before = fixture.layout().layout.clone();
    fixture.press_tab(moved);
    fixture.drag([7500, 2500]);
    assert_eq!(fixture.layout().layout, before);
    fixture.release();
    let group = fixture
        .layout()
        .areas
        .iter()
        .find(|group| group.area == 2)
        .unwrap();
    assert_eq!(group.tabs, vec![fixture.id(fixture.panels[1]), id]);
    assert_eq!(group.selected, Some(id));
    assert_eq!(fixture.layout().layout, before);
    fixture.membership();
}

#[test]
fn tab_edge_drops_split_all_four_sides_without_cloning_panels() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for (point, rect) in [
        (
            [1000, 5000],
            Rect {
                min: [0, 0],
                max: [2500, EXTENT],
            },
        ),
        (
            [4500, 5000],
            Rect {
                min: [2500, 0],
                max: [5000, EXTENT],
            },
        ),
        (
            [2500, 1000],
            Rect {
                min: [0, 0],
                max: [5000, 5000],
            },
        ),
        (
            [2500, 9000],
            Rect {
                min: [0, 5000],
                max: [5000, EXTENT],
            },
        ),
    ] {
        let mut fixture = Fixture::new();
        let moved = fixture.panels[1];
        let before = fixture.layout().clone();
        fixture.press_tab(moved);
        fixture.drag(point);
        assert_eq!(fixture.layout(), &before);
        fixture.release();
        let area = fixture.layout().area_for(fixture.id(moved)).unwrap();
        assert_ne!(area, 2);
        assert_eq!(fixture.layout().layout.area(area).rect, rect);
        assert_eq!(fixture.layout().layout.areas.len(), 3);
        assert_eq!(
            fixture
                .app
                .world_mut()
                .query::<&WorkbenchPanel>()
                .iter(fixture.app.world())
                .count(),
            3
        );
        fixture.membership();
    }
}

#[test]
fn cancelled_and_outside_tab_drops_preserve_membership() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for cancel in [None, Some(Key::Escape)] {
        let mut fixture = Fixture::new();
        let before = fixture.layout().clone();
        fixture.press_tab(fixture.panels[1]);
        fixture.drag(if cancel.is_some() {
            [2500, 5000]
        } else {
            [12000, 5000]
        });
        if let Some(key) = cancel {
            fixture.context.io_mut().add_key_event(key, true);
            fixture.frame();
            fixture.context.io_mut().add_key_event(key, false);
        }
        fixture.release();
        assert_eq!(fixture.layout(), &before);
        fixture.membership();
    }
}

#[test]
fn closing_the_final_tab_keeps_blank_areas_and_defaults_commands_to_the_largest() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    let last = fixture.panels[2];
    focus_panel(fixture.app.world_mut(), last);
    for panel in fixture.panels {
        fixture
            .app
            .world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::ClosePanel(panel));
    }
    process_requests(fixture.app.world_mut());
    fixture.settle();
    assert!(
        fixture
            .layout()
            .areas
            .iter()
            .all(|group| group.tabs.is_empty())
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<WorkbenchLayout>()
            .focused_area,
        Some(3)
    );
    assert_eq!(
        capture_command(fixture.app.world(), "unused", Value::Null).area,
        1
    );
    assert!(area_bounds(fixture.app.world(), 3).is_some());
}

#[test]
fn native_toolbar_payloads_use_the_same_occupied_and_empty_area_drop_targets() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for empty in [false, true] {
        let mut fixture = Fixture::new();
        let panel = fixture.panels[2];
        let area = fixture.layout().area_for(fixture.id(panel)).unwrap();
        if empty {
            commit_close(fixture.app.world_mut(), panel);
            fixture.settle();
        }
        let position = fixture.screen([7500, 7500]);
        fixture.context.io_mut().add_mouse_pos_event(position);
        fixture.frame();
        let mut cursor = bevy::ecs::message::MessageCursor::<api::PanelDrop>::default();
        for pressed in [true, true, false] {
            fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, pressed);
            fixture.context.io_mut().set_delta_time(1. / 60.);
            let ui = fixture.context.frame();
            ui.with_bound_context(|| unsafe {
                assert!(sys::igBeginDragDropSource(
                    sys::ImGuiDragDropFlags_SourceExtern
                ));
                let command = b"gesture.new";
                sys::igSetDragDropPayload(
                    c"BED_TOOLBAR_COMMAND".as_ptr(),
                    command.as_ptr().cast(),
                    command.len(),
                    sys::ImGuiCond_Always,
                );
                ui.text("New pane");
                sys::igEndDragDropSource();
            });
            render_ui(fixture.app.world_mut(), ui);
            drop(fixture.context.render_legacy());
        }
        let drops = cursor
            .read(fixture.app.world().resource::<Messages<api::PanelDrop>>())
            .collect::<Vec<_>>();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].area, area);
        assert_eq!(drops[0].panel, if empty { None } else { Some(panel) });
        assert_eq!(drops[0].payload_type, "BED_TOOLBAR_COMMAND");
        assert_eq!(drops[0].data, b"gesture.new");
    }
}

#[test]
fn incoming_panel_content_splits_each_edge_and_preserves_existing_tabs() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for payload_type in [c"BED_TOOLBAR_COMMAND", c"BED_FILES"] {
        for (point, axis, high) in [
            ([5100, 7500], 0, false),
            ([9900, 7500], 0, true),
            ([7500, 5600], 1, false),
            ([7500, 9900], 1, true),
        ] {
            let mut fixture = Fixture::new();
            let original = fixture.layout().clone();
            let position = fixture.screen(point);
            fixture.context.io_mut().add_mouse_pos_event(position);
            fixture.frame();
            let mut cursor = bevy::ecs::message::MessageCursor::<api::PanelDrop>::default();
            fixture.payload_frame(payload_type, position, true);
            fixture.payload_frame(payload_type, position, true);
            assert_eq!(fixture.layout(), &original, "hover is only a preview");
            let preview = fixture
                .app
                .world()
                .non_send::<tiling_ui::TilingFrontend>()
                .tiling_ui
                .preview_bounds
                .expect("shared split preview is visible");
            fixture.payload_frame(payload_type, position, false);
            let drops = cursor
                .read(fixture.app.world().resource::<Messages<api::PanelDrop>>())
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(drops.len(), 1);
            let drop = &drops[0];
            assert_eq!(drop.panel, None);
            assert_eq!(drop.payload_type, payload_type.to_str().unwrap());
            assert_eq!(drop.data, b"gesture.new");
            assert_eq!(
                fixture.layout().layout.areas.len(),
                original.layout.areas.len() + 1
            );
            assert!(
                fixture
                    .layout()
                    .areas
                    .iter()
                    .find(|group| group.area == drop.area)
                    .unwrap()
                    .tabs
                    .is_empty()
            );
            for group in &original.areas {
                assert_eq!(
                    fixture
                        .layout()
                        .areas
                        .iter()
                        .find(|next| next.area == group.area)
                        .unwrap(),
                    group
                );
            }
            let rect = fixture.layout().layout.area(drop.area).rect;
            assert_eq!(
                if high { rect.max[axis] } else { rect.min[axis] },
                if high { EXTENT } else { EXTENT / 2 }
            );
            let bounds = (fixture.screen(rect.min), fixture.screen(rect.max));
            assert!((preview.0[axis] - bounds.0[axis]).abs() <= 5.0);
            assert!((preview.1[axis] - bounds.1[axis]).abs() <= 5.0);
            fixture.settle();
            fixture.membership();
        }
    }
}

#[test]
fn incoming_panel_content_uses_tab_insertion_and_animated_preview_transitions() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    fixture
        .app
        .world_mut()
        .resource_mut::<WorkbenchAppearance>()
        .animations = true;
    let position = fixture.screen([7500, 7500]);
    fixture.context.io_mut().add_mouse_pos_event(position);
    fixture.frame();
    fixture.payload_frame(c"BED_FILES", position, true);
    let first = fixture
        .app
        .world()
        .non_send::<tiling_ui::TilingFrontend>()
        .tiling_ui
        .preview_bounds
        .unwrap();
    fixture.payload_frame(c"BED_FILES", fixture.screen([5100, 7500]), true);
    let moving = fixture
        .app
        .world()
        .non_send::<tiling_ui::TilingFrontend>()
        .tiling_ui
        .preview_bounds
        .unwrap();
    assert!(moving.1[0] < first.1[0] && moving.1[0] > fixture.screen([7500, 7500])[0]);
    let panel = fixture.panels[2];
    let area = fixture.layout().area_for(fixture.id(panel)).unwrap();
    let (min, max) = tab_bounds(fixture.app.world(), panel).unwrap();
    let position = [min[0] + 1.0, (min[1] + max[1]) * 0.5];
    fixture.payload_frame(c"BED_FILES", position, true);
    fixture.payload_frame(c"BED_FILES", position, false);
    let mut cursor = bevy::ecs::message::MessageCursor::<api::PanelDrop>::default();
    let drops = cursor
        .read(fixture.app.world().resource::<Messages<api::PanelDrop>>())
        .collect::<Vec<_>>();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].area, area);
    assert_eq!(drops[0].tab_index, Some(0));
    assert_eq!(drops[0].panel, Some(panel));
    assert_eq!(fixture.layout().layout.areas.len(), 3);
}

#[test]
fn cancelled_and_outside_payload_drops_leave_layout_unchanged() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    for cancel in ["escape", "right", "outside"] {
        let mut fixture = Fixture::new();
        let original = fixture.layout().clone();
        let position = fixture.screen([5100, 7500]);
        fixture.context.io_mut().add_mouse_pos_event(position);
        fixture.frame();
        fixture.payload_frame(c"BED_TOOLBAR_COMMAND", position, true);
        match cancel {
            "right" => fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Right, true),
            "escape" => fixture.context.io_mut().add_key_event(Key::Escape, true),
            _ => {}
        }
        let position = if cancel != "outside" {
            position
        } else {
            [-20.0; 2]
        };
        fixture.payload_frame(c"BED_TOOLBAR_COMMAND", position, true);
        fixture.payload_frame(c"BED_TOOLBAR_COMMAND", position, false);
        let mut cursor = bevy::ecs::message::MessageCursor::<api::PanelDrop>::default();
        assert_eq!(
            cursor
                .read(fixture.app.world().resource::<Messages<api::PanelDrop>>())
                .count(),
            0
        );
        assert_eq!(fixture.layout(), &original);
        assert!(
            fixture
                .app
                .world()
                .non_send::<tiling_ui::TilingFrontend>()
                .tiling_ui
                .preview_bounds
                .is_none()
        );
    }
}

#[derive(Resource, Default)]
struct WidgetDrops(usize);

struct DropWidget;

impl Panel for DropWidget {
    fn title(&self, _: &World) -> String {
        "Drop widget".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _: &bed_workbench_api::ShortcutInput,
    ) {
        let min = ui.cursor_screen_pos();
        let size = ui.content_region_avail();
        let center = [min[0] + size[0] * 0.5, min[1] + size[1] * 0.5];
        let bounds = sys::ImRect_c {
            Min: [center[0] - 50.0, center[1] - 40.0].into(),
            Max: [center[0] + 50.0, center[1] + 40.0].into(),
        };
        unsafe {
            if sys::igBeginDragDropTargetCustom(bounds, ui.get_id("drop widget").raw()) {
                let payload = sys::igAcceptDragDropPayload(
                    c"BED_FILES".as_ptr(),
                    sys::ImGuiDragDropFlags_AcceptBeforeDelivery
                        | sys::ImGuiDragDropFlags_AcceptNoDrawDefaultRect,
                );
                if !payload.is_null() && (*payload).Delivery {
                    world.resource_mut::<WidgetDrops>().0 += 1;
                }
                sys::igEndDragDropTarget();
            }
        }
    }
}

fn drop_widget_factory(_: In<api::PanelCreation>) -> PanelResult {
    Ok(Box::new(DropWidget))
}

#[test]
fn content_widget_drop_targets_take_precedence_over_workspace_placement() {
    let _lock = tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::new();
    fixture.app.init_resource::<WidgetDrops>();
    register(fixture.app.world_mut(), "drop", |registry| {
        registry.panel("drop.widget", "Drop widget", drop_widget_factory);
    })
    .unwrap();
    open_panel(
        fixture.app.world_mut(),
        json!({"panel_type":"drop.widget"}),
        Some(3),
        None,
    )
    .unwrap();
    fixture.settle();
    let original = fixture.layout().clone();
    let position = fixture.screen([7500, 7500]);
    fixture.context.io_mut().add_mouse_pos_event(position);
    fixture.frame();
    for pressed in [true, true, false] {
        fixture.payload_frame(c"BED_FILES", position, pressed);
        assert!(
            fixture
                .app
                .world()
                .non_send::<tiling_ui::TilingFrontend>()
                .tiling_ui
                .preview_bounds
                .is_none()
        );
    }
    assert_eq!(fixture.app.world().resource::<WidgetDrops>().0, 1);
    let mut cursor = bevy::ecs::message::MessageCursor::<api::PanelDrop>::default();
    assert_eq!(
        cursor
            .read(fixture.app.world().resource::<Messages<api::PanelDrop>>())
            .count(),
        0
    );
    assert_eq!(fixture.layout(), &original);
}
