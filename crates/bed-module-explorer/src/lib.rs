//! Files feature: tree panels, finder popup and concrete tree extension points.
//! Filesystem mutations and workspace navigation are requested from the host.
mod file_content;
pub mod file_finder;
pub mod file_info;
pub mod file_tree;
pub mod files;

use self::{
    file_content::{ContentStatus, FileContent},
    file_finder::{FileFinderAction, FileFinderStyle},
    file_tree::{FileNode, FileTreeAction, FileTreePreferences, FileTreeStyle},
    files::FileExplorer,
};
use bed_remote::RemoteClient;
use bed_ui::icons::{Icons, icon_key_for_file};
use bed_ui::presentation::FileIcons;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, ModuleServices,
    PanelPlacement, Registrar, Registry,
};
use dear_imgui_rs::{TextureId, Ui};
use serde_json::Value;
use std::{
    any::Any,
    cell::{Ref, RefCell, RefMut},
    collections::{BTreeMap, HashMap},
    io,
    rc::{Rc, Weak},
};

pub const MODULE_ID: &str = "bed.explorer";
pub const PANEL_ID: &str = "bed.explorer.panel";
pub const SHOW_COMMAND: &str = "bed.explorer.show";
pub const NEW_COMMAND: &str = "bed.explorer.new";
pub const FIND_COMMAND: &str = "bed.explorer.find_file";

/// A popup's originating row, captured before focus or selection can change.
#[derive(Clone, Debug)]
pub struct TreeMenuContext {
    pub command: CommandContext,
    /// Complete selection captured when this popup opened.
    pub selected_paths: Vec<String>,
    pub directory: bool,
    pub background: bool,
}
impl TreeMenuContext {
    pub fn slot(&self) -> MenuSlot {
        if self.background {
            MenuSlot::TreeBackground
        } else if self.directory {
            MenuSlot::Folder
        } else {
            MenuSlot::File
        }
    }
}

/// A Files-specific extension point; other panel types need not implement it.
pub trait TreeMenuExtension {
    /// Existing extensions operate on one originating path until they opt in.
    fn supports_multiple_selection(&self) -> bool {
        false
    }
    fn draw(
        &self,
        ui: &Ui,
        context: &TreeMenuContext,
        host: &HostContext<'_>,
        actions: &mut Vec<FileTreeAction>,
    );
}

#[derive(Clone, Default)]
pub struct ExplorerExtensions {
    menus: Rc<RefCell<Vec<Weak<dyn TreeMenuExtension>>>>,
}
impl ExplorerExtensions {
    pub fn register_menu(&self, provider: &Rc<dyn TreeMenuExtension>) {
        let mut menus = self.menus.borrow_mut();
        menus.retain(|entry| entry.strong_count() != 0);
        let registration = Rc::downgrade(provider);
        if !menus.iter().any(|entry| entry.ptr_eq(&registration)) {
            menus.push(registration);
        }
    }
    fn draw(
        &self,
        ui: &Ui,
        context: &TreeMenuContext,
        host: &HostContext<'_>,
        actions: &mut Vec<FileTreeAction>,
    ) {
        let providers: Vec<_> = self
            .menus
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for provider in providers {
            let _disabled = ui.begin_disabled_with_cond(
                context.selected_paths.len() > 1 && !provider.supports_multiple_selection(),
            );
            provider.draw(ui, context, host, actions);
        }
    }
}

/// GPU texture handles only; the application retains icon pixels and uploads.
#[derive(Clone, Default)]
pub struct IconTextures(pub BTreeMap<String, TextureId>);
impl FileIcons for IconTextures {
    fn get(&self, name: &str) -> Option<TextureId> {
        self.0.get(name).or_else(|| self.0.get("default")).copied()
    }
    fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.get(icon_key_for_file(filename))
    }
    fn file_icon_tint(&self, _filename: &str, text: [f32; 4]) -> [f32; 4] {
        text
    }
}

#[derive(Clone)]
struct MenuCommand {
    id: String,
    label: String,
    slot: MenuSlot,
    enabled: HashMap<String, bool>,
}

