//! Built-in features use the same factories, requests and layout format as extensions.
use super::*;
use crate::test_support::TempDir;
use bed_module_explorer as explorer;
use bed_module_projects as projects;
use bed_module_search as search;
use bed_module_settings as settings;
use bed_module_terminal as terminal;
use bed_workbench_api::FileDialogService;

const NATIVE_TOOLS: &[(&str, &str, PanelPlacement)] = &[
    (explorer::PANEL_ID, "explorer", PanelPlacement::Sidebar),
    (search::PANEL_ID, "search", PanelPlacement::Center),
    (projects::PANEL_ID, "projects", PanelPlacement::Center),
    (settings::PANEL_ID, "settings", PanelPlacement::Center),
    (terminal::PANEL_ID, "terminal", PanelPlacement::Bottom),
    (bed_module_debug::PANEL_ID, "debug", PanelPlacement::Bottom),
    (
        bed_module_editor::DIAGNOSTICS_PANEL_TYPE,
        "diagnostics",
        PanelPlacement::Center,
    ),
    (
        bed_module_editor::REFERENCES_PANEL_TYPE,
        "references",
        PanelPlacement::Center,
    ),
    (
        bed_module_editor::LSP_DASHBOARD_PANEL_TYPE,
        "lsp",
        PanelPlacement::Center,
    ),
];

#[derive(Default)]
struct Terminals {
    next: u64,
    shells: HashMap<u64, PathBuf>,
}
impl TerminalService for Terminals {
    fn new_shell(&mut self, cwd: Option<&Path>) -> io::Result<u64> {
        self.next += 1;
        self.shells.insert(self.next, cwd.unwrap().into());
        Ok(self.next)
    }
    fn title(&self, id: u64) -> Option<String> {
        self.shells.contains_key(&id).then(|| format!("Shell {id}"))
    }
    fn working_directory(&self, id: u64) -> Option<PathBuf> {
        self.shells.get(&id).cloned()
    }
    fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
        Err(io::Error::other("This fixture only opens shell panels"))
    }
    fn stop(&mut self, _: u64) {}
    fn release(&mut self, id: u64) {
        self.shells.remove(&id);
    }
}
struct Dialogs;
impl FileDialogService for Dialogs {
    fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<PathBuf> {
        None
    }
}

fn host(directory: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        directory.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    for key in [
        "terminal_visible",
        "treesitter",
        "git_changed_lines",
        "autosave",
    ] {
        settings.settings[key] = json!(false);
    }
    settings.terminal_visible = false;
    let mut host = Workbench::with_settings(settings, crate::builtins::modules);
    while !host.tabs.is_empty() {
        assert!(host.close_tab(0).unwrap());
    }
    host
}

fn command(host: &mut Workbench, id: &str) {
    assert!(host.dispatch_command(id).unwrap());
}

#[test]
fn all_native_panels_are_created_by_the_registered_module_factory() {
    let directory = TempDir::new();
    let root = directory.root().to_str().unwrap();
    let mut runtime = ModuleRuntime::from_composition(crate::builtins::modules());
    let mut documents = EditorSession::new();
    let text = documents.create_document(b"text").unwrap();
    let bytes = documents
        .create_document_with_kind(b"\0\xff", DocumentKind::Bytes)
        .unwrap();
    let mut terminals = Terminals::default();
    let mut dialogs = Dialogs;
    let mut services = ModuleServices {
        documents: &mut documents,
        terminals: &mut terminals,
        dialogs: &mut dialogs,
        project_root: root,
        working_directory: root,
        active_view: None,
        resources: Default::default(),
        settings_ui: None,
    };
    let mut requests = Vec::new();
    for &(kind, legacy, placement) in NATIVE_TOOLS {
        let descriptor = runtime.registry.panel(kind).unwrap();
        assert_eq!(descriptor.legacy_kind, Some(legacy));
        assert_eq!(descriptor.placement, placement);
        let mut panel = runtime
            .create_panel(
                kind,
                bed_workbench_api::PanelInput::None,
                &Value::Null,
                None,
                &mut services,
            )
            .unwrap();
        assert_eq!(panel.kind, kind);
        assert_eq!(panel.instance.attached_document(), None, "{kind}");
        assert_eq!(panel.instance.view_id(), None, "{kind}");
        assert!(
            !panel.instance.title(&runtime.frame.context()).is_empty(),
            "{kind}"
        );
        panel
            .instance
            .close_with_services(&mut services, &mut requests)
            .unwrap();
    }
    for (kind, document) in [
        (bed_module_editor::TEXT_PANEL_TYPE, text),
        (bed_module_editor::HEX_PANEL_TYPE, bytes),
    ] {
        let mut panel = runtime
            .create_panel(
                kind,
                bed_workbench_api::PanelInput::Document(document),
                &Value::Null,
                None,
                &mut services,
            )
            .unwrap();
        assert_eq!(panel.instance.attached_document(), Some(document));
        panel
            .instance
            .close_with_services(&mut services, &mut requests)
            .unwrap();
        assert!(services.documents.snapshot(document).is_ok());
    }
    assert_eq!(runtime.search.panel_count(), 0);
    assert!(terminals.shells.is_empty());
}

