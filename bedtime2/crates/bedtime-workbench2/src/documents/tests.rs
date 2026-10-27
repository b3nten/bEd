use super::*;
use crate::WorkbenchCorePlugin;
use std::{
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    name: String,
    count: i32,
}
#[derive(Clone)]
enum SettingsChange {
    Name(String),
    Count(i32),
    MutateThenReject,
}
impl DocumentModel for Settings {
    type Change = SettingsChange;
    fn apply(&mut self, change: &SettingsChange) -> Result<SettingsChange, String> {
        match change {
            SettingsChange::Name(name) => Ok(SettingsChange::Name(std::mem::replace(
                &mut self.name,
                name.clone(),
            ))),
            SettingsChange::Count(count) => Ok(SettingsChange::Count(std::mem::replace(
                &mut self.count,
                *count,
            ))),
            SettingsChange::MutateThenReject => {
                self.count = 999;
                Err("Rejected after mutating the candidate".into())
            }
        }
    }
}
fn settings() -> Settings {
    Settings {
        name: "Original".into(),
        count: 1,
    }
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    crate::tests::layout(app.world_mut());
    app
}

#[test]
fn committed_changes_include_transactions_history_and_external_replacement() {
    let mut app = app();
    let handle = text(app.world_mut(), "abcdef");
    edit_document_transaction(
        app.world_mut(),
        handle,
        0,
        [
            TextChange {
                range: 0..0,
                replacement: "L".into(),
            },
            TextChange {
                range: 7..7,
                replacement: "R".into(),
            },
        ],
        EditOptions::new("Surround"),
    )
    .unwrap();
    let commits = document_changes_since(app.world(), handle, 0)
        .unwrap()
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].changes.as_ref().unwrap().len(), 2);
    let mut observed = "abcdef".to_owned();
    for change in commits[0].changes.as_ref().unwrap() {
        DocumentModel::apply(&mut observed, change).unwrap();
    }
    assert_eq!(observed, "LabcdefR");
    undo_document(app.world_mut(), handle.id()).unwrap();
    redo_document(app.world_mut(), handle.id()).unwrap();
    for commit in document_changes_since(app.world(), handle, 1)
        .unwrap()
        .unwrap()
    {
        for change in commit.changes.unwrap() {
            DocumentModel::apply(&mut observed, &change).unwrap();
        }
    }
    assert_eq!(
        observed,
        read_document(app.world(), handle).unwrap().as_str()
    );
    // A rejected transaction and a no-op do not publish transitions.
    let revision = info(app.world(), handle).revision;
    assert!(
        edit_document(
            app.world_mut(),
            handle,
            revision,
            TextChange {
                range: 999..999,
                replacement: "bad".into()
            },
            EditOptions::default()
        )
        .is_err()
    );
    assert!(
        document_changes_since(app.world(), handle, revision)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    offer(app.world_mut(), handle, "disk", "disk");
    let info = info(app.world(), handle);
    resolve_document_conflict(
        app.world_mut(),
        handle,
        info.revision,
        info.source_generation,
        DocumentConflictResolution::AcceptIncoming,
    )
    .unwrap();
    let commits = document_changes_since(app.world(), handle, revision)
        .unwrap()
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert!(commits[0].changes.is_none());
}

