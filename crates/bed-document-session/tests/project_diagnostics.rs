use bed_document_session::{ClosePolicy, EditorSession, SessionOptions};
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
