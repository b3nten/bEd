//! Pointer gestures against the real workbench and its document panels.
use super::tests::{frame, workspace};
use super::*;
use crate::{
    test_support::TempDir,
    workspace::tiling::{EXTENT, Layout, Rect},
    workspace::tiling_state::TilingState,
};

struct Fixture {
    context: Context,
    workbench: Workbench,
    panels: [u64; 3],
    _directory: TempDir,
}

impl Fixture {
    fn t_layout() -> Self {
        let directory = TempDir::new();
        let mut workbench = workspace(&directory);
        // Most gesture tests inspect committed geometry. Animation tests opt
        // in explicitly and inspect intermediate frames before settling.
        workbench.settings.settings["ui_animations"] = json!(false);
        while !workbench.tabs.is_empty() {
            workbench.close_tab(0).unwrap();
        }
        for name in ["left.rs", "top_right.rs", "bottom_right.rs"] {
            let path = directory.write(name, b"fn main() {}\n");
            workbench.open_or_focus(&path).unwrap();
        }
        let panels = std::array::from_fn(|index| workbench.tabs[index].id);
        let mut context = Context::create();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..4 {
            frame(&mut context, &mut workbench);
        }
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5000, 1).unwrap();
        let bottom = layout.split(right, 1, 5000, 1).unwrap();
        assert_eq!([right, bottom], [2, 3]);
        workbench.tiling = TilingState::new(layout);
        workbench.pending_tiling = None;
        for (panel, area) in panels.into_iter().zip([1, 2, 3]) {
            workbench.place_panel_in_area(panel, area);
        }
        workbench.reset_tiling_docks();
        let mut fixture = Self {
            context,
            workbench,
            panels,
            _directory: directory,
        };
        fixture.settle();
        fixture
    }

    fn frame(&mut self) {
        frame(&mut self.context, &mut self.workbench);
    }
    fn settle(&mut self) {
        for _ in 0..6 {
            self.frame();
        }
    }
    fn screen(&self, point: [i32; 2]) -> [f32; 2] {
        [0, 1].map(|axis| {
            self.workbench.tiling_ui.origin[axis]
                + point[axis] as f32 * self.workbench.tiling_ui.size[axis] / EXTENT as f32
        })
    }
    fn press_at(&mut self, point: [f32; 2]) {
        self.context.io_mut().add_mouse_pos_event(point);
        self.frame();
        self.context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        self.frame();
    }
    fn press_tab(&mut self, panel: u64) {
        let (min, max) = self.workbench.tiling_ui.tab_rects[&panel];
        self.press_at([min[0] + (max[0] - min[0]) * 0.4, (min[1] + max[1]) * 0.5]);
    }
    fn group(&self, area: u32) -> &crate::workspace::tiling_state::AreaTabs {
        self.workbench
            .tiling
            .areas
            .iter()
            .find(|group| group.area == area)
            .unwrap()
    }
    fn press_top_right_corner(&mut self) {
        let point = self.screen([5000, 0]).map(|coordinate| coordinate + 4.0);
        self.press_at(point);
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
    fn assert_native_membership(&self) {
        self.context.binding().with_bound_context(|| unsafe {
            for panel in &self.workbench.tabs {
                let name = CString::new(self.workbench.title(panel)).unwrap();
                let window = sys::igFindWindowByName(name.as_ptr()).as_ref().unwrap();
                let area = self.workbench.tiling.area_for(panel.id).unwrap();
                assert_eq!(window.DockId, self.workbench.tiling_ui.docks[&area]);
                assert_eq!(window.ViewportId, (*sys::igGetMainViewport()).ID);
                assert!(sys::ImGuiDockNode_IsDockSpace(window.DockNode));
                assert!(sys::ImGuiDockNode_IsLeafNode(window.DockNode));
            }
        });
    }
    fn cleanup(&mut self) {
        self.workbench.cleanup().unwrap();
    }
}

#[test]
fn a_corner_drag_extends_across_a_t_and_keeps_the_two_lower_columns() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for (start, offset) in [
        ([5000, 0], [4.0, 4.0]),
        ([5000, 0], [16.0, 8.0]),
        ([5000, 5000], [3.0, -3.0]),
    ] {
        let mut fixture = Fixture::t_layout();
        let documents = fixture
            .workbench
            .tabs
            .iter()
            .map(|tab| tab.panel.document().unwrap())
            .collect::<Vec<_>>();
        let before = fixture.workbench.tiling.clone();
        let point = fixture.screen(start);
        fixture.press_at([point[0] + offset[0], point[1] + offset[1]]);
        for point in [[4900, 2500], [2500, 5000], [4900, 2500]] {
            fixture.drag(point);
            assert_eq!(
                fixture.workbench.tiling, before,
                "corner movement changes only the preview"
            );
        }
        fixture.release();
        assert_eq!(fixture.workbench.tiling.layout.areas.len(), 3);
        for (id, rect) in [
            (
                2,
                Rect {
                    min: [0, 0],
                    max: [EXTENT, 5000],
                },
            ),
            (
                1,
                Rect {
                    min: [0, 5000],
                    max: [5000, EXTENT],
                },
            ),
            (
                3,
                Rect {
                    min: [5000, 5000],
                    max: [EXTENT, EXTENT],
                },
            ),
        ] {
            assert_eq!(fixture.workbench.tiling.layout.area(id).rect, rect);
        }
        for (panel, area) in fixture.panels.into_iter().zip([1, 2, 3]) {
            assert_eq!(fixture.workbench.tiling.area_for(panel), Some(area));
        }
        assert_eq!(
            fixture.workbench.tabs.len(),
            documents.len(),
            "a trimmed surviving area keeps all its contents"
        );
        for document in documents {
            assert!(fixture.workbench.session.snapshot(document).is_ok());
            assert_eq!(fixture.workbench.session.view_count(document), 1);
        }
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn dragging_a_corner_deeper_replaces_the_target_and_closes_its_contents() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let removed_document = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == fixture.panels[0])
        .unwrap()
        .panel
        .document()
        .unwrap();
    let source = fixture.panels[1];
    let source_view = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == source)
        .unwrap()
        .panel
        .instance
        .view_id()
        .unwrap();
    let index = fixture
        .workbench
        .tabs
        .iter()
        .position(|tab| tab.id == source)
        .unwrap();
    fixture.workbench.switch_to_tab(index);
    fixture.settle();
    let before = fixture.workbench.tiling.clone();
    let removed = fixture.workbench.tiling_ui.docks[&1];
    fixture.press_top_right_corner();
    fixture.drag([4900, 2500]);
    fixture.drag([2500, 5000]);
    assert_eq!(fixture.workbench.tiling, before);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    assert_eq!(
        fixture.workbench.tiling.layout.area(2).rect,
        Rect {
            min: [0, 0],
            max: [5000, EXTENT],
        }
    );
    assert_eq!(
        fixture.workbench.tiling.layout.area(3).rect,
        Rect {
            min: [5000, 0],
            max: [EXTENT, EXTENT],
        }
    );
    assert_eq!(fixture.group(2).tabs, vec![source]);
    assert_eq!(fixture.workbench.tiling.area_for(fixture.panels[0]), None);
    assert!(
        !fixture
            .workbench
            .tabs
            .iter()
            .any(|tab| tab.id == fixture.panels[0])
    );
    assert!(
        !fixture
            .workbench
            .session
            .document_ids()
            .contains(&removed_document)
    );
    assert_eq!(fixture.workbench.focused, Some(source));
    assert_eq!(fixture.workbench.active_view(), Some(source_view));
    assert_eq!(fixture.group(2).selected, Some(fixture.panels[1]));
    fixture.context.binding().with_bound_context(|| unsafe {
        assert!(sys::igDockBuilderGetNode(removed).is_null());
        let name = CString::new(format!("###bed_tab_{}", fixture.panels[1])).unwrap();
        assert!((*sys::igFindWindowByName(name.as_ptr())).DockTabIsVisible());
    });
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn corner_elimination_closes_every_target_tab_and_saves_named_dirty_documents() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let dirty_path = fixture
        ._directory
        .write("eliminated_dirty.rs", b"fn removed() {}\n");
    let clean_path = fixture
        ._directory
        .write("eliminated_clean.rs", b"fn clean() {}\n");
    let mut dirty_document = None;
    for path in [&dirty_path, &clean_path] {
        fixture.workbench.open_or_focus(path).unwrap();
        let panel = fixture.workbench.active_panel_id().unwrap();
        fixture.workbench.place_panel_in_area(panel, 1);
        if path == &dirty_path {
            dirty_document = fixture.workbench.active_document();
        }
    }
    let dirty_document = dirty_document.unwrap();
    // Closing must save this document through the normal preflight; background
    // autosave is paused so it cannot hide a missing save in the corner action.
    fixture
        .workbench
        .session
        .pause_autosave(dirty_document, true)
        .unwrap();
    let revision = fixture
        .workbench
        .session
        .document_revision(dirty_document)
        .unwrap();
    fixture
        .workbench
        .session
        .apply_edits(
            dirty_document,
            revision,
            &[bed_document_session::ByteEdit {
                range: 0..0,
                bytes: b"// unsaved changes\n".to_vec(),
            }],
        )
        .unwrap();
    fixture.settle();
    let removed_tabs = fixture.group(1).tabs.clone();
    assert_eq!(removed_tabs.len(), 3);
    let removed_documents = removed_tabs
        .iter()
        .map(|id| {
            fixture
                .workbench
                .tabs
                .iter()
                .find(|tab| tab.id == *id)
                .unwrap()
                .panel
                .document()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(
        fixture
            .workbench
            .session
            .snapshot(dirty_document)
            .unwrap()
            .dirty
    );
    assert_eq!(std::fs::read(&dirty_path).unwrap(), b"fn removed() {}\n");
    let source = fixture.panels[1];
    let source_view = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == source)
        .unwrap()
        .panel
        .instance
        .view_id()
        .unwrap();
    let before = fixture.workbench.tiling.clone();
    fixture.press_top_right_corner();
    fixture.drag([2500, 5000]);
    assert_eq!(fixture.workbench.tiling, before);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    assert_eq!(fixture.group(2).tabs, vec![source]);
    assert_eq!(fixture.group(2).selected, Some(source));
    for id in removed_tabs {
        assert_eq!(fixture.workbench.tiling.area_for(id), None);
        assert!(!fixture.workbench.tabs.iter().any(|tab| tab.id == id));
    }
    for document in removed_documents {
        assert!(!fixture.workbench.session.document_ids().contains(&document));
    }
    assert_eq!(
        std::fs::read(&dirty_path).unwrap(),
        b"// unsaved changes\nfn removed() {}\n"
    );
    assert_eq!(std::fs::read(&clean_path).unwrap(), b"fn clean() {}\n");
    assert_eq!(fixture.workbench.focused, Some(source));
    assert_eq!(fixture.workbench.active_view(), Some(source_view));
    assert!(fixture.workbench.error.is_none());
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn escape_cancels_a_corner_drag_after_switching_between_extension_and_placement() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.clone();
    fixture.press_top_right_corner();
    for point in [[4900, 2500], [2500, 5000], [4900, 2500]] {
        fixture.drag(point);
    }
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.release();
    assert_eq!(fixture.workbench.tiling, before);
    assert!(fixture.workbench.tiling_ui.actions.is_empty());
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn hovering_an_unmarked_corner_shows_the_custom_split_cursor() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.clone();
    for (position, offset) in [
        ([5000, 0], [4.0, 4.0]),
        ([5000, 5000], [3.0, -3.0]),
        ([0, 0], [10.0, 10.0]),
        ([0, 0], [16.0, 8.0]),
        ([0, 0], [8.0, 16.0]),
    ] {
        let point = fixture.screen(position);
        fixture
            .context
            .io_mut()
            .add_mouse_pos_event([point[0] + offset[0], point[1] + offset[1]]);
        fixture.frame();
        fixture.context.binding().with_bound_context(|| unsafe {
            assert_eq!(
                sys::igGetMouseCursor(),
                sys::ImGuiMouseCursor_None,
                "corners own the plus cursor, including visible outer corners and T junctions"
            );
        });
        assert_eq!(fixture.workbench.tiling, before);
        assert!(fixture.workbench.tiling_ui.actions.is_empty());
    }
    fixture.cleanup();
}