#[test]
fn committed_change_readers_are_independent_and_journal_gaps_are_explicit() {
    let mut app = app();
    let handle = text(app.world_mut(), "");
    for revision in 0..140 {
        edit_document(
            app.world_mut(),
            handle,
            revision,
            TextChange {
                range: 0..0,
                replacement: "x".into(),
            },
            EditOptions::default(),
        )
        .unwrap();
    }
    assert!(
        document_changes_since(app.world(), handle, 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        document_changes_since(app.world(), handle, 139)
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        document_changes_since(app.world(), handle, 139)
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    assert!(
        document_changes_since(app.world(), handle, 140)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(document_changes_since(app.world(), handle, 141).is_err());
}
fn panel(world: &mut World, title: &str) -> Entity {
    let mut workbench = Workbench::new(world);
    let layout = workbench.active_layout();
    workbench.create_panel(layout, title).unwrap()
}
fn text(world: &mut World, initial: &str) -> DocumentHandle<String> {
    create_memory_snapshot(world, initial.to_owned(), DocumentOptions::loaded("Text")).unwrap()
}
fn info<M: DocumentModel>(world: &World, handle: DocumentHandle<M>) -> DocumentInfo {
    inspect_document(world, handle.id()).unwrap()
}
fn replace(world: &mut World, handle: DocumentHandle<String>, value: &str) -> u64 {
    let revision = info(world, handle).revision;
    let len = read_document(world, handle).unwrap().len();
    edit_document(
        world,
        handle,
        revision,
        TextChange {
            range: 0..len,
            replacement: value.into(),
        },
        EditOptions::new("Replace text"),
    )
    .unwrap()
}
fn offer(
    world: &mut World,
    handle: DocumentHandle<String>,
    value: &str,
    version: &str,
) -> DocumentExternalUpdate {
    let generation = info(world, handle).source_generation;
    offer_document_external_update(
        world,
        handle,
        generation,
        value.into(),
        Some(version.into()),
    )
    .unwrap()
}
fn resolve(
    world: &mut World,
    handle: DocumentHandle<String>,
    resolution: DocumentConflictResolution<String>,
) -> u64 {
    let current = info(world, handle);
    resolve_document_conflict(
        world,
        handle,
        current.revision,
        current.source_generation,
        resolution,
    )
    .unwrap()
}

/// The callback reports its immutable capture and blocks until the test releases
/// it. No sleeps or assumptions about worker execution order are needed.
fn controlled_save<M: DocumentModel>(
    destination: Option<&str>,
    result: SaveResult,
) -> (
    SaveBinding<M>,
    mpsc::Sender<()>,
    mpsc::Receiver<DocumentSaveSnapshot<M>>,
) {
    let (release, receive_release) = mpsc::channel();
    let receive_release = Mutex::new(receive_release);
    let (send_snapshot, snapshots) = mpsc::channel();
    let binding = SaveBinding::new(destination.map(str::to_owned), move |snapshot| {
        send_snapshot.send(snapshot).unwrap();
        receive_release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        result.clone()
    });
    (binding, release, snapshots)
}
fn wait_save(world: &mut World, id: DocumentId) -> DocumentSaveFinished {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(completion) = poll_document_saves(world)
            .into_iter()
            .find(|completion| completion.document == id)
        {
            return completion;
        }
        assert!(Instant::now() < deadline, "Save did not finish");
        thread::yield_now();
    }
}

#[test]
fn typed_and_text_models_share_revision_history_and_dirty_checkpoints() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<DocumentHandle<Settings>>();
    let mut app = app();
    let typed = create_memory_snapshot(
        app.world_mut(),
        settings(),
        DocumentOptions::loaded("Settings"),
    )
    .unwrap();
    let text = text(app.world_mut(), "Original");
    assert!(!info(app.world(), typed).dirty);
    assert!(!info(app.world(), text).dirty);
    edit_document_transaction(
        app.world_mut(),
        typed,
        0,
        [
            SettingsChange::Name("Changed".into()),
            SettingsChange::Count(2),
        ],
        EditOptions::new("Configure"),
    )
    .unwrap();
    replace(app.world_mut(), text, "Changed");
    assert_eq!(info(app.world(), typed).revision, 1);
    assert_eq!(info(app.world(), text).revision, 1);
    assert!(info(app.world(), typed).dirty);
    assert!(info(app.world(), text).dirty);
    assert_eq!(
        info(app.world(), typed).undo_label.as_deref(),
        Some("Configure")
    );
    assert_eq!(undo_document(app.world_mut(), typed.id()).unwrap(), 2);
    assert_eq!(undo_document(app.world_mut(), text.id()).unwrap(), 2);
    assert_eq!(read_document(app.world(), typed).unwrap(), &settings());
    assert_eq!(read_document(app.world(), text).unwrap(), "Original");
    assert!(!info(app.world(), typed).dirty);
    assert!(!info(app.world(), text).dirty);
    assert_eq!(redo_document(app.world_mut(), typed.id()).unwrap(), 3);
    assert_eq!(read_document(app.world(), typed).unwrap().count, 2);
    assert!(info(app.world(), typed).dirty);

    let unsaved =
        create_memory_snapshot(app.world_mut(), settings(), DocumentOptions::unsaved("New"))
            .unwrap();
    assert!(
        info(app.world(), unsaved).dirty,
        "An untouched unsaved model has no checkpoint"
    );
    assert_eq!(
        close_document(app.world_mut(), unsaved.id()),
        Err(DocumentError::Dirty)
    );
}

#[test]
fn typed_models_use_the_same_save_external_and_conflict_lifecycle_as_text() {
    let mut app = app();
    let mut options = DocumentOptions::loaded("Settings");
    options.initial_state = DocumentInitialState::Loaded(Some("initial".into()));
    let document = create_memory_snapshot(app.world_mut(), settings(), options).unwrap();
    edit_document(
        app.world_mut(),
        document,
        0,
        SettingsChange::Name("Local".into()),
        EditOptions::new("Rename"),
    )
    .unwrap();
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |snapshot| {
            assert_eq!(
                snapshot.model,
                Settings {
                    name: "Local".into(),
                    count: 1
                }
            );
            assert_eq!(snapshot.expected_source_version, Some("initial".into()));
            Ok(Some("saved".into()))
        }),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
    assert!(!info(app.world(), document).dirty);
    let remote = Settings {
        name: "Remote".into(),
        count: 8,
    };
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            1,
            remote.clone(),
            Some("remote".into())
        )
        .unwrap(),
        DocumentExternalUpdate::Applied
    );
    assert_eq!(read_document(app.world(), document).unwrap(), &remote);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap().name, "Local");
    assert!(info(app.world(), document).dirty);
    let newest = Settings {
        name: "Newest".into(),
        count: 9,
    };
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            2,
            newest.clone(),
            Some("newest".into())
        )
        .unwrap(),
        DocumentExternalUpdate::Conflict
    );
    let conflict = document_conflict(app.world(), document).unwrap().unwrap();
    assert_eq!(conflict.base, Some(remote));
    assert_eq!(conflict.incoming, newest);
    let merged = Settings {
        name: "Merged".into(),
        count: 10,
    };
    resolve_document_conflict(
        app.world_mut(),
        document,
        conflict.revision,
        conflict.source_generation,
        DocumentConflictResolution::Merged(merged.clone()),
    )
    .unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), &merged);
    assert!(info(app.world(), document).dirty);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap().name, "Local");
    assert_eq!(
        info(app.world(), document).source_version,
        Some("newest".into())
    );
}

