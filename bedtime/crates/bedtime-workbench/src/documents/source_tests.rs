use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[derive(Clone)]
struct Source {
    id: SourceId,
    loads: Arc<AtomicUsize>,
    fail: bool,
}
impl DocumentSource<String> for Source {
    fn id(&self) -> SourceId {
        self.id
    }
    fn title(&self) -> String {
        "Source".into()
    }
    fn load(&self) -> Result<SourceLoad<String>, SourceError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(SourceError::Failed("Unavailable".into()))
        } else {
            Ok(SourceLoad {
                model: "hello".into(),
                version: Some("one".into()),
                acknowledged: true,
            })
        }
    }
    fn save(&self, _: SourceSave<String>) -> Result<Option<SourceVersion>, SourceError> {
        Err(SourceError::Uncertain)
    }
}
impl DocumentSource<TextDocument> for Source {
    fn id(&self) -> SourceId {
        self.id
    }
    fn title(&self) -> String {
        "Source".into()
    }
    fn load(&self) -> Result<SourceLoad<TextDocument>, SourceError> {
        Ok(SourceLoad {
            model: "hello".into(),
            version: None,
            acknowledged: true,
        })
    }
    fn save(&self, _: SourceSave<TextDocument>) -> Result<Option<SourceVersion>, SourceError> {
        unreachable!()
    }
}
fn wait(app: &mut App, id: DocumentId) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while inspect_document(app.world(), id).unwrap().load_state == DocumentLoadState::Loading {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn source_identity_reserves_type_before_loading_and_retains_failures() {
    for fail in [false, true] {
        let mut app = App::new();
        app.add_plugins(crate::WorkbenchCorePlugin);
        let source = Source {
            id: SourceId::new(),
            loads: Arc::new(AtomicUsize::new(0)),
            fail,
        };
        let first = open_document::<String>(app.world_mut(), source.clone()).unwrap();
        let again = open_document::<String>(app.world_mut(), source.clone()).unwrap();
        assert_eq!(first, again);
        assert!(matches!(
            open_document::<TextDocument>(app.world_mut(), source.clone()),
            Err(DocumentError::WrongModel { .. })
        ));
        wait(&mut app, first.id());
        assert_eq!(source.loads.load(Ordering::SeqCst), 1);
        assert_eq!(
            open_document::<String>(app.world_mut(), source.clone()).unwrap(),
            first
        );
        assert!(matches!(
            open_document::<TextDocument>(app.world_mut(), source),
            Err(DocumentError::WrongModel { .. })
        ));
    }
}
#[test]
fn uncertain_source_callback_requires_refresh_before_retry() {
    let mut app = App::new();
    app.add_plugins(crate::WorkbenchCorePlugin);
    let source = Source {
        id: SourceId::new(),
        loads: Arc::new(AtomicUsize::new(0)),
        fail: false,
    };
    let document = open_document::<String>(app.world_mut(), source).unwrap();
    wait(&mut app, document.id());
    edit_document(
        app.world_mut(),
        document,
        0,
        TextChange {
            range: 0..5,
            replacement: "edited".into(),
        },
        EditOptions::new("Edit"),
    )
    .unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while inspect_document(app.world(), document.id()).unwrap().saving {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let info = inspect_document(app.world(), document.id()).unwrap();
    assert!(info.dirty && info.requires_source_refresh);
    assert_eq!(
        save_document(app.world_mut(), document.id()),
        Err(DocumentError::SourceRefreshRequired)
    );
}
#[test]
fn text_byte_projection_tracks_unsaved_edits_and_history() {
    let mut app = App::new();
    app.add_plugins(crate::WorkbenchCorePlugin);
    let document = open_document::<TextDocument>(
        app.world_mut(),
        MemorySource::new("Text", TextDocument::from("abc")),
    )
    .unwrap();
    wait(&mut app, document.id());
    edit_document(
        app.world_mut(),
        document,
        0,
        bedtime_document::BufferEdit::new(1..2, "é"),
        EditOptions::new("Edit"),
    )
    .unwrap();
    let edited = document_bytes(app.world(), document.id()).unwrap();
    let stamp = edited.metadata().unwrap();
    assert_eq!(
        edited.read_range(&stamp.version, 0..stamp.len).unwrap(),
        "aéc".as_bytes()
    );
    undo_document(app.world_mut(), document.id()).unwrap();
    let undone = document_bytes(app.world(), document.id()).unwrap();
    assert_ne!(edited, undone);
    let stamp = undone.metadata().unwrap();
    assert_eq!(
        undone.read_range(&stamp.version, 0..stamp.len).unwrap(),
        b"abc"
    );
    assert_eq!(
        edited
            .read_range(&edited.metadata().unwrap().version, 1..2)
            .unwrap(),
        [0xc3]
    );
}
#[test]
fn unsaved_memory_source_can_be_saved_and_reopened_without_viewers() {
    let mut app = App::new();
    app.add_plugins(crate::WorkbenchCorePlugin);
    let source = MemorySource::unsaved("Scratch", TextDocument::from("work"));
    let document = open_document::<TextDocument>(app.world_mut(), source.clone()).unwrap();
    wait(&mut app, document.id());
    assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
    save_document(app.world_mut(), document.id()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while inspect_document(app.world(), document.id()).unwrap().saving {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    assert_eq!(
        open_document::<TextDocument>(app.world_mut(), source).unwrap(),
        document
    );
}

#[test]
fn structured_sources_do_not_require_byte_encoding() {
    #[derive(Clone, PartialEq)]
    struct Record(u32);
    impl DocumentModel for Record {
        type Change = u32;
        fn apply(&mut self, value: &u32) -> Result<u32, String> {
            Ok(std::mem::replace(&mut self.0, *value))
        }
    }
    #[derive(Clone)]
    struct Table {
        id: SourceId,
        value: Arc<std::sync::Mutex<Record>>,
    }
    impl DocumentSource<Record> for Table {
        fn id(&self) -> SourceId {
            self.id
        }
        fn title(&self) -> String {
            "Rows".into()
        }
        fn load(&self) -> Result<SourceLoad<Record>, SourceError> {
            Ok(SourceLoad {
                model: self.value.lock().unwrap().clone(),
                version: None,
                acknowledged: true,
            })
        }
        fn save(&self, capture: SourceSave<Record>) -> Result<Option<SourceVersion>, SourceError> {
            let mut current = self.value.lock().unwrap();
            if capture.baseline.as_ref() != Some(&*current) {
                return Err(SourceError::Changed("Table changed".into()));
            }
            *current = capture.model;
            Ok(None)
        }
    }
    let mut app = App::new();
    app.add_plugins(crate::WorkbenchCorePlugin);
    let table = Table {
        id: SourceId::new(),
        value: Arc::new(std::sync::Mutex::new(Record(1))),
    };
    let document = open_document(app.world_mut(), table.clone()).unwrap();
    wait(&mut app, document.id());
    edit_document(app.world_mut(), document, 0, 2, EditOptions::new("Cell")).unwrap();
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap().0, 1);
    redo_document(app.world_mut(), document.id()).unwrap();
    save_document(app.world_mut(), document.id()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while inspect_document(app.world(), document.id()).unwrap().saving {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(table.value.lock().unwrap().0, 2);
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
}

#[test]
fn periodic_source_checks_rotate_beyond_the_shared_task_budget() {
    use std::sync::atomic::AtomicU64;
    #[derive(Clone)]
    struct Changing {
        id: SourceId,
        version: Arc<AtomicU64>,
    }
    impl DocumentSource<String> for Changing {
        fn id(&self) -> SourceId {
            self.id
        }
        fn title(&self) -> String {
            "Changing".into()
        }
        fn load(&self) -> Result<SourceLoad<String>, SourceError> {
            let v = self.version.load(Ordering::Acquire).to_string();
            Ok(SourceLoad {
                model: v.clone(),
                version: Some(v.into()),
                acknowledged: true,
            })
        }
        fn observation_interval(&self) -> Option<std::time::Duration> {
            Some(std::time::Duration::ZERO)
        }
        fn observed_version(&self) -> Result<Option<SourceVersion>, SourceError> {
            Ok(Some(
                self.version.load(Ordering::Acquire).to_string().into(),
            ))
        }
        fn save(&self, _: SourceSave<String>) -> Result<Option<SourceVersion>, SourceError> {
            Err(SourceError::ReadOnly)
        }
    }
    let mut app = App::new();
    app.add_plugins(crate::WorkbenchCorePlugin);
    let version = Arc::new(AtomicU64::new(0));
    let documents: Vec<_> = (0..72)
        .map(|_| {
            open_document(
                app.world_mut(),
                Changing {
                    id: SourceId::new(),
                    version: version.clone(),
                },
            )
            .unwrap()
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while documents.iter().any(|h| {
        inspect_document(app.world(), h.id()).unwrap().load_state != DocumentLoadState::Ready
    }) {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    version.store(1, Ordering::Release);
    while documents
        .iter()
        .any(|h| read_document(app.world(), *h).unwrap() != "1")
    {
        app.update();
        assert!(std::time::Instant::now() < deadline);
        assert!(
            documents
                .iter()
                .filter(|h| inspect_document(app.world(), h.id())
                    .unwrap()
                    .source_pending)
                .count()
                <= 64
        );
        std::thread::yield_now();
    }
}
