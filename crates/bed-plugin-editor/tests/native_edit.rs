#![cfg(target_os = "macos")]
use dear_imgui_rs::Context;
use std::path::PathBuf;
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
#[test]
fn native_edit_shortcuts_undo_and_redo_once_without_inserting_characters() {
    use bed_editor_widgets::{EditorView, EditorViewOptions};
    use dear_imgui_rs::{Condition, FramePrepareOptions};
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut session = bed_documents::DocumentSession::new();
    let document = session.create_document(b"baseline").unwrap();
    let mut view = EditorView::new(&mut session, document).unwrap();
    session
        .with_commands(view.id(), |commands| commands.type_text(b"!"))
        .unwrap();
    session
        .with_commands(view.id(), |commands| {
            // Nonadjacent edits stay separate, making repeated actions observable.
            commands.move_doc_end(false);
            commands.type_text(b"?");
        })
        .unwrap();
    session.request_focus(view.id()).unwrap();
    let mut render = |context: &mut Context, session: &mut bed_documents::DocumentSession| {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Native menu editing")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                let response = view
                    .draw(ui, session, &EditorViewOptions::default())
                    .unwrap();
                assert!(response.actions.is_empty());
            });
        drop(context.render_legacy());
    };
    render(&mut context, &mut session);
    assert_eq!(session.snapshot(document).unwrap().bytes, b"!baseline?");
    bed_core::queue_native_shortcut(
        &mut context,
        "Primary+Z"
            .parse::<bed_keybindings::Stroke>()
            .unwrap()
            .resolved(bed_keybindings::Platform::current()),
    );
    for _ in 0..4 {
        render(&mut context, &mut session);
    }
    assert_eq!(session.snapshot(document).unwrap().bytes, b"!baseline");
    bed_core::queue_native_shortcut(
        &mut context,
        "Primary+Z"
            .parse::<bed_keybindings::Stroke>()
            .unwrap()
            .resolved(bed_keybindings::Platform::current()),
    );
    for _ in 0..4 {
        render(&mut context, &mut session);
    }
    assert_eq!(session.snapshot(document).unwrap().bytes, b"baseline");
    bed_core::queue_native_shortcut(
        &mut context,
        "Primary+Shift+Z"
            .parse::<bed_keybindings::Stroke>()
            .unwrap()
            .resolved(bed_keybindings::Platform::current()),
    );
    for _ in 0..4 {
        render(&mut context, &mut session);
    }
    assert_eq!(session.snapshot(document).unwrap().bytes, b"!baseline");
    bed_core::queue_native_shortcut(
        &mut context,
        "Primary+Shift+Z"
            .parse::<bed_keybindings::Stroke>()
            .unwrap()
            .resolved(bed_keybindings::Platform::current()),
    );
    for _ in 0..4 {
        render(&mut context, &mut session);
    }
    assert_eq!(session.snapshot(document).unwrap().bytes, b"!baseline?");
    assert!(!context.io().key_ctrl());
    assert!(!context.io().key_super());
    assert!(!context.io().key_shift());
}
