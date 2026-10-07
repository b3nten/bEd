//! Native ImGui focus/input checks for the workspace panels and shared views.
//! ned workbench.cpp supplies docking/focus behavior; the Session ownership and
//! docked tool tabs are Bed extensions. See LICENSE and NOTICE.
use bed::{
    util::settings::Settings,
    workbench::{WindowCommand, Workbench, WorkbenchHostMode},
};
use bed_core::{editor_commands::CursorReveal, editor_view_state::Selection};
use bed_session::editor_session::{DocumentId, ViewId};
use dear_imgui_rs::{Context, FramePrepareOptions, Key, sys};
use serde_json::json;
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static CONTEXT_LOCK: Mutex<()> = Mutex::new(());
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed-workbench-focus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
    fn workbench(&self, context: &mut Context) -> Workbench {
        let mut settings = Settings::with_paths(
            self.0.join("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        for key in [
            "terminal_visible",
            "sidebar_visible",
            "treesitter",
            "git_changed_lines",
            "minimap",
        ] {
            settings.settings[key] = json!(false);
        }
        let mut workbench = Workbench::with_settings(settings);
        workbench
            .initialize(context, WorkbenchHostMode::Floating)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        workbench
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
}
fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1600.0, 1000.0], 1.0 / 60.0));
    assert!(workbench.render(context.frame()).unwrap().is_empty());
    assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
}
fn open(workbench: &mut Workbench, path: &std::path::Path) -> (DocumentId, ViewId, usize) {
    workbench.open_or_focus(path).unwrap();
    (
        workbench.active_document().unwrap(),
        workbench.active_view().unwrap(),
        workbench.active_index(),
    )
}
fn bytes(workbench: &Workbench, id: DocumentId) -> Vec<u8> {
    workbench.session.snapshot(id).unwrap().bytes
}
fn selections(workbench: &Workbench, id: ViewId) -> Vec<Selection> {
    workbench.session.view_snapshot(id).unwrap().selections
}
fn switch_shortcut(context: &mut Context, workbench: &mut Workbench, index: usize) {
    let key = [
        Key::Key1,
        Key::Key2,
        Key::Key3,
        Key::Key4,
        Key::Key5,
        Key::Key6,
        Key::Key7,
        Key::Key8,
        Key::Key9,
    ][index];
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    context.io_mut().add_key_event(modifier, true);
    context.io_mut().add_key_event(key, true);
    frame(context, workbench);
    context.io_mut().add_key_event(key, false);
    context.io_mut().add_key_event(modifier, false);
    frame(context, workbench);
}
fn window_name(workbench: &Workbench, index: usize) -> CString {
    CString::new(format!(
        "###bed_tab_{}",
        workbench.tab_window_id(index).unwrap()
    ))
    .unwrap()
}

#[derive(Clone, Copy, Debug)]
struct NativePanel {
    window_id: u32,
    dock_id: u32,
    area: f32,
    leaf: bool,
    padding: [f32; 2],
    rect: [f32; 4],
    selected: bool,
}
fn native_panel(context: &Context, workbench: &Workbench, index: usize) -> NativePanel {
    let name = window_name(workbench, index);
    context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(name.as_ptr());
        assert!(!window.is_null(), "panel must have a real native window");
        let node = (*window).DockNode;
        let rect = if node.is_null() {
            [
                (*window).Pos.x,
                (*window).Pos.y,
                (*window).Pos.x + (*window).Size.x,
                (*window).Pos.y + (*window).Size.y,
            ]
        } else {
            let rect = sys::ImGuiDockNode_Rect(node);
            [rect.Min.x, rect.Min.y, rect.Max.x, rect.Max.y]
        };
        let area = if node.is_null() {
            (*window).Size.x * (*window).Size.y
        } else {
            let rect = sys::ImGuiDockNode_Rect(node);
            (rect.Max.x - rect.Min.x) * (rect.Max.y - rect.Min.y)
        };
        NativePanel {
            window_id: (*window).ID,
            dock_id: (*window).DockId,
            area,
            leaf: !node.is_null() && sys::ImGuiDockNode_IsLeafNode(node),
            padding: [(*window).WindowPadding.x, (*window).WindowPadding.y],
            rect,
            selected: (*window).DockTabIsVisible(),
        }
    })
}