#[test]
fn one_area_fills_the_window_and_splitting_restores_gaps_and_rounded_corners() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.tiling = TilingState::default();
    fixture.workbench.pending_tiling = None;
    for panel in fixture.panels {
        fixture.workbench.place_panel_in_area(panel, 1);
    }
    fixture.workbench.reset_tiling_docks();
    let background = [0.14, 0.22, 0.36, 0.81];
    let border = [0.79, 0.17, 0.41, 0.83];
    fixture
        .context
        .style_mut()
        .set_color(StyleColor::WindowBg, background);
    fixture
        .context
        .style_mut()
        .set_color(StyleColor::Border, border);
    fixture.settle();
    let origin = fixture.workbench.tiling_ui.origin;
    let end = [0, 1].map(|axis| origin[axis] + fixture.workbench.tiling_ui.size[axis]);
    fixture.context.binding().with_bound_context(|| unsafe {
        let rect = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(
            fixture.workbench.tiling_ui.docks[&1],
        ));
        assert_eq!(rect.Min.x, origin[0]);
        assert!((rect.Min.y - fixture.workbench.tiling_ui.bar_rects[&1].1[1] - 1.0).abs() <= 1.0);
        assert_eq!([rect.Max.x, rect.Max.y], end);
        let window = &*sys::igFindWindowByName(c"##bed_workspace".as_ptr());
        let vertices = &(*window.DrawList).VtxBuffer;
        let packed = |color: [f32; 4]| {
            u32::from_le_bytes(color.map(|value| (value.clamp(0.0, 1.0) * 255.0) as u8))
        };
        let normal = packed(border);
        let focused_color = [
            border[0] + (1.0 - border[0]) * 0.2,
            border[1] + (1.0 - border[1]) * 0.2,
            border[2] + (1.0 - border[2]) * 0.2,
            border[3],
        ];
        let focused = packed(focused_color);
        let underline = fixture.workbench.tiling_ui.bar_rects[&1].1[1];
        assert!(
            (0..vertices.Size as usize).any(|index| {
                let vertex = &*vertices.Data.add(index);
                vertex.col == normal && (vertex.pos.y - underline).abs() <= 1.0
            }),
            "the theme border still separates tabs from content"
        );
        assert!(
            (0..vertices.Size as usize).all(|index| {
                let vertex = &*vertices.Data.add(index);
                let outer = (vertex.pos.x - origin[0]).abs() <= 2.0
                    || (vertex.pos.x - end[0]).abs() <= 2.0
                    || (vertex.pos.y - origin[1]).abs() <= 2.0
                    || (vertex.pos.y - end[1]).abs() <= 2.0;
                !outer
                    || (vertex.col != normal && vertex.col != focused)
                    || (vertex.pos.y - underline).abs() <= 2.0
            }),
            "the sole panel has no outer border, including the focused border"
        );
    });
    let has_background_corner = |fixture: &Fixture, point: [f32; 2]| {
        fixture.context.binding().with_bound_context(|| unsafe {
            let data = &*sys::igGetDrawData();
            (0..data.CmdLists.Size as usize).any(|list| {
                let vertices = &(**data.CmdLists.Data.add(list)).VtxBuffer;
                (0..vertices.Size as usize).any(|index| {
                    let vertex = &*vertices.Data.add(index);
                    (vertex.pos.x - point[0]).abs() < 0.01
                        && (vertex.pos.y - point[1]).abs() < 0.01
                        && (0..4).all(|channel| {
                            let emitted = ((vertex.col >> (channel * 8)) & 255) as f32 / 255.0;
                            (emitted - background[channel]).abs() <= 1.0 / 255.0
                        })
                })
            })
        })
    };
    for point in [origin, [end[0], origin[1]], end, [origin[0], end[1]]] {
        assert!(
            has_background_corner(&fixture, point),
            "the sole panel has a square background reaching every window corner"
        );
    }
    fixture.press_at([origin[0] + 8.0, origin[1] + 8.0]);
    fixture.drag([5000, 2500]);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    let boundary = fixture.screen([5000, 0])[0];
    fixture.context.binding().with_bound_context(|| unsafe {
        let left = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(
            fixture.workbench.tiling_ui.docks[&1],
        ));
        let right = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(
            fixture.workbench.tiling_ui.docks[&2],
        ));
        for (actual, expected) in [
            (left.Min.x, origin[0] + 7.0),
            (
                left.Min.y,
                fixture.workbench.tiling_ui.bar_rects[&1].1[1] + 1.0,
            ),
            (left.Max.x, boundary - 4.5),
            (right.Min.x, boundary + 4.5),
            (
                right.Min.y,
                fixture.workbench.tiling_ui.bar_rects[&2].1[1] + 1.0,
            ),
            (right.Max.x, end[0] - 7.0),
        ] {
            assert!(
                (actual - expected).abs() <= 1.0,
                "native docks align to pixels: {actual} vs {expected}"
            );
        }
    });
    assert!(!has_background_corner(&fixture, origin));
    assert!(
        !has_background_corner(&fixture, [origin[0] + 5.0, origin[1] + 5.0]),
        "multiple panels restore rounding at the inset surface corner"
    );
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn removing_a_sibling_animates_the_bar_dock_and_body_and_can_finish_immediately() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for disable_during_motion in [false, true] {
        let mut fixture = Fixture::t_layout();
        fixture.workbench.close_tab(2).unwrap();
        let mut layout = Layout::default();
        layout.split(1, 0, 5000, 1).unwrap();
        fixture.workbench.tiling = TilingState::new(layout);
        fixture.workbench.pending_tiling = None;
        for (panel, area) in fixture.panels[..2].iter().copied().zip([1, 2]) {
            fixture.workbench.place_panel_in_area(panel, area);
        }
        fixture.workbench.reset_tiling_docks();
        fixture.settle();
        let origin = fixture.workbench.tiling_ui.origin;
        let end = [0, 1].map(|axis| origin[axis] + fixture.workbench.tiling_ui.size[axis]);
        let initial_x = fixture.workbench.tiling_ui.bar_rects[&2].0[0];
        let aligned = |fixture: &Fixture, stage: &str| {
            let (bar_min, bar_max) = fixture.workbench.tiling_ui.bar_rects[&2];
            let selected = fixture.group(2).selected.unwrap();
            fixture.context.binding().with_bound_context(|| unsafe {
                let dock = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(
                    fixture.workbench.tiling_ui.docks[&2],
                ));
                let name = CString::new(format!("###bed_tab_{selected}")).unwrap();
                let body = &*sys::igFindWindowByName(name.as_ptr());
                assert!(
                    body.DockTabIsVisible(),
                    "selected body is visible during {stage}, frame={}, selected={selected}",
                    (*sys::igGetCurrentContext()).FrameCount,
                );
                for (actual, expected) in [
                    (dock.Min.x, bar_min[0]),
                    (dock.Min.y, bar_max[1] + 1.0),
                    (dock.Max.x, bar_max[0]),
                    (dock.Max.y, end[1]),
                    (body.Pos.x, dock.Min.x),
                    (body.Pos.y, dock.Min.y),
                    (body.Pos.x + body.Size.x, dock.Max.x),
                    (body.Pos.y + body.Size.y, dock.Max.y),
                ] {
                    assert!(
                        (actual - expected).abs() <= 1.0,
                        "rendered tabs, DockSpace and content move together: {actual} vs {expected}"
                    );
                }
            });
        };
        fixture.workbench.settings.settings["ui_animations"] = json!(true);
        fixture.press_tab(fixture.panels[0]);
        fixture.drag([7500, 5000]);
        fixture
            .context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        fixture.frame();
        fixture.frame();
        assert_eq!(fixture.workbench.tiling.layout.areas.len(), 1);
        assert_eq!(
            fixture.workbench.tiling.layout.area(2).rect,
            Rect {
                min: [0, 0],
                max: [EXTENT, EXTENT]
            }
        );
        // The moved native window binds to its destination during Begin. On
        // that commit frame the destination still shows its previous content;
        // selection takes effect when the next DockSpace update runs.
        fixture.frame();
        let mut previous_x = initial_x;
        for motion_frame in 0..4 {
            let x = fixture.workbench.tiling_ui.bar_rects[&2].0[0];
            assert!(
                x > origin[0] && x < previous_x,
                "the surviving panel expands over several rendered frames: {previous_x} -> {x}"
            );
            aligned(&fixture, &format!("expansion frame {motion_frame}"));
            let viewport = fixture.context.main_viewport().id().raw();
            let middle_y = (origin[1] + end[1]) * 0.5;
            assert_eq!(
                fixture
                    .workbench
                    .tiling_area_at([x + 10.0, middle_y], viewport),
                Some(2),
                "drops follow the displayed surviving panel"
            );
            assert_eq!(
                fixture
                    .workbench
                    .tiling_area_at([origin[0] + 1.0, middle_y], viewport),
                None,
                "the vacated space is not a drop target before the panel reaches it"
            );
            previous_x = x;
            fixture.frame();
        }
        if disable_during_motion {
            fixture.workbench.settings.settings["ui_animations"] = json!(false);
            fixture.frame();
            assert_eq!(fixture.workbench.tiling_ui.area_bounds[&2], (origin, end));
            aligned(&fixture, "animation disabled");
            fixture.workbench.settings.settings["ui_animations"] = json!(true);
            for _ in 0..4 {
                fixture.frame();
                assert_eq!(
                    fixture.workbench.tiling_ui.area_bounds[&2],
                    (origin, end),
                    "re-enabling animations does not replay a completed expansion"
                );
            }
        } else {
            let (min, max) = fixture.workbench.tiling_ui.tab_rects[&fixture.panels[1]];
            fixture
                .context
                .io_mut()
                .add_mouse_pos_event([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]);
            fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            fixture.frame();
            fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            fixture.frame();
            fixture.frame();
            assert_eq!(
                fixture.group(2).selected,
                Some(fixture.panels[1]),
                "a click on a displayed tab selects its content while the area is moving"
            );
            aligned(&fixture, "displayed tab clicked during expansion");
            for _ in 0..40 {
                fixture.frame();
            }
            assert_eq!(fixture.workbench.tiling_ui.area_bounds[&2], (origin, end));
            aligned(&fixture, "expansion settled");
        }
        assert_eq!(fixture.workbench.tiling_ui.bar_rects[&2].0, origin);
        assert_eq!(fixture.workbench.tabs.len(), 2);
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn an_inward_corner_split_creates_an_empty_area_without_cloning_document_views() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.tiling = TilingState::default();
    fixture.workbench.pending_tiling = None;
    for panel in fixture.panels {
        fixture.workbench.place_panel_in_area(panel, 1);
    }
    fixture.workbench.reset_tiling_docks();
    fixture.settle();
    let contents = fixture
        .workbench
        .tabs
        .iter()
        .map(|tab| {
            (
                tab.id,
                tab.panel.document().unwrap(),
                tab.panel.instance.view_id().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let selected = fixture.group(1).selected;
    let point = fixture.screen([0, 0]).map(|coordinate| coordinate + 4.0);
    fixture.press_at(point);
    fixture.drag([5000, 2500]);
    assert_eq!(
        fixture.workbench.tiling.layout.areas.len(),
        1,
        "the corner split is only a preview before release"
    );
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    assert_eq!(fixture.workbench.tabs.len(), contents.len());
    assert_eq!(fixture.group(1).selected, selected);
    assert_eq!(fixture.group(1).tabs, fixture.panels);
    assert!(fixture.group(2).tabs.is_empty());
    assert_eq!(fixture.group(2).selected, None);
    assert!(fixture.workbench.tiling_ui.bar_rects.contains_key(&2));
    for (panel, document, view) in contents {
        assert_eq!(fixture.workbench.tiling.area_for(panel), Some(1));
        assert_eq!(fixture.workbench.session.view_count(document), 1);
        let tab = fixture
            .workbench
            .tabs
            .iter()
            .find(|tab| tab.id == panel)
            .unwrap();
        assert_eq!(tab.panel.document(), Some(document));
        assert_eq!(tab.panel.instance.view_id(), Some(view));
        assert_eq!(
            fixture.workbench.session.snapshot(document).unwrap().bytes,
            b"fn main() {}\n"
        );
    }
    fixture.context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(c"###bed_empty_area_2".as_ptr())
            .as_ref()
            .unwrap();
        assert_eq!([window.ContentSize.x, window.ContentSize.y], [0.0, 0.0]);
    });
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn custom_tab_center_drop_moves_only_that_tab_and_selects_its_content() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let moved = fixture.panels[0];
    fixture.workbench.place_panel_in_area(moved, 3);
    fixture.settle();
    let before = fixture.workbench.tiling.layout.clone();
    fixture.press_tab(moved);
    fixture.drag([7500, 2500]);
    assert_eq!(
        fixture.workbench.tiling.layout, before,
        "a drag changes only its preview"
    );
    assert_eq!(fixture.workbench.tiling.area_for(moved), Some(3));
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout, before);
    assert_eq!(fixture.group(2).tabs, vec![fixture.panels[1], moved]);
    assert_eq!(fixture.group(2).selected, Some(moved));
    assert_eq!(fixture.group(3).tabs, vec![fixture.panels[2]]);
    assert_eq!(fixture.group(3).selected, Some(fixture.panels[2]));
    fixture.context.binding().with_bound_context(|| unsafe {
        for (panel, visible) in [
            (moved, true),
            (fixture.panels[1], false),
            (fixture.panels[2], true),
        ] {
            let name = CString::new(format!("###bed_tab_{panel}")).unwrap();
            assert_eq!(
                (*sys::igFindWindowByName(name.as_ptr())).DockTabIsVisible(),
                visible
            );
        }
    });
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn custom_tab_edge_drops_split_each_side_and_close_the_vacated_source_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for (point, expected) in [
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
        let mut fixture = Fixture::t_layout();
        let moved = fixture.panels[1];

        let before = fixture.workbench.tiling.clone();
        fixture.press_tab(moved);
        fixture.drag(point);
        assert_eq!(
            fixture.workbench.tiling, before,
            "edge previews do not mutate contents or geometry"
        );
        fixture.release();
        let destination = fixture.workbench.tiling.area_for(moved).unwrap();
        assert_ne!(destination, 1);
        assert_ne!(destination, 2);
        assert_eq!(fixture.workbench.tiling.layout.areas.len(), 3);
        assert_eq!(
            fixture.workbench.tiling.layout.area(destination).rect,
            expected
        );
        assert_eq!(fixture.group(destination).tabs, vec![moved]);
        assert_eq!(fixture.group(destination).selected, Some(moved));
        assert!(
            fixture
                .workbench
                .tiling
                .layout
                .areas
                .iter()
                .all(|area| area.id != 2),
            "moving the last tab removes its source area"
        );
        let vacancy = before.layout.area(2).rect;
        let covered = fixture
            .workbench
            .tiling
            .layout
            .areas
            .iter()
            .map(|area| {
                let width = (area.rect.max[0].min(vacancy.max[0])
                    - area.rect.min[0].max(vacancy.min[0]))
                .max(0);
                let height = (area.rect.max[1].min(vacancy.max[1])
                    - area.rect.min[1].max(vacancy.min[1]))
                .max(0);
                i64::from(width) * i64::from(height)
            })
            .sum::<i64>();
        assert_eq!(
            covered,
            i64::from(vacancy.max[0] - vacancy.min[0]) * i64::from(vacancy.max[1] - vacancy.min[1]),
            "surviving areas completely fill the vacated source's footprint"
        );
        assert_eq!(fixture.group(1).tabs, vec![fixture.panels[0]]);
        assert_eq!(fixture.group(3).tabs, vec![fixture.panels[2]]);
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn dragging_one_tab_to_its_own_edge_splits_without_duplicating_the_document() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let moved = fixture.panels[0];
    fixture.workbench.place_panel_in_area(moved, 2);
    fixture.settle();
    let count = fixture.workbench.tabs.len();
    let document = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == moved)
        .unwrap()
        .panel
        .document()
        .unwrap();
    fixture.press_tab(moved);
    fixture.drag([5500, 2500]);
    fixture.release();
    let destination = fixture.workbench.tiling.area_for(moved).unwrap();
    assert_ne!(destination, 2);
    assert_eq!(fixture.group(2).tabs, vec![fixture.panels[1]]);
    assert_eq!(fixture.group(2).selected, Some(fixture.panels[1]));
    assert_eq!(fixture.group(destination).tabs, vec![moved]);
    assert_eq!(fixture.workbench.tabs.len(), count);
    assert_eq!(fixture.workbench.session.view_count(document), 1);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn a_single_tab_can_split_its_own_area_without_duplicating_its_document() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    let moved = fixture.panels[1];
    let before = fixture.workbench.tiling.clone();
    let count = fixture.workbench.tabs.len();
    let document = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == moved)
        .unwrap()
        .panel
        .document()
        .unwrap();
    fixture.press_tab(moved);
    fixture.drag([5500, 2500]);
    assert_eq!(fixture.workbench.tiling, before);
    fixture.release();
    let destination = fixture.workbench.tiling.area_for(moved).unwrap();
    assert_ne!(destination, 2);
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 4);
    assert_eq!(
        fixture.workbench.tiling.layout.area(destination).rect,
        Rect {
            min: [5000, 0],
            max: [7500, 5000],
        }
    );
    assert_eq!(
        fixture.workbench.tiling.layout.area(2).rect,
        Rect {
            min: [7500, 0],
            max: [EXTENT, 5000],
        }
    );
    assert!(fixture.group(2).tabs.is_empty());
    assert_eq!(fixture.group(destination).tabs, vec![moved]);
    assert_eq!(fixture.workbench.tabs.len(), count);
    assert_eq!(fixture.workbench.session.view_count(document), 1);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn custom_tab_bar_drop_reorders_tabs_and_keeps_the_dragged_tab_selected() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    for panel in fixture.panels {
        fixture.workbench.place_panel_in_area(panel, 2);
    }
    fixture.settle();
    let before = fixture.workbench.tiling.layout.clone();
    let first = fixture.group(2).tabs[0];
    let moved = fixture.panels[2];
    let (min, max) = fixture.workbench.tiling_ui.tab_rects[&first];
    fixture.press_tab(moved);
    fixture
        .context
        .io_mut()
        .add_mouse_pos_event([min[0] + 2.0, (min[1] + max[1]) * 0.5]);
    fixture.frame();
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout, before);
    assert_eq!(fixture.group(2).tabs, vec![moved, first, fixture.panels[0]]);
    assert_eq!(fixture.group(2).selected, Some(moved));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn reordering_in_an_overflowing_bar_keeps_tabs_hidden_on_the_right_after_the_drop() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.place_panel_in_area(fixture.panels[1], 3);
    let mut panels = Vec::new();
    for index in 0..8 {
        let name = format!("long_name_for_tab_overflow_{index}_{}.rs", "a".repeat(40));
        let path = fixture._directory.write(&name, b"fn main() {}\n");
        fixture.workbench.open_or_focus(&path).unwrap();
        let panel = fixture.workbench.tabs.last().unwrap().id;
        fixture.workbench.place_panel_in_area(panel, 2);
        panels.push(panel);
    }
    let index = fixture
        .workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panels[0])
        .unwrap();
    fixture.workbench.switch_to_tab(index);
    fixture.settle();
    assert!(
        !fixture
            .workbench
            .tiling_ui
            .tab_rects
            .contains_key(panels.last().unwrap()),
        "the bar overflows before dragging"
    );
    let before = fixture.workbench.tiling.layout.clone();
    let (min, max) = fixture.workbench.tiling_ui.bar_rects[&2];
    fixture.press_tab(panels[0]);
    fixture
        .context
        .io_mut()
        .add_mouse_pos_event([max[0] - 4.0, (min[1] + max[1]) * 0.5]);
    fixture.frame();
    fixture.release();
    let mut expected = panels.clone();
    let moved = expected.remove(0);
    expected.insert(2, moved);
    assert_eq!(
        fixture.group(2).tabs,
        expected,
        "dropping near the visible right edge inserts among visible tabs, before the hidden remainder"
    );
    assert_eq!(fixture.group(2).selected, Some(moved));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn closing_a_tab_after_drop_release_discards_its_queued_move_or_split() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for point in [[2500, 5000], [1000, 5000]] {
        let mut fixture = Fixture::t_layout();
        let moved = fixture.panels[1];

        let before = fixture.workbench.tiling.layout.clone();
        fixture.press_tab(moved);
        fixture.drag(point);
        fixture
            .context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        fixture.frame();
        assert!(
            !fixture.workbench.tiling_ui.actions.is_empty(),
            "release queues placement for the next controlled update"
        );
        let index = fixture
            .workbench
            .tabs
            .iter()
            .position(|tab| tab.id == moved)
            .unwrap();
        fixture.workbench.close_tab(index).unwrap();
        fixture.settle();
        assert_eq!(fixture.workbench.tiling.area_for(moved), None);
        assert_eq!(fixture.workbench.tiling.layout, before);
        assert!(fixture.group(2).tabs.is_empty());
        assert!(fixture.workbench.tiling_ui.actions.is_empty());
        assert!(fixture.workbench.error.is_none());
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn escape_and_right_click_cancel_tab_placement_without_losing_contents() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for right_click in [false, true] {
        let mut fixture = Fixture::t_layout();

        let before = fixture.workbench.tiling.clone();
        fixture.press_tab(fixture.panels[1]);
        fixture.drag([1000, 5000]);
        fixture.drag([2500, 5000]);
        if right_click {
            fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Right, true);
        } else {
            fixture.context.io_mut().add_key_event(Key::Escape, true);
        }
        fixture.frame();
        if right_click {
            fixture
                .context
                .io_mut()
                .add_mouse_button_event(MouseButton::Right, false);
        } else {
            fixture.context.io_mut().add_key_event(Key::Escape, false);
        }
        fixture.release();
        assert_eq!(fixture.workbench.tiling, before);
        assert!(fixture.workbench.tiling_ui.actions.is_empty());
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn dropping_a_tab_outside_the_workspace_keeps_it_in_the_main_window() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    let before = fixture.workbench.tiling.clone();
    fixture.press_tab(fixture.panels[1]);
    fixture
        .context
        .io_mut()
        .add_mouse_pos_event([-100.0, -100.0]);
    fixture.frame();
    fixture.release();
    assert_eq!(fixture.workbench.tiling, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn closing_the_last_real_document_leaves_a_blank_droppable_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();

    let (min, max) = fixture.workbench.tiling_ui.tab_rects[&fixture.panels[1]];
    fixture.press_at([max[0] - 10.0, (min[1] + max[1]) * 0.5]);
    fixture.release();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.layout, before);
    let group = fixture
        .workbench
        .tiling
        .areas
        .iter()
        .find(|group| group.area == 2)
        .unwrap();
    assert!(group.tabs.is_empty());
    fixture.context.binding().with_bound_context(|| unsafe {
        let name = CString::new("###bed_empty_area_2").unwrap();
        let window = sys::igFindWindowByName(name.as_ptr()).as_ref().unwrap();
        assert_eq!(window.DockId, fixture.workbench.tiling_ui.docks[&2]);
        assert!(window.DockTabIsVisible());
        assert_eq!(window.ViewportId, (*sys::igGetMainViewport()).ID);
        assert_eq!(
            window.TitleBarHeight, 0.0,
            "the blank body has no native window title bar"
        );
        assert_eq!(
            window.ContentSize.x, 0.0,
            "an empty area contains no controls"
        );
        assert_eq!(window.ContentSize.y, 0.0, "an empty area contains no text");
    });
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn a_tab_close_button_near_the_right_corner_keeps_its_pointer_target() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    for panel in fixture.panels {
        fixture.workbench.place_panel_in_area(panel, 2);
    }
    for name in [
        "a_very_long_first_document_filename_for_tab_overflow.rs",
        "a_very_long_second_document_filename_for_tab_overflow.rs",
    ] {
        let path = fixture._directory.write(name, b"fn main() {}\n");
        fixture.workbench.open_or_focus(&path).unwrap();
        let panel = fixture.workbench.focused.unwrap();
        fixture.workbench.place_panel_in_area(panel, 2);
    }
    fixture.settle();
    let closed = fixture.workbench.focused.unwrap();
    let (min, max) = fixture.workbench.tiling_ui.tab_rects[&closed];
    let (_, bar_max) = fixture.workbench.tiling_ui.bar_rects[&2];
    assert!(
        (max[0] - bar_max[0]).abs() <= 1.0,
        "the last tab reaches the panel's right edge"
    );
    let before = fixture.workbench.tiling.layout.clone();
    fixture.press_at([max[0] - 10.0, (min[1] + max[1]) * 0.5]);
    fixture.release();
    assert!(!fixture.workbench.tabs.iter().any(|tab| tab.id == closed));
    assert_eq!(fixture.group(2).tabs.len(), 4);
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn menu_documents_and_tools_use_the_focused_smaller_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    fixture
        .workbench
        .dispatch_from_menu(WindowCommand::NewDocument)
        .unwrap();
    let document_panel = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(document_panel), Some(2));
    fixture
        .workbench
        .dispatch_from_menu(WindowCommand::NewSettings)
        .unwrap();
    let tool = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(tool), Some(2));
    assert_eq!(fixture.group(2).selected, Some(tool));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn ordinary_file_and_tool_creation_use_the_largest_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    let path = fixture
        ._directory
        .write("ordinary_file_open.rs", b"fn new() {}\n");
    fixture.workbench.open_or_focus(&path).unwrap();
    let document_panel = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(document_panel), Some(1));
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    fixture
        .workbench
        .dispatch(WindowCommand::NewSettings)
        .unwrap();
    let tool = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(tool), Some(1));
    assert_eq!(fixture.group(1).selected, Some(tool));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn caller_selected_placement_uses_last_previous_or_largest_area() {
    use bed_workbench_api::PanelTarget;

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for (target, expected) in [
        (PanelTarget::LastFocused, 3),
        (PanelTarget::PreviousFocused, 2),
        (PanelTarget::LargestArea, 1),
    ] {
        let mut fixture = Fixture::t_layout();
        fixture.workbench.switch_to_tab(1);
        fixture.settle();
        fixture.workbench.switch_to_tab(2);
        fixture.settle();
        fixture
            .workbench
            .dispatch_with_target(WindowCommand::NewSettings, target)
            .unwrap();
        let created = fixture.workbench.focused.unwrap();
        fixture.settle();
        assert_eq!(
            fixture.workbench.tiling.area_for(created),
            Some(expected),
            "{target:?}"
        );
        fixture.assert_native_membership();
        fixture.cleanup();
    }
}