#[test]
fn rejected_transactions_stale_edits_and_invalid_utf8_leave_everything_unchanged() {
    let mut app = app();
    let model = create_memory_snapshot(
        app.world_mut(),
        settings(),
        DocumentOptions::loaded("Typed"),
    )
    .unwrap();
    let error = edit_document_transaction(
        app.world_mut(),
        model,
        0,
        [SettingsChange::Count(2), SettingsChange::MutateThenReject],
        EditOptions::new("Rejected"),
    );
    assert!(matches!(error, Err(DocumentError::InvalidChange(_))));
    assert_eq!(read_document(app.world(), model).unwrap(), &settings());
    assert_eq!(info(app.world(), model).revision, 0);
    assert!(!info(app.world(), model).can_undo);
    assert!(!info(app.world(), model).dirty);

    let document = text(app.world_mut(), "a🦀b");
    let transaction = [
        TextChange {
            range: 0..1,
            replacement: "A".into(),
        },
        TextChange {
            range: 2..3,
            replacement: "invalid".into(),
        },
    ];
    assert!(matches!(
        edit_document_transaction(
            app.world_mut(),
            document,
            0,
            transaction,
            EditOptions::default()
        ),
        Err(DocumentError::InvalidChange(_))
    ));
    assert_eq!(read_document(app.world(), document).unwrap(), "a🦀b");
    assert_eq!(info(app.world(), document).revision, 0);
    assert!(!info(app.world(), document).can_undo);
    for range in [7..7, 4..2, 0..usize::MAX] {
        assert!(
            edit_document(
                app.world_mut(),
                document,
                0,
                TextChange {
                    range,
                    replacement: String::new()
                },
                EditOptions::default()
            )
            .is_err()
        );
    }
    edit_document(
        app.world_mut(),
        document,
        0,
        TextChange {
            range: 1..5,
            replacement: "é".into(),
        },
        EditOptions::new("Unicode"),
    )
    .unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "aéb");
    assert_eq!(
        edit_document(
            app.world_mut(),
            document,
            0,
            TextChange {
                range: 0..0,
                replacement: "stale".into()
            },
            EditOptions::default()
        ),
        Err(DocumentError::StaleRevision {
            expected: 0,
            actual: 1
        })
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "aéb");
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "a🦀b");
    redo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "aéb");
    assert_eq!(
        replace(app.world_mut(), document, "aéb"),
        3,
        "No-op edits do not create revisions"
    );
}

#[test]
fn explicit_groups_merge_only_consecutive_edits_and_never_reopen() {
    let mut app = app();
    let document = text(app.world_mut(), "a");
    let first = panel(app.world_mut(), "First");
    let second = panel(app.world_mut(), "Second");
    bind_document_view(app.world_mut(), first, document).unwrap();
    bind_document_view(app.world_mut(), second, document).unwrap();
    let group = begin_document_undo_group(app.world_mut(), document.id(), first).unwrap();
    edit_document(
        app.world_mut(),
        document,
        0,
        TextChange {
            range: 1..1,
            replacement: "b".into(),
        },
        EditOptions::grouped("Typing", group),
    )
    .unwrap();
    assert_eq!(
        read_document(app.world(), document).unwrap(),
        "ab",
        "Grouped edits are visible immediately"
    );
    edit_document(
        app.world_mut(),
        document,
        1,
        TextChange {
            range: 2..2,
            replacement: "c".into(),
        },
        EditOptions::grouped("Typing", group),
    )
    .unwrap();
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "a");
    redo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "abc");
    assert_eq!(
        edit_document(
            app.world_mut(),
            document,
            4,
            TextChange {
                range: 3..3,
                replacement: "!".into()
            },
            EditOptions::grouped("Old group", group)
        ),
        Err(DocumentError::StaleUndoGroup)
    );

    let group = begin_document_undo_group(app.world_mut(), document.id(), first).unwrap();
    edit_document(
        app.world_mut(),
        document,
        4,
        TextChange {
            range: 3..3,
            replacement: "d".into(),
        },
        EditOptions::grouped("Typing", group),
    )
    .unwrap();
    let mut other_edit = EditOptions::new("Other viewer");
    other_edit.viewer = Some(second);
    edit_document(
        app.world_mut(),
        document,
        5,
        TextChange {
            range: 4..4,
            replacement: "e".into(),
        },
        other_edit,
    )
    .unwrap();
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
    assert_eq!(
        edit_document(
            app.world_mut(),
            document,
            6,
            TextChange {
                range: 5..5,
                replacement: "!".into()
            },
            EditOptions::grouped("Cannot reopen", group)
        ),
        Err(DocumentError::StaleUndoGroup)
    );
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "abcd");
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "abc");
    let group = begin_document_undo_group(app.world_mut(), document.id(), first).unwrap();
    end_document_undo_group(app.world_mut(), document.id(), group).unwrap();
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
}

#[test]
fn bounded_history_retains_only_recent_transactions_and_can_be_disabled() {
    let mut app = app();
    let mut options = DocumentOptions::loaded("Bounded");
    options.history_limit = 2;
    let document =
        create_memory_snapshot(app.world_mut(), "0".to_owned(), options.clone()).unwrap();
    for value in ["1", "2", "3"] {
        replace(app.world_mut(), document, value);
    }
    undo_document(app.world_mut(), document.id()).unwrap();
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "1");
    assert_eq!(
        undo_document(app.world_mut(), document.id()),
        Err(DocumentError::NothingToUndo)
    );
    assert!(info(app.world(), document).dirty);
    options.history_limit = 0;
    let no_history = create_memory_snapshot(app.world_mut(), "0".to_owned(), options).unwrap();
    replace(app.world_mut(), no_history, "1");
    assert_eq!(info(app.world(), no_history).revision, 1);
    assert!(!info(app.world(), no_history).can_undo);
}

