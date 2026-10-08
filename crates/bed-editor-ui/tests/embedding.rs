//! Native host-frame checks for document-view embedding (a Bed extension).
use bed_document_session::{ClosePolicy, EditorSession};
use bed_editing::editor_commands::CursorReveal;
use bed_editor_ui::{EditorView, EditorViewOptions, editor_input::HostAction};
use dear_imgui_rs::{
    BackendFlags, ClipboardBackend, Condition, ConfigFlags, Context, FontSource,
    FramePrepareOptions, Key, StyleColor, Ui, WindowFlags, sys,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Mutex};
static IMGUI_TEST_LOCK: Mutex<()> = Mutex::new(());
fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .add_font(&[FontSource::default_font_with_size(15.0)]);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}
fn sources(context: &Context) -> i32 {
    context
        .binding()
        .with_bound_context(|| unsafe { (*(*sys::igGetIO_Nil()).Fonts).Sources.Size })
}
#[derive(Debug, PartialEq, Eq)]
struct Stacks {
    font: i32,
    colors: i32,
    style: i32,
    windows: i32,
    ids: i32,
    frame: i32,
    ended: i32,
    rendered: i32,
}
fn stacks(ui: &Ui) -> Stacks {
    ui.with_bound_context(|| unsafe {
        let c = &*sys::igGetCurrentContext();
        Stacks {
            font: c.FontStack.Size,
            colors: c.ColorStack.Size,
            style: c.StyleVarStack.Size,
            windows: c.CurrentWindowStack.Size,
            ids: (*sys::igGetCurrentWindowRead()).IDStack.Size,
            frame: c.FrameCount,
            ended: c.FrameCountEnded,
            rendered: c.FrameCountRendered,
        }
    })
}
fn frame(
    context: &mut Context,
    session: &mut EditorSession,
    left: &mut EditorView,
    right: &mut EditorView,
) -> Vec<(u64, Vec<HostAction>)> {
    context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
    let ui = context.frame();
    let mut actions = Vec::new();
    ui.window("Host layout")
        .position([0.0, 0.0], Condition::Always)
        .size([980.0, 650.0], Condition::Always)
        .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
        .build(|| {
            ui.text("Host content before widgets");
            let before = stacks(ui);
            for view in [left, right] {
                let response = view
                    .draw(
                        ui,
                        session,
                        &EditorViewOptions {
                            size: [450.0, 560.0],
                            ..EditorViewOptions::default()
                        },
                    )
                    .unwrap();
                actions.push((response.view.0, response.actions));
                ui.same_line();
            }
            ui.new_line();
            assert_eq!(stacks(ui), before);
            ui.text("Host content after widgets");
        });
    assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    actions
}
fn shortcut(
    context: &mut Context,
    session: &mut EditorSession,
    left: &mut EditorView,
    right: &mut EditorView,
    key: Key,
) -> Vec<(u64, Vec<HostAction>)> {
    context.io_mut().add_key_event(Key::ModCtrl, true);
    context.io_mut().add_key_event(key, true);
    let actions = frame(context, session, left, right);
    context.io_mut().add_key_event(key, false);
    context.io_mut().add_key_event(Key::ModCtrl, false);
    frame(context, session, left, right);
    actions
}

#[test]
fn construction_and_draw_preserve_host_context_fonts_style_flags_and_frame() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    context
        .io_mut()
        .set_backend_flags(BackendFlags::HAS_MOUSE_CURSORS);
    context
        .io_mut()
        .set_config_flags(ConfigFlags::NAV_ENABLE_KEYBOARD);
    context.style_mut().set_font_size_base(25.0);
    context
        .style_mut()
        .set_color(StyleColor::WindowBg, [0.11, 0.22, 0.33, 0.44]);
    let before = context.frame_lifecycle_stamp();
    let count = sources(&context);
    let mut session = EditorSession::new();
    let doc = session
        .create_document("host text 🙂\nnext".as_bytes())
        .unwrap();
    let mut left = EditorView::new(&mut session, doc).unwrap();
    let mut right = EditorView::new(&mut session, doc).unwrap();
    assert_eq!(context.frame_lifecycle_stamp(), before);
    assert_eq!(sources(&context), count);
    assert_eq!(session.view_count(doc), 2);
    frame(&mut context, &mut session, &mut left, &mut right);
    assert_eq!(sources(&context), count);
    assert_eq!(context.style().font_size_base(), 25.0);
    assert_eq!(
        context.style().color(StyleColor::WindowBg),
        [0.11, 0.22, 0.33, 0.44]
    );
    assert_eq!(
        context.io().config_flags(),
        ConfigFlags::NAV_ENABLE_KEYBOARD
    );
    assert_eq!(
        context.io().backend_flags(),
        BackendFlags::HAS_MOUSE_CURSORS
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
    assert_eq!(sources(&context), count);
}

