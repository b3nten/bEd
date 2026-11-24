//! Watching the files documents hold, on the local disk: told of writes in
//! place, of replaces (an atomic save renames a new file over the old), of
//! deletes and of the file coming back. Moves the app makes
//! (`move_document_sources`) are followed.
//!
//! One watcher serves the process. A notice means only that the file may
//! have changed; the document checks its version before reading it.
//!
//! - macOS: kqueue on the file itself, which hears writes in place. When the
//!   file is replaced or goes, the watch moves to the file now at its path,
//!   or to its folder until one is there again.
//! - Elsewhere: `notify` on the file's folder, which hears its files' writes
//!   as well as files coming and going.
use crate::document::FileDocumentState;
use bedtime_document::{SourceChanged, SourceWatch};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// The process's watcher, or `None` if the platform's couldn't start.
fn watcher() -> Option<&'static Arc<Watcher>> {
    static WATCHER: OnceLock<Option<Arc<Watcher>>> = OnceLock::new();
    WATCHER
        .get_or_init(|| match Watcher::start() {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                eprintln!("Open files won't be watched for changes: {error}");
                None
            }
        })
        .as_ref()
}

/// Watch the file `state` holds, telling `changed`, until the watch is dropped.
pub(crate) fn watch(state: &Arc<FileDocumentState>, changed: SourceChanged) -> Option<SourceWatch> {
    let watcher = watcher()?;
    let id = watcher.add(Arc::downgrade(state), changed);
    Some(SourceWatch::new(Guard {
        id,
        watcher: Arc::downgrade(watcher),
    }))
}

/// Files moved: watch them where they are now.
pub(crate) fn moved() {
    if let Some(watcher) = WATCHER_STARTED.get().and_then(|_| watcher()) {
        watcher.rearm_all();
    }
}

/// Set once something is watched, so `moved` doesn't start a watcher.
static WATCHER_STARTED: OnceLock<()> = OnceLock::new();

struct Guard {
    id: u64,
    watcher: Weak<Watcher>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.upgrade() {
            watcher.remove(self.id);
        }
    }
}

/// What a watched file's watch needs, whatever the platform.
struct Watched {
    state: Weak<FileDocumentState>,
    changed: SourceChanged,
}

impl Watched {
    /// Where the file is now; `None` once its source is gone or deleted.
    fn path(&self) -> Option<PathBuf> {
        let state = self.state.upgrade()?;
        (!state.is_deleted()).then(|| state.path())
    }
}

#[cfg(not(target_os = "macos"))]
use folders::Watcher;
#[cfg(target_os = "macos")]
use kqueue::Watcher;

