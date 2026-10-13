use bed_document_session::{ByteEdit, ClosePolicy, EditorSession, SessionOptions};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-project-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"),"[package]\nname=\"bed-check-fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        Self(root.canonicalize().unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn checked(session: &mut EditorSession) -> bed_document_session::ProjectDiagnosticsSnapshot {
    let started = Instant::now();
    loop {
        session.tick();
        let snapshot = session.project_diagnostics();
        if !snapshot.checking && snapshot.status.starts_with("Project check") {
            assert!(!snapshot.status.contains("failed"), "{}", snapshot.status);
            return snapshot;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "Check did not finish: {}",
            snapshot.status
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cargo_checks_unopened_files_and_success_atomically_clears_results() {
    let fixture = Fixture::new();
    let source = fixture.0.join("src/lib.rs");
    fs::write(&source, "pub fn broken() { missing_name(); }\n").unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(session.document_ids().is_empty());
    let snapshot = checked(&mut session);
    assert!(snapshot.complete && !snapshot.stale);
    assert!(snapshot.errors > 0);
    assert!(
        snapshot.by_path[&source.to_string_lossy().into_owned()]
            .iter()
            .any(|item| item.message.contains("missing_name"))
    );
    let document = session.open_file(&source).unwrap();
    session
        .close_document(document, ClosePolicy::Discard)
        .unwrap();
    assert!(session.project_diagnostics().errors > 0);
    fs::write(&source, "pub fn fixed() {}\n").unwrap();
    session.request_project_check();
    let snapshot = checked(&mut session);
    assert_eq!(snapshot.errors, 0);
    assert!(snapshot.by_path.is_empty());
}

#[cfg(unix)]
#[test]
fn compiler_diagnostics_reach_the_editor_before_the_workspace_check_finishes() {
    use bed_document_session::{CheckFormat, ProjectCheckConfig};
    let fixture = Fixture::new();
    let source = fixture.0.join("src/lib.rs");
    fs::write(&source, "pub fn broken() {}\n").unwrap();
    let report = format!(
        "{}\n",
        json!({"reason":"compiler-message","message":{"level":"error","message":"unknown café","spans":[{"file_name":"src/lib.rs","line_start":1,"line_end":1,"column_start":8,"column_end":14,"is_primary":true}]}})
    );
    // Split inside a UTF-8 character and hold the process at each stage so
    // publication is tested independently of check completion.
    let split = report.find('é').unwrap() + 1;
    fs::write(fixture.0.join("first"), &report.as_bytes()[..split]).unwrap();
    fs::write(fixture.0.join("rest"), &report.as_bytes()[split..]).unwrap();
    fs::write(
        fixture.0.join("finished"),
        b"{\"reason\":\"build-finished\",\"success\":false}\n",
    )
    .unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&source).unwrap();
    let view = session.create_view(document).unwrap();
    session.configure_project_check(ProjectCheckConfig {
        program: "/bin/sh".into(),
        arguments: vec![
            "-c".into(),
            "cat first; touch partial; while [ ! -f continue ]; do sleep 0.01; done; cat rest; while [ ! -f finish ]; do sleep 0.01; done; cat finished; exit 1".into(),
        ],
        directory: ".".into(),
        format: CheckFormat::CargoJson,
        automatic: false,
    });
    session.tick();
    session.request_project_check();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.0.join("partial").exists() {
        session.tick();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    session.tick();
    assert_eq!(session.project_diagnostics().errors, 0);
    assert!(session.project_diagnostics().checking);
    fs::write(fixture.0.join("continue"), b"").unwrap();
    while session.project_diagnostics().errors == 0 {
        session.tick();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let snapshot = session.project_diagnostics();
    assert!(snapshot.checking, "The checker is still waiting for finish");
    assert_eq!(snapshot.errors, 1);
    let inline = session
        .with_view(view, |editor| {
            editor
                .diagnostics
                .as_ref()
                .unwrap()
                .for_document(&editor.state.path)
        })
        .unwrap();
    assert_eq!(inline[0].message, "unknown café");
    fs::write(fixture.0.join("finish"), b"").unwrap();
    let completed = checked(&mut session);
    assert_eq!(completed.by_path, snapshot.by_path);
    assert!(!completed.stale);

    // A successful following check removes the previously streamed error.
    let mut config = session.project_check_config().clone();
    config.arguments = vec![
        "-c".into(),
        "printf '%s\\n' '{\"reason\":\"build-finished\",\"success\":true}'".into(),
    ];
    session.configure_project_check(config);
    session.request_project_check();
    assert_eq!(checked(&mut session).errors, 0);
}

#[test]
fn rust_check_does_not_claim_complete_coverage_for_other_project_languages() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("src/lib.rs"), "pub fn good() {}\n").unwrap();
    fs::write(fixture.0.join("other.py"), "this is not valid python !\n").unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        ..Default::default()
    })
    .unwrap();
    let snapshot = checked(&mut session);
    assert_eq!(snapshot.errors, 0);
    assert!(!snapshot.complete, "Cargo alone does not analyze Python");
}

