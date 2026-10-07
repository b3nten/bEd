//! Watches an active document's disk baseline using std filesystem metadata.
//!
//! Returned bytes preserve BOMs and invalid UTF-8. Reads are bounded by the
//! editor's editable-file limit. Metadata is the
//! fast path; in-place changes preserving size and timestamps are not detected.
use std::fs;
use std::io;
use std::{
    collections::hash_map::DefaultHasher,
    fs::{File, Metadata},
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub const MAX_FILE_SIZE: u64 = crate::files::MAX_FILE_SIZE as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChangeKind {
    Modified,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub raw: Option<Vec<u8>>,
    pub kind: FileChangeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}
impl Fingerprint {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        }
    }
}

#[derive(Default, Debug)]
enum DiskState {
    #[default]
    Missing,
    Present {
        fingerprint: Fingerprint,
        content_hash: u64,
    },
}

#[derive(Default, Debug)]
pub struct FileMonitor {
    path: Option<PathBuf>,
    baseline: DiskState,
}

impl FileMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Establishes a baseline without reporting an initial change.
    pub fn watch(&mut self, path: &Path) -> io::Result<()> {
        let (baseline, _) = read_current(path)?;
        self.path = Some(path.to_owned());
        self.baseline = baseline;
        Ok(())
    }

    /// Establishes a baseline only when a stable disk read matches loaded bytes.
    /// A changed or removed file leaves the previous watch intact so callers can
    /// preserve the current document and retry instead of installing stale data.
    pub fn watch_bytes(&mut self, path: &Path, expected_raw: &[u8]) -> io::Result<()> {
        let (baseline, raw) = read_current(path)?;
        if raw.as_deref() != Some(expected_raw) {
            return Err(changed_during_read());
        }
        self.path = Some(path.to_owned());
        self.baseline = baseline;
        Ok(())
    }

    /// Accepts the disk baseline after a successful save, reload, or user action.
    pub fn accept_current(&mut self, path: &Path) -> io::Result<()> {
        self.watch(path)
    }

    pub fn reset(&mut self) {
        self.path = None;
        self.baseline = DiskState::Missing;
    }

    /// Checks one active file. Changing the active path establishes a baseline.
    ///
    /// Changes are notifications only; callers decide whether a reload is safe.
    /// A removal is reported once, and later recreation is a modification even
    /// when the recreated file has the same bytes as its former contents.
    pub fn poll(&mut self, path: &Path) -> io::Result<Option<FileChange>> {
        if self.path.as_deref() != Some(path) {
            self.watch(path)?;
            return Ok(None);
        }
        let metadata = match fs::metadata(path) {
            Ok(metadata) => Some(Fingerprint::from_metadata(&metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        match (&self.baseline, &metadata) {
            (DiskState::Missing, None) => return Ok(None),
            (DiskState::Present { fingerprint, .. }, Some(current)) if fingerprint == current => {
                return Ok(None);
            }
            _ => {}
        }
        let (current, raw) = read_current(path)?;
        let kind = match (&self.baseline, &current) {
            (DiskState::Present { .. }, DiskState::Missing) => Some(FileChangeKind::Removed),
            (DiskState::Missing, DiskState::Present { .. }) => Some(FileChangeKind::Modified),
            (
                DiskState::Present {
                    content_hash: before,
                    ..
                },
                DiskState::Present {
                    content_hash: after,
                    ..
                },
            ) if before != after => Some(FileChangeKind::Modified),
            _ => None,
        };
        self.baseline = current;
        Ok(kind.map(|kind| FileChange {
            path: path.to_string_lossy().into_owned(),
            raw,
            kind,
        }))
    }
}

fn read_current(path: &Path) -> io::Result<(DiskState, Option<Vec<u8>>)> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((DiskState::Missing, None));
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(crate::files::file_too_large(path, metadata.len()));
    }
    let fingerprint = Fingerprint::from_metadata(&metadata);
    let mut raw = Vec::with_capacity(metadata.len() as usize);
    (&file).take(MAX_FILE_SIZE + 1).read_to_end(&mut raw)?;
    let after = Fingerprint::from_metadata(&file.metadata()?);
    let at_path = match fs::metadata(path) {
        Ok(metadata) => Fingerprint::from_metadata(&metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(changed_during_read());
        }
        Err(error) => return Err(error),
    };
    if fingerprint != after || after != at_path {
        return Err(changed_during_read());
    }
    if raw.len() as u64 > MAX_FILE_SIZE {
        return Err(crate::files::file_too_large(
            path,
            after.len.max(raw.len() as u64),
        ));
    }
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    Ok((
        DiskState::Present {
            fingerprint,
            content_hash: hasher.finish(),
        },
        Some(raw),
    ))
}
fn changed_during_read() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "File changed while reading; retry the operation",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-monitor-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> PathBuf {
            self.0.join("file")
        }
        fn write(&self, bytes: &[u8], tick: u64) {
            fs::write(self.path(), bytes).unwrap();
            File::open(self.path())
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(tick))
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn detects_same_size_content_change_with_new_timestamp() {
        let fixture = Fixture::new();
        fixture.write(b"before", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
        fixture.write(b"after!", 2);
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.kind, FileChangeKind::Modified);
        assert_eq!(change.raw, Some(b"after!".to_vec()));
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
    }
    #[test]
    fn watch_bytes_rejects_changed_contents_without_accepting_the_baseline() {
        let fixture = Fixture::new();
        fixture.write(b"loaded", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch_bytes(&fixture.path(), b"loaded").unwrap();
        fixture.write(b"changed", 2);
        assert_eq!(
            monitor
                .watch_bytes(&fixture.path(), b"loaded")
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.kind, FileChangeKind::Modified);
        assert_eq!(change.raw, Some(b"changed".to_vec()));
        monitor.watch_bytes(&fixture.path(), b"changed").unwrap();
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
        fs::remove_file(fixture.path()).unwrap();
        assert_eq!(
            monitor
                .watch_bytes(&fixture.path(), b"")
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            monitor.poll(&fixture.path()).unwrap().unwrap().kind,
            FileChangeKind::Removed
        );
    }
    #[test]
    fn detects_size_change_and_preserves_bom_and_invalid_bytes() {
        let fixture = Fixture::new();
        fixture.write(b"x", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        fixture.write(b"\xef\xbb\xbf\xff\r\ny", 2);
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.raw, Some(b"\xef\xbb\xbf\xff\r\ny".to_vec()));
    }
    #[test]
    fn detects_replaced_file() {
        let fixture = Fixture::new();
        fixture.write(b"before", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        let replacement = fixture.0.join("replacement");
        fs::write(&replacement, b"replacement").unwrap();
        fs::rename(replacement, fixture.path()).unwrap();
        assert_eq!(
            monitor.poll(&fixture.path()).unwrap().unwrap().raw,
            Some(b"replacement".to_vec())
        );
    }
    #[test]
    fn accepts_successful_self_save_and_ignores_metadata_only_changes() {
        let fixture = Fixture::new();
        fixture.write(b"before", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        fixture.write(b"saved", 2);
        monitor.accept_current(&fixture.path()).unwrap();
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
        fixture.write(b"saved", 3);
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
    }
    #[test]
    fn reports_removal_once_and_identical_recreation() {
        let fixture = Fixture::new();
        fixture.write(b"same", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        fs::remove_file(fixture.path()).unwrap();
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.kind, FileChangeKind::Removed);
        assert_eq!(change.raw, None);
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
        fixture.write(b"same", 2);
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.kind, FileChangeKind::Modified);
        assert_eq!(change.raw, Some(b"same".to_vec()));
    }
    #[test]
    fn switching_path_and_reset_establish_new_baselines() {
        let fixture = Fixture::new();
        fixture.write(b"first", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        let second = fixture.0.join("second");
        fs::write(&second, b"second").unwrap();
        assert!(monitor.poll(&second).unwrap().is_none());
        monitor.reset();
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
    }
    #[test]
    fn files_beyond_one_mib_keep_a_monitoring_baseline_and_report_external_changes() {
        let fixture = Fixture::new();
        let mut bytes = vec![b'a'; 2 * 1024 * 1024];
        fixture.write(&bytes, 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] = b'z';
        fixture.write(&bytes, 2);
        let change = monitor.poll(&fixture.path()).unwrap().unwrap();
        assert_eq!(change.kind, FileChangeKind::Modified);
        assert_eq!(change.raw.unwrap(), bytes);
        assert!(monitor.poll(&fixture.path()).unwrap().is_none());
    }

    #[test]
    fn oversized_change_returns_error_without_accepting_baseline() {
        let fixture = Fixture::new();
        fixture.write(b"small", 1);
        let mut monitor = FileMonitor::new();
        monitor.watch(&fixture.path()).unwrap();
        File::options()
            .write(true)
            .open(fixture.path())
            .unwrap()
            .set_len(MAX_FILE_SIZE + 1)
            .unwrap();
        assert_eq!(
            monitor.poll(&fixture.path()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fixture.write(b"recovered", 3);
        assert_eq!(
            monitor.poll(&fixture.path()).unwrap().unwrap().raw,
            Some(b"recovered".to_vec())
        );
    }
}
