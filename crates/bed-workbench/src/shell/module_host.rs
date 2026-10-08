//! Application adapter for built-in modules and explicitly linked plugins.
use super::*;
use bed_workbench_api::gpu::{GpuContext, RenderOutput, RenderTarget};
use bed_workbench_api::{
    CommandContext, EditToken, HostContext, HostRequest, MenuSlot, ModuleServices, PanelAction,
    PanelPlacement, Plugin, PluginDocument, PluginPanel, Registry, TerminalLaunch, TerminalService,
    TextureHandle,
};
use dear_imgui_rs::TextureId;
use std::sync::Arc;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/plugin_host_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/csv_host_tests.rs"]
mod csv_tests;

pub(super) struct HostedPanel {
    pub kind: String,
    pub viewer: Option<String>,
    pub instance: Box<dyn PluginPanel>,
}

#[derive(Default)]
pub(super) struct PluginFrame {
    pub documents: Vec<PluginDocument>,
    pub active_document: Option<DocumentId>,
    pub settings: Value,
    pub textures: HashMap<TextureHandle, TextureId>,
    pub animations: bool,
    pub workspace: u64,
    pub diagnostics: Value,
}
impl PluginFrame {
    pub fn context(&self) -> HostContext<'_> {
        HostContext {
            documents: &self.documents,
            active_document: self.active_document,
            settings: &self.settings,
            textures: &self.textures,
            animations: self.animations,
            workspace: self.workspace,
            diagnostics: &self.diagnostics,
        }
    }
}
pub(super) struct ModuleRuntime {
    pub editor_runtime: Rc<std::cell::RefCell<bed_module_editor::EditorRuntime>>,
    pub explorer: bed_module_explorer::ExplorerHandle,
    pub search: bed_module_search::SearchHandle,
    pub projects: Rc<std::cell::RefCell<bed_module_projects::ProjectsState>>,
    pub editor_config: Rc<std::cell::RefCell<bed_module_editor::EditorConfig>>,
    pub registry: Registry,
    pub instances: Vec<Box<dyn Plugin>>,
    pub frame: PluginFrame,
    pub requests: Vec<HostRequest>,
    pub pending_open: HashMap<String, Option<String>>,
    pending_switch: Option<(PathBuf, String)>,
}
/// Concrete feature handles supplied by application composition.
pub struct WorkbenchModules {
    pub editor_runtime: Rc<std::cell::RefCell<bed_module_editor::EditorRuntime>>,
    pub explorer: bed_module_explorer::ExplorerHandle,
    pub search: bed_module_search::SearchHandle,
    pub projects: Rc<std::cell::RefCell<bed_module_projects::ProjectsState>>,
    pub editor_config: Rc<std::cell::RefCell<bed_module_editor::EditorConfig>>,
    pub instances: Vec<Box<dyn Plugin>>,
}
impl ModuleRuntime {
    pub(super) fn from_composition(composition: WorkbenchModules) -> Self {
        let WorkbenchModules {
            editor_runtime,
            explorer,
            search,
            projects,
            editor_config,
            instances,
        } = composition;
        let mut registry = Registry::default();
        for plugin in &instances {
            registry
                .register(plugin.as_ref())
                .expect("bundled plugin registration must be valid");
        }
        Self {
            editor_runtime,
            explorer,
            search,
            projects,
            editor_config,
            registry,
            instances,
            frame: PluginFrame::default(),
            requests: Vec::new(),
            pending_open: HashMap::new(),
            pending_switch: None,
        }
    }
}
impl ModuleRuntime {
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
        viewer: Option<String>,
        services: &mut ModuleServices<'_>,
    ) -> io::Result<HostedPanel> {
        let owner = self
            .registry
            .panel(kind)
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {kind}")))?
            .plugin;
        let plugin = self
            .instances
            .iter_mut()
            .find(|plugin| plugin.id() == owner)
            .ok_or_else(|| io::Error::other("Plugin unavailable"))?;
        let instance = plugin
            .create_panel_with_services(kind, document, state, services)
            .map_err(io::Error::other)?;
        Ok(HostedPanel {
            kind: kind.to_owned(),
            viewer,
            instance,
        })
    }
    pub fn enabled(&self, command: &str, context: &CommandContext) -> bool {
        self.registry
            .command(command)
            .and_then(|descriptor| {
                self.instances
                    .iter()
                    .find(|plugin| plugin.id() == descriptor.plugin)
            })
            .is_some_and(|plugin| plugin.command_enabled(command, context, &self.frame.context()))
    }
    pub(super) fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        services: &mut ModuleServices<'_>,
    ) -> io::Result<()> {
        let owner = self
            .registry
            .command(command)
            .ok_or_else(|| io::Error::other(format!("Command unavailable: {command}")))?
            .plugin;
        if !self.enabled(command, context) {
            return Ok(());
        }
        if let Some(plugin) = self
            .instances
            .iter_mut()
            .find(|plugin| plugin.id() == owner)
        {
            plugin.command_with_services(
                command,
                context,
                &self.frame.context(),
                services,
                &mut self.requests,
            )?;
        }
        Ok(())
    }
}

