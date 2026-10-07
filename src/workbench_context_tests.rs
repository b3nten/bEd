//! Native mouse/menu regressions for the standalone shell's new context actions.
use super::*;
use crate::files::{
    file_tree::{FileTree, FileTreeAction, FileTreeStyle},
    test_support::TempDir,
};
use bed_ui::views::view_layout::{ViewLayout, line_column_x};
use dear_imgui_rs::{ClipboardBackend, FramePrepareOptions};
use std::cell::RefCell;

#[derive(Default)]
struct Clipboard {
    text: String,
    reads: usize,
    writes: usize,
}
struct HostClipboard(Rc<RefCell<Clipboard>>);
impl ClipboardBackend for HostClipboard {
    fn get(&mut self) -> Option<String> {
        let mut clipboard = self.0.borrow_mut();
        clipboard.reads += 1;
        Some(clipboard.text.clone())
    }
    fn set(&mut self, text: &str) {
        let mut clipboard = self.0.borrow_mut();
        clipboard.writes += 1;
        clipboard.text = text.to_owned();
    }
}
fn workspace(dir: &TempDir, context: &mut Context) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
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
    settings.terminal_visible = false;
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
fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
}
fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    assert!(workbench.render(context.frame()).unwrap().is_empty());
    assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
}
fn layout(workbench: &mut Workbench, view_id: ViewId) -> ViewLayout {
    workbench
        .tabs
        .iter_mut()
        .find_map(|tab| match &mut tab.panel {
            Panel::Document(view) if view.id() == view_id => Some(view.presentation().layout),
            _ => None,
        })
        .unwrap()
}
fn text_point(
    context: &mut Context,
    workbench: &mut Workbench,
    view: ViewId,
    row: i32,
    column: i32,
) -> [f32; 2] {
    let layout = layout(workbench, view);
    let document = workbench.session.document_for_view(view).unwrap();
    let line = workbench
        .session
        .with_document(document, |state| state.line(row))
        .unwrap();
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    let ui = context.frame();
    let font = workbench.settings.font.main.map(|font| ui.push_font(font));
    let offset = line_column_x(ui, &line, column, 0.0);
    drop(font);
    workbench.render(ui).unwrap();
    drop(context.render_legacy());
    [
        layout.text_pos[0] + offset,
        layout.text_pos[1] + row as f32 * layout.line_height + layout.line_height * 0.5,
    ]
}
fn mouse(
    context: &mut Context,
    workbench: &mut Workbench,
    point: [f32; 2],
    button: MouseButton,
    down: bool,
) {
    context.io_mut().add_mouse_pos_event(point);
    context.io_mut().add_mouse_button_event(button, down);
    frame(context, workbench);
}
fn popup_point(context: &Context, rows_from_last: f32) -> [f32; 2] {
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1, "one live context popup");
        let popup = &*native.OpenPopupStack.Data;
        assert!(!popup.Window.is_null());
        let window = &*popup.Window;
        let height = window.DC.PrevLineSize.y;
        assert!(height > 0.0);
        [
            window.DC.CursorStartPos.x + height,
            window.DC.CursorPosPrevLine.y - rows_from_last * (height + native.Style.ItemSpacing.y)
                + height * 0.5,
        ]
    })
}
fn click_menu(context: &mut Context, workbench: &mut Workbench, rows_from_last: f32) {
    let point = popup_point(context, rows_from_last);
    mouse(context, workbench, point, MouseButton::Left, true);
    mouse(context, workbench, point, MouseButton::Left, false);
}

fn lsp_menu_frame(context: &mut Context, ready: bool, open: bool) -> Option<u8> {
    context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    let ui = context.frame();
    let mut action = None;
    ui.window("Language actions")
        .position([20.0, 20.0], Condition::Always)
        .size([400.0, 400.0], Condition::Always)
        .build(|| {
            if open {
                ui.open_popup("Language menu");
            }
            if let Some(_popup) = ui.begin_popup("Language menu") {
                action = text_lsp_actions(ui, ready);
            }
        });
    drop(context.render_legacy());
    action
}

#[test]
fn text_language_actions_click_when_ready_and_remain_disabled_until_ready() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for ready in [false, true] {
        for command in 0..3 {
            let mut context = context();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            context.io_mut().add_mouse_pos_event([150.0, 100.0]);
            assert_eq!(lsp_menu_frame(&mut context, ready, true), None);
            assert_eq!(lsp_menu_frame(&mut context, ready, false), None);
            let point = popup_point(&context, (2 - command) as f32);
            context.io_mut().add_mouse_pos_event(point);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            let press = lsp_menu_frame(&mut context, ready, false);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            let release = lsp_menu_frame(&mut context, ready, false);
            assert_eq!(
                press.or(release),
                ready.then_some(command as u8),
                "language command {command} with server ready={ready}"
            );
        }
    }
}

