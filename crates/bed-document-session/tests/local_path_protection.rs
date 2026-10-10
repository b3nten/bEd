use bed_document_session::{EditorSession, SessionOptions};
use std::{fs, io, path::PathBuf, time::Duration};

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("bed-reserved-path-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reserved_files_cannot_be_opened_or_replaced_through_document_services() {
    let fixture = Fixture::new("services");
    let existing = fixture.path("existing.txt");
    let missing = fixture.path("reserved.txt");
    fs::write(&existing, b"original").unwrap();
    let mut session = EditorSession::new();
    let document = session.open_file(&existing).unwrap();
    session.replace_content(document, b"changed").unwrap();
    let replacement = session.create_document(b"replacement").unwrap();
    session.take_events();
    session.set_read_only_local_paths(vec![existing.clone(), missing.clone()]);
    for result in [
        session.open_file(&existing),
        session.open_file_auto(&existing),
        session.request_open_file(&existing).map(|id| id.unwrap()),
        session
            .request_open_file_auto(&existing)
            .map(|id| id.unwrap()),
    ] {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }
    assert_eq!(
        session.save(document).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        session.save_as(replacement, &existing).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        session.save_as(replacement, &missing).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(session.take_events().is_empty());
    assert_eq!(fs::read(&existing).unwrap(), b"original");
    assert!(!missing.exists());
    assert!(session.snapshot(document).unwrap().dirty);
    assert!(session.snapshot(replacement).unwrap().path.is_empty());
    assert!(
        session
            .create_snapshot_document(b"comparison", "text")
            .is_ok()
    );
    session.set_read_only_local_paths(Vec::new());
    assert_eq!(session.open_file(&existing).unwrap(), document);
    assert!(session.save(document).unwrap());
    assert!(session.save_as(replacement, &missing).unwrap());
    assert_eq!(fs::read(&existing).unwrap(), b"changed");
    assert_eq!(fs::read(&missing).unwrap(), b"replacement");
}

#[cfg(unix)]
#[test]
fn existing_aliases_and_missing_paths_under_symlinked_parents_stay_reserved() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("aliases");
    let path = fixture.path("original.txt");
    let hardlink = fixture.path("hardlink.txt");
    let symlink_path = fixture.path("symlink.txt");
    let directory_alias = fixture.path("directory-alias");
    let missing = fixture.path("missing.txt");
    fs::write(&path, b"original").unwrap();
    fs::hard_link(&path, &hardlink).unwrap();
    symlink(&path, &symlink_path).unwrap();
    symlink(&fixture.0, &directory_alias).unwrap();
    let mut session = EditorSession::new();
    session.set_read_only_local_paths(vec![path, missing]);
    for path in [hardlink, symlink_path, directory_alias.join("missing.txt")] {
        assert_eq!(
            session
                .ensure_local_path_editable(&path)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    assert!(session.open_file(&fixture.path("hardlink.txt")).is_err());
    session.set_read_only_local_paths(Vec::new());
    assert!(session.open_file(&fixture.path("hardlink.txt")).is_ok());
}

#[test]
fn autosave_cannot_write_a_path_that_becomes_reserved() {
    let fixture = Fixture::new("autosave");
    let path = fixture.path("document.txt");
    fs::write(&path, b"original").unwrap();
    let mut session = EditorSession::with_options(SessionOptions {
        autosave: Some(Duration::from_millis(1)),
        ..SessionOptions::default()
    })
    .unwrap();
    let document = session.open_file(&path).unwrap();
    session.set_read_only_local_paths(vec![path.clone()]);
    session.replace_content(document, b"changed").unwrap();
    std::thread::sleep(Duration::from_millis(5));
    session.tick();
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(session.snapshot(document).unwrap().dirty);
    session.set_read_only_local_paths(Vec::new());
    assert!(session.save(document).unwrap());
    assert_eq!(fs::read(path).unwrap(), b"changed");
}
