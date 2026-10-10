//! Application adapter for built-in modules and explicitly linked plugins.
use super::*;
use bed_workbench_api::gpu::{GpuContext, RenderOutput, RenderTarget};
use bed_workbench_api::{
    CommandContext, EditToken, HostContext, HostRequest, MenuSlot, ModuleServices, PanelAction,
    PanelInput, PanelPlacement, Plugin, PluginDocument, PluginPanel, Registry, TerminalLaunch,
    TerminalService, TextureHandle,
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
    pub input: PanelInput,
    pub kind: String,
    pub viewer: Option<String>,
    pub instance: Box<dyn PluginPanel>,
    pub inactive: HashMap<String, HostedPanel>,
    pub saved_presentations: HashMap<String, Value>,
}

impl HostedPanel {
    pub(super) fn presentations_mut(&mut self) -> impl Iterator<Item = &mut dyn PluginPanel> {
        std::iter::once(self.instance.as_mut()).chain(
            self.inactive
                .values_mut()
                .map(|panel| panel.instance.as_mut()),
        )
    }

    pub(super) fn close_presentations(
        &mut self,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        for instance in self.presentations_mut() {
            instance.close_with_services(services, requests)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct PluginFrame {
    pub remote: bool,
    pub documents: Vec<PluginDocument>,
    pub active_document: Option<DocumentId>,
    pub settings: Value,
    pub textures: HashMap<TextureHandle, TextureId>,
    pub animations: bool,
    pub workspace: u64,
    pub diagnostics: Value,
    pub default_viewers: Value,
}
impl PluginFrame {
    pub fn context(&self) -> HostContext<'_> {
        HostContext {
            remote: self.remote,
            viewer_menu: None,
            documents: &self.documents,
            active_document: self.active_document,
            settings: &self.settings,
            textures: &self.textures,
            animations: self.animations,
            workspace: self.workspace,
            diagnostics: &self.diagnostics,
            default_viewers: &self.default_viewers,
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
    pub pending_additional: HashSet<String>,
    pub(super) pending_saves: Vec<super::save_barrier::PendingSave>,
    pub(super) pending_switch: Option<(PathBuf, String, Option<u32>)>,
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
            pending_additional: HashSet::new(),
            pending_saves: Vec::new(),
            pending_switch: None,
        }
    }
}
impl ModuleRuntime {
    fn create_panel(
        &mut self,
        kind: &str,
        input: PanelInput,
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
            .create_panel_with_services(kind, input.clone(), state, services)
            .map_err(io::Error::other)?;
        let instance: Box<dyn PluginPanel> = Box::new(super::panel_boundary::PanelBoundary::new(
            kind, instance, state,
        ));
        if input.local_file().is_some()
            && (instance.attached_document().is_some() || instance.view_id().is_some())
        {
            return Err(io::Error::other(
                "A local-file panel cannot attach an editor document",
            ));
        }
        let input = if input.local_file().is_some() {
            input
        } else {
            instance.attached_document().into()
        };
        Ok(HostedPanel {
            input,
            kind: kind.to_owned(),
            viewer,
            instance,
            inactive: HashMap::new(),
            saved_presentations: HashMap::new(),
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
    fn visible(&self, command: &str, context: &CommandContext) -> bool {
        self.registry
            .command(command)
            .and_then(|descriptor| {
                self.instances
                    .iter()
                    .find(|plugin| plugin.id() == descriptor.plugin)
            })
            .is_none_or(|plugin| plugin.command_visible(command, context, &self.frame.context()))
    }
    fn label(&self, command: &str, context: &CommandContext) -> Option<String> {
        let descriptor = self.registry.command(command)?;
        self.instances
            .iter()
            .find(|plugin| plugin.id() == descriptor.plugin)?
            .command_label(command, context, &self.frame.context())
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
        self.modules.frame.remote = self.session.is_remote();
        self.modules.frame.default_viewers = self.settings.settings["default_viewers"].clone();
        self.modules.frame.settings = self.settings.settings["plugins"].clone();
        if !self.modules.frame.settings.is_object() {
            self.modules.frame.settings = json!({});
        }
        self.modules.frame.animations = self.settings.bool("ui_animations", true);
        self.modules.frame.workspace = self.session.workspace_id().0;
        let mut diagnostics = serde_json::Map::new();
        for (path, items) in self.session.project_diagnostics().by_path {
            diagnostics.insert(path, Value::Array(items.into_iter().map(|item| json!({
                "range":{"start":{"line":item.start_line,"character":item.start_character},"end":{"line":item.end_line,"character":item.end_character}},
                "severity":item.severity,"message":item.message,"source":item.source })).collect()));
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
        if let Some((path, viewer, area)) = self.modules.pending_switch.take() {
            if self.session.document_for_path(&path).is_none() {
                self.open_file_with_viewer_at(&path, Some(&viewer), false, area)?;
            } else if self.pending_tab_close.is_some() {
                self.modules.pending_switch = Some((path, viewer, area));
            }
        }
        Ok(())
    }
    pub(super) fn open_plugin_panel(
        &mut self,
        kind: &str,
        input: impl Into<PanelInput>,
        state: &Value,
        viewer: Option<String>,
    ) -> io::Result<u64> {
        self.open_plugin_panel_at(kind, input, state, viewer, None)
    }

    pub(super) fn open_plugin_panel_in_area(
        &mut self,
        kind: &str,
        input: impl Into<PanelInput>,
        state: &Value,
        viewer: Option<String>,
        area: u32,
    ) -> io::Result<u64> {
        let id = self.open_plugin_panel_at(kind, input, state, viewer, Some(area))?;
        self.place_panel_in_area(id, area);
        Ok(id)
    }

    pub(super) fn open_plugin_panel_at(
        &mut self,
        kind: &str,
        input: impl Into<PanelInput>,
        state: &Value,
        viewer: Option<String>,
        area: Option<u32>,
    ) -> io::Result<u64> {
        self.open_plugin_panel_with_reuse(kind, input.into(), state, viewer, area, false)
    }

    fn open_plugin_panel_with_reuse(
        &mut self,
        kind: &str,
        input: PanelInput,
        state: &Value,
        viewer: Option<String>,
        area: Option<u32>,
        reuse_empty: bool,
    ) -> io::Result<u64> {
        if area.is_some_and(|id| {
            !self
                .current_tiling()
                .layout
                .areas
                .iter()
                .any(|value| value.id == id)
        }) {
            return Err(io::Error::other("This area is no longer available"));
        }
        let descriptor = self
            .modules
            .registry
            .panel(kind)
            .cloned()
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {kind}")))?;
        if descriptor.legacy_kind == Some("terminal") {
            self.ensure_shell_integration()?;
        }
        if descriptor.singleton
            && let Some(index) = self.tabs.iter().position(|tab| tab.panel.kind == kind)
        {
            self.switch_to_tab(index);
            let id = self.tabs[index].id;
            return Ok(id);
        }
        if kind == bed_module_explorer::PANEL_ID {
            self.ensure_files_directory()?;
        }
        self.refresh_plugins()?;
        let empty = reuse_empty
            .then(|| self.empty_input_slot(kind, viewer.as_deref(), area))
            .flatten();
        let mut panel = self.with_module_services(|modules, services| {
            modules.create_panel(kind, input, state, viewer, services)
        })?;
        if let Some(document) = panel.instance.attached_document() {
            self.session.document_kind(document)?;
        }
        if let Some(index) = empty {
            let previous_document = self.tabs[index].panel.document();
            let mut tabs = std::mem::take(&mut self.tabs);
            let result = self.with_module_services(|_, services| {
                // Empty panels own no user work. Close resources without dispatching
                // arbitrary requests in the middle of this replacement transition.
                tabs[index]
                    .panel
                    .close_presentations(services, &mut Vec::new())
            });
            self.tabs = tabs;
            if let Err(error) = result {
                let document = panel.document();
                let _ = self.with_module_services(|_, services| {
                    panel.close_presentations(services, &mut Vec::new())
                });
                drop(panel);
                if let Some(document) = document
                    && !self
                        .tabs
                        .iter()
                        .any(|tab| tab.panel.document() == Some(document))
                    && !self.session.snapshot(document)?.dirty
                {
                    self.session
                        .close_document(document, ClosePolicy::Discard)?;
                }
                return Err(error);
            }
            let previous = std::mem::replace(&mut self.tabs[index].panel, panel);
            drop(previous);
            if let Some(document) = previous_document {
                self.session
                    .close_document(document, ClosePolicy::Discard)?;
            }
            self.sync_local_file_protections();
            self.switch_to_tab(index);
            let id = self.tabs[index].id;
            self.refresh_plugins()?;
            self.scene += 1;
            return Ok(id);
        }
        let id = self.push_panel(panel);
        let destination = area.unwrap_or_else(|| {
            self.area_for_panel(id)
                .expect("new panel belongs to an area")
        });
        self.place_panel_in_area(id, destination);
        self.refresh_plugins()?;
        Ok(id)
    }

    fn empty_input_slot(
        &self,
        kind: &str,
        viewer: Option<&str>,
        area: Option<u32>,
    ) -> Option<usize> {
        let compatible = |tab: &Tab| {
            tab.panel.kind == kind
                && tab.panel.viewer.as_deref() == viewer
                && self.panel_input_empty(&tab.panel)
        };
        let target = area.unwrap_or_else(|| self.focused_or_last_area());
        self.tabs
            .iter()
            .position(|tab| {
                Some(tab.id) == self.focused
                    && self.area_for_panel(tab.id) == Some(target)
                    && compatible(tab)
            })
            .or_else(|| {
                let groups = &self.current_tiling().areas;
                groups
                    .iter()
                    .filter(|group| group.area == target)
                    .chain(
                        groups
                            .iter()
                            .filter(|group| group.area != target && area.is_none()),
                    )
                    .flat_map(|group| &group.tabs)
                    .find_map(|id| {
                        self.tabs
                            .iter()
                            .position(|tab| tab.id == *id && compatible(tab))
                    })
            })
    }
    pub(super) fn close_plugin_panels(&mut self) -> io::Result<()> {
        let mut tabs = std::mem::take(&mut self.tabs);
        let result = self.with_module_services(|modules, services| {
            for tab in &mut tabs {
                tab.panel
                    .close_presentations(services, &mut modules.requests)?;
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
        self.add_document_panel_at(document, viewer, state, None)
    }

    pub(super) fn add_document_panel_at(
        &mut self,
        document: DocumentId,
        viewer: Option<&str>,
        state: &Value,
        area: Option<u32>,
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
        if !descriptor.supports_document_kind(self.session.document_kind(document)?) {
            return Err(io::Error::other(
                "Viewer requires a different document kind",
            ));
        }
        self.open_plugin_panel_at(
            descriptor.panel_type,
            PanelInput::Document(document),
            state,
            Some(descriptor.id.to_owned()),
            area,
        )
    }

    pub(super) fn add_opened_file_panel_at(
        &mut self,
        document: DocumentId,
        viewer: &str,
        area: Option<u32>,
        reuse_empty: bool,
    ) -> io::Result<u64> {
        let descriptor = self
            .modules
            .registry
            .viewer(viewer)
            .cloned()
            .ok_or_else(|| io::Error::other("Document viewer unavailable"))?;
        if !descriptor.supports_document_kind(self.session.document_kind(document)?) {
            return Err(io::Error::other(
                "Viewer requires a different document kind",
            ));
        }
        self.open_plugin_panel_with_reuse(
            descriptor.panel_type,
            PanelInput::Document(document),
            &Value::Null,
            Some(descriptor.id.to_owned()),
            area,
            reuse_empty,
        )
    }

    pub(super) fn resolved_document_viewer(
        &self,
        document: DocumentId,
        explicit: Option<&str>,
    ) -> io::Result<String> {
        let (path, kind) = self
            .session
            .with_document(document, |state| (state.path.clone(), state.kind))?;
        let viewer = match explicit {
            Some(id) => self.modules.registry.viewer(id),
            None => self.modules.registry.preferred_viewer_for_file(
                &path,
                kind,
                &self.settings.settings["default_viewers"],
            ),
        }
        .filter(|viewer| !viewer.is_local_file())
        .or_else(|| {
            if explicit.is_none() {
                self.modules.registry.default_viewer(kind)
            } else {
                None
            }
        })
        .ok_or_else(|| io::Error::other("Document viewer unavailable"))?;
        if !viewer.supports_document_kind(kind) {
            return Err(io::Error::other(
                "Viewer requires a different document kind",
            ));
        }
        Ok(viewer.id.to_owned())
    }

    pub(super) fn switch_document_viewer(
        &mut self,
        tab_id: u64,
        document: DocumentId,
        viewer: &str,
    ) -> io::Result<()> {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id && tab.panel.document() == Some(document))
        else {
            return Ok(());
        };
        if self.session.is_snapshot_document(document) {
            return Err(io::Error::other("Comparison panels cannot switch viewers"));
        }
        if self.tabs[index].panel.viewer.is_none() {
            return Err(io::Error::other("Panel is not a file viewer"));
        }
        let descriptor = self
            .modules
            .registry
            .viewer(viewer)
            .cloned()
            .ok_or_else(|| io::Error::other("Viewer unavailable"))?;
        let snapshot = self.session.snapshot(document)?;
        let path = if snapshot.path.is_empty() {
            self.session
                .original_path(document)?
                .unwrap_or_default()
                .to_owned()
        } else {
            snapshot.path.clone()
        };
        if (!descriptor.fallback && !descriptor.matches_path(&path))
            || !descriptor.supports_document_kind(snapshot.kind)
        {
            return Err(io::Error::other(
                "Viewer is incompatible with this document",
            ));
        }
        if self.tabs[index].panel.viewer.as_deref() == Some(descriptor.id) {
            return Ok(());
        }
        self.plugin_panel_action(index, PanelAction::CommitEdit)?;
        let mut next = if let Some(panel) = self.tabs[index].panel.inactive.remove(descriptor.id) {
            panel
        } else {
            let state = self.tabs[index]
                .panel
                .saved_presentations
                .get(descriptor.id)
                .cloned()
                .unwrap_or(Value::Null);
            self.with_module_services(|modules, services| {
                modules.create_panel(
                    descriptor.panel_type,
                    PanelInput::Document(document),
                    &state,
                    Some(descriptor.id.to_owned()),
                    services,
                )
            })?
        };
        // The tab owns every presentation; a switch never closes its document.
        std::mem::swap(&mut next, &mut self.tabs[index].panel);
        let previous = next
            .viewer
            .clone()
            .ok_or_else(|| io::Error::other("Panel is not a file viewer"))?;
        self.tabs[index].panel.inactive = std::mem::take(&mut next.inactive);
        self.tabs[index].panel.saved_presentations = std::mem::take(&mut next.saved_presentations);
        self.tabs[index]
            .panel
            .saved_presentations
            .remove(descriptor.id);
        self.tabs[index].panel.inactive.insert(previous, next);
        self.focused = None;
        self.active = None;
        self.switch_to_tab(index);
        self.refresh_plugins()?;
        self.scene += 1;
        Ok(())
    }

    pub(super) fn add_local_file_panel(
        &mut self,
        path: &Path,
        viewer: &bed_workbench_api::FileViewer,
        state: &Value,
    ) -> io::Result<u64> {
        self.add_local_file_panel_at(path, viewer, state, None)
    }

    fn add_local_file_panel_at(
        &mut self,
        path: &Path,
        viewer: &bed_workbench_api::FileViewer,
        state: &Value,
        area: Option<u32>,
    ) -> io::Result<u64> {
        if self.session.is_remote() {
            return Err(io::Error::other("This viewer opens local files only"));
        }
        if !viewer.is_local_file() {
            return Err(io::Error::other("Viewer requires an editor document"));
        }
        self.ensure_local_file_open_idle()?;
        let path = std::fs::canonicalize(path)?;
        if !path.is_file() {
            return Err(io::Error::other("Choose a database file"));
        }
        self.ensure_no_editable_local_file(&path)?;
        self.open_plugin_panel_at(
            viewer.panel_type,
            PanelInput::LocalFile(path),
            state,
            Some(viewer.id.to_owned()),
            area,
        )
    }

    pub(super) fn open_file_with_viewer(
        &mut self,
        path: &Path,
        explicit: Option<&str>,
        additional: bool,
    ) -> io::Result<bool> {
        self.open_file_with_viewer_target(path, explicit, additional, None, false)
    }

    pub(super) fn open_file_with_viewer_at(
        &mut self,
        path: &Path,
        explicit: Option<&str>,
        additional: bool,
        area: Option<u32>,
    ) -> io::Result<bool> {
        self.open_file_with_viewer_target(path, explicit, additional, area, true)
    }

    pub(super) fn open_file_from_menu(
        &mut self,
        path: &Path,
        explicit: Option<&str>,
        additional: bool,
        area: Option<u32>,
    ) -> io::Result<bool> {
        self.open_file_with_viewer_target(path, explicit, additional, area, false)
    }

    fn open_file_with_viewer_target(
        &mut self,
        path: &Path,
        explicit: Option<&str>,
        additional: bool,
        area: Option<u32>,
        move_existing: bool,
    ) -> io::Result<bool> {
        if area.is_some_and(|id| {
            !self
                .current_tiling()
                .layout
                .areas
                .iter()
                .any(|value| value.id == id)
        }) {
            return Err(io::Error::other("This area is no longer available"));
        }
        self.sync_services()?;
        let descriptor = explicit
            .map(|id| {
                self.modules
                    .registry
                    .viewer(id)
                    .cloned()
                    .ok_or_else(|| io::Error::other(format!("Viewer unavailable: {id}")))
            })
            .transpose()?;
        let descriptor = if explicit.is_none()
            && !self.session.is_remote()
            && self.modules.registry.viewers.iter().any(|viewer| {
                viewer.is_local_file() && viewer.matches_path(&path.to_string_lossy())
            }) {
            use std::io::Read;
            let file = std::fs::File::open(path)?;
            let mut prefix = Vec::with_capacity(bed_remote::MAX_FILE_PREFIX_BYTES);
            file.take(bed_remote::MAX_FILE_PREFIX_BYTES as u64)
                .read_to_end(&mut prefix)?;
            self.modules
                .registry
                .preferred_viewer_for_file(
                    &path.to_string_lossy(),
                    bed_files::files::classify_bytes(&prefix),
                    &self.settings.settings["default_viewers"],
                )
                .filter(|viewer| viewer.is_local_file())
                .cloned()
                .or(descriptor)
        } else {
            descriptor
        };
        if let Some(viewer) = descriptor.as_ref().filter(|viewer| viewer.is_local_file()) {
            if self.session.is_remote() {
                return Err(io::Error::other("SQLite Viewer opens local databases only"));
            }
            let canonical = std::fs::canonicalize(path)?;
            if !canonical.is_file() {
                return Err(io::Error::other("Choose a database file"));
            }
            if !additional
                && let Some(index) = self.tabs.iter().position(|tab| {
                    tab.panel.input.local_file() == Some(canonical.as_path())
                        && tab.panel.viewer.as_deref() == Some(viewer.id)
                })
            {
                self.switch_to_tab(index);
                if move_existing && let Some(area) = area {
                    self.place_panel_in_area(self.tabs[index].id, area);
                }
                return Ok(false);
            }
            self.ensure_local_file_open_idle()?;
            self.ensure_no_editable_local_file(&canonical)?;
            self.open_plugin_panel_with_reuse(
                viewer.panel_type,
                PanelInput::LocalFile(canonical),
                &Value::Null,
                Some(viewer.id.to_owned()),
                area,
                !additional,
            )?;
            return Ok(true);
        }
        if !self.session.is_remote() {
            self.session.ensure_local_path_editable(path)?;
        }
        if let Some(document) = self.session.document_for_path(path) {
            let current_kind = self.session.document_kind(document)?;
            if descriptor
                .as_ref()
                .is_some_and(|v| v.document_kind() == Some(DocumentKind::Text))
                && self.session.with_document(document, |state| {
                    let mut prefix = [0; bed_remote::MAX_FILE_PREFIX_BYTES];
                    let len = state.byte_size().min(prefix.len());
                    state.copy_bytes(0, len, &mut prefix);
                    bed_files::files::classify_bytes(&prefix[..len]) == DocumentKind::Bytes
                })?
            {
                return Err(io::Error::other(
                    "This file contains binary data; keep it open in the Hex Editor or another byte viewer",
                ));
            }
            if let Some(descriptor) = &descriptor
                && !descriptor.supports_document_kind(current_kind)
            {
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
                        self.modules.pending_switch =
                            Some((path.to_owned(), descriptor.id.to_owned(), area));
                    }
                    return Ok(false);
                }
            } else {
                if !additional {
                    let matches = |tab: &Tab| {
                        tab.panel.document() == Some(document)
                            && descriptor
                                .as_ref()
                                .is_none_or(|v| tab.panel.viewer.as_deref() == Some(v.id))
                    };
                    let index = self
                        .active_tab_index()
                        .filter(|index| matches(&self.tabs[*index]))
                        .or_else(|| self.tabs.iter().position(matches));
                    if let Some(index) = index {
                        self.switch_to_tab(index);
                        if move_existing && let Some(area) = area {
                            self.place_panel_in_area(self.tabs[index].id, area);
                        }
                        return Ok(false);
                    }
                }
                let viewer = self.resolved_document_viewer(document, explicit)?;
                self.add_opened_file_panel_at(document, &viewer, area, !additional)?;
                return Ok(true);
            }
        }
        // Explicit byte-only tools retain the existing exact-byte conversion path.
        let opened = match descriptor.as_ref().and_then(|v| v.single_document_kind()) {
            Some(kind) => self.session.request_open_file_with_kind(path, kind)?,
            None => self.session.request_open_file_auto(path)?,
        };
        if let Some(document) = opened {
            let viewer = self.resolved_document_viewer(document, explicit)?;
            self.add_opened_file_panel_at(document, &viewer, area, !additional)?;
        } else if let Some(path) = path.to_str() {
            self.remote_ui.opening.insert(path.to_owned());
            self.modules
                .pending_open
                .insert(path.to_owned(), explicit.map(str::to_owned));
            if additional {
                self.modules.pending_additional.insert(path.to_owned());
            } else {
                self.modules.pending_additional.remove(path);
            }
            if let Some(area) = area {
                self.remote_ui.drop_areas.insert(path.to_owned(), area);
            }
        }
        Ok(true)
    }
    pub(super) fn command_context(&self) -> CommandContext {
        if let Some(path) = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.focused)
            .and_then(|tab| tab.panel.input.local_file())
        {
            return CommandContext {
                path: Some(path.to_string_lossy().into_owned()),
                ..CommandContext::default()
            };
        }
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
            for instance in tab.panel.presentations_mut() {
                if instance.attached_document() == Some(document) {
                    instance.edit_result(token, acknowledgement.clone());
                }
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
        use crate::commands::TitlebarAction;

        let mut commands = crate::commands::core_toolbar_commands();
        let context = self.command_context();
        for item in &self.modules.registry.toolbar {
            if let Some(descriptor) = self.modules.registry.command(item.command) {
                let index = if descriptor.id == bed_module_git::SHOW_COMMAND {
                    commands
                        .iter()
                        .position(|command| command.id == TitlebarAction::Debug.command_id())
                        .map(|index| index + 1)
                } else if descriptor.id == "bed.structure.open" {
                    commands
                        .iter()
                        .position(|command| command.id == TitlebarAction::Structure.command_id())
                } else {
                    commands
                        .iter()
                        .position(|command| command.id == TitlebarAction::Settings.command_id())
                }
                .unwrap_or(commands.len());
                commands.insert(
                    index,
                    crate::commands::CommandItem {
                        id: descriptor.id.to_owned(),
                        label: descriptor.label.to_owned(),
                        icon: descriptor.icon.map(str::to_owned),
                        enabled: self.modules.enabled(descriptor.id, &context),
                    },
                );
            }
        }
        commands.retain(|command| command.id != TitlebarAction::Structure.command_id());
        for item in &mut commands {
            if item.id.starts_with("bed.editor.split_") {
                item.enabled = self.active_tab_index().is_some() || self.focused_terminal();
            }
            if let Some(label) = self.modules.label(&item.id, &context) {
                item.label = label;
            }
        }
        commands.retain(|item| self.command_visible(&item.id));
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
                if !self.command_visible(descriptor.id) {
                    return None;
                }
                Some(crate::commands::CommandItem {
                    id: descriptor.id.to_owned(),
                    label: self
                        .modules
                        .label(descriptor.id, &context)
                        .unwrap_or_else(|| descriptor.label.to_owned()),
                    icon: descriptor.icon.map(str::to_owned),
                    enabled: self.modules.enabled(descriptor.id, &context),
                })
            })
            .collect()
    }
    pub fn dispatch_command(&mut self, id: &str) -> io::Result<bool> {
        self.dispatch_command_at(id, None)
    }
    /// Menu and toolbar commands create content in the selected area.
    pub fn dispatch_command_from_menu(&mut self, id: &str) -> io::Result<bool> {
        self.dispatch_command_with_target(id, PanelTarget::LastFocused)
    }
    pub fn dispatch_command_with_target(
        &mut self,
        id: &str,
        target: PanelTarget,
    ) -> io::Result<bool> {
        let area = self.panel_target_area(target);
        self.dispatch_command_at(id, Some(area))
    }
    pub(super) fn panel_target_area(&self, target: PanelTarget) -> u32 {
        match target {
            PanelTarget::LargestArea => self.largest_area(),
            PanelTarget::LastFocused => self.focused_or_last_area(),
            PanelTarget::PreviousFocused => self
                .previous_focused_area()
                .unwrap_or_else(|| self.largest_area()),
        }
    }
    pub(super) fn dispatch_command_at(&mut self, id: &str, area: Option<u32>) -> io::Result<bool> {
        if let Some(command) = crate::commands::EditorZoomCommand::from_command_id(id) {
            return Ok(self.dispatch_editor_zoom(command));
        }
        if !self.command_visible(id) {
            return Ok(false);
        }
        use crate::commands::TitlebarAction;
        if self.modules.registry.command(id).is_none()
            && let Some(action) = TitlebarAction::from_command_id(id)
        {
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
            return self.dispatch_at(command, area);
        }
        self.refresh_plugins()?;
        let context = self.command_context();
        self.run_module_command_at(id, &context, area)?;
        Ok(true)
    }
    fn command_visible(&self, id: &str) -> bool {
        if self.workspace_spec.is_none()
            && [
                "bed.debug.",
                "bed.diagnostics.",
                "bed.lsp.",
                "bed.references.",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        {
            return false;
        }
        if id.starts_with("bed.editor.split_")
            && self.active_tab_index().is_none()
            && !self.focused_terminal()
        {
            return false;
        }
        self.modules.visible(id, &self.command_context())
    }
    pub(super) fn process_plugin_requests(&mut self) -> io::Result<()> {
        self.process_plugin_requests_inner(false)
    }
    pub(super) fn process_plugin_requests_inner(&mut self, strict: bool) -> io::Result<()> {
        let requests = std::mem::take(&mut self.modules.requests);
        self.process_plugin_requests_at(strict, None, requests)
    }
    fn process_plugin_requests_at(
        &mut self,
        strict: bool,
        area: Option<u32>,
        requests: Vec<HostRequest>,
    ) -> io::Result<()> {
        let mut failed_edits = HashSet::new();
        let mut first_error = None;
        // Resolve destinations before processing the batch: opening its first
        // panel must not redirect subsequent requests by changing focus.
        let destinations: Vec<_> = requests
            .iter()
            .map(|request| match request {
                HostRequest::OpenFileIn { target, .. }
                | HostRequest::ShowPanelIn { target, .. } => Some(self.panel_target_area(*target)),
                _ => area,
            })
            .collect();
        for (request, area) in requests.into_iter().zip(destinations) {
            let edit_document = match &request {
                HostRequest::ApplyEdits { document, .. }
                | HostRequest::ApplyEditsWithResult { document, .. }
                | HostRequest::ApplyModuleEditsWithResult { document, .. } => Some(*document),
                _ => None,
            };
            if let HostRequest::Save { document } = &request
                && failed_edits.contains(document)
            {
                continue;
            }
            let result: io::Result<()> = (|| {
                match request {
                    HostRequest::ClosePanels { panel_type } => {
                        self.close_panels_of_type(&panel_type)?;
                    }
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
                        self.navigate_file_at(&path, row, column, center, area)?;
                    }
                    HostRequest::ShowTerminal { id } => {
                        if let Some(index) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.panel.terminal_id() == Some(id))
                        {
                            self.switch_to_tab(index);
                        } else if self.terminal.session_ids().contains(&id) {
                            self.open_native_panel_at("terminal", &json!({"session":id}), area)?;
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
                        let has_input = document.is_some() || !state.is_null();
                        self.open_plugin_panel_with_reuse(
                            &panel_type,
                            document.into(),
                            &state,
                            None,
                            area,
                            has_input,
                        )?;
                    }
                    HostRequest::ShowPanel {
                        panel_type,
                        document,
                        state,
                        action,
                    }
                    | HostRequest::ShowPanelIn {
                        panel_type,
                        document,
                        state,
                        action,
                        ..
                    } => {
                        self.show_registered_panel_at(&panel_type, document, &state, action, area)?;
                    }
                    HostRequest::OpenFile { path, viewer }
                    | HostRequest::OpenFileIn { path, viewer, .. } => {
                        self.open_file_from_menu(Path::new(&path), viewer.as_deref(), false, area)?;
                    }
                    HostRequest::SwitchViewer {
                        tab,
                        document,
                        viewer,
                    } => {
                        self.switch_document_viewer(tab, document, &viewer)?;
                    }
                    HostRequest::OpenFileDialog { viewer } => {
                        self.open_file_dialog_at(viewer, area)?;
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
                                self.navigate_file_at(&path, row, column, false, area)?;
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
                    HostRequest::ApplyModuleEditsWithResult {
                        recipient,
                        token,
                        document,
                        revision,
                        edits,
                    } => {
                        let result = (|| -> io::Result<_> {
                            self.ensure_file_operation_idle(document)?;
                            if self.remote_ui.document_mutating(document) {
                                return Err(io::Error::other(
                                    "Wait for the remote file action before replacing",
                                ));
                            }
                            self.commit_plugin_edits(document)?;
                            self.session.apply_edits(document, revision, &edits)?;
                            self.session.document_revision(document)
                        })();
                        if let Some(module) = self
                            .modules
                            .instances
                            .iter_mut()
                            .find(|module| module.id() == recipient)
                        {
                            module.edit_result(
                                token,
                                result.as_ref().copied().map_err(ToString::to_string),
                            );
                        }
                        result?;
                    }
                    HostRequest::SaveDocumentsWithResult {
                        recipient,
                        token,
                        documents,
                    } => {
                        if documents
                            .iter()
                            .any(|document| failed_edits.contains(document))
                        {
                            self.module_save_result(
                                &recipient,
                                token,
                                Err("An edit was rejected before saving".into()),
                            );
                        } else {
                            self.begin_module_save(recipient, token, documents);
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
    fn active_session(&self) -> Option<u64> {
        self.terminal.active_session_id()
    }
    fn live_working_directory(&self, id: u64) -> io::Result<PathBuf> {
        self.terminal.live_working_directory(id)
    }
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
        let directory = self.working_directory().to_string_lossy().into_owned();
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
        let mut filesystem = self
            .modules
            .explorer
            .borrow()
            .file_finder
            .workspace_filesystem();
        if let Some(filesystem) = &mut filesystem {
            resources.insert(filesystem);
        }
        let mut services = ModuleServices {
            dialogs: &mut dialogs,
            documents: &mut self.session,
            terminals: &mut terminals,
            project_root: &self.project_root,
            working_directory: &directory,
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
        if command == bed_module_explorer::FIND_COMMAND {
            self.ensure_files_directory()?;
        }
        self.with_module_services(|modules, services| modules.command(command, context, services))
    }
    pub(super) fn run_module_command_at(
        &mut self,
        command: &str,
        context: &CommandContext,
        area: Option<u32>,
    ) -> io::Result<()> {
        // Only this menu command's requests inherit its selected area. Pending
        // panel/tick requests and any replies they queue use default placement.
        let first_command_request = if area.is_some() {
            self.modules.requests.len()
        } else {
            0
        };
        self.run_module_command(command, context)?;
        let requests = self.modules.requests.split_off(first_command_request);
        if area.is_some() {
            self.process_plugin_requests()?;
        }
        self.process_plugin_requests_at(false, area, requests)
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
        let Some(spec) = &self.workspace_spec else {
            return;
        };
        for module in &mut self.modules.instances {
            let state = self
                .store
                .as_ref()
                .and_then(|store| store.module_settings(spec, module.id()));
            module.restore_workspace(state, &self.project_root);
        }
    }
    pub(super) fn persist_module_settings(&mut self) -> io::Result<()> {
        let Some(spec) = &self.workspace_spec else {
            return Ok(());
        };
        let settings: serde_json::Map<String, Value> = self
            .modules
            .instances
            .iter()
            .filter_map(|module| {
                let state = module.save_workspace();
                (!state.is_null()).then(|| (module.id().to_owned(), state))
            })
            .collect();
        if let Some(store) = &mut self.store {
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
    pub(super) fn draw_module_panel(
        &mut self,
        ui: &Ui,
        tab: u64,
        panel: &mut HostedPanel,
    ) -> io::Result<()> {
        let directory = self.working_directory().to_string_lossy().into_owned();
        let viewer_menu =
            panel
                .document()
                .zip(panel.viewer.as_ref())
                .and_then(|(document, current)| {
                    if self.session.is_snapshot_document(document) {
                        return None;
                    }
                    let (path, kind) = self
                        .session
                        .with_document(document, |state| (state.path.clone(), state.kind))
                        .ok()?;
                    let path = if path.is_empty() {
                        self.session
                            .original_path(document)
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                            .to_owned()
                    } else {
                        path
                    };
                    let choices = self
                        .modules
                        .registry
                        .compatible_viewers(&path, Some(kind))
                        .into_iter()
                        .filter(|viewer| viewer.supports_document_kind(kind))
                        .map(|viewer| bed_workbench_api::ViewerChoice {
                            id: viewer.id.to_owned(),
                            label: viewer.label.to_owned(),
                            text_editor: viewer.fallback
                                && viewer.document_kind() == Some(DocumentKind::Text),
                        })
                        .collect::<Vec<_>>();
                    choices.iter().any(|v| &v.id != current).then(|| {
                        bed_workbench_api::ViewerMenu {
                            tab,
                            document,
                            current: current.clone(),
                            choices,
                        }
                    })
                });
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
        let mut filesystem = self
            .modules
            .explorer
            .borrow()
            .file_finder
            .workspace_filesystem();
        if let Some(filesystem) = &mut filesystem {
            resources.insert(filesystem);
        }
        let mut settings_ui = RegisteredSettings {
            registry: &self.modules.registry,
            modules: &mut self.modules.instances,
        };
        let mut services = ModuleServices {
            dialogs: &mut dialogs,
            documents: &mut self.session,
            terminals: &mut terminals,
            project_root: &self.project_root,
            working_directory: &directory,
            active_view: self.active,
            resources,
            settings_ui: Some(&mut settings_ui),
        };
        let mut host = self.modules.frame.context();
        host.viewer_menu = viewer_menu.as_ref();
        panel
            .instance
            .draw_with_services(ui, &host, &mut services, &mut self.modules.requests)?;
        if viewer_menu.is_some() {
            let right_click = ui.with_bound_context(|| unsafe {
                let io = &*dear_imgui_rs::sys::igGetIO_Nil();
                io.MouseDragMaxDistanceSqr[1] < io.MouseDragThreshold * io.MouseDragThreshold
            });
            if right_click
                && ui.is_mouse_released(dear_imgui_rs::MouseButton::Right)
                && ui.is_window_hovered_with_flags(
                    dear_imgui_rs::WindowHoveredFlags::ROOT_AND_CHILD_WINDOWS,
                )
                && !ui.is_popup_open_with_flags(
                    "",
                    dear_imgui_rs::PopupQueryFlags::ANY_POPUP_ID
                        | dear_imgui_rs::PopupQueryFlags::ANY_POPUP_LEVEL,
                )
            {
                ui.open_popup("##document-viewers");
            }
            let _style = bed_ui::util::popup_style::context_menu_style(ui);
            if let Some(_popup) = ui.begin_popup("##document-viewers") {
                host.draw_viewer_menu(ui, &mut self.modules.requests);
            }
        }
        Ok(())
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
        self.open_native_panel_at(legacy, state, None)
    }
    pub(super) fn open_native_panel_at(
        &mut self,
        legacy: &str,
        state: &Value,
        area: Option<u32>,
    ) -> io::Result<u64> {
        let kind = self
            .modules
            .registry
            .panels
            .iter()
            .find(|panel| panel.legacy_kind == Some(legacy))
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {legacy}")))?
            .id;
        self.open_plugin_panel_at(kind, None, state, None, area)
    }
    pub(super) fn show_native_panel_at(
        &mut self,
        legacy: &str,
        action: Option<PanelAction>,
        area: Option<u32>,
    ) -> io::Result<u64> {
        let kind = self
            .modules
            .registry
            .panels
            .iter()
            .find(|panel| panel.legacy_kind == Some(legacy))
            .ok_or_else(|| io::Error::other(format!("Panel unavailable: {legacy}")))?
            .id;
        self.show_registered_panel_at(kind, None, &Value::Null, action, area)
    }
    fn show_registered_panel_at(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
        action: Option<PanelAction>,
        area: Option<u32>,
    ) -> io::Result<u64> {
        let has_input = document.is_some() || !state.is_null();
        let id = if let Some(index) = self.tabs.iter().position(|tab| {
            tab.panel.kind == kind
                && match document {
                    Some(id) => tab.panel.document() == Some(id),
                    None => !has_input || !self.panel_input_empty(&tab.panel),
                }
        }) {
            self.switch_to_tab(index);
            self.tabs[index].id
        } else {
            self.open_plugin_panel_with_reuse(kind, document.into(), state, None, area, has_input)?
        };
        if let Some(action) = action
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.plugin_panel_action(index, action)?;
        }
        Ok(id)
    }
    pub(super) fn select_dock_tab(&self, index: usize) {
        let Some(binding) = &self.context_binding else {
            return;
        };
        let name = std::ffi::CString::new(format!("###bed_tab_{}", self.tabs[index].id)).unwrap();
        binding.with_bound_context(|| unsafe {
            let window = sys::igFindWindowByName(name.as_ptr());
            if !window.is_null() {
                bed_imgui_dock_node_select_window((*window).DockNode, window);
            }
        });
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
    fn viewers(&self) -> &[bed_workbench_api::FileViewer] {
        &self.registry.viewers
    }
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