#[test]
fn choosing_a_bottom_tool_for_an_existing_area_preserves_geometry() {
    let directory = TempDir::new();
    let mut host = host(&directory);
    host.tiling = crate::workspace::tiling_state::TilingState::default();
    host.dock_built = true;
    let layout = host.tiling.layout.clone();
    assert_eq!(
        host.modules
            .registry
            .panel(bed_module_debug::PANEL_ID)
            .unwrap()
            .open_placement,
        PanelPlacement::Bottom,
    );
    let panel = host
        .open_plugin_panel_in_area(
            bed_module_debug::PANEL_ID,
            bed_workbench_api::PanelInput::None,
            &Value::Null,
            None,
            1,
        )
        .unwrap();

    assert_eq!(host.current_tiling().layout, layout);
    assert_eq!(host.area_for_panel(panel), Some(1));
    assert_eq!(host.tabs.len(), 1);
    host.cleanup().unwrap();
}

#[test]
fn module_show_commands_reuse_panels_and_new_search_preserves_independent_state() {
    let directory = TempDir::new();
    let mut host = host(&directory);
    host.set_project(directory.root()).unwrap();
    command(&mut host, explorer::SHOW_COMMAND);
    let files = host.active_panel_id().unwrap();
    command(&mut host, search::SHOW_COMMAND);
    let first = host.active_panel_id().unwrap();
    host.search_panel(first).unwrap().query = "first".into();
    command(&mut host, search::NEW_COMMAND);
    let second = host.active_panel_id().unwrap();
    host.search_panel(second).unwrap().query = "second".into();
    host.search_panel(second).unwrap().case_sensitive = true;
    assert_ne!(first, second);
    command(&mut host, explorer::SHOW_COMMAND);
    assert_eq!(host.active_panel_id(), Some(files));
    command(&mut host, search::SHOW_COMMAND);
    assert_eq!(host.active_panel_id(), Some(first));
    assert_eq!(host.panel_count("explorer"), 1);
    assert_eq!(host.panel_count("search"), 2);
    assert_eq!(host.search_panel(first).unwrap().query, "first");
    assert_eq!(host.search_panel(second).unwrap().query, "second");
    assert!(host.search_panel(second).unwrap().case_sensitive);
    host.close_tab(host.tabs.iter().position(|tab| tab.id == first).unwrap())
        .unwrap();
    assert_eq!(host.modules.search.panel_count(), 1);
    assert_eq!(host.search_panel(second).unwrap().query, "second");
    assert!(host.session.document_ids().is_empty());
}

#[test]
fn legacy_native_layouts_restore_to_registered_ids_and_preserve_each_search_state() {
    let directory = TempDir::new();
    let mut panels: Vec<_> = NATIVE_TOOLS.iter()
        // Terminal cwd restoration is covered by its scoped-service factory test;
        // this layout test does not launch native child processes.
        .filter(|(_, legacy, _)| *legacy != "terminal")
        .enumerate()
        .map(|(index, &(_, legacy, _))| json!({
            "id": index as u64 + 11,
            "kind": legacy,
            "state": if legacy == "search" { json!({"query":"legacy", "case_sensitive":true}) } else { Value::Null },
        }))
        .collect();
    panels.push(json!({
        "id": 30, "kind": "plugin", "panel_type": search::PANEL_ID,
        "state": {"query":"registered", "include_ignored":true}
    }));
    let mut original = host(&directory);
    original.set_project(&directory.root()).unwrap();
    original
        .restore_workspace(&json!({"version":1, "focused":30, "panels":panels}))
        .unwrap();
    assert!(original.error.is_none(), "{:?}", original.error);
    assert_eq!(original.active_panel_id(), Some(30));
    assert_eq!(original.modules.search.panel_count(), 2);
    assert_eq!(original.search_panel(12).unwrap().query, "legacy");
    assert!(original.search_panel(12).unwrap().case_sensitive);
    assert_eq!(original.search_panel(30).unwrap().query, "registered");
    assert!(original.search_panel(30).unwrap().include_ignored);
    original.persist_workspace().unwrap();
    let saved = original.last_state.clone().unwrap();
    for &(kind, legacy, _) in NATIVE_TOOLS
        .iter()
        .filter(|(_, legacy, _)| *legacy != "terminal" && *legacy != "projects")
    {
        let panel = saved["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|panel| panel["kind"] == legacy)
            .unwrap();
        assert_eq!(panel["panel_type"], kind);
    }
    let mut restored = host(&directory);
    restored.restore_workspace(&saved).unwrap();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.active_panel_id(), Some(30));
    assert_eq!(restored.modules.search.panel_count(), 2);
    assert_eq!(restored.search_panel(12).unwrap().query, "legacy");
    assert!(restored.search_panel(12).unwrap().case_sensitive);
    assert_eq!(restored.search_panel(30).unwrap().query, "registered");
    assert!(restored.search_panel(30).unwrap().include_ignored);
    assert!(
        restored
            .tabs
            .iter()
            .all(|tab| tab.panel.document().is_none())
    );
    assert!(restored.session.document_ids().is_empty());
}