#[test]
fn split_controls_target_the_active_document_when_files_has_focus() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    workbench.set_project(&fixture.0).unwrap();
    let source = b"shared document\nsecond line";
    let (document, original_view, first_tab) =
        open(&mut workbench, &fixture.write("split.txt", source));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let files = native_panel(&context, &workbench, 0);
    let original_selections = selections(&workbench, original_view);
    workbench.dispatch(WindowCommand::Find).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    // A tool in the source document's group should not stay selected over the
    // original document when the split button creates its duplicate.
    workbench.dispatch(WindowCommand::NewDiagnostics).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert!(!native_panel(&context, &workbench, first_tab).selected);
    let mut source_tab = first_tab;
    for command in [WindowCommand::SplitRight, WindowCommand::SplitDown] {
        workbench.switch_to_tab(0);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
        workbench.dispatch(command).unwrap();
        let destination_tab = workbench.active_index();
        let destination_view = workbench.active_view();
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
        let source_panel = native_panel(&context, &workbench, source_tab);
        let destination = native_panel(&context, &workbench, destination_tab);
        assert!(source_panel.leaf && destination.leaf);
        assert!(
            source_panel.selected && destination.selected,
            "{command:?}: {source_panel:?} -> {destination:?}"
        );
        assert_ne!(source_panel.dock_id, destination.dock_id);
        assert_ne!(destination.dock_id, files.dock_id);
        match command {
            WindowCommand::SplitRight => assert!(destination.rect[0] >= source_panel.rect[2] - 1.0),
            WindowCommand::SplitDown => assert!(destination.rect[1] >= source_panel.rect[3] - 1.0),
            _ => unreachable!(),
        }
        assert_eq!(native_panel(&context, &workbench, 0).rect, files.rect);
        assert_eq!(workbench.active_document(), Some(document));
        assert_eq!(workbench.active_index(), destination_tab);
        assert_eq!(workbench.active_view(), destination_view);
        source_tab = destination_tab;
    }
    assert_eq!(workbench.session.view_ids(document).len(), 3);
    assert_eq!(bytes(&workbench, document), source);
    assert_eq!(selections(&workbench, original_view), original_selections);
    workbench.cleanup().unwrap();
}