#[test]
fn drafts_explicitly_block_save_history_and_retirement_in_every_viewer() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    let first = panel(app.world_mut(), "First");
    let second = panel(app.world_mut(), "Second");
    bind_document_view(app.world_mut(), first, document).unwrap();
    bind_document_view(app.world_mut(), second, document).unwrap();
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |_| Ok(None)),
    )
    .unwrap();
    replace(app.world_mut(), document, "committed");
    app.world_mut().entity_mut(second).insert(DocumentDraft);
    assert!(panel_has_draft(app.world(), second));
    assert!(document_has_drafts(app.world(), document.id()));
    assert!(info(app.world(), document).has_drafts);
    assert!(!info(app.world(), document).can_undo);
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        undo_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        redo_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        close_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        discard_document(app.world_mut(), document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        check_session_documents(app.world()),
        Err(DocumentError::Drafts)
    );
    reconcile_document_views(app.world_mut()).unwrap();
    assert!(app.world().get::<PanelInstance>(second).unwrap().dirty);
    assert_eq!(read_document(app.world(), document).unwrap(), "committed");
    app.world_mut().entity_mut(second).remove::<DocumentDraft>();
    undo_document(app.world_mut(), document.id()).unwrap();
    reconcile_document_views(app.world_mut()).unwrap();
    assert!(!app.world().get::<PanelInstance>(first).unwrap().dirty);
    assert!(!app.world().get::<PanelInstance>(second).unwrap().dirty);
    close_document(app.world_mut(), document.id()).unwrap();
    assert!(app.world().get_entity(first).is_err());
    assert!(app.world().get_entity(second).is_err());
}

#[test]
fn saves_acknowledge_only_the_captured_model_and_survive_last_viewer_close() {
    let mut app = app();
    let document = create_memory_snapshot(
        app.world_mut(),
        "captured".to_owned(),
        DocumentOptions::unsaved("New"),
    )
    .unwrap();
    let viewer = panel(app.world_mut(), "Viewer");
    bind_document_view(app.world_mut(), viewer, document).unwrap();
    let (binding, release, snapshots) =
        controlled_save(Some("global://destination"), Ok(Some("v1".into())));
    install_save_binding(app.world_mut(), document, binding.clone()).unwrap();
    let work = save_document(app.world_mut(), document.id()).unwrap();
    let capture = snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(capture.model, "captured");
    assert_eq!(capture.revision, 0);
    assert_eq!(capture.expected_source_version, None);
    assert_eq!(capture.document, document.id());
    assert!(info(app.world(), document).saving);
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Saving)
    );
    assert_eq!(
        install_save_binding(app.world_mut(), document, binding),
        Err(DocumentError::Saving)
    );
    assert_eq!(
        remove_save_binding(app.world_mut(), document.id()),
        Err(DocumentError::Saving)
    );
    assert_eq!(
        discard_document(app.world_mut(), document.id()),
        Err(DocumentError::Saving)
    );
    assert_eq!(
        close_document(app.world_mut(), document.id()),
        Err(DocumentError::Saving)
    );
    Workbench::new(app.world_mut()).close_panel(viewer).unwrap();
    app.update();
    assert!(
        app.world()
            .get_resource::<ManagedWork>()
            .unwrap()
            .list()
            .iter()
            .any(|entry| entry.id == work && entry.owner == Some(document.id().entity()))
    );
    replace(app.world_mut(), document, "newer edit");
    release.send(()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
    assert_eq!(read_document(app.world(), document).unwrap(), "newer edit");
    let current = info(app.world(), document);
    assert!(current.dirty);
    assert!(!current.saving);
    assert_eq!(current.source_version, Some("v1".into()));
    assert_eq!(current.source_generation, 1);
    assert_eq!(
        current.revision, 1,
        "Saving changes source knowledge, not content revision"
    );
    undo_document(app.world_mut(), document.id()).unwrap();
    assert!(
        !info(app.world(), document).dirty,
        "Undoing to the captured contents is clean"
    );
    redo_document(app.world_mut(), document.id()).unwrap();
    assert!(info(app.world(), document).dirty);
}

#[test]
fn undo_during_save_does_not_change_the_immutable_capture() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "save this");
    let (binding, release, snapshots) = controlled_save(None, Ok(Some("written".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        snapshots
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .model,
        "save this"
    );
    undo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
    release.send(()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
    assert_eq!(read_document(app.world(), document).unwrap(), "base");
    assert!(info(app.world(), document).dirty);
    redo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
}

#[test]
fn save_errors_and_panics_preserve_content_and_history_and_release_bindings() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "local");
    let failure = DocumentSaveError::Failed("disk full".into());
    let expected = failure.clone();
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(Some("disk".into()), move |_| Err(failure.clone())),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(expected.clone())
    );
    assert_eq!(info(app.world(), document).last_save_error, Some(expected));
    assert_eq!(read_document(app.world(), document).unwrap(), "local");
    assert_eq!(info(app.world(), document).revision, 1);
    assert!(info(app.world(), document).can_undo);
    assert!(info(app.world(), document).dirty);
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(Some("disk".into()), |_| panic!("save panicked")),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::Panicked)
    );
    assert!(!info(app.world(), document).saving);
    assert_eq!(read_document(app.world(), document).unwrap(), "local");
    undo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
    assert!(info(app.world(), document).requires_source_refresh);
    offer(app.world_mut(), document, "base", "fresh after panic");
    remove_save_binding(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::NoSaveBinding)
    );
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(Some("disk".into()), |_| Ok(Some("recovered".into()))),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
}

#[test]
fn destination_keys_serialize_saves_across_documents() {
    let mut app = app();
    let first = text(app.world_mut(), "first");
    let second = text(app.world_mut(), "second");
    let (first_binding, release_first, snapshots_first) =
        controlled_save(Some("same destination"), Ok(Some("one".into())));
    let (second_binding, release_second, snapshots_second) =
        controlled_save(Some("same destination"), Ok(Some("two".into())));
    install_save_binding(app.world_mut(), first, first_binding).unwrap();
    install_save_binding(app.world_mut(), second, second_binding).unwrap();
    save_document(app.world_mut(), first.id()).unwrap();
    snapshots_first
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        save_document(app.world_mut(), second.id()),
        Err(DocumentError::DestinationBusy("same destination".into()))
    );
    assert!(!info(app.world(), second).saving);
    release_first.send(()).unwrap();
    assert!(wait_save(app.world_mut(), first.id()).result.is_ok());
    save_document(app.world_mut(), second.id()).unwrap();
    assert_eq!(
        snapshots_second
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .model,
        "second"
    );
    release_second.send(()).unwrap();
    assert!(wait_save(app.world_mut(), second.id()).result.is_ok());
}

