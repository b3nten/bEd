//! Native mouse/menu regressions for the standalone shell's new context actions.
use super::*;
use crate::test_support::TempDir;
use bed_editor_ui::views::view_layout::{ViewLayout, line_column_x};
use bed_module_explorer::file_tree::{FileTree, FileTreeAction, FileTreeStyle};
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
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
    let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
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
        .find_map(|tab| {
            tab.panel
                .editor()
                .filter(|view| view.id() == view_id)
                .map(|view| view.presentation().layout)
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

#[test]
fn editor_zoom_commands_and_shortcuts_only_change_the_focused_text_view() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("zoom.txt", b"shared text\nsecond line");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.open_or_focus(&path).unwrap();
    let first = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::SplitRight).unwrap();
    let second = workbench.active_view().unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert!(workbench.dispatch_command("bed.editor.zoom_in").unwrap());
    let zoom = |workbench: &Workbench, id| {
        workbench
            .tabs
            .iter()
            .find_map(|tab| tab.panel.editor().filter(|view| view.id() == id))
            .unwrap()
            .zoom()
    };
    assert_eq!(zoom(&workbench, first), 1.0);
    assert!((zoom(&workbench, second) - 1.1).abs() < 0.00001);
    assert!(workbench.dispatch_command("bed.editor.reset_zoom").unwrap());
    frame(&mut context, &mut workbench);
    assert!(zoom(&workbench, second) > 1.0 && zoom(&workbench, second) < 1.1);
    for _ in 0..6 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(zoom(&workbench, second), 1.0);

    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    workbench
        .session
        .with_commands(second, |commands| {
            commands.set_selection(0, 0, 0, 6, CursorReveal::Ensure)
        })
        .unwrap();
    let before = workbench.active_snapshot().unwrap();
    let selection = workbench.session.view_snapshot(second).unwrap().selections;
    for (key, shift, text, expected) in [
        (Key::Equal, false, '=', 1.1),
        (Key::Equal, true, '+', 1.2),
        (Key::Minus, false, '-', 1.1),
        (Key::KeypadAdd, false, '+', 1.2),
        (Key::KeypadSubtract, false, '-', 1.1),
    ] {
        context.io_mut().add_key_event(modifier, true);
        context.io_mut().add_key_event(Key::ModShift, shift);
        // The native backend queues the character before the corresponding key.
        context.io_mut().add_input_character(text);
        context.io_mut().add_key_event(key, true);
        frame(&mut context, &mut workbench);
        assert!((zoom(&workbench, second) - expected).abs() < 0.00001);
        context.io_mut().add_key_event(key, false);
        context.io_mut().add_key_event(modifier, false);
        context.io_mut().add_key_event(Key::ModShift, false);
        frame(&mut context, &mut workbench);
        assert_eq!(workbench.active_snapshot().unwrap(), before);
        assert_eq!(
            workbench.session.view_snapshot(second).unwrap().selections,
            selection
        );
    }
    assert_eq!(zoom(&workbench, first), 1.0);
    workbench
        .session
        .with_commands(second, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.active_snapshot().unwrap(),
        before,
        "zoom shortcuts leave no edits to undo"
    );
    context.io_mut().add_key_event(Key::Minus, true);
    context.io_mut().add_input_character('-');
    frame(&mut context, &mut workbench);
    context.io_mut().add_key_event(Key::Minus, false);
    frame(&mut context, &mut workbench);
    assert!((zoom(&workbench, second) - 1.1).abs() < 0.00001);
    assert_ne!(
        workbench.active_snapshot().unwrap().bytes,
        b"shared text\nsecond line"
    );
    workbench
        .session
        .with_commands(second, |commands| commands.undo())
        .unwrap();

    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert!(!workbench.dispatch_command("bed.editor.zoom_in").unwrap());
    assert!((zoom(&workbench, second) - 1.1).abs() < 0.00001);
    workbench.cleanup().unwrap();
}

