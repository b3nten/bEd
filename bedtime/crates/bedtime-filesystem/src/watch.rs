//! Bounded, nonrecursive directory invalidations. Consumers reconcile their
//! own observations; notifications are never a complete filesystem history.
#[cfg(target_os = "macos")]
use crate::mac_watch::DirectoryWatcher as NativeWatcher;
#[cfg(not(target_os = "macos"))]
use notify::{Event, EventKind, RecommendedWatcher as NativeWatcher, RecursiveMode, Watcher};
#[cfg(not(target_os = "macos"))]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::{
    io,
    path::{Path, PathBuf},
};

pub struct DirectoryEvent {
    pub path: PathBuf,
    pub lost: bool,
}
#[derive(Default)]
pub struct WatchBatch {
    pub events: Vec<DirectoryEvent>,
    pub rescan: bool,
    pub error: Option<String>,
}
pub struct DirectoryWatcher {
    native: NativeWatcher,
    #[cfg(not(target_os = "macos"))]
    receiver: Receiver<notify::Result<Event>>,
    #[cfg(not(target_os = "macos"))]
    overflow: Arc<AtomicBool>,
}
impl DirectoryWatcher {
    pub fn new() -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                native: NativeWatcher::new()?,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let (sender, receiver) = mpsc::sync_channel(128);
            let overflow = Arc::new(AtomicBool::new(false));
            let callback_overflow = overflow.clone();
            let native = notify::recommended_watcher(move |event: notify::Result<Event>| {
                if event.as_ref().is_ok_and(|event| event.paths.len() > 256) {
                    callback_overflow.store(true, Ordering::Release);
                    return;
                }
                if sender.try_send(event).is_err() {
                    callback_overflow.store(true, Ordering::Release);
                }
            })
            .map_err(io::Error::other)?;
            Ok(Self {
                native,
                receiver,
                overflow,
            })
        }
    }
    pub fn watch(&mut self, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.native.watch(path)
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.native
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)
        }
    }
    pub fn unwatch(&mut self, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.native.unwatch(path)
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native.unwatch(path) {
                Ok(()) => Ok(()),
                Err(error)
                    if matches!(
                        error.kind,
                        notify::ErrorKind::WatchNotFound | notify::ErrorKind::PathNotFound
                    ) =>
                {
                    Ok(())
                }
                Err(error) => Err(io::Error::other(error)),
            }
        }
    }
    pub fn poll(&mut self) -> io::Result<WatchBatch> {
        #[cfg(target_os = "macos")]
        {
            Ok(WatchBatch {
                events: self
                    .native
                    .poll()?
                    .into_iter()
                    .map(|event| DirectoryEvent {
                        path: event.path,
                        lost: event.lost,
                    })
                    .collect(),
                ..Default::default()
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut batch = WatchBatch {
                rescan: self.overflow.swap(false, Ordering::AcqRel),
                ..Default::default()
            };
            for _ in 0..128 {
                match self.receiver.try_recv() {
                    Ok(Ok(event)) => {
                        if matches!(event.kind, EventKind::Access(_)) {
                            continue;
                        }
                        batch.rescan |= event.need_rescan() || event.paths.is_empty();
                        let lost = matches!(
                            event.kind,
                            EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        );
                        batch.events.extend(
                            event
                                .paths
                                .into_iter()
                                .map(|path| DirectoryEvent { path, lost }),
                        );
                    }
                    Ok(Err(error)) => {
                        batch.error = Some(error.to_string());
                        batch.rescan = true;
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            Ok(batch)
        }
    }
}