#[test]
fn git_panels_restore_staged_comparisons_without_persisting_snapshot_documents_as_files() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let path = directory.write("project/main.rs", b"fn original() {}\n");
    let root = path.parent().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "--", "main.rs"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "core.hooksPath=.git/hooks",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "Baseline",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(&path, b"fn staged() {}\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "--", "main.rs"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&path, b"fn working() {}\n").unwrap();
    let mut workbench = host(&directory);
    workbench.set_project(root).unwrap();
    workbench.git_smoke_setup("main.rs").unwrap();
    command(&mut workbench, bed_module_git::SHOW_COMMAND);
    command(&mut workbench, bed_module_git::SHOW_COMMAND);
    assert_eq!(workbench.panel_count(bed_module_git::PANEL_ID), 1);
    let staged = workbench
        .tabs
        .iter()
        .position(|tab| {
            tab.panel.kind == bed_module_git::DIFF_PANEL_ID
                && tab.panel.instance.save_state()["side"] == "staged"
        })
        .unwrap();
    workbench.switch_to_tab(staged);
    let mut context = dear_imgui_rs::Context::create();
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
        workbench.tick().unwrap();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
        if workbench.git_smoke_ready().unwrap()
            && workbench
                .session
                .document_ids()
                .iter()
                .any(|id| workbench.session.is_snapshot_document(*id))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Git comparison did not load: {:?}",
            workbench.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let snapshot = workbench
        .session
        .document_ids()
        .into_iter()
        .find(|id| workbench.session.is_snapshot_document(*id))
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(snapshot).unwrap().bytes,
        b"fn staged() {}\n"
    );
    workbench.last_document = Some(snapshot);
    assert!(!workbench.handle_action(HostAction::Save).unwrap());
    assert!(!workbench.handle_action(HostAction::SaveAs).unwrap());
    workbench.persist_workspace().unwrap();
    let saved = workbench.last_state.clone().unwrap();
    let staged_panel = saved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["state"]["side"] == "staged")
        .unwrap();
    assert!(staged_panel["path"].is_null());
    assert_eq!(staged_panel["state"]["path"], "main.rs");
    workbench.cleanup().unwrap();
    drop(workbench);
    drop(context);
    let mut restored = host(&directory);
    restored.set_project(root).unwrap();
    assert_eq!(restored.panel_count(bed_module_git::DIFF_PANEL_ID), 2);
    assert_eq!(restored.panel_count(bed_module_git::PANEL_ID), 1);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(std::fs::read(path).unwrap(), b"fn working() {}\n");
}

#[test]
fn module_document_edits_acknowledge_acceptance_and_reject_stale_revisions() {
    use bed_workbench_api::{
        CommandContext, EditToken, HostContext, Module, ModulePanel, Registrar, Revision,
    };
    use std::{any::Any, cell::RefCell, rc::Rc};
    struct EditModule(Rc<RefCell<Vec<Result<Revision, String>>>>);
    impl Module for EditModule {
        fn id(&self) -> &'static str {
            "fixture.edits"
        }
        fn register(&self, _: &mut Registrar<'_>) {}
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
        ) -> Result<Box<dyn ModulePanel>, String> {
            Err("No panels".into())
        }
        fn edit_result(&mut self, _: EditToken, result: Result<Revision, String>) {
            self.0.borrow_mut().push(result);
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }
    let directory = TempDir::new();
    let path = directory.write("a", b"cat cat");
    let mut host = host(&directory);
    let acknowledgements = Rc::new(RefCell::new(Vec::new()));
    host.modules
        .instances
        .push(Box::new(EditModule(acknowledgements.clone())));
    let document = host.session.open_file(&path).unwrap();
    let revision = host.session.document_revision(document).unwrap();
    for _ in 0..2 {
        host.modules
            .requests
            .push(HostRequest::ApplyModuleEditsWithResult {
                recipient: "fixture.edits".into(),
                token: EditToken::next(),
                document,
                revision,
                edits: vec![bed_document_session::ByteEdit {
                    range: 0..3,
                    bytes: b"dog".to_vec(),
                }],
            });
        host.process_plugin_requests().unwrap();
    }
    assert!(acknowledgements.borrow()[0].is_ok());
    assert!(
        acknowledgements.borrow()[1]
            .as_ref()
            .unwrap_err()
            .contains("Document changed")
    );
    assert_eq!(host.session.snapshot(document).unwrap().bytes, b"dog cat");
    host.session.undo_document(document).unwrap();
    assert_eq!(host.session.snapshot(document).unwrap().bytes, b"cat cat");
    host.cleanup().unwrap();
}