#[derive(Clone, Default)]
struct RegistryMenus {
    commands: Vec<MenuCommand>,
    viewers: Rc<Registry>,
}
impl RegistryMenus {
    fn content_status(
        &self,
        path: &str,
        host: &HostContext<'_>,
        content: &RefCell<FileContent>,
    ) -> ContentStatus {
        let mut content = content.borrow_mut();
        let resolved = content.resolved_path(path).unwrap_or(path);
        match host
            .documents
            .iter()
            .find(|document| document.path == path || document.path == resolved)
        {
            Some(document) => {
                // A cached disk prefix must be refreshed after this document
                // closes, since its unsaved bytes can become the new disk data.
                content.use_open_document(path);
                ContentStatus::Ready(bed_files::files::classify_bytes(&document.bytes))
            }
            None => content.inspect(path),
        }
    }

    fn draw(
        &self,
        ui: &Ui,
        context: &TreeMenuContext,
        host: &HostContext<'_>,
        content: &RefCell<FileContent>,
        actions: &mut Vec<FileTreeAction>,
    ) {
        let path = context.command.path.as_deref().unwrap_or_default();
        let items: Vec<_> = self
            .commands
            .iter()
            .filter(|item| item.slot == context.slot())
            .collect();
        let single_file =
            !context.directory && !context.background && context.selected_paths.len() == 1;
        if !items.is_empty() || single_file {
            ui.separator();
        }
        for item in items {
            let _id = ui.push_id(&item.id);
            if ui.menu_item_enabled_selected_no_shortcut(
                &item.label,
                false,
                context.selected_paths.len() <= 1
                    && item.enabled.get(path).copied().unwrap_or(false),
            ) {
                actions.push(FileTreeAction::Command {
                    command: item.id.clone(),
                    context: context.command.clone(),
                });
            }
        }
        if single_file {
            let status = self.content_status(path, host, content);
            if let Some(_menu) = ui.begin_menu("Open With") {
                let default = status
                    .kind()
                    .and_then(|kind| {
                        self.viewers
                            .preferred_viewer_for_file(path, kind, host.default_viewers)
                    })
                    .map(|viewer| viewer.id);
                for viewer in self.viewers.compatible_viewers(path, status.kind()) {
                    let _id = ui.push_id(viewer.id);
                    let label = if Some(viewer.id) == default {
                        format!("{} (default)", viewer.label)
                    } else {
                        viewer.label.into()
                    };
                    if ui.menu_item(label) {
                        actions.push(FileTreeAction::Command {
                            command: format!("bed.open_with:{}", viewer.id),
                            context: context.command.clone(),
                        });
                    }
                }
                match &status {
                    ContentStatus::Loading => ui.text_disabled("Checking file type…"),
                    ContentStatus::Unavailable(error) => {
                        ui.text_disabled("File type unavailable");
                        if ui.is_item_hovered() {
                            ui.tooltip_text(error);
                        }
                    }
                    ContentStatus::Ready(_) => {}
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct ExplorerConfig {
    pub tree_style: FileTreeStyle,
    pub finder_style: FileFinderStyle,
    pub icons: IconTextures,
    pub workspace_identity: String,
    pub extensions: ExplorerExtensions,
    registry_menus: Rc<RegistryMenus>,
}

/// Work requiring application services is drained after drawing, never while a
/// tree or its popup holds mutable widget state.
pub enum ExplorerAction {
    Tree(FileTreeAction),
    OpenProjectDialog,
    Import {
        paths: Vec<std::path::PathBuf>,
        destination: String,
    },
    PreferencesChanged(FileTreePreferences),
    RefreshRemote {
        force: bool,
    },
}

#[derive(Clone)]
pub struct ExplorerHandle {
    pub file_explorer: Rc<RefCell<FileExplorer>>,
    pub config: Rc<RefCell<ExplorerConfig>>,
    actions: Rc<RefCell<Vec<ExplorerAction>>>,
    file_content: Rc<RefCell<FileContent>>,
}
impl Default for ExplorerHandle {
    fn default() -> Self {
        Self {
            file_explorer: Rc::new(RefCell::new(FileExplorer::new())),
            config: Rc::new(RefCell::new(ExplorerConfig::default())),
            actions: Rc::default(),
            file_content: Rc::default(),
        }
    }
}
impl ExplorerHandle {
    pub fn borrow(&self) -> Ref<'_, FileExplorer> {
        self.file_explorer.borrow()
    }
    pub fn borrow_mut(&self) -> RefMut<'_, FileExplorer> {
        self.file_explorer.borrow_mut()
    }
    pub fn config(&self) -> RefMut<'_, ExplorerConfig> {
        self.config.borrow_mut()
    }
    pub fn extensions(&self) -> ExplorerExtensions {
        self.config.borrow().extensions.clone()
    }
    pub fn take_actions(&self) -> Vec<ExplorerAction> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }
    /// Lifecycle consumers drain the same stream used by tree/finder discovery.
    pub fn take_filesystem_updates(&self) -> Vec<bed_remote::WorkspaceUpdate> {
        let mut state = self.file_explorer.borrow_mut();
        state.file_finder.poll();
        let updates = state.file_finder.take_workspace_updates();
        let mut content = self.file_content.borrow_mut();
        for update in &updates {
            if update.root != state.project_root {
                continue;
            }
            if update.indexed_files.is_some() {
                content.reset();
            } else {
                for change in &update.changes {
                    match change {
                        bed_remote::FilesystemChange::Renamed { from, to } => {
                            content.invalidate(from);
                            content.invalidate(to);
                        }
                        bed_remote::FilesystemChange::Created { path }
                        | bed_remote::FilesystemChange::Modified { path }
                        | bed_remote::FilesystemChange::Removed { path } => {
                            content.invalidate(path)
                        }
                    }
                }
            }
        }
        updates
    }
    pub fn finder_visible(&self) -> bool {
        self.file_explorer.borrow().file_finder.show_ff_window
    }
    pub fn toggle_finder(&self) {
        self.file_explorer.borrow_mut().file_finder.toggle_window();
    }
    pub fn reset_finder(&self) {
        let mut state = self.file_explorer.borrow_mut();
        state.file_finder.cancel_and_close();
        state.file_finder.set_query("");
    }
    pub fn refresh_finder(&self) {
        let mut state = self.file_explorer.borrow_mut();
        let root = state.project_root.clone();
        state.file_finder.set_project_dir(&root);
    }
    pub fn configure_presentation(
        &self,
        tree_style: FileTreeStyle,
        finder_style: FileFinderStyle,
        icons: &Icons,
        workspace_identity: String,
    ) {
        let mut config = self.config.borrow_mut();
        config.tree_style = tree_style;
        config.finder_style = finder_style;
        config.icons = IconTextures(icons.textures.clone());
        config.workspace_identity = workspace_identity;
    }
    /// Replace a transport only at workspace activation/reconnection. Replacing
    /// it cancels the old generation of discovery work.
    pub fn set_remote_client(&self, client: Option<RemoteClient>) {
        self.file_content.borrow_mut().reset();
        let mut state = self.file_explorer.borrow_mut();
        state.file_finder.set_project_dir("");
        state.file_finder.set_remote_client(client);
        let root = state.project_root.clone();
        state.file_finder.set_project_dir(&root);
    }
    pub fn apply_directory(&self, path: &str, entries: Vec<bed_remote::DirectoryEntry>) {
        self.file_explorer
            .borrow_mut()
            .file_tree
            .apply_directory(path, entries);
    }
    pub fn open_directories(&self) -> Vec<String> {
        self.file_explorer.borrow().file_tree.open_directories()
    }
    /// Expand loaded ancestors. The host's normal remote-directory service
    /// supplies any missing children on its next pass.
    pub fn reveal(&self, path: &str) {
        fn expand(node: &mut FileNode, path: &str) {
            if node.is_directory
                && (node.full_path == path
                    || path
                        .strip_prefix(node.full_path.trim_end_matches('/'))
                        .is_some_and(|tail| tail.starts_with('/')))
            {
                node.is_open = true;
                for child in &mut node.children {
                    expand(child, path);
                }
            }
        }
        expand(
            &mut self.file_explorer.borrow_mut().file_tree.root_node,
            path,
        );
        self.actions
            .borrow_mut()
            .push(ExplorerAction::RefreshRemote { force: false });
    }
    /// Adapt registry contributions into a Files-owned menu model. The host
    /// resolves enabled states against the captured path; drawing requires no
    /// reference to the workbench or module runtime.
    pub fn update_menu_model(
        &self,
        registry: &Registry,
        host: &HostContext<'_>,
        enabled: impl Fn(&str, &CommandContext) -> bool,
    ) {
        fn paths(node: &FileNode, output: &mut Vec<String>) {
            output.push(node.full_path.clone());
            for child in &node.children {
                paths(child, output);
            }
        }
        let mut loaded_paths = Vec::new();
        paths(
            &self.file_explorer.borrow().file_tree.root_node,
            &mut loaded_paths,
        );
        let contexts: Vec<_> = loaded_paths
            .iter()
            .map(|path| context_for_path(path, host))
            .collect();
        let commands = registry
            .menus
            .iter()
            .filter(|item| {
                matches!(
                    item.slot,
                    MenuSlot::File | MenuSlot::Folder | MenuSlot::TreeBackground
                )
            })
            .filter_map(|item| {
                let descriptor = registry.command(item.command)?;
                Some(MenuCommand {
                    id: descriptor.id.into(),
                    label: descriptor.label.into(),
                    slot: item.slot,
                    enabled: contexts
                        .iter()
                        .filter_map(|context| {
                            Some((context.path.clone()?, enabled(descriptor.id, context)))
                        })
                        .collect(),
                })
            })
            .collect();
        let mut viewers = Registry::default();
        viewers.viewers = registry
            .viewers
            .iter()
            .filter(|viewer| !host.remote || !viewer.is_local_file())
            .cloned()
            .collect();
        self.config.borrow_mut().registry_menus = Rc::new(RegistryMenus {
            commands,
            viewers: Rc::new(viewers),
        });
    }
    fn submit(
        &self,
        actions: Vec<FileTreeAction>,
        remote: bool,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        for action in actions {
            let mut state = self.file_explorer.borrow_mut();
            if state.file_tree.apply_visibility_action(&action, remote) {
                if !matches!(action, FileTreeAction::SetShowHidden(_)) {
                    self.actions
                        .borrow_mut()
                        .push(ExplorerAction::PreferencesChanged(
                            state.file_tree.preferences.clone(),
                        ));
                }
                let directories = state.file_tree.open_directories();
                state.file_finder.refresh_directories(directories);
                requests.push(HostRequest::Invalidate);
            } else {
                self.actions.borrow_mut().push(ExplorerAction::Tree(action));
            }
        }
        Ok(())
    }
}

fn context_for_path(path: &str, host: &HostContext<'_>) -> CommandContext {
    let document = host.documents.iter().find(|document| document.path == path);
    CommandContext {
        path: Some(path.into()),
        document: document.map(|document| document.id),
        revision: document.map(|document| document.revision),
        selection: None,
    }
}

pub struct ExplorerModule {
    handle: ExplorerHandle,
}
impl ExplorerModule {
    pub fn new(handle: ExplorerHandle) -> Self {
        Self { handle }
    }
    pub fn handle(&self) -> &ExplorerHandle {
        &self.handle
    }
}
impl Module for ExplorerModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.command(SHOW_COMMAND, "Files", None);
        registrar.command(NEW_COMMAND, "New Files Panel", None);
        registrar.command(FIND_COMMAND, "Find File", None);
        registrar.panel_options(
            PANEL_ID,
            "Files",
            false,
            PanelPlacement::Sidebar,
            Some("explorer"),
        );
        registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        match command {
            SHOW_COMMAND => requests.push(HostRequest::ShowPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
                action: None,
            }),
            NEW_COMMAND => requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            }),
            FIND_COMMAND => {
                self.handle.toggle_finder();
                requests.push(HostRequest::Invalidate);
            }
            _ => {}
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        _: Option<bed_document_session::DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown explorer panel: {panel_type}"));
        }
        Ok(Box::new(ExplorerPanel {
            handle: self.handle.clone(),
            drop_targets: Vec::new(),
            drop_root: String::new(),
            panel_bounds: None,
            external_destination: None,
            external_position: None,
        }))
    }
    fn tick_with_services(
        &mut self,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let mut state = self.handle.file_explorer.borrow_mut();
        {
            let mut content = self.handle.file_content.borrow_mut();
            content.configure(
                &self.handle.config.borrow().workspace_identity,
                &state.project_root,
                services.documents.is_remote(),
                services.documents.remote_client(),
            );
            if content.poll() {
                requests.push(HostRequest::Invalidate);
            }
        }
        let include_ignored = state.file_tree.preferences.include_ignored;
        state.file_finder.set_include_ignored(include_ignored);
        if state.file_finder.poll() {
            requests.push(HostRequest::Invalidate);
        }
        let directories = state.file_tree.open_directories();
        state.file_finder.request_directories(directories);
        let directory_updates = state.file_finder.take_directory_updates();
        if !directory_updates.is_empty() {
            for directory in directory_updates {
                state
                    .file_tree
                    .apply_directory(&directory.path, directory.entries);
                if let Some(warning) = directory.warning {
                    state.file_tree.error = Some(warning);
                }
            }
            requests.push(HostRequest::Invalidate);
        }
        if let Some(error) = state.file_tree.error.take() {
            requests.push(HostRequest::Notify { message: error });
        }
        Ok(())
    }
    fn draw_popups(&mut self, ui: &Ui, _: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let config = self.handle.config.borrow();
        let action = self
            .handle
            .file_explorer
            .borrow_mut()
            .file_finder
            .render_window(ui, &config.finder_style, Some(&config.icons));
        {
            let mut state = self.handle.file_explorer.borrow_mut();
            let include_ignored = state.file_finder.include_ignored();
            if state.file_tree.preferences.include_ignored != include_ignored {
                state.file_tree.preferences.include_ignored = include_ignored;
                self.handle
                    .actions
                    .borrow_mut()
                    .push(ExplorerAction::PreferencesChanged(
                        state.file_tree.preferences.clone(),
                    ));
            }
        }
        match action {
            FileFinderAction::Open(path) => {
                requests.push(HostRequest::OpenFile { path, viewer: None })
            }
            FileFinderAction::Close => requests.push(HostRequest::Invalidate),
            FileFinderAction::None => {}
        }
    }
    fn restore_workspace(&mut self, value: Option<&Value>, root: &str) {
        self.handle.file_content.borrow_mut().reset();
        let mut state = self.handle.file_explorer.borrow_mut();
        state.project_root = root.into();
        state.file_finder.set_directory_label(None);
        state.file_tree.preferences = value
            .map(FileTreePreferences::from_value)
            .unwrap_or_default();
        state.file_tree.show_hidden = false;
        state.file_tree.error = None;
        let include_ignored = state.file_tree.preferences.include_ignored;
        state.file_finder.set_include_ignored(include_ignored);
    }
    fn save_workspace(&self) -> Value {
        let state = self.handle.file_explorer.borrow();
        let mut value = state.file_tree.preferences.to_value();
        value["include_ignored"] = Value::Bool(state.file_finder.include_ignored());
        value
    }
    fn shutdown(&mut self, _: &mut ModuleServices<'_>) {
        // Replacing the finder drops its worker even if an embedding host keeps
        // a feature handle alive after module shutdown.
        let mut state = self.handle.file_explorer.borrow_mut();
        state.file_finder = file_finder::FileFinder::new();
        state.file_tree.file_info = Default::default();
        self.handle.file_content.borrow_mut().reset();
        self.handle.actions.borrow_mut().clear();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct ExplorerPanel {
    handle: ExplorerHandle,
    drop_targets: Vec<file_tree::FileTreeDropTarget>,
    drop_root: String,
    panel_bounds: Option<([f32; 2], [f32; 2])>,
    external_destination: Option<String>,
    external_position: Option<[f32; 2]>,
}
impl ExplorerPanel {
    pub fn handle(&self) -> &ExplorerHandle {
        &self.handle
    }
    pub fn drop_destination_at(&self, position: [f32; 2]) -> Option<String> {
        let contains = |min: [f32; 2], max: [f32; 2]| {
            position[0] >= min[0]
                && position[0] < max[0]
                && position[1] >= min[1]
                && position[1] < max[1]
        };
        let root = self.handle.file_explorer.borrow().project_root.clone();
        if root.is_empty() || self.drop_root != root {
            return None;
        }
        let (min, max) = self.panel_bounds?;
        if !contains(min, max) {
            return None;
        }
        if let Some(target) = self
            .drop_targets
            .iter()
            .find(|target| contains(target.min, target.max))
        {
            return Some(target.destination.clone());
        }
        Some(root)
    }
    pub fn set_external_drop_position(&mut self, position: Option<[f32; 2]>) {
        self.external_position = position;
        self.external_destination =
            position.and_then(|position| self.drop_destination_at(position));
    }
}
impl ModulePanel for ExplorerPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Files".into()
    }
    fn window_padding(&self) -> Option<[f32; 2]> {
        Some([2.0; 2])
    }
    fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {}
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let root = self.handle.file_explorer.borrow().project_root.clone();
        self.drop_root = root.clone();
        self.panel_bounds = Some(ui.with_bound_context(|| unsafe {
            let rect = (*dear_imgui_rs::sys::igGetCurrentWindow()).InnerClipRect;
            ([rect.Min.x, rect.Min.y], [rect.Max.x, rect.Max.y])
        }));
        if root.is_empty() {
            self.drop_targets.clear();
            ui.text_disabled("Open a project to browse files");
            if ui.button("Open Folder") {
                self.handle
                    .actions
                    .borrow_mut()
                    .push(ExplorerAction::OpenProjectDialog);
            }
            return Ok(());
        }
        if services.project_root.is_empty() {
            bed_ui::presentation::directory_label(ui, &root);
        }
        let config = self.handle.config.borrow().clone();
        let active_path = host
            .active()
            .map(|document| document.path.as_str())
            .unwrap_or_default();
        let remote = services.documents.is_remote();
        let git = services.active_view.and_then(|view| {
            services
                .documents
                .with_view(view, |editor| Rc::clone(&editor.git))
                .ok()
        });
        let documents = &*services.documents;
        let modified = |path: &str| {
            if remote {
                documents.remote_file_modified(path)
            } else {
                git.as_ref()
                    .is_some_and(|git| git.borrow().is_file_modified(path))
            }
        };
        let menus = |ui: &Ui,
                     path: &str,
                     directory: bool,
                     background: bool,
                     selected_paths: &[String],
                     actions: &mut Vec<FileTreeAction>| {
            let context = TreeMenuContext {
                command: context_for_path(path, host),
                selected_paths: selected_paths.to_vec(),
                directory,
                background,
            };
            config
                .registry_menus
                .draw(ui, &context, host, &self.handle.file_content, actions);
            config.extensions.draw(ui, &context, host, actions);
        };
        let actions = {
            let mut state = self.handle.file_explorer.borrow_mut();
            if let Some(status) = &state.file_finder.discovery_status {
                ui.text_disabled("File watching degraded — periodic refresh active");
                if ui.is_item_hovered() {
                    ui.tooltip_text(status);
                }
            }
            state.file_tree.file_info.configure(
                config.workspace_identity,
                &root,
                documents.remote_client(),
            );
            let mut style = config.tree_style;
            style.rainbow_time = ui.time() as f32;
            state.file_tree.external_drop_target = self.external_destination.clone();
            state.file_tree.external_drag_position = self.external_position;
            let actions = state.file_tree.display_backend_actions_with_menu(
                ui,
                active_path,
                &style,
                Some(&config.icons),
                Some(&modified),
                remote,
                Some(&menus),
            );
            self.drop_targets = state.file_tree.drop_targets(ui);
            state.file_tree.external_drop_target = None;
            state.file_tree.external_drag_position = None;
            actions
        };
        self.handle.submit(actions, remote, requests)
    }
    fn external_files_with_services(
        &mut self,
        event: &bed_workbench_api::ExternalFileDrag,
        _: &HostContext<'_>,
        _: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<bed_workbench_api::ExternalFileDropResponse> {
        use bed_workbench_api::{ExternalFileDragPhase, ExternalFileDropResponse};
        if event.phase == ExternalFileDragPhase::Cancel {
            self.set_external_drop_position(None);
            requests.push(HostRequest::Invalidate);
            return Ok(ExternalFileDropResponse::Accepted);
        }
        let Some(destination) = self.drop_destination_at(event.position) else {
            self.set_external_drop_position(None);
            return Ok(ExternalFileDropResponse::Ignored);
        };
        match event.phase {
            ExternalFileDragPhase::Hover => {
                self.external_destination = Some(destination);
                self.external_position = Some(event.position);
            }
            ExternalFileDragPhase::Drop => {
                self.external_destination = None;
                self.external_position = None;
                self.handle
                    .actions
                    .borrow_mut()
                    .push(ExplorerAction::Import {
                        paths: event.paths.clone(),
                        destination,
                    });
            }
            ExternalFileDragPhase::Cancel => unreachable!(),
        }
        requests.push(HostRequest::Invalidate);
        Ok(ExternalFileDropResponse::Accepted)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_drop_destinations_use_pointer_rectangles_and_reject_stale_workspaces() {
        let handle = ExplorerHandle::default();
        handle.borrow_mut().project_root = "/project".into();
        let mut panel = ExplorerPanel {
            handle: handle.clone(),
            drop_targets: vec![file_tree::FileTreeDropTarget {
                min: [0.0, 20.0],
                max: [100.0, 40.0],
                destination: "/project/src".into(),
            }],
            drop_root: "/project".into(),
            panel_bounds: Some(([10.0, 10.0], [200.0, 200.0])),
            external_destination: None,
            external_position: None,
        };
        assert_eq!(
            panel.drop_destination_at([50.0, 30.0]).as_deref(),
            Some("/project/src")
        );
        assert_eq!(
            panel.drop_destination_at([150.0, 150.0]).as_deref(),
            Some("/project")
        );
        assert_eq!(
            panel.drop_destination_at([5.0, 30.0]),
            None,
            "clipped rows cannot capture native drops outside their panel"
        );
        panel.set_external_drop_position(Some([50.0, 30.0]));
        assert_eq!(panel.external_destination.as_deref(), Some("/project/src"));
        assert_eq!(panel.external_position, Some([50.0, 30.0]));
        handle.borrow_mut().project_root = "/different".into();
        assert_eq!(
            panel.drop_destination_at([50.0, 30.0]),
            None,
            "an old panel rectangle cannot mutate the previous project"
        );
        handle.borrow_mut().project_root.clear();
        assert_eq!(panel.drop_destination_at([50.0, 30.0]), None);
        panel.set_external_drop_position(None);
        assert!(panel.external_destination.is_none());
    }
    #[test]
    fn files_panels_share_feature_state_and_preserve_independent_host_lifetimes() {
        let handle = ExplorerHandle::default();
        let mut module = ExplorerModule::new(handle.clone());
        let first = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        let second = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        handle.borrow_mut().file_tree.preferences.hide_hidden = true;
        drop(first);
        let second = second.as_any().downcast_ref::<ExplorerPanel>().unwrap();
        assert!(second.handle.borrow().file_tree.preferences.hide_hidden);
        assert!(module.save_workspace()["hide_hidden"].as_bool().unwrap());
    }
    #[test]
    fn reveal_expands_only_ancestors_without_changing_preferences() {
        let handle = ExplorerHandle::default();
        handle.borrow_mut().file_tree.root_node = FileNode {
            full_path: "/project".into(),
            is_directory: true,
            children: vec![
                FileNode {
                    full_path: "/project/src".into(),
                    is_directory: true,
                    ..Default::default()
                },
                FileNode {
                    full_path: "/project/vendor".into(),
                    is_directory: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        handle.reveal("/project/src/main.rs");
        let state = handle.borrow();
        assert!(state.file_tree.root_node.is_open);
        assert!(state.file_tree.root_node.children[0].is_open);
        assert!(!state.file_tree.root_node.children[1].is_open);
        assert_eq!(state.file_tree.preferences, FileTreePreferences::default());
    }

    #[test]
    fn remote_visibility_preserves_remote_entries_and_persists_only_preferences() {
        let directory = crate::test_support::TempDir::new();
        directory.write("local-only.txt", b"local content");
        let root = directory.root().to_str().unwrap().to_owned();
        let handle = ExplorerHandle::default();
        let remote_entry = FileNode {
            name: "remote-only.txt".into(),
            full_path: format!("{root}/remote-only.txt"),
            ..Default::default()
        };
        {
            let mut state = handle.borrow_mut();
            state.project_root = root.clone();
            state.file_tree.root_node = FileNode {
                full_path: root,
                is_directory: true,
                is_open: true,
                children: vec![remote_entry.clone()],
                ..Default::default()
            };
        }
        let mut requests = Vec::new();
        handle
            .submit(
                vec![
                    FileTreeAction::SetHideHidden(true),
                    FileTreeAction::SetShowHidden(true),
                ],
                true,
                &mut requests,
            )
            .unwrap();
        let state = handle.borrow();
        assert_eq!(state.file_tree.root_node.children, vec![remote_entry]);
        assert!(state.file_tree.preferences.hide_hidden);
        assert!(state.file_tree.show_hidden);
        let actions = handle.take_actions();
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, ExplorerAction::PreferencesChanged(_)))
                .count(),
            1,
            "Show Hidden is a transient reveal, not a workspace preference"
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, ExplorerAction::RefreshRemote { force: true }))
                .count(),
            0,
            "Directory refreshes use the shared filesystem service"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| matches!(request, HostRequest::Invalidate))
                .count(),
            2
        );
    }

    #[test]
    fn registry_menu_model_keeps_enabled_state_for_each_originating_path() {
        let handle = ExplorerHandle::default();
        handle.borrow_mut().file_tree.root_node = FileNode {
            full_path: "/project".into(),
            is_directory: true,
            children: vec![
                FileNode {
                    full_path: "/project/data.csv".into(),
                    ..Default::default()
                },
                FileNode {
                    full_path: "/project/notes.txt".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let csv = bed_plugin_csv::CsvPlugin;
        let mut registry = Registry::default();
        registry.register(&csv).unwrap();
        // Ordinary file contributions still keep their per-path enabled state;
        // the CSV plugin itself no longer contributes a duplicate open action.
        registry.menus.push(bed_workbench_api::MenuContribution {
            slot: MenuSlot::File,
            command: bed_plugin_csv::OPEN_FILE_COMMAND,
        });
        let textures = HashMap::new();
        let host = HostContext {
            remote: false,
            default_viewers: &Value::Null,
            viewer_menu: None,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 0,
            diagnostics: &Value::Null,
        };
        handle.update_menu_model(&registry, &host, |command, context| {
            csv.command_enabled(command, context, &host)
        });
        let config = handle.config.borrow();
        let command = config
            .registry_menus
            .commands
            .iter()
            .find(|command| command.id == bed_plugin_csv::OPEN_FILE_COMMAND)
            .unwrap();
        assert!(command.enabled["/project/data.csv"]);
        assert!(!command.enabled["/project/notes.txt"]);
        assert!(
            config
                .registry_menus
                .viewers
                .viewer(bed_plugin_csv::VIEWER_ID)
                .is_some()
        );
    }

    #[test]
    fn viewer_menu_uses_unsaved_content_without_probing_disk() {
        use bed_workbench_api::{DocumentKind, PluginDocument};
        let mut registry = Registry::default();
        registry.register(&bed_plugin_csv::CsvPlugin).unwrap();
        assert!(
            !registry
                .menus
                .iter()
                .any(|item| item.slot == MenuSlot::File)
        );
        let menus = RegistryMenus {
            commands: Vec::new(),
            viewers: Rc::new(registry),
        };
        let content = RefCell::new(FileContent::default());
        let textures = HashMap::new();
        let mut document = PluginDocument {
            id: bed_document_session::DocumentId::next(),
            path: "/project/table.csv".into(),
            kind: DocumentKind::Bytes,
            language_id: String::new(),
            revision: (1, 1),
            dirty: true,
            bytes: std::sync::Arc::from(&b"name,value\nfirst,1\n"[..]),
            text: None,
        };
        for (bytes, expected) in [
            (&b"name,value\nfirst,1\n"[..], DocumentKind::Text),
            (&b"\0\0\0\0"[..], DocumentKind::Bytes),
        ] {
            document.bytes = std::sync::Arc::from(bytes);
            let host = HostContext {
                remote: false,
                default_viewers: &Value::Null,
                viewer_menu: None,
                documents: std::slice::from_ref(&document),
                active_document: None,
                settings: &Value::Null,
                textures: &textures,
                animations: false,
                workspace: 0,
                diagnostics: &Value::Null,
            };
            let status = menus.content_status(&document.path, &host, &content);
            assert_eq!(status, ContentStatus::Ready(expected));
            assert_eq!(
                menus
                    .viewers
                    .compatible_viewers(&document.path, status.kind())
                    .iter()
                    .any(|viewer| viewer.id == bed_plugin_csv::VIEWER_ID),
                expected == DocumentKind::Text,
            );
        }
    }
}
