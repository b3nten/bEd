//! Cross-service shutdown and document ownership regressions.
use crate::*;
use std::{
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    app
}
fn drain(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !work_finished(app.world()) {
        app.update();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn empty_session_and_no_selected_layout_are_valid() {
    let mut app = app();
    app.update();
    assert!(active_layout(app.world()).is_none());
    assert!(document_ids(app.world()).is_empty());
    assert!(preflight_session_close(app.world(), SessionCloseMode::Saved).is_ok());
    shutdown_work(app.world_mut()).unwrap();
    drain(&mut app);
}
#[test]
fn viewless_dirty_documents_still_require_an_explicit_discard_decision() {
    let mut app = app();
    let document = open_memory_document(
        app.world_mut(),
        "unsaved".to_owned(),
        DocumentOptions::unsaved("File"),
    )
    .unwrap();
    assert!(preflight_session_close(app.world(), SessionCloseMode::Saved).is_err());
    assert!(preflight_session_close(app.world(), SessionCloseMode::Discard).is_ok());
    assert_eq!(document_ids(app.world()), [document.id()]);
    assert!(read_document(app.world(), document).is_ok());
}
#[test]
fn a_draft_in_another_layout_blocks_shutdown_before_any_work_is_cancelled() {
    let mut app = app();
    let world = app.world_mut();
    let document =
        open_memory_document(world, "base".to_owned(), DocumentOptions::loaded("File")).unwrap();
    let first = crate::tests::layout(world);
    let second = crate::tests::layout(world);
    let a = Workbench::new(world).create_panel(first, "First").unwrap();
    let b = Workbench::new(world)
        .create_panel(second, "Second")
        .unwrap();
    bind_document_view(world, a, document).unwrap();
    bind_document_view(world, b, document).unwrap();
    world.entity_mut(b).insert(DocumentDraft);
    assert!(shutdown_work(world).is_err());
    assert!(!world.resource::<ManagedWork>().is_closing());
    assert!(world.get::<PanelInstance>(a).is_some());
    world.entity_mut(b).remove::<DocumentDraft>();
    shutdown_work(world).unwrap();
    drain(&mut app);
    assert!(matches!(
        edit_document(
            app.world_mut(),
            document,
            0,
            TextChange {
                range: 0..0,
                replacement: "x".into()
            },
            EditOptions::new("Edit")
        ),
        Err(DocumentError::SessionClosing)
    ));
}
#[test]
fn admitted_save_finishes_before_shutdown_and_acknowledges_only_its_snapshot() {
    let mut app = app();
    let document = open_memory_document(
        app.world_mut(),
        "base".to_owned(),
        DocumentOptions::unsaved("File"),
    )
    .unwrap();
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    bind_document_save(
        app.world_mut(),
        document,
        DocumentSaveBinding::new(Some("destination".into()), move |_| {
            wait.lock().unwrap().recv().unwrap();
            Ok(Some(SourceVersion("written".into())))
        }),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(shutdown_work(app.world_mut()).is_err());
    assert!(!app.world().resource::<ManagedWork>().is_closing());
    edit_document(
        app.world_mut(),
        document,
        0,
        TextChange {
            range: 4..4,
            replacement: " later".into(),
        },
        EditOptions::new("Edit"),
    )
    .unwrap();
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while inspect_document(app.world(), document.id()).unwrap().saving {
        app.update();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
    shutdown_work(app.world_mut()).unwrap();
    drain(&mut app);
}
#[test]
fn removing_a_panel_cancels_its_work_and_session_shutdown_joins_remaining_work() {
    let mut app = app();
    let layout = crate::tests::layout(app.world_mut());
    let panel = Workbench::new(app.world_mut())
        .create_panel(layout, "Worker")
        .unwrap();
    let task = app
        .world_mut()
        .resource_mut::<ManagedWork>()
        .spawn_task(Some(panel), "Panel work", |cancel| {
            cancel.wait(Duration::from_secs(30));
        })
        .unwrap();
    app.world_mut().despawn(panel);
    app.update();
    drain(&mut app);
    assert!(matches!(task.status(), WorkStatus::Cancelled));
    let task = app
        .world_mut()
        .resource_mut::<ManagedWork>()
        .spawn_task(None, "Session work", |cancel| {
            cancel.wait(Duration::from_secs(30));
        })
        .unwrap();
    shutdown_work(app.world_mut()).unwrap();
    drain(&mut app);
    assert!(matches!(task.status(), WorkStatus::Cancelled));
}
