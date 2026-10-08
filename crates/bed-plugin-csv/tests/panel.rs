use bed_editing::identity::DocumentId;
use bed_plugin::{
    DocumentKind, HostContext, HostRequest, PanelAction, PluginDocument, PluginPanel,
};
use bed_plugin_csv::CsvPanel;
use dear_imgui_rs::{Condition, Context, FramePrepareOptions, Key, WindowFlags};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

struct Clipboard(Rc<RefCell<String>>);
impl dear_imgui_rs::ClipboardBackend for Clipboard {
    fn get(&mut self) -> Option<String> {
        Some(self.0.borrow().clone())
    }
    fn set(&mut self, text: &str) {
        *self.0.borrow_mut() = text.to_owned();
    }
}

static IMGUI_LOCK: Mutex<()> = Mutex::new(());

struct Harness {
    context: Context,
    panel: CsvPanel,
    documents: Vec<PluginDocument>,
    requests: Vec<HostRequest>,
}

impl Harness {
    fn new(bytes: &[u8], state: Value) -> Self {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let id = DocumentId::next();
        Self {
            context,
            panel: CsvPanel::new(id, &state),
            documents: vec![PluginDocument {
                id,
                path: "fixture.csv".into(),
                kind: DocumentKind::Text,
                language_id: "csv".into(),
                revision: (0, 0),
                dirty: false,
                bytes: bytes.to_vec().into(),
                text: None,
            }],
            requests: Vec::new(),
        }
    }

    fn frame(&mut self) -> usize {
        self.context
            .prepare_frame(FramePrepareOptions::new([1000.0, 700.0], 1.0 / 60.0));
        let ui = self.context.frame();
        let host = HostContext {
            documents: &self.documents,
            active_document: Some(self.documents[0].id),
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        };
        ui.window("CSV fixture")
            .position([0.0, 0.0], Condition::Always)
            .size([1000.0, 700.0], Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR)
            .build(|| self.panel.draw(ui, &host, &mut self.requests));
        self.context.render_legacy().total_vtx_count()
    }

    fn wait_ready(&mut self) {
        // Observe a replacement host snapshot before consulting panel readiness.
        self.frame();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.panel.is_ready() {
            self.frame();
            assert!(self.panel.error().is_none(), "{:?}", self.panel.error());
            assert!(Instant::now() < deadline, "CSV worker did not finish");
            thread::sleep(Duration::from_millis(1));
        }
        self.frame();
    }

    fn key(&mut self, key: Key) {
        self.context.io_mut().add_key_event(key, true);
        self.frame();
        self.context.io_mut().add_key_event(key, false);
        self.frame();
    }

    fn type_replacement(&mut self, value: &str) {
        self.context.io_mut().add_input_characters_utf8(value);
        self.frame();
        self.frame();
    }

    fn chord(&mut self, key: Key) {
        self.context.io_mut().add_key_event(Key::ModCtrl, true);
        self.context.io_mut().add_key_event(key, true);
        self.frame();
        self.context.io_mut().add_key_event(key, false);
        self.context.io_mut().add_key_event(Key::ModCtrl, false);
        self.frame();
    }

    fn action(&mut self, action: PanelAction) -> Result<bool, String> {
        let host = HostContext {
            documents: &self.documents,
            active_document: Some(self.documents[0].id),
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        };
        self.panel.action(action, &host, &mut self.requests)
    }

    fn apply_requests(&mut self) -> Vec<u8> {
        let mut bytes = self.documents[0].bytes.to_vec();
        for request in &self.requests {
            let (document, revision, edits, token) = match request {
                HostRequest::ApplyEdits {
                    document,
                    revision,
                    edits,
                } => (document, revision, edits, None),
                HostRequest::ApplyEditsWithResult {
                    token,
                    document,
                    revision,
                    edits,
                } => (document, revision, edits, Some(*token)),
                _ => continue,
            };
            assert_eq!(*document, self.documents[0].id);
            assert_eq!(*revision, self.documents[0].revision);
            let mut edits = edits.clone();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
            for edit in edits {
                bytes.splice(edit.range, edit.bytes);
            }
            if let Some(token) = token {
                self.panel
                    .edit_result(token, Ok((revision.0, revision.1 + 1)));
            }
        }
        bytes
    }
}

