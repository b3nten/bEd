use super::*;
use crate::test_support::TempDir;
use bed_document_session::editor_session::ByteEdit;
#[cfg(unix)]
use bed_remote::RemoteClient;
use bed_workbench_api::{EditToken, PanelAction, Revision};
use std::{any::Any, cell::RefCell, fs, rc::Rc};

fn host(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.settings["terminal_visible"] = json!(false);
    settings.terminal_visible = false;
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    settings.settings["autosave"] = json!(false);
    Workbench::with_settings(settings, crate::builtins::modules)
}

fn edit(
    workbench: &mut Workbench,
    document: DocumentId,
    range: std::ops::Range<usize>,
    bytes: &[u8],
) {
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range,
                bytes: bytes.to_vec(),
            }],
        )
        .unwrap();
}

#[test]
fn csv_and_tsv_route_to_the_table_viewer_as_text_documents() {
    let dir = TempDir::new();
    let mut workbench = host(&dir);
    for (name, bytes) in [
        ("project/data.CsV", b"name,value\nfirst,001\n".as_slice()),
        ("project/data.TSV", b"name\tvalue\nfirst\t001\n".as_slice()),
    ] {
        let path = dir.write(name, bytes);
        workbench.open_or_focus(&path).unwrap();
        let document = workbench.active_document().unwrap();
        assert_eq!(
            workbench.session.document_kind(document).unwrap(),
            DocumentKind::Text
        );
        let panel = &workbench.tabs.last().unwrap().panel;
        assert_eq!(panel.kind, "bed.csv.panel");
        assert_eq!(panel.viewer.as_deref(), Some("bed.csv.viewer"));
        assert_eq!(workbench.session.snapshot(document).unwrap().bytes, bytes);
        let count = workbench.tabs.len();
        workbench.open_or_focus(&path).unwrap();
        assert_eq!(workbench.tabs.len(), count);
        assert_eq!(workbench.active_document(), Some(document));
    }
}

#[test]
fn csv_and_text_views_share_document_edits_and_undo_without_saving_or_reopening() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/data.csv", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let csv_panel = workbench.tabs.last().unwrap().id;
    let revision = workbench.session.document_revision(document).unwrap();
    workbench.modules.requests.push(HostRequest::ApplyEdits {
        document,
        revision,
        edits: vec![ByteEdit {
            range: 17..20,
            bytes: b"002".to_vec(),
        }],
    });
    workbench.process_plugin_requests().unwrap();
    workbench
        .open_file_with_viewer(&path, Some("bed.text"), false)
        .unwrap();
    assert_eq!(workbench.active_document(), Some(document));
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,002\n"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
    edit(&mut workbench, document, 11..16, b"other");
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == csv_panel)
        .unwrap();
    workbench.switch_to_tab(index);
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"name,value\nother,001\n"
    );
    workbench
        .modules
        .requests
        .push(HostRequest::Undo { document });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
}

#[test]
fn csv_workspace_restores_view_settings_and_a_shared_text_view() {
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"name,value\nfirst,001\n");
    let mut first = host(&dir);
    first.set_project(&dir.path("project")).unwrap();
    first.open_or_focus(&path).unwrap();
    let document = first.active_document().unwrap();
    let original = first.tabs.last().unwrap().id;
    first
        .open_file_with_viewer(&path, Some("bed.text"), true)
        .unwrap();
    let index = first
        .tabs
        .iter()
        .position(|tab| tab.id == original)
        .unwrap();
    first.close_tab(index).unwrap();
    first
        .add_document_panel(
            document,
            Some("bed.csv.viewer"),
            &json!({
                "delimiter": 44, "header": true, "global_filter": "first",
                "widths": [240.0, 100.0], "scroll": [15.0, 20.0]
            }),
        )
        .unwrap();
    assert_eq!(first.tabs.last().unwrap().panel.kind, "bed.csv.panel");
    let expected = first.tabs.last().unwrap().panel.instance.save_state();
    assert_eq!(expected["delimiter"], 44);
    assert_eq!(expected["header"], true);
    assert_eq!(expected["global_filter"], "first");
    assert_eq!(expected["widths"], json!([240.0, 100.0]));
    assert_eq!(expected["scroll"], json!([15.0, 20.0]));
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    let documents: Vec<_> = second
        .tabs
        .iter()
        .filter_map(|tab| tab.panel.document())
        .collect();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0], documents[1]);
    assert_eq!(
        second.session.document_kind(documents[0]).unwrap(),
        DocumentKind::Text
    );
    let restored = second
        .tabs
        .iter()
        .map(|tab| &tab.panel)
        .find(|panel| panel.viewer.as_deref() == Some("bed.csv.viewer"))
        .unwrap();
    assert_eq!(restored.instance.save_state(), expected);
}

