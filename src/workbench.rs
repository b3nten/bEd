//! Bed's standalone docked workspace.
//! Documents and services belong to EditorSession; this shell owns panels and
//! layout. See LICENSE, NOTICE, UPSTREAM_REVISION and PORTING.md.
use crate::{
    files::{
        content_search::{ContentSearch, ContentSearchAction},
        file_actions,
        file_finder::{FileFinderAction, FileFinderStyle},
        file_tree::{FileTreeAction, FileTreeStyle},
        files::FileExplorer,
    },
    util::{
        icons::Icons,
        settings::Settings,
        ui_animations::UiAnimations,
        welcome::Welcome,
        workspace_state::{WorkspaceSpec, WorkspaceStore, WorkspaceTarget},
    },
};
use bed_core::{
    editor_commands::CursorReveal, editor_events::Overlay, editor_state::DocumentKind,
    editor_view_state::Selection, util::utf8::utf16_to_utf8_byte_offset,
};
use bed_highlight::{capture_map::ThemeSlot, tree_sitter::ThemeColors};
use bed_lsp::lsp_locations::LspLocation;
use bed_session::{
    editor::Editor,
    editor_session::{
        ClosePolicy, DocumentId, DocumentSnapshot, EditorSession, SessionEvent, SessionOptions,
        ViewId,
    },
};
use bed_terminal::{bed_terminal::BedTerminal, terminal_font::TerminalFonts};
use bed_ui::{
    editor_input::HostAction,
    editor_view::{EditorView, EditorViewOptions},
    lsp::lsp_ui::{ContextLspAction, LspAction, LspUi, LspView},
    views::view_layout::column_at_x,
};
use dear_imgui_rs::{
    Condition, ConfigFlags, Context, ContextBinding, ContextId, FocusedFlags, Key, MouseButton,
    StyleVar, Ui, WindowFlags, sys,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, CString},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

