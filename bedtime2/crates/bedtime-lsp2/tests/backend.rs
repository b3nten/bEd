#![cfg(unix)]
use bedtime_editor2::{BufferEdit, TextBuffer};
use bedtime_lsp2::*;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
#[test]
fn progress_and_logs_are_coalesced_without_filling_document_results() {
    let mut fixture = Fixture::new("progress");
    fixture.update(TextBuffer::from("fn main() {}"), 0, None);
    fixture.ready();
    fixture.wait(|| {
        fixture.handle.activity().messages.len() == 64
            && fixture.handle.activity().progress.len() == 2
            && !fixture.handle.activity().stderr.is_empty()
    });
    let activity = fixture.handle.activity();
    assert!(activity.process_id.is_some());
    assert_eq!(activity.server_name.as_deref(), Some("Bedtime fixture"));
    fixture.update(TextBuffer::from("fn changed() {}"), 1, None);
    fixture.wait(|| {
        let a = fixture.handle.activity();
        a.progress.len() == 1 && a.progress[0].percentage == Some(50)
    });
    assert_eq!(
        fixture.handle.activity().progress[0].message.as_deref(),
        Some("src")
    );
    assert!(matches!(
        fixture.handle.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    fixture
        .handle
        .saved(fixture.key(), TextBuffer::from("fn changed() {}"))
        .unwrap();
    fixture.wait(|| fixture.handle.activity().progress.is_empty());
    assert_eq!(fixture.handle.status(), ServerStatus::Ready);
    fixture.stop();
    assert!(fixture.handle.activity().progress.is_empty());
}
#[test]
fn cancellation_sends_shutdown_before_stopping_the_server() {
    let mut fixture = Fixture::new("basic");
    fixture.update(TextBuffer::from("fn main() {}"), 0, None);
    fixture.ready();
    fixture.stop();
    assert!(
        fixture.records().iter().any(|r| r["method"] == "shutdown"),
        "{:?}",
        fixture.records()
    );
    assert!(fixture.records().iter().any(|r| r["method"] == "exit"));
}
struct Fixture {
    root: PathBuf,
    handle: ServerHandle,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Fixture {
    fn new(scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bedtime-lsp2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (handle, worker) = ServerHandle::prepare(Launch {
            root: root.clone(),
            config: ServerConfig {
                program: "python3".into(),
                arguments: vec![
                    format!("{}/tests/server.py", env!("CARGO_MANIFEST_DIR")),
                    scenario.into(),
                    root.join("transcript.jsonl").to_str().unwrap().into(),
                ],
                ..Default::default()
            },
        })
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = thread::spawn(move || worker.run(|| stop.load(Ordering::Acquire)));
        Self {
            root,
            handle,
            cancel,
            worker: Some(worker),
        }
    }
    fn key(&self) -> DocumentKey {
        DocumentKey {
            id: 1,
            generation: 1,
        }
    }
    fn update(&self, text: TextBuffer, revision: u64, changes: Option<(u64, Vec<BufferEdit>)>) {
        self.handle
            .update(DocumentUpdate {
                key: self.key(),
                uri: FileUri::from_path(&self.root.join("sp ace🙂.rs")).unwrap(),
                language: "rust".into(),
                text,
                revision,
                changes,
            })
            .unwrap();
    }
    fn wait(&self, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "timed out: {:?}",
                self.handle.status()
            );
            if let ServerStatus::Failed(e) = self.handle.status() {
                panic!("{e}");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn ready(&self) {
        self.wait(|| self.handle.status() == ServerStatus::Ready);
    }
    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("transcript.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
    fn definition(&self, revision: u64, byte: usize) -> Result<Vec<Location>, String> {
        self.handle
            .definition(
                RequestOrigin {
                    document: self.key(),
                    revision,
                    panel: 1,
                    ticket: revision + 1,
                },
                byte,
            )
            .unwrap();
        let mut result = None;
        self.wait(|| {
            while let Ok(event) = self.handle.try_recv() {
                if let Event::Definition { result: r, .. } = event {
                    result = Some(r);
                }
            }
            result.is_some()
        });
        result.unwrap()
    }
    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.stop();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn initialization_uses_latest_root_and_closed_documents_do_not_resurrect() {
    let f = Fixture::new("delayed");
    f.update("old".into(), 0, None);
    f.update("latest🙂".into(), 4, None);
    f.ready();
    assert_eq!(f.definition(4, 0).unwrap().len(), 1);
    let opens: Vec<_> = f
        .records()
        .into_iter()
        .filter(|r| r["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0]["mirror"], "latest🙂");
    let g = Fixture::new("delayed");
    g.update("closed".into(), 0, None);
    g.handle.close(g.key()).unwrap();
    g.ready();
    g.wait(|| g.records().iter().any(|r| r["method"] == "initialized"));
    assert!(
        !g.records()
            .iter()
            .any(|r| r["method"] == "textDocument/didOpen")
    );
}
#[test]
fn incremental_unicode_changes_and_full_refresh_reach_identical_mirrors() {
    for scenario in ["basic", "utf8", "full"] {
        let f = Fixture::new(scenario);
        f.update("a🙂b\r\nnext".into(), 0, None);
        f.ready();
        f.definition(0, 1).unwrap();
        let edits = vec![
            BufferEdit::new(1..5, "é"),
            BufferEdit::new(3..4, "!"),
            BufferEdit::new(0..0, "X"),
        ];
        let mut text = TextBuffer::from("a🙂b\r\nnext");
        for e in &edits {
            e.apply(&mut text).unwrap();
        }
        f.update(text.clone(), 1, Some((0, edits)));
        f.definition(1, 4).unwrap();
        f.wait(|| {
            f.records().iter().any(|r| {
                r["method"] == "textDocument/definition" && r["mirror"] == text.to_string()
            })
        });
        let records = f.records();
        let change = records
            .iter()
            .find(|r| r["method"] == "textDocument/didChange")
            .unwrap();
        assert_eq!(change["mirror"], "Xaé!\r\nnext");
        assert_eq!(
            change["params"]["contentChanges"][0].get("range").is_some(),
            scenario != "full"
        );
        if scenario == "utf8" {
            assert_eq!(
                change["params"]["contentChanges"][0]["range"]["end"]["character"],
                5
            );
        } else if scenario == "basic" {
            assert_eq!(
                change["params"]["contentChanges"][0]["range"]["end"]["character"],
                3
            );
        }
        f.update("replacement".into(), 200, None);
        f.definition(200, 0).unwrap();
        f.wait(|| {
            f.records()
                .iter()
                .any(|r| r["method"] == "textDocument/definition" && r["mirror"] == "replacement")
        });
    }
}
#[test]
fn save_uses_written_capture_even_after_newer_edits() {
    let f = Fixture::new("basic");
    f.update("saved".into(), 0, None);
    f.ready();
    f.update("newer".into(), 1, None);
    f.handle.saved(f.key(), "saved".into()).unwrap();
    f.definition(1, 0).unwrap();
    f.wait(|| {
        f.records()
            .iter()
            .any(|r| r["method"] == "textDocument/didSave")
    });
    let record = f
        .records()
        .into_iter()
        .find(|r| r["method"] == "textDocument/didSave")
        .unwrap();
    assert_eq!(record["params"]["text"], "saved");
    assert_eq!(record["mirror"], "newer");
}
#[test]
fn omitted_sync_and_false_save_do_not_send_unrequested_notifications() {
    for scenario in ["none", "false-save"] {
        let f = Fixture::new(scenario);
        f.update("source".into(), 0, None);
        f.ready();
        f.handle.saved(f.key(), "source".into()).unwrap();
        f.definition(0, 0).unwrap();
        f.wait(|| {
            f.records()
                .iter()
                .any(|r| r["method"] == "textDocument/definition")
        });
        let records = f.records();
        assert!(
            !records
                .iter()
                .any(|r| r["method"] == "textDocument/didSave")
        );
        if scenario == "none" {
            assert!(
                !records
                    .iter()
                    .any(|r| r["method"] == "textDocument/didOpen")
            );
        }
    }
}
#[test]
fn diagnostic_reports_are_revision_bound_and_empty_reports_clear() {
    let f = Fixture::new("basic");
    f.update("source".into(), 0, None);
    f.ready();
    let mut report = None;
    f.wait(|| {
        while let Ok(event) = f.handle.try_recv() {
            if let Event::Diagnostics { report: r, .. } = event {
                report = Some(r);
            }
        }
        report.is_some()
    });
    let report = report.unwrap();
    assert_eq!(report.revision, 0);
    assert!(report.verified);
    assert_eq!(report.items()[0].range, 0..1);
    f.update("changed".into(), 1, None);
    let mut cleared = false;
    f.wait(|| {
        while let Ok(event) = f.handle.try_recv() {
            if let Event::Diagnostics { report, .. } = event {
                cleared = report.revision == 1 && report.items().is_empty();
            }
        }
        cleared
    });
}
#[test]
fn idle_blocked_and_inherited_pipes_cancel_and_reap_off_thread() {
    for scenario in ["basic", "blocked", "inherited"] {
        let mut f = Fixture::new(scenario);
        f.ready();
        f.wait(|| f.records().iter().any(|r| r["method"] == "initialized"));
        if scenario == "blocked" {
            f.update("x".repeat(512 * 1024).into(), 0, None);
            thread::sleep(Duration::from_millis(50));
        }
        let pid = f.records()[0]["pid"].as_u64().unwrap() as i32;
        let now = Instant::now();
        f.stop();
        assert!(now.elapsed() < Duration::from_secs(2), "{scenario}");
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "child was not reaped: {scenario}"
        );
    }
}
#[test]
fn large_sources_are_refused_without_sending_partial_content() {
    let f = Fixture::new("basic");
    f.ready();
    f.update("x".repeat(MAX_SOURCE_BYTES + 1).into(), 0, None);
    let mut refused = false;
    f.wait(|| {
        while let Ok(event) = f.handle.try_recv() {
            if matches!(event, Event::DocumentError { .. }) {
                refused = true;
            }
        }
        refused
    });
    assert!(
        !f.records()
            .iter()
            .any(|r| r["method"] == "textDocument/didOpen")
    );
}
#[test]
fn crashes_preserve_actionable_stderr() {
    let f = Fixture::new("crash");
    f.update("source".into(), 0, None);
    let until = Instant::now() + Duration::from_secs(5);
    while !matches!(f.handle.status(), ServerStatus::Failed(_)) {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        matches!(f.handle.status(),ServerStatus::Failed(error) if error.contains("fixture startup failure"))
    );
}

#[test]
fn crlf_splitting_edits_refresh_instead_of_failing_the_session() {
    let f = Fixture::new("basic");
    f.update("a\r\nb".into(), 0, None);
    f.ready();
    let edit = BufferEdit::new(2..3, "");
    let mut text = TextBuffer::from("a\r\nb");
    edit.apply(&mut text).unwrap();
    f.update(text, 1, Some((0, vec![edit])));
    f.definition(1, 0).unwrap();
    f.wait(|| {
        f.records()
            .iter()
            .any(|r| r["method"] == "textDocument/didChange")
    });
    let r = f
        .records()
        .into_iter()
        .find(|r| r["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(r["mirror"], "a\rb");
    assert!(r["params"]["contentChanges"][0].get("range").is_none());
}

#[test]
#[ignore = "requires an installed rust-analyzer and Rust toolchain"]
fn real_rust_analyzer_definition_and_unsaved_diagnostics() {
    let root = std::env::temp_dir().join(format!("bedtime-lsp2-real-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='lsp2_fixture'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    )
    .unwrap();
    let text = "fn answer() -> u32 { 42 }\nfn caller() { let _ = answer(); }\n";
    std::fs::write(root.join("src/lib.rs"), text).unwrap();
    let config = ServerConfig {
        environment: [("CARGO_NET_OFFLINE".into(), "true".into())].into(),
        settings: serde_json::json!({"rust-analyzer":{"checkOnSave":false,"cargo":{"sysroot":null},"procMacro":{"enable":false}}}),
        ..Default::default()
    };
    let (handle, worker) = ServerHandle::prepare(Launch {
        root: root.clone(),
        config,
    })
    .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let worker = thread::spawn(move || worker.run(|| stop.load(Ordering::Acquire)));
    let key = DocumentKey {
        id: 1,
        generation: 1,
    };
    let uri = FileUri::from_path(&root.join("src/lib.rs")).unwrap();
    handle
        .update(DocumentUpdate {
            key,
            uri: uri.clone(),
            language: "rust".into(),
            revision: 0,
            text: text.into(),
            changes: None,
        })
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let until = Instant::now() + Duration::from_secs(30);
        while handle.status() != ServerStatus::Ready {
            assert!(Instant::now() < until, "{:?}", handle.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut ticket = 0;
        let mut found = false;
        while Instant::now() < until && !found {
            ticket += 1;
            handle
                .definition(
                    RequestOrigin {
                        document: key,
                        revision: 0,
                        panel: 1,
                        ticket,
                    },
                    text.rfind("answer").unwrap(),
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match handle.try_recv() {
                    Ok(Event::Definition {
                        result: Ok(locations),
                        ..
                    }) => {
                        found = locations.iter().any(|l| {
                            l.uri == uri && l.range.start.line == 0 && l.range.start.character == 3
                        });
                        break;
                    }
                    Ok(Event::Definition { .. }) => break,
                    Ok(_) => {}
                    Err(_) => {
                        assert!(
                            Instant::now() < deadline,
                            "definition timed out: {:?}",
                            handle.status()
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            if !found {
                thread::sleep(Duration::from_millis(100));
            }
        }
        assert!(found, "rust-analyzer did not resolve the local definition");
        handle
            .update(DocumentUpdate {
                key,
                uri,
                language: "rust".into(),
                revision: 1,
                text: "fn broken( {".into(),
                changes: None,
            })
            .unwrap();
        let mut diagnostic = false;
        while Instant::now() < until && !diagnostic {
            while let Ok(event) = handle.try_recv() {
                if let Event::Diagnostics { report, .. } = event {
                    diagnostic = report.revision == 1 && !report.items().is_empty();
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            diagnostic,
            "rust-analyzer did not publish unsaved syntax diagnostics: {:?}",
            handle.status()
        );
    }));
    cancel.store(true, Ordering::Release);
    worker.join().unwrap();
    let _ = std::fs::remove_dir_all(root);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn hover_completion_and_resolution_use_the_synced_unicode_source() {
    let fixture = Fixture::new("basic");
    let source = TextBuffer::from("// 🙂\nfn main() { fi }\n");
    fixture.update(source.clone(), 0, None);
    fixture.ready();
    let byte = source.to_string().find("fi }").unwrap() + 2;
    let origin = RequestOrigin {
        document: fixture.key(),
        revision: 0,
        panel: 91,
        ticket: 1,
    };
    let receive = |kind| {
        let mut result = None;
        fixture.wait(|| {
            while let Ok(event) = fixture.handle.try_recv() {
                if let Event::Intelligence {
                    kind: k, result: r, ..
                } = event
                {
                    if k == kind {
                        result = Some(r);
                    }
                }
            }
            result.is_some()
        });
        result.unwrap().unwrap()
    };
    fixture
        .handle
        .query(origin, LanguageQuery::Hover { byte })
        .unwrap();
    let Intelligence::Hover(Some(hover)) = receive(QueryKind::Hover) else {
        panic!("hover missing");
    };
    assert!(hover.contents.markdown);
    assert!(hover.contents.text.contains("fn first()"));
    fixture
        .handle
        .query(
            origin,
            LanguageQuery::Completion {
                byte,
                context: completion_context(Some("."), false),
            },
        )
        .unwrap();
    let Intelligence::Completion(list) = receive(QueryKind::Completion) else {
        panic!("completion missing");
    };
    assert!(list.incomplete);
    assert_eq!(list.items[0].label, "first");
    assert_eq!(list.items[0].edit.primary.range, byte - 2..byte);
    assert!(list.items[0].edit.snippet);
    fixture
        .handle
        .query(
            origin,
            LanguageQuery::Resolve {
                byte,
                item: list.items[0].raw.clone(),
            },
        )
        .unwrap();
    let Intelligence::Resolved(resolved) = receive(QueryKind::Resolve) else {
        panic!("resolve missing");
    };
    assert!(resolved.documentation.text.contains("Resolved"));
    assert_eq!(resolved.edit.additional.len(), 1);
    let prepared = resolved.edit.prepare(&source).unwrap();
    let mut accepted = source.clone();
    prepared.edit.apply(&mut accepted).unwrap();
    assert_eq!(
        accepted.to_string(),
        "// auto import\n// 🙂\nfn main() { first(value) }\n"
    );
    assert!(
        fixture
            .records()
            .iter()
            .any(|r| r["method"] == "textDocument/completion"
                && r["params"]["context"]["triggerKind"] == 2)
    );
}
#[test]
fn superseded_hover_is_cancelled_and_late_response_is_ignored() {
    let fixture = Fixture::new("slow-intelligence");
    fixture.update(TextBuffer::from("fn first() {}"), 0, None);
    fixture.ready();
    let first = RequestOrigin {
        document: fixture.key(),
        revision: 0,
        panel: 1,
        ticket: 1,
    };
    let latest = RequestOrigin { ticket: 2, ..first };
    fixture
        .handle
        .query(first, LanguageQuery::Hover { byte: 4 })
        .unwrap();
    fixture
        .handle
        .query(latest, LanguageQuery::Hover { byte: 5 })
        .unwrap();
    let mut received = Vec::new();
    fixture.wait(|| {
        while let Ok(event) = fixture.handle.try_recv() {
            if let Event::Intelligence { origin, result, .. } = event {
                assert!(result.is_ok());
                received.push(origin);
            }
        }
        !received.is_empty()
    });
    assert_eq!(received, vec![latest]);
    fixture.wait(|| {
        fixture
            .records()
            .iter()
            .any(|r| r["method"] == "$/cancelRequest")
    });
    assert_eq!(fixture.handle.status(), ServerStatus::Ready);
}

#[test]
fn workspace_reload_errors_retry_but_repeated_errors_have_a_finite_limit() {
    let fixture = Fixture::new("transient");
    fixture.update(TextBuffer::from("fn first() {}"), 0, None);
    fixture.ready();
    assert_eq!(fixture.definition(0, 4).unwrap().len(), 1);
    let origin = RequestOrigin {
        document: fixture.key(),
        revision: 0,
        panel: 2,
        ticket: 2,
    };
    fixture
        .handle
        .query(origin, LanguageQuery::Hover { byte: 4 })
        .unwrap();
    let mut answer = None;
    fixture.wait(|| {
        while let Ok(event) = fixture.handle.try_recv() {
            if let Event::Intelligence { result, .. } = event {
                answer = Some(result);
            }
        }
        answer.is_some()
    });
    assert!(matches!(answer.unwrap(), Ok(Intelligence::Hover(Some(_)))));
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|r| r["method"] == "textDocument/hover")
            .count(),
        2
    );
    assert_eq!(fixture.handle.status(), ServerStatus::Ready);
    let always = Fixture::new("always-transient");
    always.update(TextBuffer::from("fn first() {}"), 0, None);
    always.ready();
    assert!(
        always
            .definition(0, 4)
            .unwrap_err()
            .contains("content modified")
    );
    assert_eq!(
        always
            .records()
            .iter()
            .filter(|r| r["method"] == "textDocument/definition")
            .count(),
        4
    );
    assert_eq!(always.handle.status(), ServerStatus::Ready);
}

#[test]
fn empty_definition_and_hover_while_project_loads_wait_for_analysis() {
    let fixture = Fixture::new("loading-empty");
    fixture.update(TextBuffer::from("fn first() {}"), 0, None);
    fixture.ready();
    assert_eq!(fixture.definition(0, 4).unwrap().len(), 1);
    let origin = RequestOrigin {
        document: fixture.key(),
        revision: 0,
        panel: 2,
        ticket: 2,
    };
    fixture
        .handle
        .query(origin, LanguageQuery::Hover { byte: 4 })
        .unwrap();
    let mut answer = None;
    fixture.wait(|| {
        while let Ok(event) = fixture.handle.try_recv() {
            if let Event::Intelligence { result, .. } = event {
                answer = Some(result);
            }
        }
        answer.is_some()
    });
    assert!(matches!(answer.unwrap(), Ok(Intelligence::Hover(Some(_)))));
    fixture.wait(|| {
        fixture
            .handle
            .activity()
            .project_status
            .as_ref()
            .is_some_and(|p| p.quiescent)
    });
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|r| r["method"] == "textDocument/hover")
            .count(),
        2
    );
    assert!(fixture.handle.activity().analysis_epoch >= 2);
    assert_eq!(
        fixture
            .records()
            .iter()
            .filter(|r| r["method"] == "textDocument/definition")
            .count(),
        2
    );
}