fn button_point(context: &Context, name: &str, final_button: bool) -> [f32; 2] {
    let name = CString::new(name).unwrap();
    context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(name.as_ptr());
        assert!(!window.is_null(), "live button window");
        let window = &*window;
        [
            if final_button {
                window.DC.CursorPosPrevLine.x - window.FontRefSize
            } else {
                window.DC.CursorStartPos.x + window.FontRefSize
            },
            window.DC.CursorPosPrevLine.y + window.DC.PrevLineSize.y * 0.5,
        ]
    })
}

#[test]
fn dirty_disk_reload_requires_confirmation_and_cancel_preserves_live_edits() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("reload.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    let mut options = workbench.session.options().clone();
    options.autosave = None;
    workbench.session.configure(options).unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench
        .session
        .with_commands(view, |commands| {
            commands.move_doc_end(false);
            commands.type_text("🙂".as_bytes());
        })
        .unwrap();
    std::fs::write(&path, b"disk changed").unwrap();
    let report = workbench.session.tick();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let before = workbench.session.snapshot(document).unwrap();
    assert!(before.dirty && before.disk_conflict.is_some());
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let conflict_window = format!("###conflict_{}", document.0);
    let reload = button_point(&context, &conflict_window, false);
    mouse(
        &mut context,
        &mut workbench,
        reload,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        reload,
        MouseButton::Left,
        false,
    );
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.reload_confirmation, Some(document));
    context.io_mut().add_input_characters_utf8("unwanted");
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.session.snapshot(document).unwrap(), before);
    let cancel = button_point(&context, "Discard Changes?", false);
    mouse(
        &mut context,
        &mut workbench,
        cancel,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        cancel,
        MouseButton::Left,
        false,
    );
    assert!(workbench.reload_confirmation.is_none());
    assert_eq!(workbench.session.snapshot(document).unwrap(), before);
    let reload = button_point(&context, &conflict_window, false);
    mouse(
        &mut context,
        &mut workbench,
        reload,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        reload,
        MouseButton::Left,
        false,
    );
    frame(&mut context, &mut workbench);
    let confirm = button_point(&context, "Discard Changes?", true);
    mouse(
        &mut context,
        &mut workbench,
        confirm,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        confirm,
        MouseButton::Left,
        false,
    );
    assert!(workbench.reload_confirmation.is_none());
    let reloaded = workbench.session.snapshot(document).unwrap();
    assert_eq!(reloaded.bytes, b"disk changed");
    assert!(!reloaded.dirty && reloaded.disk_conflict.is_none());
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"disk changed"
    );
    workbench.cleanup().unwrap();
}

#[test]
fn text_context_copy_paste_preserves_selection_and_uses_host_clipboard() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("selected.txt", "A🙂éZ\nsecond".as_bytes());
    let mut context = context();
    let clipboard = Rc::new(RefCell::new(Clipboard::default()));
    context.set_clipboard_backend(HostClipboard(Rc::clone(&clipboard)));
    let mut workbench = workspace(&dir, &mut context);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench
        .session
        .with_commands(view, |commands| {
            commands.set_selection(0, 1, 0, 7, CursorReveal::Ensure)
        })
        .unwrap();
    let before = workbench.session.view_snapshot(view).unwrap().selections;
    let point = text_point(&mut context, &mut workbench, view, 0, 5);
    let host_padding = context
        .binding()
        .with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).Style.WindowPadding });
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        false,
    );
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        before
    );
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1);
        let popup = &*(*native.OpenPopupStack.Data).Window;
        assert_eq!([popup.WindowPadding.x, popup.WindowPadding.y], [8.0, 6.0]);
        assert_eq!(
            [native.Style.WindowPadding.x, native.Style.WindowPadding.y],
            [host_padding.x, host_padding.y],
            "popup padding must restore the caller's style"
        );
    });
    click_menu(&mut context, &mut workbench, 2.0); // Copy precedes Paste and Select All.
    assert_eq!(clipboard.borrow().text, "🙂é");
    assert_eq!(clipboard.borrow().writes, 1);
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        before
    );
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        "A🙂éZ\nsecond".as_bytes()
    );
    clipboard.borrow_mut().text = "Ω".to_owned();
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        false,
    );
    click_menu(&mut context, &mut workbench, 1.0); // Paste precedes Select All.
    assert_eq!(clipboard.borrow().reads, 1);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        "AΩZ\nsecond".as_bytes()
    );
    assert_eq!(std::fs::read(path).unwrap(), "A🙂éZ\nsecond".as_bytes());
    workbench.cleanup().unwrap();
}