#[path = "plugin_host.rs"]
mod plugin_host;
#[path = "remote_workbench.rs"]
mod remote_workbench;
use plugin_host::{HostedPanel, PluginRuntime};

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
    fn name(self) -> &'static str {
        match self {
            Self::Explorer => "Files",
            Self::Settings => "Settings",
            Self::Projects => "Projects",
            Self::Search => "Search",
            Self::Diagnostics => "Diagnostics",
            Self::References => "References",
            Self::LspDashboard => "Language Servers",
        }
    }
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
    fn from_key(key: &str) -> Option<Self> {
        [
            Self::Explorer,
            Self::Settings,
            Self::Projects,
            Self::Search,
            Self::Diagnostics,
            Self::References,
            Self::LspDashboard,
        ]
        .into_iter()
        .find(|tool| tool.key() == key)
    }
}
enum Panel {
    Document(EditorView),
    Hex(bed_ui::hex_editor::HexEditor),
    Plugin(HostedPanel),
    Terminal(u64),
    Tool(Tool),
}
impl Panel {
    fn document(&self) -> Option<DocumentId> {
        match self {
            Self::Document(view) => Some(view.document_id()),
            Self::Hex(view) => Some(view.document()),
            Self::Plugin(panel) => panel.instance.attached_document(),
            _ => None,
        }
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
    pub file_explorer: FileExplorer,
    pub terminal: BedTerminal,
    pub welcome: Welcome,
    pub terminal_fonts: TerminalFonts,
    pub error: Option<String>,
    pub root_top_inset: f32,
    tabs: Vec<Tab>,
    next_tab: u64,
    active: Option<ViewId>,
    last_document: Option<DocumentId>,
    focused: Option<u64>,
    lsp_ui: LspUi,
    content_search: HashMap<u64, ContentSearch>,
    plugins: PluginRuntime,
    editor_menu_context: HashMap<ViewId, bed_plugin::CommandContext>,
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
    last_persist: Instant,
    last_state: Option<Value>,
    last_settings_check: Instant,
    last_tree_refresh: Instant,
    file_dialog: Option<FileDialog>,
    navigation_kind: ContextLspAction,
    pending_navigation: Option<ContextLspAction>,
    service_settings: Option<Value>,
    closed: bool,
    composition: Vec<PanelComposition>,
    viewport_focus: HashMap<u32, u64>,
    reload_confirmation: Option<DocumentId>,
    tab_context: Option<u64>,
    pending_tab_close: Option<Vec<u64>>,
}
impl Workbench {
    pub fn new() -> io::Result<Self> {
        Ok(Self::with_settings(Settings::new()?))
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
    pub fn with_settings(settings: Settings) -> Self {
        let (store, error) = match WorkspaceStore::load(&settings.config_dir) {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let mut welcome = Welcome::new();
        if let Some(store) = &store {
            welcome.set_recent_workspaces(store.recent_workspaces());
        }
        let now = Instant::now();
        let mut this = Self {
            settings,
            project_root: String::new(),
            workspace_spec: None,
            remote_ui: remote_workbench::RemoteUi::default(),
            session: EditorSession::new(),
            icons: Icons::default(),
            file_explorer: FileExplorer::new(),
            terminal: BedTerminal::new_empty(),
            welcome,
            terminal_fonts: TerminalFonts::default(),
            error,
            root_top_inset: 0.0,
            tabs: Vec::new(),
            next_tab: 1,
            active: None,
            last_document: None,
            focused: None,
            lsp_ui: LspUi::default(),
            content_search: HashMap::new(),
            plugins: PluginRuntime::default(),
            editor_menu_context: HashMap::new(),
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
            last_persist: now,
            last_state: None,
            last_settings_check: now,
            last_tree_refresh: now,
            file_dialog: None,
            navigation_kind: ContextLspAction::References,
            pending_navigation: None,
            service_settings: None,
            closed: false,
            composition: Vec::new(),
            viewport_focus: HashMap::new(),
            reload_confirmation: None,
            tab_context: None,
            pending_tab_close: None,
        };
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
        context
            .io_mut()
            .set_config_windows_move_from_title_bar_only(true);
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
            self.content_search.clear();
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
        f: impl FnOnce(&mut bed_session::ViewContext<'_>) -> R,
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
                matches!(tab.panel,Panel::Tool(tool) if tool.key()==kind)
                    || (kind == "terminal" && matches!(tab.panel, Panel::Terminal(_)))
                    || (kind == "document" && tab.panel.document().is_some())
                    || (kind == "hex" && matches!(tab.panel, Panel::Hex(_)))
                    || matches!(&tab.panel, Panel::Plugin(panel) if panel.kind == kind || (kind == "structure" && panel.kind == "bed.structure.panel"))
            })
            .count()
    }
    pub fn active_overlay(&self) -> Overlay {
        self.tabs
            .iter()
            .find_map(|tab| match &tab.panel {
                Panel::Document(view) if Some(view.id()) == self.active => {
                    Some(if view.find_visible() {
                        Overlay::Find
                    } else if view.line_jump_visible() {
                        Overlay::LineJump
                    } else {
                        Overlay::None
                    })
                }
                _ => None,
            })
            .unwrap_or(Overlay::None)
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
            .any(|tab| Some(tab.id) == self.focused && matches!(tab.panel, Panel::Terminal(_)))
    }
    fn clear_focus_requests(&mut self) {
        self.terminal.cancel_focus_request();
        for tab in &mut self.tabs {
            tab.focus = false;
            if let Panel::Document(view) = &tab.panel {
                let _ = self
                    .session
                    .with_view(view.id(), |editor| editor.view_mut().request_focus = false);
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
        match &tab.panel {
            Panel::Document(view) => {
                self.active = Some(view.id());
                let _ = self.session.request_focus(view.id());
            }
            Panel::Terminal(id) => {
                self.terminal.focus_session(*id);
            }
            _ => {}
        }
        true
    }
    fn push_panel(&mut self, panel: Panel) -> u64 {
        self.clear_focus_requests();
        if let Some(document) = panel.document() {
            self.last_document = Some(document);
        }
        let id = self.next_tab;
        self.next_tab += 1;
        let dock = self.largest_dock();
        if matches!(panel, Panel::Tool(Tool::Search)) {
            let mut search = ContentSearch::new_lazy();
            search.set_remote_client(self.session.remote_client());
            search.open();
            self.content_search.insert(id, search);
        }
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
        if let Panel::Terminal(terminal) = self.tabs.last().unwrap().panel {
            self.terminal.focus_session(terminal);
        }
        self.scene += 1;
        id
    }
    fn largest_dock(&self) -> u32 {
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
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| matches!(tab.panel,Panel::Tool(value)if value==tool))
        {
            self.switch_to_tab(index);
        } else {
            self.push_panel(Panel::Tool(tool));
        }
    }
    fn add_view(&mut self, document: DocumentId) -> io::Result<u64> {
        let view = EditorView::new(&mut self.session, document)?;
        self.session.request_focus(view.id())?;
        self.active = Some(view.id());
        Ok(self.push_panel(Panel::Document(view)))
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
                self.welcome
                    .set_recent_workspaces(store.recent_workspaces());
            }
            return Ok(false);
        }
        self.remote_ui.cancel_connection();
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
        self.persist_workspace()?;
        self.session.shutdown(ClosePolicy::Discard)?;
        self.close_plugin_panels()?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.lsp_ui.cancel_requests();
        self.content_search.clear();
        self.plugins = PluginRuntime::default();
        self.editor_menu_context.clear();
        self.remote_ui = remote_workbench::RemoteUi::default();
        self.file_explorer.file_finder.set_project_dir("");
        self.file_explorer.file_finder.set_remote_client(None);
        self.terminal.set_ssh_target(None);
        self.session = EditorSession::with_options(self.session_options(Some(root.clone())))?;
        self.workspace_spec = Some(spec.clone());
        self.project_root = path.clone();
        self.service_settings = None;
        self.sync_services()?;
        self.file_explorer.project_root = path.clone();
        self.file_explorer.file_tree.root_node.children.clear();
        self.restore_tree_preferences(&spec);
        self.file_explorer.file_tree.refresh_file_tree(&path)?;
        self.file_explorer.file_finder.set_project_dir(&path);
        self.terminal.set_project_root(&path);
        let restored = self
            .store
            .as_ref()
            .and_then(|store| store.layout(&spec))
            .cloned();
        if let Some(store) = &mut self.store {
            self.workspace_spec = Some(store.record_workspace(spec)?);
            self.welcome
                .set_recent_workspaces(store.recent_workspaces());
        }
        self.dock_built = false;
        self.center_dock = 0;
        self.explorer_dock = 0;
        self.terminal_dock = 0;
        self.next_tab = 1;
        self.last_state = None;
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
        let colors = ThemeColors::from_settings(&self.settings.settings);
        self.session.set_highlight_theme(colors.clone());
        let foreground = self.settings.text_color();
        let background = self.settings.background_color();
        let base = [
            background,
            colors.color(ThemeSlot::Constant),
            colors.color(ThemeSlot::String),
            colors.color(ThemeSlot::Number),
            colors.color(ThemeSlot::Function),
            colors.color(ThemeSlot::Keyword),
            colors.color(ThemeSlot::Type),
            foreground,
        ];
        self.terminal.set_theme(
            background,
            foreground,
            std::array::from_fn(|index| base[index % 8]),
        );
        Ok(())
    }
    pub fn apply_settings(&mut self, context: &mut Context) -> io::Result<bool> {
        ensure_between_frames(context)?;
        if let Some(ini) = self.pending_ini.take() {
            context.binding().with_bound_context(|| unsafe {
                sys::igLoadIniSettingsFromMemory(ini.as_ptr().cast(), ini.len());
            });
        }
        let applied = self.settings.apply(context, &mut self.scratch)?;
        if applied {
            self.terminal_fonts.reload(
                context,
                &self.settings.resources_root,
                self.settings.font_size(),
            )?;
            self.terminal
                .reload_terminal_fonts(self.settings.font_size());
            self.sync_services()?;
            self.scene += 1;
        }
        Ok(applied)
    }
    pub fn tick(&mut self) -> io::Result<()> {
        self.poll_remote_workspace()?;
        let report = self.session.tick();
        if self.session.is_remote() {
            for event in &report.events {
                if let SessionEvent::Opened { document } = event {
                    self.finish_remote_open(*document)?;
                }
            }
            self.finish_remote_restoration()?;
            self.queue_remote_directories(false)?;
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
        self.file_explorer.poll();
        for search in self.content_search.values_mut() {
            search.poll();
        }
        self.tick_plugins()?;
        if self.last_settings_check.elapsed() > Duration::from_millis(500) {
            self.last_settings_check = Instant::now();
            self.settings.check_settings_file();
        }
        if !self.project_root.is_empty()
            && self.last_tree_refresh.elapsed() > Duration::from_secs(2)
        {
            self.last_tree_refresh = Instant::now();
            if self.session.is_remote() {
                self.queue_remote_directories(true)?;
            } else {
                self.file_explorer
                    .file_tree
                    .refresh_file_tree(&self.project_root)?;
            }
        }
        let session = &self.session;
        self.lsp_ui
            .retain_requests(|origin| session.accepts_lsp_origin(origin));
        self.finish_navigation_request()
    }
    fn new_terminal(&mut self) {
        let root = if self.project_root.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            PathBuf::from(&self.project_root)
        };
        let id = self.terminal.new_session_at(root);
        self.push_panel(Panel::Terminal(id));
    }
    pub fn dispatch(&mut self, command: WindowCommand) -> io::Result<bool> {
        match command {
            WindowCommand::NewDocument => {
                if self.project_root.is_empty() {
                    self.session.configure(self.session_options(None))?;
                }
                let document = self.session.create_document(b"")?;
                self.add_view(document)?;
            }
            WindowCommand::NewTerminal => self.new_terminal(),
            WindowCommand::NewExplorer => {
                self.push_panel(Panel::Tool(Tool::Explorer));
            }
            WindowCommand::NewSettings => {
                self.push_panel(Panel::Tool(Tool::Settings));
            }
            WindowCommand::NewProjects => {
                self.push_panel(Panel::Tool(Tool::Projects));
            }
            WindowCommand::NewDiagnostics => {
                self.push_panel(Panel::Tool(Tool::Diagnostics));
            }
            WindowCommand::NewStructure => {
                self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)?;
            }
            WindowCommand::NewLspDashboard => {
                self.push_panel(Panel::Tool(Tool::LspDashboard));
            }
            WindowCommand::NewContentSearch => {
                self.push_panel(Panel::Tool(Tool::Search));
            }
            WindowCommand::NewReferences => {
                self.push_panel(Panel::Tool(Tool::References));
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
                    let panel = self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
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
                self.scene += 1;
            }
            WindowCommand::Projects => self.show_tool(Tool::Projects),
            WindowCommand::Diagnostics => self.show_tool(Tool::Diagnostics),
            WindowCommand::Structure => {
                if let Some(index) = self.tabs.iter().position(|tab| matches!(&tab.panel, Panel::Plugin(panel) if panel.kind == "bed.structure.panel")) { self.switch_to_tab(index); }
                else { self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)?; }
            },
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
                if let Some(index) = self.active_tab_index()
                    && let Panel::Document(view) = &mut self.tabs[index].panel
                {
                    view.open_find(&mut self.session)?;
                    self.tabs[index].focus = true;
                }
            }
            WindowCommand::GoToLine => {
                if let Some(index) = self.active_tab_index()
                    && let Panel::Document(view) = &mut self.tabs[index].panel
                {
                    view.open_line_jump();
                    self.tabs[index].focus = true;
                }
            }
            WindowCommand::Explorer => self.show_tool(Tool::Explorer),
            WindowCommand::Terminal => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| matches!(tab.panel, Panel::Terminal(_)))
                {
                    self.switch_to_tab(index);
                } else {
                    self.new_terminal();
                }
            }
            WindowCommand::Settings => self.show_tool(Tool::Settings),
            WindowCommand::LspDashboard => self.show_tool(Tool::LspDashboard),
            WindowCommand::FindFile => {
                self.file_explorer.file_finder.toggle_window();
            }
            WindowCommand::FindProject => {
                self.show_tool(Tool::Search);
                if let Some(search) = self.focused.and_then(|id| self.content_search.get_mut(&id)) {
                    search.open();
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
            if self.active_view().is_some_and(|view| self.session.document_for_view(view) == Some(document))
                && let Some(index) = self.tabs.iter().position(|tab| matches!(&tab.panel, Panel::Document(view) if Some(view.id()) == self.active)) {
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
            .and_then(|tab| match &tab.panel {
                Panel::Document(_) => Some("bed.text".to_owned()),
                Panel::Hex(_) => Some("bed.hex".to_owned()),
                Panel::Plugin(panel) => panel.viewer.clone(),
                _ => None,
            })
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
                if self.remote_ui.document_mutating(document) {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Wait for the remote file action before saving",
                    ));
                }
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
        if !snapshot.path.is_empty() {
            dialog = dialog.set_file_name(
                Path::new(&snapshot.path)
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
        let _style = crate::util::context_menu_style(ui);
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
        let tab = self.tabs.remove(index);
        self.content_search.remove(&tab.id);
        let document = tab.panel.document();
        match tab.panel {
            Panel::Document(view) => {
                self.session.detach_view(view.id());
                self.editor_menu_context.remove(&view.id());
                if self.active == Some(view.id()) {
                    self.active = None;
                }
            }
            Panel::Terminal(id) => {
                self.terminal.close_session_id(id);
            }
            Panel::Hex(_) => {}
            Panel::Plugin(mut panel) => panel.instance.close(&mut self.plugins.requests),
            Panel::Tool(_) => {}
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
        let indices = (0..self.tabs.len()).collect::<Vec<_>>();
        if !self.preflight_close(&indices)? {
            return Ok(false);
        }
        self.persist_workspace()?;
        self.session.shutdown(ClosePolicy::Discard)?;
        self.close_plugin_panels()?;
        self.terminal.shutdown();
        self.tabs.clear();
        self.pending_tab_close = None;
        self.content_search.clear();
        self.plugins = PluginRuntime::default();
        self.editor_menu_context.clear();
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
        let name = match &tab.panel {
            Panel::Document(view) => self
                .session
                .snapshot(view.document_id())
                .map(|doc| {
                    if doc.path.is_empty() {
                        "Untitled".to_owned()
                    } else {
                        Path::new(&doc.path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    }
                })
                .unwrap_or_else(|_| "Closed".to_owned()),
            Panel::Terminal(id) => self
                .terminal
                .session_title(*id)
                .unwrap_or_else(|| format!("Terminal {id}")),
            Panel::Hex(view) => self
                .session
                .with_document(view.document(), |state| {
                    format!(
                        "{} [Hex]",
                        Path::new(&state.path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    )
                })
                .unwrap_or_else(|_| "Closed".into()),
            Panel::Plugin(panel) => panel.instance.title(&self.plugins.frame.context()),
            Panel::Tool(tool) => tool.name().to_owned(),
        };
        format!("{name}###bed_tab_{}", tab.id)
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
        if self.dock_built {
            return;
        }
        unsafe {
            sys::igDockBuilderRemoveNode(self.dock_root);
            sys::igDockBuilderAddNode(self.dock_root, sys::ImGuiDockNodeFlags_DockSpace);
            sys::igDockBuilderSetNodeSize(self.dock_root, size.into());
            let mut center = self.dock_root;
            let mut left = 0;
            let mut rest = 0;
            if self
                .tabs
                .iter()
                .any(|tab| matches!(tab.panel, Panel::Tool(Tool::Explorer)))
            {
                sys::igDockBuilderSplitNode(center, sys::ImGuiDir_Left, 0.2, &mut left, &mut rest);
                center = rest;
            }
            let mut bottom = 0;
            if self
                .tabs
                .iter()
                .any(|tab| matches!(tab.panel, Panel::Terminal(_)))
            {
                sys::igDockBuilderSplitNode(
                    center,
                    sys::ImGuiDir_Down,
                    0.25,
                    &mut bottom,
                    &mut rest,
                );
                center = rest;
            }
            self.center_dock = center;
            self.explorer_dock = if left != 0 { left } else { center };
            self.terminal_dock = if bottom != 0 { bottom } else { center };
            for tab in &mut self.tabs {
                tab.dock = Some(match tab.panel {
                    Panel::Tool(Tool::Explorer) => self.explorer_dock,
                    Panel::Terminal(_) => self.terminal_dock,
                    _ => self.center_dock,
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
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
        let _workspace_border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        let mut dialog_result = Ok(());
        ui.set_next_window_viewport(ui.main_viewport().id());
        ui.window("##bed_workspace")
            .position([pos[0], pos[1] + self.root_top_inset], Condition::Always)
            .size(size, Condition::Always)
            .flags(
                WindowFlags::NO_DECORATION
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
                let _menu_style = crate::util::context_menu_style(ui);
                unsafe {
                    sys::igDockSpace(self.dock_root, [0.0; 2].into(), 0, std::ptr::null());
                }
                drop(_menu_style);
                // Popup IDs belong to their submitting window. Keep file
                // actions rooted here even when their source tab is hidden or
                // an embedding host changes the enclosing window.
                dialog_result = self.draw_file_dialog(ui);
            });
        drop(_workspace_border);
        drop(_rounding);
        drop(_padding);
        dialog_result?;
        let mut close = Vec::new();
        let count = self.tabs.len();
        let mut tree_actions = Vec::new();
        let mut project_action = None;
        let mut workspace_action = None;
        let mut reconnect = false;
        let mut search_actions = Vec::new();
        let mut lsp_actions = Vec::new();
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
            let _panel_padding = match &tab.panel {
                Panel::Document(_) => Some(ui.push_style_var(StyleVar::WindowPadding([0.0; 2]))),
                Panel::Tool(Tool::Explorer) => {
                    Some(ui.push_style_var(StyleVar::WindowPadding([2.0; 2])))
                }
                _ => None,
            };
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
                        if let Some(document) = tab.panel.document() { self.last_document = Some(document); }
                        if let Panel::Document(view) = &tab.panel {
                            self.active = Some(view.id());
                        }
                    }
                    unsafe {
                        tab.viewport = (*sys::igGetWindowViewport()).ID;
                        tab.dock_id = sys::igGetWindowDockID();
                    }
                    result = match &mut tab.panel {
                        Panel::Document(view) => self.draw_document(ui, view, &mut actions),
                        Panel::Hex(view) => view.draw(ui, &mut self.session),
                        Panel::Plugin(panel) => self.refresh_plugins().map(|()| {
                            let host = self.plugins.frame.context();
                            panel.instance.draw(ui, &host, &mut self.plugins.requests);
                        }),
                        Panel::Terminal(id) => self
                            .terminal
                            .render_session(ui, &self.terminal_fonts, *id)
                            .map(|_| ()),
                        Panel::Tool(Tool::Explorer) => {
                            if self.project_root.is_empty() {
                                ui.text_disabled("Open a project to browse files");
                                if ui.button("Open Folder") {
                                    project_action = Some(PathBuf::new());
                                }
                            } else {
                                let active_path = self
                                    .active_snapshot()
                                    .map(|doc| doc.path)
                                    .unwrap_or_default();
                                let style = FileTreeStyle {
                                    text_color: self.settings.text_color(),
                                    rainbow: self.settings.rainbow(),
                                    rainbow_time: ui.time() as f32,
                                    animations: self.settings.bool("ui_animations", true),
                                };
                                let git = self.active.and_then(|view| {
                                    self.session
                                        .with_view(view, |editor| Rc::clone(&editor.git))
                                        .ok()
                                });
                                let remote = self.session.is_remote();
                                let session = &self.session;
                                let modified = |path: &str| {
                                    if remote {
                                        session.remote_file_modified(path)
                                    } else {
                                        git.as_ref()
                                            .is_some_and(|git| git.borrow().is_file_modified(path))
                                    }
                                };
                                let plugins = &self.plugins;
                                let extensions = |ui: &Ui, path: &str, directory: bool, background: bool, actions: &mut Vec<FileTreeAction>| {
                                    Self::draw_tree_plugin_menu(plugins, ui, path, directory, background, actions);
                                };
                                tree_actions.extend(
                                    self.file_explorer.file_tree.display_backend_actions_with_menu(
                                        ui,
                                        &active_path,
                                        &style,
                                        Some(&self.icons),
                                        Some(&modified),
                                        remote,
                                        Some(&extensions),
                                    ),
                                );
                            }
                            Ok(())
                        }
                        Panel::Tool(Tool::Projects) => {
                            if self.remote_ui.connecting() {
                                ui.text_disabled("Connecting over SSH…");
                            }
                            if self.session.is_remote() && !self.session.remote_connected() {
                                ui.text_disabled("SSH connection lost. Your buffers are retained.");
                                reconnect = ui.button("Reconnect");
                            }
                            let action = self.welcome.draw_body(ui);
                            if action.open_folder {
                                project_action = Some(PathBuf::new());
                            }
                            if let Some(project) = action.project {
                                project_action = Some(project);
                            }
                            workspace_action = action
                                .workspace
                                .or(action.connect)
                                .or(workspace_action.take());
                            if let Some(project) = action.remove_recent
                                && let Some(store) = &mut self.store
                            {
                                if let Err(error) = store.forget_project(&project) {
                                    self.error = Some(error.to_string());
                                }
                                self.welcome
                                    .set_recent_workspaces(store.recent_workspaces());
                            }
                            if let Some(spec) = action.remove_workspace
                                && let Some(store) = &mut self.store
                            {
                                if let Err(error) = store.forget_workspace(&spec) {
                                    self.error = Some(error.to_string());
                                }
                                self.welcome
                                    .set_recent_workspaces(store.recent_workspaces());
                            }
                            if let Some((spec, name)) = action.rename_workspace
                                && let Some(store) = &mut self.store
                            {
                                match store.rename_workspace(&spec, &name) {
                                    Ok(renamed) => {
                                        if self.workspace_spec.as_ref().is_some_and(|active| {
                                            active.identity() == renamed.identity()
                                        }) {
                                            self.workspace_spec = Some(renamed);
                                        }
                                    }
                                    Err(error) => self.error = Some(error.to_string()),
                                }
                                self.welcome
                                    .set_recent_workspaces(store.recent_workspaces());
                            }
                            if let Some(error) = action.error {
                                self.error = Some(error);
                            }
                            Ok(())
                        }
                        Panel::Tool(Tool::Settings) => {
                            let plugins = &mut self.plugins;
                            self.settings.draw_tab_with_extensions(ui, &mut self.scratch, Some(&self.icons), &mut |ui, settings| {
                                let mut changed = false;
                                if !settings["plugins"].is_object() { settings["plugins"] = json!({}); }
                                for section in &plugins.registry.settings {
                                    let _id = ui.push_id(section.id);
                                    if ui.collapsing_header(section.label, dear_imgui_rs::TreeNodeFlags::empty())
                                        && let Some(plugin) = plugins.instances.iter_mut().find(|plugin| plugin.id() == section.plugin) {
                                        let namespace = &mut settings["plugins"][section.plugin];
                                        if !namespace.is_object() { *namespace = json!({}); }
                                        changed |= plugin.draw_settings(section.id, ui, namespace);
                                    }
                                }
                                changed
                            });
                            Ok(())
                        }
                        Panel::Tool(Tool::Search) => {
                            if let Some(search) = self.content_search.get_mut(&tab.id) {
                                search_actions.push(search.draw_body(ui, &self.project_root));
                            }
                            Ok(())
                        }
                        Panel::Tool(Tool::Diagnostics) => {
                            if let Some(action) = self.draw_diagnostics(ui) {
                                lsp_actions.push(action);
                            }
                            Ok(())
                        }
                        Panel::Tool(Tool::References) => {
                            if let Some(action) =
                                self.lsp_ui.render_navigation_body(ui, self.navigation_kind)
                            {
                                lsp_actions.push(action);
                            }
                            Ok(())
                        }
                        Panel::Tool(Tool::LspDashboard) => {
                            if let Some(pool) = self.session.lsp_mut() {
                                if let Some(action) =
                                    self.lsp_ui.render_workspace_dashboard_body(ui, pool)
                                {
                                    lsp_actions.push(action);
                                }
                            } else {
                                ui.text_disabled("Language servers start when a project is open");
                            }
                            Ok(())
                        }
                    };
                });
            drop(_panel_padding);
            self.tabs.insert(index, tab);
            result?;
            if !opened {
                close.push(index);
            }
        }
        self.finish_navigation_request()?;
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
        if let Some(error) = self.file_explorer.file_tree.error.take() {
            self.error = Some(error);
        }
        for action in tree_actions {
            if let Err(error) = self.handle_tree_action(action) {
                self.error = Some(error.to_string());
            }
        }
        if let Some(path) = project_action {
            let result = if path.as_os_str().is_empty() {
                self.dispatch(WindowCommand::OpenFolder)
            } else {
                self.set_project(&path)
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        if let Some(spec) = workspace_action
            && let Err(error) = self.set_workspace(spec)
        {
            self.error = Some(error.to_string());
        }
        if reconnect && let Err(error) = self.reconnect_workspace() {
            self.error = Some(error.to_string());
        }
        for action in search_actions {
            let ContentSearchAction::Open(found) = action else {
                continue;
            };
            if let Err(error) =
                self.navigate_file(&found.file.full_path, found.row, found.column, false)
            {
                self.error = Some(error.to_string());
            }
        }
        for action in lsp_actions {
            if let Err(error) = self.open_lsp_action(action) {
                self.error = Some(error.to_string());
            }
        }
        let style = FileFinderStyle {
            background_color: self.settings.background_color(),
            embedded_pane: None,
        };
        if let FileFinderAction::Open(path) =
            self.file_explorer
                .file_finder
                .render_window(ui, &style, Some(&self.icons))
            && let Err(error) = self.open_or_focus(Path::new(&path))
        {
            self.error = Some(error.to_string());
        }
        self.draw_remote_path_dialog(ui)?;
        {
            let host = self.plugins.frame.context();
            for plugin in &mut self.plugins.instances {
                let _id = ui.push_id(plugin.id());
                plugin.draw_popups(ui, &host, &mut self.plugins.requests);
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
        if ui.io().want_text_input() {
            return Ok(());
        }
        let ctrl = ui.io().key_ctrl() || ui.io().key_super();
        if !ctrl {
            return Ok(());
        }
        if self.focused_hex() && ui.is_key_pressed_with_repeat(Key::S, false) {
            self.handle_action(if ui.io().key_shift() {
                HostAction::SaveAs
            } else {
                HostAction::Save
            })?;
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
    fn draw_document(
        &mut self,
        ui: &Ui,
        view: &mut EditorView,
        actions: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if let Some(activity) = self.document_lsp_activity(view.document_id()) {
            ui.text_disabled(&activity);
            if ui.is_item_hovered() {
                ui.tooltip_text(&activity);
            }
        }
        let options = EditorViewOptions {
            font: None,
            rainbow_mode: self.settings.rainbow(),
            minimap_enabled: self.settings.bool("minimap", true),
            background_color: Some(self.settings.background_color()),
            line_jump_key: self.settings.keybinds.get_action_key("line_jump_key"),
            block_input: self
                .file_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.visible)
                || self.reload_confirmation.is_some()
                || self.file_explorer.file_finder.show_ff_window,
            ..Default::default()
        };
        view.set_navigation_animations(self.settings.bool("ui_animations", true));
        let response = view.draw(ui, &mut self.session, &options)?;
        if response.focused {
            self.active = Some(view.id());
        }
        if let Some(request) = response.definition_request {
            self.request_lsp_at(view, 0, Some((request.row, request.column)))?;
        }
        actions.extend(response.actions);
        self.text_context_menu(ui, view)?;
        let session = &self.session;
        self.lsp_ui
            .retain_requests(|origin| session.accepts_lsp_origin(origin));
        if let Some(client) = self
            .session
            .lsp()
            .and_then(|pool| pool.client_for_document(view.document_id()))
        {
            let snapshot = self.session.snapshot(view.document_id())?;
            let view_id = view.id();
            let origin = self.session.lsp().and_then(|pool| {
                pool.request_origin(view.document_id(), view_id, snapshot.generation, 0)
            });
            let frame = view.presentation();
            let layout = frame.layout;
            let hover = frame.hover_info;
            let dismissed = frame.hover_dismissed;
            if response.focused
                && let Some(origin) = origin
            {
                let requested = self.session.with_view(view_id, |editor| {
                    self.lsp_ui.keybinds_with_origin(
                        ui,
                        &mut client.borrow_mut(),
                        editor,
                        &self.settings.lsp_presentation(),
                        origin,
                    )
                })?;
                if requested {
                    for kind in [ContextLspAction::Definition, ContextLspAction::References] {
                        if self.lsp_ui.navigation_results(kind).is_some_and(|results| {
                            results.origin.is_some_and(|request| {
                                request.view_id == origin.view_id
                                    && request.document_id == origin.document_id
                                    && request.version == origin.version
                            }) && results.pending
                        }) {
                            self.pending_navigation = Some(kind);
                        }
                    }
                }
            }
            let mouse = ui.io().mouse_pos();
            let hover_here = mouse[0] >= layout.pane_pos[0]
                && mouse[0] < layout.pane_pos[0] + layout.pane_size[0]
                && mouse[1] >= layout.pane_pos[1]
                && mouse[1] < layout.pane_pos[1] + layout.pane_size[1];
            let caret_here = self.lsp_ui.symbol_at_caret()
                && self
                    .lsp_ui
                    .symbol_origin()
                    .is_some_and(|request| request.view_id == view_id);
            let over_symbol_popup = self.lsp_ui.symbol_popup_contains(mouse);
            let popup_here = over_symbol_popup
                && self
                    .lsp_ui
                    .symbol_origin()
                    .is_some_and(|request| request.view_id == view_id);
            if caret_here
                || (!self.lsp_ui.symbol_at_caret()
                    && (popup_here || (hover_here && !over_symbol_popup)))
            {
                self.lsp_ui.set_mouse_origin(origin);
                self.session.with_view(view_id, |editor| {
                    self.lsp_ui.render_hover(
                        ui,
                        &mut client.borrow_mut(),
                        editor,
                        &self.settings.lsp_presentation(),
                        LspView {
                            layout: &layout,
                            hover_info: hover,
                            hover_dismissed: dismissed,
                            tooltip_arbiter: frame.tooltip_arbiter,
                        },
                    )
                })?;
            }
        }
        Ok(())
    }
    fn document_lsp_activity(&self, document: DocumentId) -> Option<String> {
        let client = self.session.lsp()?.client_for_document(document)?;
        let client = client.borrow();
        if !client.is_process_started() {
            return None;
        }
        let language = client.current_language();
        if let Some(progress) = client.progress().into_iter().find(|job| !job.finished) {
            let mut text = format!("{language}: {}", progress.title);
            if let Some(message) = progress
                .message
                .filter(|message| !message.is_empty() && *message != progress.title)
            {
                text.push_str(" — ");
                text.push_str(&message);
            }
            if let Some(percentage) = progress.percentage {
                text.push_str(&format!(" ({percentage}%)"));
            }
            return Some(text);
        }
        (!client.is_initialized()).then(|| format!("{language}: starting language server…"))
    }
    fn text_context_menu(&mut self, ui: &Ui, view: &mut EditorView) -> io::Result<()> {
        let layout = view.presentation().layout;
        let mouse = ui.io().mouse_pos();
        let inside = mouse[0] >= layout.text_pos[0]
            && mouse[0] < layout.pane_pos[0] + layout.pane_size[0]
            && mouse[1] >= layout.pane_pos[1]
            && mouse[1] < layout.pane_pos[1] + layout.pane_size[1];
        let name = format!("##text_actions_{}", view.id().0);
        if inside && ui.is_mouse_clicked(MouseButton::Right) {
            let state = self.session.view_snapshot(view.id())?;
            let row =
                ((mouse[1] - layout.text_pos[1]) / layout.line_height.max(1.0)).floor() as i32;
            let (row, column) = self.session.with_document(view.document_id(), |document| {
                let row = row.clamp(0, document.line_count() - 1);
                (
                    row,
                    column_at_x(ui, &document.line(row), mouse[0] - layout.text_pos[0]),
                )
            })?;
            let selected = state.selections.iter().any(|selection| {
                let (ar, ac, br, bc) = selection.ordered();
                !selection.empty() && (row, column) >= (ar, ac) && (row, column) <= (br, bc)
            });
            if !selected {
                self.session.with_commands(view.id(), |commands| {
                    commands.set_cursor(row, column, false, CursorReveal::Ensure)
                })?;
            }
            self.active = Some(view.id());
            self.capture_editor_context(view)?;
            ui.open_popup(&name);
        }
        let _menu_style = crate::util::context_menu_style(ui);
        if let Some(_popup) = ui.begin_popup(&name) {
            if let Some(activity) = self.document_lsp_activity(view.document_id()) {
                ui.text_disabled(activity);
            }
            let ready = self
                .session
                .lsp()
                .and_then(|pool| pool.client_for_document(view.document_id()))
                .is_some_and(|client| client.borrow().is_initialized());
            if let Some(action) = text_lsp_actions(ui, ready) {
                self.request_lsp(view, action)?;
            }
            if !ready && ui.menu_item("Language Servers…") {
                self.show_tool(Tool::LspDashboard);
            }
            ui.separator();
            self.draw_editor_plugin_menu(ui, view)?;
            let minimap = view.minimap_enabled(self.settings.bool("minimap", true));
            if ui.menu_item_enabled_selected_no_shortcut("Show Minimap", minimap, true) {
                view.set_minimap_enabled(!minimap);
            }
            ui.separator();
            if ui.menu_item("Undo") {
                self.session
                    .with_commands(view.id(), |commands| commands.undo())?;
            }
            if ui.menu_item("Redo") {
                self.session
                    .with_commands(view.id(), |commands| commands.redo())?;
            }
            ui.separator();
            if ui.menu_item("Cut") {
                let bytes = self
                    .session
                    .with_commands(view.id(), |commands| commands.cut())?;
                set_clipboard(ui, &bytes);
            }
            if ui.menu_item("Copy") {
                let bytes = self
                    .session
                    .with_commands(view.id(), |commands| commands.copy())?;
                set_clipboard(ui, &bytes);
            }
            if ui.menu_item("Paste")
                && let Some(text) = get_clipboard(ui)
            {
                self.session
                    .with_commands(view.id(), |commands| commands.paste(text.as_bytes()))?;
            }
            if ui.menu_item("Select All") {
                self.session
                    .with_commands(view.id(), |commands| commands.select_all())?;
            }
        }
        Ok(())
    }
    fn request_lsp(&mut self, view: &EditorView, action: u8) -> io::Result<()> {
        self.request_lsp_at(view, action, None)
    }
    fn request_lsp_at(
        &mut self,
        view: &EditorView,
        action: u8,
        position: Option<(i32, i32)>,
    ) -> io::Result<()> {
        let kind = match action {
            0 => ContextLspAction::Definition,
            1 => ContextLspAction::References,
            _ => ContextLspAction::Symbol,
        };
        if let Some(client) = self
            .session
            .lsp()
            .and_then(|pool| pool.client_for_document(view.document_id()))
        {
            let snapshot = self.session.snapshot(view.document_id())?;
            if let Some(origin) = self.session.lsp().and_then(|pool| {
                pool.request_origin(view.document_id(), view.id(), snapshot.generation, 0)
            }) {
                let requested = self.session.with_view(view.id(), |editor| {
                    if let Some((row, column)) = position {
                        self.lsp_ui.request_at_position(
                            kind,
                            &mut client.borrow_mut(),
                            editor,
                            origin,
                            row,
                            column,
                        )
                    } else {
                        self.lsp_ui
                            .request_at(kind, &mut client.borrow_mut(), editor, origin)
                    }
                })?;
                if requested && kind != ContextLspAction::Symbol {
                    self.pending_navigation = Some(kind);
                }
            }
        }
        Ok(())
    }
    fn finish_navigation_request(&mut self) -> io::Result<()> {
        let Some(kind) = self.pending_navigation else {
            return Ok(());
        };
        let Some(results) = self.lsp_ui.navigation_results(kind) else {
            self.pending_navigation = None;
            return Ok(());
        };
        if results.origin.is_none() {
            self.pending_navigation = None;
            return Ok(());
        }
        if kind == ContextLspAction::Definition && results.pending {
            return Ok(());
        }
        self.pending_navigation = None;
        if kind == ContextLspAction::Definition
            && let Some(action) = self.lsp_ui.take_single_definition()
        {
            self.open_lsp_action(action)?;
        } else {
            self.navigation_kind = kind;
            self.show_tool(Tool::References);
        }
        Ok(())
    }
    fn draw_diagnostics(&self, ui: &Ui) -> Option<LspAction> {
        let mut action = None;
        if let Some(pool) = self.session.lsp() {
            for document in self.session.document_ids() {
                let Ok(snapshot) = self.session.snapshot(document) else {
                    continue;
                };
                if let Some(store) = pool.diagnostics_for_document(document) {
                    for item in store.for_document(&snapshot.path) {
                        if ui.selectable(format!(
                            "{}:{}  {}",
                            snapshot.path,
                            item.start_line + 1,
                            item.message
                        )) {
                            action = Some(LspAction::OpenLocation(LspLocation {
                                file: snapshot.path.clone(),
                                line: item.start_line,
                                character: item.start_character,
                            }));
                        }
                    }
                }
            }
        }
        if action.is_none() {
            ui.text_disabled("Diagnostics from open documents appear here");
        }
        action
    }
    fn open_lsp_action(&mut self, action: LspAction) -> io::Result<()> {
        match action {
            LspAction::OpenConfig(path) => {
                if self.session.is_remote() {
                    return Err(io::Error::other(format!(
                        "Language server configuration is local to this computer: {}. Open it in a local workspace, then reconnect.",
                        path.display()
                    )));
                }
                self.open_or_focus(&path)?;
            }
            LspAction::RestartServer(language) => {
                if let Some(pool) = self.session.lsp_mut() {
                    pool.retry_language(&language)?;
                }
            }
            LspAction::OpenLocation(location) => {
                let origin = self.lsp_ui.take_navigation_origin();
                if let Some(origin) = origin {
                    if !self.session.accepts_lsp_origin(&origin) {
                        return Ok(());
                    }
                    if let Some(index) = self.tabs.iter().position(
                        |tab| matches!(&tab.panel,Panel::Document(view)if view.id()==origin.view_id),
                    ) {
                        self.switch_to_tab(index);
                    }
                }
                self.navigate_file(&location.file, location.line, location.character, true)?;
            }
        }
        Ok(())
    }
    fn restore_tree_preferences(&mut self, spec: &WorkspaceSpec) {
        let tree = &mut self.file_explorer.file_tree;
        tree.preferences = self
            .store
            .as_ref()
            .map(|store| store.tree_preferences(spec))
            .unwrap_or_default();
        tree.show_hidden = false;
        tree.error = None;
    }
    fn handle_tree_action(&mut self, action: FileTreeAction) -> io::Result<()> {
        if let FileTreeAction::Command { command, context } = &action {
            if let Some(viewer) = command.strip_prefix("bed.open_with:") {
                if let Some(path) = &context.path {
                    self.open_file_with_viewer(Path::new(path), Some(viewer), true)?;
                }
            } else {
                self.refresh_plugins()?;
                self.plugins.command(command, context)?;
                self.process_plugin_requests()?;
            }
            return Ok(());
        }
        if self
            .file_explorer
            .file_tree
            .apply_visibility_action(&action, self.session.is_remote())
        {
            if !matches!(action, FileTreeAction::SetShowHidden(_))
                && let (Some(store), Some(spec)) = (&mut self.store, &self.workspace_spec)
            {
                store.set_tree_preferences(spec, &self.file_explorer.file_tree.preferences)?;
            }
            if self.session.is_remote() {
                self.queue_remote_directories(true)?;
            } else {
                self.file_explorer
                    .file_tree
                    .refresh_file_tree(&self.project_root)?;
            }
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
        let _dialog_style = crate::util::dialog_style(ui);
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
                    && self.session.snapshot(other)?.path == destination.to_string_lossy()
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
        self.refresh_files()?;
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
        for (id, _) in affected {
            self.session.invalidate_removed_path(id)?;
        }
        self.refresh_files()
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
                let snapshot = self.session.snapshot(id).ok()?;
                let within = if self.session.is_remote() {
                    source.to_str().is_some_and(|source| {
                        snapshot.path == source
                            || snapshot
                                .path
                                .strip_prefix(source.trim_end_matches('/'))
                                .is_some_and(|suffix| suffix.starts_with('/'))
                    })
                } else {
                    Path::new(&snapshot.path).starts_with(source)
                };
                within.then_some((id, PathBuf::from(snapshot.path)))
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
                self.refresh_files()?;
                self.open_or_focus(&path)?;
            }
            FileTreeAction::NewFolder(directory) => {
                self.validate_directory(directory)?;
                file_actions::create_folder(Path::new(directory), name)?;
                self.refresh_files()?;
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
        if self.session.is_remote() {
            return self.refresh_remote_files();
        }
        self.file_explorer
            .file_tree
            .refresh_file_tree(&self.project_root)?;
        self.file_explorer
            .file_finder
            .set_project_dir(&self.project_root);
        for search in self.content_search.values_mut() {
            search.cancel();
        }
        Ok(())
    }
    fn draw_errors(&mut self, ui: &Ui) -> io::Result<()> {
        let _dialog_style = crate::util::dialog_style(ui);
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
        for tab in &mut self.tabs {
            if let Panel::Hex(view) = &mut tab.panel {
                view.synchronize(&self.session)?;
            }
        }
        if self.store.is_none() || self.remote_ui.connecting() || !self.remote_ui.restore.is_empty()
        {
            return Ok(());
        }
        // An unapplied layout is still authoritative when closing before the
        // first frame. Never replace it with a fresh context's empty settings.
        let ini = self.pending_ini.clone().unwrap_or_else(|| {
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
            let value = match &tab.panel {
                Panel::Document(view) => {
                    let snapshot = self.session.snapshot(view.document_id())?;
                    if snapshot.path.is_empty() {
                        continue;
                    }
                    let state = self.session.view_snapshot(view.id())?;
                    json!({"kind":"document","path":snapshot.path,"selections":state.selections.iter().map(|s|[s.head_row,s.head_column,s.anchor_row,s.anchor_column]).collect::<Vec<_>>(),"primary":state.primary_index,"scroll":state.scroll_position})
                }
                Panel::Terminal(id) => {
                    json!({"kind":"terminal","cwd":self.terminal.working_directory(*id)})
                }
                Panel::Hex(view) => {
                    let doc = self.session.snapshot(view.document())?;
                    if doc.path.is_empty() {
                        continue;
                    }
                    json!({"kind":"hex","path":doc.path,"viewer":"bed.hex","document_kind":"bytes","state":view.state()})
                }
                Panel::Plugin(panel) => {
                    let doc = panel
                        .instance
                        .attached_document()
                        .map(|id| self.session.snapshot(id))
                        .transpose()?;
                    if doc.as_ref().is_some_and(|doc| doc.path.is_empty()) {
                        continue;
                    }
                    json!({"kind":"plugin","panel_type":panel.kind,"viewer":panel.viewer,
                        "path":doc.as_ref().map(|doc|doc.path.as_str()),"document_kind":doc.as_ref().map(|doc|if doc.kind == DocumentKind::Bytes {"bytes"} else {"text"}),"state":panel.instance.save_state()})
                }
                Panel::Tool(tool) => json!({"kind":tool.key()}),
            };
            let mut value = value;
            value["id"] = json!(tab.id);
            panels.push(value);
        }
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
        self.remote_ui.restored_active = state["active_document_panel"].as_u64();
        self.remote_ui.restored_focus = state["focused"].as_u64();
        if self.session.is_remote() {
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
                        self.session
                            .open_file(Path::new(path))
                            .and_then(|doc| self.add_view(doc))
                    } else {
                        continue;
                    }
                }
                "terminal" => {
                    let root = panel["cwd"]
                        .as_str()
                        .filter(|path| self.session.is_remote() || Path::new(path).is_dir())
                        .unwrap_or(&self.project_root);
                    let id = self.terminal.new_session_at(root);
                    Ok(self.push_panel(Panel::Terminal(id)))
                }
                _ => {
                    if let Some(tool) = Tool::from_key(kind) {
                        Ok(self.push_panel(Panel::Tool(tool)))
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
                if let Some(search) = self.content_search.remove(&tab.id) {
                    self.content_search.insert(id, search);
                }
                tab.id = id;
                self.next_tab = self.next_tab.max(id + 1);
            }
            if let Panel::Document(view) = &tab.panel {
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
                self.session.with_commands(view.id(), |commands| {
                    commands.set_selections(
                        selections,
                        panel["primary"].as_u64().unwrap_or(0) as usize,
                        CursorReveal::Ensure,
                    )
                })?;
                if let Some(scroll) = panel["scroll"].as_array().filter(|value| value.len() == 2) {
                    self.session.set_scroll(
                        view.id(),
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
            if let Panel::Document(view) = &tab.panel {
                self.active = Some(view.id());
            }
        }
        if let Some(ini) = state["ini"].as_str().filter(|ini| !ini.is_empty()) {
            self.pending_ini = Some(ini.to_owned());
            self.dock_built = true;
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

fn set_clipboard(ui: &Ui, bytes: &[u8]) {
    if let Ok(text) = CString::new(String::from_utf8_lossy(bytes).as_bytes()) {
        ui.with_bound_context(|| unsafe {
            sys::igSetClipboardText(text.as_ptr());
        });
    }
}
fn text_lsp_actions(ui: &Ui, ready: bool) -> Option<u8> {
    let mut action = None;
    for (title, command) in [
        ("Go to Definition", 0),
        ("Find References", 1),
        ("Symbol Info", 2),
    ] {
        if ui.menu_item_enabled_selected_no_shortcut(title, false, ready) {
            action = Some(command);
        }
    }
    action
}
fn get_clipboard(ui: &Ui) -> Option<String> {
    ui.with_bound_context(|| unsafe {
        let text = sys::igGetClipboardText();
        (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::test_support::TempDir;
    use dear_imgui_rs::FramePrepareOptions;
    fn workspace(dir: &TempDir) -> Workbench {
        let mut settings = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        settings.settings["terminal_visible"] = json!(false);
        settings.terminal_visible = false;
        settings.settings["treesitter"] = json!(false);
        settings.settings["git_changed_lines"] = json!(false);
        Workbench::with_settings(settings)
    }
    #[test]
    fn tree_actions_persist_without_a_frame_and_project_switch_resets_reveal() {
        let dir = TempDir::new();
        dir.write("first/.secret", b"keep");
        dir.write("first/visible", b"keep");
        dir.write("second/visible", b"keep");
        let mut workbench = workspace(&dir);
        workbench.set_project(&dir.path("first")).unwrap();
        let hidden = dir
            .path("first/.secret")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        workbench.open_or_focus(Path::new(&hidden)).unwrap();
        workbench
            .handle_tree_action(FileTreeAction::SetPathHidden {
                path: hidden.clone(),
                hidden: true,
            })
            .unwrap();
        workbench
            .handle_tree_action(FileTreeAction::SetHideHidden(true))
            .unwrap();
        workbench
            .handle_tree_action(FileTreeAction::SetHideGitignored(true))
            .unwrap();
        workbench
            .handle_tree_action(FileTreeAction::SetShowHidden(true))
            .unwrap();
        assert!(workbench.file_dialog.is_none());
        assert_eq!(workbench.active_snapshot().unwrap().bytes, b"keep");
        let spec = workbench.workspace_spec.clone().unwrap();
        let reloaded = WorkspaceStore::load(&dir.path("config")).unwrap();
        let expected = workbench.file_explorer.file_tree.preferences.clone();
        assert_eq!(reloaded.tree_preferences(&spec), expected);
        workbench.set_project(&dir.path("second")).unwrap();
        assert_eq!(
            workbench.file_explorer.file_tree.preferences,
            Default::default()
        );
        assert!(!workbench.file_explorer.file_tree.show_hidden);
        workbench.set_project(&dir.path("first")).unwrap();
        assert_eq!(workbench.file_explorer.file_tree.preferences, expected);
        assert!(!workbench.file_explorer.file_tree.show_hidden);
        workbench
            .handle_tree_action(FileTreeAction::SetPathHidden {
                path: hidden,
                hidden: false,
            })
            .unwrap();
        assert!(
            workbench
                .file_explorer
                .file_tree
                .preferences
                .hidden_paths
                .is_empty()
        );
        assert!(workbench.file_explorer.file_tree.preferences.hide_hidden);
        let source = dir.path("first/visible").canonicalize().unwrap();
        workbench
            .handle_tree_action(FileTreeAction::SetPathHidden {
                path: source.to_str().unwrap().into(),
                hidden: true,
            })
            .unwrap();
        workbench.rename_path(&source, "renamed").unwrap();
        assert!(
            workbench
                .file_explorer
                .file_tree
                .preferences
                .hidden_paths
                .contains("visible")
        );
        assert!(
            !workbench
                .file_explorer
                .file_tree
                .preferences
                .hidden_paths
                .contains("renamed")
        );
        workbench.cleanup().unwrap();
    }
    fn frame(context: &mut Context, workbench: &mut Workbench) {
        context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
        workbench.render(context.frame()).unwrap();
        drop(context.render_legacy());
    }
    #[test]
    fn project_picker_has_no_documents_or_terminal_processes() {
        let dir = TempDir::new();
        let workspace = workspace(&dir);
        assert_eq!(workspace.session.document_ids().len(), 0);
        assert_eq!(workspace.terminal.session_count(), 0);
        assert!(workspace.panel_visible("projects"));
        assert!(!workspace.panel_visible("explorer"));
    }
    #[test]
    fn new_document_from_picker_has_undo_without_a_project() {
        let dir = TempDir::new();
        let mut workspace = workspace(&dir);
        workspace.dispatch(WindowCommand::NewDocument).unwrap();
        let doc = workspace.active_document().unwrap();
        let view = workspace.active_view().unwrap();
        workspace
            .session
            .with_commands(view, |commands| commands.paste(b"hello"))
            .unwrap();
        workspace
            .session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"");
        assert!(workspace.session.lsp().is_none());
        assert!(!workspace.session.options().persistent_history);
        assert!(workspace.project_root.is_empty());
    }
    #[test]
    fn spawned_search_panels_keep_queries_and_worker_results_independent() {
        let dir = TempDir::new();
        let first_file = dir.write("first.txt", "first_🙂_needle".as_bytes());
        let second_file = dir.write("second.txt", "second_é_needle".as_bytes());
        let mut workspace = workspace(&dir);
        workspace.set_project(dir.root()).unwrap();
        workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
        let first = workspace.active_panel_id().unwrap();
        workspace.content_search.get_mut(&first).unwrap().start(
            &workspace.project_root,
            "first_🙂_needle",
            false,
        );
        workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
        let second = workspace.active_panel_id().unwrap();
        workspace.content_search.get_mut(&second).unwrap().start(
            &workspace.project_root,
            "second_é_needle",
            true,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace
            .content_search
            .values()
            .any(|search| search.searching)
        {
            workspace.tick().unwrap();
            assert!(
                Instant::now() < deadline,
                "independent searches must complete"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        for (id, query, path) in [
            (first, "first_🙂_needle", first_file),
            (second, "second_é_needle", second_file),
        ] {
            let search = &workspace.content_search[&id];
            assert_eq!(search.query, query);
            assert_eq!(search.results.len(), 1);
            assert_eq!(
                Path::new(&search.results[0].file.full_path),
                std::fs::canonicalize(path).unwrap()
            );
        }
        workspace.dispatch(WindowCommand::FindProject).unwrap();
        assert_eq!(workspace.panel_count("search"), 2);
        let index = workspace
            .tabs
            .iter()
            .position(|tab| tab.id == first)
            .unwrap();
        workspace.close_tab(index).unwrap();
        assert_eq!(workspace.panel_count("search"), 1);
        assert!(!workspace.content_search.contains_key(&first));
        assert_eq!(workspace.content_search[&second].query, "second_é_needle");
        assert_eq!(workspace.content_search[&second].results.len(), 1);
        workspace.cleanup().unwrap();
        assert!(workspace.content_search.is_empty());
    }
    #[test]
    fn navigation_within_shared_document_keeps_the_requesting_view() {
        let dir = TempDir::new();
        let path = dir.write("file.txt", "éxy".as_bytes());
        let mut workspace = workspace(&dir);
        workspace.open_or_focus(&path).unwrap();
        let first = workspace.active_view().unwrap();
        workspace.dispatch(WindowCommand::DuplicateView).unwrap();
        let second = workspace.active_view().unwrap();
        let old = workspace.session.view_snapshot(first).unwrap();
        let file = workspace.active_snapshot().unwrap().path;
        workspace
            .open_lsp_action(LspAction::OpenLocation(LspLocation {
                file,
                line: 0,
                character: 2,
            }))
            .unwrap();
        assert_eq!(workspace.active_view(), Some(second));
        assert_eq!(workspace.session.view_snapshot(first).unwrap(), old);
        assert_eq!(workspace.session.view_snapshot(second).unwrap().column, 3);
    }
    #[test]
    fn duplicate_views_share_document_and_undo_but_not_selection() {
        let dir = TempDir::new();
        let path = dir.write("file", b"abc");
        let mut workspace = workspace(&dir);
        workspace.open_or_focus(&path).unwrap();
        let first = workspace.active_view().unwrap();
        let doc = workspace.active_document().unwrap();
        workspace.dispatch(WindowCommand::DuplicateView).unwrap();
        let second = workspace.active_view().unwrap();
        assert_ne!(first, second);
        assert_eq!(workspace.session.document_ids(), vec![doc]);
        workspace
            .session
            .with_commands(first, |commands| {
                commands.set_cursor(0, 1, false, CursorReveal::Ensure)
            })
            .unwrap();
        workspace
            .session
            .with_commands(second, |commands| {
                commands.set_cursor(0, 3, false, CursorReveal::Ensure);
                commands.paste("🙂".as_bytes());
            })
            .unwrap();
        assert_eq!(
            workspace.session.snapshot(doc).unwrap().bytes,
            "abc🙂".as_bytes()
        );
        assert_eq!(workspace.session.view_snapshot(first).unwrap().column, 1);
        workspace
            .session
            .with_commands(first, |commands| commands.undo())
            .unwrap();
        assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"abc");
        workspace.close_tab(workspace.active_index()).unwrap();
        assert_eq!(workspace.session.view_count(doc), 1);
    }
    #[test]
    fn folder_rename_rebinds_every_view_and_preserves_undo() {
        let dir = TempDir::new();
        std::fs::create_dir(dir.path("old")).unwrap();
        let path = dir.write("old/file.rs", b"abc");
        let mut workspace = workspace(&dir);
        workspace.set_project(dir.root()).unwrap();
        workspace.open_or_focus(&path).unwrap();
        workspace.dispatch(WindowCommand::DuplicateView).unwrap();
        let view = workspace.active_view().unwrap();
        let doc = workspace.active_document().unwrap();
        workspace
            .session
            .with_commands(view, |commands| commands.paste(b"x"))
            .unwrap();
        let target = workspace.rename_path(&dir.path("old"), "new").unwrap();
        assert_eq!(
            Path::new(&workspace.session.snapshot(doc).unwrap().path),
            target.join("file.rs")
        );
        assert_eq!(workspace.session.view_count(doc), 2);
        assert!(!dir.path("old").exists());
        workspace
            .session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(workspace.session.snapshot(doc).unwrap().bytes, b"abc");
        workspace.session.save(doc).unwrap();
        assert_eq!(std::fs::read(target.join("file.rs")).unwrap(), b"abc");
    }
    #[test]
    fn failed_rename_and_trash_preserve_identity_and_writes() {
        let dir = TempDir::new();
        let path = dir.write("source", b"abc");
        dir.write("target", b"keep");
        let mut workspace = workspace(&dir);
        workspace.open_or_focus(&path).unwrap();
        let doc = workspace.active_document().unwrap();
        let before = workspace.session.snapshot(doc).unwrap();
        assert!(workspace.rename_path(&path, "target").is_err());
        assert!(
            workspace
                .trash_path_with(&path, |_| Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "denied"
                )))
                .is_err()
        );
        let after = workspace.session.snapshot(doc).unwrap();
        assert_eq!(after.path, before.path);
        assert_eq!(after.bytes, before.bytes);
        assert!(after.disk_conflict.is_none());
        assert!(path.is_file());
        workspace
            .trash_path_with(&path, |path| std::fs::remove_file(path))
            .unwrap();
        workspace
            .session
            .with_commands(workspace.active_view().unwrap(), |commands| {
                commands.paste(b"dirty")
            })
            .unwrap();
        assert!(workspace.session.save(doc).is_err());
        assert!(!path.exists());
        let moved = dir.path("rescued");
        workspace.session.save_as(doc, &moved).unwrap();
        assert!(moved.is_file());
        assert!(!path.exists());
    }
    #[test]
    fn failed_group_save_keeps_all_panels_and_terminals() {
        let dir = TempDir::new();
        let first = dir.write("a", b"a");
        std::fs::create_dir(dir.path("folder")).unwrap();
        let second = dir.write("folder/b", b"b");
        let mut workspace = workspace(&dir);
        workspace.open_or_focus(&first).unwrap();
        workspace.open_or_focus(&second).unwrap();
        let view = workspace.active_view().unwrap();
        workspace
            .session
            .with_commands(view, |commands| commands.paste(b"change"))
            .unwrap();
        workspace.dispatch(WindowCommand::NewTerminal).unwrap();
        let count = workspace.tab_count();
        let terminals = workspace.terminal.session_ids();
        std::fs::remove_file(second).unwrap();
        std::fs::remove_dir(dir.path("folder")).unwrap();
        assert!(workspace.request_close_all().is_err());
        assert_eq!(workspace.tab_count(), count);
        assert_eq!(workspace.terminal.session_ids(), terminals);
        assert_eq!(workspace.session.document_ids().len(), 2);
    }
    #[test]
    fn guarded_close_persists_layout_once_and_restores_shared_views() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let path = dir.write("file.txt", b"abc");
        let mut context = Context::create();
        let mut workspace = workspace(&dir);
        workspace
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        workspace.open_or_focus(&path).unwrap();
        workspace.dispatch(WindowCommand::DuplicateView).unwrap();
        workspace.dispatch(WindowCommand::Settings).unwrap();
        workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
        workspace.close_tab(workspace.active_index()).unwrap();
        workspace.dispatch(WindowCommand::NewContentSearch).unwrap();
        let search_id = workspace.active_panel_id().unwrap();
        frame(&mut context, &mut workspace);
        frame(&mut context, &mut workspace);
        let ids = workspace.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        workspace.request_close_all().unwrap();
        let stored = std::fs::read(dir.path("config/workspaces.json")).unwrap();
        workspace.cleanup().unwrap();
        assert_eq!(
            stored,
            std::fs::read(dir.path("config/workspaces.json")).unwrap()
        );
        let mut restored = super::tests::workspace(&dir);
        restored.set_project(dir.root()).unwrap();
        assert_eq!(
            restored.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            ids
        );
        assert_eq!(restored.session.document_ids().len(), 1);
        let doc = restored.session.document_ids()[0];
        assert_eq!(restored.session.view_count(doc), 2);
        assert!(restored.panel_visible("settings"));
        assert_eq!(restored.panel_count("search"), 1);
        assert!(restored.content_search.contains_key(&search_id));
        restored.dispatch(WindowCommand::FindProject).unwrap();
        assert_eq!(restored.active_panel_id(), Some(search_id));
    }
}

#[cfg(test)]
#[path = "workbench_context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "structure_tests.rs"]
mod structure_tests;

#[cfg(test)]
#[path = "navigation_scroll_tests.rs"]
mod navigation_scroll_tests;

#[cfg(test)]
#[path = "ui_animation_tests.rs"]
mod ui_animation_tests;

#[cfg(test)]
#[path = "workbench_feature_tests.rs"]
mod feature_tests;
