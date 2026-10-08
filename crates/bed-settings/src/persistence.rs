//! Atomic replacement for application-owned configuration files.
use serde_json::Value;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
pub fn write_atomic(path: &Path, value: &Value) -> io::Result<()> {
    write(path, value, true)
}

/// Publish a new file atomically without replacing an existing destination.
pub fn write_atomic_new(path: &Path, value: &Value) -> io::Result<()> {
    write(path, value, false)
}

fn write(path: &Path, value: &Value, replace: bool) -> io::Result<()> {
    static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Missing configuration directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".bed-settings-write-{}-{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&temporary, metadata.permissions())?;
        }
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        if replace {
            fs::rename(&temporary, path)
        } else {
            fs::hard_link(&temporary, path)?;
            fs::remove_file(&temporary)
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