#[test]
fn opening_files_from_narrow_files_panel_uses_largest_content_leaf() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    workbench.set_project(&fixture.0).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert!(workbench.panel_visible("explorer"));
    let files = native_panel(&context, &workbench, 0);
    assert!(files.leaf);
    // The first document must also choose the empty central leaf, rather than
    // the only currently populated (narrow Files) leaf.
    let (_, _, first_tab) = open(&mut workbench, &fixture.write("first.txt", b"first"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let first = native_panel(&context, &workbench, first_tab);
    assert!(first.leaf);
    assert_ne!(first.dock_id, files.dock_id);
    assert!(first.area > files.area);
    workbench.switch_to_tab(0);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let (_, _, second_tab) = open(&mut workbench, &fixture.write("second.txt", b"second"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(
        native_panel(&context, &workbench, second_tab).dock_id,
        first.dock_id
    );
    assert_eq!(native_panel(&context, &workbench, 0).dock_id, files.dock_id);
    workbench.cleanup().unwrap();
}

#[test]
fn spawning_documents_recomputes_the_largest_resized_content_group() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (_, _, first_tab) = open(&mut workbench, &fixture.write("left.txt", b"left"));
    let (_, _, second_tab) = open(&mut workbench, &fixture.write("right.txt", b"right"));
    for tab in [first_tab, second_tab] {
        workbench.switch_to_tab(tab);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
    }
    let names = [
        window_name(&workbench, first_tab),
        window_name(&workbench, second_tab),
    ];
    let root = native_panel(&context, &workbench, first_tab).dock_id;
    let mut left = 0;
    let mut right = 0;
    context.binding().with_bound_context(|| unsafe {
        sys::igDockBuilderSplitNode(root, sys::ImGuiDir_Left, 0.75, &mut left, &mut right);
        sys::igDockBuilderDockWindow(names[0].as_ptr(), left);
        sys::igDockBuilderDockWindow(names[1].as_ptr(), right);
        sys::igDockBuilderFinish(root);
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert!(
        native_panel(&context, &workbench, first_tab).area
            > native_panel(&context, &workbench, second_tab).area
    );
    workbench.switch_to_tab(second_tab);
    let (_, _, third_tab) = open(&mut workbench, &fixture.write("opened.txt", b"opened"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(native_panel(&context, &workbench, third_tab).dock_id, left);
    context.binding().with_bound_context(|| unsafe {
        let left_rect = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(left));
        let right_rect = sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(right));
        let width = right_rect.Max.x - left_rect.Min.x;
        let height = left_rect.Max.y - left_rect.Min.y;
        sys::igDockBuilderSetNodeSize(left, [width * 0.2, height].into());
        sys::igDockBuilderSetNodeSize(right, [width * 0.8, height].into());
        sys::igDockBuilderFinish(root);
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert!(
        native_panel(&context, &workbench, second_tab).area
            > native_panel(&context, &workbench, third_tab).area
    );
    workbench.switch_to_tab(third_tab);
    workbench.dispatch(WindowCommand::NewDocument).unwrap();
    let spawned = workbench.active_index();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(native_panel(&context, &workbench, spawned).dock_id, right);
    assert!(native_panel(&context, &workbench, spawned).leaf);
    workbench.cleanup().unwrap();
}

#[test]
fn repeated_tool_panels_have_unique_stable_native_ids_and_close_independently() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (document, _, document_tab) = open(&mut workbench, &fixture.write("main.txt", b"source"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let content_dock = native_panel(&context, &workbench, document_tab).dock_id;
    let commands = [
        (WindowCommand::NewExplorer, "explorer"),
        (WindowCommand::NewSettings, "settings"),
        (WindowCommand::NewProjects, "projects"),
        (WindowCommand::NewDiagnostics, "diagnostics"),
        (WindowCommand::NewLspDashboard, "lsp"),
        (WindowCommand::NewContentSearch, "search"),
        (WindowCommand::NewReferences, "references"),
    ];
    let mut tab_ids = BTreeSet::new();
    let mut native_ids = BTreeSet::new();
    let mut spawned = Vec::new();
    for (command, kind) in commands {
        for _ in 0..2 {
            let before = workbench.panel_count(kind);
            let before_tabs = workbench.tab_count();
            workbench.dispatch(command).unwrap();
            assert_eq!(workbench.tab_count(), before_tabs + 1);
            assert_eq!(workbench.panel_count(kind), before + 1);
            let index = workbench.active_index();
            let id = workbench.tab_window_id(index).unwrap();
            assert!(
                tab_ids.insert(id),
                "new panels must have unique persistent IDs"
            );
            frame(&mut context, &mut workbench);
            frame(&mut context, &mut workbench);
            let native = native_panel(&context, &workbench, index);
            assert!(
                native_ids.insert(native.window_id),
                "same-title panels must not alias native windows"
            );
            assert_eq!(native.dock_id, content_dock);
            assert!(native.leaf);
            spawned.push((id, native.window_id));
        }
    }
    // Closing one of the two same-title Settings tabs leaves the other native
    // window and all distinct documents/tool instances intact.
    let closed = spawned[2].0;
    let index = (0..workbench.tab_count())
        .find(|index| workbench.tab_window_id(*index) == Some(closed))
        .unwrap();
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(workbench.panel_count("settings"), 1);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    for (id, native_id) in spawned.iter().copied().filter(|(id, _)| *id != closed) {
        let index = (0..workbench.tab_count())
            .find(|index| workbench.tab_window_id(*index) == Some(id))
            .unwrap();
        assert_eq!(
            native_panel(&context, &workbench, index).window_id,
            native_id
        );
    }
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    let replacement = workbench.tab_window_id(workbench.active_index()).unwrap();
    assert!(replacement > *tab_ids.last().unwrap());
    assert_eq!(workbench.panel_count("settings"), 2);
    assert_eq!(bytes(&workbench, document), b"source");
    workbench.cleanup().unwrap();
}

#[test]
fn largest_floating_dock_group_accepts_new_files_but_bare_windows_are_excluded() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (_, _, main_tab) = open(&mut workbench, &fixture.write("main.txt", b"main"));
    let (_, _, floating_tab) = open(&mut workbench, &fixture.write("floating.txt", b"floating"));
    for tab in [main_tab, floating_tab] {
        workbench.switch_to_tab(tab);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
    }
    let floating_name = window_name(&workbench, floating_tab);
    // Build an actual movable floating dock tree. Headless ImGui still uses
    // the same leaf/node metadata that secondary platform viewports expose.
    let floating_dock = context.binding().with_bound_context(|| unsafe {
        let id = sys::igDockBuilderAddNode(0, 0);
        sys::igDockBuilderSetNodePos(id, [15.0, 15.0].into());
        sys::igDockBuilderSetNodeSize(id, [1550.0, 950.0].into());
        sys::igDockBuilderDockWindow(floating_name.as_ptr(), id);
        sys::igDockBuilderFinish(id);
        id
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.binding().with_bound_context(|| unsafe {
        sys::igSetWindowSize_Str(
            floating_name.as_ptr(),
            [1550.0, 950.0].into(),
            sys::ImGuiCond_Always,
        );
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let floating = native_panel(&context, &workbench, floating_tab);
    assert_eq!(floating.dock_id, floating_dock);
    assert!(floating.leaf);
    assert!(
        floating.area > native_panel(&context, &workbench, main_tab).area,
        "floating {floating:?}, main {:?}",
        native_panel(&context, &workbench, main_tab)
    );
    workbench.switch_to_tab(main_tab);
    let (_, _, opened_tab) = open(&mut workbench, &fixture.write("opened.txt", b"opened"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(
        native_panel(&context, &workbench, opened_tab).dock_id,
        floating_dock
    );
    // A larger free window has no dock group. It must not turn a subsequent
    // open into another undocked window or supersede the largest real leaf.
    let main_id = workbench.tab_window_id(main_tab).unwrap();
    assert!(workbench.detach_panel_for_smoke(main_id, [5.0, 5.0]));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let bare_name = window_name(&workbench, main_tab);
    context.binding().with_bound_context(|| unsafe {
        sys::igSetWindowSize_Str(
            bare_name.as_ptr(),
            [1580.0, 980.0].into(),
            sys::ImGuiCond_Always,
        );
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let bare = native_panel(&context, &workbench, main_tab);
    assert_eq!(bare.dock_id, 0);
    assert!(bare.area > floating.area);
    workbench.switch_to_tab(main_tab);
    workbench.dispatch(WindowCommand::NewDocument).unwrap();
    let spawned = workbench.active_index();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(
        native_panel(&context, &workbench, spawned).dock_id,
        floating_dock
    );
    workbench.cleanup().unwrap();
}

#[test]
fn compact_document_and_files_padding_does_not_change_the_host_style() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let host_padding = [11.0, 13.0];
    context.style_mut().set_window_padding(host_padding);
    let (_, _, document_tab) = open(&mut workbench, &fixture.write("main.txt", b"source"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(
        native_panel(&context, &workbench, document_tab).padding,
        [0.0; 2]
    );
    assert_eq!(native_panel(&context, &workbench, 0).padding, [2.0; 2]);
    assert_eq!(context.style().window_padding(), host_padding);
    workbench.cleanup().unwrap();
}

#[test]
fn native_dock_tab_list_has_popup_padding_and_preserves_compact_panel_geometry() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let host_padding = [19.0, 13.0];
    let host_spacing = [11.0, 7.0];
    context.style_mut().set_window_padding(host_padding);
    context.style_mut().set_item_spacing(host_spacing);
    context
        .style_mut()
        .set_window_menu_button_position(dear_imgui_rs::Direction::Left);
    let (_, first_view, first_tab) = open(&mut workbench, &fixture.write("first.txt", b"first"));
    let (_, _, second_tab) = open(&mut workbench, &fixture.write("second.txt", b"second"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let files_before = native_panel(&context, &workbench, 0);
    let content_before = native_panel(&context, &workbench, second_tab);
    let name = window_name(&workbench, second_tab);
    let (arrow, font_size) = context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(name.as_ptr());
        assert!(!window.is_null());
        let rect = sys::ImGuiDockNode_Rect((*window).DockNode);
        let font_size = (*window).FontRefSize;
        let style = sys::igGetStyle();
        (
            [
                rect.Min.x + (*style).WindowBorderSize + (*style).FramePadding.x + font_size * 0.5,
                rect.Min.y + (*style).FramePadding.y + font_size * 0.5,
            ],
            font_size,
        )
    });
    context.io_mut().add_mouse_pos_event(arrow);
    context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let popup_position = context.binding().with_bound_context(|| unsafe {
        let stack = &(*sys::igGetCurrentContext()).OpenPopupStack;
        assert_eq!(
            stack.Size, 1,
            "clicking the dock triangle opens its native tab-list popup"
        );
        let window = (*stack.Data).Window;
        assert!(!window.is_null());
        assert_eq!(
            [(*window).WindowPadding.x, (*window).WindowPadding.y],
            [8.0, 6.0]
        );
        assert_eq!(
            [
                (*window).DC.CursorStartPos.x - (*window).Pos.x,
                (*window).DC.CursorStartPos.y - (*window).Pos.y,
            ],
            [8.0, 6.0]
        );
        [(*window).Pos.x, (*window).Pos.y]
    });
    let files_after = native_panel(&context, &workbench, 0);
    let content_after = native_panel(&context, &workbench, second_tab);
    assert_eq!(files_after.padding, [2.0; 2]);
    assert_eq!(content_after.padding, [0.0; 2]);
    assert_eq!(files_after.area, files_before.area);
    assert_eq!(content_after.area, content_before.area);
    assert_eq!(context.style().window_padding(), host_padding);
    assert_eq!(context.style().item_spacing(), host_spacing);
    // The stock list remains interactive after the spacing override.
    context.io_mut().add_mouse_pos_event([
        popup_position[0] + 20.0,
        popup_position[1] + 6.0 + font_size * 0.5,
    ]);
    context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.active_view(), Some(first_view));
    assert_eq!(
        native_panel(&context, &workbench, first_tab).padding,
        [0.0; 2]
    );
    workbench.cleanup().unwrap();
}

#[test]
fn panel_spawn_uses_its_originating_context_and_restores_the_hosts_current_context() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (_, _, first_tab) = open(&mut workbench, &fixture.write("first.txt", b"first"));
    let (_, _, second_tab) = open(&mut workbench, &fixture.write("second.txt", b"second"));
    for tab in [first_tab, second_tab] {
        workbench.switch_to_tab(tab);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
    }
    let names = [
        window_name(&workbench, first_tab),
        window_name(&workbench, second_tab),
    ];
    let root = native_panel(&context, &workbench, first_tab).dock_id;
    let mut left = 0;
    let mut right = 0;
    context.binding().with_bound_context(|| unsafe {
        sys::igDockBuilderSplitNode(root, sys::ImGuiDir_Left, 0.8, &mut left, &mut right);
        sys::igDockBuilderDockWindow(names[0].as_ptr(), left);
        sys::igDockBuilderDockWindow(names[1].as_ptr(), right);
        sys::igDockBuilderFinish(root);
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert!(
        native_panel(&context, &workbench, first_tab).area
            > native_panel(&context, &workbench, second_tab).area
    );
    let suspended = context.suspend().unwrap();
    let mut other = Context::create();
    other.set_ini_filename(None::<PathBuf>).unwrap();
    let other_native = unsafe { sys::igGetCurrentContext() };
    assert!(!other_native.is_null());
    // Host menu actions may run between frames with a different ImGui context
    // active. Geometry reads must bind the workspace's context temporarily.
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    let spawned = workbench.active_index();
    assert_eq!(unsafe { sys::igGetCurrentContext() }, other_native);
    drop(other);
    let mut context = suspended.activate().unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(native_panel(&context, &workbench, spawned).dock_id, left);
    workbench.cleanup().unwrap();
}

#[test]
fn panel_shortcuts_route_unicode_and_preserve_inactive_document_selections() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (first, a, first_tab) = open(&mut workbench, &fixture.write("a.txt", b"alpha\nline"));
    let (second, b, second_tab) = open(&mut workbench, &fixture.write("b.txt", b"beta"));
    workbench
        .session
        .with_commands(a, |c| c.set_selection(0, 0, 0, 5, CursorReveal::Ensure))
        .unwrap();
    workbench
        .session
        .with_commands(b, |c| c.set_cursor(0, 4, false, CursorReveal::Ensure))
        .unwrap();
    let first_selection = selections(&workbench, a);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("🙂");
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.active_view(), Some(b));
    assert_eq!(bytes(&workbench, second), "beta🙂".as_bytes());
    assert_eq!(bytes(&workbench, first), b"alpha\nline");
    assert_eq!(selections(&workbench, a), first_selection);
    switch_shortcut(&mut context, &mut workbench, first_tab);
    assert_eq!(workbench.active_view(), Some(a));
    let inactive = selections(&workbench, b);
    context.io_mut().add_input_characters_utf8("雪");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, first), "雪\nline".as_bytes());
    assert_eq!(bytes(&workbench, second), "beta🙂".as_bytes());
    assert_eq!(selections(&workbench, b), inactive);
    switch_shortcut(&mut context, &mut workbench, second_tab);
    context.io_mut().add_input_characters_utf8("é");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, second), "beta🙂é".as_bytes());
    assert_eq!(workbench.session.view_snapshot(b).unwrap().column, 10);
    assert_eq!(workbench.session.view_snapshot(a).unwrap().column, 3);
    workbench.cleanup().unwrap();
}

#[test]
fn settings_tool_focus_preserves_multi_carets_then_document_focus_restores_editing() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (first, _, _) = open(&mut workbench, &fixture.write("a.txt", b"first"));
    let (second, view, tab) = open(&mut workbench, &fixture.write("b.txt", b"second"));
    let mut a = Selection::default();
    a.set_both(0, 2);
    let mut b = Selection::default();
    b.set_both(0, 5);
    workbench
        .session
        .with_commands(view, |c| {
            c.set_selections(vec![a, b], 1, CursorReveal::Ensure)
        })
        .unwrap();
    let selection = selections(&workbench, view);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.dispatch(WindowCommand::Settings).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("x");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, first), b"first");
    assert_eq!(bytes(&workbench, second), b"second");
    assert_eq!(selections(&workbench, view), selection);
    assert!(workbench.panel_visible("settings"));
    workbench.switch_to_tab(tab);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("é");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, first), b"first");
    assert_eq!(bytes(&workbench, second), "seéconéd".as_bytes());
    assert_eq!(
        workbench
            .session
            .view_snapshot(view)
            .unwrap()
            .selection_count(),
        2
    );
    workbench.cleanup().unwrap();
}

#[test]
fn docked_dashboard_keeps_host_padding_and_creates_no_floating_dashboard() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    context.style_mut().set_window_padding([19.0, 13.0]);
    context.style_mut().set_window_border_size(2.0);
    let mut workbench = fixture.workbench(&mut context);
    open(&mut workbench, &fixture.write("note.txt", b"document"));
    workbench.dispatch(WindowCommand::LspDashboard).unwrap();
    let name = window_name(&workbench, workbench.active_index());
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(name.as_ptr());
        assert!(!window.is_null());
        assert_ne!((*window).DockId, 0);
        assert_eq!(
            [(*window).WindowPadding.x, (*window).WindowPadding.y],
            [19.0, 13.0]
        );
        assert_eq!((*window).WindowBorderSize, 2.0);
        assert!(sys::igFindWindowByName(c"LSP Server Dashboard".as_ptr()).is_null());
    });
    assert_eq!(context.style().window_padding(), [19.0, 13.0]);
    assert_eq!(context.style().window_border_size(), 2.0);
    workbench.cleanup().unwrap();
}

