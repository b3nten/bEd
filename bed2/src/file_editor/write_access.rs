//! Admission for application-owned file writes. Leases span actual worker I/O,
//! including cancellation cleanup; acquiring one never waits on another writer.
use bevy::prelude::*;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Resource, Default)]
pub(crate) struct FileWrites(Arc<Mutex<Vec<Scope>>>);

#[derive(Clone, PartialEq, Eq)]
enum Scope {
    File(PathBuf),
    Repository(PathBuf),
}

pub(crate) struct WriteLease {
    writes: FileWrites,
    scope: Scope,
}

impl FileWrites {
    pub fn file(&self, path: &Path) -> Result<WriteLease, String> {
        self.acquire(Scope::File(path.to_owned()))
    }
    pub fn repository(&self, root: &Path) -> Result<WriteLease, String> {
        self.acquire(Scope::Repository(root.to_owned()))
    }
    pub fn blocks(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|scope| matches!(scope, Scope::Repository(root) if path.starts_with(root)))
    }
    fn acquire(&self, scope: Scope) -> Result<WriteLease, String> {
        let mut active = self.0.lock().unwrap();
        if active.iter().any(|other| match (&scope, other) {
            (Scope::File(a), Scope::File(b)) => a == b,
            (Scope::Repository(a), Scope::Repository(b)) => a.starts_with(b) || b.starts_with(a),
            (Scope::File(file), Scope::Repository(root))
            | (Scope::Repository(root), Scope::File(file)) => file.starts_with(root),
        }) {
            return Err("A file save or Git operation is writing this source. Wait for it to finish and retry.".into());
        }
        active.push(scope.clone());
        Ok(WriteLease {
            writes: self.clone(),
            scope,
        })
    }
}

impl Drop for WriteLease {
    fn drop(&mut self) {
        let mut active = self.writes.0.lock().unwrap();
        let index = active
            .iter()
            .position(|scope| *scope == self.scope)
            .expect("write lease owns its admission");
        active.swap_remove(index);
    }
}