impl Workbench {
    pub(super) fn refresh_plugins(&mut self) -> io::Result<()> {
        let ids = self.session.document_ids();
        self.modules
            .frame
            .documents
            .retain(|doc| ids.contains(&doc.id));
        for id in ids {
            let revision = self.session.document_revision(id)?;
            let existing = self
                .modules
                .frame
                .documents
                .iter()
                .position(|doc| doc.id == id);
            if let Some(index) =
                existing.filter(|&index| self.modules.frame.documents[index].revision == revision)
            {
                self.session.with_document(id, |state| {
                    let cached = &mut self.modules.frame.documents[index];
                    cached.path.clone_from(&state.path);
                    cached.language_id.clone_from(&state.language_id);
                    cached.dirty = state.dirty;
                })?;
            } else {
                let doc = self.session.snapshot(id)?;
                let text = if doc.kind == DocumentKind::Text {
                    Some(self.session.with_document(id, |state| state.snapshot())?)
                } else {
                    None
                };
                let document = PluginDocument {
                    id,
                    path: doc.path,
                    kind: doc.kind,
                    language_id: doc.language_id,
                    revision,
                    dirty: doc.dirty,
                    bytes: Arc::from(doc.bytes),
                    text,
                };
                if let Some(index) = existing {
                    self.modules.frame.documents[index] = document;
                } else {
                    self.modules.frame.documents.push(document);
                }
            }
        }
        self.modules.frame.active_document = self.active_document();
        self.modules.frame.settings = self.settings.settings["plugins"].clone();
        if !self.modules.frame.settings.is_object() {
            self.modules.frame.settings = json!({});
        }
        self.modules.frame.animations = self.settings.bool("ui_animations", true);
        self.modules.frame.workspace = self.session.workspace_id().0;
        let mut diagnostics = serde_json::Map::new();
        for doc in &self.modules.frame.documents {
            if let Some(store) = self
                .session
                .lsp()
                .and_then(|pool| pool.diagnostics_for_document(doc.id))
            {
                diagnostics.insert(doc.path.clone(), Value::Array(store.for_document(&doc.path).into_iter().map(|item| json!({
                    "range":{"start":{"line":item.start_line,"character":item.start_character},"end":{"line":item.end_line,"character":item.end_character}},
                    "severity":item.severity,"message":item.message,"source":item.source })).collect()));
            }
        }
        self.modules.frame.diagnostics = Value::Object(diagnostics);
        Ok(())
    }
    pub(super) fn tick_plugins(&mut self) -> io::Result<()> {
        self.refresh_plugins()?;
        self.with_module_services(|modules, services| {
            let host = modules.frame.context();
            for module in &mut modules.instances {
                module.tick_with_services(&host, services, &mut modules.requests)?;
            }
            Ok(())
        })?;
        self.process_plugin_requests()?;
        if let Some((path, viewer)) = self.modules.pending_switch.take() {
            if self.session.document_for_path(&path).is_none() {
                self.open_file_with_viewer(&path, Some(&viewer), false)?;
            } else if self.pending_tab_close.is_some() {
                self.modules.pending_switch = Some((path, viewer));
            }
        }
        Ok(())
    }
    pub(super) fn open_plugin_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
        viewer: Option<String>,
    ) -> io::Result<u64> {
        let descriptor = self
            .modules
            .registry
            .panel(kind)
            .cloned()
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {kind}")))?;
        if descriptor.singleton
            && let Some(index) = self.tabs.iter().position(|tab| tab.panel.kind == kind)
        {
            self.switch_to_tab(index);
            self.select_dock_tab(index);
            return Ok(self.tabs[index].id);
        }
        let panel = self.with_module_services(|modules, services| {
            modules.create_panel(kind, document, state, viewer, services)
        })?;
        if let Some(document) = panel.instance.attached_document() {
            self.session.document_kind(document)?;
        }
        let id = self.push_panel(panel);
        self.place_registered_panel(id, descriptor.open_placement);
        self.refresh_plugins()?;
        Ok(id)
    }
    pub(super) fn close_plugin_panels(&mut self) -> io::Result<()> {
        let mut tabs = std::mem::take(&mut self.tabs);
        let result = self.with_module_services(|modules, services| {
            for tab in &mut tabs {
                tab.panel
                    .instance
                    .close_with_services(services, &mut modules.requests)?;
            }
            Ok(())
        });
        self.tabs = tabs;
        result?;
        self.process_plugin_requests()
    }

    pub(super) fn add_document_panel(
        &mut self,
        document: DocumentId,
        viewer: Option<&str>,
        state: &Value,
    ) -> io::Result<u64> {
        let descriptor = match viewer {
            Some(id) => self.modules.registry.viewer(id),
            None => self
                .modules
                .registry
                .default_viewer(self.session.document_kind(document)?),
        }
        .cloned()
        .ok_or_else(|| io::Error::other("Document viewer unavailable"))?;
        self.open_plugin_panel(
            descriptor.panel_type,
            Some(document),
            state,
            Some(descriptor.id.to_owned()),
        )
    }

    pub(super) fn open_file_with_viewer(
        &mut self,
        path: &Path,
        explicit: Option<&str>,
        additional: bool,
    ) -> io::Result<bool> {
        if self.project_root.is_empty() {
            let canonical = std::fs::canonicalize(path)?;
            self.set_project(
                canonical
                    .parent()
                    .ok_or_else(|| io::Error::other("File has no parent directory"))?,
            )?;
            if self.pending_workspace.is_some() {
                self.pending_workspace_files.push((
                    canonical,
                    explicit.map(str::to_owned),
                    additional,
                ));
                return Ok(true);
            }
        }
        let descriptor = match explicit {
            Some(id) => Some(
                self.modules
                    .registry
                    .viewer(id)
                    .cloned()
                    .ok_or_else(|| io::Error::other(format!("Viewer unavailable: {id}")))?,
            ),
            None => self
                .modules
                .registry
                .viewer_for_path(&path.to_string_lossy())
                .cloned(),
        };
        let viewer = descriptor
            .as_ref()
            .map(|descriptor| descriptor.id.to_owned());
        let kind = descriptor.as_ref().map(|descriptor| descriptor.kind);
        if let Some(document) = self.session.document_for_path(path) {
            if let Some(explicit) = explicit
                && kind.is_some_and(|kind| self.session.document_kind(document).ok() != Some(kind))
            {
                if kind == Some(DocumentKind::Text)
                    && bed_files::files::classify_bytes(&self.session.snapshot(document)?.bytes)
                        == DocumentKind::Bytes
                {
                    return Err(io::Error::other(
                        "This file contains binary data; keep it open in the Hex Editor or another byte viewer",
                    ));
                }
                let indices = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, tab)| {
                        (tab.panel.document() == Some(document)).then_some(index)
                    })
                    .collect();
                if !self.close_tabs(indices)? {
                    if self.pending_tab_close.is_some() {
                        self.modules.pending_switch = Some((path.to_owned(), explicit.to_owned()));
                    }
                    return Ok(false);
                }
            } else if !additional {
                let matches = |tab: &Tab| {
                    tab.panel.document() == Some(document)
                        && match explicit {
                            None => true,
                            Some(_) => tab.panel.viewer == viewer,
                        }
                };
                let index = self
                    .active_tab_index()
                    .filter(|index| matches(&self.tabs[*index]))
                    .or_else(|| self.tabs.iter().position(matches));
                if let Some(index) = index {
                    self.switch_to_tab(index);
                    return Ok(false);
                }
            } else {
                self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
                return Ok(true);
            }
        }
        let opened = match kind {
            Some(kind) => self.session.request_open_file_with_kind(path, kind)?,
            None => self.session.request_open_file_auto(path)?,
        };
        if let Some(document) = opened {
            self.add_document_panel(document, viewer.as_deref(), &Value::Null)?;
        } else if let Some(path) = path.to_str() {
            self.remote_ui.opening.insert(path.to_owned());
            self.modules.pending_open.insert(path.to_owned(), viewer);
        }
        Ok(true)
    }
    pub(super) fn command_context(&self) -> CommandContext {
        let document = self.active_document();
        CommandContext {
            path: document.and_then(|id| {
                self.session
                    .with_document(id, |state| state.path.clone())
                    .ok()
            }),
            document,
            revision: document.and_then(|id| self.session.document_revision(id).ok()),
            selection: None,
        }
    }
    pub fn focused_hex(&self) -> bool {
        self.tabs.iter().any(|tab| {
            Some(tab.id) == self.focused && tab.panel.kind == bed_module_editor::HEX_PANEL_TYPE
        })
    }
    pub fn focused_document_plugin(&self) -> bool {
        self.tabs.iter().any(|tab| {
            Some(tab.id) == self.focused
                && tab.panel.document().is_some()
                && tab.panel.view_id().is_none()
        })
    }
    /// Route a native action to the panel that actually owns focus.
    pub fn focused_plugin_action(&mut self, action: PanelAction) -> io::Result<bool> {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == self.focused)
        else {
            return Ok(false);
        };
        self.plugin_panel_action(index, action)
    }
    pub(super) fn plugin_panel_action(
        &mut self,
        index: usize,
        action: PanelAction,
    ) -> io::Result<bool> {
        if index >= self.tabs.len() {
            return Ok(false);
        }
        self.refresh_plugins()?;
        let mut tab = self.tabs.remove(index);
        let mut requests = Vec::new();
        let result = self.with_module_services(|modules, services| {
            tab.panel
                .instance
                .action_with_services(action, &modules.frame.context(), services, &mut requests)
                .map_err(io::Error::other)
        });
        self.tabs.insert(index, tab);
        let handled = result?;
        if action == PanelAction::CommitEdit {
            if requests.iter().any(|request| {
                !matches!(
                    request,
                    HostRequest::ApplyEdits { .. } | HostRequest::ApplyEditsWithResult { .. }
                )
            }) {
                return Err(io::Error::other("CommitEdit may only queue document edits"));
            }
            for request in requests {
                match request {
                    HostRequest::ApplyEdits {
                        document,
                        revision,
                        edits,
                    } => {
                        self.session.apply_edits(document, revision, &edits)?;
                    }
                    HostRequest::ApplyEditsWithResult {
                        token,
                        document,
                        revision,
                        edits,
                    } => {
                        self.apply_plugin_edits(token, document, revision, &edits)?;
                    }
                    _ => unreachable!("CommitEdit requests were validated"),
                }
            }
        } else {
            self.modules.requests.extend(requests);
            self.process_plugin_requests_inner(true)?;
        }
        Ok(handled)
    }
    fn apply_plugin_edits(
        &mut self,
        token: EditToken,
        document: DocumentId,
        revision: bed_workbench_api::Revision,
        edits: &[bed_document_session::editor_session::ByteEdit],
    ) -> io::Result<()> {
        let result = self.session.apply_edits(document, revision, edits);
        let acknowledgement = match &result {
            Ok(()) => self
                .session
                .document_revision(document)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        for tab in &mut self.tabs {
            if tab.panel.instance.attached_document() == Some(document) {
                tab.panel
                    .instance
                    .edit_result(token, acknowledgement.clone());
            }
        }
        result
    }
    pub(super) fn commit_plugin_edits(&mut self, document: DocumentId) -> io::Result<()> {
        self.process_plugin_requests_inner(true)?;
        let indices: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (tab.panel.document() == Some(document)).then_some(index))
            .collect();
        for index in indices {
            self.plugin_panel_action(index, PanelAction::CommitEdit)?;
        }
        Ok(())
    }
    pub fn toolbar_commands(&self) -> Vec<crate::commands::CommandItem> {
        let mut commands = crate::commands::core_toolbar_commands();
        commands.retain(|command| command.id != "bed.structure.new");
        let context = self.command_context();
        for item in &self.modules.registry.toolbar {
            if let Some(descriptor) = self.modules.registry.command(item.command) {
                commands.push(crate::commands::CommandItem {
                    id: descriptor.id.to_owned(),
                    label: descriptor.label.to_owned(),
                    icon: descriptor.icon.map(str::to_owned),
                    enabled: self.modules.enabled(descriptor.id, &context),
                });
            }
        }
        for item in &mut commands {
            if item.id.starts_with("bed.editor.split_") {
                item.enabled = self.active_document().is_some();
            }
        }
        commands
    }
    pub fn application_commands(&self) -> Vec<crate::commands::CommandItem> {
        let context = self.command_context();
        self.modules
            .registry
            .menus
            .iter()
            .filter(|item| item.slot == MenuSlot::Application)
            .filter_map(|item| {
                let descriptor = self.modules.registry.command(item.command)?;
                Some(crate::commands::CommandItem {
                    id: descriptor.id.to_owned(),
                    label: descriptor.label.to_owned(),
                    icon: descriptor.icon.map(str::to_owned),
                    enabled: self.modules.enabled(descriptor.id, &context),
                })
            })
            .collect()
    }
    pub fn dispatch_command(&mut self, id: &str) -> io::Result<bool> {
        if self.modules.registry.command(id).is_some() {
            self.refresh_plugins()?;
            let context = self.command_context();
            self.run_module_command(id, &context)?;
            self.process_plugin_requests()?;
            return Ok(true);
        }
        use crate::commands::TitlebarAction;
        if let Some(action) = TitlebarAction::from_command_id(id) {
            let command = match action {
                TitlebarAction::Sidebar => WindowCommand::NewExplorer,
                TitlebarAction::Terminal => WindowCommand::NewTerminal,
                TitlebarAction::Settings => WindowCommand::NewSettings,
                TitlebarAction::Search => WindowCommand::NewContentSearch,
                TitlebarAction::Diagnostics => WindowCommand::NewDiagnostics,
                TitlebarAction::Debug => WindowCommand::Debug,
                TitlebarAction::Structure => WindowCommand::NewStructure,
                TitlebarAction::SplitRight => WindowCommand::SplitRight,
                TitlebarAction::SplitDown => WindowCommand::SplitDown,
            };
            return self.dispatch(command);
        }
        self.refresh_plugins()?;
        let context = self.command_context();
        self.run_module_command(id, &context)?;
        self.process_plugin_requests()?;
        Ok(true)
    }
    pub(super) fn process_plugin_requests(&mut self) -> io::Result<()> {
        self.process_plugin_requests_inner(false)
    }
    pub(super) fn process_plugin_requests_inner(&mut self, strict: bool) -> io::Result<()> {
        let mut failed_edits = HashSet::new();
        let mut first_error = None;
        for request in std::mem::take(&mut self.modules.requests) {
            let edit_document = match &request {
                HostRequest::ApplyEdits { document, .. }
                | HostRequest::ApplyEditsWithResult { document, .. } => Some(*document),
                _ => None,
            };
            if let HostRequest::Save { document } = &request
                && failed_edits.contains(document)
            {
                continue;
            }
            let result: io::Result<()> = (|| {
                match request {
                    HostRequest::FocusView { view } => {
                        if let Some(index) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.panel.view_id() == Some(view))
                        {
                            self.switch_to_tab(index);
                        }
                    }
                    HostRequest::RevealSource {
                        path,
                        row,
                        column,
                        center,
                    } => {
                        self.navigate_file(&path, row, column, center)?;
                    }
                    HostRequest::ShowTerminal { id } => {
                        if let Some(index) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.panel.terminal_id() == Some(id))
                        {
                            self.switch_to_tab(index);
                        } else if self.terminal.session_ids().contains(&id) {
                            self.open_native_panel("terminal", &json!({"session":id}))?;
                        }
                    }
                    HostRequest::Invalidate => {
                        self.scene += 1;
                    }
                    HostRequest::OpenPanel {
                        panel_type,
                        document,
                        state,
                    } => {
                        self.open_plugin_panel(&panel_type, document, &state, None)?;
                    }
                    HostRequest::ShowPanel {
                        panel_type,
                        document,
                        state,
                        action,
                    } => {
                        self.show_registered_panel(&panel_type, document, &state, action)?;
                    }
                    HostRequest::OpenFile { path, viewer } => {
                        self.open_file_with_viewer(
                            Path::new(&path),
                            viewer.as_deref(),
                            viewer.is_some(),
                        )?;
                    }
                    HostRequest::OpenFileDialog { viewer } => {
                        if self.session.is_remote() {
                            self.remote_ui.dialog_viewer = viewer;
                            self.show_remote_path_dialog(None)?;
                        } else if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.open_file_with_viewer(&path, viewer.as_deref(), false)?;
                        }
                    }
                    HostRequest::Navigate {
                        document,
                        byte_offset,
                        revision,
                    } => {
                        if self.session.document_revision(document)? == revision {
                            let (row, column) = self.session.with_document(document, |state| {
                                state.row_col_from_offset(byte_offset)
                            })?;
                            let path = self
                                .session
                                .with_document(document, |state| state.path.clone())?;
                            if self.active_view().is_some_and(|view| {
                                self.session.document_for_view(view) == Some(document)
                            }) {
                                if let Some(index) = self.tabs.iter().position(|tab| {
                                    tab.panel.view_id() == self.active && self.active.is_some()
                                }) {
                                    self.switch_to_tab(index);
                                    self.position_active(row, column, false)?;
                                }
                            } else if path.is_empty() {
                                if let Some(index) = self.tabs.iter().position(|tab| {
                                    tab.panel.view_id().is_some()
                                        && tab.panel.document() == Some(document)
                                }) {
                                    self.switch_to_tab(index);
                                    self.position_active(row, column, false)?;
                                }
                            } else {
                                self.navigate_file(&path, row, column, false)?;
                            }
                        }
                    }
                    HostRequest::ApplyEdits {
                        document,
                        revision,
                        edits,
                    } => self.session.apply_edits(document, revision, &edits)?,
                    HostRequest::ApplyEditsWithResult {
                        token,
                        document,
                        revision,
                        edits,
                    } => {
                        self.apply_plugin_edits(token, document, revision, &edits)?;
                    }
                    HostRequest::Save { document } => {
                        self.ensure_file_operation_idle(document)?;
                        self.commit_plugin_edits(document)?;
                        if self.session.snapshot(document)?.path.is_empty() {
                            self.save_as(document)?;
                        } else {
                            self.session.save(document)?;
                        }
                    }
                    HostRequest::Undo { document } => self.session.undo_document(document)?,
                    HostRequest::Redo { document } => self.session.redo_document(document)?,
                    HostRequest::SetSetting { plugin, key, value } => {
                        if !self.settings.settings["plugins"].is_object() {
                            self.settings.settings["plugins"] = json!({});
                        }
                        let namespace = &mut self.settings.settings["plugins"][plugin];
                        if !namespace.is_object() {
                            *namespace = json!({});
                        }
                        namespace[key] = value;
                        self.settings.save_settings()?;
                    }
                    HostRequest::Notify { message } => self.error = Some(message),
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.error = Some(error.to_string());
                if let Some(document) = edit_document {
                    failed_edits.insert(document);
                }
                if strict && first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn plugin_render_outputs(&self) -> Vec<RenderOutput> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.panel.instance.render_output())
            .collect()
    }
    pub fn render_plugin_output(
        &mut self,
        handle: TextureHandle,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        for tab in &mut self.tabs {
            if tab
                .panel
                .instance
                .render_output()
                .is_some_and(|output| output.handle == handle)
            {
                return tab.panel.instance.render(gpu, target);
            }
        }
        Err("Plugin output no longer exists".into())
    }
    pub fn set_plugin_texture(&mut self, handle: TextureHandle, texture: TextureId) {
        self.modules.frame.textures.insert(handle, texture);
    }
    pub fn retain_plugin_textures(&mut self, live: &HashSet<TextureHandle>) {
        self.modules
            .frame
            .textures
            .retain(|handle, _| live.contains(handle));
    }
    pub fn invalidate_plugin_textures(&mut self) {
        self.modules.frame.textures.clear();
    }
}

