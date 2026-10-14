//! Opt-in acceptance of real completion and workspace-changing LSP actions.
use bedtime_documents::{
    ByteEdit, ClosePolicy, DocumentId, DocumentSession, SessionOptions, ViewId,
};
use bedtime_lsp::{lsp_uri::LspUri, workspace_lsp::RoutedLspResponse};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use crate::temp_dir;

fn pump(session: &mut DocumentSession, ready: impl Fn(&DocumentSession) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !ready(session) {
        let report = session.tick();
        assert!(
            report.errors.is_empty(),
            "session errors: {:?}",
            report.errors
        );
        assert!(Instant::now() < deadline, "rust-analyzer timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn request(
    session: &mut DocumentSession,
    document: DocumentId,
    view: ViewId,
    method: &str,
    params: Value,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let received: Rc<RefCell<Option<RoutedLspResponse>>> = Rc::new(RefCell::new(None));
        let callback = received.clone();
        let generation = session.snapshot(document).unwrap().generation;
        let origin = session
            .lsp()
            .unwrap()
            .request_origin(document, view, generation, 0)
            .unwrap();
        session
            .lsp_mut()
            .unwrap()
            .send_request(origin, method, params.clone(), move |reply| {
                *callback.borrow_mut() = Some(reply)
            })
            .unwrap();
        pump(session, |_| received.borrow().is_some());
        let response = received.borrow_mut().take().unwrap();
        assert!(session.accepts_lsp_origin(&response.origin));
        match response.result {
            Ok(value) => return value,
            Err(error) if matches!(error.code, -32800 | -32801) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
                session.tick();
            }
            Err(error) => panic!("{method}: {error}"),
        }
    }
}

fn replace(session: &mut DocumentSession, document: DocumentId, bytes: &[u8]) {
    let revision = session.document_revision(document).unwrap();
    let len = session.snapshot(document).unwrap().bytes.len();
    session
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..len,
                bytes: bytes.to_vec(),
            }],
        )
        .unwrap();
    session.tick();
}