#[test]
fn docked_project_search_owns_text_input_while_split_documents_keep_selections() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    workbench.set_project(&fixture.0).unwrap();
    let (first, a, first_tab) = open(&mut workbench, &fixture.write("a.txt", b"first"));
    let (second, b, second_tab) = open(&mut workbench, &fixture.write("b.txt", b"second"));
    for tab in [first_tab, second_tab] {
        workbench.switch_to_tab(tab);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
    }
    let names = [
        window_name(&workbench, first_tab),
        window_name(&workbench, second_tab),
    ];
    let mut left = 0;
    let mut right = 0;
    context.binding().with_bound_context(|| unsafe {
        let root = (*sys::igFindWindowByName(names[0].as_ptr())).DockId;
        assert_ne!(root, 0);
        sys::igDockBuilderSplitNode(root, sys::ImGuiDir_Left, 0.5, &mut left, &mut right);
        sys::igDockBuilderDockWindow(names[0].as_ptr(), left);
        sys::igDockBuilderDockWindow(names[1].as_ptr(), right);
        sys::igDockBuilderFinish(root);
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    for (view, len) in [(a, 5), (b, 6)] {
        workbench
            .session
            .with_commands(view, |c| {
                c.set_selection(0, 0, 0, len, CursorReveal::Ensure)
            })
            .unwrap();
    }
    let selection = [selections(&workbench, a), selections(&workbench, b)];
    workbench.dispatch(WindowCommand::FindProject).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("needle🙂");
    frame(&mut context, &mut workbench);
    assert!(context.io().want_text_input());
    assert!(workbench.panel_visible("search"));
    assert_eq!(bytes(&workbench, first), b"first");
    assert_eq!(bytes(&workbench, second), b"second");
    assert_eq!(selections(&workbench, a), selection[0]);
    assert_eq!(selections(&workbench, b), selection[1]);
    workbench.switch_to_tab(first_tab);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("é");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, first), "é".as_bytes());
    assert_eq!(bytes(&workbench, second), b"second");
    assert_eq!(selections(&workbench, b), selection[1]);
    workbench.cleanup().unwrap();
}

