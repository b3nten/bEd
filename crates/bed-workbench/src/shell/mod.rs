//! Bed's standalone docked workspace.
//! Documents and services belong to EditorSession; this shell owns panels and
//! layout. See README.md for architecture and LICENSE and NOTICE for attribution.
use crate::workspace::file_actions;
use crate::{
    presentation::ui_animations::UiAnimations,
    workspace::store::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget},
};
use bed_document_session::{
    editor::Editor,
    editor_session::{
        ClosePolicy, DocumentId, DocumentSnapshot, EditorSession, SessionEvent, SessionOptions,
        ViewId,
    },
};
use bed_editing::{
    editor_commands::CursorReveal, editor_events::Overlay, editor_state::DocumentKind,
    editor_view_state::Selection,
};
use bed_editor_ui::editor_input::HostAction;
use bed_module_explorer::{file_tree::FileTreeAction, files::FileExplorer};
use bed_settings::Settings;
use bed_terminal::{bed_terminal::BedTerminal, terminal_font::TerminalFonts};
use bed_ui::icons::Icons;
use dear_imgui_rs::{
    Condition, ConfigFlags, Context, ContextBinding, ContextId, FocusedFlags, Key, MouseButton,
    StyleVar, Ui, WindowFlags, sys,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

#[path = "debugger.rs"]
mod debugger;
#[path = "editor_host.rs"]
mod editor_host;
mod file_operations;
#[path = "module_host.rs"]
mod module_host;
#[path = "native_host.rs"]
mod native_host;
#[path = "remote_workbench.rs"]
mod remote_workbench;
#[cfg(test)]
use bed_editor_ui::EditorView;
#[cfg(test)]
use bed_lsp::lsp_locations::LspLocation;
#[cfg(test)]
use bed_module_editor::lsp::lsp_ui::LspAction;
pub use module_host::WorkbenchModules;
use module_host::{HostedPanel, ModuleRuntime};
#[cfg(test)]
use std::ffi::CStr;

unsafe extern "C" {
    fn bed_imgui_dock_node_id(node: *const sys::ImGuiDockNode) -> u32;
    fn bed_imgui_dock_node_tab_bar(node: *const sys::ImGuiDockNode) -> *mut sys::ImGuiTabBar;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkbenchHostMode {
    Fullscreen,
    Floating,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCommand {
    Debug,
    NewDocument,
    NewTerminal,
    NewExplorer,
    NewSettings,
    NewProjects,
    NewDiagnostics,
    NewStructure,
    NewLspDashboard,
    NewContentSearch,
    NewReferences,
    DuplicateView,
    SplitRight,
    SplitDown,
    ResetLayout,
    Projects,
    Diagnostics,
    Structure,
    OpenFolder,
    OpenFile,
    Save,
    SaveAs,
    Close,
    Quit,
    Find,
    GoToLine,
    Explorer,
    Terminal,
    Settings,
    LspDashboard,
    FindFile,
    FindProject,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Explorer,
    Settings,
    Projects,
    Search,
    Diagnostics,
    References,
    LspDashboard,
}
impl Tool {
    fn key(self) -> &'static str {
        match self {
            Self::Explorer => "explorer",
            Self::Settings => "settings",
            Self::Projects => "projects",
            Self::Search => "search",
            Self::Diagnostics => "diagnostics",
            Self::References => "references",
            Self::LspDashboard => "lsp",
        }
    }
}
type Panel = HostedPanel;
impl HostedPanel {
    fn document(&self) -> Option<DocumentId> {
        self.instance.attached_document()
    }
    fn view_id(&self) -> Option<ViewId> {
        self.instance.view_id()
    }
    fn terminal_id(&self) -> Option<u64> {
        self.instance
            .as_any()
            .downcast_ref::<bed_module_terminal::TerminalPanel>()
            .map(|panel| panel.session_id())
    }
    #[cfg(test)]
    fn editor(&self) -> Option<&EditorView> {
        self.instance
            .as_any()
            .downcast_ref::<bed_module_editor::TextPanel>()
            .map(|panel| panel.view())
    }
    #[cfg(test)]
    fn editor_mut(&mut self) -> Option<&mut EditorView> {
        self.instance
            .as_any_mut()
            .downcast_mut::<bed_module_editor::TextPanel>()
            .map(|panel| panel.view_mut())
    }
    #[cfg(test)]
    fn hex(&self) -> Option<&bed_editor_ui::hex_editor::HexEditor> {
        self.instance
            .as_any()
            .downcast_ref::<bed_module_editor::HexPanel>()
            .map(|panel| panel.editor())
    }
    #[cfg(test)]
    fn hex_mut(&mut self) -> Option<&mut bed_editor_ui::hex_editor::HexEditor> {
        self.instance
            .as_any_mut()
            .downcast_mut::<bed_module_editor::HexPanel>()
            .map(|panel| panel.editor_mut())
    }
}
struct Tab {
    id: u64,
    panel: Panel,
    focus: bool,
    dock: Option<u32>,
    viewport: u32,
    dock_id: u32,
    detach: Option<[f32; 2]>,
}
struct FileDialog {
    action: FileTreeAction,
    name: String,
    error: Option<String>,
    appearing: bool,
    visible: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabCloseAction {
    Close,
    All,
    Others,
    Left,
    Right,
}
type PanelComposition = (u64, u32, u32, bool, [u32; 4]);

/// Application-owned shell. Embedding uses EditorSession and EditorView directly.
pub struct Workbench {
    pub settings: Settings,
    pub project_root: String,
    pub workspace_spec: Option<WorkspaceSpec>,
    remote_ui: remote_workbench::RemoteUi,
    pub session: EditorSession,
    pub icons: Icons,
    pub terminal: BedTerminal,
    pub terminal_fonts: TerminalFonts,
    pub error: Option<String>,
    pub root_top_inset: f32,
    tabs: Vec<Tab>,
    next_tab: u64,
    active: Option<ViewId>,
    last_document: Option<DocumentId>,
    focused: Option<u64>,
    modules: ModuleRuntime,
    module_factory: Rc<dyn Fn() -> WorkbenchModules>,
    scratch: Editor,
    store: Option<WorkspaceStore>,
    initialized: bool,
    context_id: Option<ContextId>,
    context_binding: Option<ContextBinding>,
    ui_animations: Option<UiAnimations>,
    mode: WorkbenchHostMode,
    dock_root: u32,
    center_dock: u32,
    explorer_dock: u32,
    terminal_dock: u32,
    dock_built: bool,
    scene: u64,
    pending_ini: Option<String>,
    pending_layout_reset: bool,
    pending_workspace: Option<WorkspaceSpec>,
    pending_workspace_files: Vec<(PathBuf, Option<String>, bool)>,
    last_persist: Instant,
    last_state: Option<Value>,
    last_settings_check: Instant,
    file_dialog: Option<FileDialog>,
    service_settings: Option<Value>,
    closed: bool,
    composition: Vec<PanelComposition>,
    viewport_focus: HashMap<u32, u64>,
    reload_confirmation: Option<DocumentId>,
    tab_context: Option<u64>,
    pending_tab_close: Option<Vec<u64>>,
    file_operations: file_operations::FileOperations,
}
impl Workbench {
    pub fn new(modules: impl Fn() -> WorkbenchModules + 'static) -> io::Result<Self> {
        Ok(Self::with_settings(Settings::new()?, modules))
    }
    /// Shared by custom captions and the OS window list, independent of the selected panel.
    pub fn window_title(&self) -> String {
        let name = self
            .workspace_spec
            .as_ref()
            .map(|spec| spec.name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                (!self.project_root.is_empty())
                    .then(|| WorkspaceSpec::local(&self.project_root).name)
            });
        match name {
            Some(name) => format!("bEd • {name}"),
            None => "bEd".to_owned(),
        }
    }
    pub fn with_settings(
        settings: Settings,
        modules: impl Fn() -> WorkbenchModules + 'static,
    ) -> Self {
        let (store, error) = match WorkspaceStore::load(&settings.config_dir) {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let now = Instant::now();
        let module_factory: Rc<dyn Fn() -> WorkbenchModules> = Rc::new(modules);
        let modules = ModuleRuntime::from_composition(module_factory());
        let mut this = Self {
            settings,
            project_root: String::new(),
            workspace_spec: None,
            remote_ui: remote_workbench::RemoteUi::default(),
            session: EditorSession::new(),
            icons: Icons::default(),
            terminal: BedTerminal::new_empty(),
            terminal_fonts: TerminalFonts::default(),
            error,
            root_top_inset: 0.0,
            tabs: Vec::new(),
            next_tab: 1,
            active: None,
            last_document: None,
            focused: None,
            modules,
            module_factory,
            scratch: Editor::new(),
            store,
            initialized: false,
            context_id: None,
            context_binding: None,
            ui_animations: None,
            mode: WorkbenchHostMode::Fullscreen,
            dock_root: 0,
            center_dock: 0,
            explorer_dock: 0,
            terminal_dock: 0,
            dock_built: false,
            scene: 1,
            pending_ini: None,
            pending_layout_reset: false,
            pending_workspace: None,
            pending_workspace_files: Vec::new(),
            last_persist: now,
            last_state: None,
            last_settings_check: now,
            file_dialog: None,
            service_settings: None,
            closed: false,
            composition: Vec::new(),
            viewport_focus: HashMap::new(),
            reload_confirmation: None,
            tab_context: None,
            pending_tab_close: None,
            file_operations: file_operations::FileOperations::default(),
        };
        this.refresh_projects();
        this.show_tool(Tool::Projects);
        this
    }
    pub fn initialize(
        &mut self,
        context: &mut Context,
        mode: WorkbenchHostMode,
    ) -> io::Result<bool> {
        if self.context_id.is_some_and(|id| id != context.id()) {
            return Err(io::Error::other(
                "Workbench belongs to a different ImGui context",
            ));
        }
        ensure_between_frames(context)?;
        if self.initialized {
            return Ok(false);
        }
        self.mode = mode;
        self.context_id = Some(context.id());
        self.context_binding = Some(context.binding());
        self.settings.is_embedded = false;
        let flags = context.io().config_flags() | ConfigFlags::DOCKING_ENABLE;
        context.io_mut().set_config_flags(flags);
        context.io_mut().set_config_docking_with_shift(false);
        context
            .io_mut()
            .set_config_windows_move_from_title_bar_only(true);
        // Persist layouts in workspaces.json, without a separate ImGui INI file.
        context
            .set_ini_filename(None::<PathBuf>)
            .map_err(io::Error::other)?;
        self.settings.request_apply();
        self.apply_settings(context)?;
        self.icons = Icons::load(&self.settings.resources_root);
        self.ui_animations = Some(UiAnimations::new(context));
        self.initialized = true;
        Ok(true)
    }
    pub fn initialized(&self) -> bool {
        self.initialized
    }
    /// Resume the previous window only when the host was launched without a path.
    pub fn restore_last_workspace(&mut self) -> io::Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        if let Some(spec) = store.last_workspace() {
            self.set_workspace(spec)?;
        } else if let Some(state) = store.standalone_layout().cloned() {
            self.close_plugin_panels()?;
            self.tabs.clear();
            self.reset_workspace_layout();
            self.modules.search.cancel_all();
            self.active = None;
            self.last_document = None;
            self.focused = None;
            self.next_tab = 1;
            self.restore_workspace(&state)?;
        }
        Ok(())
    }
    pub fn host_mode(&self) -> WorkbenchHostMode {
        self.mode
    }
    pub fn active_view(&self) -> Option<ViewId> {
        self.active
            .filter(|view| self.session.document_for_view(*view).is_some())
    }
    pub fn active_document(&self) -> Option<DocumentId> {
        self.last_document
            .filter(|document| self.session.document_kind(*document).is_ok())
    }
    pub fn active_snapshot(&self) -> Option<DocumentSnapshot> {
        self.session.snapshot(self.active_document()?).ok()
    }
    pub fn with_active_view<R>(
        &mut self,
        f: impl FnOnce(&mut bed_document_session::ViewContext<'_>) -> R,
    ) -> io::Result<Option<R>> {
        let Some(view) = self.active_view() else {
            return Ok(None);
        };
        self.session.with_view(view, f).map(Some)
    }
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }
    pub fn active_index(&self) -> usize {
        self.tabs
            .iter()
            .position(|tab| Some(tab.id) == self.focused)
            .unwrap_or(0)
    }
    pub fn tab_window_id(&self, index: usize) -> Option<u64> {
        self.tabs.get(index).map(|tab| tab.id)
    }
    pub fn active_panel_id(&self) -> Option<u64> {
        self.focused
    }
    pub fn panel_viewport_id(&self, id: u64) -> Option<u32> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.viewport)
    }
    /// Synchronize command targeting before the native menu can dispatch after
    /// an OS focus event. In a dock group only its selected tab receives focus.
    pub fn focus_viewport(&mut self, viewport: u32) -> bool {
        self.update_window_metadata();
        let visible = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                if tab.viewport != viewport {
                    return None;
                }
                let title = CString::new(self.title(tab)).ok()?;
                // SAFETY: Workbench is confined to its bound main-thread context.
                let visible = unsafe {
                    let window = sys::igFindWindowByName(title.as_ptr());
                    !window.is_null()
                        && ((*window).DockNode.is_null() || (*window).DockTabIsVisible())
                };
                visible.then_some((index, tab.id))
            })
            .collect::<Vec<_>>();
        let remembered = self.viewport_focus.get(&viewport).copied();
        let selected = visible
            .iter()
            .find(|(_, id)| Some(*id) == self.focused)
            .or_else(|| visible.iter().find(|(_, id)| Some(*id) == remembered))
            .or_else(|| visible.first())
            .map(|(index, _)| *index);
        selected.is_some_and(|index| self.switch_to_tab(index))
    }
    pub fn panel_visible(&self, kind: &str) -> bool {
        self.panel_count(kind) > 0
    }
    pub fn panel_count(&self, kind: &str) -> usize {
        self.tabs
            .iter()
            .filter(|tab| {
                tab.panel.kind == kind
                    || (kind == "document" && tab.panel.document().is_some())
                    || self
                        .modules
                        .registry
                        .panel(&tab.panel.kind)
                        .is_some_and(|descriptor| descriptor.legacy_kind == Some(kind))
                    || (kind == "structure" && tab.panel.kind == "bed.structure.panel")
            })
            .count()
    }
    pub fn active_overlay(&self) -> Overlay {
        self.active.map_or(Overlay::None, |view| {
            self.modules.editor_runtime.borrow().overlay(view)
        })
    }
    pub fn detach_panel_for_smoke(&mut self, id: u64, position: [f32; 2]) -> bool {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.detach = Some(position);
            tab.dock = None;
            tab.focus = true;
            self.scene += 1;
            true
        } else {
            false
        }
    }
    pub fn scene_generation(&self) -> u64 {
        self.scene
    }
    pub fn focused_terminal(&self) -> bool {
        self.tabs
            .iter()
            .any(|tab| Some(tab.id) == self.focused && tab.panel.terminal_id().is_some())
    }
    fn clear_focus_requests(&mut self) {
        self.terminal.cancel_focus_request();
        for tab in &mut self.tabs {
            tab.focus = false;
            if let Some(view) = tab.panel.view_id() {
                let _ = self
                    .session
                    .with_view(view, |editor| editor.view_mut().request_focus = false);
            }
        }
    }
    pub fn switch_to_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return false;
        }
        self.clear_focus_requests();
        let tab = &mut self.tabs[index];
        if let Some(document) = tab.panel.document() {
            self.last_document = Some(document);
        }
        tab.focus = true;
        self.focused = Some(tab.id);
        if let Some(view) = tab.panel.view_id() {
            self.active = Some(view);
            let _ = self.session.request_focus(view);
        }
        let _ = self.focus_module_panel(index);
        true
    }
    fn push_panel(&mut self, panel: Panel) -> u64 {
        self.clear_focus_requests();
        if let Some(document) = panel.document() {
            self.last_document = Some(document);
        }
        if let Some(view) = panel.view_id() {
            self.active = Some(view);
            let _ = self.session.request_focus(view);
        }
        let id = self.next_tab;
        self.next_tab += 1;
        let dock = self.largest_dock();
        self.tabs.push(Tab {
            id,
            panel,
            focus: true,
            dock: (dock != 0).then_some(dock),
            viewport: 0,
            dock_id: 0,
            detach: None,
        });
        self.focused = Some(id);
        let _ = self.focus_module_panel(self.tabs.len() - 1);
        self.scene += 1;
        id
    }
    fn largest_dock(&self) -> u32 {
        if self.pending_layout_reset {
            return 0;
        }
        let Some(binding) = &self.context_binding else {
            return self.center_dock;
        };
        // All native reads run on the UI thread in this workspace's live
        // context. Copy geometry/IDs only; no ImGui pointer is retained.
        binding
            .try_with_bound_context(|| unsafe {
                if sys::igGetCurrentContext().is_null() {
                    return self.center_dock;
                }
                let central = sys::igDockBuilderGetCentralNode(self.dock_root);
                let central_id = bed_imgui_dock_node_id(central);
                let mut candidates = Vec::new();
                if central_id != 0 {
                    candidates.push(central_id);
                }
                candidates.extend(self.tabs.iter().filter_map(|tab| {
                    let title = CString::new(self.title(tab)).unwrap();
                    let window = sys::igFindWindowByName(title.as_ptr());
                    (!window.is_null() && (*window).DockId != 0).then(|| (*window).DockId)
                }));
                candidates.sort_unstable();
                candidates.dedup();
                let mut best = None;
                for id in candidates {
                    let node = sys::igDockBuilderGetNode(id);
                    if node.is_null() || !sys::ImGuiDockNode_IsLeafNode(node) {
                        continue;
                    }
                    let rect = sys::ImGuiDockNode_Rect(node);
                    let width = rect.Max.x - rect.Min.x;
                    let height = rect.Max.y - rect.Min.y;
                    let area = width * height;
                    if width <= 0.0 || height <= 0.0 || !area.is_finite() {
                        continue;
                    }
                    let candidate = (area, id == central_id, id);
                    if best.is_none_or(|old: (f32, bool, u32)| {
                        area > old.0 || (area == old.0 && candidate.1 && !old.1)
                    }) {
                        best = Some(candidate);
                    }
                }
                best.map_or(self.center_dock, |(_, _, id)| id)
            })
            .unwrap_or(self.center_dock)
    }
    fn show_tool(&mut self, tool: Tool) {
        if let Err(error) = self.show_native_panel(tool.key(), None) {
            self.error = Some(error.to_string());
        }
    }
    fn add_view(&mut self, document: DocumentId) -> io::Result<u64> {
        let viewer = self
            .modules
            .registry
            .default_viewer(DocumentKind::Text)
            .ok_or_else(|| io::Error::other("Text viewer unavailable"))?
            .id;
        self.add_document_panel(document, Some(viewer), &Value::Null)
    }
    pub fn open_or_focus(&mut self, path: &Path) -> io::Result<bool> {
        self.open_file_with_viewer(path, None, false)
    }
    pub fn set_project(&mut self, root: &Path) -> io::Result<bool> {
        let canonical = std::fs::canonicalize(root)?;
        let text = canonical.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
        })?;
        let existing = self
            .store
            .as_ref()
            .and_then(|store| store.stored_spec(&WorkspaceSpec::local(text)));
        self.set_local_workspace(existing.unwrap_or_else(|| WorkspaceSpec::local(text)))
    }
    fn set_local_workspace(&mut self, mut spec: WorkspaceSpec) -> io::Result<bool> {
        if self.defer_workspace_switch(&spec) {
            return Ok(true);
        }
        let root = Path::new(&spec.root);
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Project must be a directory",
            ));
        }
        let path = root
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
            })?
            .to_owned();
        spec.root = path.clone();
        if self
            .workspace_spec
            .as_ref()
            .is_some_and(|active| active.identity() == spec.identity())
        {
            if let Some(store) = &mut self.store {
                self.workspace_spec = Some(store.record_workspace(spec)?);
                self.refresh_projects();
            }
            return Ok(false);
        }
        self.remote_ui.cancel_connection();
        self.cancel_file_operations();
        if !self.preflight_close(&(0..self.tabs.len()).collect::<Vec<_>>())? {
            if self
                .session
                .document_ids()
                .into_iter()
                .any(|id| self.session.save_pending(id))
                || self.remote_ui.mutation_pending()
                || self.remote_ui.path_dialog_pending()
            {
                self.remote_ui.pending_local = Some(spec);
            }
            return Ok(false);
        }
        let session = EditorSession::with_options(self.session_options(Some(root.clone())))?;
        self.cancel_file_operations();
        self.persist_workspace()?;
        let mut tree = bed_module_explorer::file_tree::FileTree {
            preferences: self
                .store
                .as_ref()
                .and_then(|store| store.module_settings(&spec, bed_module_explorer::MODULE_ID))
                .map(bed_module_explorer::file_tree::FileTreePreferences::from_value)
                .unwrap_or_default(),
            ..Default::default()
        };
        tree.root_node = bed_module_explorer::file_tree::FileNode {
            name: Path::new(&path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            full_path: path.clone(),
            is_directory: true,
            is_open: true,
            ..Default::default()
        };
        self.shutdown_modules()?;
        self.close_plugin_panels()?;
        self.session.shutdown(ClosePolicy::Discard)?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.modules.editor_runtime.borrow_mut().cancel_requests();
        self.modules.search.cancel_all();
        self.modules = ModuleRuntime::from_composition((self.module_factory)());

        self.remote_ui = remote_workbench::RemoteUi::default();
        self.file_explorer().file_finder.set_project_dir("");
        self.file_explorer().file_finder.set_remote_client(None);
        self.terminal.set_ssh_target(None);
        self.reset_workspace_layout();
        self.session = session;
        self.workspace_spec = Some(spec.clone());
        self.project_root = path.clone();
        self.restore_module_settings();
        self.service_settings = None;
        self.sync_services()?;
        self.file_explorer().project_root = path.clone();
        self.file_explorer().file_tree = tree;
        self.file_explorer().file_finder.set_project_dir(&path);
        self.terminal.set_project_root(&path);
        let restored = self
            .store
            .as_ref()
            .and_then(|store| store.layout(&spec))
            .cloned();
        if let Some(store) = &mut self.store {
            self.workspace_spec = Some(store.record_workspace(spec)?);
            self.refresh_projects();
        }
        if let Some(state) = restored {
            self.restore_workspace(&state)?;
        } else {
            self.show_tool(Tool::Explorer);
            if self.settings.terminal_visible {
                self.new_terminal();
            }
        }
        self.scene += 1;
        Ok(true)
    }
    fn defer_workspace_switch(&mut self, spec: &WorkspaceSpec) -> bool {
        let inside_frame = self.context_binding.as_ref().is_some_and(|binding| {
            binding.with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).WithinFrameScope })
        });
        if inside_frame {
            if self
                .pending_workspace
                .as_ref()
                .is_none_or(|pending| pending.identity() != spec.identity())
            {
                self.pending_workspace_files.clear();
            }
            self.pending_workspace = Some(spec.clone());
        }
        inside_frame
    }
    fn reset_workspace_layout(&mut self) {
        self.pending_layout_reset = self.context_binding.as_ref().is_none_or(|binding| {
            binding.with_bound_context(|| unsafe {
                if (*sys::igGetCurrentContext()).WithinFrameScope {
                    true
                } else {
                    sys::igClearIniSettings();
                    false
                }
            })
        });
        self.pending_ini = None;
        self.dock_built = false;
        self.center_dock = 0;
        self.explorer_dock = 0;
        self.terminal_dock = 0;
        self.next_tab = 1;
        self.last_state = None;
        self.composition.clear();
        self.viewport_focus.clear();
        self.file_dialog = None;
        self.reload_confirmation = None;
        self.tab_context = None;
        self.modules.editor_runtime.borrow_mut().cancel_requests();
        self.file_explorer().file_finder.cancel_and_close();
        self.file_explorer().file_finder.set_query("");
    }
    fn session_options(&self, root: Option<PathBuf>) -> SessionOptions {
        let project_services = root.is_some();
        SessionOptions {
            project_root: root,
            autosave: self
                .settings
                .autosave_enabled()
                .then(|| self.settings.autosave_delay()),
            monitoring: true,
            git: project_services && self.settings.bool("git_changed_lines", true),
            highlighting: self.settings.bool("treesitter", true),
            persistent_history: project_services,
            lsp_config: project_services.then(|| self.settings.config_dir.join("lsp.json")),
        }
    }
    fn sync_services(&mut self) -> io::Result<()> {
        if self.service_settings.as_ref() == Some(&self.settings.settings) {
            return Ok(());
        }
        self.service_settings = Some(self.settings.settings.clone());
        let mut options = self.session.options().clone();
        options.autosave = self
            .settings
            .autosave_enabled()
            .then(|| self.settings.autosave_delay());
        self.session.configure(options)?;
        self.session
            .set_git_enabled(self.settings.bool("git_changed_lines", true))?;
        self.session
            .set_highlighting_enabled(self.settings.bool("treesitter", true))?;
        let colors = self.settings.highlight_colors();
        self.session.set_highlight_theme(colors.clone());
        let foreground = self.settings.text_color();
        let background = self.settings.background_color();
        self.terminal
            .set_theme(background, foreground, self.settings.theme.terminal);
        Ok(())
    }
    pub fn apply_settings(&mut self, context: &mut Context) -> io::Result<bool> {
        ensure_between_frames(context)?;
        if self.pending_layout_reset {
            // Loading ini merges window settings. Clear both the saved settings
            // and live docking references so previous workspaces cannot rebind
            // obsolete windows into the new workspace's split containers.
            context.binding().with_bound_context(|| unsafe {
                sys::igClearIniSettings();
            });
            self.pending_layout_reset = false;
        }
        if let Some(ini) = self.pending_ini.take() {
            context.binding().with_bound_context(|| unsafe {
                sys::igLoadIniSettingsFromMemory(ini.as_ptr().cast(), ini.len());
            });
        }
        let applied = self.settings.apply(context, &mut self.scratch)?;
        if applied {
            if let Some(fonts) = &self.settings.font.resolved {
                self.terminal_fonts.reload_resolved(
                    context,
                    &self.settings.resources_root,
                    self.settings.font_size(),
                    &fonts.faces,
                    &fonts.fallbacks,
                )?;
            }
            self.terminal
                .reload_terminal_fonts(self.settings.font_size());
            self.service_settings = None;
            self.sync_services()?;
            self.scene += 1;
        }
        Ok(applied)
    }
    pub fn tick(&mut self) -> io::Result<()> {
        self.poll_file_operations()?;
        self.poll_workspace_filesystem()?;
        if let Some(spec) = self.pending_workspace.take() {
            let identity = spec.identity();
            let files = std::mem::take(&mut self.pending_workspace_files);
            self.set_workspace(spec)?;
            if self
                .workspace_spec
                .as_ref()
                .is_some_and(|active| active.identity() == identity)
            {
                for (path, viewer, additional) in files {
                    self.open_file_with_viewer(&path, viewer.as_deref(), additional)?;
                }
            }
        }
        self.poll_remote_workspace()?;
        let report = self.session.tick();
        for event in &report.events {
            if let SessionEvent::Removed { document, path } = event {
                self.handle_removed_document(*document, path)?;
            }
        }
        for module in &mut self.modules.instances {
            module.document_events(&self.session, &report.events)?;
        }
        if self.session.is_remote() {
            for event in &report.events {
                if let SessionEvent::Opened { document } = event {
                    self.finish_remote_open(*document)?;
                }
            }
            self.finish_remote_restoration()?;
        }
        for error in report.errors {
            if let Some(targets) = &self.pending_tab_close {
                let affected = error.document.is_none_or(|document| {
                    self.tabs.iter().any(|tab| {
                        targets.contains(&tab.id) && tab.panel.document() == Some(document)
                    })
                });
                if affected {
                    self.pending_tab_close = None;
                }
            }
            self.error = Some(format!("{}: {}", error.service, error.message));
        }
        if !self.remote_ui.path_dialog_pending()
            && let Some(ids) = self.pending_tab_close.take()
        {
            let indices = self
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(index, tab)| ids.contains(&tab.id).then_some(index))
                .collect();
            if let Err(error) = self.close_tabs(indices) {
                self.error = Some(error.to_string());
            }
        }
        self.terminal.poll()?;
        self.tick_plugins()?;
        if self.last_settings_check.elapsed() > Duration::from_millis(500) {
            self.last_settings_check = Instant::now();
            self.settings.check_settings_file();
        }
        self.process_native_actions(&mut Vec::new())?;
        self.finish_navigation_request()
    }
    fn new_terminal(&mut self) {
        if let Err(error) = self.open_native_panel("terminal", &Value::Null) {
            self.error = Some(error.to_string());
        }
    }
    pub fn dispatch(&mut self, command: WindowCommand) -> io::Result<bool> {
        match command {
            WindowCommand::Debug => {
                self.dispatch_command(bed_module_debug::SHOW_COMMAND)?;
            }
            WindowCommand::NewDocument => {
                if self.project_root.is_empty() {
                    self.session.configure(self.session_options(None))?;
                }
                let document = self.session.create_document(b"")?;
                self.add_view(document)?;
            }
            WindowCommand::NewTerminal => self.new_terminal(),
            WindowCommand::NewExplorer => {
                self.open_native_panel(Tool::Explorer.key(), &Value::Null)?;
            }
            WindowCommand::NewSettings => {
                self.open_native_panel(Tool::Settings.key(), &Value::Null)?;
            }
            WindowCommand::NewProjects => {
                self.open_native_panel(Tool::Projects.key(), &Value::Null)?;
            }
            WindowCommand::NewDiagnostics => {
                self.open_native_panel(Tool::Diagnostics.key(), &Value::Null)?;
            }
            WindowCommand::NewStructure => {
                self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)?;
            }
            WindowCommand::NewLspDashboard => {
                self.open_native_panel(Tool::LspDashboard.key(), &Value::Null)?;
            }
            WindowCommand::NewContentSearch => {
                self.open_native_panel(Tool::Search.key(), &Value::Null)?;
            }
            WindowCommand::NewReferences => {
                self.open_native_panel(Tool::References.key(), &Value::Null)?;
            }
            WindowCommand::DuplicateView => {
                if let Some(document) = self.active_document() {
                    let viewer = self.focused_document_viewer();
                    self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
                }
            }
            WindowCommand::SplitRight | WindowCommand::SplitDown => {
                if let Some(document) = self.active_document() {
                    let source_index = self.active_tab_index();
                    let source = source_index
                        .and_then(|index| self.tabs.get(index))
                        .map(|tab| {
                            if tab.dock_id != 0 {
                                tab.dock_id
                            } else {
                                tab.dock.unwrap_or(self.center_dock)
                            }
                        })
                        .unwrap_or(self.center_dock);
                    let viewer = self.focused_document_viewer();
                    let panel =
                        self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
                    if source != 0 {
                        let mut first = 0;
                        let mut second = 0;
                        unsafe {
                            sys::igDockBuilderSplitNode(
                                source,
                                if command == WindowCommand::SplitRight {
                                    sys::ImGuiDir_Right
                                } else {
                                    sys::ImGuiDir_Down
                                },
                                0.5,
                                &mut first,
                                &mut second,
                            );
                            sys::igDockBuilderFinish(self.dock_root);
                            if let Some(index) = source_index {
                                let name =
                                    CString::new(format!("###bed_tab_{}", self.tabs[index].id))
                                        .unwrap();
                                let window = sys::igFindWindowByName(name.as_ptr());
                                if !window.is_null() {
                                    let tab_bar = bed_imgui_dock_node_tab_bar((*window).DockNode);
                                    if !tab_bar.is_null() {
                                        // FocusWindow deliberately does not select dock tabs.
                                        // Select this leaf explicitly while the duplicate
                                        // keeps its separate keyboard focus request.
                                        (*tab_bar).SelectedTabId = (*window).TabId;
                                        (*tab_bar).NextSelectedTabId = (*window).TabId;
                                    }
                                }
                            }
                        }
                        if source == self.center_dock {
                            self.center_dock = second;
                        }
                        if source == self.explorer_dock {
                            self.explorer_dock = second;
                        }
                        if source == self.terminal_dock {
                            self.terminal_dock = second;
                        }
                        self.tabs
                            .iter_mut()
                            .find(|tab| tab.id == panel)
                            .unwrap()
                            .dock = Some(first);
                    }
                }
            }
            WindowCommand::ResetLayout => {
                self.dock_built = false;
                self.pending_ini = None;
                self.pending_layout_reset = true;
                self.scene += 1;
            }
            WindowCommand::Projects => self.show_tool(Tool::Projects),
            WindowCommand::Diagnostics => self.show_tool(Tool::Diagnostics),
            WindowCommand::Structure => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.panel.kind == "bed.structure.panel")
                {
                    self.switch_to_tab(index);
                } else {
                    self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)?;
                }
            }
            WindowCommand::OpenFolder => {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    return self.set_project(&path);
                } else {
                    return Ok(false);
                }
            }
            WindowCommand::OpenFile => return self.handle_action(HostAction::Open),
            WindowCommand::Save => return self.handle_action(HostAction::Save),
            WindowCommand::SaveAs => return self.handle_action(HostAction::SaveAs),
            WindowCommand::Close => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| Some(tab.id) == self.focused)
                {
                    return self.close_tab(index);
                }
            }
            WindowCommand::Quit => return self.request_close_all(),
            WindowCommand::Find => {
                self.focused_plugin_action(bed_workbench_api::PanelAction::Find)?;
            }
            WindowCommand::GoToLine => {
                self.focused_plugin_action(bed_workbench_api::PanelAction::GoToLine)?;
            }
            WindowCommand::Explorer => self.show_tool(Tool::Explorer),
            WindowCommand::Terminal => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.panel.terminal_id().is_some())
                {
                    self.switch_to_tab(index);
                } else {
                    self.new_terminal();
                }
            }
            WindowCommand::Settings => self.show_tool(Tool::Settings),
            WindowCommand::LspDashboard => self.show_tool(Tool::LspDashboard),
            WindowCommand::FindFile => {
                self.file_explorer().file_finder.toggle_window();
            }
            WindowCommand::FindProject => {
                self.show_tool(Tool::Search);
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| Some(tab.id) == self.focused)
                {
                    self.plugin_panel_action(index, bed_workbench_api::PanelAction::Find)?;
                }
            }
        }
        Ok(true)
    }
    fn active_tab_index(&self) -> Option<usize> {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == self.focused && tab.panel.document().is_some())
        {
            return Some(index);
        }
        if let Some(document) = self.active_document() {
            if self
                .active_view()
                .is_some_and(|view| self.session.document_for_view(view) == Some(document))
                && let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.panel.view_id() == self.active && self.active.is_some())
            {
                return Some(index);
            }
            if let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.panel.document() == Some(document))
            {
                return Some(index);
            }
        }
        None
    }
    fn focused_document_viewer(&self) -> Option<String> {
        self.active_tab_index()
            .and_then(|index| self.tabs.get(index))
            .and_then(|tab| tab.panel.viewer.clone())
    }
    pub fn handle_action(&mut self, action: HostAction) -> io::Result<bool> {
        match action {
            HostAction::Open => {
                if self.session.is_remote() {
                    self.show_remote_path_dialog(None)?;
                    return Ok(false);
                }
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    self.open_or_focus(&path)
                } else {
                    Ok(false)
                }
            }
            HostAction::Save => {
                let Some(document) = self.active_document() else {
                    return Ok(false);
                };
                self.ensure_file_operation_idle(document)?;
                if self.remote_ui.document_mutating(document) {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Wait for the remote file action before saving",
                    ));
                }
                self.commit_plugin_edits(document)?;
                let result = if self.session.snapshot(document)?.path.is_empty() {
                    self.save_as(document)
                } else {
                    self.session.save(document)
                };
                self.restore_save_focus()?;
                result
            }
            HostAction::SaveAs => {
                let Some(document) = self.active_document() else {
                    return Ok(false);
                };
                self.ensure_file_operation_idle(document)?;
                self.commit_plugin_edits(document)?;
                let result = self.save_as(document);
                self.restore_save_focus()?;
                result
            }
        }
    }
    fn restore_save_focus(&mut self) -> io::Result<()> {
        if let Some(index) = self.active_tab_index() {
            self.switch_to_tab(index);
            if let Some(view) = self.active_view() {
                self.session.with_view(view, |editor| {
                    editor.view_mut().cursor_blink_time = std::f32::consts::FRAC_PI_8;
                })?;
            }
        }
        Ok(())
    }
    fn save_as(&mut self, document: DocumentId) -> io::Result<bool> {
        if self.remote_ui.document_mutating(document) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the remote file action before saving",
            ));
        }
        if self.session.is_remote() {
            self.show_remote_path_dialog(Some(document))?;
            return Ok(false);
        }
        let snapshot = self.session.snapshot(document)?;
        let mut dialog = rfd::FileDialog::new();
        let suggested_path = snapshot.original_path.as_deref().unwrap_or(&snapshot.path);
        if !suggested_path.is_empty() {
            dialog = dialog.set_file_name(
                Path::new(suggested_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            );
        }
        if let Some(path) = dialog.save_file() {
            self.session.save_as(document, &path)
        } else {
            Ok(false)
        }
    }
    fn preflight_close(&mut self, indices: &[usize]) -> io::Result<bool> {
        if self.remote_ui.mutation_pending() {
            return Ok(false);
        }
        let closing_panels: Vec<_> = indices
            .iter()
            .map(|&index| self.tabs.get(index).map(|tab| tab.id))
            .collect();
        self.process_plugin_requests_inner(true)?;
        if indices
            .iter()
            .zip(closing_panels)
            .any(|(&index, expected)| self.tabs.get(index).map(|tab| tab.id) != expected)
        {
            return Ok(false);
        }
        for &index in indices {
            self.plugin_panel_action(index, bed_workbench_api::PanelAction::CommitEdit)?;
        }
        let closing = indices
            .iter()
            .filter_map(|index| self.tabs.get(*index))
            .map(|tab| tab.id)
            .collect::<HashSet<_>>();
        let docs = indices
            .iter()
            .filter_map(|index| self.tabs.get(*index))
            .filter_map(|tab| tab.panel.document())
            .collect::<HashSet<_>>();
        for document in docs {
            if self.ensure_file_operation_idle(document).is_err() {
                return Ok(false);
            }
            if self
                .tabs
                .iter()
                .filter(|tab| tab.panel.document() == Some(document))
                .all(|tab| closing.contains(&tab.id))
            {
                if self.session.save_pending(document) {
                    return Ok(false);
                }
                let snapshot = self.session.snapshot(document)?;
                if snapshot.dirty {
                    if snapshot.path.is_empty() {
                        if !self.save_as(document)? {
                            return Ok(false);
                        }
                    } else {
                        self.session.save(document)?;
                    }
                    if self.session.snapshot(document)?.dirty || self.session.save_pending(document)
                    {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    pub fn close_tab(&mut self, index: usize) -> io::Result<bool> {
        if index >= self.tabs.len() {
            return Ok(false);
        }
        if !self.preflight_close(&[index])? {
            return Ok(false);
        }
        self.remove_tab(index)?;
        Ok(true)
    }
    fn close_tabs(&mut self, mut indices: Vec<usize>) -> io::Result<bool> {
        indices.sort_unstable();
        indices.dedup();
        if !self.preflight_close(&indices)? {
            let waiting = self.session.is_remote()
                && (self.remote_ui.path_dialog_pending()
                    || self.remote_ui.mutation_pending()
                    || indices.iter().any(|index| {
                        self.tabs[*index]
                            .panel
                            .document()
                            .is_some_and(|doc| self.session.save_pending(doc))
                    }));
            if waiting {
                self.pending_tab_close =
                    Some(indices.iter().map(|index| self.tabs[*index].id).collect());
            }
            return Ok(false);
        }
        for index in indices.into_iter().rev() {
            self.remove_tab(index)?;
        }
        Ok(true)
    }
    /// Read the visible order from ImGui, which owns tab dragging/reordering.
    fn tab_group_order(&self, id: u64) -> Vec<usize> {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return Vec::new();
        };
        let Some(binding) = &self.context_binding else {
            return (0..self.tabs.len()).collect();
        };
        binding.with_bound_context(|| unsafe {
            let name = CString::new(self.title(&self.tabs[index])).unwrap();
            let window = sys::igFindWindowByName(name.as_ptr());
            if window.is_null() || (*window).DockNode.is_null() {
                return vec![index];
            }
            let bar = bed_imgui_dock_node_tab_bar((*window).DockNode);
            if bar.is_null() || (*bar).Tabs.Size <= 0 {
                return vec![index];
            }
            let native = std::slice::from_raw_parts((*bar).Tabs.Data, (*bar).Tabs.Size as usize);
            native
                .iter()
                .filter_map(|item| {
                    self.tabs.iter().position(|tab| {
                        let title = CString::new(self.title(tab)).unwrap();
                        sys::igFindWindowByName(title.as_ptr()) == item.Window
                    })
                })
                .collect()
        })
    }
    fn tab_close_indices(&self, id: u64, action: TabCloseAction) -> Vec<usize> {
        let group = self.tab_group_order(id);
        let Some(position) = group.iter().position(|index| self.tabs[*index].id == id) else {
            return Vec::new();
        };
        group
            .into_iter()
            .enumerate()
            .filter_map(|(offset, index)| {
                match action {
                    TabCloseAction::Close => offset == position,
                    TabCloseAction::All => true,
                    TabCloseAction::Others => offset != position,
                    TabCloseAction::Left => offset < position,
                    TabCloseAction::Right => offset > position,
                }
                .then_some(index)
            })
            .collect()
    }
    fn draw_tab_context_menu(&mut self, ui: &Ui) -> io::Result<()> {
        if ui.is_mouse_released(MouseButton::Right) && !ui.is_popup_open("Tab actions") {
            let mouse = ui.io().mouse_pos();
            self.tab_context = self.tabs.iter().find_map(|tab| unsafe {
                let name = CString::new(self.title(tab)).unwrap();
                let window = sys::igFindWindowByName(name.as_ptr());
                if window.is_null() || !(*window).Active || !(*window).DockIsActive() {
                    return None;
                }
                let rect = (*window).DC.DockTabItemRect;
                (mouse[0] >= rect.Min.x
                    && mouse[0] < rect.Max.x
                    && mouse[1] >= rect.Min.y
                    && mouse[1] < rect.Max.y)
                    .then_some(tab.id)
            });
            if self.tab_context.is_some() {
                ui.open_popup("Tab actions");
            }
        }
        if ui.is_popup_open("Tab actions")
            && let Some(tab) = self
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == self.tab_context)
        {
            // Tab menus are submitted after panel windows end. Route the popup
            // to the clicked tab's real viewport rather than the fallback window.
            ui.set_next_window_viewport(tab.viewport.into());
        }
        let _style = bed_ui::util::popup_style::context_menu_style(ui);
        let mut action = None;
        if let Some(_popup) = ui.begin_popup("Tab actions")
            && let Some(id) = self.tab_context
        {
            for (label, command) in [
                ("Close", TabCloseAction::Close),
                ("Close All", TabCloseAction::All),
                ("Close Others", TabCloseAction::Others),
                ("Close to the Right", TabCloseAction::Right),
                ("Close to the Left", TabCloseAction::Left),
            ] {
                let enabled = !self.tab_close_indices(id, command).is_empty();
                if ui.menu_item_enabled_selected_no_shortcut(label, false, enabled) {
                    action = Some((id, command));
                }
            }
        }
        if let Some((id, command)) = action {
            self.close_tabs(self.tab_close_indices(id, command))?;
        }
        Ok(())
    }
    fn remove_tab(&mut self, index: usize) -> io::Result<()> {
        let mut tab = self.tabs.remove(index);
        let mut requests = Vec::new();
        let result = self.with_module_services(|_, services| {
            tab.panel
                .instance
                .close_with_services(services, &mut requests)
        });
        if let Err(error) = result {
            self.tabs.insert(index, tab);
            return Err(error);
        }
        self.modules.requests.extend(requests);

        let document = tab.panel.document();
        if let Some(view) = tab.panel.view_id() {
            self.modules.editor_runtime.borrow_mut().forget_view(view);
            if self.active == Some(view) {
                self.active = None;
            }
        }
        if let Some(document) = document
            && !self
                .tabs
                .iter()
                .any(|tab| tab.panel.document() == Some(document))
        {
            self.session
                .close_document(document, ClosePolicy::Discard)?;
            if self.last_document == Some(document) {
                self.last_document = None;
            }
        }
        self.process_plugin_requests()?;
        if self.focused == Some(tab.id) {
            self.focused = None;
            if !self.tabs.is_empty() {
                self.switch_to_tab(index.min(self.tabs.len() - 1));
            }
        }
        self.scene += 1;
        self.tick_plugins()?;
        Ok(())
    }
    /// All removal sources converge here after the session has blocked writes.
    /// Finish staged panel input before deciding whether any data can be closed.
    pub(super) fn handle_removed_document(
        &mut self,
        document: DocumentId,
        path: &str,
    ) -> io::Result<()> {
        if !self
            .session
            .with_document(document, |state| !state.path.is_empty())
            .unwrap_or(false)
        {
            return Ok(());
        }
        let committed = match self.commit_plugin_edits(document) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(format!("Deleted file buffer preserved: {error}"));
                false
            }
        };
        let indices = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (tab.panel.document() == Some(document)).then_some(index))
            .collect::<Vec<_>>();
        let mut notified = true;
        for &index in &indices {
            let tab = &mut self.tabs[index];
            let mut requests = Vec::new();
            // Temporarily remove the panel so callback services can borrow the
            // workbench without aliasing its panel storage.
            let tab_id = tab.id;
            let mut tab = self.tabs.remove(index);
            let result = self.with_module_services(|_, services| {
                tab.panel.instance.document_removed_with_services(
                    document,
                    path,
                    services,
                    &mut requests,
                )
            });
            self.tabs.insert(index, tab);
            debug_assert_eq!(self.tabs[index].id, tab_id);
            self.modules.requests.extend(requests);
            if let Err(error) = result {
                self.error = Some(format!("Deleted file buffer preserved: {error}"));
                notified = false;
            }
        }
        if !committed || !notified || self.session.with_document(document, |state| state.dirty)? {
            self.session.detach_removed_document(document)?;
            self.scene += 1;
        } else {
            for index in indices.into_iter().rev() {
                if let Err(error) = self.remove_tab(index) {
                    // A cleanup failure must retain the document safely too.
                    self.session.detach_removed_document(document)?;
                    self.error = Some(format!("Deleted file buffer preserved: {error}"));
                    return Ok(());
                }
            }
            if self.session.document_ids().contains(&document) {
                self.session
                    .close_document(document, ClosePolicy::Discard)?;
            }
        }
        Ok(())
    }
    pub fn close_viewport(&mut self, id: u32) -> io::Result<bool> {
        self.update_window_metadata();
        let indices = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (tab.viewport == id).then_some(index))
            .collect::<Vec<_>>();
        if !self.preflight_close(&indices)? {
            return Ok(false);
        }
        for index in indices.into_iter().rev() {
            self.remove_tab(index)?;
        }
        Ok(true)
    }
    pub fn request_close_all(&mut self) -> io::Result<bool> {
        if self.closed {
            return Ok(true);
        }
        self.cancel_file_operations();
        let indices = (0..self.tabs.len()).collect::<Vec<_>>();
        if !self.preflight_close(&indices)? {
            return Ok(false);
        }
        self.persist_workspace()?;
        self.shutdown_modules()?;
        self.close_plugin_panels()?;
        self.session.shutdown(ClosePolicy::Discard)?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.modules.search.cancel_all();
        self.modules = ModuleRuntime::from_composition((self.module_factory)());

        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.closed = true;
        Ok(true)
    }
    pub fn cleanup(&mut self) -> io::Result<()> {
        if self.request_close_all()? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Close cancelled",
            ))
        }
    }
    fn title(&self, tab: &Tab) -> String {
        let title = tab
            .panel
            .document()
            .and_then(|document| self.session.original_path(document).ok().flatten())
            .map(|path| {
                format!(
                    "{} (deleted)",
                    Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )
            })
            .unwrap_or_else(|| tab.panel.instance.title(&self.modules.frame.context()));
        format!("{}###bed_tab_{}", title, tab.id)
    }
    fn update_window_metadata(&mut self) {
        let mut composition = Vec::with_capacity(self.tabs.len());
        for index in 0..self.tabs.len() {
            let title = CString::new(self.title(&self.tabs[index])).unwrap();
            // SAFETY: This shell uses its live main-thread context; all copied
            // window metadata is owned and no native pointer escapes.
            unsafe {
                let window = sys::igFindWindowByName(title.as_ptr());
                if !window.is_null() {
                    let tab = &mut self.tabs[index];
                    tab.viewport = (*window).ViewportId;
                    tab.dock_id = (*window).DockId;
                    let visible = if (*window).DockNode.is_null() {
                        (*window).Active && !(*window).Hidden
                    } else {
                        (*window).DockTabIsVisible()
                    };
                    composition.push((
                        tab.id,
                        tab.viewport,
                        tab.dock_id,
                        visible,
                        [
                            (*window).Pos.x.to_bits(),
                            (*window).Pos.y.to_bits(),
                            (*window).Size.x.to_bits(),
                            (*window).Size.y.to_bits(),
                        ],
                    ));
                }
            }
        }
        if self.composition != composition {
            self.composition = composition;
            self.scene += 1;
        }
        if let Some(id) = self.focused
            && let Some(tab) = self.tabs.iter().find(|tab| tab.id == id)
        {
            self.viewport_focus.insert(tab.viewport, id);
        }
    }
    fn build_layout(&mut self, ui: &Ui, size: [f32; 2]) {
        if self.dock_built || self.pending_layout_reset {
            return;
        }
        unsafe {
            sys::igDockBuilderRemoveNode(self.dock_root);
            sys::igDockBuilderAddNode(self.dock_root, sys::ImGuiDockNodeFlags_DockSpace);
            sys::igDockBuilderSetNodeSize(self.dock_root, size.into());
            let mut center = self.dock_root;
            let mut left = 0;
            let mut rest = 0;
            if self.tabs.iter().any(|tab| {
                self.panel_placement(&tab.panel) == bed_workbench_api::PanelPlacement::Sidebar
            }) {
                sys::igDockBuilderSplitNode(center, sys::ImGuiDir_Left, 0.2, &mut left, &mut rest);
                center = rest;
            }
            let mut bottom = 0;
            if self.tabs.iter().any(|tab| {
                tab.panel.terminal_id().is_some()
                    || self.panel_placement(&tab.panel) == bed_workbench_api::PanelPlacement::Bottom
            }) {
                sys::igDockBuilderSplitNode(
                    center,
                    sys::ImGuiDir_Down,
                    if self.tabs.iter().any(|tab| {
                        tab.panel.kind != bed_module_terminal::PANEL_ID
                            && self.panel_placement(&tab.panel)
                                == bed_workbench_api::PanelPlacement::Bottom
                    }) {
                        0.4
                    } else {
                        0.25
                    },
                    &mut bottom,
                    &mut rest,
                );
                center = rest;
            }
            self.center_dock = center;
            self.explorer_dock = if left != 0 { left } else { center };
            self.terminal_dock = if bottom != 0 { bottom } else { center };
            for tab in &mut self.tabs {
                let placement = self
                    .modules
                    .registry
                    .panel(&tab.panel.kind)
                    .map_or(bed_workbench_api::PanelPlacement::Center, |descriptor| {
                        descriptor.placement
                    });
                tab.dock = Some(match placement {
                    bed_workbench_api::PanelPlacement::Sidebar => self.explorer_dock,
                    bed_workbench_api::PanelPlacement::Bottom => self.terminal_dock,
                    bed_workbench_api::PanelPlacement::Center => self.center_dock,
                });
            }
            sys::igDockBuilderFinish(self.dock_root);
        }
        self.dock_built = true;
        let _ = ui;
    }
    pub fn render(&mut self, ui: &Ui) -> io::Result<Vec<HostAction>> {
        if self.context_id != Some(ui.context_id()) {
            return Err(io::Error::other(
                "Initialize Workbench in this ImGui context before rendering",
            ));
        }
        ui.with_bound_context(|| self.render_bound(ui))
    }
    fn render_bound(&mut self, ui: &Ui) -> io::Result<Vec<HostAction>> {
        if let Some(animations) = &mut self.ui_animations {
            animations.begin(ui, self.settings.bool("ui_animations", true));
        }
        let _font = self.settings.font.main.map(|font| ui.push_font(font));
        let mut actions = Vec::new();
        self.shortcuts(ui)?;
        self.refresh_plugins()?;
        let viewport = ui.main_viewport();
        let pos = viewport.work_pos();
        let mut size = viewport.work_size();
        size[1] = (size[1] - self.root_top_inset).max(1.0);
        if self.root_top_inset > 0.0 {
            let mut background = self.settings.background_color();
            background[3] = self.settings.background_opacity();
            ui.get_background_draw_list()
                .add_rect(
                    pos,
                    [pos[0] + size[0], pos[1] + self.root_top_inset],
                    background,
                )
                .filled(true)
                .build();
        }
        // Panels meet the native titlebar and neighboring dock nodes without
        // rounded cutouts. Dialogs push their own rounding after this scope.
        let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        let _workspace_border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        let mut dialog_result = Ok(());
        ui.set_next_window_viewport(ui.main_viewport().id());
        ui.window("##bed_workspace")
            .position([pos[0], pos[1] + self.root_top_inset], Condition::Always)
            .size(size, Condition::Always)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_BACKGROUND
                    | WindowFlags::NO_DOCKING
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
                    | WindowFlags::NO_NAV_FOCUS,
            )
            .build(|| {
                if let Some(animations) = &mut self.ui_animations {
                    animations.exclude_current(ui);
                }
                self.dock_root = ui.get_id("bed_workspace_dock").raw();
                self.build_layout(ui, size);
                // Native dock tab-list menus are submitted inside DockSpace;
                // keep their popup padding independent of the zero-pad root.
                let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
                // DockSpace creates a child host, which otherwise inherits
                // the submenu rounding from the menu style above.
                let _dock_rounding = ui.push_style_var(StyleVar::ChildRounding(0.0));
                unsafe {
                    sys::igDockSpace(self.dock_root, [0.0; 2].into(), 0, std::ptr::null());
                }
                drop(_dock_rounding);
                drop(_menu_style);
                // Popup IDs belong to their submitting window. Keep file
                // actions rooted here even when their source tab is hidden or
                // an embedding host changes the enclosing window.
                dialog_result = self.draw_file_dialog(ui);
            });
        drop(_workspace_border);
        drop(_padding);
        dialog_result?;
        self.configure_native_modules(ui);
        self.prepare_editor_menus()?;
        let mut close = Vec::new();
        let count = self.tabs.len();
        for index in 0..count {
            let mut tab = self.tabs.remove(index);
            let title = self.title(&tab);
            let mut opened = true;
            if let Some(position) = tab.detach.take() {
                unsafe {
                    sys::igSetNextWindowDockID(0, sys::ImGuiCond_Always);
                }
                unsafe {
                    sys::igSetNextWindowPos(
                        position.into(),
                        sys::ImGuiCond_Always,
                        [0.0; 2].into(),
                    );
                    sys::igSetNextWindowSize([650.0, 520.0].into(), sys::ImGuiCond_Always);
                }
            } else if let Some(dock) = tab.dock.take() {
                unsafe {
                    sys::igSetNextWindowDockID(dock, sys::ImGuiCond_Always);
                }
            }
            if std::mem::take(&mut tab.focus) {
                unsafe {
                    sys::igSetNextWindowFocus();
                }
            }
            let flags = WindowFlags::NO_COLLAPSE
                | WindowFlags::NO_FOCUS_ON_APPEARING
                | match tab.panel.document() {
                    Some(document) if self.session.snapshot(document)?.dirty => {
                        WindowFlags::UNSAVED_DOCUMENT
                    }
                    _ => WindowFlags::empty(),
                };
            let mut result = Ok(());
            let _panel_padding = tab
                .panel
                .instance
                .window_padding()
                .map(|padding| ui.push_style_var(StyleVar::WindowPadding(padding)));
            ui.window(&title)
                .opened(&mut opened)
                .flags(flags)
                .size([700.0, 520.0], Condition::FirstUseEver)
                .build(|| {
                    if let Some(animations) = &mut self.ui_animations {
                        animations.register_panel(ui);
                    }
                    if ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS) {
                        self.focused = Some(tab.id);
                        if let Some(document) = tab.panel.document() {
                            self.last_document = Some(document);
                        }
                        if let Some(view) = tab.panel.view_id() {
                            self.active = Some(view);
                        }
                    }
                    unsafe {
                        tab.viewport = (*sys::igGetWindowViewport()).ID;
                        tab.dock_id = sys::igGetWindowDockID();
                    }
                    result = self.draw_module_panel(ui, &mut tab.panel);
                });
            drop(_panel_padding);
            self.tabs.insert(index, tab);
            result?;
            if !opened {
                close.push(index);
            }
        }
        drop(_rounding);
        self.modules
            .editor_runtime
            .borrow_mut()
            .tick(&mut self.session, &mut self.modules.requests)?;
        self.sync_services()?;
        self.update_window_metadata();
        for index in close.into_iter().rev() {
            if let Err(error) = self.close_tab(index) {
                self.error = Some(error.to_string());
            }
        }
        if let Err(error) = self.draw_tab_context_menu(ui) {
            self.error = Some(error.to_string());
        }
        let tree_error = self.file_explorer().file_tree.error.take();
        if let Some(error) = tree_error {
            self.error = Some(error);
        }
        self.process_native_actions(&mut actions)?;
        self.draw_remote_path_dialog(ui)?;
        {
            let host = self.modules.frame.context();
            for plugin in &mut self.modules.instances {
                let _id = ui.push_id(plugin.id());
                plugin.draw_popups(ui, &host, &mut self.modules.requests);
            }
        }
        self.process_plugin_requests()?;
        self.draw_errors(ui)?;
        if let Some(path) = self.settings.request_config_file.take() {
            if self.session.is_remote() {
                self.error = Some(format!(
                    "bEd configuration is local to this computer: {}. Open it in a local workspace.",
                    path.display()
                ));
            } else {
                self.open_or_focus(&path)?;
            }
        }
        if std::mem::take(&mut self.settings.request_lsp_dashboard) {
            self.show_tool(Tool::LspDashboard);
        }
        if self.last_persist.elapsed() > Duration::from_secs(2) {
            if let Err(error) = self.persist_workspace() {
                self.error = Some(error.to_string());
            }
            self.last_persist = Instant::now();
        }
        if let Some(animations) = &mut self.ui_animations {
            animations.end(ui, self.settings.bool("ui_animations", true));
        }
        Ok(actions)
    }
    fn shortcuts(&mut self, ui: &Ui) -> io::Result<()> {
        self.module_shortcuts(ui)?;
        let ctrl = ui.io().key_ctrl() || ui.io().key_super();
        if ctrl
            && (self.focused_hex() || self.focused_document_plugin())
            && ui.is_key_pressed_with_repeat(Key::S, false)
            && let Err(error) = self.handle_action(if ui.io().key_shift() {
                HostAction::SaveAs
            } else {
                HostAction::Save
            })
        {
            self.error = Some(error.to_string());
        }
        if ui.io().want_text_input() {
            return Ok(());
        }
        if !ctrl {
            return Ok(());
        }
        let pressed = |name| {
            self.settings
                .keybinds
                .get_action_key(name)
                .is_some_and(|key| ui.is_key_pressed_with_repeat(key, false))
        };
        let command = if ui.io().key_shift() && ui.is_key_pressed_with_repeat(Key::F, false) {
            Some(WindowCommand::FindProject)
        } else if ui.is_key_pressed_with_repeat(Key::Slash, false) {
            Some(WindowCommand::Projects)
        } else if pressed("toggle_settings_window") {
            Some(WindowCommand::Settings)
        } else if pressed("toggle_terminal") {
            Some(WindowCommand::Terminal)
        } else if pressed("toggle_file_finder") {
            Some(WindowCommand::FindFile)
        } else if pressed("find_in_project") {
            Some(WindowCommand::FindProject)
        } else {
            None
        };
        if let Some(command) = command {
            self.dispatch(command)?;
        }
        for (index, key) in [
            Key::Key1,
            Key::Key2,
            Key::Key3,
            Key::Key4,
            Key::Key5,
            Key::Key6,
            Key::Key7,
            Key::Key8,
            Key::Key9,
        ]
        .into_iter()
        .enumerate()
        {
            if ui.is_key_pressed_with_repeat(key, false) {
                self.switch_to_tab(index);
            }
        }
        Ok(())
    }
    fn restore_tree_preferences(&mut self, spec: &WorkspaceSpec) {
        let preferences = self
            .store
            .as_ref()
            .and_then(|store| store.module_settings(spec, bed_module_explorer::MODULE_ID))
            .map(bed_module_explorer::file_tree::FileTreePreferences::from_value)
            .unwrap_or_default();
        let mut explorer = self.file_explorer();
        explorer.file_tree.preferences = preferences;
        explorer.file_tree.show_hidden = false;
        explorer.file_tree.error = None;
    }

    fn handle_tree_action(&mut self, action: FileTreeAction) -> io::Result<()> {
        if self.handle_batch_file_action(&action)? {
            return Ok(());
        }
        if let FileTreeAction::Command { command, context } = &action {
            if let Some(viewer) = command.strip_prefix("bed.open_with:") {
                if let Some(path) = &context.path {
                    self.open_file_with_viewer(Path::new(path), Some(viewer), true)?;
                }
            } else {
                self.refresh_plugins()?;
                self.run_module_command(command, context)?;
                self.process_plugin_requests()?;
            }
            return Ok(());
        }
        if self
            .file_explorer()
            .file_tree
            .apply_visibility_action(&action, self.session.is_remote())
        {
            let preferences = self.file_explorer().file_tree.preferences.clone();
            if !matches!(action, FileTreeAction::SetShowHidden(_))
                && let (Some(store), Some(spec)) = (&mut self.store, &self.workspace_spec)
            {
                store.set_module_settings(
                    spec,
                    bed_module_explorer::MODULE_ID,
                    preferences.to_value(),
                )?;
            }
            let paths = self.modules.explorer.open_directories();
            self.refresh_file_directories(paths);
            return Ok(());
        }
        if let FileTreeAction::Open(path) = action {
            self.open_or_focus(Path::new(&path))?;
        } else {
            let name = match &action {
                FileTreeAction::Rename(path) if self.session.is_remote() => {
                    path.rsplit('/').next().unwrap_or_default().to_owned()
                }
                FileTreeAction::Rename(path) => Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                _ => String::new(),
            };
            self.file_dialog = Some(FileDialog {
                action,
                name,
                error: None,
                appearing: true,
                visible: false,
            });
        }
        Ok(())
    }
    fn draw_file_dialog(&mut self, ui: &Ui) -> io::Result<()> {
        let Some(mut dialog) = self.file_dialog.take() else {
            return Ok(());
        };
        let title = match &dialog.action {
            FileTreeAction::NewFile(_) => "New file",
            FileTreeAction::NewFolder(_) => "New folder",
            FileTreeAction::Rename(_) => "Rename",
            FileTreeAction::Trash(_) if self.session.is_remote() => "Delete permanently",
            FileTreeAction::Trash(_) => "Move to Trash",
            FileTreeAction::Open(_) => "Open",
            _ => unreachable!("visibility actions do not open file dialogs"),
        };
        let popup_name = format!("{title}###bed_file_action");
        let appearing = std::mem::take(&mut dialog.appearing);
        dialog.visible = false;
        if appearing {
            self.clear_focus_requests();
            ui.open_popup(&popup_name);
        }
        let viewport = ui.main_viewport();
        let position = viewport.work_pos();
        let size = viewport.work_size();
        let fs = ui.current_font_size();
        let width = (fs * 30.0).min((size[0] - fs * 2.0).max(1.0));
        let max_height = (size[1] - self.root_top_inset - fs * 2.0).max(1.0);
        ui.set_next_window_viewport(viewport.id());
        ui.with_bound_context(|| unsafe {
            sys::igSetNextWindowSizeConstraints(
                [width, (fs * 9.0).min(max_height)].into(),
                [width, max_height].into(),
                None,
                std::ptr::null_mut(),
            );
            sys::igSetNextWindowPos(
                [
                    position[0] + size[0] * 0.5,
                    position[1] + self.root_top_inset + (size[1] - self.root_top_inset) * 0.5,
                ]
                .into(),
                sys::ImGuiCond_Appearing,
                [0.5; 2].into(),
            );
        });
        let mut opened = true;
        let mut done = false;
        let _dialog_style = bed_ui::util::popup_style::dialog_style(ui);
        if let Some(_popup) = ui
            .begin_modal_popup_config(&popup_name)
            .opened(&mut opened)
            .flags(
                WindowFlags::ALWAYS_AUTO_RESIZE
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_COLLAPSE
                    | WindowFlags::NO_SAVED_SETTINGS
                    | WindowFlags::NO_DOCKING,
            )
            .begin()
        {
            dialog.visible = true;
            if ui.is_key_pressed_with_repeat(Key::Escape, false) {
                // The editor resumes later in this frame; consume the modal's
                // Escape so it cannot also collapse the editor's selection.
                ui.with_bound_context(|| unsafe {
                    sys::igSetKeyOwner(
                        sys::ImGuiKey_Escape,
                        ui.get_id("file_dialog_dismiss").raw(),
                        sys::ImGuiInputFlags_LockThisFrame,
                    );
                });
                done = true;
                ui.close_current_popup();
            } else {
                if !matches!(dialog.action, FileTreeAction::Trash(_)) {
                    ui.text("Name");
                    if appearing {
                        ui.set_keyboard_focus_here();
                    }
                    ui.set_next_item_width(-1.0);
                    ui.input_text("##file_action_name", &mut dialog.name)
                        .build();
                } else if let FileTreeAction::Trash(path) = &dialog.action {
                    ui.text_wrapped(path);
                }
                if let Some(error) = &dialog.error {
                    ui.text_wrapped(error);
                }
                if ui.button("Cancel") {
                    done = true;
                    ui.close_current_popup();
                }
                ui.same_line();
                if ui.button(title) && !done {
                    match self.apply_file_action(&dialog.action, &dialog.name) {
                        Ok(()) => {
                            done = true;
                            ui.close_current_popup();
                        }
                        Err(error) => dialog.error = Some(error.to_string()),
                    }
                }
            }
        }
        if done || !opened || !ui.is_popup_open(&popup_name) {
            self.restore_save_focus()?;
        } else {
            self.file_dialog = Some(dialog);
        }
        Ok(())
    }
    pub fn rename_path(&mut self, source: &Path, name: &str) -> io::Result<PathBuf> {
        if self.session.is_remote() {
            file_actions::validate_name(name)?;
            let target = remote_workbench::remote_rename_target(
                source.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Remote path is not UTF-8")
                })?,
                name,
            )?;
            self.queue_remote_file_action(
                &FileTreeAction::Rename(source.to_string_lossy().into_owned()),
                name,
            )?;
            return Ok(PathBuf::from(target));
        }
        let source = file_actions::validate_project_entry(Path::new(&self.project_root), source)?;
        let target = source.parent().unwrap().join(name);
        file_actions::validate_name(name)?;
        let mappings = self
            .affected_documents(&source)
            .into_iter()
            .map(|(id, path)| {
                let relative = path.strip_prefix(&source).unwrap();
                let destination = if relative.as_os_str().is_empty() {
                    target.clone()
                } else {
                    target.join(relative)
                };
                (id, path, destination)
            })
            .collect::<Vec<_>>();
        for (id, _, destination) in &mappings {
            if destination.to_str().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Path is not UTF-8",
                ));
            }
            for other in self.session.document_ids() {
                if other != *id
                    && self
                        .session
                        .with_document(other, |state| state.path == destination.to_string_lossy())?
                {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Destination is open as another document",
                    ));
                }
            }
        }
        let renamed = file_actions::rename_entry(&source, name)?;
        for (id, _, destination) in &mappings {
            if let Err(error) = self.session.rebind_path(*id, destination) {
                let _ = std::fs::rename(&renamed, &source);
                for (id, old, _) in &mappings {
                    let _ = self.session.rebind_path(*id, old);
                }
                return Err(error);
            }
        }
        self.refresh_file_directories([source.parent().unwrap().to_string_lossy().into_owned()]);
        Ok(renamed)
    }
    pub fn trash_path(&mut self, source: &Path) -> io::Result<()> {
        if self.session.is_remote() {
            return self.queue_remote_file_action(
                &FileTreeAction::Trash(source.to_string_lossy().into_owned()),
                "",
            );
        }
        self.trash_path_with(source, file_actions::move_to_trash)
    }
    fn trash_path_with(
        &mut self,
        source: &Path,
        trash: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        let source = file_actions::validate_project_entry(Path::new(&self.project_root), source)?;
        let affected = self.affected_documents(&source);
        trash(&source)?;
        for (id, path) in affected {
            self.session.invalidate_removed_path(id)?;
            self.handle_removed_document(id, &path.to_string_lossy())?;
        }
        self.refresh_file_directories([source.parent().unwrap().to_string_lossy().into_owned()]);
        Ok(())
    }
    fn affected_documents(&self, source: &Path) -> Vec<(DocumentId, PathBuf)> {
        if !self.session.is_remote()
            && std::fs::symlink_metadata(source)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Vec::new();
        }
        self.session
            .document_ids()
            .into_iter()
            .filter_map(|id| {
                let path = self
                    .session
                    .with_document(id, |state| state.path.clone())
                    .ok()?;
                let within = if self.session.is_remote() {
                    source.to_str().is_some_and(|source| {
                        path == source
                            || path
                                .strip_prefix(source.trim_end_matches('/'))
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
                } else {
                    Path::new(&path).starts_with(source)
                };
                within.then_some((id, PathBuf::from(path)))
            })
            .collect()
    }
    fn apply_file_action(&mut self, action: &FileTreeAction, name: &str) -> io::Result<()> {
        if matches!(
            action,
            FileTreeAction::SetHideGitignored(_)
                | FileTreeAction::SetHideHidden(_)
                | FileTreeAction::SetShowHidden(_)
                | FileTreeAction::SetPathHidden { .. }
        ) {
            return self.handle_tree_action(action.clone());
        }
        if self.session.is_remote() {
            return self.queue_remote_file_action(action, name);
        }
        match action {
            FileTreeAction::NewFile(directory) => {
                self.validate_directory(directory)?;
                let path = file_actions::create_file(Path::new(directory), name)?;
                self.refresh_file_directories([directory.clone()]);
                self.open_or_focus(&path)?;
            }
            FileTreeAction::NewFolder(directory) => {
                self.validate_directory(directory)?;
                file_actions::create_folder(Path::new(directory), name)?;
                self.refresh_file_directories([directory.clone()]);
            }
            FileTreeAction::Rename(path) => {
                self.rename_path(Path::new(path), name)?;
            }
            FileTreeAction::Trash(path) => self.trash_path(Path::new(path))?,
            FileTreeAction::Open(path) => {
                self.open_or_focus(Path::new(path))?;
            }
            _ => unreachable!("visibility actions handled above"),
        }
        Ok(())
    }
    fn validate_directory(&self, path: &str) -> io::Result<()> {
        let path = std::fs::canonicalize(path)?;
        if !path.is_dir() || !path.starts_with(&self.project_root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Directory is outside this project",
            ));
        }
        Ok(())
    }
    fn refresh_files(&mut self) -> io::Result<()> {
        self.file_explorer().file_finder.request_refresh();
        self.modules.search.cancel_all();
        Ok(())
    }
    fn refresh_file_directories(&mut self, directories: impl IntoIterator<Item = String>) {
        self.file_explorer()
            .file_finder
            .refresh_directories(directories);
        self.modules.search.cancel_all();
    }
    fn draw_errors(&mut self, ui: &Ui) -> io::Result<()> {
        self.draw_file_operations(ui)?;
        let _dialog_style = bed_ui::util::popup_style::dialog_style(ui);
        if let Some(error) = self.error.clone() {
            let mut open = true;
            let mut dismiss = false;
            let viewport = ui.main_viewport();
            let position = viewport.work_pos();
            let available = viewport.work_size();
            let width = (ui.current_font_size() * 32.0).min((available[0] - 32.0).max(1.0));
            ui.set_next_window_viewport(viewport.id());
            ui.window("Error")
                .opened(&mut open)
                .position(
                    [
                        position[0] + (available[0] - width) * 0.5,
                        position[1] + available[1] * 0.25,
                    ],
                    Condition::Appearing,
                )
                .size_constraints([width, 0.0], [width, (available[1] * 0.70).max(1.0)])
                .flags(
                    WindowFlags::NO_DOCKING
                        | WindowFlags::NO_SAVED_SETTINGS
                        | WindowFlags::NO_COLLAPSE
                        | WindowFlags::ALWAYS_AUTO_RESIZE,
                )
                .build(|| {
                    if ui.is_window_appearing() {
                        ui.with_bound_context(|| unsafe {
                            sys::igFocusWindow(
                                sys::igGetCurrentWindow(),
                                sys::ImGuiFocusRequestFlags_UnlessBelowModal,
                            );
                        });
                    }
                    ui.text_wrapped(error);
                    if ui.button("Dismiss")
                        || (ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS)
                            && ui.is_key_pressed_with_repeat(Key::Escape, false))
                    {
                        dismiss = true;
                    }
                });
            if !open || dismiss {
                self.error = None;
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| Some(tab.id) == self.focused)
                {
                    self.switch_to_tab(index);
                }
            }
        }
        let conflicts = self
            .session
            .document_ids()
            .into_iter()
            .filter_map(|id| {
                self.session
                    .snapshot(id)
                    .ok()
                    .filter(|snapshot| snapshot.disk_conflict.is_some())
                    .map(|snapshot| (id, snapshot))
            })
            .collect::<Vec<_>>();
        for (id, snapshot) in conflicts {
            let mut operation = None;
            ui.window(format!("File changed###conflict_{}", id.0))
                .flags(WindowFlags::NO_DOCKING)
                .build(|| {
                    ui.text_wrapped(&snapshot.path);
                    if let Some(message) = &snapshot.disk_conflict {
                        ui.text_wrapped(message);
                    }
                    if ui.button("Reload from Disk") {
                        operation = Some(0)
                    }
                    ui.same_line();
                    if ui.button("Keep Buffer") {
                        operation = Some(1)
                    }
                    ui.same_line();
                    if ui.button("Save As") {
                        operation = Some(2)
                    }
                });
            if let Some(operation) = operation {
                let result = match operation {
                    0 if snapshot.dirty => {
                        self.reload_confirmation = Some(id);
                        ui.open_popup("Discard Changes?");
                        Ok(())
                    }
                    0 => self.session.reload_from_disk(id),
                    1 => self.session.keep_buffer(id),
                    _ => self.save_as(id).map(|_| ()),
                };
                if let Err(error) = result {
                    self.error = Some(error.to_string());
                }
            }
        }
        if let Some(id) = self.reload_confirmation
            && let Some(_modal) = ui.begin_modal_popup("Discard Changes?")
        {
            ui.text_wrapped(
                "Reloading from disk discards the unsaved buffer and its undo history.",
            );
            if ui.button("Cancel") {
                self.reload_confirmation = None;
                ui.close_current_popup();
            }
            ui.same_line();
            if ui.button("Discard Changes and Reload") {
                if let Err(error) = self.session.reload_from_disk(id) {
                    self.error = Some(error.to_string());
                }
                self.reload_confirmation = None;
                ui.close_current_popup();
            }
        }
        Ok(())
    }
    fn persist_workspace(&mut self) -> io::Result<()> {
        self.persist_module_settings()?;
        let mut tabs = std::mem::take(&mut self.tabs);
        let result = self.with_module_services(|_, services| {
            let mut states = HashMap::new();
            for tab in &mut tabs {
                states.insert(
                    tab.id,
                    tab.panel.instance.save_state_with_services(services)?,
                );
            }
            Ok(states)
        });
        self.tabs = tabs;
        let panel_states = result?;
        if self.store.is_none() || self.remote_ui.connecting() || !self.remote_ui.restore.is_empty()
        {
            return Ok(());
        }
        // An unapplied layout is still authoritative when closing before the
        // first frame. Never replace it with a fresh context's empty settings.
        let ini = self.pending_ini.clone().unwrap_or_else(|| {
            if self.pending_layout_reset {
                return String::new();
            }
            self.context_binding
                .as_ref()
                .map(|binding| {
                    binding.with_bound_context(|| unsafe {
                        let mut length = 0;
                        let ptr = sys::igSaveIniSettingsToMemory(&mut length);
                        String::from_utf8_lossy(std::slice::from_raw_parts(
                            ptr.cast::<u8>(),
                            length,
                        ))
                        .into_owned()
                    })
                })
                .unwrap_or_default()
        });
        let mut panels = Vec::new();
        for tab in &self.tabs {
            let panel = &tab.panel;
            if !panel.instance.persist() {
                continue;
            }
            let doc = panel
                .instance
                .attached_document()
                .map(|id| self.session.snapshot(id))
                .transpose()?;
            if doc.as_ref().is_some_and(|doc| doc.path.is_empty()) {
                continue;
            }
            let legacy = self
                .modules
                .registry
                .panel(&panel.kind)
                .and_then(|descriptor| descriptor.legacy_kind);
            let mut value = json!({"kind":legacy.unwrap_or("plugin"),"panel_type":panel.kind,"viewer":panel.viewer,
                "path":doc.as_ref().map(|doc| doc.path.as_str()),"document_kind":doc.as_ref().map(|doc|if doc.kind == DocumentKind::Bytes {"bytes"} else {"text"}),"state":panel_states.get(&tab.id)});
            if matches!(legacy, Some("document" | "terminal"))
                && let Some(state) = value["state"].as_object().cloned()
            {
                value.as_object_mut().unwrap().extend(state);
            }
            value["id"] = json!(tab.id);
            panels.push(value);
        }
        let ini = crate::workspace::layout::prepare(
            &ini,
            panels.iter().filter_map(|panel| panel["id"].as_u64()),
        )
        .unwrap_or_default();
        let state = json!({"version":1,"ini":ini,"panels":panels,"focused":self.focused,"active_document_panel":self.active_tab_index().and_then(|index|self.tabs.get(index)).map(|tab|tab.id)});
        if self.last_state.as_ref() != Some(&state) {
            self.store
                .as_mut()
                .unwrap()
                .save_session_layout(self.workspace_spec.as_ref(), state.clone())?;
            self.last_state = Some(state);
        }
        Ok(())
    }
    fn restore_workspace(&mut self, state: &Value) -> io::Result<()> {
        let mut state = state.clone();
        let mut ids = HashSet::new();
        let invalid_ids = state["panels"].as_array().is_some_and(|panels| {
            panels.iter().any(|panel| {
                !panel["id"]
                    .as_u64()
                    .is_some_and(|id| id > 0 && id <= u32::MAX as u64 && ids.insert(id))
            })
        });
        if invalid_ids {
            if let Some(panels) = state["panels"].as_array_mut() {
                panels.retain(Value::is_object);
                for (index, panel) in panels.iter_mut().enumerate() {
                    panel["id"] = json!(index + 1);
                }
            }
            state["ini"] = Value::Null;
            state["focused"] = Value::Null;
            state["active_document_panel"] = Value::Null;
            self.error = Some(
                "Workspace panel IDs were invalid; restored panels with a default layout".into(),
            );
        }
        self.remote_ui.restored_active = state["active_document_panel"].as_u64();
        self.remote_ui.restored_focus = state["focused"].as_u64();
        if self.session.is_remote() {
            self.remote_ui.restore_order = state["panels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|panel| panel["id"].as_u64())
                .collect();
            self.next_tab = state["panels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|panel| panel["id"].as_u64())
                .max()
                .unwrap_or(0)
                + 1;
        }
        for panel in state["panels"].as_array().into_iter().flatten() {
            let kind = panel["kind"].as_str().unwrap_or("");
            let result = match kind {
                "structure" => {
                    self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)
                }
                "hex" | "plugin" => {
                    let viewer = panel["viewer"].as_str();
                    let path = panel["path"].as_str();
                    if let Some(path) = path {
                        let document_kind =
                            if panel["document_kind"].as_str() == Some("bytes") || kind == "hex" {
                                DocumentKind::Bytes
                            } else {
                                DocumentKind::Text
                            };
                        if self.session.is_remote() {
                            self.remote_ui
                                .restore
                                .entry(path.to_owned())
                                .or_default()
                                .push(panel.clone());
                            self.remote_ui.opening.insert(path.to_owned());
                            if let Err(error) = self
                                .session
                                .request_open_file_with_kind(Path::new(path), document_kind)
                            {
                                self.remote_ui.restore.remove(path);
                                self.error = Some(error.to_string());
                            }
                            continue;
                        }
                        self.session
                            .open_file_with_kind(Path::new(path), document_kind)
                            .and_then(|document| {
                                if kind == "hex" || viewer.is_some() {
                                    self.add_document_panel(document, viewer, &panel["state"])
                                } else {
                                    self.open_plugin_panel(
                                        panel["panel_type"].as_str().unwrap_or(""),
                                        Some(document),
                                        &panel["state"],
                                        None,
                                    )
                                }
                            })
                    } else {
                        self.open_plugin_panel(
                            panel["panel_type"].as_str().unwrap_or(""),
                            None,
                            &panel["state"],
                            viewer.map(str::to_owned),
                        )
                    }
                }
                "document" => {
                    if self.session.is_remote() {
                        if let Some(path) = panel["path"].as_str() {
                            self.remote_ui
                                .restore
                                .entry(path.to_owned())
                                .or_default()
                                .push(panel.clone());
                            self.remote_ui.opening.insert(path.to_owned());
                            if let Err(error) = self.session.request_open_file(Path::new(path)) {
                                self.remote_ui.restore.remove(path);
                                self.error = Some(error.to_string());
                            }
                        }
                        continue;
                    }
                    if let Some(path) = panel["path"]
                        .as_str()
                        .filter(|path| Path::new(path).is_file())
                    {
                        self.session.open_file(Path::new(path)).and_then(|doc| {
                            self.add_document_panel(doc, panel["viewer"].as_str(), &panel["state"])
                        })
                    } else {
                        continue;
                    }
                }
                "terminal" => self.open_native_panel(
                    "terminal",
                    if panel["state"].is_null() {
                        panel
                    } else {
                        &panel["state"]
                    },
                ),
                _ => {
                    if let Some(descriptor) = self
                        .modules
                        .registry
                        .panels
                        .iter()
                        .find(|descriptor| descriptor.legacy_kind == Some(kind))
                        .cloned()
                    {
                        self.open_plugin_panel(descriptor.id, None, &panel["state"], None)
                    } else {
                        continue;
                    }
                }
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
                continue;
            }
            let tab = self.tabs.last_mut().unwrap();
            if let Some(id) = panel["id"].as_u64() {
                tab.id = id;
                self.next_tab = self.next_tab.max(id + 1);
            }
            if let Some(view) = tab.panel.view_id()
                && panel["selections"].is_array()
            {
                let selections = panel["selections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| {
                        let v = value.as_array()?;
                        Some(Selection {
                            head_row: v.first()?.as_i64()? as i32,
                            head_column: v.get(1)?.as_i64()? as i32,
                            anchor_row: v.get(2)?.as_i64()? as i32,
                            anchor_column: v.get(3)?.as_i64()? as i32,
                            preferred_column: 0,
                        })
                    })
                    .collect::<Vec<_>>();
                self.session.with_commands(view, |commands| {
                    commands.set_selections(
                        selections,
                        panel["primary"].as_u64().unwrap_or(0) as usize,
                        CursorReveal::Ensure,
                    )
                })?;
                if let Some(scroll) = panel["scroll"].as_array().filter(|value| value.len() == 2) {
                    self.session.set_scroll(
                        view,
                        scroll[0].as_f64().unwrap_or(0.0) as f32,
                        scroll[1].as_f64().unwrap_or(0.0) as f32,
                    )?;
                }
            }
        }
        if let Some(panel) = state["active_document_panel"].as_u64()
            && let Some(tab) = self.tabs.iter().find(|tab| tab.id == panel)
        {
            self.last_document = tab.panel.document();
            if let Some(view) = tab.panel.view_id() {
                self.active = Some(view);
            }
        }
        if let Some(ini) = state["ini"].as_str().filter(|ini| !ini.is_empty()) {
            let ids = state["panels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|panel| panel["id"].as_u64());
            match crate::workspace::layout::prepare(ini, ids) {
                Ok(ini) => {
                    self.pending_ini = Some(ini);
                    self.dock_built = true;
                }
                Err(error) => {
                    self.error = Some(format!(
                        "Workspace layout was invalid ({error}); restored panels with a default layout"
                    ));
                }
            }
        }
        if self.tabs.is_empty() && self.remote_ui.restore.is_empty() {
            self.show_tool(Tool::Projects);
        }
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == state["focused"].as_u64())
        {
            self.switch_to_tab(index);
        }
        Ok(())
    }
}
fn ensure_between_frames(context: &Context) -> io::Result<()> {
    let inside = context
        .binding()
        .with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).WithinFrameScope });
    if inside {
        Err(io::Error::other("Apply settings before NewFrame"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/workbench_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/workbench_context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/structure_tests.rs"]
mod structure_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/navigation_scroll_tests.rs"]
mod navigation_scroll_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/terminal_scroll_tests.rs"]
mod terminal_scroll_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/ui_animation_tests.rs"]
mod ui_animation_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/workbench_feature_tests.rs"]
mod feature_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/workspace_tests.rs"]
mod workspace_tests;