#[test]
fn previous_focus_tracks_distinct_tabs_in_one_area_and_follows_their_moves() {
    use bed_workbench_api::PanelTarget;

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.place_panel_in_area(fixture.panels[0], 2);
    fixture.workbench.switch_to_tab(2);
    fixture.settle();
    fixture.workbench.switch_to_tab(0);
    fixture.settle();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    assert_eq!(fixture.workbench.focused_area(), Some(2));
    assert_eq!(
        fixture.workbench.previous_focused_area(),
        Some(2),
        "two distinct tabs in the same area remain the last two focused contents"
    );
    fixture.workbench.place_panel_in_area(fixture.panels[0], 1);
    fixture.settle();
    assert_eq!(fixture.workbench.focused_area(), Some(2));
    assert_eq!(
        fixture.workbench.previous_focused_area(),
        Some(1),
        "history resolves the previous tab's current area after it moves"
    );
    fixture
        .workbench
        .dispatch_with_target(WindowCommand::NewSettings, PanelTarget::PreviousFocused)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(1));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn a_missing_previous_focus_uses_the_largest_area_instead_of_the_last_area() {
    use bed_workbench_api::PanelTarget;

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.focused = None;
    fixture.workbench.active = None;
    fixture.workbench.focus_history = [None; 2];
    fixture.workbench.switch_to_tab(1);
    assert_eq!(fixture.workbench.focused_area(), Some(2));
    assert_eq!(fixture.workbench.previous_focused_area(), None);
    fixture
        .workbench
        .dispatch_with_target(WindowCommand::NewSettings, PanelTarget::PreviousFocused)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(1));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn remapping_preserves_both_focused_contents_and_closing_the_previous_tab_forgets_it() {
    use bed_workbench_api::PanelTarget;

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    fixture.workbench.switch_to_tab(2);
    fixture.settle();
    let previous = fixture.panels[1] + 100;
    let current = fixture.panels[2] + 100;
    fixture
        .workbench
        .replace_panel_id(fixture.panels[1], previous);
    fixture
        .workbench
        .replace_panel_id(fixture.panels[2], current);
    fixture.settle();
    assert_eq!(fixture.workbench.focused, Some(current));
    assert_eq!(fixture.workbench.focused_area(), Some(3));
    assert_eq!(fixture.workbench.previous_focused_area(), Some(2));
    fixture.workbench.place_panel_in_area(previous, 1);
    fixture.settle();
    assert_eq!(
        fixture.workbench.previous_focused_area(),
        Some(1),
        "the remapped previous entry still follows the same content"
    );
    let index = fixture
        .workbench
        .tabs
        .iter()
        .position(|tab| tab.id == previous)
        .unwrap();
    fixture.workbench.close_tab(index).unwrap();
    fixture.settle();
    assert_eq!(
        fixture.workbench.focused,
        Some(current),
        "closing the previous content retains current focus"
    );
    assert_eq!(fixture.workbench.previous_focused_area(), None);
    fixture
        .workbench
        .dispatch_with_target(WindowCommand::NewSettings, PanelTarget::PreviousFocused)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(
        fixture.workbench.tiling.area_for(created),
        Some(1),
        "a removed previous entry uses the largest-area fallback"
    );
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn a_menu_command_does_not_redirect_an_already_queued_panel_request() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    fixture
        .workbench
        .modules
        .requests
        .push(bed_workbench_api::HostRequest::OpenPanel {
            panel_type: bed_module_settings::PANEL_ID.to_owned(),
            document: None,
            state: json!({}),
        });
    assert!(
        fixture
            .workbench
            .dispatch_command_from_menu(bed_module_settings::NEW_COMMAND)
            .unwrap()
    );
    fixture.settle();
    let created = fixture
        .workbench
        .tabs
        .iter()
        .filter(|tab| !fixture.panels.contains(&tab.id))
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 2);
    assert_eq!(
        fixture.workbench.tiling.area_for(created[0]),
        Some(1),
        "the queued module request keeps ordinary placement"
    );
    assert_eq!(
        fixture.workbench.tiling.area_for(created[1]),
        Some(2),
        "the menu command creates in the area selected when it was invoked"
    );
    assert_eq!(fixture.workbench.focused, Some(created[1]));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn direct_menu_host_requests_capture_the_area_and_keep_existing_contents_in_place() {
    use bed_workbench_api::{HostRequest, PanelTarget};

    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.switch_to_tab(2);
    fixture.settle();
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    let ordinary_path = fixture
        ._directory
        .write("ordinary_host.rs", b"fn ordinary() {}\n");
    let menu_path = fixture._directory.write("menu_host.rs", b"fn menu() {}\n");
    let requests = [
        HostRequest::OpenFile {
            path: ordinary_path.to_string_lossy().into_owned(),
            viewer: None,
        },
        HostRequest::OpenFileIn {
            target: PanelTarget::LastFocused,
            path: menu_path.to_string_lossy().into_owned(),
            viewer: None,
        },
        HostRequest::ShowPanel {
            panel_type: bed_module_settings::PANEL_ID.to_owned(),
            document: None,
            state: json!({}),
            action: None,
        },
        HostRequest::ShowPanelIn {
            target: PanelTarget::PreviousFocused,
            panel_type: bed_module_editor::LSP_DASHBOARD_PANEL_TYPE.to_owned(),
            document: None,
            state: json!({}),
            action: None,
        },
    ];
    fixture.workbench.modules.requests.extend(requests);
    fixture.workbench.process_plugin_requests().unwrap();
    fixture.settle();
    let created = fixture.workbench.tabs[3..]
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 4);
    for (panel, area) in created.iter().copied().zip([1, 2, 1, 3]) {
        assert_eq!(fixture.workbench.tiling.area_for(panel), Some(area));
    }
    assert_eq!(fixture.workbench.focused, Some(created[3]));

    fixture.workbench.switch_to_tab(2);
    fixture.settle();
    fixture.workbench.modules.requests.extend([
        HostRequest::OpenFileIn {
            target: PanelTarget::LastFocused,
            path: menu_path.to_string_lossy().into_owned(),
            viewer: None,
        },
        HostRequest::ShowPanelIn {
            target: PanelTarget::LastFocused,
            panel_type: bed_module_settings::PANEL_ID.to_owned(),
            document: None,
            state: json!({}),
            action: None,
        },
        HostRequest::ShowPanelIn {
            target: PanelTarget::LastFocused,
            panel_type: bed_module_editor::LSP_DASHBOARD_PANEL_TYPE.to_owned(),
            document: None,
            state: json!({}),
            action: None,
        },
    ]);
    fixture.workbench.process_plugin_requests().unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tabs.len(), 7);
    for (panel, area) in created.iter().copied().zip([1, 2, 1, 3]) {
        assert_eq!(
            fixture.workbench.tiling.area_for(panel),
            Some(area),
            "menu reopen focuses existing contents without moving them"
        );
    }
    assert_eq!(fixture.workbench.focused, Some(created[3]));
    assert_eq!(fixture.group(3).selected, Some(created[3]));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn closing_all_content_keeps_the_last_focused_area_as_the_menu_tab_destination() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let last = fixture.panels[1];
    while fixture.workbench.tabs.len() > 1 {
        let index = fixture
            .workbench
            .tabs
            .iter()
            .position(|tab| tab.id != last)
            .unwrap();
        fixture.workbench.close_tab(index).unwrap();
    }
    fixture.workbench.switch_to_tab(0);
    fixture.settle();
    assert_eq!(fixture.workbench.focused, Some(last));
    fixture.workbench.close_tab(0).unwrap();
    fixture.settle();
    assert!(fixture.workbench.focused.is_none());
    assert!(fixture.workbench.active_view().is_none());
    fixture
        .workbench
        .dispatch_from_menu(WindowCommand::NewDocument)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(2));
    assert!(fixture.group(1).tabs.is_empty());
    assert!(fixture.group(3).tabs.is_empty());
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn closing_the_focused_areas_last_tab_keeps_focus_and_menu_tabs_in_that_blank_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    let (min, max) = fixture.workbench.tiling_ui.tab_rects[&fixture.panels[1]];
    fixture.press_at([max[0] - 10.0, (min[1] + max[1]) * 0.5]);
    fixture.release();
    assert!(fixture.group(2).tabs.is_empty());
    assert_eq!(
        fixture.workbench.tabs.len(),
        2,
        "the other areas remain occupied"
    );
    assert!(fixture.workbench.focused.is_none());
    assert!(fixture.workbench.active_view().is_none());
    assert_eq!(fixture.workbench.focused_area(), Some(2));
    fixture
        .workbench
        .dispatch_from_menu(WindowCommand::NewDocument)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(2));
    for (panel, area) in [(fixture.panels[0], 1), (fixture.panels[2], 3)] {
        assert_eq!(fixture.workbench.tiling.area_for(panel), Some(area));
    }
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn a_workspace_without_focus_history_places_menu_tabs_in_the_largest_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let border = fixture.screen([5000, 7500]);
    fixture.press_at(border);
    fixture.drag([2500, 7500]);
    fixture.release();
    assert!(
        fixture.workbench.tiling.layout.area(1).rect.max[0]
            < fixture.workbench.tiling.layout.area(2).rect.max[0]
                - fixture.workbench.tiling.layout.area(2).rect.min[0],
        "the first area is smaller than the others"
    );
    // Exercise the valid initial state before any area has acquired focus.
    fixture.workbench.focused = None;
    fixture.workbench.active = None;
    fixture.workbench.focus_history = [None; 2];
    fixture
        .workbench
        .dispatch_from_menu(WindowCommand::NewDocument)
        .unwrap();
    let created = fixture.workbench.focused.unwrap();
    fixture.settle();
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(2));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn only_the_focused_area_has_the_lighter_theme_border() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let normal = [0.21, 0.17, 0.29, 0.8];
    let lighter = [0.368, 0.336, 0.432, 0.8];
    fixture
        .context
        .style_mut()
        .set_color(StyleColor::Border, normal);
    fixture.workbench.switch_to_tab(1);
    fixture.settle();
    for (area, color) in [(1, normal), (2, lighter), (3, normal)] {
        let rect = fixture.workbench.tiling.layout.area(area).rect;
        let point = fixture.screen(rect.min);
        let min = [0, 1].map(|axis| point[axis] + if rect.min[axis] == 0 { 5.0 } else { 2.5 });
        fixture.context.binding().with_bound_context(|| unsafe {
            let data = &*sys::igGetDrawData();
            let found = (0..data.CmdLists.Size as usize).any(|index| {
                let vertices = &(**data.CmdLists.Data.add(index)).VtxBuffer;
                (0..vertices.Size as usize).any(|index| {
                    let vertex = &*vertices.Data.add(index);
                    (vertex.pos.x - min[0]).abs() <= 1.5
                        && (vertex.pos.y - min[1] - 5.0).abs() <= 4.0
                        && (0..4).all(|channel| {
                            let emitted = ((vertex.col >> (channel * 8)) & 255) as f32 / 255.0;
                            (emitted - color[channel]).abs() <= 1.0 / 255.0
                        })
                })
            });
            assert!(
                found,
                "area {area} has the expected themed border: {color:?}"
            );
        });
    }
    fixture.cleanup();
}

