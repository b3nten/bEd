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
    StyleColor, StyleVar, Ui, WindowFlags, sys,
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

#[cfg(test)]
mod attachment_edge_tests;
#[path = "debugger.rs"]
mod debugger;
#[cfg(test)]
mod default_input_tests;
mod default_layout;
#[cfg(test)]
mod default_viewer_tests;
#[path = "editor_host.rs"]
mod editor_host;
mod editor_zoom;
pub use editor_zoom::EditorPinchPhase;
#[cfg(test)]
mod directory_tests;
#[cfg(test)]
mod file_opening_tests;
mod file_operations;
mod git_smoke;
#[path = "module_host.rs"]
mod module_host;
#[path = "native_host.rs"]
mod native_host;
mod panel_boundary;
mod promotion;
#[path = "remote_workbench.rs"]
mod remote_workbench;
mod save_barrier;
mod terminal_integration;
mod tiling_commands;
mod tiling_host;
#[cfg(test)]
mod tiling_interaction_tests;
#[cfg(test)]
mod tiling_transparency_tests;
use crate::workspace::tiling_state::TilingState;
use bed_editor_ui::EditorView;
#[cfg(test)]
use bed_lsp::lsp_locations::LspLocation;
#[cfg(test)]
use bed_module_editor::lsp::lsp_ui::LspAction;
pub use bed_workbench_api::PanelTarget;
pub use module_host::WorkbenchModules;
use module_host::{HostedPanel, ModuleRuntime};
#[cfg(test)]
use std::ffi::CStr;