#[test]
fn csv_host_honors_select_all_requested_before_the_table_finishes_parsing() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut bytes = String::from("name,value\n");
    for row in 0..100 {
        use std::fmt::Write;
        writeln!(bytes, "row-{row},{row}").unwrap();
    }
    let path = dir.write("project/large.csv", bytes.as_bytes());
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let panel_id = workbench.active_panel_id().unwrap();
    assert!(
        workbench
            .focused_plugin_action(PanelAction::SelectAll)
            .unwrap()
    );
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Floating)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
        let panel = workbench
            .tabs
            .iter()
            .find(|tab| tab.id == panel_id)
            .and_then(|tab| {
                tab.panel
                    .instance
                    .as_any()
                    .downcast_ref::<bed_plugin_csv::CsvPanel>()
            })
            .unwrap();
        if panel.is_ready() {
            let state = panel.save_state();
            assert_eq!(
                state["selection"],
                json!({ "anchor": [0, 0], "focus": [99, 1] })
            );
            assert_eq!(panel.error(), None);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "CSV table did not become ready: {:?}",
            panel.error()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(workbench.active_snapshot().unwrap().bytes, bytes.as_bytes());
    workbench.cleanup().unwrap();
}

struct Draft {
    revision: Revision,
    range: std::ops::Range<usize>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct ActionState {
    actions: Vec<PanelAction>,
    draft: Option<Draft>,
    closes: usize,
    removals: Vec<String>,
    pending_token: Option<EditToken>,
    acknowledgements: Vec<(EditToken, Result<Revision, String>)>,
    save_results: Vec<(
        bed_workbench_api::SaveToken,
        Result<Vec<bed_workbench_api::SavedDocument>, String>,
    )>,
}

struct ActionPlugin {
    shared: Rc<RefCell<ActionState>>,
    id: &'static str,
    panel: &'static str,
}
struct ActionPanel {
    shared: Rc<RefCell<ActionState>>,
    document: DocumentId,
}

impl Plugin for ActionPlugin {
    fn save_result(
        &mut self,
        token: bed_workbench_api::SaveToken,
        result: Result<Vec<bed_workbench_api::SavedDocument>, String>,
    ) {
        self.shared.borrow_mut().save_results.push((token, result));
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn register(&self, registrar: &mut bed_workbench_api::Registrar<'_>) {
        registrar.panel(self.panel, "Draft table");
    }
    fn command(
        &mut self,
        _: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        _: &mut Vec<HostRequest>,
    ) {
    }
    fn create_panel(
        &mut self,
        _: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn PluginPanel>, String> {
        Ok(Box::new(ActionPanel {
            shared: Rc::clone(&self.shared),
            document: document.ok_or("Document required")?,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl PluginPanel for ActionPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Draft table".into()
    }
    fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {}
    fn action(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        let mut state = self.shared.borrow_mut();
        state.actions.push(action);
        if action == PanelAction::CommitEdit
            && let Some(draft) = &state.draft
        {
            if host
                .document(self.document)
                .ok_or("Document unavailable")?
                .revision
                != draft.revision
            {
                return Err("Draft is stale; restart editing against the current document".into());
            }
            let edits = vec![ByteEdit {
                range: draft.range.clone(),
                bytes: draft.bytes.clone(),
            }];
            if let Some(token) = state.pending_token {
                requests.push(HostRequest::ApplyEditsWithResult {
                    token,
                    document: self.document,
                    revision: draft.revision,
                    edits,
                });
            } else {
                requests.push(HostRequest::ApplyEdits {
                    document: self.document,
                    revision: draft.revision,
                    edits,
                });
                state.draft = None;
            }
        }
        Ok(true)
    }
    fn edit_result(&mut self, token: EditToken, result: Result<Revision, String>) {
        let mut state = self.shared.borrow_mut();
        if state.pending_token == Some(token) {
            state.pending_token = None;
            if result.is_ok() {
                state.draft = None;
            }
        }
        state.acknowledgements.push((token, result));
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.shared.borrow_mut().closes += 1;
    }
    fn document_removed_with_services(
        &mut self,
        document: DocumentId,
        path: &str,
        _: &mut bed_workbench_api::ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        assert_eq!(document, self.document);
        self.shared.borrow_mut().removals.push(path.to_owned());
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn attach_probe(
    workbench: &mut Workbench,
    document: DocumentId,
) -> (Rc<RefCell<ActionState>>, u64) {
    attach_named_probe(
        workbench,
        document,
        "test.csv_actions",
        "test.csv_actions.panel",
    )
}

fn attach_named_probe(
    workbench: &mut Workbench,
    document: DocumentId,
    id: &'static str,
    panel: &'static str,
) -> (Rc<RefCell<ActionState>>, u64) {
    let shared = Rc::new(RefCell::new(ActionState::default()));
    let plugin = ActionPlugin {
        shared: Rc::clone(&shared),
        id,
        panel,
    };
    workbench.modules.registry.register(&plugin).unwrap();
    workbench.modules.instances.push(Box::new(plugin));
    let panel = workbench
        .open_plugin_panel(panel, Some(document), &Value::Null, None)
        .unwrap();
    (shared, panel)
}

fn draft(workbench: &Workbench, shared: &Rc<RefCell<ActionState>>, document: DocumentId) {
    shared.borrow_mut().draft = Some(Draft {
        revision: workbench.session.document_revision(document).unwrap(),
        range: 0..4,
        bytes: b"changed".to_vec(),
    });
}

#[test]
fn deletion_commits_panel_drafts_then_preserves_the_document_and_undo() {
    let dir = TempDir::new();
    let path = dir
        .write("project/source.txt", b"name,value\nfirst,001\n")
        .canonicalize()
        .unwrap();
    let mut workbench = host(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    workbench
        .trash_path_with(&path, |path| fs::remove_file(path))
        .unwrap();
    let snapshot = workbench.session.snapshot(document).unwrap();
    assert_eq!(snapshot.bytes, b"changed,value\nfirst,001\n");
    assert!(snapshot.path.is_empty());
    assert!(snapshot.disk_conflict.is_none());
    assert!(shared.borrow().draft.is_none());
    assert_eq!(shared.borrow().removals, [path.to_string_lossy()]);
    assert_eq!(shared.borrow().closes, 0);
    assert!(workbench.tabs.iter().any(|tab| tab.id == panel));
    workbench.session.undo_document(document).unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,001\n"
    );
    assert!(!path.exists());
}

#[test]
fn deletion_preserves_a_clean_buffer_with_an_invalid_uncommitted_draft() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\n");
    let mut workbench = host(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    shared.borrow_mut().draft.as_mut().unwrap().revision.0 += 1;
    workbench
        .trash_path_with(&path, |path| fs::remove_file(path))
        .unwrap();
    let snapshot = workbench.session.snapshot(document).unwrap();
    assert!(snapshot.path.is_empty());
    assert!(snapshot.dirty);
    assert_eq!(snapshot.bytes, b"name,value\n");
    assert!(shared.borrow().draft.is_some());
    assert_eq!(shared.borrow().closes, 0);
    assert!(workbench.tabs.iter().any(|tab| tab.id == panel));
    assert!(
        workbench
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Draft is stale"))
    );
}

#[test]
fn clean_external_deletion_notifies_panels_before_closing_them_once() {
    let dir = TempDir::new();
    let path = dir
        .write("project/source.txt", b"name,value\n")
        .canonicalize()
        .unwrap();
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    fs::remove_file(&path).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while workbench.session.snapshot(document).is_ok() {
        workbench.tick().unwrap();
        assert!(
            Instant::now() < until,
            "deleted clean document did not close"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(workbench.session.snapshot(document).is_err());
    assert_eq!(shared.borrow().removals, [path.to_string_lossy()]);
    assert_eq!(shared.borrow().closes, 1);
    workbench.tick().unwrap();
    assert_eq!(shared.borrow().closes, 1);
}

#[test]
fn focused_find_and_select_all_target_each_panel_through_the_same_contract() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let text = workbench.tabs.last().unwrap().id;
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    workbench.dispatch(WindowCommand::Find).unwrap();
    assert!(
        workbench
            .focused_plugin_action(PanelAction::SelectAll)
            .unwrap()
    );
    assert_eq!(
        shared.borrow().actions,
        [PanelAction::Find, PanelAction::SelectAll]
    );
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == text)
        .unwrap();
    workbench.switch_to_tab(index);
    assert!(
        workbench
            .focused_plugin_action(PanelAction::SelectAll)
            .unwrap()
    );
    let view = workbench.active_view().unwrap();
    assert_eq!(
        workbench
            .session
            .view_snapshot(view)
            .unwrap()
            .primary()
            .ordered(),
        (0, 0, 2, 0)
    );
    workbench.dispatch(WindowCommand::Find).unwrap();
    assert_eq!(workbench.active_overlay(), Overlay::Find);
    assert_eq!(shared.borrow().actions.len(), 2);
}

#[test]
fn saving_text_commits_pending_edits_from_its_attached_table_first() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let text = workbench.tabs.last().unwrap().id;
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == text)
        .unwrap();
    workbench.switch_to_tab(index);
    assert!(workbench.handle_action(HostAction::Save).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"changed,value\nfirst,001\n");
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    assert!(shared.borrow().draft.is_none());
    assert_eq!(shared.borrow().actions, [PanelAction::CommitEdit]);
}

#[test]
fn a_queued_save_commits_pending_edits_from_attached_tables() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    workbench
        .modules
        .requests
        .push(HostRequest::Save { document });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"changed,value\nfirst,001\n");
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    assert!(shared.borrow().draft.is_none());
    assert_eq!(shared.borrow().actions, [PanelAction::CommitEdit]);
}

#[test]
fn tokenized_save_flushes_clean_panel_drafts_and_acknowledges_the_saved_revision() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    let token = bed_workbench_api::SaveToken::next();
    workbench
        .modules
        .requests
        .push(HostRequest::SaveDocumentsWithResult {
            recipient: "test.csv_actions".into(),
            token,
            documents: vec![document, document],
        });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"changed,value\n");
    let state = shared.borrow();
    assert_eq!(state.save_results.len(), 1);
    assert_eq!(state.save_results[0].0, token);
    let saved = state.save_results[0].1.as_ref().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].document, document);
    assert_eq!(
        saved[0].revision,
        workbench.session.document_revision(document).unwrap()
    );
    assert_eq!(state.actions, [PanelAction::CommitEdit]);
}