#[test]
fn newly_detected_source_languages_make_cargo_coverage_incomplete() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("src/lib.rs"), "pub fn good() {}\n").unwrap();
    // Project configuration and prose do not invalidate a Rust check's coverage.
    fs::write(fixture.0.join("README.md"), "A Rust project.\n").unwrap();
    fs::write(fixture.0.join("config.json"), "{}\n").unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(checked(&mut session).complete);
    let lua = fixture.0.join("other.lua");
    fs::write(&lua, "this is not valid lua !\n").unwrap();
    session.observe_project_file_changes(&[bed_remote::FilesystemChange::Created {
        path: lua.to_string_lossy().into_owned(),
    }]);
    session.request_project_check();
    let snapshot = checked(&mut session);
    assert_eq!(snapshot.errors, 0);
    assert!(!snapshot.complete, "Cargo alone does not analyze Lua");
    let mut reopened = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        !checked(&mut reopened).complete,
        "Project discovery must detect Lua too"
    );
}

fn saved_semantic_errors_reach_editor(lsp: bool) {
    let fixture = Fixture::new();
    let source = fixture.0.join("src/lib.rs");
    fs::write(&source, "pub fn broken() { String::wtf(); qdfwh; }\n").unwrap();
    let lsp_config = lsp.then(|| {
        let path = fixture.0.join("lsp.json");
        fs::write(&path, serde_json::to_vec(&json!({
            "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust-analyzer"}],
            "language_servers":{"rust-analyzer":{"command":"rust-analyzer"}}
        })).unwrap()).unwrap();
        path
    });
    let mut session = EditorSession::with_options(SessionOptions {
        project_root: Some(fixture.0.clone()),
        lsp_config,
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&source).unwrap();
    let view = session.create_view(document).unwrap();
    let initial = checked(&mut session);
    assert_eq!(
        initial.errors, 2,
        "initial diagnostics: {:?}",
        initial.by_path
    );
    if lsp {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !session
            .lsp()
            .unwrap()
            .client_for_document(document)
            .unwrap()
            .borrow()
            .is_initialized()
        {
            session.tick();
            assert!(
                Instant::now() < deadline,
                "rust-analyzer did not initialize"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            session.tick();
            thread::sleep(Duration::from_millis(10));
        }
        let snapshot = session.project_diagnostics();
        let raw = session
            .lsp()
            .unwrap()
            .client_for_document(document)
            .unwrap()
            .borrow()
            .diagnostics()
            .raw_for_document(source.to_str().unwrap());
        assert_eq!(
            snapshot.errors, 2,
            "duplicate checker reports: {:?}; raw: {raw:?}",
            snapshot.by_path
        );
    }
    assert_eq!(
        session
            .with_view(view, |editor| {
                editor
                    .diagnostics
                    .as_ref()
                    .unwrap()
                    .for_document(&editor.state.path)
                    .iter()
                    .filter(|item| item.severity == 1)
                    .count()
            })
            .unwrap(),
        2
    );
    for (text, token) in [
        ("pub fn broken() { String::wtf(); }\n", Some("wtf")),
        ("pub fn broken() { qdfwh; }\n", Some("qdfwh")),
        ("pub fn valid() {}\n", None),
    ] {
        let revision = session.document_revision(document).unwrap();
        let size = session.snapshot(document).unwrap().bytes.len();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..size,
                    bytes: text.as_bytes().to_vec(),
                }],
            )
            .unwrap();
        session.save(document).unwrap();
        let snapshot = checked(&mut session);
        assert_eq!(snapshot.errors, usize::from(token.is_some()));
        let inline = session
            .with_view(view, |editor| {
                editor
                    .diagnostics
                    .as_ref()
                    .map(|diagnostics| diagnostics.for_document(&editor.state.path))
                    .unwrap_or_default()
            })
            .unwrap();
        assert_eq!(
            inline.iter().any(|item| item.severity == 1
                && token.map_or(true, |token| item.message.contains(token))),
            token.is_some(),
            "saved semantic errors must reach the same diagnostics handle used by editor squiggles: {inline:?}"
        );
        assert_eq!(
            inline.iter().filter(|item| item.severity == 1).count(),
            usize::from(token.is_some()),
            "duplicate inline diagnostics: {inline:?}"
        );
    }
    session.shutdown(ClosePolicy::Discard).unwrap();
}

#[test]
fn saved_cargo_semantic_errors_reach_editor_and_corrections_clear_them() {
    saved_semantic_errors_reach_editor(false);
}

#[test]
#[ignore = "requires installed rust-analyzer, Cargo and the Rust toolchain"]
fn saved_rust_analyzer_semantic_errors_reach_editor_and_corrections_clear_them() {
    saved_semantic_errors_reach_editor(true);
}

#[cfg(unix)]
#[test]
#[ignore = "requires installed rust-analyzer, Cargo and the Rust toolchain"]
fn gui_path_reaches_saved_semantic_diagnostics_with_rust_analyzer() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "saved_rust_analyzer_semantic_errors_reach_editor_and_corrections_clear_them",
        ])
        .env("PATH", "/usr/bin:/bin")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "GUI-style PATH failed to produce diagnostics"
    );
}
