//! Application adapter for explicitly linked native feature plugins.
use super::*;
use bed_plugin::{
    CommandContext, HostContext, HostRequest, MenuSlot, Plugin, PluginDocument, PluginPanel,
    Registry, TextureHandle,
};
use dear_imgui_rs::TextureId;
use std::sync::Arc;

pub type PluginTextureImage = (TextureHandle, [u32; 2], Arc<[u8]>, u64);

#[cfg(test)]
#[path = "plugin_host_tests.rs"]
mod tests;

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
pub(super) struct TextureImage {
    pub size: [u32; 2],
    pub rgba: Arc<[u8]>,
    pub revision: u64,
}
pub(super) struct PluginRuntime {
    pub registry: Registry,
    pub instances: Vec<Box<dyn Plugin>>,
    pub frame: PluginFrame,
    pub requests: Vec<HostRequest>,
    pub images: HashMap<TextureHandle, TextureImage>,
    pub pending_open: HashMap<String, Option<String>>,
    pending_switch: Option<(PathBuf, String)>,
    texture_revision: u64,
}
impl Default for PluginRuntime {
    fn default() -> Self {
        let instances: Vec<Box<dyn Plugin>> = vec![
            Box::new(bed_plugin_structure::StructurePlugin::default()),
            Box::new(bed_plugin_image::ImagePlugin::default()),
        ];
        let mut registry = Registry::default();
        for plugin in &instances {
            registry
                .register(plugin.as_ref())
                .expect("bundled plugin registration must be valid");
        }
        Self {
            registry,
            instances,
            frame: PluginFrame::default(),
            requests: Vec::new(),
            images: HashMap::new(),
            pending_open: HashMap::new(),
            pending_switch: None,
            texture_revision: 0,
        }
    }
}
impl PluginRuntime {
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        state: &Value,
        viewer: Option<String>,
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
            .create_panel(kind, document, state)
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
    pub(super) fn command(&mut self, command: &str, context: &CommandContext) -> io::Result<()> {
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
            plugin.command(command, context, &self.frame.context(), &mut self.requests);
        }
        Ok(())
    }
}