#[test]
fn rejected_panel_draft_reports_save_failure_without_writing_the_file() {
    let dir = TempDir::new();
    let original = b"name,value\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    shared.borrow_mut().draft.as_mut().unwrap().revision.1 += 1;
    workbench
        .modules
        .requests
        .push(HostRequest::SaveDocumentsWithResult {
            recipient: "test.csv_actions".into(),
            token: bed_workbench_api::SaveToken::next(),
            documents: vec![document],
        });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(shared.borrow().draft.is_some());
    assert!(shared.borrow().save_results[0].1.is_err());
}

#[cfg(unix)]
fn remote_save_probe(
    dir: &TempDir,
    path: &Path,
) -> (Workbench, DocumentId, Rc<RefCell<ActionState>>) {
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bed-headless");
    let client = if helper.is_file() {
        RemoteClient::launch_local(helper).unwrap()
    } else {
        let mut command = std::process::Command::new(env!("CARGO"));
        command
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .args([
                "run",
                "--quiet",
                "--offline",
                "-p",
                "bed-headless",
                "--",
                "--stdio",
            ]);
        RemoteClient::launch_command(command).unwrap()
    };
    let mut workbench = host(dir);
    workbench.session = EditorSession::with_remote_options(
        bed_document_session::SessionOptions {
            monitoring: false,
            ..Default::default()
        },
        bed_remote::SshTarget::new("fixture"),
        client,
        path.parent().unwrap().to_str().unwrap().into(),
    )
    .unwrap();
    workbench.session.request_open_file(path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.session.open_pending(path) {
        workbench.session.tick();
        assert!(Instant::now() < deadline, "remote file open timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    let document = workbench.session.document_for_path(path).unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    (workbench, document, shared)
}

#[cfg(unix)]
fn await_save_result(workbench: &mut Workbench, shared: &Rc<RefCell<ActionState>>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while shared.borrow().save_results.is_empty() {
        let report = workbench.session.tick();
        workbench.poll_module_saves(&report);
        assert!(
            Instant::now() < deadline,
            "remote save acknowledgement timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(unix)]
#[test]
fn tokenized_remote_save_waits_for_acknowledgement_after_flushing_drafts() {
    let dir = TempDir::new();
    let path = dir
        .write("project/source.txt", b"name,value\n")
        .canonicalize()
        .unwrap();
    let (mut workbench, document, shared) = remote_save_probe(&dir, &path);
    workbench.begin_module_save(
        "test.csv_actions".into(),
        bed_workbench_api::SaveToken::next(),
        vec![document],
    );
    assert!(workbench.session.save_pending(document));
    assert!(shared.borrow().save_results.is_empty());
    assert!(shared.borrow().draft.is_none());
    await_save_result(&mut workbench, &shared);
    assert!(shared.borrow().save_results[0].1.is_ok());
    assert_eq!(fs::read(path).unwrap(), b"changed,value\n");
}

#[cfg(unix)]
#[test]
fn tokenized_remote_save_rejects_edits_after_the_requested_revision() {
    let dir = TempDir::new();
    let path = dir
        .write("project/source.txt", b"name,value\n")
        .canonicalize()
        .unwrap();
    let (mut workbench, document, shared) = remote_save_probe(&dir, &path);
    workbench.begin_module_save(
        "test.csv_actions".into(),
        bed_workbench_api::SaveToken::next(),
        vec![document],
    );
    edit(&mut workbench, document, 0..7, b"newer");
    await_save_result(&mut workbench, &shared);
    assert!(
        shared.borrow().save_results[0]
            .1
            .as_ref()
            .unwrap_err()
            .contains("changed while saving")
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
}

#[cfg(unix)]
#[test]
fn tokenized_remote_save_reports_target_write_conflicts() {
    let dir = TempDir::new();
    let path = dir
        .write("project/source.txt", b"name,value\n")
        .canonicalize()
        .unwrap();
    let (mut workbench, document, shared) = remote_save_probe(&dir, &path);
    fs::write(&path, b"external\n").unwrap();
    workbench.begin_module_save(
        "test.csv_actions".into(),
        bed_workbench_api::SaveToken::next(),
        vec![document],
    );
    await_save_result(&mut workbench, &shared);
    assert!(shared.borrow().save_results[0].1.is_err());
    assert_eq!(fs::read(path).unwrap(), b"external\n");
    assert!(workbench.session.snapshot(document).unwrap().dirty);
}

#[test]
fn closing_a_table_commits_its_draft_into_the_remaining_text_view() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panel)
        .unwrap();
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(shared.borrow().closes, 1);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"changed,value\nfirst,001\n"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        original
    );
}

#[test]
fn closing_the_last_table_commits_and_saves_its_draft() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let text = workbench.tabs.last().unwrap().id;
    let (shared, panel) = attach_probe(&mut workbench, document);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == text)
        .unwrap();
    workbench.close_tab(index).unwrap();
    draft(&workbench, &shared, document);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panel)
        .unwrap();
    assert!(workbench.close_tab(index).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"changed,value\nfirst,001\n");
    assert!(workbench.session.snapshot(document).is_err());
    assert_eq!(shared.borrow().closes, 1);
}

#[test]
fn stale_table_drafts_block_save_and_close_and_remain_available() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    edit(&mut workbench, document, 17..20, b"002");
    let panels: Vec<_> = workbench.tabs.iter().map(|tab| tab.id).collect();
    assert!(workbench.handle_action(HostAction::Save).is_err());
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panel)
        .unwrap();
    assert!(workbench.close_tab(index).is_err());
    assert!(workbench.request_close_all().is_err());
    assert_eq!(
        workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        panels
    );
    assert!(shared.borrow().draft.is_some());
    assert_eq!(shared.borrow().closes, 0);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,002\n"
    );
}

