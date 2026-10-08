//! Translated from ned lsp/lsp_document_sync.{h,cpp}; see LICENSE and NOTICE.
//! State stays on the UI thread; a borrowed sink forwards ordered messages to
//! the transport worker. Full-text providers are invoked only when needed.
use crate::{
    diagnostics::LspDiagnostics,
    lsp_config::{LanguageServerInfo, LspConfig},
    lsp_uri::LspUri,
};
use bed_editing::{editor_events::DocumentChange, util::doc_path};
use serde_json::{Value, json};
use std::{collections::HashSet, io};

pub trait NotificationSink {
    fn send_notification(&self, method: &str, params: Option<Value>) -> io::Result<()>;
}

fn invalid_capabilities() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid LSP initialize capabilities",
    )
}

fn sync_number(value: &Value) -> io::Result<i32> {
    // Pinned framework JSON numbers are doubles, converted to `int` before
    // assigning the enumeration. Custom integer enumeration values are valid.
    // Truncate fractions the same way and reject C++'s undefined out-of-range
    // cast instead of wrapping a malformed server response.
    let number = value.as_f64().ok_or_else(invalid_capabilities)?.trunc();
    if number.is_finite() && number >= f64::from(i32::MIN) && number <= f64::from(i32::MAX) {
        Ok(number as i32)
    } else {
        Err(invalid_capabilities())
    }
}