#[test]
fn editor_zoom_consumes_repeated_characters_at_limits_and_preserves_other_text() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("repeat.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let before = workbench.active_snapshot().unwrap();
    let selection = workbench.session.view_snapshot(view).unwrap().selections;
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    for (command, key, text, limit) in [
        ("bed.editor.zoom_in", Key::Equal, '=', 1.4),
        ("bed.editor.zoom_out", Key::Minus, '-', 0.6),
    ] {
        for _ in 0..10 {
            assert!(workbench.dispatch_command(command).unwrap());
        }
        context.io_mut().add_key_event(modifier, true);
        // Repeated native key presses keep the key down while producing text.
        // Cover frames both before and after ImGui's own repeat delay.
        for _ in 0..60 {
            context.io_mut().add_input_character(text);
            context.io_mut().add_key_event(key, true);
            frame(&mut context, &mut workbench);
            let zoom = workbench
                .tabs
                .iter()
                .find_map(|tab| tab.panel.editor().filter(|editor| editor.id() == view))
                .unwrap()
                .zoom();
            assert!((zoom - limit).abs() < 0.00001);
            assert_eq!(workbench.active_snapshot().unwrap(), before);
            assert_eq!(
                workbench.session.view_snapshot(view).unwrap().selections,
                selection
            );
        }
        context.io_mut().add_key_event(key, false);
        context.io_mut().add_key_event(modifier, false);
        frame(&mut context, &mut workbench);
    }

    context.io_mut().add_key_event(modifier, true);
    context.io_mut().add_input_characters_utf8("é=雪+🙂-");
    context.io_mut().add_key_event(Key::Equal, true);
    frame(&mut context, &mut workbench);
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        "é雪🙂-original".as_bytes(),
        "only characters belonging to the held zoom chord are consumed"
    );
    context.io_mut().add_key_event(Key::Equal, false);
    context.io_mut().add_key_event(modifier, false);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("=+-");
    context.io_mut().add_key_event(Key::Equal, true);
    frame(&mut context, &mut workbench);
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        "é雪🙂-=+-original".as_bytes(),
        "zoom keys still type normally without the primary modifier"
    );
    context.io_mut().add_key_event(Key::Equal, false);
    frame(&mut context, &mut workbench);
    workbench.cleanup().unwrap();
}

#[test]
fn editor_context_reset_uses_its_own_view_and_remains_available_read_only() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    let document = workbench
        .session
        .create_snapshot_document(b"text\nsecond line", "text")
        .unwrap();
    workbench
        .add_document_panel(document, Some(bed_module_editor::TEXT_VIEWER), &Value::Null)
        .unwrap();
    let first = workbench.active_view().unwrap();
    workbench.dispatch_command("bed.editor.zoom_in").unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::SplitRight).unwrap();
    let second = workbench.active_view().unwrap();
    workbench.dispatch_command("bed.editor.zoom_in").unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    let pane = layout(&mut workbench, first);
    let point = [
        pane.text_pos[0] + 2.0,
        pane.text_pos[1] + pane.line_height * 0.5,
    ];
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
    frame(&mut context, &mut workbench);
    context.binding().with_bound_context(|| unsafe {
        assert_eq!(
            (*sys::igGetCurrentContext()).OpenPopupStack.Size,
            1,
            "right-clicking the zoomed read-only editor opens its menu"
        );
    });
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    context.io_mut().add_key_event(modifier, true);
    context.io_mut().add_key_event(Key::Equal, true);
    frame(&mut context, &mut workbench);
    context.io_mut().add_key_event(Key::Equal, false);
    context.io_mut().add_key_event(modifier, false);
    frame(&mut context, &mut workbench);
    assert!(
        (workbench
            .tabs
            .iter()
            .find_map(|tab| tab.panel.editor().filter(|view| view.id() == first))
            .unwrap()
            .zoom()
            - 1.1)
            .abs()
            < 0.00001
    );
    // Reset precedes six edit actions and their two separators.
    let mut reset = popup_point(&context, 6.0);
    reset[1] -= (context.style().item_spacing()[1] + 1.0) * 2.0;
    mouse(&mut context, &mut workbench, reset, MouseButton::Left, true);
    mouse(
        &mut context,
        &mut workbench,
        reset,
        MouseButton::Left,
        false,
    );
    let zoom = |workbench: &Workbench, id| {
        workbench
            .tabs
            .iter()
            .find_map(|tab| tab.panel.editor().filter(|view| view.id() == id))
            .unwrap()
            .zoom()
    };
    assert!(zoom(&workbench, first) > 1.0);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert!(zoom(&workbench, first) > 1.0 && zoom(&workbench, first) < 1.1);
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(zoom(&workbench, first), 1.0);
    assert!((zoom(&workbench, second) - 1.1).abs() < 0.00001);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"text\nsecond line"
    );
    workbench.cleanup().unwrap();
}