#[test]
fn keyboard_save_shows_a_stale_draft_error_and_keeps_the_frame_and_panel_open() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    edit(&mut workbench, document, 17..20, b"002");
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Floating)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for _ in 0..2 {
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
    }
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panel)
        .unwrap();
    workbench.switch_to_tab(index);
    context.io_mut().add_key_event(Key::ModCtrl, true);
    context.io_mut().add_key_event(Key::S, true);
    context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
        [1200.0, 800.0],
        1.0 / 60.0,
    ));
    workbench.render(context.frame()).unwrap();
    drop(context.render_legacy());
    assert!(
        workbench
            .error
            .as_deref()
            .is_some_and(|error| error.contains("stale"))
    );
    assert!(workbench.tabs.iter().any(|tab| tab.id == panel));
    assert!(shared.borrow().draft.is_some());
    assert_eq!(shared.borrow().closes, 0);
    assert_eq!(fs::read(&path).unwrap(), original);
    context.io_mut().add_key_event(Key::S, false);
    context.io_mut().add_key_event(Key::ModCtrl, false);
    shared.borrow_mut().draft = None;
    workbench.cleanup().unwrap();
}

#[test]
fn rejected_table_edit_prevents_a_following_queued_save_of_the_same_document() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/data.csv", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let revision = workbench.session.document_revision(document).unwrap();
    edit(&mut workbench, document, 17..20, b"002");
    let other_path = dir.write("project/other.txt", b"before");
    workbench.open_or_focus(&other_path).unwrap();
    let other_document = workbench.active_document().unwrap();
    edit(&mut workbench, other_document, 0..6, b"after");
    workbench.modules.requests.extend([
        HostRequest::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 17..20,
                bytes: b"003".to_vec(),
            }],
        },
        HostRequest::Save { document },
        HostRequest::Save {
            document: other_document,
        },
    ]);
    workbench.process_plugin_requests().unwrap();
    assert!(workbench.error.is_some());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,002\n"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(fs::read(&other_path).unwrap(), b"after");
    assert!(!workbench.session.snapshot(other_document).unwrap().dirty);
}