#[test]
fn duplicated_document_views_share_edits_but_keep_independent_carets() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (document, first, first_tab) = open(&mut workbench, &fixture.write("shared.txt", b"abcd"));
    workbench
        .session
        .with_commands(first, |c| c.set_cursor(0, 1, false, CursorReveal::Ensure))
        .unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workbench.active_view().unwrap();
    let second_tab = workbench.active_index();
    assert_ne!(first, second);
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(workbench.session.document_ids().len(), 1);
    assert_eq!(workbench.session.view_count(document), 2);
    workbench
        .session
        .with_commands(second, |c| c.set_cursor(0, 4, false, CursorReveal::Ensure))
        .unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.active_view(), Some(second));
    context.io_mut().add_input_characters_utf8("🙂");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, document), "abcd🙂".as_bytes());
    assert_eq!(workbench.session.view_snapshot(first).unwrap().column, 1);
    assert_eq!(workbench.session.view_snapshot(second).unwrap().column, 8);
    switch_shortcut(&mut context, &mut workbench, first_tab);
    assert_eq!(
        workbench.active_view(),
        Some(first),
        "numbered panel shortcut should focus the requested view"
    );
    context.io_mut().add_input_characters_utf8("雪");
    frame(&mut context, &mut workbench);
    assert_eq!(bytes(&workbench, document), "a雪bcd🙂".as_bytes());
    assert_eq!(workbench.session.view_snapshot(first).unwrap().column, 4);
    assert_eq!(workbench.session.view_snapshot(second).unwrap().column, 11);
    switch_shortcut(&mut context, &mut workbench, second_tab);
    assert_eq!(workbench.active_view(), Some(second));
    workbench.cleanup().unwrap();
}