#[test]
fn a_custom_tab_can_be_dropped_into_a_blank_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let moved = fixture.panels[0];
    let closed = fixture
        .workbench
        .tabs
        .iter()
        .position(|tab| tab.id == fixture.panels[1])
        .unwrap();
    fixture.workbench.close_tab(closed).unwrap();

    fixture.press_tab(moved);
    fixture.drag([7500, 2500]);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    assert_eq!(fixture.group(2).tabs, vec![moved]);
    assert_eq!(fixture.group(2).selected, Some(moved));
    assert!(
        fixture
            .workbench
            .tiling
            .layout
            .areas
            .iter()
            .all(|area| area.id != 1)
    );
    assert_eq!(fixture.workbench.focused, Some(moved));
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn tab_bars_stay_visible_for_single_multiple_and_empty_contents() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();
    let visible_areas = |fixture: &Fixture| {
        let mut areas = fixture
            .workbench
            .tiling_ui
            .bar_rects
            .keys()
            .copied()
            .collect::<Vec<_>>();
        areas.sort_unstable();
        areas
    };
    assert_eq!(visible_areas(&fixture), vec![1, 2, 3]);
    assert_eq!(fixture.workbench.tiling_ui.tab_rects.len(), 3);
    fixture.workbench.close_tab(1).unwrap();
    fixture.settle();
    assert_eq!(visible_areas(&fixture), vec![1, 2, 3]);
    assert!(fixture.group(2).tabs.is_empty());
    assert!(
        !fixture
            .workbench
            .tiling_ui
            .tab_rects
            .contains_key(&fixture.panels[1])
    );
    let (_, bar_max) = fixture.workbench.tiling_ui.bar_rects[&2];
    fixture.context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(c"###bed_empty_area_2".as_ptr())
            .as_ref()
            .unwrap();
        assert!(window.Pos.y >= bar_max[1]);
        assert_eq!(
            [window.ContentSize.x, window.ContentSize.y],
            [0.0, 0.0],
            "the empty panel's body contains no text or controls below its persistent tab bar"
        );
    });
    fixture.workbench.place_panel_in_area(fixture.panels[0], 3);
    fixture.settle();
    assert_eq!(visible_areas(&fixture), vec![1, 2, 3]);
    assert_eq!(fixture.group(3).tabs.len(), 2);
    for panel in [fixture.panels[0], fixture.panels[2]] {
        assert!(fixture.workbench.tiling_ui.tab_rects.contains_key(&panel));
    }
    fixture.press_tab(fixture.panels[2]);
    fixture.release();
    assert_eq!(fixture.group(3).selected, Some(fixture.panels[2]));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn command_t_has_no_special_tab_bar_behavior_or_key_ownership() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();
    let bars = fixture.workbench.tiling_ui.bar_rects.clone();
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    fixture.context.io_mut().add_key_event(modifier, true);
    fixture.context.io_mut().add_key_event(Key::T, true);
    fixture
        .context
        .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
    let ui = fixture.context.frame();
    assert!(ui.is_key_pressed_with_repeat(Key::T, false));
    fixture.workbench.render(ui).unwrap();
    assert!(
        ui.is_key_pressed_with_repeat(Key::T, false),
        "the removed tab-bar shortcut must leave the key available to panel input"
    );
    drop(fixture.context.render_legacy());
    fixture.context.io_mut().add_key_event(Key::T, false);
    fixture.context.io_mut().add_key_event(modifier, false);
    fixture.settle();
    // Panel keybindings may still create content; the workspace no longer
    // owns this chord or changes its layout and persistent tab bars.
    assert_eq!(fixture.workbench.tiling.layout, before);
    assert_eq!(fixture.workbench.tiling_ui.bar_rects, bars);
    fixture.cleanup();
}