#[test]
fn saving_acknowledges_a_tokenized_table_commit_with_its_actual_document_revision() {
    let dir = TempDir::new();
    let path = dir.write("project/source.txt", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    let before = workbench.session.document_revision(document).unwrap();
    let token = EditToken::next();
    shared.borrow_mut().pending_token = Some(token);
    assert!(workbench.handle_action(HostAction::Save).unwrap());
    let applied = workbench.session.document_revision(document).unwrap();
    assert_ne!(applied, before);
    assert_eq!(shared.borrow().acknowledgements, [(token, Ok(applied))]);
    assert!(shared.borrow().draft.is_none());
    assert!(shared.borrow().pending_token.is_none());
    assert_eq!(fs::read(&path).unwrap(), b"changed,value\nfirst,001\n");
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,001\n"
    );
}

#[test]
fn a_rejected_tokenized_edit_acknowledges_the_error_retains_the_draft_and_blocks_save() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, _) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    let revision = workbench.session.document_revision(document).unwrap();
    edit(&mut workbench, document, 17..20, b"002");
    let current = workbench.session.document_revision(document).unwrap();
    let edits = vec![ByteEdit {
        range: 0..4,
        bytes: b"changed".to_vec(),
    }];
    let expected_error = workbench
        .session
        .apply_edits(document, revision, &edits)
        .unwrap_err()
        .to_string();
    let token = EditToken::next();
    shared.borrow_mut().pending_token = Some(token);
    workbench.modules.requests.extend([
        HostRequest::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        },
        HostRequest::Save { document },
    ]);
    workbench.process_plugin_requests().unwrap();
    assert_eq!(
        shared.borrow().acknowledgements,
        [(token, Err(expected_error))]
    );
    assert!(shared.borrow().draft.is_some());
    assert!(shared.borrow().pending_token.is_none());
    assert_eq!(
        workbench.session.document_revision(document).unwrap(),
        current
    );
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"name,value\nfirst,002\n"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn a_rejected_tokenized_close_commit_preserves_the_panel_and_its_draft() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/source.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let (shared, panel) = attach_probe(&mut workbench, document);
    draft(&workbench, &shared, document);
    let revision = workbench.session.document_revision(document).unwrap();
    let token = EditToken::next();
    {
        let mut state = shared.borrow_mut();
        state.pending_token = Some(token);
        state.draft.as_mut().unwrap().range = 0..original.len() + 1;
    }
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == panel)
        .unwrap();
    let error = workbench.close_tab(index).unwrap_err().to_string();
    assert_eq!(shared.borrow().acknowledgements, [(token, Err(error))]);
    assert!(shared.borrow().draft.is_some());
    assert_eq!(shared.borrow().closes, 0);
    assert!(workbench.tabs.iter().any(|tab| tab.id == panel));
    assert_eq!(
        workbench.session.document_revision(document).unwrap(),
        revision
    );
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        original
    );
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn strict_batches_acknowledge_later_edits_after_an_earlier_token_is_rejected() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let stale_path = dir.write("project/stale.txt", original);
    let valid_path = dir.write("project/valid.txt", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&stale_path).unwrap();
    let stale_document = workbench.active_document().unwrap();
    let (stale_state, _) = attach_probe(&mut workbench, stale_document);
    draft(&workbench, &stale_state, stale_document);
    let stale_revision = workbench.session.document_revision(stale_document).unwrap();
    edit(&mut workbench, stale_document, 17..20, b"002");
    let stale_current = workbench.session.document_revision(stale_document).unwrap();
    workbench.open_or_focus(&valid_path).unwrap();
    let valid_document = workbench.active_document().unwrap();
    let (valid_state, _) = attach_named_probe(
        &mut workbench,
        valid_document,
        "test.csv_actions.other",
        "test.csv_actions.other.panel",
    );
    draft(&workbench, &valid_state, valid_document);
    let valid_revision = workbench.session.document_revision(valid_document).unwrap();
    let stale_token = EditToken::next();
    let valid_token = EditToken::next();
    stale_state.borrow_mut().pending_token = Some(stale_token);
    valid_state.borrow_mut().pending_token = Some(valid_token);
    let edits = vec![ByteEdit {
        range: 0..4,
        bytes: b"changed".to_vec(),
    }];
    workbench.modules.requests.extend([
        HostRequest::ApplyEditsWithResult {
            token: stale_token,
            document: stale_document,
            revision: stale_revision,
            edits: edits.clone(),
        },
        HostRequest::Save {
            document: stale_document,
        },
        HostRequest::ApplyEditsWithResult {
            token: valid_token,
            document: valid_document,
            revision: valid_revision,
            edits,
        },
        HostRequest::Save {
            document: valid_document,
        },
    ]);
    let error = workbench
        .process_plugin_requests_inner(true)
        .unwrap_err()
        .to_string();
    let applied = workbench.session.document_revision(valid_document).unwrap();
    assert_eq!(
        stale_state.borrow().acknowledgements,
        [(stale_token, Err(error))]
    );
    assert_eq!(
        valid_state.borrow().acknowledgements,
        [(valid_token, Ok(applied))]
    );
    assert!(stale_state.borrow().draft.is_some());
    assert!(valid_state.borrow().draft.is_none());
    assert_eq!(
        workbench.session.document_revision(stale_document).unwrap(),
        stale_current
    );
    assert_eq!(
        workbench.session.snapshot(stale_document).unwrap().bytes,
        b"name,value\nfirst,002\n"
    );
    assert_eq!(fs::read(&stale_path).unwrap(), original);
    assert_eq!(
        fs::read(&valid_path).unwrap(),
        b"changed,value\nfirst,001\n"
    );
}