#[test]
fn native_viewport_refocus_retains_its_selected_editor_instead_of_the_sidebar() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (other, _, other_tab) = open(&mut workbench, &fixture.write("other.txt", b"inactive"));
    let (_, view, selected_tab) = open(&mut workbench, &fixture.write("selected.txt", b"document"));
    for tab in [other_tab, selected_tab] {
        workbench.switch_to_tab(tab);
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
    }
    let names = [
        window_name(&workbench, other_tab),
        window_name(&workbench, selected_tab),
    ];
    context.binding().with_bound_context(|| unsafe {
        let root = (*sys::igFindWindowByName(names[0].as_ptr())).DockId;
        let mut left = 0;
        let mut right = 0;
        sys::igDockBuilderSplitNode(root, sys::ImGuiDir_Left, 0.5, &mut left, &mut right);
        sys::igDockBuilderDockWindow(names[0].as_ptr(), left);
        sys::igDockBuilderDockWindow(names[1].as_ptr(), right);
        sys::igDockBuilderFinish(root);
    });
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.switch_to_tab(selected_tab);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let selected = workbench.active_panel_id();
    let viewport = context.main_viewport().id().raw();
    let files = window_name(&workbench, 0);
    context.binding().with_bound_context(|| unsafe {
        for name in [files.as_c_str(), names[0].as_c_str(), names[1].as_c_str()] {
            let window = sys::igFindWindowByName(name.as_ptr());
            assert!(!window.is_null());
            assert!((*window).DockTabIsVisible());
        }
        assert!(workbench.focus_viewport(viewport));
    });
    assert_eq!(
        workbench.active_panel_id(),
        selected,
        "native refocus must not choose the first visible Files panel"
    );
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("x");
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"xdocument");
    assert_eq!(bytes(&workbench, other), b"inactive");
    workbench.cleanup().unwrap();
}