#[test]
fn the_tab_bar_add_button_opens_a_picker_and_places_its_window_in_that_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    let before = fixture.workbench.tiling.layout.clone();
    let count = fixture.workbench.tabs.len();
    let (min, max) = fixture.workbench.tiling_ui.bar_rects[&2];
    let height = max[1] - min[1];
    fixture.press_at([min[0] + height * 0.5, min[1] + height * 0.5]);
    fixture.release();
    let first_item = fixture.context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(
            native.OpenPopupStack.Size, 1,
            "the + button opens the window picker"
        );
        let popup = &*(*native.OpenPopupStack.Data).Window;
        assert!(!popup.Hidden);
        let height = popup.DC.PrevLineSize.y;
        [
            popup.DC.CursorStartPos.x + height,
            popup.DC.CursorStartPos.y + height * 0.5,
        ]
    });
    fixture.press_at(first_item);
    fixture.release();
    assert_eq!(fixture.workbench.tabs.len(), count + 1);
    let created = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| !fixture.panels.contains(&tab.id))
        .expect("choosing a window creates a panel")
        .id;
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(2));
    assert_eq!(fixture.group(2).selected, Some(created));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn right_clicking_a_blank_area_uses_the_same_window_picker_and_fills_that_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    let popup = |fixture: &Fixture| {
        fixture.context.binding().with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            assert_eq!(native.OpenPopupStack.Size, 1);
            let popup = &*(*native.OpenPopupStack.Data).Window;
            assert!(!popup.Hidden);
            let height = popup.DC.PrevLineSize.y;
            (
                [
                    popup.DC.CursorStartPos.x + height,
                    popup.DC.CursorStartPos.y + height * 0.5,
                ],
                [popup.Size.x, popup.Size.y],
                [popup.WindowPadding.x, popup.WindowPadding.y],
            )
        })
    };
    let (min, max) = fixture.workbench.tiling_ui.bar_rects[&2];
    let height = max[1] - min[1];
    fixture.press_at([min[0] + height * 0.5, min[1] + height * 0.5]);
    fixture.release();
    let (first_item, size, padding) = popup(&fixture);
    assert_eq!(padding, [8.0, 6.0]);
    fixture.press_at(first_item);
    fixture.release();
    let plus_created = fixture.workbench.focused.unwrap();
    let first_kind = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == plus_created)
        .unwrap()
        .panel
        .kind
        .clone();
    assert_eq!(fixture.workbench.tiling.area_for(plus_created), Some(2));
    let index = fixture
        .workbench
        .tabs
        .iter()
        .position(|tab| tab.id == fixture.panels[0])
        .unwrap();
    fixture.workbench.close_tab(index).unwrap();
    fixture.settle();
    assert!(fixture.group(1).tabs.is_empty());
    let before = fixture.workbench.tiling.layout.clone();
    let count = fixture.workbench.tabs.len();
    let point = fixture.screen([2500, 5000]);
    fixture.context.io_mut().add_mouse_pos_event(point);
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
    fixture.settle();
    let (first_item, blank_size, blank_padding) = popup(&fixture);
    assert_eq!(blank_padding, padding);
    assert_eq!(blank_size[0], size[0]);
    assert!(
        blank_size[1] > size[1],
        "the blank area's picker includes a Close Panel action below the same window list"
    );
    assert!(fixture.workbench.focused.is_none());
    assert!(fixture.workbench.active_view().is_none());
    fixture.press_at(first_item);
    fixture.release();
    let created = fixture.workbench.focused.unwrap();
    let tab = fixture
        .workbench
        .tabs
        .iter()
        .find(|tab| tab.id == created)
        .unwrap();
    assert_eq!(
        tab.panel.kind, first_kind,
        "the first picker entry creates the same window type"
    );
    assert_eq!(fixture.workbench.tabs.len(), count + 1);
    assert_eq!(fixture.workbench.tiling.area_for(created), Some(1));
    assert_eq!(fixture.group(1).selected, Some(created));
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn the_empty_area_picker_closes_its_area_and_disables_closing_the_final_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    fixture.workbench.close_tab(1).unwrap();
    fixture.settle();
    let remaining = fixture
        .workbench
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<Vec<_>>();
    let point = fixture.screen([7500, 2500]);
    fixture.context.io_mut().add_mouse_pos_event(point);
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
    fixture.settle();
    let close_item = fixture.context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1);
        let popup = &*(*native.OpenPopupStack.Data).Window;
        [
            popup.DC.CursorStartPos.x + popup.DC.PrevLineSize.y,
            popup.DC.CursorPosPrevLine.y + popup.DC.PrevLineSize.y * 0.5,
        ]
    });
    fixture.press_at(close_item);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout.areas.len(), 2);
    assert!(
        fixture
            .workbench
            .tiling
            .layout
            .areas
            .iter()
            .all(|area| area.id != 2)
    );
    assert_eq!(
        fixture
            .workbench
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        remaining
    );
    fixture.assert_native_membership();

    while !fixture.workbench.tabs.is_empty() {
        fixture.workbench.close_tab(0).unwrap();
    }
    fixture.workbench.tiling = TilingState::new(Layout::default());
    fixture.workbench.pending_tiling = None;
    fixture.workbench.reset_tiling_docks();
    fixture.settle();
    let point = fixture.screen([5000, 5000]);
    fixture.context.io_mut().add_mouse_pos_event(point);
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
    fixture.settle();
    let close_item = fixture.context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1);
        let popup = &*(*native.OpenPopupStack.Data).Window;
        [
            popup.DC.CursorStartPos.x + popup.DC.PrevLineSize.y,
            popup.DC.CursorPosPrevLine.y + popup.DC.PrevLineSize.y * 0.5,
        ]
    });
    fixture.press_at(close_item);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout, Layout::default());
    assert!(fixture.workbench.tabs.is_empty());
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.settle();
    fixture.cleanup();
}