#[test]
fn save_cas_failure_requires_refresh_before_retry_even_if_source_contents_match() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "local");
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |_| {
            Err(DocumentSaveError::Conflict("newer source".into()))
        }),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(matches!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::Conflict(_))
    ));
    assert!(info(app.world(), document).conflict);
    assert!(info(app.world(), document).requires_source_refresh);
    assert!(document_conflict(app.world(), document).unwrap().is_none());
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::SourceRefreshRequired)
    );
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            0,
            "base".into(),
            Some("read begun before failure".into())
        ),
        Err(DocumentError::StaleSourceGeneration {
            expected: 0,
            actual: 1
        })
    );
    assert!(info(app.world(), document).requires_source_refresh);
    assert_eq!(
        offer(app.world_mut(), document, "base", "fresh"),
        DocumentExternalUpdate::Duplicate
    );
    assert!(!info(app.world(), document).conflict);
    assert_eq!(info(app.world(), document).source_generation, 2);
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |snapshot| {
            assert_eq!(snapshot.expected_source_version, Some("fresh".into()));
            Ok(Some("written".into()))
        }),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
    assert!(!info(app.world(), document).dirty);
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            0,
            "stale async read".into(),
            Some("old".into())
        ),
        Err(DocumentError::StaleSourceGeneration {
            expected: 0,
            actual: 3
        })
    );
}

#[test]
fn saves_and_external_observations_close_explicit_groups() {
    let mut app = app();
    let document = text(app.world_mut(), "a");
    let viewer = panel(app.world_mut(), "Viewer");
    bind_document_view(app.world_mut(), viewer, document).unwrap();
    let group = begin_document_undo_group(app.world_mut(), document.id(), viewer).unwrap();
    assert_eq!(
        offer_document_external_update(app.world_mut(), document, 0, "a".into(), None).unwrap(),
        DocumentExternalUpdate::Duplicate
    );
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
    let group = begin_document_undo_group(app.world_mut(), document.id(), viewer).unwrap();
    let (binding, release, snapshots) = controlled_save(None, Ok(None));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
    let group = begin_document_undo_group(app.world_mut(), document.id(), viewer).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
    let group = begin_document_undo_group(app.world_mut(), document.id(), viewer).unwrap();
    release.send(()).unwrap();
    wait_save(app.world_mut(), document.id());
    assert_eq!(
        end_document_undo_group(app.world_mut(), document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
}

#[test]
fn clean_external_refresh_is_undoable_without_undoing_acknowledged_source_knowledge() {
    let mut app = app();
    let mut options = DocumentOptions::loaded("Source");
    options.initial_state = DocumentInitialState::Loaded(Some("v1".into()));
    let document = create_memory_snapshot(app.world_mut(), "one".to_owned(), options).unwrap();
    assert_eq!(
        offer(app.world_mut(), document, "two", "v2"),
        DocumentExternalUpdate::Applied
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "two");
    assert!(!info(app.world(), document).dirty);
    assert_eq!(info(app.world(), document).revision, 1);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "one");
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("v2".into())
    );
    assert_eq!(info(app.world(), document).source_generation, 1);
    redo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
    assert_eq!(
        offer(app.world_mut(), document, "two", "v2"),
        DocumentExternalUpdate::Duplicate
    );
    assert_eq!(info(app.world(), document).revision, 3);
    assert_eq!(info(app.world(), document).source_generation, 1);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        undo_document(app.world_mut(), document.id()),
        Err(DocumentError::NothingToUndo),
        "Duplicate observations add no history"
    );
}

#[test]
fn dirty_external_duplicates_convergence_and_self_write_echoes_add_no_history() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "local");
    assert_eq!(
        offer(app.world_mut(), document, "base", "v1"),
        DocumentExternalUpdate::Duplicate
    );
    assert_eq!(info(app.world(), document).revision, 1);
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        offer(app.world_mut(), document, "local", "v2"),
        DocumentExternalUpdate::Acknowledged
    );
    assert!(!info(app.world(), document).dirty);
    assert_eq!(info(app.world(), document).revision, 1);
    assert_eq!(
        offer(app.world_mut(), document, "local", "v2"),
        DocumentExternalUpdate::Duplicate
    );
    assert_eq!(
        offer(app.world_mut(), document, "local", "new token same content"),
        DocumentExternalUpdate::Acknowledged
    );
    assert_eq!(info(app.world(), document).revision, 1);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "base");
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        undo_document(app.world_mut(), document.id()),
        Err(DocumentError::NothingToUndo)
    );
    redo_document(app.world_mut(), document.id()).unwrap();
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |_| Ok(Some("self write".into()))),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    wait_save(app.world_mut(), document.id());
    replace(app.world_mut(), document, "new local");
    let before = info(app.world(), document);
    assert_eq!(
        offer(app.world_mut(), document, "local", "self write"),
        DocumentExternalUpdate::Duplicate
    );
    let after = info(app.world(), document);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.source_generation, before.source_generation);
    assert!(!after.conflict);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "local");
    assert!(!info(app.world(), document).dirty);
}