/// Validate the original typed initialize fields used by synchronization.
/// Optional nulls are absent, as in lsp-framework's `fromJson(optional<T>)`.
pub fn validate_initialize_result(result: &Value) -> io::Result<()> {
    let capabilities = result
        .as_object()
        .and_then(|result| result.get("capabilities"))
        .and_then(Value::as_object)
        .ok_or_else(invalid_capabilities)?;
    let Some(sync) = capabilities
        .get("textDocumentSync")
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    let Some(options) = sync.as_object() else {
        return sync_number(sync).map(|_| ());
    };
    if let Some(change) = options.get("change").filter(|value| !value.is_null()) {
        sync_number(change)?;
    }
    for key in ["openClose", "willSave", "willSaveWaitUntil"] {
        if options
            .get(key)
            .is_some_and(|value| !value.is_null() && !value.is_boolean())
        {
            return Err(invalid_capabilities());
        }
    }
    if let Some(save) = options.get("save").filter(|value| !value.is_null()) {
        if let Some(options) = save.as_object() {
            if options
                .get("includeText")
                .is_some_and(|value| !value.is_null() && !value.is_boolean())
            {
                return Err(invalid_capabilities());
            }
        } else if !save.is_boolean() {
            return Err(invalid_capabilities());
        }
    }
    Ok(())
}
#[derive(Clone, Debug)]
struct PendingOpen {
    key: String,
    content: String,
    language_id: String,
    version: i32,
}
pub struct LspDocumentSync {
    remote_paths: bool,
    diagnostics: LspDiagnostics,
    languages: LspConfig,
    connected: bool,
    ready: bool,
    sync_kind: i32,
    notify_did_save: bool,
    save_include_text: bool,
    pending_opens: Vec<PendingOpen>,
    open_documents: HashSet<String>,
}
impl Default for LspDocumentSync {
    fn default() -> Self {
        Self::new(LspDiagnostics::new())
    }
}
impl LspDocumentSync {
    pub fn new(diagnostics: LspDiagnostics) -> Self {
        Self {
            remote_paths: false,
            diagnostics,
            languages: LspConfig::default(),
            connected: false,
            ready: false,
            sync_kind: 2,
            notify_did_save: false,
            save_include_text: false,
            pending_opens: Vec::new(),
            open_documents: HashSet::new(),
        }
    }
    pub fn set_languages(&mut self, languages: &[LanguageServerInfo]) {
        self.languages.language_servers = languages.to_vec();
    }
    pub fn set_remote_paths(&mut self, remote: bool) {
        self.disconnect();
        self.remote_paths = remote;
    }
    fn key(&self, path: &str) -> String {
        if self.remote_paths {
            path.to_owned()
        } else {
            doc_path::normalize(path)
        }
    }
    fn file_uri(&self, path: &str) -> io::Result<LspUri> {
        if self.remote_paths {
            LspUri::file_uri_from_remote_path(path)
        } else {
            LspUri::file_uri_from_path(path)
        }
    }
    pub fn connect(&mut self) {
        self.connected = true;
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.ready = false;
        self.notify_did_save = false;
        self.save_include_text = false;
        self.sync_kind = 2;
        self.pending_opens.clear();
        self.open_documents.clear();
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    pub fn is_document_open(&self, path: &str) -> bool {
        !path.is_empty() && self.open_documents.contains(&self.key(path))
    }
    /// A closed shared document must not be resurrected after initialize.
    pub fn cancel_pending_open(&mut self, path: &str) {
        let key = self.key(path);
        self.pending_opens.retain(|document| document.key != key);
        if !self.open_documents.contains(&key) {
            self.diagnostics.clear(&key);
        }
    }
    /// The caller has already decoded the initialize result. Missing sync
    /// capabilities retain the original default incremental behavior.
    pub fn apply_capabilities(&mut self, result: &Value) {
        self.sync_kind = 2;
        self.notify_did_save = false;
        self.save_include_text = false;
        let Some(sync) = result
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("textDocumentSync"))
        else {
            return;
        };
        if let Some(options) = sync.as_object() {
            if let Some(kind) = options
                .get("change")
                .and_then(|value| sync_number(value).ok())
            {
                self.sync_kind = kind;
            }
            if let Some(save) = options.get("save").filter(|save| !save.is_null()) {
                // The framework decodes null optional values as absent;
                // presence, including `false`, is what upstream checks.
                self.notify_did_save = true;
                self.save_include_text = save
                    .get("includeText")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
        } else if let Ok(kind) = sync_number(sync) {
            self.sync_kind = kind;
        }
    }
    pub fn mark_handshake_ready(&mut self, sink: &impl NotificationSink) -> io::Result<()> {
        self.ready = true;
        self.flush_pending(sink)
    }
    fn upsert_pending_open(
        &mut self,
        key: String,
        content: String,
        version: i32,
        language_id: String,
    ) {
        if let Some(document) = self
            .pending_opens
            .iter_mut()
            .find(|document| document.key == key)
        {
            document.content = content;
            document.version = version;
            return;
        }
        self.pending_opens.push(PendingOpen {
            key,
            content,
            version,
            language_id,
        });
    }
    fn send_did_open(
        &mut self,
        key: &str,
        content: &str,
        version: i32,
        language_id: &str,
        sink: &impl NotificationSink,
    ) -> io::Result<()> {
        if !self.connected || key.is_empty() {
            return Ok(());
        }
        let detected = self.languages.detect_language(key);
        let language = if !detected.is_empty() {
            detected.as_str()
        } else if !language_id.is_empty() {
            language_id
        } else {
            "plaintext"
        };
        sink.send_notification("textDocument/didOpen",Some(json!({"textDocument":{"uri":self.file_uri(key)?.to_string(),"languageId":language,"version":version,"text":content}})))?;
        self.open_documents.insert(key.to_owned());
        Ok(())
    }
    fn flush_pending(&mut self, sink: &impl NotificationSink) -> io::Result<()> {
        let mut first_error = None;
        for document in std::mem::take(&mut self.pending_opens) {
            let result = if self.open_documents.contains(&document.key) {
                self.did_change(
                    &document.key,
                    document.version,
                    &[],
                    || Ok(document.content),
                    sink,
                )
            } else {
                self.send_did_open(
                    &document.key,
                    &document.content,
                    document.version,
                    &document.language_id,
                    sink,
                )
            };
            // The original flush attempts every queued open even if one fails.
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
    pub fn did_open(
        &mut self,
        path: &str,
        content: &str,
        version: i32,
        language_id: &str,
        sink: &impl NotificationSink,
    ) -> io::Result<()> {
        if !self.connected || path.is_empty() {
            return Ok(());
        }
        let key = self.key(path);
        if !self.ready {
            self.upsert_pending_open(key, content.to_owned(), version, language_id.to_owned());
            return Ok(());
        }
        if self.open_documents.contains(&key) {
            return self.did_change(&key, version, &[], || Ok(content.to_owned()), sink);
        }
        self.send_did_open(&key, content, version, language_id, sink)
    }
    pub fn did_change(
        &mut self,
        path: &str,
        version: i32,
        changes: &[DocumentChange],
        full_text: impl FnOnce() -> io::Result<String>,
        sink: &impl NotificationSink,
    ) -> io::Result<()> {
        if !self.connected || path.is_empty() {
            return Ok(());
        }
        let key = self.key(path);
        if !self.open_documents.contains(&key) {
            self.upsert_pending_open(key, full_text()?, version, String::new());
            if self.ready {
                self.flush_pending(sink)?;
            }
            return Ok(());
        }
        if self.sync_kind == 0 {
            return Ok(());
        }
        let content_changes = if self.sync_kind == 2 && !changes.is_empty() {
            changes.iter().map(|change| {
                let text=std::str::from_utf8(&change.text).map_err(|error|io::Error::new(io::ErrorKind::InvalidData,error))?;
                Ok(json!({"range":{"start":{"line":change.start_line.max(0),"character":change.start_character.max(0)},"end":{"line":change.end_line.max(0),"character":change.end_character.max(0)}},"text":text}))
            }).collect::<io::Result<Vec<Value>>>()?
        } else {
            vec![json!({"text":full_text()?})]
        };
        sink.send_notification("textDocument/didChange",Some(json!({"textDocument":{"uri":self.file_uri(&key)?.to_string(),"version":version},"contentChanges":content_changes})))
    }
    pub fn did_save(
        &mut self,
        path: &str,
        full_text: impl FnOnce() -> io::Result<String>,
        sink: &impl NotificationSink,
    ) -> io::Result<()> {
        if !self.ready || !self.connected || path.is_empty() || !self.notify_did_save {
            return Ok(());
        }
        let key = self.key(path);
        if !self.open_documents.contains(&key) {
            return Ok(());
        }
        let mut params = json!({"textDocument":{"uri":self.file_uri(&key)?.to_string()}});
        if self.save_include_text {
            params["text"] = Value::String(full_text()?);
        }
        sink.send_notification("textDocument/didSave", Some(params))
    }
    pub fn did_close(&mut self, path: &str, sink: &impl NotificationSink) -> io::Result<()> {
        if !self.connected || path.is_empty() {
            return Ok(());
        }
        let key = self.key(path);
        if !self.open_documents.contains(&key) {
            return Ok(());
        }
        let result = self.file_uri(&key).and_then(|uri| {
            sink.send_notification(
                "textDocument/didClose",
                Some(json!({"textDocument":{"uri":uri.to_string()}})),
            )
        });
        self.open_documents.remove(&key);
        self.diagnostics.clear(&key);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[derive(Default)]
    struct Sink {
        messages: RefCell<Vec<(String, Value)>>,
        fail: Cell<bool>,
    }
    impl NotificationSink for Sink {
        fn send_notification(&self, method: &str, params: Option<Value>) -> io::Result<()> {
            if self.fail.get() {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "mock pipe failed",
                ));
            }
            self.messages
                .borrow_mut()
                .push((method.into(), params.unwrap_or(Value::Null)));
            Ok(())
        }
    }
    fn ready(sync: &mut LspDocumentSync, sink: &Sink, capabilities: Value) {
        sync.connect();
        sync.apply_capabilities(&json!({"capabilities":capabilities}));
        sync.mark_handshake_ready(sink).unwrap();
    }
    #[test]
    fn remote_document_paths_remain_target_native_through_sync() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        sync.set_remote_paths(true);
        ready(
            &mut sync,
            &sink,
            json!({"textDocumentSync":{"change":2,"save":true}}),
        );
        let path = "/remote/alias/../file name.rs";
        sync.did_open(path, "first", 1, "rust", &sink).unwrap();
        sync.did_change(path, 2, &[], || Ok("second".into()), &sink)
            .unwrap();
        sync.did_save(path, || panic!("save should not join text"), &sink)
            .unwrap();
        assert!(sync.is_document_open(path));
        sync.did_close(path, &sink).unwrap();
        assert!(!sync.is_document_open(path));
        assert_eq!(sink.messages.borrow().len(), 4);
        for (_, message) in sink.messages.borrow().iter() {
            assert_eq!(
                message["textDocument"]["uri"],
                "file:///remote/alias/../file%20name.rs"
            );
        }
    }
    #[test]
    fn queued_open_latest_text_first_language_and_insertion_order() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        sync.connect();
        sync.did_open("Cargo.toml", "old", 1, "first", &sink)
            .unwrap();
        sync.did_open("Cargo.toml", "new", 2, "second", &sink)
            .unwrap();
        sync.did_open("src/lib.rs", "other", 1, "rust", &sink)
            .unwrap();
        sync.did_change("Cargo.toml", 3, &[], || Ok("latest".into()), &sink)
            .unwrap();
        assert!(sink.messages.borrow().is_empty());
        assert!(!sync.is_document_open("Cargo.toml"));
        sync.mark_handshake_ready(&sink).unwrap();
        let messages = sink.messages.borrow();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].1["textDocument"]["text"], "latest");
        assert_eq!(messages[0].1["textDocument"]["version"], 3);
        assert_eq!(messages[0].1["textDocument"]["languageId"], "first");
        assert_eq!(messages[1].1["textDocument"]["text"], "other");
    }
    #[test]
    fn incremental_changes_keep_utf16_sequence_and_never_join_full_text() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        ready(&mut sync, &sink, json!({}));
        sync.did_open("src/lib.rs", "a🙂b", 0, "rust", &sink)
            .unwrap();
        let changes = [
            DocumentChange {
                start_character: 1,
                end_character: 3,
                text: b"x".to_vec(),
                ..Default::default()
            },
            DocumentChange {
                start_character: -4,
                end_character: -3,
                text: "é".as_bytes().to_vec(),
                ..Default::default()
            },
        ];
        sync.did_change(
            "src/lib.rs",
            1,
            &changes,
            || panic!("incremental sync joined text"),
            &sink,
        )
        .unwrap();
        let messages = sink.messages.borrow();
        let events = messages[1].1["contentChanges"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["range"]["end"]["character"], 3);
        assert_eq!(events[1]["range"]["start"]["character"], 0);
        assert_eq!(events[1]["text"], "é");
    }
    #[test]
    fn full_none_and_empty_incremental_change_list_negotiate_lazy_provider() {
        for kind in [0, 1, 2] {
            let sink = Sink::default();
            let mut sync = LspDocumentSync::default();
            ready(&mut sync, &sink, json!({"textDocumentSync":kind}));
            sync.did_open("Cargo.toml", "old", 0, "toml", &sink)
                .unwrap();
            let calls = Cell::new(0);
            sync.did_change(
                "Cargo.toml",
                1,
                &[],
                || {
                    calls.set(calls.get() + 1);
                    Ok("full".into())
                },
                &sink,
            )
            .unwrap();
            assert_eq!(calls.get(), i32::from(kind != 0));
            assert_eq!(sink.messages.borrow().len(), if kind == 0 { 1 } else { 2 });
        }
    }
    #[test]
    fn save_presence_false_quirk_and_include_text_provider() {
        for save in [
            None,
            Some(Value::Null),
            Some(json!(false)),
            Some(json!(true)),
            Some(json!({"includeText":true})),
        ] {
            let sink = Sink::default();
            let mut sync = LspDocumentSync::default();
            let mut options = json!({"change":2});
            if let Some(save) = &save {
                options["save"] = save.clone();
            }
            ready(&mut sync, &sink, json!({"textDocumentSync":options}));
            sync.did_open("Cargo.toml", "initial", 0, "toml", &sink)
                .unwrap();
            let calls = Cell::new(0);
            sync.did_save(
                "Cargo.toml",
                || {
                    calls.set(calls.get() + 1);
                    Ok("saved".into())
                },
                &sink,
            )
            .unwrap();
            let include = save
                .as_ref()
                .is_some_and(|value| value.get("includeText") == Some(&json!(true)));
            assert_eq!(calls.get(), i32::from(include));
            assert_eq!(
                sink.messages.borrow().len(),
                if save.as_ref().is_some_and(|value| !value.is_null()) {
                    2
                } else {
                    1
                }
            );
            if include {
                assert_eq!(sink.messages.borrow()[1].1["text"], "saved");
            }
        }
    }
    #[test]
    fn reopen_is_change_and_close_clears_shared_store_even_on_failed_notify() {
        let sink = Sink::default();
        let diagnostics = LspDiagnostics::new();
        let mut sync = LspDocumentSync::new(diagnostics.clone());
        ready(&mut sync, &sink, json!({}));
        sync.did_open("Cargo.toml", "old", 0, "toml", &sink)
            .unwrap();
        sync.did_open("src/../Cargo.toml", "new", 1, "toml", &sink)
            .unwrap();
        assert_eq!(sink.messages.borrow()[1].0, "textDocument/didChange");
        diagnostics.replace("Cargo.toml", vec![Default::default()], 1);
        sink.fail.set(true);
        assert!(sync.did_close("Cargo.toml", &sink).is_err());
        assert!(!sync.is_document_open("Cargo.toml"));
        assert!(diagnostics.for_document("Cargo.toml").is_empty());
    }
    #[test]
    fn untracked_change_opens_and_pending_close_retains_original_quirk() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        sync.connect();
        sync.did_open("Cargo.toml", "pending", 0, "toml", &sink)
            .unwrap();
        sync.did_close("Cargo.toml", &sink).unwrap();
        sync.mark_handshake_ready(&sink).unwrap();
        assert!(sync.is_document_open("Cargo.toml"));
        sync.did_change("src/lib.rs", 9, &[], || Ok("untracked".into()), &sink)
            .unwrap();
        assert!(sync.is_document_open("src/lib.rs"));
        assert_eq!(sink.messages.borrow()[1].0, "textDocument/didOpen");
        sync.disconnect();
        assert!(!sync.is_ready());
        assert!(!sync.is_document_open("Cargo.toml"));
    }
    #[test]
    fn workspace_close_cancels_queued_open_before_handshake() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        sync.connect();
        sync.did_open("closed.rs", "must not reopen", 2, "rust", &sink)
            .unwrap();
        sync.did_open("kept.rs", "retained", 3, "rust", &sink)
            .unwrap();
        sync.cancel_pending_open("closed.rs");
        sync.did_close("closed.rs", &sink).unwrap();
        sync.mark_handshake_ready(&sink).unwrap();
        assert!(!sync.is_document_open("closed.rs"));
        assert!(sync.is_document_open("kept.rs"));
        assert_eq!(sink.messages.borrow().len(), 1);
    }
    #[test]
    fn configured_language_precedes_editor_language_and_disconnect_guards_providers() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        sync.set_languages(&[LanguageServerInfo {
            language: "cpp".into(),
            file_extensions: vec![".c".into()],
            ..Default::default()
        }]);
        ready(
            &mut sync,
            &sink,
            json!({"textDocumentSync":{"openClose":false,"change":2}}),
        );
        sync.did_open("a.c", "int x;", 0, "c", &sink).unwrap();
        assert_eq!(
            sink.messages.borrow()[0].1["textDocument"]["languageId"],
            "cpp"
        );
        sync.disconnect();
        sync.did_change("a.c", 1, &[], || panic!("disconnected joined text"), &sink)
            .unwrap();
        sync.did_save("a.c", || panic!("disconnected joined text"), &sink)
            .unwrap();
    }
    #[test]
    fn invalid_edit_bytes_and_failed_full_provider_do_not_send_lossy_text() {
        let sink = Sink::default();
        let mut sync = LspDocumentSync::default();
        ready(&mut sync, &sink, json!({}));
        sync.did_open("Cargo.toml", "valid", 0, "toml", &sink)
            .unwrap();
        let change = DocumentChange {
            text: vec![0xff],
            ..Default::default()
        };
        assert_eq!(
            sync.did_change(
                "Cargo.toml",
                1,
                &[change],
                || panic!("invalid incremental joined"),
                &sink
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(
            sync.did_change(
                "Cargo.toml",
                2,
                &[],
                || Err(io::Error::new(io::ErrorKind::InvalidData, "invalid buffer")),
                &sink
            )
            .is_err()
        );
        assert_eq!(sink.messages.borrow().len(), 1);
    }
    #[test]
    fn typed_initialize_capabilities_accept_null_fractional_and_custom_enumerations() {
        for sync in [
            Value::Null,
            json!(2.9),
            json!(37),
            json!({"change":null,"openClose":null,"save":null}),
            json!({"change":1.9,"save":{"includeText":null}}),
        ] {
            let result = json!({"capabilities":{"textDocumentSync":sync}});
            validate_initialize_result(&result).unwrap();
            let mut sync = LspDocumentSync::default();
            sync.apply_capabilities(&result);
            if result["capabilities"]["textDocumentSync"] == json!(2.9) {
                assert_eq!(sync.sync_kind, 2);
            }
        }
        for result in [
            json!({}),
            json!({"capabilities":null}),
            json!({"capabilities":{"textDocumentSync":true}}),
            json!({"capabilities":{"textDocumentSync":2147483648_u64}}),
            json!({"capabilities":{"textDocumentSync":{"change":"2"}}}),
            json!({"capabilities":{"textDocumentSync":{"save":{"includeText":1}}}}),
            json!({"capabilities":{"textDocumentSync":{"openClose":1}}}),
        ] {
            assert_eq!(
                validate_initialize_result(&result).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