#[test]
fn tab_commits_once_and_advances_while_enter_commits_in_place() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(
        b"name,value\nfirst,001\nsecond,002\n",
        json!({"header":true}),
    );
    harness.wait_ready();
    harness.type_replacement("changed");
    harness.key(Key::Tab);
    assert_eq!(harness.requests.len(), 1, "Tab must commit the draft once");
    assert_eq!(
        harness.apply_requests(),
        b"name,value\nchanged,001\nsecond,002\n"
    );
    assert_eq!(
        harness.panel.save_state()["selection"]["focus"],
        json!([0, 1])
    );
    harness.documents[0].bytes = harness.apply_requests().into();
    harness.documents[0].revision = (0, 1);
    harness.requests.clear();
    harness.wait_ready();
    harness.type_replacement("0007");
    assert_eq!(
        harness.panel.save_state()["global_filter"],
        json!(""),
        "Tab must retain grid focus"
    );
    harness.key(Key::Enter);
    assert_eq!(
        harness.requests.len(),
        1,
        "Enter must commit the draft; state {:?}, error {:?}",
        harness.panel.save_state(),
        harness.panel.error()
    );
    assert_eq!(
        harness.apply_requests(),
        b"name,value\nchanged,0007\nsecond,002\n"
    );
    assert_eq!(
        harness.panel.save_state()["selection"]["focus"],
        json!([0, 1])
    );
}

#[test]
fn queued_non_bmp_and_combining_text_is_preserved() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(b"name,value\nfirst,001\n", json!({"header":true}));
    harness.wait_ready();
    harness.type_replacement("東京 🦉 e\u{301}");
    harness.action(PanelAction::CommitEdit).unwrap();
    assert_eq!(
        harness.apply_requests(),
        "name,value\n東京 🦉 e\u{301},001\n".as_bytes()
    );
}

#[test]
fn sorted_filtered_clipboard_edits_visible_rows_and_rejects_overflow() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(
        b"id,value\n1,a\n2,b\n3,c\n",
        json!({
            "header":true,
            "sort":{"column":0,"descending":true,"mode":"Number"},
            "filters":[{"column":1,"value":"b","op":"Equals"}],
            "selection":{"anchor":[0,1],"focus":[0,1]}
        }),
    );
    let clipboard = Rc::new(RefCell::new(String::from("updated")));
    harness
        .context
        .set_clipboard_backend(Clipboard(Rc::clone(&clipboard)));
    harness.wait_ready();
    harness.chord(Key::V);
    assert_eq!(harness.requests.len(), 1);
    assert_eq!(harness.apply_requests(), b"id,value\n1,a\n2,updated\n3,c\n");
    harness.documents[0].bytes = harness.apply_requests().into();
    harness.documents[0].revision = (0, 1);
    harness.requests.clear();
    harness.wait_ready();
    *clipboard.borrow_mut() = "x\ny".into();
    harness.chord(Key::V);
    assert!(harness.requests.is_empty());
    assert!(
        harness
            .panel
            .error()
            .is_some_and(|error| error.contains("entire paste was rejected"))
    );
}

#[test]
fn removing_columns_in_text_discards_obsolete_view_preferences() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(
        b"name,value\nfirst,001\n",
        json!({
            "header":true,
            "sort":{"column":1,"descending":false,"mode":"Text"},
            "filters":[{"column":1,"value":"00","op":"Contains"}]
        }),
    );
    harness.wait_ready();
    harness.documents[0].bytes = b"name\nfirst\n".to_vec().into();
    harness.documents[0].revision = (0, 1);
    harness.frame();
    harness.wait_ready();
    assert!(harness.panel.error().is_none());
    assert_eq!(harness.panel.save_state()["sort"], Value::Null);
    assert_eq!(harness.panel.save_state()["filters"], json!([]));
    assert!(harness.requests.is_empty());
}