unsafe extern "C" {
    fn bed_imgui_dock_node_clear_background(node: *mut sys::ImGuiDockNode);
    fn bed_imgui_dock_node_select_window(
        node: *mut sys::ImGuiDockNode,
        window: *mut sys::ImGuiWindow,
    ) -> bool;
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
    SaveDefaultLayout,
    ResetDefaultLayout,
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
    fn editor(&self) -> Option<&EditorView> {
        self.instance
            .as_any()
            .downcast_ref::<bed_module_editor::TextPanel>()
            .map(|panel| panel.view())
    }
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
    dock_next: bool,
    viewport: u32,
    dock_id: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusedPanel {
    Tab(u64),
    EmptyArea(u32),
}
struct FileDialog {
    action: FileTreeAction,
    target_area: Option<u32>,
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
enum PendingTabClose {
    Tabs(Vec<u64>),
    Area {
        panels: Vec<u64>,
        area: u32,
        original: crate::workspace::tiling::Layout,
        plan: crate::workspace::tiling::DockPlan,
    },
}
impl PendingTabClose {
    fn panels(&self) -> &[u64] {
        match self {
            Self::Tabs(panels) | Self::Area { panels, .. } => panels,
        }
    }
}
type PanelComposition = (u64, u32, u32, bool, [u32; 4]);

/// Application-owned shell. Embedding uses EditorSession and EditorView directly.
pub struct Workbench {
    pub settings: Settings,
    pub project_root: String,
    pub workspace_spec: Option<WorkspaceSpec>,
    directory: PathBuf,
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
    focus_history: [Option<FocusedPanel>; 2],
    pending_editor_pinches: Vec<editor_zoom::EditorPinchEvent>,
    editor_pinch_target: Option<(u32, ViewId)>,
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
    tiling: TilingState,
    pending_tiling: Option<TilingState>,
    rendering_panels: bool,
    tiling_ui: tiling_host::TilingUi,
    scene: u64,
    pending_ini: Option<String>,
    pending_layout_reset: bool,
    pending_workspace: Option<WorkspaceSpec>,
    pending_attach: Option<(PathBuf, Option<u64>)>,
    workspace_windows: Vec<WorkspaceSpec>,
    pending_graft: Option<promotion::WorkspaceGraft>,
    shell_integration: terminal_integration::ShellIntegration,
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
    pending_tab_close: Option<PendingTabClose>,
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
            None => format!("bEd • {}", self.working_directory().display()),
        }
    }
    /// Window tools use a fixed base, independent of terminal focus and shell cd.
    pub fn working_directory(&self) -> PathBuf {
        if let Some(spec) = &self.workspace_spec {
            return PathBuf::from(&spec.root);
        }
        self.directory.clone()
    }
    pub fn set_directory(&mut self, directory: &Path) -> io::Result<()> {
        let directory = std::fs::canonicalize(directory)?;
        if !directory.is_dir() || directory.to_str().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Choose a UTF-8 directory",
            ));
        }
        self.directory = directory;
        Ok(())
    }
    /// Files panels and file mutations share one captured directory, independent
    /// of later terminal cd commands and the directory shown in the titlebar.
    fn files_root(&self) -> String {
        if self.workspace_spec.is_some() {
            self.project_root.clone()
        } else {
            self.modules.explorer.borrow().project_root.clone()
        }
    }
    fn ensure_files_directory(&mut self) -> io::Result<()> {
        if !self.files_root().is_empty() {
            return Ok(());
        }
        let directory = self.working_directory();
        let directory = std::fs::canonicalize(directory)?;
        let root = directory
            .to_str()
            .ok_or_else(|| io::Error::other("Directory is not UTF-8"))?
            .to_owned();
        let mut explorer = self.file_explorer();
        explorer.project_root = root.clone();
        explorer.file_tree.root_node = bed_module_explorer::file_tree::FileNode {
            name: directory
                .file_name()
                .unwrap_or(directory.as_os_str())
                .to_string_lossy()
                .into_owned(),
            full_path: root.clone(),
            is_directory: true,
            is_open: true,
            ..Default::default()
        };
        explorer.file_finder.set_project_dir(&root);
        explorer.file_finder.set_directory_label(Some(root));
        Ok(())
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
            directory: std::env::current_dir()
                .ok()
                .filter(|path| path.is_dir() && path.parent().is_some() && path.to_str().is_some())
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .filter(|path| path.is_dir() && path.to_str().is_some())
                })
                .unwrap_or_else(|| PathBuf::from("/")),
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
            focus_history: [None; 2],
            pending_editor_pinches: Vec::new(),
            editor_pinch_target: None,
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
            tiling: TilingState::default(),
            pending_tiling: None,
            rendering_panels: false,
            tiling_ui: tiling_host::TilingUi::default(),
            scene: 1,
            pending_ini: None,
            pending_layout_reset: false,
            pending_workspace: None,
            pending_attach: None,
            workspace_windows: Vec::new(),
            pending_graft: None,
            shell_integration: terminal_integration::ShellIntegration::default(),
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
        this.restore_module_settings();
        if let Err(error) = this.session.configure(this.session_options(None)) {
            this.error = Some(error.to_string());
        }
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
    /// Select an exact saved launch folder before any user panels are composed.
    /// Interactive workspace opening continues to carry live work through attachment.
    pub fn open_startup_workspace(&mut self, directory: &Path) -> io::Result<bool> {
        if self.initialized
            || self
                .tabs
                .iter()
                .any(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
        {
            return Err(io::Error::other(
                "Startup workspace selection requires a fresh window",
            ));
        }
        let root = std::fs::canonicalize(directory)?;
        let root = root
            .to_str()
            .ok_or_else(|| io::Error::other("Workspace path is not UTF-8"))?;
        let local = WorkspaceSpec::local(root);
        let Some(spec) = self
            .store
            .as_ref()
            .and_then(|store| store.stored_spec(&local))
        else {
            return Ok(false);
        };
        self.set_local_workspace(spec)
    }
    /// Open only an exact saved local workspace root, without discovering new
    /// projects or searching parent directories. Unknown directories return false.
    pub fn open_saved_workspace_at(&mut self, directory: &Path) -> io::Result<bool> {
        let root = std::fs::canonicalize(directory)?;
        self.store = Some(WorkspaceStore::load(&self.settings.config_dir)?);
        let Some(root) = root.to_str() else {
            return Ok(false);
        };
        let spec = self
            .store
            .as_ref()
            .and_then(|store| store.stored_spec(&WorkspaceSpec::local(root)));
        let Some(spec) = spec else {
            return Ok(false);
        };
        self.set_workspace(spec)?;
        Ok(true)
    }

    /// Restore the most recently used project, independently of standalone windows.
    pub fn restore_last_workspace(&mut self) -> io::Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        if let Some(spec) = store.recent_workspaces().into_iter().next() {
            if matches!(spec.target, WorkspaceTarget::Local)
                && self.workspace_spec.is_none()
                && !self.initialized
                && self
                    .tabs
                    .iter()
                    .all(|tab| tab.panel.kind == bed_module_projects::PANEL_ID)
            {
                // A fresh window resumes saved focus/layout directly. Interactive
                // attachment instead preserves the caller's current live focus.
                self.open_startup_workspace(Path::new(&spec.root))?;
            } else {
                self.set_workspace(spec)?;
            }
        }
        Ok(())
    }
    pub fn host_mode(&self) -> WorkbenchHostMode {
        self.mode
    }
    pub fn active_view(&self) -> Option<ViewId> {
        if self.focused_local_file().is_some() {
            return None;
        }
        self.active
            .filter(|view| self.session.document_for_view(*view).is_some())
    }
    pub fn active_document(&self) -> Option<DocumentId> {
        if self.focused_local_file().is_some() {
            return None;
        }
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
    /// Visible, clipped tab bounds in ImGui screen coordinates. Hidden tab
    /// strips and tabs scrolled completely out of view have no bounds.
    pub fn tab_bounds(&self, id: u64) -> Option<([f32; 2], [f32; 2])> {
        self.tiling_ui.tab_rects.get(&id).copied()
    }
    pub fn active_panel_id(&self) -> Option<u64> {
        self.focused
    }
    /// Native idle overlay eligibility: the focused text editor and no modal flow.
    pub fn bedtime_viewport(&self) -> Option<u32> {
        if self.file_dialog.is_some()
            || self.reload_confirmation.is_some()
            || self.file_operations.modal_visible()
            || self.remote_ui.path_dialog_pending()
            || self.modules.explorer.finder_visible()
            || self.active_overlay() != Overlay::None
        {
            return None;
        }
        self.tabs
            .iter()
            .find(|tab| Some(tab.id) == self.focused && tab.panel.editor().is_some())
            .map(|tab| tab.viewport)
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
    pub fn scene_generation(&self) -> u64 {
        self.scene
    }
    fn focused_local_file(&self) -> Option<&Path> {
        self.tabs
            .iter()
            .find(|tab| Some(tab.id) == self.focused)
            .and_then(|tab| tab.panel.input.local_file())
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
        } else if tab.panel.input.local_file().is_some() {
            self.last_document = None;
            self.active = None;
        }
        tab.focus = true;
        self.focused = Some(tab.id);
        if let Some(view) = tab.panel.view_id() {
            self.active = Some(view);
            let _ = self.session.request_focus(view);
        }
        let id = tab.id;
        let group = self
            .edit_tiling()
            .areas
            .iter_mut()
            .find(|group| group.tabs.contains(&id))
            .expect("focused panel belongs to an area");
        group.selected = Some(id);
        self.remember_focus(FocusedPanel::Tab(id));
        self.select_dock_tab(index);
        let _ = self.focus_module_panel(index);
        true
    }
    fn push_panel(&mut self, panel: Panel) -> u64 {
        let area = self.largest_area();
        self.clear_focus_requests();
        if let Some(document) = panel.document() {
            self.last_document = Some(document);
        } else if panel.input.local_file().is_some() {
            self.last_document = None;
            self.active = None;
        }
        if let Some(view) = panel.view_id() {
            self.active = Some(view);
            let _ = self.session.request_focus(view);
        }
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(Tab {
            id,
            panel,
            focus: true,
            dock_next: true,
            viewport: 0,
            dock_id: 0,
        });
        self.edit_tiling().assign(id, area);
        self.sync_local_file_protections();
        self.focused = Some(id);
        self.remember_focus(FocusedPanel::Tab(id));
        let _ = self.focus_module_panel(self.tabs.len() - 1);
        self.scene += 1;
        id
    }
    fn show_tool(&mut self, tool: Tool) {
        self.show_tool_at(tool, None);
    }
    fn show_tool_at(&mut self, tool: Tool, area: Option<u32>) {
        if let Err(error) = self.show_native_panel_at(tool.key(), None, area) {
            self.error = Some(error.to_string());
        }
    }
    #[cfg(test)]
    fn add_view(&mut self, document: DocumentId) -> io::Result<u64> {
        self.add_view_at(document, None)
    }
    fn add_view_at(&mut self, document: DocumentId, area: Option<u32>) -> io::Result<u64> {
        let viewer = self
            .modules
            .registry
            .default_viewer(DocumentKind::Text)
            .ok_or_else(|| io::Error::other("Text viewer unavailable"))?
            .id;
        self.add_document_panel_at(document, Some(viewer), &Value::Null, area)
    }
    pub fn open_or_focus(&mut self, path: &Path) -> io::Result<bool> {
        self.open_file_with_viewer(path, None, false)
    }
    pub fn set_project(&mut self, root: &Path) -> io::Result<bool> {
        if self.workspace_spec.is_none() {
            return self.attach_workspace(root);
        }
        let canonical = std::fs::canonicalize(root)?;
        if !canonical.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Project must be a directory",
            ));
        }
        let text = canonical.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
        })?;
        let local = WorkspaceSpec::local(text);
        if self
            .workspace_spec
            .as_ref()
            .is_some_and(|active| active.identity() == local.identity())
        {
            return Ok(false);
        }
        // SSH switching remains a separate flow. Local windows never replace
        // an attached workspace or its live panels with another workspace.
        if self.session.is_remote() {
            return self.set_local_workspace(local);
        }
        self.request_workspace_window(local)?;
        Ok(true)
    }
    /// Queue a separate window without changing this window's live workspace.
    pub fn request_workspace_window(&mut self, mut spec: WorkspaceSpec) -> io::Result<()> {
        if matches!(spec.target, WorkspaceTarget::Local) {
            let canonical = std::fs::canonicalize(&spec.root)?;
            if !canonical.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Project must be a directory",
                ));
            }
            spec.root = canonical
                .to_str()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
                })?
                .to_owned();
        }
        self.store = Some(WorkspaceStore::load(&self.settings.config_dir)?);
        let store = self.store.as_mut().unwrap();
        if let Some(saved) = store.stored_spec(&spec) {
            spec = saved;
        } else {
            store.record_workspace(spec.clone())?;
        }
        self.workspace_windows.push(spec);
        self.refresh_projects();
        Ok(())
    }
    /// The desktop host launches these workspaces with this window's config directory.
    pub fn take_workspace_windows(&mut self) -> Vec<WorkspaceSpec> {
        std::mem::take(&mut self.workspace_windows)
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
        self.sync_local_file_protections();
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
        self.directory = PathBuf::from(&path);
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
            self.apply_default_layout()?;
        }
        self.finish_workspace_open()?;
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
        self.pending_attach = None;
        self.pending_graft = None;
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
        self.tiling = TilingState::default();
        self.pending_tiling = None;
        self.focus_history = [None; 2];
        self.reset_tiling_docks();
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
        self.pending_editor_pinches.clear();
        self.editor_pinch_target = None;
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
            git: self.settings.bool("git_changed_lines", true),
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
            match TilingState::from_legacy_ini(&ini, self.tabs.iter().map(|tab| tab.id)) {
                Ok(state) => {
                    self.tiling = state;
                    self.pending_tiling = None;
                    self.dock_built = true;
                    self.reset_tiling_docks();
                }
                Err(error) => {
                    self.default_tiling_layout();
                    self.error = Some(format!("Workspace layout ignored: {error}"));
                }
            }
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
        self.poll_shell_integration()?;
        if let Some((root, terminal)) = self.pending_attach.take() {
            self.attach_workspace_with_terminal(&root, terminal)?;
        }
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
        self.poll_module_saves(&report);
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
                        targets.panels().contains(&tab.id) && tab.panel.document() == Some(document)
                    })
                });
                if affected {
                    self.pending_tab_close = None;
                }
            }
            self.error = Some(format!("{}: {}", error.service, error.message));
        }
        if !self.remote_ui.path_dialog_pending()
            && let Some(request) = self.pending_tab_close.take()
        {
            let result = match request {
                PendingTabClose::Tabs(ids) => {
                    let indices = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter_map(|(index, tab)| ids.contains(&tab.id).then_some(index))
                        .collect();
                    self.close_tabs(indices)
                }
                PendingTabClose::Area {
                    panels,
                    area,
                    original,
                    plan,
                } => self.close_tiling_area(area, original, plan, panels),
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        self.terminal.poll()?;
        for (session, target) in self.terminal.take_open_links() {
            if let Err(error) = self.open_terminal_link(session, &target) {
                self.error = Some(error.to_string());
            }
        }
        self.tick_plugins()?;
        if self.last_settings_check.elapsed() > Duration::from_millis(500) {
            self.last_settings_check = Instant::now();
            self.settings.check_settings_file();
        }
        self.process_native_actions(&mut Vec::new())?;
        self.finish_navigation_request()
    }
    #[cfg(test)]
    fn new_terminal(&mut self) {
        self.new_terminal_at(None);
    }
    fn new_terminal_at(&mut self, area: Option<u32>) {
        if let Err(error) = self.open_native_panel_at("terminal", &Value::Null, area) {
            self.error = Some(error.to_string());
        }
    }
    pub fn dispatch(&mut self, command: WindowCommand) -> io::Result<bool> {
        self.dispatch_at(command, None)
    }
    /// Menu and toolbar actions create content in the selected area.
    pub fn dispatch_from_menu(&mut self, command: WindowCommand) -> io::Result<bool> {
        self.dispatch_with_target(command, PanelTarget::LastFocused)
    }
    pub fn dispatch_with_target(
        &mut self,
        command: WindowCommand,
        target: PanelTarget,
    ) -> io::Result<bool> {
        let area = self.panel_target_area(target);
        self.dispatch_at(command, Some(area))
    }
    fn dispatch_at(&mut self, command: WindowCommand, area: Option<u32>) -> io::Result<bool> {
        if self.workspace_spec.is_none()
            && matches!(
                command,
                WindowCommand::Debug
                    | WindowCommand::NewDiagnostics
                    | WindowCommand::Diagnostics
                    | WindowCommand::NewLspDashboard
                    | WindowCommand::LspDashboard
                    | WindowCommand::NewReferences
            )
        {
            return Ok(false);
        }
        match command {
            WindowCommand::Debug => {
                self.dispatch_command_at(bed_module_debug::SHOW_COMMAND, area)?;
            }
            WindowCommand::NewDocument => {
                if self.project_root.is_empty() {
                    self.session.configure(self.session_options(None))?;
                }
                let document = self.session.create_document(b"")?;
                self.add_view_at(document, area)?;
            }
            WindowCommand::NewTerminal => self.new_terminal_at(area),
            WindowCommand::NewExplorer => {
                self.open_native_panel_at(Tool::Explorer.key(), &Value::Null, area)?;
            }
            WindowCommand::NewSettings => {
                self.open_native_panel_at(Tool::Settings.key(), &Value::Null, area)?;
            }
            WindowCommand::NewProjects => {
                self.open_native_panel_at(Tool::Projects.key(), &Value::Null, area)?;
            }
            WindowCommand::NewDiagnostics => {
                self.open_native_panel_at(Tool::Diagnostics.key(), &Value::Null, area)?;
            }
            WindowCommand::NewStructure => {
                self.open_plugin_panel_at("bed.structure.panel", None, &Value::Null, None, area)?;
            }
            WindowCommand::NewLspDashboard => {
                self.open_native_panel_at(Tool::LspDashboard.key(), &Value::Null, area)?;
            }
            WindowCommand::NewContentSearch => {
                self.open_native_panel_at(Tool::Search.key(), &Value::Null, area)?;
            }
            WindowCommand::NewReferences => {
                self.open_native_panel_at(Tool::References.key(), &Value::Null, area)?;
            }
            WindowCommand::DuplicateView => {
                if let Some(index) = self.active_tab_index() {
                    let panel = &self.tabs[index].panel;
                    if panel.input.local_file().is_some() {
                        let (kind, input, state, viewer) = (
                            panel.kind.clone(),
                            panel.input.clone(),
                            panel.instance.save_state(),
                            panel.viewer.clone(),
                        );
                        self.open_plugin_panel_at(&kind, input, &state, viewer, area)?;
                    } else if let Some(document) = self.active_document() {
                        let viewer = self.focused_document_viewer();
                        self.add_document_panel_at(
                            document,
                            viewer.as_deref(),
                            &Value::Null,
                            area,
                        )?;
                    }
                }
            }
            WindowCommand::SplitRight | WindowCommand::SplitDown => {
                if self.focused_terminal() {
                    self.split_last_terminal(command == WindowCommand::SplitDown)?;
                } else if let Some(index) = self.active_tab_index() {
                    let source = self.tabs[index].id;
                    let area = self
                        .area_for_panel(source)
                        .expect("document belongs to an area");
                    let axis = usize::from(command == WindowCommand::SplitDown);
                    let rect = self.current_tiling().layout.area(area).rect;
                    if let Some(new) =
                        self.split_area(area, axis, (rect.min[axis] + rect.max[axis]) / 2)
                    {
                        self.switch_to_tab(index);
                        self.duplicate_panel(source, new)?;
                    }
                }
            }
            WindowCommand::ResetLayout => {
                self.pending_ini = None;
                self.default_tiling_layout();
                self.scene += 1;
            }
            WindowCommand::SaveDefaultLayout => self.save_default_layout()?,
            WindowCommand::ResetDefaultLayout => self.reset_default_layout()?,
            WindowCommand::Projects => self.show_tool_at(Tool::Projects, area),
            WindowCommand::Diagnostics => self.show_tool_at(Tool::Diagnostics, area),
            WindowCommand::Structure => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.panel.kind == "bed.structure.panel")
                {
                    self.switch_to_tab(index);
                } else {
                    self.open_plugin_panel_at(
                        "bed.structure.panel",
                        None,
                        &Value::Null,
                        None,
                        area,
                    )?;
                }
            }
            WindowCommand::OpenFolder => {
                let mut dialog = rfd::FileDialog::new();
                if !self.session.is_remote() {
                    dialog = dialog.set_directory(self.working_directory());
                }
                if let Some(path) = dialog.pick_folder() {
                    return self.set_project(&path);
                } else {
                    return Ok(false);
                }
            }
            WindowCommand::OpenFile => {
                if area.is_some() {
                    self.open_file_dialog_at(None, area)?;
                } else {
                    return self.handle_action(HostAction::Open);
                }
            }
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
            WindowCommand::Explorer => self.show_tool_at(Tool::Explorer, area),
            WindowCommand::Terminal => {
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.panel.terminal_id().is_some())
                {
                    self.switch_to_tab(index);
                } else {
                    self.new_terminal_at(area);
                }
            }
            WindowCommand::Settings => self.show_tool_at(Tool::Settings, area),
            WindowCommand::LspDashboard => self.show_tool_at(Tool::LspDashboard, area),
            WindowCommand::FindFile => {
                self.ensure_files_directory()?;
                self.file_explorer().file_finder.toggle_window();
            }
            WindowCommand::FindProject => {
                self.show_tool_at(Tool::Search, area);
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
        if let Some(index) = self.tabs.iter().position(|tab| {
            Some(tab.id) == self.focused
                && (tab.panel.document().is_some() || tab.panel.input.local_file().is_some())
        }) {
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
                if let Some(path) = rfd::FileDialog::new()
                    .set_directory(self.working_directory())
                    .pick_file()
                {
                    self.open_or_focus(&path)
                } else {
                    Ok(false)
                }
            }
            HostAction::Save => {
                let Some(document) = self.active_document() else {
                    return Ok(false);
                };
                if self.session.is_snapshot_document(document) {
                    return Ok(false);
                }
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
                if self.session.is_snapshot_document(document) {
                    return Ok(false);
                }
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
        if self.session.is_snapshot_document(document) {
            return Ok(false);
        }
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
        let mut dialog = rfd::FileDialog::new().set_directory(self.working_directory());
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
            self.session.ensure_local_path_editable(&path)?;
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
    fn close_is_pending(&self, panels: &[u64]) -> bool {
        self.session.is_remote()
            && (self.remote_ui.path_dialog_pending()
                || self.remote_ui.mutation_pending()
                || self.tabs.iter().any(|tab| {
                    panels.contains(&tab.id)
                        && tab
                            .panel
                            .document()
                            .is_some_and(|doc| self.session.save_pending(doc))
                }))
    }
    fn close_tabs(&mut self, mut indices: Vec<usize>) -> io::Result<bool> {
        indices.sort_unstable();
        indices.dedup();
        let panels = indices
            .iter()
            .filter_map(|index| self.tabs.get(*index))
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        if !self.preflight_close(&indices)? {
            if self.close_is_pending(&panels) {
                self.pending_tab_close = Some(PendingTabClose::Tabs(panels));
            }
            return Ok(false);
        }
        for index in indices.into_iter().rev() {
            self.remove_tab(index)?;
        }
        Ok(true)
    }
    /// Close actions use the same workspace-owned order that the tab strip draws.
    fn tab_group_order(&self, id: u64) -> Vec<usize> {
        self.current_tiling()
            .areas
            .iter()
            .find(|group| group.tabs.contains(&id))
            .map(|group| {
                group
                    .tabs
                    .iter()
                    .filter_map(|id| self.tabs.iter().position(|tab| tab.id == *id))
                    .collect()
            })
            .unwrap_or_default()
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
            self.tab_context = self
                .tiling_ui
                .tab_rects
                .iter()
                .find_map(|(id, (min, max))| {
                    (mouse[0] >= min[0]
                        && mouse[0] < max[0]
                        && mouse[1] >= min[1]
                        && mouse[1] < max[1])
                        .then_some(*id)
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
        let area = self
            .area_for_panel(self.tabs[index].id)
            .expect("closing panel belongs to an area");
        let mut tab = self.tabs.remove(index);
        let mut requests = Vec::new();
        let result = self.with_module_services(|_, services| {
            tab.panel.close_presentations(services, &mut requests)
        });
        if let Err(error) = result {
            self.tabs.insert(index, tab);
            return Err(error);
        }
        self.forget_focus(FocusedPanel::Tab(tab.id));
        self.sync_local_file_protections();
        self.modules.requests.extend(requests);
        self.tiling.remove(tab.id);
        if let Some(state) = &mut self.pending_tiling {
            state.remove(tab.id);
        }

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
            if let Some(group) = self
                .current_tiling()
                .areas
                .iter()
                .find(|group| group.area == area)
            {
                if let Some(index) = group
                    .selected
                    .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                {
                    self.switch_to_tab(index);
                } else {
                    self.focus_empty_area(area);
                }
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
                for instance in tab.panel.presentations_mut() {
                    instance.document_removed_with_services(
                        document,
                        path,
                        services,
                        &mut requests,
                    )?;
                }
                Ok(())
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
        self.sync_local_file_protections();
        self.pending_tab_close = None;
        self.modules.search.cancel_all();
        self.modules = ModuleRuntime::from_composition((self.module_factory)());

        self.active = None;
        self.last_document = None;
        self.focused = None;
        self.focus_history = [None; 2];
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
    pub fn render(&mut self, ui: &Ui) -> io::Result<Vec<HostAction>> {
        if self.context_id != Some(ui.context_id()) {
            return Err(io::Error::other(
                "Initialize Workbench in this ImGui context before rendering",
            ));
        }
        ui.with_bound_context(|| self.render_bound(ui))
    }
    fn render_bound(&mut self, ui: &Ui) -> io::Result<Vec<HostAction>> {
        self.prepare_tiling()?;
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
            let mut background = self.settings.window_background_color();
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
        // Area surfaces own their rounded background and outline. Native
        // docking and content windows provide only their contents and tabs.
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
                self.draw_tiling(ui, [pos[0], pos[1] + self.root_top_inset], size);
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
        self.rendering_panels = true;
        let count = self.tabs.len();
        for index in 0..count {
            let mut tab = self.tabs.remove(index);
            let title = self.title(&tab);
            let mut opened = true;
            ui.set_next_window_viewport(ui.main_viewport().id());
            if std::mem::take(&mut tab.dock_next) {
                let area = self
                    .tiling
                    .area_for(tab.id)
                    .expect("panel belongs to an area");
                let dock = self.tiling_ui.docks[&area];
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
                | WindowFlags::NO_MOVE
                | WindowFlags::NO_BACKGROUND
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
            let _menu_background = ui.push_style_color(StyleColor::MenuBarBg, [0.0; 4]);
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
                        self.remember_focus(FocusedPanel::Tab(tab.id));
                        if let Some(document) = tab.panel.document() {
                            self.last_document = Some(document);
                        } else if tab.panel.input.local_file().is_some() {
                            self.last_document = None;
                            self.active = None;
                        }
                        if let Some(view) = tab.panel.view_id() {
                            self.active = Some(view);
                        }
                    }
                    unsafe {
                        tab.viewport = (*sys::igGetWindowViewport()).ID;
                        tab.dock_id = sys::igGetWindowDockID();
                    }
                    result = self.draw_module_panel(ui, tab.id, &mut tab.panel);
                    if result.is_ok() {
                        result = self.accept_tiling_file_drop(ui);
                    }
                });
            drop(_menu_background);
            drop(_panel_padding);
            self.tabs.insert(index, tab);
            result?;
            if !opened {
                close.push(index);
            }
        }
        self.draw_empty_areas(ui);
        self.finish_tiling_file_drops()?;
        self.sync_tiling_tabs(ui);
        self.finish_tiling(ui);
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
        self.finish_workspace_graft();
        self.finish_shell_integration();
        self.rendering_panels = false;
        Ok(actions)
    }
    fn shortcuts(&mut self, ui: &Ui) -> io::Result<()> {
        self.editor_zoom_input(ui);
        self.module_shortcuts(ui)?;
        let ctrl = ui.io().key_ctrl() || (!self.focused_terminal() && ui.io().key_super());
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
            let area = Some(self.panel_target_area(PanelTarget::PreviousFocused));
            if let Some(viewer) = command.strip_prefix("bed.open_with:") {
                if let Some(path) = &context.path {
                    self.open_file_from_menu(Path::new(path), Some(viewer), true, area)?;
                }
            } else {
                self.refresh_plugins()?;
                self.run_module_command_at(command, context, area)?;
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
            let area = self.panel_target_area(PanelTarget::PreviousFocused);
            self.open_file_from_menu(Path::new(&path), None, false, Some(area))?;
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
                target_area: matches!(&action, FileTreeAction::NewFile(_))
                    .then(|| self.panel_target_area(PanelTarget::PreviousFocused)),
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
                    match self.apply_file_action_at(
                        &dialog.action,
                        &dialog.name,
                        dialog.target_area,
                    ) {
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
        let source = file_actions::validate_project_entry(Path::new(&self.files_root()), source)?;
        let target = source.parent().unwrap().join(name);
        file_actions::validate_name(name)?;
        self.ensure_local_files_unaffected(&source)?;
        self.ensure_local_files_unaffected(&target)?;
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
        let source = file_actions::validate_project_entry(Path::new(&self.files_root()), source)?;
        self.ensure_local_files_unaffected(&source)?;
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
    #[cfg(test)]
    #[cfg(test)]
    fn apply_file_action(&mut self, action: &FileTreeAction, name: &str) -> io::Result<()> {
        self.apply_file_action_at(action, name, None)
    }
    fn apply_file_action_at(
        &mut self,
        action: &FileTreeAction,
        name: &str,
        area: Option<u32>,
    ) -> io::Result<()> {
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
            return self.queue_remote_file_action_at(action, name, area);
        }
        match action {
            FileTreeAction::NewFile(directory) => {
                self.validate_directory(directory)?;
                file_actions::validate_name(name)?;
                self.ensure_local_files_unaffected(&Path::new(directory).join(name))?;
                let path = file_actions::create_file(Path::new(directory), name)?;
                self.refresh_file_directories([directory.clone()]);
                self.open_file_from_menu(&path, None, false, area)?;
            }
            FileTreeAction::NewFolder(directory) => {
                self.validate_directory(directory)?;
                file_actions::validate_name(name)?;
                self.ensure_local_files_unaffected(&Path::new(directory).join(name))?;
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
        if !path.is_dir() || !path.starts_with(self.files_root()) {
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
        if self.workspace_spec.is_none() {
            return Ok(());
        }
        self.persist_module_settings()?;
        self.refresh_plugins()?;
        let mut tabs = std::mem::take(&mut self.tabs);
        let result = self.with_module_services(|_, services| {
            let mut states = HashMap::new();
            for tab in &mut tabs {
                let mut presentations = tab.panel.saved_presentations.clone();
                for (viewer, panel) in &mut tab.panel.inactive {
                    presentations.insert(
                        viewer.clone(),
                        panel.instance.save_state_with_services(services)?,
                    );
                }
                tab.panel.saved_presentations = presentations;
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
        // Geometry and membership are authoritative even before the first
        // native frame. Saving never depends on ImGui's transient dock tree.
        if !self.dock_built {
            self.default_tiling_layout();
        }
        let mut panels = Vec::new();
        for tab in &self.tabs {
            let panel = &tab.panel;
            if panel.instance.is_input_empty(&self.modules.frame.context()) {
                panels.push(json!({"id":tab.id,"kind":"plugin","panel_type":panel.kind,
                    "viewer":panel.viewer,"default_input":true}));
                continue;
            }
            if !panel.instance.persist() {
                continue;
            }
            let doc = panel
                .instance
                .attached_document()
                // Snapshot panels restore their comparison from panel state;
                // their unnamed backing document is never a workspace file.
                .filter(|id| !self.session.is_snapshot_document(*id))
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
            let local_file = panel
                .input
                .local_file()
                .map(|path| path.to_string_lossy().into_owned());
            let mut value = json!({"kind":legacy.unwrap_or("plugin"),"panel_type":panel.kind,"viewer":panel.viewer,
                "path":local_file.as_deref().or_else(|| doc.as_ref().map(|doc| doc.path.as_str())),"document_kind":doc.as_ref().map(|doc|if doc.kind == DocumentKind::Bytes {"bytes"} else {"text"}),"state":panel_states.get(&tab.id)});
            if local_file.is_some() {
                value["backing"] = json!("local_file");
            }
            if !panel.saved_presentations.is_empty() {
                value["presentations"] = json!(panel.saved_presentations);
            }
            if matches!(legacy, Some("document" | "terminal"))
                && let Some(state) = value["state"].as_object().cloned()
            {
                value.as_object_mut().unwrap().extend(state);
            }
            value["id"] = json!(tab.id);
            panels.push(value);
        }
        let tiling = self
            .current_tiling()
            .to_value(panels.iter().filter_map(|panel| panel["id"].as_u64()));
        let mut state = json!({"version":2,"tiling":tiling,"ini":"","panels":panels,"focused":self.focused,"active_document_panel":self.active_tab_index().and_then(|index|self.tabs.get(index)).map(|tab|tab.id)});
        if let Some(graft) = &self.pending_graft {
            state["graft"] = graft.to_value();
        }
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
            state["tiling"] = Value::Null;
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
        // Older image/model panels used byte storage even for source files.
        // Reclassify only when every saved presentation accepts text; a hex
        // sibling still requires the original exact-byte document.
        let mut text_capable_paths = HashMap::new();
        for panel in state["panels"].as_array().into_iter().flatten() {
            if let Some(path) = panel["path"].as_str() {
                let viewer = panel["viewer"]
                    .as_str()
                    .and_then(|id| self.modules.registry.viewer(id))
                    .or_else(|| {
                        (panel["kind"].as_str() == Some("document"))
                            .then(|| self.modules.registry.default_viewer(DocumentKind::Text))
                            .flatten()
                    });
                let accepts_text =
                    viewer.is_some_and(|viewer| viewer.supports_document_kind(DocumentKind::Text));
                text_capable_paths
                    .entry(path.to_owned())
                    .and_modify(|value| *value &= accepts_text)
                    .or_insert(accepts_text);
            }
        }
        for panel in state["panels"].as_array().into_iter().flatten() {
            let kind = panel["kind"].as_str().unwrap_or("");
            let result = if panel["default_input"].as_bool() == Some(true) {
                self.open_plugin_panel(
                    panel["panel_type"].as_str().unwrap_or(""),
                    bed_workbench_api::PanelInput::None,
                    &Value::Null,
                    panel["viewer"].as_str().map(str::to_owned),
                )
            } else {
                match kind {
                    "structure" => {
                        self.open_plugin_panel("bed.structure.panel", None, &Value::Null, None)
                    }
                    "hex" | "plugin" => {
                        let viewer = panel["viewer"].as_str();
                        let path = panel["path"].as_str();
                        if panel["backing"].as_str() == Some("local_file") {
                            (|| {
                                let path = path.ok_or_else(|| {
                                    io::Error::other("Saved local-file panel has no path")
                                })?;
                                let viewer = viewer
                                    .and_then(|id| self.modules.registry.viewer(id))
                                    .cloned()
                                    .ok_or_else(|| {
                                        io::Error::other("Saved local-file viewer is unavailable")
                                    })?;
                                self.add_local_file_panel(Path::new(path), &viewer, &panel["state"])
                            })()
                        } else if let Some(path) = path {
                            let document_kind = if panel["document_kind"].as_str() == Some("bytes")
                                || kind == "hex"
                            {
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
                                let opening = if document_kind == DocumentKind::Bytes
                                    && text_capable_paths.get(path) == Some(&true)
                                {
                                    self.session.request_open_file_auto(Path::new(path))
                                } else {
                                    self.session
                                        .request_open_file_with_kind(Path::new(path), document_kind)
                                };
                                if let Err(error) = opening {
                                    self.remote_ui.restore.remove(path);
                                    self.error = Some(error.to_string());
                                }
                                continue;
                            }
                            let opening = self
                                .session
                                .ensure_local_path_editable(Path::new(path))
                                .and_then(|_| {
                                    if document_kind == DocumentKind::Bytes
                                        && text_capable_paths.get(path) == Some(&true)
                                    {
                                        self.session.open_file_auto(Path::new(path))
                                    } else {
                                        self.session
                                            .open_file_with_kind(Path::new(path), document_kind)
                                    }
                                });
                            opening.and_then(|document| {
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
                                if let Err(error) = self.session.request_open_file(Path::new(path))
                                {
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
                                .ensure_local_path_editable(Path::new(path))
                                .and_then(|_| self.session.open_file(Path::new(path)))
                                .and_then(|doc| {
                                    self.add_document_panel(
                                        doc,
                                        panel["viewer"].as_str(),
                                        &panel["state"],
                                    )
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
                }
            };
            let panel_id = match result {
                Ok(id) => id,
                Err(error) => {
                    self.error = Some(error.to_string());
                    continue;
                }
            };
            let restored_id = panel["id"].as_u64().unwrap_or(panel_id);
            self.replace_panel_id(panel_id, restored_id);
            let tab = self
                .tabs
                .iter_mut()
                .find(|tab| tab.id == restored_id)
                .unwrap();
            tab.panel.saved_presentations =
                serde_json::from_value(panel["presentations"].clone()).unwrap_or_default();
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
        let ids = state["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|panel| panel["id"].as_u64())
            .collect::<Vec<_>>();
        let restored = if !state["tiling"].is_null() {
            Some(crate::workspace::tiling_state::TilingState::from_value(
                &state["tiling"],
                ids.iter().copied(),
            ))
        } else {
            state["ini"]
                .as_str()
                .filter(|ini| !ini.is_empty())
                .map(|ini| {
                    crate::workspace::tiling_state::TilingState::from_legacy_ini(
                        ini,
                        ids.iter().copied(),
                    )
                })
        };
        self.pending_tiling = None;
        match restored {
            Some(Ok(tiling)) => {
                if self.rendering_panels {
                    self.pending_tiling = Some(tiling);
                } else {
                    self.tiling = tiling;
                }
            }
            Some(Err(error)) => {
                self.default_tiling_layout();
                self.error = Some(format!(
                    "Workspace layout was invalid ({error}); restored panels with a default layout"
                ));
            }
            None => self.default_tiling_layout(),
        }
        self.pending_ini = None;
        self.dock_built = true;
        self.reset_tiling_docks();
        if self.remote_ui.restore.is_empty() {
            let live = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
            self.edit_tiling().retain_panels(live);
        }
        if self.tabs.is_empty() && self.remote_ui.restore.is_empty() && state["tiling"].is_null() {
            self.show_tool(Tool::Projects);
        }
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == state["focused"].as_u64())
        {
            self.switch_to_tab(index);
        }
        if self.workspace_spec.is_some() {
            self.pending_graft = promotion::WorkspaceGraft::from_value(&state["graft"]);
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

#[cfg(test)]
mod sqlite_tests;

#[cfg(test)]
mod tiling_persistence_tests;