#[test]
fn editor_pinch_keeps_the_starting_view_and_ignores_invalid_or_ended_input() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("zoom.txt", b"text\nsecond line");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.open_or_focus(&path).unwrap();
    let first = workbench.active_view().unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.dispatch(WindowCommand::SplitRight).unwrap();
    let second = workbench.active_view().unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    let pane = layout(&mut workbench, first);
    let point = [
        pane.text_pos[0] + 10.0,
        pane.text_pos[1] + pane.line_height * 0.5,
    ];
    let viewport = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.view_id() == Some(first))
        .unwrap()
        .viewport;
    context.io_mut().add_mouse_pos_event(point);
    workbench.queue_editor_pinch(viewport, point, 0.25, EditorPinchPhase::Started);
    frame(&mut context, &mut workbench);
    let starting_zoom = workbench
        .tabs
        .iter()
        .find_map(|tab| tab.panel.editor().filter(|view| view.id() == first))
        .unwrap()
        .zoom();
    assert!(
        (starting_zoom - 1.25).abs() < 0.00001,
        "pinch begins in the editor under the pointer: {starting_zoom}"
    );
    let other = layout(&mut workbench, second);
    let point = [
        other.text_pos[0] + 10.0,
        other.text_pos[1] + other.line_height * 0.5,
    ];
    context.io_mut().add_mouse_pos_event(point);
    for delta in [f64::NAN, f64::INFINITY, -2.0] {
        workbench.queue_editor_pinch(viewport, point, delta, EditorPinchPhase::Moved);
    }
    workbench.queue_editor_pinch(viewport, point, 0.2, EditorPinchPhase::Moved);
    workbench.queue_editor_pinch(viewport, point, 0.0, EditorPinchPhase::Ended);
    workbench.queue_editor_pinch(viewport, point, 1.0, EditorPinchPhase::Moved);
    frame(&mut context, &mut workbench);
    let zoom = |workbench: &Workbench, id| {
        workbench
            .tabs
            .iter()
            .find_map(|tab| tab.panel.editor().filter(|view| view.id() == id))
            .unwrap()
            .zoom()
    };
    assert_eq!(zoom(&workbench, first), 1.4);
    assert_eq!(zoom(&workbench, second), 1.0);

    workbench.queue_editor_pinch(viewport, point, 0.1, EditorPinchPhase::Started);
    workbench.queue_editor_pinch(viewport, point, 1.0, EditorPinchPhase::Cancelled);
    workbench.queue_editor_pinch(viewport, point, 1.0, EditorPinchPhase::Moved);
    frame(&mut context, &mut workbench);
    assert!((zoom(&workbench, second) - 1.1).abs() < 0.00001);

    workbench.queue_editor_pinch(viewport, point, 0.0, EditorPinchPhase::Started);
    workbench.queue_editor_pinch(viewport, point, -0.9, EditorPinchPhase::Moved);
    workbench.queue_editor_pinch(viewport, point, 0.0, EditorPinchPhase::Ended);
    frame(&mut context, &mut workbench);
    assert_eq!(zoom(&workbench, second), 0.6);

    workbench.queue_editor_pinch(viewport, point, 0.0, EditorPinchPhase::Started);
    frame(&mut context, &mut workbench);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.view_id() == Some(second))
        .unwrap();
    workbench.close_tab(index).unwrap();
    workbench.queue_editor_pinch(viewport, point, 0.5, EditorPinchPhase::Moved);
    frame(&mut context, &mut workbench);
    assert_eq!(zoom(&workbench, first), 1.4);
    workbench.cleanup().unwrap();
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
    let rename = tree_popup_item(&context, 7, 1);
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
            (4, FileTreeAction::SetHideGitignored(enabled)),
            (5, FileTreeAction::SetHideHidden(enabled)),
            (6, FileTreeAction::SetShowHidden(enabled)),
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
        click_item(&mut context, &mut tree, file_row, 9, 1).as_slice(),
        std::slice::from_ref(&hide)
    );
    tree.apply_visibility_action(&hide, false);
    tree_frame(&mut context, &mut tree);
    let reveal = FileTreeAction::SetShowHidden(true);
    assert_eq!(
        click_item(&mut context, &mut tree, background, 6, 1).as_slice(),
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
        click_item(&mut context, &mut tree, file_row, 9, 1).as_slice(),
        std::slice::from_ref(&unhide)
    );
    tree.apply_visibility_action(&unhide, false);
    assert!(tree.preferences.hidden_paths.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), b"keep");
}

