use super::*;
use std::{
    fs,
    io::{Cursor, SeekFrom, Write},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bedtime-filesystem-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn source(&self, name: &str) -> FileSource {
        FileSource::local(self.0.join(name))
    }
    fn entries(&self) -> usize {
        fs::read_dir(&self.0).unwrap().count()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn canonical_document_sources_share_identity_across_provider_clones() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file.txt");
    fs::write(&path, "text").unwrap();
    let filesystem = Filesystem::new(LocalFilesystem);
    let source = filesystem.document_source(&path).unwrap();
    assert_eq!(
        source.id(),
        filesystem.clone().document_source(&path).unwrap().id()
    );
    assert_ne!(
        source.id(),
        Filesystem::new(LocalFilesystem)
            .document_source(&path)
            .unwrap()
            .id()
    );
    assert_eq!(
        Filesystem::local().document_source(&path).unwrap().id(),
        Filesystem::local().document_source(&path).unwrap().id()
    );
}

#[test]
fn document_sources_save_against_their_version_and_follow_confirmed_moves() {
    use bedtime_document::{DocumentSource, SourceError, TextBuffer};
    let fixture = Fixture::new();
    let path = fixture.0.join("before.txt");
    fs::write(&path, "baseline").unwrap();
    let filesystem = Filesystem::new(LocalFilesystem);
    let source = filesystem.document_source(&path).unwrap();
    let loaded = <FsDocumentSource as DocumentSource<TextBuffer>>::load(&source).unwrap();
    assert_eq!(loaded.model.to_string(), "baseline");
    let destination = fixture.0.join("after.txt");
    fs::rename(&path, &destination).unwrap();
    filesystem.move_document_sources(&path, &destination);
    assert_eq!(source.path(), destination);
    assert_eq!(
        source.id(),
        filesystem.document_source(&destination).unwrap().id()
    );
    let version = source
        .save(TextBuffer::from("saved"), loaded.version)
        .unwrap();
    assert_eq!(fs::read_to_string(&destination).unwrap(), "saved");
    // Changed by something else since: refused, and left alone.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&destination, "external").unwrap();
    let result = source.save(TextBuffer::from("overwrite"), version);
    assert!(matches!(result, Err(SourceError::Changed)));
    assert_eq!(fs::read_to_string(destination).unwrap(), "external");
}

#[test]
fn text_files_must_be_text() {
    use bedtime_document::{DocumentSource, SourceError, TextBuffer};
    let fixture = Fixture::new();
    let path = fixture.0.join("binary");
    fs::write(&path, b"\0\xff").unwrap();
    let source = Filesystem::local().document_source(&path).unwrap();
    assert!(matches!(
        <FsDocumentSource as DocumentSource<TextBuffer>>::load(&source),
        Err(SourceError::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn document_source_symlink_aliases_share_targets_and_retarget_independently() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let first = fixture.0.join("first");
    let second = fixture.0.join("second");
    let link = fixture.0.join("link");
    let hard = fixture.0.join("hard");
    fs::write(&first, "one").unwrap();
    fs::write(&second, "two").unwrap();
    symlink(&first, &link).unwrap();
    fs::hard_link(&first, &hard).unwrap();
    let filesystem = Filesystem::new(LocalFilesystem);
    let target = filesystem.document_source(&first).unwrap();
    let alias = filesystem.document_source(&link).unwrap();
    assert_eq!(target.id(), alias.id());
    assert_ne!(target.id(), filesystem.document_source(&hard).unwrap().id());
    fs::remove_file(&link).unwrap();
    symlink(&second, &link).unwrap();
    let retargeted = filesystem.document_source(&link).unwrap();
    assert_ne!(retargeted.id(), target.id());
    assert_eq!(target.path(), first);
    assert_eq!(retargeted.path(), second);
}

#[test]
fn source_identity_includes_filesystem_and_preserves_native_path() {
    let a = Filesystem::new(LocalFilesystem);
    let b = Filesystem::new(LocalFilesystem);
    assert_ne!(a.source("/project/file"), b.source("/project/file"));
    assert_eq!(a.source("/project/file"), a.clone().source("/project/file"));
    assert_eq!(
        FileSource::local("/project/file"),
        FileSource::local("/project/file")
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let native = PathBuf::from(std::ffi::OsString::from_vec(b"/project/\xff".to_vec()));
        assert_eq!(a.source(&native).path(), native);
        assert_ne!(
            a.source(&native),
            a.source(native.to_string_lossy().as_ref())
        );
    }
}

#[test]
fn reads_are_bounded_and_range_reads_reject_replaced_sources() {
    let fixture = Fixture::new();
    let source = fixture.source("binary");
    fs::write(source.path(), b"\0\xffABC").unwrap();
    let snapshot = source.read(5).unwrap();
    assert_eq!(snapshot.bytes, b"\0\xffABC");
    assert!(matches!(source.read(4), Err(Error::TooLarge { limit: 4 })));
    assert_eq!(source.read_prefix(2).unwrap().bytes, b"\0\xff");
    assert_eq!(
        source.read_range(&snapshot.metadata.version, 2..5).unwrap(),
        b"ABC"
    );
    assert!(source.read_range(&snapshot.metadata.version, 4..6).is_err());
    let replacement = fixture.source("replacement");
    fs::write(replacement.path(), b"other").unwrap();
    fs::rename(replacement.path(), source.path()).unwrap();
    assert!(matches!(
        source.read_range(&snapshot.metadata.version, 0..5),
        Err(Error::Changed)
    ));
}

#[test]
fn sparse_reads_above_four_gib_do_not_materialize_the_source() {
    let fixture = Fixture::new();
    let source = fixture.source("large");
    let offset = (1_u64 << 32) + 37;
    let mut file = fs::File::create(source.path()).unwrap();
    file.set_len(offset + 3).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[0, 255, 1]).unwrap();
    drop(file);
    let observed = source.metadata().unwrap();
    assert_eq!(
        source
            .read_range(&observed.version, offset..offset + 3)
            .unwrap(),
        [0, 255, 1]
    );
    assert!(matches!(source.read(1024), Err(Error::TooLarge { .. })));
}

#[test]
fn staging_does_not_change_destination_and_abandonment_removes_temporary() {
    let fixture = Fixture::new();
    let source = fixture.source("file");
    fs::write(source.path(), b"baseline").unwrap();
    let pending = source
        .prepare_write(&mut Cursor::new(b"replacement"))
        .unwrap();
    assert_eq!(fs::read(source.path()).unwrap(), b"baseline");
    drop(pending);
    assert_eq!(fixture.entries(), 1);
    assert_eq!(fs::read(source.path()).unwrap(), b"baseline");
}

#[test]
fn checked_writes_compare_exact_streamed_baselines_and_preserve_newer_content() {
    let fixture = Fixture::new();
    let source = fixture.source("file");
    fs::write(source.path(), b"original").unwrap();
    let pending = source.prepare_write(&mut Cursor::new(b"mine")).unwrap();
    fs::write(source.path(), b"external").unwrap();
    assert!(matches!(
        pending.commit(WriteExpectation::Contents(&mut Cursor::new(b"original"))),
        Err(Error::Changed)
    ));
    assert_eq!(fs::read(source.path()).unwrap(), b"external");
    assert_eq!(fixture.entries(), 1);
    let mut content = ChunkReader::new([b"new".as_slice(), b"", b" content"].into_iter());
    let pending = source.prepare_write(&mut content).unwrap();
    let mut baseline = ChunkReader::new([b"ex".as_slice(), b"ter", b"nal"].into_iter());
    pending
        .commit(WriteExpectation::Contents(&mut baseline))
        .unwrap();
    assert_eq!(fs::read(source.path()).unwrap(), b"new content");
    assert_eq!(fixture.entries(), 1);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn new_binary_files_are_published_without_overwriting_concurrent_creates() {
    let fixture = Fixture::new();
    let source = fixture.source("new");
    let pending = source.prepare_write(&mut Cursor::new([0, 255, 1])).unwrap();
    let receipt = pending.commit(WriteExpectation::Missing).unwrap();
    assert_eq!(
        source.read_range(&receipt.version, 0..3).unwrap(),
        [0, 255, 1]
    );
    assert_eq!(fs::read(source.path()).unwrap(), [0, 255, 1]);
    let other = fixture.source("raced");
    let pending = other.prepare_write(&mut Cursor::new(b"mine")).unwrap();
    fs::write(other.path(), b"theirs").unwrap();
    assert_eq!(
        pending
            .commit(WriteExpectation::Missing)
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(other.path()).unwrap(), b"theirs");
    assert_eq!(fixture.entries(), 2);
}

#[test]
fn failed_streaming_preparation_preserves_destination_and_cleans_up() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("source encoding failed"))
        }
    }
    let fixture = Fixture::new();
    let source = fixture.source("file");
    fs::write(source.path(), b"preserve").unwrap();
    assert!(source.prepare_write(&mut Broken).is_err());
    assert_eq!(fs::read(source.path()).unwrap(), b"preserve");
    assert_eq!(fixture.entries(), 1);
}