#[test]
fn text_context_outside_selection_captures_utf8_caret_before_paste() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut context = context();
    let clipboard = Rc::new(RefCell::new(Clipboard {
        text: "!".into(),
        ..Clipboard::default()
    }));
    context.set_clipboard_backend(HostClipboard(Rc::clone(&clipboard)));
    let mut workbench = workspace(&dir, &mut context);
    workbench
        .open_or_focus(&dir.write("outside.txt", "A🙂éZ".as_bytes()))
        .unwrap();
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench
        .session
        .with_commands(view, |commands| {
            commands.set_selection(0, 0, 0, 1, CursorReveal::Ensure)
        })
        .unwrap();
    let point = text_point(&mut context, &mut workbench, view, 0, 5);
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        point,
        MouseButton::Right,
        false,
    );
    let cursor = workbench.session.view_snapshot(view).unwrap();
    assert_eq!((cursor.row, cursor.column), (0, 5));
    assert!(!cursor.has_selection());
    click_menu(&mut context, &mut workbench, 1.0);
    assert_eq!(clipboard.borrow().reads, 1);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        "A🙂!éZ".as_bytes()
    );
    workbench.cleanup().unwrap();
}

fn tree_frame(context: &mut Context, tree: &mut FileTree) -> (Vec<FileTreeAction>, [f32; 2], f32) {
    context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    let ui = context.frame();
    let mut actions = Vec::new();
    let mut origin = [0.0; 2];
    let font = ui.current_font_size();
    ui.window("Tree actions")
        .position([20.0, 20.0], Condition::Always)
        .size([400.0, 400.0], Condition::Always)
        .build(|| {
            origin = ui.cursor_screen_pos();
            actions = tree.display_actions(ui, "", &FileTreeStyle::default(), None, None);
        });
    drop(context.render_legacy());
    (actions, origin, font)
}
fn tree_mouse(
    context: &mut Context,
    tree: &mut FileTree,
    point: [f32; 2],
    button: MouseButton,
    down: bool,
) -> Vec<FileTreeAction> {
    context.io_mut().add_mouse_pos_event(point);
    context.io_mut().add_mouse_button_event(button, down);
    tree_frame(context, tree).0
}
fn tree_popup_item(context: &Context, index: usize, separators_before: usize) -> [f32; 2] {
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        let popup = &*native.OpenPopupStack.Data;
        let window = &*popup.Window;
        let font = sys::igGetFontSize();
        [
            window.DC.CursorStartPos.x + 24.0,
            window.DC.CursorStartPos.y
                + index as f32 * (font + 4.0)
                + separators_before as f32 * 5.0
                + font * 0.5,
        ]
    })
}
#[test]
fn file_tree_menus_dispatch_exact_paths_and_protect_project_root_actions() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"keep");
    let root = dir.path("");
    let mut tree = FileTree::default();
    tree.refresh_file_tree(root.to_str().unwrap()).unwrap();
    let mut context = context();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    tree_frame(&mut context, &mut tree);
    let (_, origin, font) = tree_frame(&mut context, &mut tree);
    // The first root label has its source row padding and icon reservation.
    let row_height = (font * 1.2).max(font * 0.88 + 2.0);
    let root_advance = row_height * 0.5 + font * 0.5 + font * 0.15;
    let file = [origin[0] + 80.0, origin[1] + root_advance + font * 0.5];
    assert!(tree_mouse(&mut context, &mut tree, file, MouseButton::Right, true).is_empty());
    assert!(tree_mouse(&mut context, &mut tree, file, MouseButton::Right, false).is_empty());
    tree_frame(&mut context, &mut tree);
    let rename = tree_popup_item(&context, 2, 1);
    assert!(tree_mouse(&mut context, &mut tree, rename, MouseButton::Left, true).is_empty());
    assert_eq!(
        tree_mouse(&mut context, &mut tree, rename, MouseButton::Left, false),
        vec![FileTreeAction::Rename(path.to_str().unwrap().into())]
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    // The root exposes creation and visibility controls, never destructive actions.
    let root_row = [origin[0] + 80.0, origin[1] + font * 0.5];
    tree_mouse(&mut context, &mut tree, root_row, MouseButton::Right, true);
    tree_mouse(&mut context, &mut tree, root_row, MouseButton::Right, false);
    tree_frame(&mut context, &mut tree);
    let create_folder = tree_popup_item(&context, 1, 0);
    tree_mouse(
        &mut context,
        &mut tree,
        create_folder,
        MouseButton::Left,
        true,
    );
    assert_eq!(
        tree_mouse(
            &mut context,
            &mut tree,
            create_folder,
            MouseButton::Left,
            false
        ),
        vec![FileTreeAction::NewFolder(root.to_str().unwrap().into())]
    );
    // Empty window background also binds creation to this same project root.
    let background = [origin[0] + 80.0, origin[1] + 240.0];
    tree_mouse(
        &mut context,
        &mut tree,
        background,
        MouseButton::Right,
        true,
    );
    tree_mouse(
        &mut context,
        &mut tree,
        background,
        MouseButton::Right,
        false,
    );
    tree_frame(&mut context, &mut tree);
    let create_file = tree_popup_item(&context, 0, 0);
    tree_mouse(
        &mut context,
        &mut tree,
        create_file,
        MouseButton::Left,
        true,
    );
    assert_eq!(
        tree_mouse(
            &mut context,
            &mut tree,
            create_file,
            MouseButton::Left,
            false
        ),
        vec![FileTreeAction::NewFile(root.to_str().unwrap().into())]
    );
    assert!(root.is_dir());
}