#[test]
fn conflicts_retain_base_editable_local_and_newest_incoming_and_reject_stale_resolutions() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "local");
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, |_| Ok(None)),
    )
    .unwrap();
    assert_eq!(
        offer(app.world_mut(), document, "remote", "v2"),
        DocumentExternalUpdate::Conflict
    );
    let conflict = document_conflict(app.world(), document).unwrap().unwrap();
    assert_eq!(conflict.base.as_deref(), Some("base"));
    assert_eq!(conflict.local, "local");
    assert_eq!(conflict.incoming, "remote");
    assert_eq!(read_document(app.world(), document).unwrap(), "local");
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        offer(app.world_mut(), document, "remote", "v2"),
        DocumentExternalUpdate::Duplicate
    );
    assert_eq!(
        info(app.world(), document).source_generation,
        conflict.source_generation
    );
    replace(app.world_mut(), document, "new local");
    assert!(matches!(
        resolve_document_conflict(
            app.world_mut(),
            document,
            conflict.revision,
            conflict.source_generation,
            DocumentConflictResolution::AcceptIncoming
        ),
        Err(DocumentError::StaleRevision { .. })
    ));
    let local_revision = info(app.world(), document).revision;
    assert_eq!(
        offer(app.world_mut(), document, "new remote", "v3"),
        DocumentExternalUpdate::Conflict
    );
    assert!(matches!(
        resolve_document_conflict(
            app.world_mut(),
            document,
            local_revision,
            conflict.source_generation,
            DocumentConflictResolution::AcceptIncoming
        ),
        Err(DocumentError::StaleSourceGeneration { .. })
    ));
    let newest = document_conflict(app.world(), document).unwrap().unwrap();
    assert_eq!(newest.base.as_deref(), Some("base"));
    assert_eq!(newest.local, "new local");
    assert_eq!(newest.incoming, "new remote");
    assert_eq!(newest.incoming_version, Some("v3".into()));
    resolve(
        app.world_mut(),
        document,
        DocumentConflictResolution::AcceptIncoming,
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "new remote");
    assert!(!info(app.world(), document).dirty);
    assert!(!info(app.world(), document).conflict);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "new local");
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("v3".into())
    );
    assert_eq!(info(app.world(), document).source_generation, 3);
    redo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
}

#[test]
fn keep_local_and_supplied_merge_acknowledge_incoming_without_implicitly_writing() {
    let mut app = app();
    let keep = text(app.world_mut(), "base");
    replace(app.world_mut(), keep, "local");
    offer(app.world_mut(), keep, "remote", "v2");
    let revision = info(app.world(), keep).revision;
    resolve(app.world_mut(), keep, DocumentConflictResolution::KeepLocal);
    assert_eq!(info(app.world(), keep).revision, revision);
    assert_eq!(read_document(app.world(), keep).unwrap(), "local");
    assert!(info(app.world(), keep).dirty);
    assert!(!info(app.world(), keep).conflict);
    assert!(!info(app.world(), keep).saving);
    undo_document(app.world_mut(), keep.id()).unwrap();
    assert_eq!(read_document(app.world(), keep).unwrap(), "base");
    assert!(
        info(app.world(), keep).dirty,
        "Undo cannot restore obsolete source knowledge"
    );
    assert_eq!(info(app.world(), keep).source_version, Some("v2".into()));

    let merged = text(app.world_mut(), "base");
    replace(app.world_mut(), merged, "local");
    offer(app.world_mut(), merged, "remote", "v2");
    resolve(
        app.world_mut(),
        merged,
        DocumentConflictResolution::Merged("merged".into()),
    );
    assert_eq!(read_document(app.world(), merged).unwrap(), "merged");
    assert!(info(app.world(), merged).dirty);
    assert_eq!(info(app.world(), merged).revision, 2);
    undo_document(app.world_mut(), merged.id()).unwrap();
    assert_eq!(read_document(app.world(), merged).unwrap(), "local");
    assert!(info(app.world(), merged).dirty);
    assert_eq!(info(app.world(), merged).source_version, Some("v2".into()));
    redo_document(app.world_mut(), merged.id()).unwrap();
    assert_eq!(read_document(app.world(), merged).unwrap(), "merged");
}

#[test]
fn retained_conflicts_require_explicit_resolution_but_can_converge_or_revert_at_source() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "local");
    offer(app.world_mut(), document, "remote", "v2");
    undo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
    assert!(info(app.world(), document).conflict);
    assert_eq!(
        offer(app.world_mut(), document, "new remote", "v3"),
        DocumentExternalUpdate::Conflict
    );
    assert_eq!(
        read_document(app.world(), document).unwrap(),
        "base",
        "A retained conflict is not implicitly resolved by later observations"
    );
    replace(app.world_mut(), document, "new remote");
    assert_eq!(
        offer(app.world_mut(), document, "new remote", "v3"),
        DocumentExternalUpdate::Acknowledged
    );
    assert!(!info(app.world(), document).dirty);
    assert!(!info(app.world(), document).conflict);
    replace(app.world_mut(), document, "local again");
    offer(app.world_mut(), document, "another remote", "v4");
    assert_eq!(
        offer(app.world_mut(), document, "new remote", "v5"),
        DocumentExternalUpdate::Duplicate
    );
    assert!(!info(app.world(), document).conflict);
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("v5".into())
    );
}