#[test]
fn placement_previews_use_theme_derived_translucent_fill_and_a_bright_outline() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    fixture.press_tab(fixture.panels[1]);
    for (legacy_fill, border, text) in [
        (
            [0.87, 0.13, 0.42, 0.62],
            [0.19, 0.83, 0.25, 0.8],
            [0.94, 0.91, 0.86, 1.0],
        ),
        (
            [0.29, 0.91, 0.63, 0.73],
            [0.76, 0.22, 0.94, 1.0],
            [0.82, 0.88, 0.97, 0.92],
        ),
    ] {
        fixture
            .context
            .style_mut()
            .set_color(StyleColor::DockingPreview, legacy_fill);
        fixture
            .context
            .style_mut()
            .set_color(StyleColor::Border, border);
        fixture
            .context
            .style_mut()
            .set_color(StyleColor::Text, text);
        let outline =
            [0, 1, 2, 3].map(|channel| border[channel] + (text[channel] - border[channel]) * 0.75);
        let fill = [outline[0], outline[1], outline[2], outline[3] * 0.14];
        let old_target = [0.08, 0.38, 0.97, 0.91];
        fixture
            .context
            .style_mut()
            .set_color(StyleColor::DragDropTarget, old_target);
        fixture.drag([1000, 5000]);
        let (min, max) = fixture.workbench.tiling_ui.preview_bounds.unwrap();
        let sample = [0, 1].map(|axis| min[axis] + (max[axis] - min[axis]) * [0.37, 0.43][axis]);
        fixture.context.binding().with_bound_context(|| unsafe {
            let draw = &*sys::igGetForegroundDrawList_ViewportPtr(sys::igGetMainViewport());
            let vertices = &draw.VtxBuffer;
            let matches_color = |packed: u32, color: [f32; 4]| {
                (0..4).all(|channel| {
                    let emitted = ((packed >> (channel * 8)) & 255) as f32 / 255.0;
                    (emitted - color[channel]).abs() <= 1.0 / 255.0
                })
            };
            let has_color = |color: [f32; 4]| {
                (0..vertices.Size as usize)
                    .any(|index| matches_color((*vertices.Data.add(index)).col, color))
            };
            assert!(
                has_color(outline),
                "the placement outline brightens the current theme's Border toward Text"
            );
            assert!(
                has_color(fill),
                "placement uses a translucent fill derived from its outline"
            );
            assert!(
                !has_color(legacy_fill),
                "the old docking palette does not determine the fill"
            );
            assert!(
                !has_color(old_target),
                "the old drag/drop palette does not determine the outline"
            );
            assert!(
                (0..vertices.Size as usize).any(|index| {
                    let vertex = &*vertices.Data.add(index);
                    matches_color(vertex.col, outline)
                        && (vertex.pos.x - min[0]).abs() <= 2.0
                        && vertex.pos.y <= min[1] + 10.0
                }),
                "the bright theme-derived color is drawn at the primary preview outline"
            );
            assert!(
                (0..vertices.Size as usize).all(|index| {
                    let vertex = &*vertices.Data.add(index);
                    !matches_color(vertex.col, outline)
                        || (vertex.pos.x >= min[0] - 2.0
                            && vertex.pos.x <= max[0] + 2.0
                            && vertex.pos.y >= min[1] - 2.0
                            && vertex.pos.y <= max[1] + 2.0)
                }),
                "only the placement indicator has a bright border; other panels keep their normal borders"
            );
            let cross = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
            };
            let indices = &draw.IdxBuffer;
            let covered = (0..indices.Size as usize / 3).any(|triangle| {
                let triangle_vertices = [0, 1, 2].map(|offset| {
                    let index = *indices.Data.add(triangle * 3 + offset) as usize;
                    *vertices.Data.add(index)
                });
                if triangle_vertices
                    .iter()
                    .any(|vertex| !matches_color(vertex.col, fill))
                {
                    return false;
                }
                let points = triangle_vertices.map(|vertex| [vertex.pos.x, vertex.pos.y]);
                if cross(points[0], points[1], points[2]).abs() < 0.001 {
                    return false;
                }
                let sides =
                    [0, 1, 2].map(|side| cross(points[side], points[(side + 1) % 3], sample));
                sides.iter().all(|side| *side >= 0.0) || sides.iter().all(|side| *side <= 0.0)
            });
            assert!(
                covered,
                "the target's interior is visibly highlighted with a low-alpha fill"
            );
            assert!(fill[3] > 0.0 && fill[3] < 0.2);
        });
    }
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.release();
    fixture.cleanup();
}

