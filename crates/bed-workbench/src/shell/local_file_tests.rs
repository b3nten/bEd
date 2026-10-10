//! File-backed tabs must retain their database and SQLite sidecar paths.
use super::*;
use crate::test_support::TempDir;
use std::thread;

fn workspace(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    settings.terminal_visible = false;
    let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open_database(workbench: &mut Workbench, path: &Path) -> io::Result<bool> {
    workbench.open_file_with_viewer(path, Some(bed_plugin_sqlite::VIEWER_ID), false)
}

fn finish(workbench: &mut Workbench, choices: &[ConflictChoice]) -> OperationSummary {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut choices = choices.iter();
    while workbench.file_operations.active.is_some() {
        workbench.poll_file_operations().unwrap();
        if let Some(conflict) = workbench.file_operations.conflict.take() {
            conflict
                .reply
                .send(ConflictDecision {
                    choice: *choices.next().expect("unexpected conflict"),
                    apply_to_all: false,
                })
                .unwrap();
        }
        assert!(Instant::now() < deadline, "file operation stalled");
        thread::sleep(Duration::from_millis(2));
    }
    assert!(choices.next().is_none(), "unused conflict choice");
    let summary = workbench.file_operations.summary.clone().unwrap();
    assert!(!summary.canceled);
    summary
}

#[test]
fn file_viewer_blocks_path_changes_and_allows_copying_out() {
    let dir = TempDir::new();
    // Validation failure still leaves an open file association to protect.
    let database = dir.write("project/data/live.sqlite", b"database fixture");
    let wal = dir.write("project/data/live.sqlite-wal", b"wal fixture");
    let shm = dir.write("project/data/live.sqlite-shm", b"shm fixture");
    let other = dir.write("project/other.sqlite", b"another fixture");
    fs::create_dir_all(dir.path("project/archive")).unwrap();
    let mut workbench = workspace(&dir);
    open_database(&mut workbench, &database).unwrap();
    assert!(workbench.session.document_for_path(&database).is_none());

    for source in [&database, &wal, &shm, &dir.path("project/data")] {
        assert_eq!(
            workbench.rename_path(source, "renamed").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let mut trashed = false;
        let error = workbench
            .trash_path_with(source, |_| {
                trashed = true;
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(!trashed, "trash must not run before viewer preflight");
        for action in [
            FileTreeAction::Move {
                paths: vec![path(source)],
                destination: path(&dir.path("project/archive")),
            },
            FileTreeAction::TrashMany(vec![path(source)]),
        ] {
            assert_eq!(
                workbench.handle_tree_action(action).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert!(source.exists());
    }

    workbench
        .import_files(vec![database.clone()], path(&dir.path("project/archive")))
        .unwrap();
    assert_eq!(
        open_database(&mut workbench, &other).unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "local opens wait for jobs whose destructive-source preflight already ran"
    );
    assert!(finish(&mut workbench, &[]).errors.is_empty());
    assert_eq!(
        fs::read(dir.path("project/archive/live.sqlite")).unwrap(),
        b"database fixture"
    );
    open_database(&mut workbench, &other).unwrap();
    for (action, name) in [
        (
            FileTreeAction::NewFile(path(&dir.path("project"))),
            "other.sqlite-wal",
        ),
        (
            FileTreeAction::NewFolder(path(&dir.path("project"))),
            "other.sqlite-shm",
        ),
    ] {
        assert_eq!(
            workbench
                .apply_file_action(&action, name)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!dir.path(&format!("project/{name}")).exists());
    }
    assert_eq!(fs::read(&database).unwrap(), b"database fixture");
    workbench.cleanup().unwrap();
}

#[test]
fn imports_protect_replacements_and_merged_children_but_allow_keep_both() {
    let dir = TempDir::new();
    let database = dir.write("project/data/live.sqlite", b"original database");
    let source = dir.write("external/data/live.sqlite", b"replacement database");
    let mut workbench = workspace(&dir);
    open_database(&mut workbench, &database).unwrap();

    workbench
        .import_files(vec![source.clone()], path(&dir.path("project/data")))
        .unwrap();
    let summary = finish(&mut workbench, &[ConflictChoice::Replace]);
    assert_eq!(summary.errors.len(), 1);
    assert!(summary.errors[0].contains("Close the file viewer"));
    assert_eq!(fs::read(&database).unwrap(), b"original database");

    workbench
        .import_files(vec![source.clone()], path(&dir.path("project/data")))
        .unwrap();
    assert!(
        finish(&mut workbench, &[ConflictChoice::KeepBoth])
            .errors
            .is_empty()
    );
    assert_eq!(
        fs::read(dir.path("project/data/live copy.sqlite")).unwrap(),
        b"replacement database"
    );

    workbench
        .import_files(vec![dir.path("external/data")], path(&dir.path("project")))
        .unwrap();
    let summary = finish(
        &mut workbench,
        &[ConflictChoice::Merge, ConflictChoice::Replace],
    );
    assert_eq!(summary.errors.len(), 1);
    assert_eq!(fs::read(&database).unwrap(), b"original database");
    assert!(source.exists());
    assert!(
        fs::read_dir(dir.path("project/data"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".bed-transfer-"))
    );
    workbench.cleanup().unwrap();
}

#[test]
fn missing_sidecars_are_protected_at_resolved_import_and_move_destinations() {
    let dir = TempDir::new();
    let database = dir.write("project/data/live.sqlite", b"original database");
    let copy_database = dir.write("project/data/live copy.sqlite", b"second database");
    dir.write("project/data/live.sqlite-wal", b"original wal");
    let imported = dir.write("external/live.sqlite-wal", b"imported wal");
    let moved = dir.write("project/staging/live copy.sqlite-shm", b"moved shm");
    let mut workbench = workspace(&dir);
    open_database(&mut workbench, &database).unwrap();
    open_database(&mut workbench, &copy_database).unwrap();

    workbench
        .import_files(vec![imported], path(&dir.path("project/data")))
        .unwrap();
    let summary = finish(&mut workbench, &[ConflictChoice::KeepBoth]);
    assert_eq!(summary.errors.len(), 1);
    assert!(!dir.path("project/data/live copy.sqlite-wal").exists());
    assert_eq!(
        fs::read(dir.path("project/data/live.sqlite-wal")).unwrap(),
        b"original wal"
    );

    workbench
        .handle_tree_action(FileTreeAction::Move {
            paths: vec![path(&moved)],
            destination: path(&dir.path("project/data")),
        })
        .unwrap();
    assert_eq!(finish(&mut workbench, &[]).errors.len(), 1);
    assert_eq!(fs::read(moved).unwrap(), b"moved shm");
    assert!(!dir.path("project/data/live copy.sqlite-shm").exists());
    workbench.cleanup().unwrap();
}

#[cfg(unix)]
#[test]
fn changing_a_symlink_entry_preserves_its_open_database_target() {
    let dir = TempDir::new();
    let database = dir.write("project/data/live.sqlite", b"original database");
    let link = dir.path("project/link.sqlite");
    std::os::unix::fs::symlink(&database, &link).unwrap();
    let mut workbench = workspace(&dir);
    open_database(&mut workbench, &link).unwrap();
    let renamed = workbench.rename_path(&link, "renamed.sqlite").unwrap();
    workbench
        .trash_path_with(&renamed, |path| fs::remove_file(path))
        .unwrap();
    assert_eq!(fs::read(database).unwrap(), b"original database");
    workbench.cleanup().unwrap();
}

#[cfg(unix)]
#[test]
fn hardlink_aliases_cannot_be_edited_alongside_the_database_viewer() {
    let dir = TempDir::new();
    let database = dir.write("project/live.sqlite", b"original database");
    let alias = dir.path("project/alias.txt");
    fs::hard_link(&database, &alias).unwrap();
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&alias).unwrap();
    assert_eq!(
        open_database(&mut workbench, &database).unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "an existing editable hardlink prevents a database viewer"
    );
    workbench.dispatch(WindowCommand::Close).unwrap();
    open_database(&mut workbench, &database).unwrap();
    assert_eq!(
        workbench.open_or_focus(&alias).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied,
        "in-place editor saves through hardlinks would truncate the database"
    );
    assert_eq!(
        workbench
            .session
            .ensure_local_path_editable(&alias)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied,
        "Save As destinations have the same write protection"
    );
    let renamed = workbench.rename_path(&alias, "renamed.txt").unwrap();
    workbench
        .trash_path_with(&renamed, |path| fs::remove_file(path))
        .unwrap();
    assert_eq!(fs::read(database).unwrap(), b"original database");
    workbench.cleanup().unwrap();
}