#[test]
fn workspace_restore_focus_survives_the_first_native_frames() {
    let _lock = CONTEXT_LOCK.lock().unwrap();
    let fixture = Fixture::new();
    let mut context = context();
    let mut workbench = fixture.workbench(&mut context);
    let (first, _, first_tab) = open(&mut workbench, &fixture.write("first.txt", b"first"));
    open(&mut workbench, &fixture.write("last.txt", b"last"));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.switch_to_tab(first_tab);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.active_document(), Some(first));
    let panel = workbench.active_panel_id().unwrap();
    workbench.cleanup().unwrap();
    drop(workbench);
    drop(context);
    let mut restored_context = self::context();
    let mut restored = fixture.workbench(&mut restored_context);
    restored.set_project(&fixture.0).unwrap();
    restored.apply_settings(&mut restored_context).unwrap();
    assert_eq!(restored.active_panel_id(), Some(panel));
    frame(&mut restored_context, &mut restored);
    assert_eq!(
        restored.active_panel_id(),
        Some(panel),
        "native window creation must retain the persisted focus"
    );
    frame(&mut restored_context, &mut restored);
    assert_eq!(
        restored.active_panel_id(),
        Some(panel),
        "restoring subsequent tabs must not overwrite persisted focus"
    );
    assert_eq!(
        restored.active_snapshot().unwrap().path,
        fs::canonicalize(fixture.0.join("first.txt"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    restored.cleanup().unwrap();
}