#[test]
fn placement_outline_moves_between_targets_and_resets_on_cancel() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();

    fixture.workbench.settings.settings["ui_animations"] = json!(true);
    let before = fixture.workbench.tiling.clone();
    fixture.press_tab(fixture.panels[1]);
    fixture.drag([1000, 5000]);
    let first = fixture.workbench.tiling_ui.preview_bounds.unwrap();
    fixture.drag([5500, 7500]);
    let moving = fixture.workbench.tiling_ui.preview_bounds.unwrap();
    fixture.workbench.settings.settings["ui_animations"] = json!(false);
    fixture.frame();
    let second = fixture.workbench.tiling_ui.preview_bounds.unwrap();
    let mut changed = 0;
    for (initial, intermediate, target) in [
        (first.0[0], moving.0[0], second.0[0]),
        (first.0[1], moving.0[1], second.0[1]),
        (first.1[0], moving.1[0], second.1[0]),
        (first.1[1], moving.1[1], second.1[1]),
    ] {
        if initial == target {
            assert_eq!(intermediate, initial, "unchanged edges remain stationary");
        } else {
            changed += 1;
            assert!(
                intermediate > initial.min(target) && intermediate < initial.max(target),
                "a target change advances the outline without snapping: {initial} -> {intermediate} -> {target}"
            );
        }
    }
    assert!(changed > 0, "the two targets require visible motion");
    fixture.frame();
    assert_eq!(fixture.workbench.tiling_ui.preview_bounds, Some(second));
    fixture.drag([1000, 5000]);
    assert_eq!(
        fixture.workbench.tiling_ui.preview_bounds,
        Some(first),
        "disabling animations places the outline directly at the target"
    );
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    assert!(fixture.workbench.tiling_ui.preview_bounds.is_none());
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.release();
    assert_eq!(fixture.workbench.tiling, before);
    fixture.workbench.settings.settings["ui_animations"] = json!(true);
    fixture.press_tab(fixture.panels[1]);
    fixture.drag([5500, 7500]);
    assert_eq!(
        fixture.workbench.tiling_ui.preview_bounds,
        Some(second),
        "a new drag starts at its own target without stale animation"
    );
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.release();
    fixture.cleanup();
}

