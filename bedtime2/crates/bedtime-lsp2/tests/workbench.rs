#![cfg(all(unix, feature = "workbench"))]
use bedtime_editor2::{
    BufferEdit, TextBuffer,
    workbench::{bind_text_editor, unbind_text_editor},
};
use bedtime_lsp2::{FileUri, ServerConfig, ServerStatus, workbench::*};
use bedtime_workbench2::*;
use bevy::prelude::*;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    app: App,
    workspace: Entity,
    doc: DocumentHandle<TextBuffer>,
    panel: Entity,
    root: PathBuf,
}
impl Fixture {
    fn new(scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bedtime-lsp2-world-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, LspPlugin));
        let workspace = Workspaces::new(app.world_mut()).open("Code").entity();
        app.world_mut().entity_mut(workspace).insert(LspWorkspace {
            root: root.clone(),
            servers: [(
                "rust".into(),
                ServerConfig {
                    program: "python3".into(),
                    arguments: vec![
                        format!("{}/tests/server.py", env!("CARGO_MANIFEST_DIR")),
                        scenario.into(),
                        root.join("transcript.jsonl").to_str().unwrap().into(),
                    ],
                    ..Default::default()
                },
            )]
            .into(),
        });
        let doc = open_document(
            app.world_mut(),
            workspace,
            TextBuffer::from("source"),
            DocumentOptions::loaded("source.rs"),
        )
        .unwrap();
        app.world_mut().entity_mut(doc.id().0).insert(LspDocument {
            uri: FileUri::from_path(&root.join("source.rs")).unwrap(),
            language: "rust".into(),
        });
        let panel = {
            let mut w = Workbench::for_entity(app.world_mut(), workspace).unwrap();
            let layout = w.active_layout();
            w.create_panel(layout, "Code").unwrap()
        };
        bind_text_editor(app.world_mut(), panel, doc).unwrap();
        Self {
            app,
            workspace,
            doc,
            panel,
            root,
        }
    }
    fn wait(&mut self, mut predicate: impl FnMut(&World) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.update();
            if predicate(self.app.world()) {
                return;
            }
            assert!(
                Instant::now() < until,
                "timed out: {:?}",
                server_status(self.app.world(), self.doc.id())
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn ready(&mut self) {
        let doc = self.doc.id();
        self.wait(|w| server_status(w, doc) == Some(ServerStatus::Ready));
    }
    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("transcript.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect()
    }
    fn definition(&mut self) {
        request_definition(self.app.world_mut(), self.panel).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            self.app.update();
            if !take_definitions(self.app.world_mut()).is_empty() {
                return;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Workspaces::new(self.app.world_mut()).shutdown_work();
        let until = Instant::now() + Duration::from_secs(3);
        while !Workspaces::new(self.app.world_mut()).work_finished() && Instant::now() < until {
            self.app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn sibling_hidden_views_share_one_stream_and_dirty_viewless_documents_keep_sync() {
    let mut f = Fixture::new("basic");
    let second = {
        let mut w = Workbench::for_entity(f.app.world_mut(), f.workspace).unwrap();
        let layout = w.active_layout();
        w.create_panel(layout, "Sibling").unwrap()
    };
    bind_text_editor(f.app.world_mut(), second, f.doc).unwrap();
    f.ready();
    f.definition();
    assert_eq!(
        f.records()
            .iter()
            .filter(|r| r["method"] == "textDocument/didOpen")
            .count(),
        1
    );
    edit_document(
        f.app.world_mut(),
        f.doc,
        0,
        BufferEdit::new(0..0, "x"),
        EditOptions::new("Typing"),
    )
    .unwrap();
    unbind_text_editor(f.app.world_mut(), f.panel).unwrap();
    unbind_text_editor(f.app.world_mut(), second).unwrap();
    f.app.update();
    assert!(f.app.world().get::<DocumentLsp>(f.doc.id().0).is_some());
    undo_document(f.app.world_mut(), f.doc.id()).unwrap();
    let doc = f.doc.id();
    f.wait(|world| world.get::<DocumentLsp>(doc.0).is_none());
    let until = Instant::now() + Duration::from_secs(5);
    while !f
        .records()
        .iter()
        .any(|r| r["method"] == "textDocument/didClose")
    {
        f.app.update();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        read_document(f.app.world(), f.doc).unwrap().to_string(),
        "source"
    );
}
#[test]
fn journal_gaps_undo_redo_and_inactive_workspaces_synchronize_canonical_text() {
    let mut f = Fixture::new("basic");
    f.ready();
    f.definition();
    for revision in 0..140 {
        edit_document(
            f.app.world_mut(),
            f.doc,
            revision,
            BufferEdit::new(0..0, "x"),
            EditOptions::new("Edit"),
        )
        .unwrap();
    }
    undo_document(f.app.world_mut(), f.doc.id()).unwrap();
    redo_document(f.app.world_mut(), f.doc.id()).unwrap();
    let other = Workspaces::new(f.app.world_mut()).open("Other");
    Workspaces::new(f.app.world_mut()).activate(other).unwrap();
    f.definition();
    let text = read_document(f.app.world(), f.doc).unwrap().to_string();
    let until = Instant::now() + Duration::from_secs(5);
    while !f
        .records()
        .iter()
        .any(|r| r["method"] == "textDocument/definition" && r["mirror"] == text)
    {
        f.app.update();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let changes: Vec<_> = f
        .records()
        .into_iter()
        .filter(|r| r["method"] == "textDocument/didChange")
        .collect();
    assert_eq!(changes.len(), 1);
    assert!(
        changes[0]["params"]["contentChanges"][0]
            .get("range")
            .is_none()
    );
}
#[test]
fn source_changes_and_restart_invalidate_pending_navigation() {
    let mut f = Fixture::new("slow-definition");
    f.ready();
    request_definition(f.app.world_mut(), f.panel).unwrap();
    f.app.update();
    edit_document(
        f.app.world_mut(),
        f.doc,
        0,
        BufferEdit::new(0..0, "x"),
        EditOptions::default(),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < until {
        f.app.update();
        assert!(take_definitions(f.app.world_mut()).is_empty());
        std::thread::sleep(Duration::from_millis(5));
    }
    f.app
        .world_mut()
        .get_mut::<LspDocument>(f.doc.id().0)
        .unwrap()
        .uri = FileUri::from_path(&f.root.join("moved.rs")).unwrap();
    f.app.update();
    f.ready();
    f.definition();
    let doc = f.doc.id();
    restart(f.app.world_mut(), doc).unwrap();
    f.app.update();
    f.ready();
    f.definition();
    let pids: std::collections::HashSet<_> = f
        .records()
        .iter()
        .filter(|r| r["method"] == "initialize")
        .map(|r| r["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(pids.len(), 2);
}
#[test]
fn stopping_suppresses_relaunch_until_explicit_restart_and_preserves_document_edits() {
    let mut f = Fixture::new("basic");
    f.ready();
    f.definition();
    let server = document_server(f.app.world(), f.doc.id()).unwrap();
    stop_instance(f.app.world_mut(), server).unwrap();
    assert!(request_definition(f.app.world_mut(), f.panel).is_err());
    let doc = f.doc.id();
    f.wait(|world| world.get::<DocumentLsp>(doc.0).is_none());
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        f.app.update();
        let servers = workspace_servers(f.app.world_mut(), f.workspace);
        assert_eq!(servers.len(), 1);
        if !servers[0].retiring && servers[0].status == ServerStatus::Stopped {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    edit_document(
        f.app.world_mut(),
        f.doc,
        0,
        BufferEdit::new(0..0, "edited "),
        EditOptions::default(),
    )
    .unwrap();
    for _ in 0..10 {
        f.app.update();
        assert!(f.app.world().get::<DocumentLsp>(f.doc.id().0).is_none());
    }
    assert_eq!(
        f.records()
            .iter()
            .filter(|r| r["method"] == "initialize")
            .count(),
        1
    );
    assert!(f.records().iter().any(|r| r["method"] == "exit"));
    restart_instance(f.app.world_mut(), server).unwrap();
    f.ready();
    f.definition();
    assert_eq!(
        f.records()
            .iter()
            .filter(|r| r["method"] == "initialize")
            .count(),
        2
    );
    assert_eq!(
        read_document(f.app.world(), f.doc).unwrap().to_string(),
        "edited source"
    );
}

#[test]
fn large_documents_do_not_launch_language_servers() {
    let mut f = Fixture::new("basic");
    edit_document(
        f.app.world_mut(),
        f.doc,
        0,
        BufferEdit::new(0..6, "x".repeat(bedtime_lsp2::MAX_SOURCE_BYTES + 1)),
        EditOptions::default(),
    )
    .unwrap();
    for _ in 0..5 {
        f.app.update();
    }
    assert!(f.app.world().get::<DocumentLsp>(f.doc.id().0).is_none());
    assert!(f.records().is_empty());
}

#[test]
fn hover_and_completion_have_independent_tickets_and_edits_invalidate_both() {
    use bedtime_lsp2::{Intelligence, LanguageQuery, QueryKind, completion_context};
    let mut f = Fixture::new("basic");
    f.ready();
    let hover =
        request_query(f.app.world_mut(), f.panel, LanguageQuery::Hover { byte: 2 }).unwrap();
    let completion = request_query(
        f.app.world_mut(),
        f.panel,
        LanguageQuery::Completion {
            byte: 0,
            context: completion_context(None, false),
        },
    )
    .unwrap();
    assert!(query_is_current(f.app.world(), &hover, QueryKind::Hover));
    assert!(query_is_current(
        f.app.world(),
        &completion,
        QueryKind::Completion
    ));
    request_definition_at(f.app.world_mut(), f.panel, 3).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let mut kinds = Vec::new();
    while kinds.len() < 2 {
        f.app.update();
        for reply in take_intelligence(f.app.world_mut()) {
            match reply.result.unwrap() {
                Intelligence::Hover(_) => kinds.push(QueryKind::Hover),
                Intelligence::Completion(_) => kinds.push(QueryKind::Completion),
                _ => panic!("unexpected resolve"),
            }
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let revision = inspect_document(f.app.world(), f.doc.id())
        .unwrap()
        .revision;
    edit_document(
        f.app.world_mut(),
        f.doc,
        revision,
        BufferEdit::new(0..0, "!"),
        EditOptions::new("Edit"),
    )
    .unwrap();
    assert!(!query_is_current(f.app.world(), &hover, QueryKind::Hover));
    assert!(!query_is_current(
        f.app.world(),
        &completion,
        QueryKind::Completion
    ));
    f.app.update();
    assert!(take_intelligence(f.app.world_mut()).is_empty());
}
