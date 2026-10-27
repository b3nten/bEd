use super::*;
use crate::WorkbenchCorePlugin;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub(crate) fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    crate::tests::layout(app.world_mut());
    app
}

pub(crate) fn update_until(app: &mut App, done: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app.world()) {
        assert!(Instant::now() < deadline, "Timed out");
        app.update();
        std::thread::yield_now();
    }
}

fn info(world: &World, id: DocumentId) -> DocumentInfo {
    inspect_document(world, id).unwrap()
}

/// Open a document and wait for it to load.
pub(crate) fn open<M: DocumentModel>(
    app: &mut App,
    source: impl DocumentSource<M>,
) -> DocumentHandle<M> {
    let document = open_document(app.world_mut(), source).unwrap();
    update_until(app, |world| {
        info(world, document.id()).status != DocumentStatus::Loading
    });
    document
}

pub(crate) fn panel(world: &mut World) -> Entity {
    let mut workbench = Workbench::new(world);
    let layout = workbench.active_layout();
    workbench.create_panel(layout, "Viewer").unwrap()
}

/// Insert `text` at `at` as its own undo step.
pub(crate) fn type_text(
    world: &mut World,
    document: DocumentHandle<TextBuffer>,
    at: usize,
    text: &str,
) {
    let revision = info(world, document.id()).revision;
    edit_document(
        world,
        document,
        revision,
        BufferEdit::new(at..at, text),
        EditOptions::new("Type"),
    )
    .unwrap();
}

fn content(world: &World, document: DocumentHandle<TextBuffer>) -> String {
    read_document(world, document).unwrap().to_string()
}

fn save_and_wait(app: &mut App, id: DocumentId) -> Result<(), String> {
    save_document(app.world_mut(), id).unwrap();
    update_until(app, |world| !info(world, id).saving);
    info(app.world(), id).error.map_or(Ok(()), Err)
}

fn refresh_and_wait(app: &mut App, id: DocumentId) {
    refresh_document(app.world_mut(), id).unwrap();
    app.update();
    update_until(app, |world| {
        world
            .get::<Document>(id.0)
            .is_some_and(|document| !document.store.reading())
    });
}

/// A text source whose reads and saves can be held back or made to fail.
#[derive(Clone)]
pub(crate) struct TestSource {
    pub memory: MemorySource<TextBuffer>,
    control: Arc<Mutex<Control>>,
}

#[derive(Default)]
struct Control {
    fail_saves: Option<String>,
    held_save: Option<mpsc::Receiver<()>>,
    held_read: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    check_interval: Option<Duration>,
    read_only: bool,
}

impl TestSource {
    pub(crate) fn new(contents: &str) -> Self {
        Self {
            memory: MemorySource::new("notes.txt", TextBuffer::from(contents)),
            control: Arc::default(),
        }
    }

    pub(crate) fn fail_saves(&self, error: Option<&str>) {
        self.control.lock().unwrap().fail_saves = error.map(Into::into);
    }

    /// The next save waits until the returned sender is used or dropped.
    pub(crate) fn hold_next_save(&self) -> mpsc::Sender<()> {
        let (release, wait) = mpsc::channel();
        self.control.lock().unwrap().held_save = Some(wait);
        release
    }

    /// The next read loads, says so, then waits before reporting what it read.
    fn hold_next_read(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (loaded, has_loaded) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        self.control.lock().unwrap().held_read = Some((loaded, wait));
        (has_loaded, release)
    }
}

