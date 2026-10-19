//! Directory-only vnode invalidations, polled by the existing filesystem worker.
//!
//! Neither registration nor polling enumerates children. In particular, a write
//! to a directory must never install watches on newly discovered subtrees.
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
    ptr,
};

// The backend admits 512 cached directories and two root parents per 64 views.
const MAX_WATCHES: usize = 512 + 2 * 64;
const MAX_EVENTS: usize = 128;
const LOST_NOTES: u32 = libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;
const WATCH_NOTES: u32 =
    LOST_NOTES | libc::NOTE_WRITE | libc::NOTE_ATTRIB | libc::NOTE_LINK | libc::NOTE_EXTEND;

fn ensure_watch_capacity(count: usize) -> io::Result<()> {
    if count >= MAX_WATCHES {
        return Err(io::Error::other(format!(
            "Directory watch capacity reached ({MAX_WATCHES})"
        )));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct DirectoryEvent {
    pub path: PathBuf,
    /// The old descriptor has been released. Reconcile and watch this path again
    /// if it still exists (or after it is recreated).
    pub lost: bool,
}

pub(crate) struct DirectoryWatcher {
    queue: OwnedFd,
    // One actual directory descriptor per explicitly supplied native path.
    // All mutations live here; closing a file also removes its kqueue filter.
    watched: BTreeMap<PathBuf, File>,
}

impl DirectoryWatcher {
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: kqueue takes no pointers and returns a new owned descriptor.
        let descriptor = unsafe { libc::kqueue() };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful kqueue call transferred this unique descriptor.
        let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // SAFETY: queue owns a live descriptor; F_SETFD takes an integer flag.
        if unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            queue,
            watched: BTreeMap::new(),
        })
    }

    pub(crate) fn watch(&mut self, path: &Path) -> io::Result<()> {
        if self.watched.contains_key(path) {
            return Ok(());
        }
        ensure_watch_capacity(self.watched.len())?;
        // OpenOptions preserves native path bytes. O_NOFOLLOW rejects a final
        // symlink atomically; fstat validates the opened object, not a prior path
        // observation that could race replacement. No directory is enumerated.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_EVTONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        if !file.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Only directories can be watched",
            ));
        }
        let change = libc::kevent {
            ident: file.as_raw_fd() as libc::uintptr_t,
            filter: libc::EVFILT_VNODE,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: WATCH_NOTES,
            data: 0,
            udata: ptr::null_mut(),
        };
        // SAFETY: queue and file are live; change points to one initialized
        // record. With no output records kevent only applies this change and
        // cannot wait. A failure drops file, also removing any installed filter.
        let result = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                &change,
                1,
                ptr::null_mut(),
                0,
                ptr::null(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        self.watched.insert(path.to_owned(), file);
        Ok(())
    }

    pub(crate) fn unwatch(&mut self, path: &Path) -> io::Result<()> {
        // kqueue automatically deletes all filters for a closed descriptor,
        // including pending notifications. No separate fallible EV_DELETE is
        // needed, and releasing an already-lost/absent path is idempotent.
        self.watched.remove(path);
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> io::Result<Vec<DirectoryEvent>> {
        let mut events = [libc::kevent {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: ptr::null_mut(),
        }; MAX_EVENTS];
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: the live queue has no input changes; events is writable for
        // MAX_EVENTS initialized records and timeout is valid for this call.
        // Zero timeout makes this a single bounded, nonblocking worker poll.
        let count = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                ptr::null(),
                0,
                events.as_mut_ptr(),
                MAX_EVENTS as libc::c_int,
                &timeout,
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(Vec::new())
            } else {
                Err(error)
            };
        }
        let mut invalidations = Vec::with_capacity(count as usize);
        for event in &events[..count as usize] {
            if event.flags & libc::EV_ERROR != 0 {
                if event.data != 0 {
                    return Err(io::Error::from_raw_os_error(event.data as libc::c_int));
                }
                continue;
            }
            if event.filter != libc::EVFILT_VNODE {
                continue;
            }
            // The small hard cap bounds this in-memory lookup. Keeping one map
            // avoids a second path/descriptor index with its own lifetime state.
            let Some(path) = self.watched.iter().find_map(|(path, file)| {
                (file.as_raw_fd() as libc::uintptr_t == event.ident).then(|| path.clone())
            }) else {
                continue;
            };
            let lost = event.fflags & LOST_NOTES != 0;
            if lost {
                // A renamed/deleted vnode is not the directory currently at its
                // old path. Release it now so reconciliation can install a fresh
                // descriptor instead of retaining an obsolete object forever.
                self.watched.remove(&path);
            }
            invalidations.push(DirectoryEvent { path, lost });
        }
        Ok(invalidations)
    }

    #[cfg(test)]
    pub(crate) fn watch_count(&self) -> usize {
        self.watched.len()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::{
        collections::BTreeSet,
        ffi::OsString,
        fs,
        os::unix::{ffi::OsStringExt, fs::symlink},
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bedtime-directory-watch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            // macOS temp paths can start with the /var symlink.
            Self(fs::canonicalize(path).unwrap())
        }
        fn directory(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn next_events(watcher: &mut DirectoryWatcher) -> Vec<DirectoryEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = watcher.poll().unwrap();
            assert!(events.len() <= MAX_EVENTS);
            if !events.is_empty() {
                return events;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for a directory vnode event"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn expect_event(watcher: &mut DirectoryWatcher, path: &Path, lost: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = watcher.poll().unwrap();
            assert!(events.len() <= MAX_EVENTS);
            if events
                .iter()
                .any(|event| event.path == path && event.lost == lost)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Missing event for {path:?}, lost={lost}; last poll: {events:?}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn child_creation_and_rename_only_invalidate_the_supplied_directory() {
        let fixture = Fixture::new();
        let mut watcher = DirectoryWatcher::new().unwrap();
        watcher.watch(&fixture.0).unwrap();
        watcher.watch(&fixture.0).unwrap();
        assert_eq!(
            watcher.watch_count(),
            1,
            "Duplicate registration must not own another FD"
        );
        assert!(watcher.poll().unwrap().is_empty());

        let child = fixture.0.join("child");
        fs::write(&child, b"created").unwrap();
        let events = next_events(&mut watcher);
        assert!(
            events
                .iter()
                .all(|event| event.path == fixture.0 && !event.lost)
        );
        assert_eq!(watcher.watch_count(), 1);
        fs::rename(&child, fixture.0.join("renamed")).unwrap();
        expect_event(&mut watcher, &fixture.0, false);
        assert_eq!(watcher.watch_count(), 1);

        watcher.unwatch(&fixture.0).unwrap();
        watcher.unwatch(&fixture.0).unwrap();
        assert_eq!(watcher.watch_count(), 0);
        fs::write(fixture.0.join("after-unwatch"), b"unobserved").unwrap();
        assert!(watcher.poll().unwrap().is_empty());
    }

    #[test]
    fn root_rename_releases_the_old_vnode_and_allows_reinstallation() {
        let fixture = Fixture::new();
        let root = fixture.directory("root");
        let mut watcher = DirectoryWatcher::new().unwrap();
        watcher.watch(&root).unwrap();
        let moved = fixture.0.join("moved");
        fs::rename(&root, &moved).unwrap();
        expect_event(&mut watcher, &root, true);
        assert_eq!(watcher.watch_count(), 0);
        fs::write(moved.join("old-vnode"), b"not watched").unwrap();
        assert!(watcher.poll().unwrap().is_empty());

        fs::create_dir(&root).unwrap();
        watcher.watch(&root).unwrap();
        fs::write(root.join("new-vnode"), b"watched").unwrap();
        expect_event(&mut watcher, &root, false);
        assert_eq!(watcher.watch_count(), 1);
    }

    #[test]
    fn root_disappearance_releases_the_watch_and_recreated_root_can_be_watched() {
        let fixture = Fixture::new();
        let root = fixture.directory("root");
        let mut watcher = DirectoryWatcher::new().unwrap();
        watcher.watch(&root).unwrap();
        fs::remove_dir(&root).unwrap();
        expect_event(&mut watcher, &root, true);
        assert_eq!(watcher.watch_count(), 0);
        watcher.unwatch(&root).unwrap();
        assert!(watcher.watch(&root).is_err());
        assert_eq!(watcher.watch_count(), 0);

        fs::create_dir(&root).unwrap();
        watcher.watch(&root).unwrap();
        fs::write(root.join("child"), b"recreated").unwrap();
        expect_event(&mut watcher, &root, false);
        assert_eq!(watcher.watch_count(), 1);
    }

    #[test]
    fn neither_registration_nor_first_create_event_discovers_large_subtrees() {
        let fixture = Fixture::new();
        for index in 0..128 {
            fixture.directory(&format!("existing/branch-{index}/nested/deeper"));
        }
        let explicit = fixture.0.join("existing/branch-0/nested");
        let mut watcher = DirectoryWatcher::new().unwrap();
        watcher.watch(&fixture.0).unwrap();
        assert_eq!(
            watcher.watch_count(),
            1,
            "Registration must not crawl existing children"
        );
        watcher.watch(&explicit).unwrap();
        assert_eq!(watcher.watch_count(), 2);

        // All of these children already exist by the FIRST poll. The former
        // notify kqueue backend discovered an unknown child on NOTE_WRITE and
        // recursively registered its subtree even for a NonRecursive watch.
        for index in 0..128 {
            fixture.directory(&format!("created/branch-{index}/nested/deeper"));
        }
        let events = next_events(&mut watcher);
        assert!(
            events
                .iter()
                .all(|event| event.path == fixture.0 && !event.lost)
        );
        assert_eq!(
            watcher.watch_count(),
            2,
            "A create invalidation must not discover any watches"
        );
        watcher.watch(&fixture.0.join("created/branch-0")).unwrap();
        assert_eq!(
            watcher.watch_count(),
            3,
            "Only the supplied directory owns a new FD"
        );
    }

    #[test]
    fn native_paths_work_and_files_or_final_symlinks_do_not_create_watches() {
        let fixture = Fixture::new();
        let directory = fixture.directory("directory-é-東");
        let link = fixture.0.join("link");
        symlink(&directory, &link).unwrap();
        let file = fixture.0.join("file");
        fs::write(&file, b"not a directory").unwrap();
        let mut watcher = DirectoryWatcher::new().unwrap();
        assert!(watcher.watch(&link).is_err());
        assert!(watcher.watch(&file).is_err());
        assert!(watcher.watch(&fixture.0.join("missing")).is_err());
        assert_eq!(watcher.watch_count(), 0);
        watcher.watch(&directory).unwrap();
        fs::write(directory.join("child-é-東"), b"native").unwrap();
        expect_event(&mut watcher, &directory, false);
        assert_eq!(watcher.watch_count(), 1);

        // APFS can reject invalid UTF-8 names before the watcher is involved.
        // Probe only this additional capability AFTER file/symlink validation
        // and ordinary native-path events have been exercised on every volume.
        let native = fixture
            .0
            .join(OsString::from_vec(b"directory-\xff".to_vec()));
        match fs::create_dir(&native) {
            Ok(()) => {}
            Err(error)
                if error.raw_os_error() == Some(libc::EILSEQ)
                    || error.kind() == io::ErrorKind::InvalidInput =>
            {
                eprintln!(
                    "Filesystem rejects invalid-byte names; skipping only that path case: {error}"
                );
                return;
            }
            Err(error) => panic!("Cannot create invalid-byte path fixture: {error}"),
        }
        watcher.watch(&native).unwrap();
        fs::write(native.join("child"), b"native bytes").unwrap();
        expect_event(&mut watcher, &native, false);
        assert_eq!(watcher.watch_count(), 2);
    }

    #[test]
    fn production_capacity_guard_rejects_640_without_opening_640_descriptors() {
        assert_eq!(MAX_WATCHES, 640);
        ensure_watch_capacity(0).unwrap();
        ensure_watch_capacity(MAX_WATCHES - 1).unwrap();
        for count in [MAX_WATCHES, MAX_WATCHES + 1, usize::MAX] {
            let error = ensure_watch_capacity(count).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert!(error.to_string().contains("640"));
        }
    }

    #[test]
    fn registration_resource_error_preserves_existing_watches_and_allows_retry() {
        let fixture = Fixture::new();
        let existing = fixture.directory("existing");
        let attempted = fixture.directory("attempted");
        let mut watcher = DirectoryWatcher::new().unwrap();
        watcher.watch(&existing).unwrap();

        // Supply a live descriptor that is not a kqueue to force a native
        // registration error, without exhausting or changing the process-wide
        // descriptor limit (which would interfere with parallel worker tests).
        let invalid_queue: OwnedFd = File::open(&fixture.0).unwrap().into();
        let queue = std::mem::replace(&mut watcher.queue, invalid_queue);
        let error = watcher.watch(&attempted).unwrap_err();
        watcher.queue = queue;
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        assert_eq!(watcher.watch_count(), 1);
        assert!(watcher.watched.contains_key(&existing));
        assert!(!watcher.watched.contains_key(&attempted));

        fs::write(existing.join("still-watched"), b"preserved").unwrap();
        expect_event(&mut watcher, &existing, false);
        watcher.watch(&attempted).unwrap();
        fs::write(attempted.join("after-retry"), b"registered").unwrap();
        expect_event(&mut watcher, &attempted, false);
        assert_eq!(watcher.watch_count(), 2);
    }

    #[test]
    fn native_directory_invalidations_are_bounded_and_all_queued_paths_arrive() {
        // Leave room under macOS's low default soft FD limit for other tests.
        // Production capacity is checked separately without resource exhaustion.
        const TEST_WATCHES: usize = 16;
        assert_eq!(MAX_EVENTS, 128);
        let fixture = Fixture::new();
        let mut watcher = DirectoryWatcher::new().unwrap();
        let mut paths = BTreeSet::new();
        for index in 0..TEST_WATCHES {
            let path = fixture.directory(&format!("directory-{index}"));
            watcher.watch(&path).unwrap();
            paths.insert(path);
        }
        assert_eq!(watcher.watch_count(), TEST_WATCHES);
        for path in &paths {
            fs::write(path.join("child"), b"invalidate").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observed = BTreeSet::new();
        while observed.len() != paths.len() {
            let events = watcher.poll().unwrap();
            assert!(events.len() <= MAX_EVENTS);
            for event in events {
                assert!(!event.lost);
                assert!(paths.contains(&event.path));
                observed.insert(event.path);
            }
            assert!(
                Instant::now() < deadline,
                "Not all queued directory invalidations were delivered"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(watcher.watch_count(), TEST_WATCHES);
        watcher.unwatch(paths.first().unwrap()).unwrap();
        assert_eq!(watcher.watch_count(), TEST_WATCHES - 1);
        watcher.watch(&fixture.directory("replacement")).unwrap();
        assert_eq!(watcher.watch_count(), TEST_WATCHES);
    }
}