#[test]
fn same_parent_widgets_receive_unicode_and_find_only_in_focused_view() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let mut session = EditorSession::new();
    let doc = session.create_document(b"alpha\nline").unwrap();
    let mut left = EditorView::new(&mut session, doc).unwrap();
    let mut right = EditorView::new(&mut session, doc).unwrap();
    session
        .with_commands(left.id(), |c| {
            c.set_selection(0, 0, 0, 5, CursorReveal::Ensure)
        })
        .unwrap();
    session
        .with_commands(right.id(), |c| {
            c.set_cursor(1, 4, false, CursorReveal::Ensure)
        })
        .unwrap();
    session.request_focus(right.id()).unwrap();
    frame(&mut context, &mut session, &mut left, &mut right);
    frame(&mut context, &mut session, &mut left, &mut right);
    let left_selection = session.view_snapshot(left.id()).unwrap().selections;
    context.io_mut().add_input_characters_utf8("🙂");
    frame(&mut context, &mut session, &mut left, &mut right);
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "alpha\nline🙂".as_bytes()
    );
    assert_eq!(
        session.view_snapshot(left.id()).unwrap().selections,
        left_selection
    );
    assert_eq!(session.view_snapshot(right.id()).unwrap().column, 8);
    shortcut(&mut context, &mut session, &mut left, &mut right, Key::F);
    assert!(!left.find_visible());
    assert!(right.find_visible());
    shortcut(
        &mut context,
        &mut session,
        &mut left,
        &mut right,
        Key::Escape,
    );
    assert!(!right.find_visible());
    session.request_focus(left.id()).unwrap();
    frame(&mut context, &mut session, &mut left, &mut right);
    context.io_mut().add_input_characters_utf8("é");
    frame(&mut context, &mut session, &mut left, &mut right);
    assert_eq!(session.snapshot(doc).unwrap().bytes, "é\nline🙂".as_bytes());
}

#[derive(Default)]
struct Clipboard {
    value: String,
    gets: usize,
    sets: usize,
}
struct HostClipboard(Rc<RefCell<Clipboard>>);
impl ClipboardBackend for HostClipboard {
    fn get(&mut self) -> Option<String> {
        let mut c = self.0.borrow_mut();
        c.gets += 1;
        Some(c.value.clone())
    }
    fn set(&mut self, text: &str) {
        let mut c = self.0.borrow_mut();
        c.sets += 1;
        c.value = text.to_owned();
    }
}
#[test]
fn clipboard_and_save_actions_use_host_hooks_and_focus() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let clipboard = Rc::new(RefCell::new(Clipboard::default()));
    context.set_clipboard_backend(HostClipboard(Rc::clone(&clipboard)));
    let mut session = EditorSession::new();
    let doc = session.create_document(b"alpha").unwrap();
    let mut left = EditorView::new(&mut session, doc).unwrap();
    let mut right = EditorView::new(&mut session, doc).unwrap();
    session
        .with_commands(left.id(), |c| c.select_all())
        .unwrap();
    session.request_focus(left.id()).unwrap();
    frame(&mut context, &mut session, &mut left, &mut right);
    frame(&mut context, &mut session, &mut left, &mut right);
    shortcut(&mut context, &mut session, &mut left, &mut right, Key::C);
    assert_eq!(clipboard.borrow().value, "alpha");
    assert_eq!(clipboard.borrow().sets, 1);
    clipboard.borrow_mut().value = "HOST🙂".into();
    session
        .with_commands(right.id(), |c| c.move_doc_end(false))
        .unwrap();
    session.request_focus(right.id()).unwrap();
    frame(&mut context, &mut session, &mut left, &mut right);
    shortcut(&mut context, &mut session, &mut left, &mut right, Key::V);
    assert_eq!(
        session.snapshot(doc).unwrap().bytes,
        "alphaHOST🙂".as_bytes()
    );
    assert_eq!(clipboard.borrow().gets, 1);
    let actions = shortcut(&mut context, &mut session, &mut left, &mut right, Key::S);
    assert_eq!(
        actions,
        vec![
            (left.id().0, vec![]),
            (right.id().0, vec![HostAction::Save])
        ]
    );
    assert!(session.snapshot(doc).unwrap().path.is_empty());
    assert!(session.snapshot(doc).unwrap().dirty);
}

#[test]
fn widget_drop_detaches_without_closing_document_and_new_context_needs_new_view() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut session = EditorSession::new();
    let doc = session.create_document(b"buffer").unwrap();
    let mut left = EditorView::new(&mut session, doc).unwrap();
    let mut right = EditorView::new(&mut session, doc).unwrap();
    let mut old_context = context();
    frame(&mut old_context, &mut session, &mut left, &mut right);
    let old_id = left.context_id();
    drop(right);
    assert_eq!(session.view_count(doc), 1);
    drop(old_context);
    let mut next_context = context();
    assert_ne!(Some(next_context.id()), old_id);
    next_context.prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
    let ui = next_context.frame();
    ui.window("New host").build(|| {
        let before = stacks(ui);
        assert!(
            left.draw(ui, &mut session, &EditorViewOptions::default())
                .is_err()
        );
        assert_eq!(stacks(ui), before);
    });
    let _ = next_context.render_legacy();
    drop(left);
    assert_eq!(session.view_count(doc), 0);
    assert_eq!(session.snapshot(doc).unwrap().bytes, b"buffer");
    session.tick();
    let mut new_left = EditorView::new(&mut session, doc).unwrap();
    let mut new_right = EditorView::new(&mut session, doc).unwrap();
    frame(
        &mut next_context,
        &mut session,
        &mut new_left,
        &mut new_right,
    );
}

#[test]
fn commands_for_a_different_session_and_detached_views_are_rejected() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let mut a = EditorSession::new();
    let da = a.create_document(b"A").unwrap();
    let mut view = EditorView::new(&mut a, da).unwrap();
    let mut b = EditorSession::new();
    let db = b.create_document(b"B").unwrap();
    context.prepare_frame(FramePrepareOptions::new([500.0, 500.0], 1.0 / 60.0));
    let ui = context.frame();
    ui.window("Host").build(|| {
        assert!(
            view.draw(ui, &mut b, &EditorViewOptions::default())
                .is_err()
        );
    });
    let _ = context.render_legacy();
    assert_eq!(a.snapshot(da).unwrap().bytes, b"A");
    assert_eq!(b.snapshot(db).unwrap().bytes, b"B");
    a.close_document(da, ClosePolicy::Discard).unwrap();
    assert!(view.open_find(&mut a).is_err());
}