impl DocumentSource<TextBuffer> for TestSource {
    fn id(&self) -> SourceId {
        self.memory.id()
    }
    fn title(&self) -> String {
        self.memory.title()
    }
    fn load(&self) -> Result<SourceLoad<TextBuffer>, SourceError> {
        let loaded = self.memory.load();
        let held = self.control.lock().unwrap().held_read.take();
        if let Some((has_loaded, wait)) = held {
            let _ = has_loaded.send(());
            let _ = wait.recv();
        }
        loaded
    }
    fn save(
        &self,
        model: TextBuffer,
        expected: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        let held = self.control.lock().unwrap().held_save.take();
        if let Some(wait) = held {
            let _ = wait.recv();
        }
        if let Some(error) = self.control.lock().unwrap().fail_saves.clone() {
            return Err(SourceError::Failed(error));
        }
        self.memory.save(model, expected)
    }
    fn writable(&self) -> bool {
        !self.control.lock().unwrap().read_only
    }
    fn version(&self) -> Result<Option<SourceVersion>, SourceError> {
        self.memory.version()
    }
    fn check_interval(&self) -> Option<Duration> {
        self.control.lock().unwrap().check_interval
    }
}

#[test]
fn opening_a_source_twice_returns_the_same_document() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("text"));
    let first = open_document(app.world_mut(), source.clone()).unwrap();
    let second = open_document(app.world_mut(), source).unwrap();
    assert_eq!(first, second);
    assert_eq!(document_ids(app.world()), [first.id()]);
    #[derive(Clone, PartialEq)]
    struct Other;
    impl DocumentModel for Other {
        type Change = ();
        fn apply(&mut self, _: &()) -> Result<(), String> {
            Ok(())
        }
    }
    assert!(matches!(
        cast_document::<Other>(app.world(), first.id()),
        Err(DocumentError::WrongModel { .. })
    ));
}

#[test]
fn a_failed_load_is_reported_and_refresh_tries_again() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("text"));
    source.remove();
    let document = open(&mut app, source.clone());
    assert!(matches!(
        info(app.world(), document.id()).status,
        DocumentStatus::Failed(_)
    ));
    assert!(matches!(
        read_document(app.world(), document),
        Err(DocumentError::LoadFailed(_))
    ));

    source.set(TextBuffer::from("back"));
    refresh_and_wait(&mut app, document.id());
    assert_eq!(
        info(app.world(), document.id()).status,
        DocumentStatus::Ready
    );
    assert_eq!(content(app.world(), document), "back");
}

#[test]
fn edits_undo_and_redo_and_dirty_follows_the_saved_content() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("abc")));
    let world = app.world_mut();
    assert!(!info(world, document.id()).dirty);

    type_text(world, document, 3, "d");
    assert_eq!(content(world, document), "abcd");
    assert!(info(world, document.id()).dirty);
    assert_eq!(
        info(world, document.id()).undo_label.as_deref(),
        Some("Type")
    );

    undo_document(world, document.id()).unwrap();
    assert!(!info(world, document.id()).dirty);
    redo_document(world, document.id()).unwrap();
    assert_eq!(content(world, document), "abcd");
    assert_eq!(undo_document(world, document.id()).map(|_| ()), Ok(()));
    assert_eq!(
        undo_document(world, document.id()),
        Err(DocumentError::NothingToUndo)
    );
}

#[test]
fn stale_and_invalid_edits_change_nothing() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("abc")));
    let world = app.world_mut();
    let edit = |world: &mut World, revision, range: std::ops::Range<usize>| {
        edit_document(
            world,
            document,
            revision,
            BufferEdit::new(range, "x"),
            EditOptions::default(),
        )
    };
    assert!(matches!(
        edit(world, 7, 0..0),
        Err(DocumentError::StaleRevision { .. })
    ));
    assert!(matches!(
        edit(world, 0, 2..9),
        Err(DocumentError::InvalidChange(_))
    ));
    // A transaction applies entirely or not at all.
    let result = edit_document_transaction(
        world,
        document,
        0,
        [BufferEdit::new(0..0, "x"), BufferEdit::new(50..50, "y")],
        EditOptions::default(),
    );
    assert!(matches!(result, Err(DocumentError::InvalidChange(_))));
    assert_eq!(content(world, document), "abc");
    assert_eq!(info(world, document.id()).revision, 0);
}