#[cfg(target_os = "macos")]
mod kqueue {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::fs::OpenOptionsExt,
        },
        path::Path,
        ptr,
        time::Duration,
    };

    const LOST: u32 = libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;
    const FILE_NOTES: u32 = LOST | libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_ATTRIB;
    /// A folder's entries changed: the file may be back.
    const FOLDER_NOTES: u32 = LOST | libc::NOTE_WRITE;
    const MAX_EVENTS: usize = 64;
    /// How often a watch with nothing to watch (its folder is gone too)
    /// tries again.
    const RETRY: Duration = Duration::from_secs(1);

    pub(super) struct Watcher {
        queue: OwnedFd,
        entries: Mutex<Entries>,
    }

    #[derive(Default)]
    struct Entries {
        next: u64,
        by_id: BTreeMap<u64, Entry>,
    }

    struct Entry {
        watched: Watched,
        /// The file, when it's there; else its folder, when that is.
        watching: Option<(File, bool)>,
        /// The path `watching` is for.
        at: Option<PathBuf>,
    }

    /// An event's `udata`: the entry's id, and whether it's for its folder.
    fn key(id: u64, folder: bool) -> usize {
        ((id << 1) | folder as u64) as usize
    }

    impl Watcher {
        pub(super) fn start() -> io::Result<Arc<Self>> {
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
            let watcher = Arc::new(Self {
                queue,
                entries: Mutex::default(),
            });
            let weak = Arc::downgrade(&watcher);
            std::thread::Builder::new()
                .name("document-watch".into())
                .spawn(move || {
                    while let Some(watcher) = weak.upgrade() {
                        watcher.wait();
                    }
                })?;
            Ok(watcher)
        }

        pub(super) fn add(&self, state: Weak<FileDocumentState>, changed: SourceChanged) -> u64 {
            let _ = WATCHER_STARTED.set(());
            let mut entries = self.entries.lock().unwrap();
            entries.next += 1;
            let id = entries.next;
            let mut entry = Entry {
                watched: Watched { state, changed },
                watching: None,
                at: None,
            };
            self.arm(id, &mut entry);
            entries.by_id.insert(id, entry);
            id
        }

        pub(super) fn remove(&self, id: u64) {
            // Closing its descriptor removes its filter, and anything pending.
            self.entries.lock().unwrap().by_id.remove(&id);
        }

        pub(super) fn rearm_all(&self) {
            let mut entries = self.entries.lock().unwrap();
            for (&id, entry) in &mut entries.by_id {
                if entry.at != entry.watched.path() {
                    entry.watching = None;
                    self.arm(id, entry);
                }
            }
        }

        /// Watch the file at the entry's path, or its folder if it isn't
        /// there. Keeps what it's watching if that's still right.
        fn arm(&self, id: u64, entry: &mut Entry) {
            let Some(path) = entry.watched.path() else {
                entry.watching = None;
                entry.at = None;
                return;
            };
            if matches!(entry.watching, Some((_, false))) && entry.at.as_ref() == Some(&path) {
                return;
            }
            entry.watching = None;
            entry.at = Some(path.clone());
            if let Ok(file) = self.add_filter(&path, key(id, false), FILE_NOTES) {
                entry.watching = Some((file, false));
            } else if let Some(folder) = path.parent()
                && let Ok(file) = self.add_filter(folder, key(id, true), FOLDER_NOTES)
            {
                entry.watching = Some((file, true));
            }
        }

        fn add_filter(&self, path: &Path, udata: usize, notes: u32) -> io::Result<File> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_EVTONLY | libc::O_CLOEXEC)
                .open(path)?;
            let change = libc::kevent {
                ident: file.as_raw_fd() as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: notes,
                data: 0,
                udata: udata as *mut libc::c_void,
            };
            // SAFETY: queue and file are live; change points to one
            // initialized record; with no output records kevent only applies
            // it. A failure drops file, removing any filter installed.
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
            Ok(file)
        }

        /// Wait for events (or `RETRY`), and tell the entries they're for.
        fn wait(&self) {
            let mut events = [libc::kevent {
                ident: 0,
                filter: 0,
                flags: 0,
                fflags: 0,
                data: 0,
                udata: ptr::null_mut(),
            }; MAX_EVENTS];
            let timeout = libc::timespec {
                tv_sec: RETRY.as_secs() as libc::time_t,
                tv_nsec: 0,
            };
            // SAFETY: the live queue has no input changes; events is writable
            // for MAX_EVENTS initialized records; timeout is valid.
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
            let mut entries = self.entries.lock().unwrap();
            for event in &events[..count.max(0) as usize] {
                if event.flags & libc::EV_ERROR != 0 || event.filter != libc::EVFILT_VNODE {
                    continue;
                }
                let udata = event.udata as usize as u64;
                let (id, folder) = (udata >> 1, udata & 1 == 1);
                let Some(entry) = entries.by_id.get_mut(&id) else {
                    continue;
                };
                // Stale: from a watch since replaced.
                if entry.watching.as_ref().is_none_or(|(_, f)| *f != folder) {
                    continue;
                }
                if folder || event.fflags & LOST != 0 {
                    // The file came back, or this one went: watch what's there now.
                    entry.watching = None;
                    self.arm(id, entry);
                }
                entry.watched.changed.notify();
            }
            // Nothing to watch (the folder went too): try again.
            for (&id, entry) in &mut entries.by_id {
                if entry.watching.is_none() && entry.watched.path().is_some() {
                    self.arm(id, entry);
                    if entry.watching.is_some() {
                        entry.watched.changed.notify();
                    }
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod folders {
    use super::*;
    use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
    use std::{io, path::Path};

    pub(super) struct Watcher {
        /// Never locked while `native` is, nor `native` called while this
        /// is: notify's thread calls back into `entries`.
        entries: Arc<Mutex<Entries>>,
        native: Mutex<RecommendedWatcher>,
    }

    #[derive(Default)]
    struct Entries {
        next: u64,
        by_id: BTreeMap<u64, Entry>,
        /// Watched folders, with how many entries are in each.
        folders: BTreeMap<PathBuf, usize>,
    }

    struct Entry {
        watched: Watched,
        /// The folder watched for it.
        folder: Option<PathBuf>,
    }

    impl Watcher {
        pub(super) fn start() -> io::Result<Arc<Self>> {
            let entries = Arc::new(Mutex::new(Entries::default()));
            let heard = Arc::downgrade(&entries);
            let native = notify::recommended_watcher(move |event: notify::Result<Event>| {
                if let Some(entries) = heard.upgrade() {
                    tell(&entries.lock().unwrap(), event);
                }
            })
            .map_err(io::Error::other)?;
            Ok(Arc::new(Self {
                entries,
                native: Mutex::new(native),
            }))
        }

        pub(super) fn add(&self, state: Weak<FileDocumentState>, changed: SourceChanged) -> u64 {
            let _ = WATCHER_STARTED.set(());
            let id = {
                let mut entries = self.entries.lock().unwrap();
                entries.next += 1;
                let id = entries.next;
                entries.by_id.insert(
                    id,
                    Entry {
                        watched: Watched { state, changed },
                        folder: None,
                    },
                );
                id
            };
            self.rearm_all();
            id
        }

        pub(super) fn remove(&self, id: u64) {
            self.entries.lock().unwrap().by_id.remove(&id);
            self.rearm_all();
        }

        /// Watch each entry's folder, and stop watching folders nothing is in.
        pub(super) fn rearm_all(&self) {
            let (watch, unwatch) = {
                let mut entries = self.entries.lock().unwrap();
                let entries = &mut *entries;
                let mut wanted: BTreeMap<PathBuf, usize> = BTreeMap::new();
                for entry in entries.by_id.values_mut() {
                    entry.folder = entry
                        .watched
                        .path()
                        .and_then(|path| path.parent().map(Path::to_owned));
                    if let Some(folder) = &entry.folder {
                        *wanted.entry(folder.clone()).or_default() += 1;
                    }
                }
                let watch: Vec<_> = wanted
                    .keys()
                    .filter(|folder| !entries.folders.contains_key(*folder))
                    .cloned()
                    .collect();
                let unwatch: Vec<_> = entries
                    .folders
                    .keys()
                    .filter(|folder| !wanted.contains_key(*folder))
                    .cloned()
                    .collect();
                entries.folders = wanted;
                (watch, unwatch)
            };
            let mut native = self.native.lock().unwrap();
            for folder in unwatch {
                let _ = native.unwatch(&folder);
            }
            for folder in watch {
                let _ = native.watch(&folder, RecursiveMode::NonRecursive);
            }
        }
    }

    /// Tell the entries an event may be about: those whose file it names,
    /// or in whose folder it happened. Every entry, when it's unclear.
    fn tell(entries: &Entries, event: notify::Result<Event>) {
        let paths = match event {
            Ok(event) if !event.need_rescan() && !event.paths.is_empty() => event.paths,
            _ => {
                for entry in entries.by_id.values() {
                    entry.watched.changed.notify();
                }
                return;
            }
        };
        for entry in entries.by_id.values() {
            let Some(folder) = &entry.folder else {
                continue;
            };
            let file = entry.watched.path();
            if paths.iter().any(|path| {
                Some(path) == file.as_ref() || path.parent() == Some(folder) || path == folder
            }) {
                entry.watched.changed.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Filesystem;
    use bedtime_document::{DocumentSource, SourceChanged, TextBuffer};
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Folder(PathBuf);
    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("bedtime-file-watch-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Watch `path`; the count goes up with each notice.
    fn watch(path: &std::path::Path) -> (impl Sized, Arc<AtomicUsize>) {
        let source = Filesystem::local().document_source(path).unwrap();
        let heard = Arc::new(AtomicUsize::new(0));
        let count = heard.clone();
        let watch = DocumentSource::<TextBuffer>::watch(
            &source,
            SourceChanged::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .expect("local files are watched");
        ((source, watch), heard)
    }

    /// Wait until the count passes `seen`, and return it.
    fn heard_after(heard: &AtomicUsize, seen: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let now = heard.load(Ordering::SeqCst);
            if now > seen {
                // Let a burst of events settle.
                std::thread::sleep(Duration::from_millis(50));
                return heard.load(Ordering::SeqCst);
            }
            assert!(Instant::now() < deadline, "no notice");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn hears_writes_in_place_replaces_deletes_and_the_file_coming_back() {
        let folder = Folder::new("kinds");
        let file = folder.0.join("a.txt");
        fs::write(&file, "one").unwrap();
        let (_watch, heard) = watch(&file);

        // Written in place.
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .and_then(|mut f| std::io::Write::write_all(&mut f, b" two"))
            .unwrap();
        let seen = heard_after(&heard, 0);

        // Replaced, as an atomic save does; then written in place again,
        // which only a watch on the new file hears.
        let temporary = folder.0.join(".a.txt.tmp");
        fs::write(&temporary, "three").unwrap();
        fs::rename(&temporary, &file).unwrap();
        let seen = heard_after(&heard, seen);
        fs::write(&file, "four").unwrap();
        let seen = heard_after(&heard, seen);

        // Deleted, then back.
        fs::remove_file(&file).unwrap();
        let seen = heard_after(&heard, seen);
        fs::write(&file, "five").unwrap();
        let seen = heard_after(&heard, seen);
        fs::write(&file, "six").unwrap();
        heard_after(&heard, seen);
    }

    #[test]
    fn follows_a_move_the_app_makes() {
        let folder = Folder::new("moves");
        fs::create_dir(folder.0.join("there")).unwrap();
        let from = folder.0.join("a.txt");
        let to = folder.0.join("there").join("b.txt");
        fs::write(&from, "one").unwrap();
        let (_watch, heard) = watch(&from);
        fs::rename(&from, &to).unwrap();
        Filesystem::local().move_document_sources(&from, &to);
        std::thread::sleep(Duration::from_millis(100));
        let seen = heard.load(Ordering::SeqCst);
        fs::write(&to, "two").unwrap();
        heard_after(&heard, seen);
    }

    #[test]
    fn stops_when_the_watch_is_dropped() {
        let folder = Folder::new("dropped");
        let file = folder.0.join("a.txt");
        fs::write(&file, "one").unwrap();
        let (watch, heard) = watch(&file);
        drop(watch);
        fs::write(&file, "two").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(heard.load(Ordering::SeqCst), 0);
    }
}