impl Workbench {
    pub(super) fn draw_tree_plugin_menu(
        plugins: &PluginRuntime,
        ui: &Ui,
        path: &str,
        directory: bool,
        background: bool,
        actions: &mut Vec<FileTreeAction>,
    ) {
        let document = plugins.frame.documents.iter().find(|doc| doc.path == path);
        let context = CommandContext {
            path: Some(path.to_owned()),
            document: document.map(|doc| doc.id),
            revision: document.map(|doc| doc.revision),
            selection: None,
        };
        let slot = if background {
            MenuSlot::TreeBackground
        } else if directory {
            MenuSlot::Folder
        } else {
            MenuSlot::File
        };
        let items: Vec<_> = plugins
            .registry
            .menus
            .iter()
            .filter(|item| item.slot == slot)
            .collect();
        if !items.is_empty() || !directory {
            ui.separator();
        }
        for item in items {
            if let Some(command) = plugins.registry.command(item.command) {
                let _id = ui.push_id(command.id);
                if ui.menu_item_enabled_selected_no_shortcut(
                    command.label,
                    false,
                    plugins.enabled(command.id, &context),
                ) {
                    actions.push(FileTreeAction::Command {
                        command: command.id.to_owned(),
                        context: context.clone(),
                    });
                }
            }
        }
        if !directory && let Some(_menu) = ui.begin_menu("Open With") {
            for (id, label) in [("bed.text", "Text Editor"), ("bed.hex", "Hex Editor")]
                .into_iter()
                .chain(
                    plugins
                        .registry
                        .viewers
                        .iter()
                        .map(|viewer| (viewer.id, viewer.label)),
                )
            {
                let _id = ui.push_id(id);
                if ui.menu_item(label) {
                    actions.push(FileTreeAction::Command {
                        command: format!("bed.open_with:{id}"),
                        context: context.clone(),
                    });
                }
            }
        }
    }
    pub(super) fn capture_editor_context(&mut self, view: &EditorView) -> io::Result<()> {
        let document = view.document_id();
        let revision = self.session.document_revision(document)?;
        let selections = self.session.view_snapshot(view.id())?.selections;
        let (path, ranges, text) = self.session.with_document(document, |state| {
            let mut ranges = Vec::new();
            let mut selected = Vec::new();
            for selection in selections.iter().filter(|selection| !selection.empty()) {
                let (ar, ac, br, bc) = selection.ordered();
                let range = state.offset_from_row_col(ar, ac)..state.offset_from_row_col(br, bc);
                let mut bytes = vec![0; range.len()];
                state.copy_bytes(range.start, range.len(), &mut bytes);
                selected.push(String::from_utf8_lossy(&bytes).into_owned());
                ranges.push(range);
            }
            (state.path.clone(), ranges, selected.join("\n"))
        })?;
        self.editor_menu_context.insert(
            view.id(),
            CommandContext {
                path: Some(path),
                document: Some(document),
                revision: Some(revision),
                selection: Some(bed_plugin::SelectionContext {
                    document,
                    revision,
                    ranges,
                    text,
                }),
            },
        );
        Ok(())
    }
    pub(super) fn draw_editor_plugin_menu(&mut self, ui: &Ui, view: &EditorView) -> io::Result<()> {
        self.refresh_plugins()?;
        if !self.editor_menu_context.contains_key(&view.id()) {
            self.capture_editor_context(view)?;
        }
        let context = self.editor_menu_context[&view.id()].clone();
        let commands: Vec<_> = self
            .plugins
            .registry
            .menus
            .iter()
            .filter(|item| item.slot == MenuSlot::TextSelection)
            .filter_map(|item| self.plugins.registry.command(item.command).cloned())
            .collect();
        for command in commands {
            let _id = ui.push_id(command.id);
            if ui.menu_item_enabled_selected_no_shortcut(
                command.label,
                false,
                self.plugins.enabled(command.id, &context),
            ) {
                self.plugins.command(command.id, &context)?;
            }
        }
        Ok(())
    }
    pub(super) fn refresh_plugins(&mut self) -> io::Result<()> {
        let ids = self.session.document_ids();
        self.plugins
            .frame
            .documents
            .retain(|doc| ids.contains(&doc.id));
        for id in ids {
            let revision = self.session.document_revision(id)?;
            let existing = self
                .plugins
                .frame
                .documents
                .iter()
                .position(|doc| doc.id == id);
            if let Some(index) =
                existing.filter(|&index| self.plugins.frame.documents[index].revision == revision)
            {
                self.session.with_document(id, |state| {
                    let cached = &mut self.plugins.frame.documents[index];
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
                    self.plugins.frame.documents[index] = document;
                } else {
                    self.plugins.frame.documents.push(document);
                }
            }
        }
        self.plugins.frame.active_document = self.active_document();
        self.plugins.frame.settings = self.settings.settings["plugins"].clone();
        if !self.plugins.frame.settings.is_object() {
            self.plugins.frame.settings = json!({});
        }
        self.plugins.frame.animations = self.settings.bool("ui_animations", true);
        self.plugins.frame.workspace = self.session.workspace_id().0;
        let mut diagnostics = serde_json::Map::new();
        for doc in &self.plugins.frame.documents {
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
        self.plugins.frame.diagnostics = Value::Object(diagnostics);
        Ok(())
    }
    pub(super) fn tick_plugins(&mut self) -> io::Result<()> {
        self.refresh_plugins()?;
        let host = self.plugins.frame.context();
        for plugin in &mut self.plugins.instances {
            plugin.tick(&host, &mut self.plugins.requests);
        }
        self.process_plugin_requests()?;
        if let Some((path, viewer)) = self.plugins.pending_switch.take() {
            if self.session.document_for_path(&path).is_none() {
                self.open_file_with_viewer(&path, Some(&viewer), false)?;
            } else if self.pending_tab_close.is_some() {
                self.plugins.pending_switch = Some((path, viewer));
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
        let panel = self.plugins.create_panel(kind, document, state, viewer)?;
        if let Some(document) = panel.instance.attached_document() {
            self.session.document_kind(document)?;
        }
        Ok(self.push_panel(Panel::Plugin(panel)))
    }
    pub(super) fn close_plugin_panels(&mut self) -> io::Result<()> {
        for tab in &mut self.tabs {
            if let Panel::Plugin(panel) = &mut tab.panel {
                panel.instance.close(&mut self.plugins.requests);
            }
        }
        self.process_plugin_requests()
    }
    pub(super) fn add_document_panel(
        &mut self,
        document: DocumentId,
        viewer: Option<&str>,
        state: &Value,
    ) -> io::Result<u64> {
        if let Some(viewer) = viewer.filter(|id| *id != "bed.text" && *id != "bed.hex") {
            let descriptor = self
                .plugins
                .registry
                .viewer(viewer)
                .cloned()
                .ok_or_else(|| io::Error::other(format!("Viewer unavailable: {viewer}")))?;
            return self.open_plugin_panel(
                descriptor.panel_type,
                Some(document),
                state,
                Some(viewer.to_owned()),
            );
        }
        if self.session.document_kind(document)? == DocumentKind::Bytes {
            let mut editor = bed_ui::hex_editor::HexEditor::new(document);
            editor.restore_state(state);
            Ok(self.push_panel(Panel::Hex(editor)))
        } else {
            self.add_view(document)
        }
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
        }
        let descriptor = match explicit {
            Some("bed.text" | "bed.hex") => None,
            Some(id) => Some(
                self.plugins
                    .registry
                    .viewer(id)
                    .cloned()
                    .ok_or_else(|| io::Error::other(format!("Viewer unavailable: {id}")))?,
            ),
            None => self
                .plugins
                .registry
                .viewer_for_path(&path.to_string_lossy())
                .cloned(),
        };
        let viewer = explicit.map(str::to_owned).or_else(|| {
            descriptor
                .as_ref()
                .map(|descriptor| descriptor.id.to_owned())
        });
        let kind = match explicit {
            Some("bed.text") => Some(DocumentKind::Text),
            Some("bed.hex") => Some(DocumentKind::Bytes),
            _ => descriptor.as_ref().map(|descriptor| descriptor.kind),
        };
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
                        self.plugins.pending_switch = Some((path.to_owned(), explicit.to_owned()));
                    }
                    return Ok(false);
                }
            } else if !additional {
                let matches = |tab: &Tab| {
                    tab.panel.document() == Some(document)
                        && match explicit {
                            None => true,
                            Some("bed.text") => matches!(tab.panel, Panel::Document(_)),
                            Some("bed.hex") => matches!(tab.panel, Panel::Hex(_)),
                            Some(viewer) => {
                                matches!(&tab.panel, Panel::Plugin(panel) if panel.viewer.as_deref() == Some(viewer))
                            }
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
            self.plugins.pending_open.insert(path.to_owned(), viewer);
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
        self.tabs
            .iter()
            .any(|tab| Some(tab.id) == self.focused && matches!(tab.panel, Panel::Hex(_)))
    }
    pub fn toolbar_commands(&self) -> Vec<crate::util::command_ui::CommandItem> {
        let mut commands = crate::util::command_ui::core_toolbar_commands();
        commands.retain(|command| command.id != "bed.structure.new");
        let context = self.command_context();
        for item in &self.plugins.registry.toolbar {
            if let Some(descriptor) = self.plugins.registry.command(item.command) {
                commands.push(crate::util::command_ui::CommandItem {
                    id: descriptor.id.to_owned(),
                    label: descriptor.label.to_owned(),
                    icon: descriptor.icon.map(str::to_owned),
                    enabled: self.plugins.enabled(descriptor.id, &context),
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
    pub fn application_commands(&self) -> Vec<crate::util::command_ui::CommandItem> {
        let context = self.command_context();
        self.plugins
            .registry
            .menus
            .iter()
            .filter(|item| item.slot == MenuSlot::Application)
            .filter_map(|item| {
                let descriptor = self.plugins.registry.command(item.command)?;
                Some(crate::util::command_ui::CommandItem {
                    id: descriptor.id.to_owned(),
                    label: descriptor.label.to_owned(),
                    icon: descriptor.icon.map(str::to_owned),
                    enabled: self.plugins.enabled(descriptor.id, &context),
                })
            })
            .collect()
    }
    pub fn dispatch_command(&mut self, id: &str) -> io::Result<bool> {
        use crate::util::command_ui::TitlebarAction;
        if let Some(action) = TitlebarAction::from_command_id(id) {
            let command = match action {
                TitlebarAction::Sidebar => WindowCommand::NewExplorer,
                TitlebarAction::Terminal => WindowCommand::NewTerminal,
                TitlebarAction::Settings => WindowCommand::NewSettings,
                TitlebarAction::Search => WindowCommand::NewContentSearch,
                TitlebarAction::Diagnostics => WindowCommand::NewDiagnostics,
                TitlebarAction::Structure => WindowCommand::NewStructure,
                TitlebarAction::SplitRight => WindowCommand::SplitRight,
                TitlebarAction::SplitDown => WindowCommand::SplitDown,
            };
            return self.dispatch(command);
        }
        self.refresh_plugins()?;
        let context = self.command_context();
        self.plugins.command(id, &context)?;
        self.process_plugin_requests()?;
        Ok(true)
    }
    pub(super) fn process_plugin_requests(&mut self) -> io::Result<()> {
        for request in std::mem::take(&mut self.plugins.requests) {
            let result: io::Result<()> = (|| {
                match request {
                    HostRequest::OpenPanel {
                        panel_type,
                        document,
                        state,
                    } => {
                        self.open_plugin_panel(&panel_type, document, &state, None)?;
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
                                if let Some(index) = self.tabs.iter().position(|tab| matches!(&tab.panel, Panel::Document(view) if Some(view.id()) == self.active)) {
                                    self.switch_to_tab(index);
                                    self.position_active(row, column, false)?;
                                }
                            } else if path.is_empty() {
                                if let Some(index) = self.tabs.iter().position(|tab| matches!(&tab.panel, Panel::Document(view) if view.document_id() == document)) { self.switch_to_tab(index); self.position_active(row, column, false)?; }
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
                    HostRequest::Save { document } => {
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
                    HostRequest::UploadTexture { handle, size, rgba } => {
                        let length = u64::from(size[0])
                            .checked_mul(u64::from(size[1]))
                            .and_then(|pixels| pixels.checked_mul(4))
                            .ok_or_else(|| {
                                io::Error::other("Image dimensions exceed texture limits")
                            })?;
                        if size.contains(&0)
                            || length != rgba.len() as u64
                            || length > 64 * 1024 * 1024
                        {
                            return Err(io::Error::other("Image dimensions exceed texture limits"));
                        }
                        self.plugins.texture_revision += 1;
                        self.plugins.images.insert(
                            handle,
                            TextureImage {
                                size,
                                rgba,
                                revision: self.plugins.texture_revision,
                            },
                        );
                    }
                    HostRequest::ReleaseTexture { handle } => {
                        self.plugins.images.remove(&handle);
                        self.plugins.frame.textures.remove(&handle);
                    }
                    HostRequest::Notify { message } => self.error = Some(message),
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        Ok(())
    }
    pub fn plugin_texture_images(&self) -> Vec<PluginTextureImage> {
        self.plugins
            .images
            .iter()
            .map(|(&handle, image)| (handle, image.size, Arc::clone(&image.rgba), image.revision))
            .collect()
    }
    pub fn set_plugin_texture(&mut self, handle: TextureHandle, texture: TextureId) {
        self.plugins.frame.textures.insert(handle, texture);
    }
    pub fn invalidate_plugin_textures(&mut self) {
        self.plugins.frame.textures.clear();
    }
}