#[test]
fn grouped_edits_undo_together_and_groups_belong_to_their_viewer() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("")));
    let world = app.world_mut();
    let viewer = panel(world);
    let stranger = panel(world);
    assert_eq!(
        begin_document_undo_group(world, document.id(), stranger),
        Err(DocumentError::NotReferenced(stranger))
    );
    world
        .entity_mut(viewer)
        .insert(DocumentReference::new(document.id()));
    let group = begin_document_undo_group(world, document.id(), viewer).unwrap();
    for (revision, letter) in ["a", "b", "c"].into_iter().enumerate() {
        let options = EditOptions::grouped("Typing", group);
        let edit = BufferEdit::new(revision..revision, letter);
        edit_document(world, document, revision as u64, edit, options).unwrap();
    }
    end_document_undo_group(world, document.id(), group).unwrap();
    undo_document(world, document.id()).unwrap();
    assert_eq!(content(world, document), "");

    // Removing the viewer's reference ends its group.
    redo_document(world, document.id()).unwrap();
    let group = begin_document_undo_group(world, document.id(), viewer).unwrap();
    world.entity_mut(viewer).remove::<DocumentReference>();
    assert_eq!(
        end_document_undo_group(world, document.id(), group),
        Err(DocumentError::StaleUndoGroup)
    );
}

#[test]
fn changes_since_reports_each_commit() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("")));
    let world = app.world_mut();
    type_text(world, document, 0, "a");
    type_text(world, document, 1, "b");
    let commits = document_changes_since(world, document, 0).unwrap().unwrap();
    assert_eq!(
        commits.iter().map(|c| c.revision).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        document_changes_since(world, document, 2)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(document_changes_since(world, document, 3).is_err());
}

#[test]
fn unsaved_sources_start_dirty_and_saving_makes_them_clean() {
    let mut app = app();
    let source = MemorySource::unsaved("Untitled", TextBuffer::from("new"));
    let document = open(&mut app, source.clone());
    assert!(info(app.world(), document.id()).dirty);
    save_and_wait(&mut app, document.id()).unwrap();
    assert!(!info(app.world(), document.id()).dirty);
    assert_eq!(source.get().unwrap().to_string(), "new");
}

#[test]
fn a_save_writes_its_snapshot_and_later_edits_stay_unsaved() {
    let mut app = app();
    let source = TestSource::new("a");
    let document = open(&mut app, source.clone());
    type_text(app.world_mut(), document, 1, "b");
    let release = source.hold_next_save();
    save_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Saving)
    );
    type_text(app.world_mut(), document, 2, "c");
    release.send(()).unwrap();
    update_until(&mut app, |world| !info(world, document.id()).saving);

    assert_eq!(source.memory.get().unwrap().to_string(), "ab");
    assert_eq!(content(app.world(), document), "abc");
    assert!(info(app.world(), document.id()).dirty);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert!(!info(app.world(), document.id()).dirty);
}

#[test]
fn a_failed_save_keeps_everything_and_reports_the_error() {
    let mut app = app();
    let source = TestSource::new("a");
    let document = open(&mut app, source.clone());
    type_text(app.world_mut(), document, 1, "b");
    source.fail_saves(Some("disk full"));
    let error = save_and_wait(&mut app, document.id()).unwrap_err();
    assert!(error.contains("disk full"));
    let finished: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<DocumentSaveFinished>>()
        .drain()
        .collect();
    assert!(
        finished[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("disk full")
    );
    assert_eq!(content(app.world(), document), "ab");
    assert!(info(app.world(), document.id()).dirty);

    source.fail_saves(None);
    save_and_wait(&mut app, document.id()).unwrap();
    assert!(info(app.world(), document.id()).error.is_none());
}

#[test]
fn read_only_sources_refuse_to_save() {
    let mut app = app();
    let source = TestSource::new("a");
    source.control.lock().unwrap().read_only = true;
    let document = open(&mut app, source);
    type_text(app.world_mut(), document, 1, "b");
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::ReadOnly)
    );
}