#[test]
fn viewer_preferences_override_baselines_and_explicit_opens_override_preferences() {
    let dir = TempDir::new();
    let csv = dir.write("project/data.CsV", b"name,value\nfirst,001\n");
    let svg = dir.write(
        "project/drawing.SVG",
        br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
    );
    let misleading = dir.write("project/text.png", b"plain text");
    let mut workbench = host(&dir);
    for (path, expected) in [
        (&csv, bed_plugin_csv::VIEWER_ID),
        (&svg, bed_plugin_image::VIEWER_ID),
    ] {
        workbench.open_or_focus(path).unwrap();
        assert_eq!(
            workbench.tabs.last().unwrap().panel.viewer.as_deref(),
            Some(expected)
        );
        assert_eq!(
            workbench.active_snapshot().unwrap().kind,
            DocumentKind::Text
        );
    }
    workbench.open_or_focus(&misleading).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    workbench.settings.settings["default_viewers"] = json!({});
    let fresh = dir.write("project/fresh.csv", b"name,value\nfirst,001\n");
    workbench.open_or_focus(&fresh).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    // Preferences affect new opens, never an existing presentation.
    workbench.open_or_focus(&csv).unwrap();
    assert_eq!(
        workbench.tabs[workbench.active_index()]
            .panel
            .viewer
            .as_deref(),
        Some(bed_plugin_csv::VIEWER_ID)
    );
    workbench.settings.settings["default_viewers"] = json!({"csv":"bed.text"});
    workbench
        .open_file_with_viewer(&fresh, Some(bed_plugin_csv::VIEWER_ID), true)
        .unwrap();
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_csv::VIEWER_ID)
    );
    workbench.settings.settings["default_viewers"] = json!({"csv":"uninstalled.viewer"});
    let missing = dir.write("project/missing.csv", b"a,b\n1,2\n");
    workbench.open_or_focus(&missing).unwrap();
    assert!(workbench.tabs.last().unwrap().panel.editor().is_some());
    assert_eq!(
        workbench.settings.settings["default_viewers"]["csv"],
        "uninstalled.viewer"
    );
}