fn file_dialog_frame(context: &mut Context, workbench: &mut Workbench, host: &str, size: [f32; 2]) {
    context.prepare_frame(FramePrepareOptions::new(size, 1.0 / 60.0));
    let ui = context.frame();
    ui.window(host)
        .flags(WindowFlags::NO_DECORATION | WindowFlags::NO_SAVED_SETTINGS)
        .position([0.0; 2], Condition::Always)
        .size(size, Condition::Always)
        .build(|| assert!(workbench.render(ui).unwrap().is_empty()));
    drop(context.render_legacy());
}

fn visible_file_dialog(context: &Context) -> ([f32; 2], [f32; 2], f32) {
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1, "one active file modal");
        let window = &*(*native.OpenPopupStack.Data).Window;
        assert!(window.Active && !window.Hidden && !window.Collapsed);
        assert!(
            CStr::from_ptr(window.Name)
                .to_string_lossy()
                .contains("###bed_file_action")
        );
        assert_eq!(
            CStr::from_ptr((*window.ParentWindowInBeginStack).Name).to_bytes(),
            b"##bed_workspace",
            "the modal belongs to the stable workspace, not its source tab"
        );
        (
            [window.Pos.x, window.Pos.y],
            [window.Size.x, window.Size.y],
            window.FontRefSize,
        )
    })
}

#[test]
fn file_action_dialog_survives_hidden_origin_and_host_changes_and_escape_restores_typing() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("editor.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench
        .session
        .with_commands(view, |commands| {
            commands.set_selection(0, 0, 0, 4, CursorReveal::Ensure)
        })
        .unwrap();
    let selection = workbench.session.view_snapshot(view).unwrap().selections;
    workbench
        .handle_tree_action(FileTreeAction::NewFile(dir.root().to_str().unwrap().into()))
        .unwrap();
    // The originating tool need not remain visible, and an embedding host may
    // change the current window between frames while the modal is open.
    workbench.dispatch(WindowCommand::Settings).unwrap();
    for host in ["Caller A", "Caller B", "Caller A"] {
        file_dialog_frame(&mut context, &mut workbench, host, [1200.0, 800.0]);
    }
    let (position, size, fs) = visible_file_dialog(&context);
    assert!(size[0] >= fs * 29.0 && size[1] >= fs * 8.0);
    assert!((position[0] + size[0] * 0.5 - 600.0).abs() < 2.0);
    assert!((position[1] + size[1] * 0.5 - 400.0).abs() < 2.0);
    assert!(workbench.file_dialog.as_ref().unwrap().visible);
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_ne!(native.InputTextState.ID, 0);
        assert_eq!(
            native.ActiveId, native.InputTextState.ID,
            "name receives keyboard focus"
        );
    });
    context.io_mut().add_input_characters_utf8("draft.rs");
    file_dialog_frame(&mut context, &mut workbench, "Caller B", [1200.0, 800.0]);
    assert_eq!(workbench.file_dialog.as_ref().unwrap().name, "draft.rs");
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"original"
    );
    context.io_mut().add_key_event(Key::Escape, true);
    file_dialog_frame(&mut context, &mut workbench, "Caller A", [1200.0, 800.0]);
    assert!(workbench.file_dialog.is_none());
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        selection
    );
    context.io_mut().add_key_event(Key::Escape, false);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("x");
    frame(&mut context, &mut workbench);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"xinal"
    );
    assert!(!dir.path("draft.rs").exists());
}