#[test]
fn outside_changes_apply_to_a_clean_document_as_an_undoable_edit() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("mine"));
    let document = open(&mut app, source.clone());
    source.set(TextBuffer::from("theirs"));
    refresh_and_wait(&mut app, document.id());
    assert_eq!(content(app.world(), document), "theirs");
    assert!(!info(app.world(), document.id()).dirty);

    // Undo brings back the old content, which is now an unsaved change.
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(content(app.world(), document), "mine");
    assert!(info(app.world(), document.id()).dirty);
}

#[test]
fn outside_changes_with_unsaved_edits_conflict_and_block_saving() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("base"));
    let document = open(&mut app, source.clone());
    type_text(app.world_mut(), document, 4, " mine");
    source.set(TextBuffer::from("base theirs"));
    refresh_and_wait(&mut app, document.id());

    let world = app.world_mut();
    assert!(info(world, document.id()).conflict);
    assert_eq!(content(world, document), "base mine");
    let conflict = document_conflict(world, document).unwrap().unwrap();
    assert_eq!(conflict.base.unwrap().to_string(), "base");
    assert_eq!(conflict.incoming.to_string(), "base theirs");
    assert_eq!(
        save_document(world, document.id()),
        Err(DocumentError::Conflict)
    );

    // Keeping ours makes the next save expect what came in, so it succeeds.
    let revision = info(world, document.id()).revision;
    resolve_document_conflict(
        world,
        document,
        revision,
        DocumentConflictResolution::KeepLocal,
    )
    .unwrap();
    assert!(info(world, document.id()).dirty);
    save_and_wait(&mut app, document.id()).unwrap();
    assert_eq!(source.get().unwrap().to_string(), "base mine");
}

#[test]
fn conflicts_resolve_to_the_incoming_or_a_merged_version() {
    for (resolution, expected) in [
        (DocumentConflictResolution::AcceptIncoming, "theirs"),
        (
            DocumentConflictResolution::Merged(TextBuffer::from("both")),
            "both",
        ),
    ] {
        let mut app = app();
        let source = MemorySource::new("a", TextBuffer::from("base"));
        let document = open(&mut app, source.clone());
        type_text(app.world_mut(), document, 0, "mine ");
        source.set(TextBuffer::from("theirs"));
        refresh_and_wait(&mut app, document.id());
        let world = app.world_mut();
        assert_eq!(
            resolve_document_conflict(world, document, 0, resolution.clone()),
            Err(DocumentError::StaleRevision {
                expected: 0,
                actual: 1
            })
        );
        resolve_document_conflict(world, document, 1, resolution).unwrap();
        assert_eq!(content(world, document), expected);
        assert!(!info(world, document.id()).conflict);
        assert_eq!(info(world, document.id()).dirty, expected != "theirs");
    }
}

#[test]
fn a_save_refused_because_the_source_changed_becomes_a_conflict() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("base"));
    let document = open(&mut app, source.clone());
    type_text(app.world_mut(), document, 4, " mine");
    source.set(TextBuffer::from("theirs"));
    assert!(save_and_wait(&mut app, document.id()).is_err());
    update_until(&mut app, |world| info(world, document.id()).conflict);
    assert_eq!(source.get().unwrap().to_string(), "theirs");
}

#[test]
fn a_read_still_running_when_a_save_starts_is_dropped() {
    let mut app = app();
    let source = TestSource::new("old");
    let document = open(&mut app, source.clone());
    // The read loads "old" now, but reports it only after the save.
    let (has_loaded, release) = source.hold_next_read();
    refresh_document(app.world_mut(), document.id()).unwrap();
    app.update();
    has_loaded.recv_timeout(Duration::from_secs(5)).unwrap();
    type_text(app.world_mut(), document, 3, " new");
    save_and_wait(&mut app, document.id()).unwrap();
    drop(release);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(content(app.world(), document), "old new");
    assert!(!info(app.world(), document.id()).dirty);
}

