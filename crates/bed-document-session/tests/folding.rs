use bed_document_session::EditorSession;
use bed_editing::{editor_commands::CursorReveal, folding::FoldRange};

#[test]
fn folds_are_independent_between_views_and_shift_once_on_edit_undo_and_redo() {
    let mut session = EditorSession::new();
    let document = session
        .create_document(b"before\nfn outer() {\n  a\n  b\n}\nafter")
        .unwrap();
    let a = session.create_view(document).unwrap();
    let b = session.create_view(document).unwrap();
    let initial = FoldRange {
        start_line: 1,
        end_line: 4,
    };
    for view in [a, b] {
        session
            .with_view(view, |editor| {
                editor.view_mut().folds.set_ranges(vec![initial])
            })
            .unwrap();
    }
    session
        .with_commands(a, |commands| commands.toggle_fold(1))
        .unwrap();
    assert!(session.view_snapshot(a).unwrap().folds.is_hidden(2));
    assert!(!session.view_snapshot(b).unwrap().folds.is_hidden(2));
    session
        .with_commands(b, |commands| commands.fold_all())
        .unwrap();
    session
        .with_commands(a, |commands| commands.type_text(b"inserted\r\n"))
        .unwrap();
    let shifted = FoldRange {
        start_line: 2,
        end_line: 5,
    };
    for view in [a, b] {
        assert_eq!(
            session
                .view_snapshot(view)
                .unwrap()
                .folds
                .collapsed_ranges(),
            &[shifted]
        );
    }
    session
        .with_commands(a, |commands| commands.undo())
        .unwrap();
    for view in [a, b] {
        assert_eq!(
            session
                .view_snapshot(view)
                .unwrap()
                .folds
                .collapsed_ranges(),
            &[initial]
        );
    }
    session
        .with_commands(b, |commands| commands.redo())
        .unwrap();
    for view in [a, b] {
        assert_eq!(
            session
                .view_snapshot(view)
                .unwrap()
                .folds
                .collapsed_ranges(),
            &[shifted]
        );
    }
    // Navigating one pane into a fold reveals only that pane. Editing there
    // reveals the changed body in the other pane as well.
    session
        .with_commands(a, |commands| {
            commands.set_cursor(3, 0, false, CursorReveal::Center)
        })
        .unwrap();
    assert!(!session.view_snapshot(a).unwrap().folds.is_hidden(3));
    assert!(session.view_snapshot(b).unwrap().folds.is_hidden(3));
    session
        .with_commands(a, |commands| commands.type_text(b"edited "))
        .unwrap();
    assert!(!session.view_snapshot(b).unwrap().folds.is_hidden(3));
    assert_eq!(session.view_snapshot(b).unwrap().folds.ranges(), &[shifted]);
    session.replace_content(document, b"replacement").unwrap();
    for view in [a, b] {
        assert!(
            session
                .view_snapshot(view)
                .unwrap()
                .folds
                .ranges()
                .is_empty()
        );
    }
}