#[test]
fn late_save_completion_preserves_newer_conflict_and_requires_reconciliation() {
    let mut app = app();
    let mut options = DocumentOptions::loaded("Source");
    options.initial_state = DocumentInitialState::Loaded(Some("v1".into()));
    let document = create_memory_snapshot(app.world_mut(), "base".to_owned(), options).unwrap();
    replace(app.world_mut(), document, "local");
    let (binding, release, snapshots) = controlled_save(None, Ok(Some("save v3".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    let snapshot = snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(snapshot.expected_source_version, Some("v1".into()));
    assert_eq!(
        offer(app.world_mut(), document, "remote", "external v2"),
        DocumentExternalUpdate::Conflict
    );
    release.send(()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::SourceChanged)
    );
    let current = info(app.world(), document);
    assert!(current.conflict);
    assert!(current.requires_source_refresh);
    assert_eq!(
        current.source_version,
        Some("v1".into()),
        "Late save cannot overwrite the acknowledged checkpoint"
    );
    assert_eq!(current.source_generation, 2);
    assert_eq!(current.revision, 1);
    let conflict = document_conflict(app.world(), document).unwrap().unwrap();
    for resolution in [
        DocumentConflictResolution::AcceptIncoming,
        DocumentConflictResolution::KeepLocal,
        DocumentConflictResolution::Merged("merged".into()),
    ] {
        assert_eq!(
            resolve_document_conflict(
                app.world_mut(),
                document,
                current.revision,
                current.source_generation,
                resolution
            ),
            Err(DocumentError::SourceRefreshRequired)
        );
        assert_eq!(read_document(app.world(), document).unwrap(), "local");
        assert_eq!(info(app.world(), document).revision, current.revision);
    }
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            1,
            "remote".into(),
            Some("external v2".into())
        ),
        Err(DocumentError::StaleSourceGeneration {
            expected: 1,
            actual: 2
        })
    );
    assert_eq!(conflict.incoming, "remote");
    assert_eq!(conflict.incoming_version, Some("external v2".into()));
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        offer(app.world_mut(), document, "local", "save v3"),
        DocumentExternalUpdate::Acknowledged
    );
    assert!(!info(app.world(), document).dirty);
    assert!(!info(app.world(), document).conflict);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("save v3".into())
    );
}

#[test]
fn late_save_cannot_make_a_clean_external_refresh_falsely_clean_against_its_snapshot() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    let (binding, release, snapshots) = controlled_save(None, Ok(Some("save v3".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        offer(app.world_mut(), document, "remote", "external v2"),
        DocumentExternalUpdate::Applied
    );
    release.send(()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::SourceChanged)
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "remote");
    let current = info(app.world(), document);
    assert!(!current.dirty);
    assert!(current.conflict);
    assert_eq!(current.source_version, Some("external v2".into()));
    assert_eq!(current.source_generation, 2);
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::SourceRefreshRequired)
    );
    assert_eq!(
        close_document(app.world_mut(), document.id()),
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        offer(app.world_mut(), document, "base", "save v3"),
        DocumentExternalUpdate::Applied
    );
    assert!(!info(app.world(), document).conflict);
    assert!(!info(app.world(), document).dirty);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "remote");
    assert!(info(app.world(), document).dirty);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("save v3".into())
    );
}

#[test]
fn stale_source_observations_are_atomic_and_dont_end_current_groups() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    offer(app.world_mut(), document, "incoming", "v2");
    let viewer = panel(app.world_mut(), "Viewer");
    bind_document_view(app.world_mut(), viewer, document).unwrap();
    let group = begin_document_undo_group(app.world_mut(), document.id(), viewer).unwrap();
    assert_eq!(
        offer_document_external_update(
            app.world_mut(),
            document,
            0,
            "stale".into(),
            Some("v1".into())
        ),
        Err(DocumentError::StaleSourceGeneration {
            expected: 0,
            actual: 1
        })
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "incoming");
    assert_eq!(info(app.world(), document).revision, 1);
    assert_eq!(
        info(app.world(), document).source_version,
        Some("v2".into())
    );
    end_document_undo_group(app.world_mut(), document.id(), group).unwrap();
}

#[test]
fn explicit_discard_closes_all_bound_viewers_but_not_other_documents() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    let other = text(app.world_mut(), "unrelated");
    let first = panel(app.world_mut(), "First");
    let second = panel(app.world_mut(), "Second");
    let unrelated = panel(app.world_mut(), "Unrelated");
    bind_document_view(app.world_mut(), first, document).unwrap();
    bind_document_view(app.world_mut(), second, document).unwrap();
    bind_document_view(app.world_mut(), unrelated, other).unwrap();
    replace(app.world_mut(), document, "local");
    offer(app.world_mut(), document, "remote", "v2");
    assert_eq!(
        close_document(app.world_mut(), document.id()),
        Err(DocumentError::Conflict)
    );
    discard_document(app.world_mut(), document.id()).unwrap();
    assert!(app.world().get_entity(first).is_err());
    assert!(app.world().get_entity(second).is_err());
    assert!(app.world().get::<PanelInstance>(unrelated).is_some());
    assert_eq!(read_document(app.world(), other).unwrap(), "unrelated");
    assert_eq!(document_ids(app.world()), [other.id()]);
}

#[test]
fn reconciliation_rejects_invalid_references_ownership_and_unbound_drafts_without_partial_flag_writes()
 {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    let viewer = panel(app.world_mut(), "Viewer");
    bind_document_view(app.world_mut(), viewer, document).unwrap();
    replace(app.world_mut(), document, "dirty");
    let draft = panel(app.world_mut(), "Unbound draft");
    app.world_mut().entity_mut(draft).insert(DocumentDraft);
    assert!(reconcile_document_views(app.world_mut()).is_err());
    assert!(!app.world().get::<PanelInstance>(viewer).unwrap().dirty);
    app.world_mut().entity_mut(draft).remove::<DocumentDraft>();
    reconcile_document_views(app.world_mut()).unwrap();
    assert!(app.world().get::<PanelInstance>(viewer).unwrap().dirty);
    app.world_mut().despawn(document.id().entity());
    assert!(reconcile_document_views(app.world_mut()).is_err());
}