#[test]
fn file_action_dialog_menu_cancel_close_and_reopen_do_not_leave_modal_blockers() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("editor.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.set_project(dir.root()).unwrap();
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    workbench.show_tool(Tool::Explorer);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let explorer = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    let title = CString::new(workbench.title(explorer)).unwrap();
    let row = context.binding().with_bound_context(|| unsafe {
        let window = &*sys::igFindWindowByName(title.as_ptr());
        [
            window.DC.CursorStartPos.x + 75.0,
            window.DC.CursorStartPos.y + window.FontRefSize * 0.5,
        ]
    });
    mouse(&mut context, &mut workbench, row, MouseButton::Right, true);
    mouse(&mut context, &mut workbench, row, MouseButton::Right, false);
    frame(&mut context, &mut workbench);
    let create = tree_popup_item(&context, 0, 0);
    mouse(
        &mut context,
        &mut workbench,
        create,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        create,
        MouseButton::Left,
        false,
    );
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    visible_file_dialog(&context);
    let cancel = button_point(&context, "New file###bed_file_action", false);
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
    assert!(workbench.file_dialog.is_none());
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    for close_with_x in [true, false] {
        workbench
            .handle_tree_action(FileTreeAction::NewFile(dir.root().to_str().unwrap().into()))
            .unwrap();
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
        let (position, size, fs) = visible_file_dialog(&context);
        if close_with_x {
            let point = [position[0] + size[0] - fs * 1.05, position[1] + fs * 0.8];
            mouse(&mut context, &mut workbench, point, MouseButton::Left, true);
            mouse(
                &mut context,
                &mut workbench,
                point,
                MouseButton::Left,
                false,
            );
        } else {
            // Native closure must clear application state as well, even if a
            // different popup or host caused it before this frame.
            context
                .binding()
                .with_bound_context(|| unsafe { sys::igClosePopupToLevel(0, true) });
            frame(&mut context, &mut workbench);
        }
        assert!(workbench.file_dialog.is_none());
        assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
        context.binding().with_bound_context(|| unsafe {
            assert_eq!((*sys::igGetCurrentContext()).OpenPopupStack.Size, 0);
        });
    }
    workbench
        .handle_tree_action(FileTreeAction::NewFile(dir.root().to_str().unwrap().into()))
        .unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("created.txt");
    frame(&mut context, &mut workbench);
    let create = button_point(&context, "New file###bed_file_action", true);
    mouse(
        &mut context,
        &mut workbench,
        create,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        create,
        MouseButton::Left,
        false,
    );
    assert!(workbench.file_dialog.is_none());
    assert!(dir.path("created.txt").is_file());
    let created = workbench.active_snapshot().unwrap();
    assert!(created.path.ends_with("created.txt"));
    assert!(created.bytes.is_empty());
}

#[test]
fn file_action_dialog_stays_inside_small_viewport_below_titlebar() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("editor.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.root_top_inset = 35.0;
    workbench.open_or_focus(&path).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::NewFile(dir.root().to_str().unwrap().into()))
        .unwrap();
    for _ in 0..3 {
        file_dialog_frame(&mut context, &mut workbench, "Small host", [360.0, 260.0]);
    }
    let (position, size, _) = visible_file_dialog(&context);
    assert!(position[0] >= 0.0 && position[1] >= 35.0);
    assert!(position[0] + size[0] <= 360.0 && position[1] + size[1] <= 260.0);
}

#[test]
fn file_action_dialog_keeps_drafted_name_and_focus_when_background_error_arrives() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("editor.txt", b"original");
    let mut context = context();
    let mut workbench = workspace(&dir, &mut context);
    workbench.settings.settings["ui_animations"] = json!(false);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench
        .handle_tree_action(FileTreeAction::NewFile(dir.root().to_str().unwrap().into()))
        .unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    context.io_mut().add_input_characters_utf8("draft.rs");
    frame(&mut context, &mut workbench);
    workbench.error = Some("Background service could not refresh".into());
    for _ in 0..2 {
        frame(&mut context, &mut workbench);
        visible_file_dialog(&context);
        assert_eq!(workbench.file_dialog.as_ref().unwrap().name, "draft.rs");
        context.binding().with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            assert_eq!(native.NavWindow, (*native.OpenPopupStack.Data).Window);
            assert_eq!(native.ActiveId, native.InputTextState.ID);
            assert_ne!(native.ActiveId, 0, "the name input keeps keyboard focus");
        });
    }
    context.io_mut().add_input_characters_utf8(".tmp");
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.file_dialog.as_ref().unwrap().name, "draft.rs.tmp");
    context.io_mut().add_key_event(Key::Escape, true);
    frame(&mut context, &mut workbench);
    assert!(workbench.file_dialog.is_none());
    assert!(
        workbench.error.is_some(),
        "Escape dismisses only the active modal"
    );
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    context.io_mut().add_key_event(Key::Escape, false);
    frame(&mut context, &mut workbench);
    let dismiss = button_point(&context, "Error", false);
    mouse(
        &mut context,
        &mut workbench,
        dismiss,
        MouseButton::Left,
        true,
    );
    mouse(
        &mut context,
        &mut workbench,
        dismiss,
        MouseButton::Left,
        false,
    );
    frame(&mut context, &mut workbench);
    assert!(workbench.error.is_none());
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"original");
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    assert!(!dir.path("draft.rs.tmp").exists());
}

fn text_lsp_actions(ui: &Ui, ready: bool) -> Option<u8> {
    let mut action = None;
    for (title, command) in [
        ("Go to Definition", 0),
        ("Find References", 1),
        ("Symbol Info", 2),
    ] {
        if ui.menu_item_enabled_selected_no_shortcut(title, false, ready) {
            action = Some(command);
        }
    }
    action
}