#[test]
fn switching_the_origin_tab_retains_document_history_and_transformed_source_selection() {
    let dir = TempDir::new();
    let original = b"name,value\nfirst,001\n";
    let path = dir.write("project/data.csv", original);
    let mut workbench = host(&dir);
    workbench.settings.settings["default_viewers"] = json!({});
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let view = workbench.tabs.last().unwrap().panel.view_id().unwrap();
    workbench
        .session
        .with_commands(view, |commands| {
            commands.set_cursor(1, 4, false, CursorReveal::Ensure)
        })
        .unwrap();
    workbench.session.set_scroll(view, 7.0, 40.0).unwrap();
    workbench.tabs.last_mut().unwrap().dock_id = 42;
    workbench
        .tabs
        .last_mut()
        .unwrap()
        .panel
        .saved_presentations
        .insert(
            bed_plugin_csv::VIEWER_ID.into(),
            json!({"header":true,"global_filter":"first","widths":[220.0,120.0]}),
        );
    workbench
        .open_file_with_viewer(&path, Some("bed.text"), true)
        .unwrap();
    let sibling = workbench.tabs.last().unwrap().id;
    let count = workbench.tabs.len();
    workbench.modules.requests.push(HostRequest::SwitchViewer {
        tab,
        document,
        viewer: bed_plugin_csv::VIEWER_ID.into(),
    });
    workbench.process_plugin_requests().unwrap();
    assert_eq!(workbench.tabs.len(), count);
    let index = workbench.tabs.iter().position(|t| t.id == tab).unwrap();
    assert_eq!(workbench.tabs[index].dock_id, 42);
    assert_eq!(
        workbench.tabs[index].panel.instance.save_state()["global_filter"],
        "first"
    );
    assert!(
        workbench
            .tabs
            .iter()
            .find(|t| t.id == sibling)
            .unwrap()
            .panel
            .editor()
            .is_some()
    );
    edit(&mut workbench, document, 0..0, b"extra,row\n");
    let transformed = workbench.session.view_snapshot(view).unwrap();
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    assert_eq!(workbench.tabs[index].panel.view_id(), Some(view));
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        transformed.selections
    );
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(fs::read(&path).unwrap(), original);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
    workbench
        .switch_document_viewer(tab, document, bed_plugin_csv::VIEWER_ID)
        .unwrap();
    assert_eq!(
        workbench.tabs[index].panel.instance.save_state()["global_filter"],
        "first"
    );
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    assert_eq!(workbench.tabs[index].panel.view_id(), Some(view));
    workbench.close_tab(index).unwrap();
    assert!(workbench.session.view_snapshot(view).is_err());
    assert!(workbench.session.snapshot(document).is_ok());
}

#[test]
fn svg_switching_shares_unsaved_source_without_saving_or_reopening() {
    let dir = TempDir::new();
    let original = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
    let path = dir.write("project/drawing.svg", original);
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let count = workbench.tabs.len();
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    edit(
        &mut workbench,
        document,
        original.len()..original.len(),
        b"\n<!-- unsaved -->",
    );
    workbench
        .switch_document_viewer(tab, document, bed_plugin_image::VIEWER_ID)
        .unwrap();
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.active_document(), Some(document));
    assert!(workbench.active_snapshot().unwrap().dirty);
    assert!(
        workbench
            .active_snapshot()
            .unwrap()
            .bytes
            .ends_with(b"<!-- unsaved -->")
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    let view = workbench.tabs.last().unwrap().panel.view_id().unwrap();
    workbench.close_tab(workbench.tabs.len() - 1).unwrap();
    assert!(workbench.session.view_snapshot(view).is_err());
    assert!(workbench.session.snapshot(document).is_err());
}

#[test]
fn switching_flushes_drafts_and_keeps_rejected_drafts_in_the_origin_presentation() {
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"name,value\nfirst,001\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let shared = Rc::new(RefCell::new(ActionState::default()));
    workbench.tabs.last_mut().unwrap().panel.instance = Box::new(ActionPanel {
        shared: Rc::clone(&shared),
        document,
    });
    draft(&workbench, &shared, document);
    shared.borrow_mut().draft.as_mut().unwrap().revision.0 += 1;
    assert!(
        workbench
            .switch_document_viewer(tab, document, "bed.text")
            .is_err()
    );
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_csv::VIEWER_ID)
    );
    assert!(shared.borrow().draft.is_some());
    draft(&workbench, &shared, document);
    shared.borrow_mut().pending_token = Some(EditToken::next());
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    assert!(shared.borrow().draft.is_none());
    assert!(
        workbench
            .active_snapshot()
            .unwrap()
            .bytes
            .starts_with(b"changed")
    );
    assert_eq!(shared.borrow().acknowledgements.len(), 1);
    assert_eq!(fs::read(&path).unwrap(), b"name,value\nfirst,001\n");
    workbench.close_tab(workbench.tabs.len() - 1).unwrap();
    assert_eq!(shared.borrow().closes, 1);
}