/// Adapter for the workbench's shared terminal service. Visibility and process
/// lifetime are separate: a module may retain a session after its panel closes.
struct WorkbenchTerminals<'a> {
    terminal: &'a mut BedTerminal,
    visible: HashSet<u64>,
    retained: HashSet<u64>,
    fonts: &'a TerminalFonts,
}
impl TerminalService for WorkbenchTerminals<'_> {
    fn spawn(&mut self, launch: TerminalLaunch) -> io::Result<(u64, u32)> {
        let program = launch
            .args
            .first()
            .ok_or_else(|| io::Error::other("Terminal command is empty"))?;
        let mut options = bed_terminal::terminal_pty::PtyOptions {
            working_directory: launch.cwd,
            shell: Some(bed_terminal::terminal_pty::TerminalShell::new(
                program,
                launch.args[1..].to_vec(),
            )),
            ..Default::default()
        };
        for (name, value) in launch.env {
            if let Some(value) = value {
                options.env.insert(name, value);
            } else {
                options.env_remove.push(name);
            }
        }
        self.terminal.new_command_session(options, launch.title)
    }
    fn new_shell(&mut self, cwd: Option<&Path>) -> io::Result<u64> {
        Ok(match cwd {
            Some(path) => self.terminal.new_session_at(path),
            None => self.terminal.new_session(),
        })
    }
    fn render(&mut self, ui: &Ui, id: u64) -> io::Result<()> {
        self.terminal.render_session(ui, self.fonts, id).map(|_| ())
    }
    fn title(&self, id: u64) -> Option<String> {
        self.terminal.session_title(id)
    }
    fn working_directory(&self, id: u64) -> Option<PathBuf> {
        self.terminal.working_directory(id).map(Path::to_owned)
    }
    fn is_command(&self, id: u64) -> bool {
        self.terminal.is_command_session(id)
    }
    fn focus(&mut self, id: u64) {
        self.terminal.focus_session(id);
    }
    fn close_panel(&mut self, id: u64) {
        if !self.visible.contains(&id) && !self.retained.contains(&id) {
            self.terminal.close_session_id(id);
        }
    }
    fn stop(&mut self, id: u64) {
        self.terminal.stop_session_id(id);
    }
    fn release(&mut self, id: u64) {
        if !self.visible.contains(&id) {
            self.terminal.close_session_id(id);
        }
    }
}