#[test]
fn shared_border_resize_uses_a_snapshot_and_escape_restores_it() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let before = fixture.workbench.tiling.layout.clone();
    let point = fixture.screen([5000, 7500]);
    fixture.context.io_mut().add_mouse_pos_event(point);
    fixture.frame();
    fixture
        .context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    fixture.frame();
    fixture.drag([6000, 7500]);
    assert_eq!(fixture.workbench.tiling.layout.area(1).rect.max[0], 6000);
    assert_eq!(fixture.workbench.tiling.layout.area(2).rect.min[0], 6000);
    assert_eq!(fixture.workbench.tiling.layout.area(3).rect.min[0], 6000);
    fixture.context.io_mut().add_key_event(Key::Escape, true);
    fixture.frame();
    fixture.context.io_mut().add_key_event(Key::Escape, false);
    fixture.release();
    assert_eq!(fixture.workbench.tiling.layout, before);
    fixture.cleanup();
}

#[test]
fn file_drop_hit_testing_before_the_first_frame_needs_no_native_context() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let workbench = workspace(&directory);
    assert_eq!(workbench.tiling_area_at([100.0, 100.0], 0), None);
}

#[test]
fn an_outside_drop_from_a_group_keeps_membership_until_the_tab_returns() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut fixture = Fixture::t_layout();
    let floated = fixture.panels[0];
    fixture.workbench.place_panel_in_area(floated, 2);
    fixture.settle();
    let expected = fixture
        .workbench
        .tiling
        .areas
        .iter()
        .find(|group| group.area == 2)
        .unwrap()
        .tabs
        .clone();
    fixture.context.binding().with_bound_context(|| unsafe {
        let node = sys::igDockBuilderAddNode(0, 0);
        sys::igDockBuilderSetNodePos(node, [1300.0, 100.0].into());
        sys::igDockBuilderSetNodeSize(node, [500.0, 300.0].into());
        let name = CString::new(format!("###bed_tab_{floated}")).unwrap();
        sys::igDockBuilderDockWindow(name.as_ptr(), node);
        sys::igDockBuilderFinish(node);
    });
    for _ in 0..6 {
        fixture.frame();
        assert_eq!(
            fixture.workbench.tiling.area_for(floated),
            Some(2),
            "an unsettled outside tab still owns its previous area membership"
        );
        let group = fixture
            .workbench
            .tiling
            .areas
            .iter()
            .find(|group| group.area == 2)
            .unwrap();
        assert_eq!(group.tabs, expected);
    }
    fixture.assert_native_membership();
    fixture.cleanup();
}

#[test]
fn dragging_a_real_files_row_into_an_empty_area_opens_the_document_there() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let path = directory
        .write("project/dragged.rs", b"fn dragged() {}\n")
        .canonicalize()
        .unwrap();
    let mut workbench = workspace(&directory);
    workbench.set_project(&directory.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    workbench.split_area(1, 0, 3000).unwrap();
    // Discovery delivers listings through this boundary. Supply the real file
    // here so native watcher startup does not control a pointer-gesture test.
    let root = path.parent().unwrap().to_str().unwrap();
    workbench.file_explorer().file_tree.apply_directory(
        root,
        vec![bed_remote::DirectoryEntry {
            path: path.to_string_lossy().into_owned(),
            name: "dragged.rs".to_owned(),
            is_directory: false,
            is_symlink: false,
            is_gitignored: false,
        }],
    );
    let mut context = Context::create();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    workbench.file_explorer().file_tree.root_node.is_open = true;
    for _ in 0..20 {
        frame(&mut context, &mut workbench);
    }
    let files = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    let files_id = files.id;
    let title = workbench.title(files);
    let files_area = workbench.tiling.area_for(files_id).unwrap();
    let empty = workbench
        .tiling
        .areas
        .iter()
        .find(|group| group.tabs.is_empty())
        .unwrap()
        .area;
    // Re-enter this native window only to read its existing public row bounds.
    // The drag itself goes through the real Files panel and its BED_FILES source.
    context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [1200.0, 800.0],
        1.0 / 60.0,
    ));
    let ui = context.frame();
    workbench.render(ui).unwrap();
    let mut rows = Vec::new();
    ui.window(&title).build(|| {
        rows = workbench.file_explorer().file_tree.drop_targets(ui);
    });
    drop(context.render_legacy());
    assert_eq!(
        rows.len(),
        2,
        "the project row and its single file are visible"
    );
    let source = [
        (rows[1].min[0] + rows[1].max[0]) * 0.5,
        (rows[1].min[1] + rows[1].max[1]) * 0.5,
    ];
    context.io_mut().add_mouse_pos_event(source);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_pos_event([source[0] + 20.0, source[1]]);
    frame(&mut context, &mut workbench);
    assert_eq!(
        workbench.file_explorer().file_tree.dragged_paths(),
        Some([path.to_string_lossy().into_owned()].as_slice())
    );
    let rect = workbench.tiling.layout.area(empty).rect;
    let target = [0, 1].map(|axis| {
        workbench.tiling_ui.origin[axis]
            + ((rect.min[axis] + rect.max[axis]) / 2) as f32 * workbench.tiling_ui.size[axis]
                / EXTENT as f32
    });
    context.io_mut().add_mouse_pos_event(target);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    for _ in 0..8 {
        frame(&mut context, &mut workbench);
    }
    let document = workbench
        .session
        .document_for_path(&path)
        .expect("the delivered Files row opens a document");
    let panel = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.document() == Some(document))
        .unwrap();
    assert_eq!(workbench.tiling.area_for(panel.id), Some(empty));
    assert_eq!(workbench.tiling.area_for(files_id), Some(files_area));
    assert!(
        path.exists(),
        "opening a file in an area does not move it on disk"
    );
    context.binding().with_bound_context(|| unsafe {
        let name = CString::new(workbench.title(panel)).unwrap();
        let window = sys::igFindWindowByName(name.as_ptr()).as_ref().unwrap();
        assert_eq!(window.DockId, workbench.tiling_ui.docks[&empty]);
        assert!(window.DockTabIsVisible());
    });
    workbench.cleanup().unwrap();
}