#[test]
fn select_all_waits_for_the_latest_document_view() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(b"name,value\nfirst,001\n", json!({"header":true}));
    harness.wait_ready();
    harness.action(PanelAction::SelectAll).unwrap();
    harness.documents[0].bytes = b"name,value\nfirst,001\nsecond,002\nthird,003\n"
        .to_vec()
        .into();
    harness.documents[0].revision = (0, 1);
    harness.action(PanelAction::SelectAll).unwrap();
    harness.wait_ready();
    assert_eq!(
        harness.panel.save_state()["selection"],
        json!({"anchor":[0,0],"focus":[2,1]})
    );
}

#[test]
fn a_rejected_queued_edit_restores_the_draft_and_allows_cancellation() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(b"name,value\nfirst,001\n", json!({"header":true}));
    harness.wait_ready();
    harness.type_replacement("retained draft");
    harness.action(PanelAction::CommitEdit).unwrap();
    let HostRequest::ApplyEditsWithResult { token, .. } = harness.requests[0] else {
        panic!("draft commit needs an acknowledgement");
    };
    harness.documents[0].revision = (0, 1);
    harness.documents[0].bytes = b"name,value\nexternal,001\n".to_vec().into();
    harness
        .panel
        .edit_result(token, Err("Document revision changed".into()));
    assert!(
        harness
            .panel
            .error()
            .is_some_and(|error| error.contains("revision changed"))
    );
    assert!(harness.action(PanelAction::CommitEdit).is_err());
    assert_eq!(harness.requests.len(), 1);
    harness.key(Key::Escape);
    harness.action(PanelAction::CommitEdit).unwrap();
    assert_eq!(harness.requests.len(), 1);
}

#[test]
fn table_draws_one_hundred_thousand_rows_with_bounded_visible_geometry() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut source = String::from("name,value\n");
    for row in 0..100_000 {
        source.push_str(&format!("item{row},{row:06}\n"));
    }
    let mut harness = Harness::new(source.as_bytes(), json!({"header": true}));
    harness.wait_ready();
    let vertices = harness.frame();
    assert!(vertices > 100, "grid did not draw");
    assert!(
        vertices < 100_000,
        "grid rendered offscreen records: {vertices}"
    );
    assert!(harness.requests.is_empty());
}

#[test]
fn unicode_typing_replaces_a_cell_and_commit_is_one_document_transaction() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(
        b"name,value\nfirst,001\nsecond,002\n",
        json!({"header": true}),
    );
    harness.wait_ready();
    harness.type_replacement("élève");
    harness.action(PanelAction::CommitEdit).unwrap();
    assert_eq!(harness.requests.len(), 1);
    assert_eq!(
        harness.apply_requests(),
        "name,value\nélève,001\nsecond,002\n".as_bytes()
    );
}

#[test]
fn external_revision_change_keeps_a_draft_and_blocks_commit_until_cancelled() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(b"name,value\nfirst,001\n", json!({"header": true}));
    harness.wait_ready();
    harness.type_replacement("local draft");
    harness.documents[0].revision = (0, 1);
    harness.documents[0].bytes = b"name,value\nexternal,002\n".to_vec().into();
    assert!(harness.action(PanelAction::CommitEdit).is_err());
    assert!(harness.requests.is_empty());
    harness.key(Key::Escape);
    harness.action(PanelAction::CommitEdit).unwrap();
    assert!(harness.requests.is_empty());
}

#[test]
fn malformed_csv_reports_an_error_without_editing_the_document() {
    let _lock = IMGUI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut harness = Harness::new(
        b"name,value\n\"unterminated,001\n",
        json!({"delimiter": 44}),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.panel.error().is_none() {
        harness.frame();
        assert!(
            Instant::now() < deadline,
            "malformed CSV did not report an error"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!harness.panel.is_ready());
    assert!(harness.requests.is_empty());
}