impl Workbench {
    pub(super) fn with_module_services<R>(
        &mut self,
        f: impl FnOnce(&mut ModuleRuntime, &mut ModuleServices<'_>) -> io::Result<R>,
    ) -> io::Result<R> {
        let visible = self
            .tabs
            .iter()
            .filter_map(|tab| tab.panel.terminal_id())
            .collect();
        let retained = self
            .terminal
            .session_ids()
            .into_iter()
            .filter(|id| {
                self.modules
                    .instances
                    .iter()
                    .any(|module| module.retains_terminal(*id))
            })
            .collect();
        let mut terminals = WorkbenchTerminals {
            terminal: &mut self.terminal,
            visible,
            retained,
            fonts: &self.terminal_fonts,
        };
        let mut dialogs = NativeFileDialogs;
        let mut resources = bed_workbench_api::ScopedServices::default();
        resources.insert(&mut self.settings);
        resources.insert(&mut self.icons);
        let mut services = ModuleServices {
            dialogs: &mut dialogs,
            documents: &mut self.session,
            terminals: &mut terminals,
            project_root: &self.project_root,
            active_view: self.active,
            resources,
            settings_ui: None,
        };
        f(&mut self.modules, &mut services)
    }
    pub(super) fn run_module_command(
        &mut self,
        command: &str,
        context: &CommandContext,
    ) -> io::Result<()> {
        self.with_module_services(|modules, services| modules.command(command, context, services))
    }
    pub(super) fn shutdown_modules(&mut self) -> io::Result<()> {
        self.with_module_services(|modules, services| {
            for module in &mut modules.instances {
                module.shutdown(services);
            }
            Ok(())
        })
    }
    pub(super) fn module_shortcuts(&mut self, ui: &Ui) -> io::Result<()> {
        if self.focused_terminal()
            || self.active_overlay() != Overlay::None
            || self.file_dialog.is_some()
            || self.reload_confirmation.is_some()
            || self.file_operations.modal_visible()
            || self.modules.explorer.finder_visible()
        {
            return Ok(());
        }
        self.with_module_services(|modules, services| {
            let host = modules.frame.context();
            for module in &mut modules.instances {
                module.shortcuts(ui, &host, services, &mut modules.requests)?;
            }
            Ok(())
        })?;
        self.process_plugin_requests()
    }
    pub(super) fn restore_module_settings(&mut self) {
        for module in &mut self.modules.instances {
            let state = self
                .workspace_spec
                .as_ref()
                .and_then(|spec| self.store.as_ref()?.module_settings(spec, module.id()));
            module.restore_workspace(state, &self.project_root);
        }
    }
    pub(super) fn persist_module_settings(&mut self) -> io::Result<()> {
        if self.session.is_remote() {
            return Ok(());
        }
        let settings: serde_json::Map<String, Value> = self
            .modules
            .instances
            .iter()
            .filter_map(|module| {
                let state = module.save_workspace();
                (!state.is_null()).then(|| (module.id().to_owned(), state))
            })
            .collect();
        if let (Some(store), Some(spec)) = (&mut self.store, &self.workspace_spec) {
            store.save_module_settings(spec, Value::Object(settings))?;
        }
        Ok(())
    }
}

struct NativeFileDialogs;
impl bed_workbench_api::FileDialogService for NativeFileDialogs {
    fn pick_file(&mut self, directory: &Path, extensions: &[&str]) -> Option<PathBuf> {
        let dialog = rfd::FileDialog::new().set_directory(directory);
        if extensions.is_empty() {
            dialog.pick_file()
        } else {
            dialog.add_filter("Files", extensions).pick_file()
        }
    }
}

impl Workbench {
    pub(super) fn draw_module_panel(&mut self, ui: &Ui, panel: &mut HostedPanel) -> io::Result<()> {
        let visible = self
            .tabs
            .iter()
            .filter_map(|tab| tab.panel.terminal_id())
            .collect();
        let retained = self
            .terminal
            .session_ids()
            .into_iter()
            .filter(|id| {
                self.modules
                    .instances
                    .iter()
                    .any(|module| module.retains_terminal(*id))
            })
            .collect();
        let mut terminals = WorkbenchTerminals {
            terminal: &mut self.terminal,
            visible,
            retained,
            fonts: &self.terminal_fonts,
        };
        let mut dialogs = NativeFileDialogs;
        let mut resources = bed_workbench_api::ScopedServices::default();
        resources.insert(&mut self.settings);
        resources.insert(&mut self.icons);
        let mut settings_ui = RegisteredSettings {
            registry: &self.modules.registry,
            modules: &mut self.modules.instances,
        };
        let mut services = ModuleServices {
            dialogs: &mut dialogs,
            documents: &mut self.session,
            terminals: &mut terminals,
            project_root: &self.project_root,
            active_view: self.active,
            resources,
            settings_ui: Some(&mut settings_ui),
        };
        panel.instance.draw_with_services(
            ui,
            &self.modules.frame.context(),
            &mut services,
            &mut self.modules.requests,
        )
    }
    pub(super) fn panel_placement(&self, panel: &Panel) -> PanelPlacement {
        self.modules
            .registry
            .panel(&panel.kind)
            .map_or(PanelPlacement::Center, |descriptor| descriptor.placement)
    }
    pub(super) fn focus_module_panel(&mut self, index: usize) -> io::Result<()> {
        let mut tab = self.tabs.remove(index);
        let result = self
            .with_module_services(|_, services| tab.panel.instance.focus_with_services(services));
        self.tabs.insert(index, tab);
        result
    }
    pub(super) fn open_native_panel(&mut self, legacy: &str, state: &Value) -> io::Result<u64> {
        let kind = self
            .modules
            .registry
            .panels
            .iter()
            .find(|panel| panel.legacy_kind == Some(legacy))
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {legacy}")))?
            .id;
        self.open_plugin_panel(kind, None, state, None)
    }
    pub(super) fn show_native_panel(
        &mut self,
        legacy: &str,
        action: Option<PanelAction>,
    ) -> io::Result<u64> {
        let kind = self
            .modules
            .registry
            .panels
            .iter()
            .find(|panel| panel.legacy_kind == Some(legacy))
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {legacy}")))?
            .id;
        self.show_registered_panel(kind, None, &Value::Null, action)
    }
    fn show_registered_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
        action: Option<PanelAction>,
    ) -> io::Result<u64> {
        let id = if let Some(index) = self.tabs.iter().position(|tab| {
            tab.panel.kind == kind && document.is_none_or(|id| tab.panel.document() == Some(id))
        }) {
            self.switch_to_tab(index);
            self.select_dock_tab(index);
            self.tabs[index].id
        } else {
            self.open_plugin_panel(kind, document, state, None)?
        };
        if let Some(action) = action
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.plugin_panel_action(index, action)?;
        }
        Ok(id)
    }
    fn select_dock_tab(&self, index: usize) {
        if !self.dock_built {
            return;
        }
        let Some(binding) = &self.context_binding else {
            return;
        };
        let name = std::ffi::CString::new(format!("###bed_tab_{}", self.tabs[index].id)).unwrap();
        binding.with_bound_context(|| unsafe {
            let window = sys::igFindWindowByName(name.as_ptr());
            if !window.is_null() {
                let bar = bed_imgui_dock_node_tab_bar((*window).DockNode);
                if !bar.is_null() {
                    (*bar).SelectedTabId = (*window).TabId;
                    (*bar).NextSelectedTabId = (*window).TabId;
                }
            }
        });
    }
    fn place_registered_panel(&mut self, id: u64, placement: PanelPlacement) {
        if placement == PanelPlacement::Center || !self.dock_built || self.center_dock == 0 {
            return;
        }
        let Some(binding) = &self.context_binding else {
            return;
        };
        let (mut placed, mut center) = (0, 0);
        let (direction, fraction) = match placement {
            PanelPlacement::Bottom => (sys::ImGuiDir_Down, 0.4),
            PanelPlacement::Sidebar => (sys::ImGuiDir_Left, 0.2),
            PanelPlacement::Center => return,
        };
        binding.with_bound_context(|| unsafe {
            sys::igDockBuilderSplitNode(
                self.center_dock,
                direction,
                fraction,
                &mut placed,
                &mut center,
            );
            sys::igDockBuilderFinish(self.dock_root);
        });
        self.center_dock = center;
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.dock = Some(placed);
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/module_state_tests.rs"]
mod module_state_tests;

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/native_module_tests.rs"]
mod native_module_tests;

/// Contributions render against the canonical settings value borrowed by the
/// Settings panel. Each feature owns its section and persistence stays shared.
struct RegisteredSettings<'a> {
    registry: &'a Registry,
    modules: &'a mut [Box<dyn Plugin>],
}
impl bed_workbench_api::SettingsContributions for RegisteredSettings<'_> {
    fn draw(&mut self, ui: &Ui, values: &mut Value) -> bool {
        let mut changed = false;
        if !values["plugins"].is_object() {
            values["plugins"] = json!({});
        }
        for section in &self.registry.settings {
            let _id = ui.push_id(section.id);
            if ui.collapsing_header(section.label, dear_imgui_rs::TreeNodeFlags::empty())
                && let Some(module) = self
                    .modules
                    .iter_mut()
                    .find(|module| module.id() == section.plugin)
            {
                let namespace = &mut values["plugins"][section.plugin];
                if !namespace.is_object() {
                    *namespace = json!({});
                }
                changed |= module.draw_settings(section.id, ui, namespace);
            }
        }
        changed
    }
}
