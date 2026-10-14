//! Translated from ned diagnostics_store.{h,cpp}; see LICENSE and NOTICE.
use bedtime_editing::util::doc_path;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub use bedtime_editing::diagnostic::{DiagnosticItem, diagnostic_contains};

#[derive(Default, Debug)]
struct Store {
    remote_paths: bool,
    by_path: HashMap<String, Vec<DiagnosticItem>>,
    raw_by_path: HashMap<String, Vec<serde_json::Value>>,
    versions: HashMap<String, i32>,
    key_cache: HashMap<String, String>,
}

/// Clone handles share the workspace store, replacing C++ owner references.
#[derive(Clone, Default, Debug)]
pub struct LspDiagnostics {
    inner: Arc<Mutex<Store>>,
}

impl LspDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_remote_paths(&self, remote: bool) {
        *self.inner.lock().unwrap() = Store {
            remote_paths: remote,
            ..Default::default()
        };
    }

    pub(crate) fn uses_remote_paths(&self) -> bool {
        self.inner.lock().unwrap().remote_paths
    }

    fn key_for(store: &mut Store, path: &str) -> String {
        if path.is_empty() {
            return String::new();
        }
        let remote = store.remote_paths;
        store
            .key_cache
            .entry(path.to_owned())
            .or_insert_with(|| {
                if remote {
                    path.to_owned()
                } else {
                    doc_path::normalize(path)
                }
            })
            .clone()
    }

    pub fn replace(&self, path: &str, items: Vec<DiagnosticItem>, version: i32) {
        self.replace_raw(path, items, Vec::new(), version);
    }

    pub(crate) fn replace_raw(
        &self,
        path: &str,
        items: Vec<DiagnosticItem>,
        raw: Vec<serde_json::Value>,
        version: i32,
    ) {
        let mut store = self.inner.lock().unwrap();
        let key = Self::key_for(&mut store, path);
        if key.is_empty() {
            return;
        }
        if version >= 0
            && store
                .versions
                .get(&key)
                .is_some_and(|old| *old >= 0 && version < *old)
        {
            return;
        }
        store.versions.insert(key.clone(), version);
        store.raw_by_path.insert(key.clone(), raw);
        store.by_path.insert(key, items);
    }

    pub fn clear(&self, path: &str) {
        let mut store = self.inner.lock().unwrap();
        let key = Self::key_for(&mut store, path);
        if key.is_empty() {
            return;
        }
        store.by_path.remove(&key);
        store.raw_by_path.remove(&key);
        store.versions.remove(&key);
    }

    pub fn clear_all(&self) {
        let mut store = self.inner.lock().unwrap();
        *store = Store {
            remote_paths: store.remote_paths,
            ..Default::default()
        };
    }

    /// Publish a complete presentation snapshot without changing server-owned
    /// versions or raw diagnostic payloads in the source stores.
    pub fn replace_snapshot(
        &self,
        items: std::collections::BTreeMap<String, Vec<DiagnosticItem>>,
        remote_paths: bool,
    ) {
        let mut store = self.inner.lock().unwrap();
        let key_cache = if store.remote_paths == remote_paths {
            std::mem::take(&mut store.key_cache)
        } else {
            HashMap::new()
        };
        let mut next = Store {
            remote_paths,
            key_cache,
            ..Default::default()
        };
        for (path, items) in items {
            let key = Self::key_for(&mut next, &path);
            if !key.is_empty() && !items.is_empty() {
                next.by_path.entry(key).or_default().extend(items);
            }
        }
        *store = next;
    }

    /// One consistent snapshot, including files that have never been opened.
    pub fn snapshot(&self) -> std::collections::BTreeMap<String, Vec<DiagnosticItem>> {
        self.inner
            .lock()
            .unwrap()
            .by_path
            .iter()
            .map(|(path, items)| (path.clone(), items.clone()))
            .collect()
    }

    /// A closed document's last report remains useful, but its next open starts
    /// a new version sequence and must not be rejected as an old publication.
    pub fn forget_version(&self, path: &str) {
        let mut store = self.inner.lock().unwrap();
        let key = Self::key_for(&mut store, path);
        store.versions.remove(&key);
    }

    /// Filesystem identity changes are handled before presentation is published.
    pub fn invalidate_document_keys(&self) {
        self.inner.lock().unwrap().key_cache.clear();
    }

    /// Resolve a document identity before its presentation is drawn.
    pub fn register_document(&self, path: &str) {
        Self::key_for(&mut self.inner.lock().unwrap(), path);
    }

    pub fn for_document(&self, path: &str) -> Vec<DiagnosticItem> {
        let mut store = self.inner.lock().unwrap();
        let key = Self::key_for(&mut store, path);
        store.by_path.get(&key).cloned().unwrap_or_default()
    }

    /// Preserve server-specific diagnostic code/data when requesting quick fixes.
    pub fn raw_for_document(&self, path: &str) -> Vec<serde_json::Value> {
        let mut store = self.inner.lock().unwrap();
        let key = Self::key_for(&mut store, path);
        store.raw_by_path.get(&key).cloned().unwrap_or_default()
    }

    pub fn for_line(&self, path: &str, line: i32) -> Vec<DiagnosticItem> {
        if line < 0 {
            return Vec::new();
        }
        self.for_document(path)
            .into_iter()
            .filter(|item| item.start_line <= line && line <= item.end_line)
            .collect()
    }

    pub fn max_severity_by_line(&self, path: &str, line_count: i32) -> Vec<i32> {
        let mut result = vec![0; line_count.max(0) as usize];
        for item in self.for_document(path) {
            let severity = item.severity.max(1);
            // Equivalent to upstream's inclusive loop, with bounded work for a
            // malformed negative range and no signed overflow at INT_MAX.
            let start = item.start_line.max(0);
            let end = item.end_line.min(line_count.saturating_sub(1));
            for row in start..=end {
                let slot = &mut result[row as usize];
                if *slot == 0 || severity < *slot {
                    *slot = severity;
                }
            }
        }
        result
    }

    pub fn contains(&self, path: &str, line: i32, utf16_column: i32) -> bool {
        self.for_line(path, line)
            .iter()
            .any(|item| diagnostic_contains(item, line, utf16_column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_refresh_preserves_registered_identity_until_explicit_invalidation() {
        let store = LspDiagnostics::new();
        let alias = "/document-alias.rs";
        let canonical = "/canonical/document.rs";
        // Represents an alias registered at document-open time.
        store
            .inner
            .lock()
            .unwrap()
            .key_cache
            .insert(alias.into(), canonical.into());
        let diagnostic = DiagnosticItem {
            message: "still associated with the open document".into(),
            ..Default::default()
        };
        for _ in 0..2 {
            store.replace_snapshot([(canonical.into(), vec![diagnostic.clone()])].into(), false);
            assert_eq!(store.for_document(alias), vec![diagnostic.clone()]);
        }
        store.invalidate_document_keys();
        assert!(store.for_document(alias).is_empty());
        store.replace_snapshot([(alias.into(), vec![diagnostic.clone()])].into(), true);
        assert_eq!(store.for_document(alias), vec![diagnostic]);
        assert!(store.for_document(canonical).is_empty());
    }

    #[test]
    fn presentation_snapshots_replace_all_paths_and_keep_remote_identity() {
        let store = LspDiagnostics::new();
        let diagnostic = DiagnosticItem {
            message: "saved semantic error".into(),
            ..Default::default()
        };
        let path = "/remote/alias/../file.rs";
        store.replace_snapshot([(path.into(), vec![diagnostic.clone()])].into(), true);
        assert_eq!(store.for_document(path), vec![diagnostic]);
        assert!(store.for_document("/remote/file.rs").is_empty());
        store.replace_snapshot(Default::default(), true);
        assert!(store.for_document(path).is_empty());
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn raw_diagnostics_preserve_quick_fix_data_and_follow_version_rejection() {
        let store = LspDiagnostics::new();
        let raw = serde_json::json!({"code":"missing-import","data":{"symbol":"HashMap"}});
        store.replace_raw(
            "/tmp/action.rs",
            vec![DiagnosticItem::default()],
            vec![raw.clone()],
            2,
        );
        store.replace_raw("/tmp/action.rs", vec![], vec![], 1);
        assert_eq!(store.raw_for_document("/tmp/action.rs"), vec![raw]);
        store.clear("/tmp/action.rs");
        assert!(store.raw_for_document("/tmp/action.rs").is_empty());
    }

    #[test]
    fn remote_diagnostic_keys_do_not_use_local_path_normalization_after_clear() {
        let store = LspDiagnostics::new();
        store.set_remote_paths(true);
        let path = "/remote/alias/../file.rs";
        for _ in 0..2 {
            store.replace(path, vec![DiagnosticItem::default()], 1);
            assert_eq!(store.for_document(path).len(), 1);
            assert!(store.for_document("/remote/file.rs").is_empty());
            store.clear_all();
        }
    }

    #[test]
    fn upstream_replace_and_query_by_line() {
        let store = LspDiagnostics::new();
        let error = DiagnosticItem {
            start_line: 2,
            end_line: 2,
            end_character: 4,
            message: "undeclared".into(),
            source: "clang".into(),
            ..Default::default()
        };
        let warning = DiagnosticItem {
            start_line: 2,
            end_line: 3,
            severity: 2,
            message: "unused".into(),
            ..Default::default()
        };
        store.replace("/tmp/a.cpp", vec![error, warning], -1);
        assert_eq!(
            store.max_severity_by_line("/tmp/a.cpp", 5),
            vec![0, 0, 1, 2, 0]
        );
        assert_eq!(store.for_line("/tmp/a.cpp", 2).len(), 2);
        store.clear("/tmp/a.cpp");
        assert_eq!(store.max_severity_by_line("/tmp/a.cpp", 5)[2], 0);
    }

    #[test]
    fn upstream_stale_versions_rejected() {
        let store = LspDiagnostics::new();
        let item = |text: &str| DiagnosticItem {
            message: text.into(),
            ..Default::default()
        };
        store.replace("/tmp/stale.cpp", vec![item("v2")], 2);
        store.replace("/tmp/stale.cpp", vec![item("v1")], 1);
        assert_eq!(store.for_line("/tmp/stale.cpp", 0)[0].message, "v2");
        store.replace("/tmp/stale.cpp", vec![item("v3")], 3);
        assert_eq!(store.for_line("/tmp/stale.cpp", 0)[0].message, "v3");
    }

    #[test]
    fn upstream_versionless_publishes_do_not_block_updates() {
        let store = LspDiagnostics::new();
        let item = |text: &str| DiagnosticItem {
            message: text.into(),
            ..Default::default()
        };
        store.replace("/tmp/anon.cpp", vec![item("anon")], -1);
        store.replace("/tmp/anon.cpp", vec![item("v1")], 1);
        assert_eq!(store.for_line("/tmp/anon.cpp", 0)[0].message, "v1");
        store.replace("/tmp/anon.cpp", vec![item("anon2")], -1);
        store.replace("/tmp/anon.cpp", vec![item("v0")], 0);
        assert_eq!(store.for_line("/tmp/anon.cpp", 0)[0].message, "v0");
    }

    #[test]
    fn inclusive_utf16_ranges_shared_keys_and_severity_limits() {
        let store = LspDiagnostics::new();
        let item = DiagnosticItem {
            start_line: 0,
            start_character: 2,
            end_line: 1,
            end_character: 1,
            severity: 0,
            ..Default::default()
        };
        store.replace("src/../Cargo.toml", vec![item.clone()], 1);
        assert!(store.clone().contains("Cargo.toml", 0, 2));
        assert!(store.contains("Cargo.toml", 1, 1));
        assert!(!store.contains("Cargo.toml", 1, 2));
        assert_eq!(store.max_severity_by_line("Cargo.toml", 2), vec![1, 1]);
        assert!(store.for_line("Cargo.toml", -1).is_empty());
        store.clear_all();
        assert!(store.for_document("Cargo.toml").is_empty());
        assert!(!diagnostic_contains(&item, 0, 1));
    }
}