#[test]
fn workspace_restores_the_active_viewer_and_lazily_restores_inactive_view_state() {
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"name,value\nfirst,001\n");
    let mut first = host(&dir);
    first.set_project(&dir.path("project")).unwrap();
    first.settings.settings["default_viewers"] = json!({});
    first.open_or_focus(&path).unwrap();
    let document = first.active_document().unwrap();
    let tab = first.tabs.last().unwrap().id;
    first
        .tabs
        .last_mut()
        .unwrap()
        .panel
        .saved_presentations
        .insert(
            bed_plugin_csv::VIEWER_ID.into(),
            json!({"header":true,"global_filter":"first","widths":[210.0,110.0]}),
        );
    first
        .switch_document_viewer(tab, document, bed_plugin_csv::VIEWER_ID)
        .unwrap();
    let expected = first.tabs.last().unwrap().panel.instance.save_state();
    first
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    first.persist_workspace().unwrap();
    let mut second = host(&dir);
    second.restore_last_workspace().unwrap();
    let document = second.active_document().unwrap();
    let index = second.tabs.iter().position(|t| t.id == tab).unwrap();
    assert!(second.tabs[index].panel.editor().is_some());
    assert!(second.tabs[index].panel.inactive.is_empty());
    second
        .switch_document_viewer(tab, document, bed_plugin_csv::VIEWER_ID)
        .unwrap();
    assert_eq!(second.tabs[index].panel.instance.save_state(), expected);
}

#[test]
fn stale_or_incompatible_switch_requests_leave_the_existing_tab_untouched() {
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"a,b\n1,2\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let count = workbench.tabs.len();
    workbench
        .switch_document_viewer(tab + 100, document, "bed.text")
        .unwrap();
    workbench
        .switch_document_viewer(tab, DocumentId(document.0 + 100), "bed.text")
        .unwrap();
    for viewer in ["missing.viewer", "bed.hex", bed_plugin_image::VIEWER_ID] {
        assert!(
            workbench
                .switch_document_viewer(tab, document, viewer)
                .is_err()
        );
    }
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.tabs.last().unwrap().id, tab);
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_csv::VIEWER_ID)
    );
}

#[test]
fn a_new_registered_viewer_is_discovered_and_factory_failure_preserves_the_current_tab() {
    struct BrokenViewer;
    impl Plugin for BrokenViewer {
        fn id(&self) -> &'static str {
            "test.broken"
        }
        fn register(&self, registrar: &mut bed_workbench_api::Registrar<'_>) {
            registrar.panel("test.broken.panel", "Broken viewer");
            registrar.viewer(
                "test.broken.viewer",
                "Broken viewer",
                "test.broken.panel",
                &["csv"],
                DocumentKind::Text,
            );
        }
        fn command(
            &mut self,
            _: &str,
            _: &CommandContext,
            _: &HostContext<'_>,
            _: &mut Vec<HostRequest>,
        ) {
        }
        fn create_panel(
            &mut self,
            _: &str,
            _: Option<DocumentId>,
            _: &Value,
        ) -> Result<Box<dyn PluginPanel>, String> {
            Err("Viewer construction failed".into())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }
    let dir = TempDir::new();
    let path = dir.write("project/data.csv", b"a,b\n1,2\n");
    let mut workbench = host(&dir);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let original_state = workbench.tabs.last().unwrap().panel.instance.save_state();
    workbench.modules.registry.register(&BrokenViewer).unwrap();
    workbench.modules.instances.push(Box::new(BrokenViewer));
    assert!(
        workbench
            .modules
            .registry
            .compatible_viewers("data.csv", Some(DocumentKind::Text))
            .iter()
            .any(|v| v.id == "test.broken.viewer")
    );
    edit(&mut workbench, document, 4..5, b"3");
    let count = workbench.tabs.len();
    let error = workbench
        .switch_document_viewer(tab, document, "test.broken.viewer")
        .unwrap_err();
    assert!(error.to_string().contains("construction failed"));
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.tabs.last().unwrap().id, tab);
    assert_eq!(
        workbench.tabs.last().unwrap().panel.instance.save_state(),
        original_state
    );
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"a,b\n3,2\n");
    assert_eq!(fs::read(&path).unwrap(), b"a,b\n1,2\n");
}

#[test]
fn legacy_byte_backed_svg_workspaces_upgrade_unless_a_hex_sibling_needs_bytes() {
    for with_hex in [false, true] {
        let dir = TempDir::new();
        let path = dir.write(
            "project/drawing.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
        );
        let mut first = host(&dir);
        first.set_project(&dir.path("project")).unwrap();
        let document = first
            .session
            .open_file_with_kind(&path, DocumentKind::Bytes)
            .unwrap();
        let tab = first
            .add_document_panel(document, Some(bed_plugin_image::VIEWER_ID), &Value::Null)
            .unwrap();
        if with_hex {
            first
                .add_document_panel(document, Some("bed.hex"), &Value::Null)
                .unwrap();
        }
        first.persist_workspace().unwrap();
        let mut second = host(&dir);
        second.restore_last_workspace().unwrap();
        let document = second.session.document_for_path(&path).unwrap();
        assert_eq!(
            second.session.document_kind(document).unwrap(),
            if with_hex {
                DocumentKind::Bytes
            } else {
                DocumentKind::Text
            }
        );
        assert_eq!(
            second
                .tabs
                .iter()
                .filter(|t| t.panel.document() == Some(document))
                .count(),
            if with_hex { 2 } else { 1 }
        );
        if !with_hex {
            second
                .switch_document_viewer(tab, document, "bed.text")
                .unwrap();
            assert!(
                second
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .unwrap()
                    .panel
                    .editor()
                    .is_some()
            );
            assert_eq!(second.active_document(), Some(document));
        }
    }
}