#[test]
fn file_tree_visibility_menus_toggle_hide_reveal_and_unhide_exact_paths() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("file.txt", b"keep");
    let path = path.to_str().unwrap().to_owned();
    let mut tree = FileTree::default();
    tree.refresh_file_tree(dir.root().to_str().unwrap())
        .unwrap();
    let mut context = context();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    tree_frame(&mut context, &mut tree);
    let (_, origin, font) = tree_frame(&mut context, &mut tree);
    let background = [origin[0] + 80.0, origin[1] + 240.0];
    fn click_item(
        context: &mut Context,
        tree: &mut FileTree,
        row: [f32; 2],
        index: usize,
        separators: usize,
    ) -> Vec<FileTreeAction> {
        tree_mouse(context, tree, row, MouseButton::Right, true);
        tree_mouse(context, tree, row, MouseButton::Right, false);
        tree_frame(context, tree);
        let point = tree_popup_item(context, index, separators);
        tree_mouse(context, tree, point, MouseButton::Left, true);
        tree_mouse(context, tree, point, MouseButton::Left, false)
    }
    // Clicking the checked menu again must emit the inverse state.
    for enabled in [true, false] {
        for (index, action) in [
            (2, FileTreeAction::SetHideGitignored(enabled)),
            (3, FileTreeAction::SetHideHidden(enabled)),
            (4, FileTreeAction::SetShowHidden(enabled)),
        ] {
            let actions = click_item(&mut context, &mut tree, background, index, 1);
            assert_eq!(actions.as_slice(), std::slice::from_ref(&action));
            assert!(tree.apply_visibility_action(&action, false));
        }
    }
    let row_height = (font * 1.2).max(font * 0.88 + 2.0);
    let root_advance = row_height * 0.5 + font * 0.5 + font * 0.15;
    let file_row = [origin[0] + 80.0, origin[1] + root_advance + font * 0.5];
    let hide = FileTreeAction::SetPathHidden {
        path: path.clone(),
        hidden: true,
    };
    assert_eq!(
        click_item(&mut context, &mut tree, file_row, 4, 1).as_slice(),
        std::slice::from_ref(&hide)
    );
    tree.apply_visibility_action(&hide, false);
    tree_frame(&mut context, &mut tree);
    let reveal = FileTreeAction::SetShowHidden(true);
    assert_eq!(
        click_item(&mut context, &mut tree, background, 4, 1).as_slice(),
        std::slice::from_ref(&reveal)
    );
    tree.apply_visibility_action(&reveal, false);
    tree_frame(&mut context, &mut tree);
    // The revealed label's vertex alpha is dimmed, while menu text is unaffected.
    let dimmed_vertices = context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(c"Tree actions".as_ptr());
        let vertices = &(*(*window).DrawList).VtxBuffer;
        std::slice::from_raw_parts(vertices.Data, vertices.Size as usize)
            .iter()
            .filter(|vertex| vertex.col >> 24 == 115 && vertex.pos.y >= file_row[1] - font)
            .count()
    });
    assert!(dimmed_vertices > 0, "revealed names must be visibly dimmed");
    let unhide = FileTreeAction::SetPathHidden {
        path: path.clone(),
        hidden: false,
    };
    assert_eq!(
        click_item(&mut context, &mut tree, file_row, 4, 1).as_slice(),
        std::slice::from_ref(&unhide)
    );
    tree.apply_visibility_action(&unhide, false);
    assert!(tree.preferences.hidden_paths.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), b"keep");
}