fn position(source: &str, token: &str, after: bool) -> Value {
    let offset = source.find(token).unwrap() + if after { token.len() } else { 0 };
    let prefix = &source[..offset];
    json!({"line":prefix.bytes().filter(|&b|b==b'\n').count(),"character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()})
}

fn items(value: Value) -> Vec<Value> {
    if value.is_array() {
        value.as_array().unwrap().clone()
    } else {
        value["items"].as_array().cloned().unwrap_or_default()
    }
}

#[test]
#[ignore = "requires installed rust-analyzer and rust-src; edits only a temporary Rust project"]
fn real_completion_import_format_rename_quick_fix_and_workspace_undo() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = temp_dir::TempDir::new();
    dir.write("Cargo.toml", b"[package]\nname='bed_lsp_actions_acceptance'\nversion='0.1.0'\nedition='2024'\n[workspace]\n");
    let helper = dir.write(
        "src/helper.rs",
        b"pub struct Widget;\nimpl Widget { pub fn greet(&self) {} }\n",
    );
    let source = "mod helper;\nfn main() { let item = helper::Widget; item. }\n";
    let path = dir.write("src/main.rs", source.as_bytes());
    let config = dir.write("lsp.json", &serde_json::to_vec(&json!({
        "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust-analyzer"}],
        "language_servers":{"rust-analyzer":{"command":"rust-analyzer"}}
    })).unwrap());
    let mut session = DocumentSession::with_options(SessionOptions {
        project_root: Some(dir.root().into()),
        lsp_config: Some(config),
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&path).unwrap();
    let view = session.create_view(document).unwrap();
    let uri = LspUri::file_uri_from_path(&session.snapshot(document).unwrap().path)
        .unwrap()
        .to_string();
    pump(&mut session, |session| {
        session
            .lsp()
            .unwrap()
            .client_for_document(document)
            .is_some_and(|client| client.borrow().is_initialized())
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let completion = request(
            &mut session,
            document,
            view,
            "textDocument/completion",
            json!({
                "textDocument":{"uri":uri},"position":position(source,"item.",true),"context":{"triggerKind":2,"triggerCharacter":"."}
            }),
        );
        if items(completion).iter().any(|item| {
            item["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("greet"))
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "member completion never included greet"
        );
        thread::sleep(Duration::from_millis(50));
        session.tick();
    }

    let import_source = "mod helper;\nfn main() { let _ = Widg; }\n";
    replace(&mut session, document, import_source.as_bytes());
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut selected = loop {
        let completion = request(
            &mut session,
            document,
            view,
            "textDocument/completion",
            json!({
                "textDocument":{"uri":uri},"position":position(import_source,"Widg",true),"context":{"triggerKind":1}
            }),
        );
        if let Some(item) = items(completion).into_iter().find(|item| {
            item["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("Widget"))
        }) {
            break item;
        }
        assert!(
            Instant::now() < deadline,
            "auto-import completion never included Widget"
        );
        thread::sleep(Duration::from_millis(50));
        session.tick();
    };
    selected = request(
        &mut session,
        document,
        view,
        "completionItem/resolve",
        selected,
    );
    let mut edits = selected["additionalTextEdits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !edits.is_empty(),
        "Widget completion should insert its import: {selected}"
    );
    let mut primary = selected.get("textEdit").cloned().unwrap_or_else(||json!({
        "range":{"start":position(import_source,"Widg",false),"end":position(import_source,"Widg",true)},
        "newText":selected.get("insertText").unwrap_or(&selected["label"])
    }));
    if primary.get("range").is_none() {
        primary["range"] = primary["replace"].clone();
    }
    edits.push(primary);
    let revision = session.document_revision(document).unwrap();
    session
        .apply_lsp_text_edits(document, revision, &json!(edits))
        .unwrap();
    let completed = String::from_utf8(session.snapshot(document).unwrap().bytes).unwrap();
    assert!(
        completed.contains("use ") && completed.contains("Widget"),
        "{completed}"
    );
    session.undo_document(document).unwrap();
    assert_eq!(
        session.snapshot(document).unwrap().bytes,
        import_source.as_bytes()
    );

    let format_source = "mod helper;fn main( ){let _=helper::Widget;}\n";
    replace(&mut session, document, format_source.as_bytes());
    let formatting = request(
        &mut session,
        document,
        view,
        "textDocument/formatting",
        json!({
            "textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}
        }),
    );
    let revision = session.document_revision(document).unwrap();
    session
        .apply_lsp_text_edits(document, revision, &formatting)
        .unwrap();
    let formatted = String::from_utf8(session.snapshot(document).unwrap().bytes).unwrap();
    assert!(formatted.contains("fn main() {"), "{formatted}");
    session.tick();
    let rename_position = position(&formatted, "Widget", false);
    let prepared = request(
        &mut session,
        document,
        view,
        "textDocument/prepareRename",
        json!({"textDocument":{"uri":uri},"position":rename_position}),
    );
    assert!(!prepared.is_null());
    let rename = request(
        &mut session,
        document,
        view,
        "textDocument/rename",
        json!({"textDocument":{"uri":uri},"position":rename_position,"newName":"RenamedWidget"}),
    );
    let documents = session.apply_lsp_workspace_edit(&rename, None).unwrap();
    assert_eq!(
        documents.len(),
        2,
        "rename should affect main and helper: {rename}"
    );
    assert!(
        fs::read_to_string(&helper)
            .unwrap()
            .contains("RenamedWidget")
    );
    assert!(
        String::from_utf8(session.snapshot(document).unwrap().bytes)
            .unwrap()
            .contains("RenamedWidget")
    );
    session.undo_document(document).unwrap();
    assert_eq!(
        session.snapshot(document).unwrap().bytes,
        formatted.as_bytes()
    );
    assert!(
        fs::read_to_string(&helper)
            .unwrap()
            .contains("pub struct Widget;")
    );
    session.redo_document(document).unwrap();
    assert!(
        fs::read_to_string(&helper)
            .unwrap()
            .contains("RenamedWidget")
    );
    session.undo_document(document).unwrap();

    let fix_source = "mod helper;\nfn main() { let _ = Widget; }\n";
    replace(&mut session, document, fix_source.as_bytes());
    let deadline = Instant::now() + Duration::from_secs(90);
    let fix = loop {
        let fixes = request(
            &mut session,
            document,
            view,
            "textDocument/codeAction",
            json!({
                "textDocument":{"uri":uri},"range":{"start":position(fix_source,"Widget",false),"end":position(fix_source,"Widget",true)},
                "context":{"diagnostics":[],"only":["quickfix"]}
            }),
        );
        if let Some(fix) = fixes
            .as_array()
            .into_iter()
            .flatten()
            .find(|fix| {
                fix["title"]
                    .as_str()
                    .is_some_and(|title| title.to_lowercase().contains("import"))
            })
            .cloned()
        {
            break fix;
        }
        assert!(
            Instant::now() < deadline,
            "quick fixes never included import Widget: {fixes}"
        );
        thread::sleep(Duration::from_millis(50));
        session.tick();
    };
    let fix = if fix.get("edit").is_some() {
        fix
    } else {
        request(&mut session, document, view, "codeAction/resolve", fix)
    };
    session
        .apply_lsp_workspace_edit(&fix["edit"], None)
        .unwrap();
    let fixed = String::from_utf8(session.snapshot(document).unwrap().bytes).unwrap();
    assert!(fixed.contains("use "), "{fixed}");
    session.undo_document(document).unwrap();
    assert_eq!(
        session.snapshot(document).unwrap().bytes,
        fix_source.as_bytes()
    );
    session.shutdown(ClosePolicy::Discard).unwrap();
}