#[test]
fn cancelling_work_after_a_write_starts_cannot_leave_a_false_clean_checkpoint() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "written snapshot");
    let stored = Arc::new(Mutex::new(String::from("base")));
    let source = stored.clone();
    let (started, receive_started) = mpsc::channel();
    let (release, receive_release) = mpsc::channel();
    let receive_release = Mutex::new(receive_release);
    install_save_binding(
        app.world_mut(),
        document,
        SaveBinding::new(None, move |snapshot| {
            started.send(()).unwrap();
            receive_release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            *source.lock().unwrap() = snapshot.model;
            Ok(Some("written version".into()))
        }),
    )
    .unwrap();
    let work = save_document(app.world_mut(), document.id()).unwrap();
    receive_started
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    undo_document(app.world_mut(), document.id()).unwrap();
    app.world_mut()
        .get_resource_mut::<ManagedWork>()
        .unwrap()
        .cancel(work)
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::Cancelled)
    );
    assert_eq!(*stored.lock().unwrap(), "written snapshot");
    assert_eq!(read_document(app.world(), document).unwrap(), "base");
    assert!(info(app.world(), document).requires_source_refresh);
    assert_eq!(
        close_document(app.world_mut(), document.id()),
        Err(DocumentError::Conflict)
    );
    assert!(crate::preflight_session_close(app.world(), crate::SessionCloseMode::Saved).is_err());
    assert!(matches!(
        offer_document_external_update(app.world_mut(), document, 0, "base".into(), None),
        Err(DocumentError::StaleSourceGeneration { .. })
    ));
}

#[test]
fn a_watcher_echo_before_the_save_receipt_never_reapplies_undone_content() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "saved capture");
    let (binding, release, snapshots) = controlled_save(None, Ok(Some("saved version".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    undo_document(app.world_mut(), document.id()).unwrap();
    let revision = info(app.world(), document).revision;
    assert_eq!(
        offer(app.world_mut(), document, "saved capture", "saved version"),
        DocumentExternalUpdate::Conflict
    );
    assert_eq!(read_document(app.world(), document).unwrap(), "base");
    assert_eq!(info(app.world(), document).revision, revision);
    release.send(()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_ok());
    let current = info(app.world(), document);
    assert!(current.dirty);
    assert!(!current.conflict);
    assert!(!current.requires_source_refresh);
    assert!(current.can_redo);
    assert_eq!(current.source_version, Some("saved version".into()));
    redo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document).dirty);
}

#[test]
fn a_watcher_echo_waits_for_success_and_a_newer_observation_is_not_mistaken_for_it() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "capture");
    let (binding, release, snapshots) = controlled_save(None, Ok(Some("our version".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    offer(app.world_mut(), document, "capture", "our version");
    assert!(
        info(app.world(), document).dirty,
        "An unacknowledged write is not saved yet"
    );
    offer(app.world_mut(), document, "newer external", "newer version");
    release.send(()).unwrap();
    assert_eq!(
        wait_save(app.world_mut(), document.id()).result,
        Err(DocumentSaveError::SourceChanged)
    );
    let conflict = document_conflict(app.world(), document).unwrap().unwrap();
    assert_eq!(conflict.incoming, "newer external");
    assert_eq!(conflict.incoming_version, Some("newer version".into()));
    assert_eq!(read_document(app.world(), document).unwrap(), "capture");
}

#[test]
fn a_failed_write_with_a_watcher_echo_does_not_acknowledge_durable_success() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    replace(app.world_mut(), document, "capture");
    let (binding, release, snapshots) =
        controlled_save(None, Err(DocumentSaveError::Failed("flush failed".into())));
    install_save_binding(app.world_mut(), document, binding).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    offer(app.world_mut(), document, "capture", "visible bytes");
    release.send(()).unwrap();
    assert!(wait_save(app.world_mut(), document.id()).result.is_err());
    assert!(info(app.world(), document).dirty);
    assert!(info(app.world(), document).conflict);
    assert_eq!(
        document_conflict(app.world(), document)
            .unwrap()
            .unwrap()
            .base
            .as_deref(),
        Some("base")
    );
}

#[cfg(feature = "ui")]
#[test]
fn retirement_during_drawing_is_rejected_without_closing_viewers() {
    let mut app = app();
    let document = text(app.world_mut(), "base");
    let viewer = panel(app.world_mut(), "Viewer");
    bind_document_view(app.world_mut(), viewer, document).unwrap();
    app.world_mut()
        .get_resource_mut::<crate::WorkbenchUi>()
        .unwrap()
        .drawing = true;
    assert!(close_document(app.world_mut(), document.id()).is_err());
    assert!(discard_document(app.world_mut(), document.id()).is_err());
    assert!(app.world().get::<PanelInstance>(viewer).is_some());
    assert!(read_document(app.world(), document).is_ok());
    app.world_mut()
        .get_resource_mut::<crate::WorkbenchUi>()
        .unwrap()
        .drawing = false;
    close_document(app.world_mut(), document.id()).unwrap();
}

#[test]
fn viewers_share_documents_across_layouts_and_do_not_own_them() {
    let mut app = app();
    let document = create_memory_snapshot(
        app.world_mut(),
        "base".to_owned(),
        DocumentOptions::loaded("File"),
    )
    .unwrap();
    let mut w = Workbench::new(app.world_mut());
    let first = w.create_panel(w.active_layout(), "One").unwrap();
    let other = w.create_layout("Other").unwrap();
    let second = w.create_panel(other, "Two").unwrap();
    bind_document_view(w.world_mut(), first, document).unwrap();
    bind_document_view(w.world_mut(), second, document).unwrap();
    w.close_panel(first).unwrap();
    w.delete_layout(other).unwrap();
    assert_eq!(read_document(w.world(), document).unwrap(), "base");
}
