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
            .create_panel(kind, None, &Value::Null, None, &mut services)
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
            .create_panel(kind, Some(document), &Value::Null, None, &mut services)
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
fn module_show_commands_reuse_panels_and_new_search_preserves_independent_state() {
    let directory = TempDir::new();
    let mut host = host(&directory);
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
        .filter(|(_, legacy, _)| *legacy != "terminal")
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