#[cfg(unix)]
#[test]
fn writes_preserve_permissions_and_reject_retargeted_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let a = fixture.source("a");
    let b = fixture.source("b");
    let link = fixture.source("link");
    fs::write(a.path(), b"same").unwrap();
    fs::write(b.path(), b"same").unwrap();
    fs::set_permissions(a.path(), fs::Permissions::from_mode(0o640)).unwrap();
    symlink(a.path(), link.path()).unwrap();
    let pending = link.prepare_write(&mut Cursor::new(b"mine")).unwrap();
    fs::remove_file(link.path()).unwrap();
    symlink(b.path(), link.path()).unwrap();
    assert!(matches!(
        pending.commit(WriteExpectation::Contents(&mut Cursor::new(b"same"))),
        Err(Error::Changed)
    ));
    assert_eq!(fs::read(a.path()).unwrap(), b"same");
    assert_eq!(fs::read(b.path()).unwrap(), b"same");
    a.prepare_write(&mut Cursor::new(b"mine"))
        .unwrap()
        .commit(WriteExpectation::Contents(&mut Cursor::new(b"same")))
        .unwrap();
    assert_eq!(
        fs::metadata(a.path()).unwrap().permissions().mode() & 0o777,
        0o640
    );
    fs::set_permissions(a.path(), fs::Permissions::from_mode(0o440)).unwrap();
    assert!(a.prepare_write(&mut Cursor::new(b"no")).is_err());
    assert_eq!(fs::read(a.path()).unwrap(), b"mine");
}

#[test]
fn directory_reads_retain_hidden_entries_and_honor_cancellation() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join(".hidden"), b"").unwrap();
    fs::create_dir(fixture.0.join("folder")).unwrap();
    let root = FileSource::local(&fixture.0);
    let entries = root.read_directory(&|| false).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == ".hidden" && entry.hidden)
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "folder" && entry.kind == EntryKind::Directory)
    );
    assert_eq!(
        root.read_directory(&|| true).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}