#[test]
fn a_missing_source_keeps_the_content_and_refuses_saves() {
    let mut app = app();
    let source = MemorySource::new("a", TextBuffer::from("text"));
    let document = open(&mut app, source.clone());
    source.remove();
    refresh_and_wait(&mut app, document.id());
    assert!(info(app.world(), document.id()).missing);
    assert_eq!(content(app.world(), document), "text");
    type_text(app.world_mut(), document, 0, "more ");
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::Missing)
    );

    source.set(TextBuffer::from("text"));
    refresh_and_wait(&mut app, document.id());
    assert!(!info(app.world(), document.id()).missing);
}

#[test]
fn sources_that_can_tell_they_changed_are_checked_periodically() {
    let mut app = app();
    let source = TestSource::new("mine");
    source.control.lock().unwrap().check_interval = Some(Duration::from_millis(1));
    let document = open(&mut app, source.clone());
    source.memory.set(TextBuffer::from("theirs"));
    update_until(&mut app, |world| content(world, document) == "theirs");
}

#[test]
fn drafts_block_saving_undo_and_closing() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("a")));
    let world = app.world_mut();
    type_text(world, document, 1, "b");
    let viewer = panel(world);
    world
        .entity_mut(viewer)
        .insert((DocumentReference::new(document.id()), DocumentDraft));
    assert_eq!(
        save_document(world, document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        undo_document(world, document.id()),
        Err(DocumentError::Drafts)
    );
    assert_eq!(
        discard_document(world, document.id()),
        Err(DocumentError::Drafts)
    );
    world.entity_mut(viewer).remove::<DocumentDraft>();
    undo_document(world, document.id()).unwrap();
}

#[test]
fn closing_a_document_closes_its_panels_and_is_refused_while_unsaved() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("a")));
    let other = open(&mut app, MemorySource::new("b", TextBuffer::from("b")));
    let world = app.world_mut();
    let viewers = [panel(world), panel(world)];
    for viewer in viewers {
        world
            .entity_mut(viewer)
            .insert(DocumentReference::new(document.id()));
    }
    let unrelated = panel(world);
    world
        .entity_mut(unrelated)
        .insert(DocumentReference::new(other.id()));
    type_text(world, document, 1, "b");
    assert_eq!(
        close_document(world, document.id()),
        Err(DocumentError::Dirty)
    );

    discard_document(world, document.id()).unwrap();
    assert!(
        viewers
            .iter()
            .all(|&viewer| world.get_entity(viewer).is_err())
    );
    assert!(world.get_entity(unrelated).is_ok());
    assert_eq!(document_ids(world), [other.id()]);
}

#[test]
fn panels_look_dirty_while_showing_an_unsaved_document() {
    let mut app = app();
    let document = open(&mut app, MemorySource::new("a", TextBuffer::from("a")));
    let viewer = panel(app.world_mut());
    app.world_mut()
        .entity_mut(viewer)
        .insert(DocumentReference::new(document.id()));
    type_text(app.world_mut(), document, 1, "b");
    app.update();
    assert!(app.world().get::<PanelInstance>(viewer).unwrap().dirty);
    app.world_mut()
        .entity_mut(viewer)
        .remove::<DocumentReference>();
    assert!(!app.world().get::<PanelInstance>(viewer).unwrap().dirty);
}

#[test]
fn shutting_down_waits_for_saves_in_flight() {
    let mut app = app();
    let source = TestSource::new("a");
    let document = open(&mut app, source.clone());
    type_text(app.world_mut(), document, 1, "b");
    let release = source.hold_next_save();
    save_document(app.world_mut(), document.id()).unwrap();
    assert!(crate::shutdown_work(app.world_mut()).is_err());
    release.send(()).unwrap();
    update_until(&mut app, |world| !info(world, document.id()).saving);
    crate::shutdown_work(app.world_mut()).unwrap();
    update_until(&mut app, crate::work_finished);
    assert_eq!(source.memory.get().unwrap().to_string(), "ab");
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::SessionClosing)
    );
}
