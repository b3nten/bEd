//! Files feature: tree panels, finder popup and concrete tree extension points.
//! Filesystem mutations and workspace navigation are requested from the host.
pub mod file_finder;
pub mod file_info;
pub mod file_tree;
pub mod files;

use self::{
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
    time::{Duration, Instant},
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
    viewers: Vec<(String, String)>,
}
impl RegistryMenus {
    fn draw(&self, ui: &Ui, context: &TreeMenuContext, actions: &mut Vec<FileTreeAction>) {
        let path = context.command.path.as_deref().unwrap_or_default();
        let items: Vec<_> = self
            .commands
            .iter()
            .filter(|item| item.slot == context.slot())
            .collect();
        if !items.is_empty() || !context.directory {
            ui.separator();
        }
        for item in items {
            let _id = ui.push_id(&item.id);
            if ui.menu_item_enabled_selected_no_shortcut(
                &item.label,
                false,
                item.enabled.get(path).copied().unwrap_or(false),
            ) {
                actions.push(FileTreeAction::Command {
                    command: item.id.clone(),
                    context: context.command.clone(),
                });
            }
        }
        if !context.directory
            && let Some(_menu) = ui.begin_menu("Open With")
        {
            for (id, label) in &self.viewers {
                let _id = ui.push_id(id);
                if ui.menu_item(label) {
                    actions.push(FileTreeAction::Command {
                        command: format!("bed.open_with:{id}"),
                        context: context.command.clone(),
                    });
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
    PreferencesChanged(FileTreePreferences),
    RefreshRemote { force: bool },
}

#[derive(Clone)]
pub struct ExplorerHandle {
    pub file_explorer: Rc<RefCell<FileExplorer>>,
    pub config: Rc<RefCell<ExplorerConfig>>,
    actions: Rc<RefCell<Vec<ExplorerAction>>>,
}
impl Default for ExplorerHandle {
    fn default() -> Self {
        Self {
            file_explorer: Rc::new(RefCell::new(FileExplorer::new())),
            config: Rc::new(RefCell::new(ExplorerConfig::default())),
            actions: Rc::default(),
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
        self.config.borrow_mut().registry_menus = Rc::new(RegistryMenus {
            commands,
            viewers: registry
                .viewers
                .iter()
                .map(|viewer| (viewer.id.into(), viewer.label.into()))
                .collect(),
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
                if remote {
                    self.actions
                        .borrow_mut()
                        .push(ExplorerAction::RefreshRemote { force: true });
                } else {
                    let root = state.project_root.clone();
                    state.file_tree.refresh_file_tree(&root)?;
                }
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
    last_refresh: Instant,
}
impl ExplorerModule {
    pub fn new(handle: ExplorerHandle) -> Self {
        Self {
            handle,
            last_refresh: Instant::now(),
        }
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
        }))
    }
    fn tick_with_services(
        &mut self,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let mut state = self.handle.file_explorer.borrow_mut();
        if state.file_finder.poll() {
            requests.push(HostRequest::Invalidate);
        }
        if !state.project_root.is_empty() && self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.last_refresh = Instant::now();
            if !services.documents.is_remote() {
                let root = state.project_root.clone();
                state.file_tree.refresh_file_tree(&root)?;
            } else {
                self.handle
                    .actions
                    .borrow_mut()
                    .push(ExplorerAction::RefreshRemote { force: true });
            }
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
        match action {
            FileFinderAction::Open(path) => {
                requests.push(HostRequest::OpenFile { path, viewer: None })
            }
            FileFinderAction::Close => requests.push(HostRequest::Invalidate),
            FileFinderAction::None => {}
        }
    }
    fn restore_workspace(&mut self, value: Option<&Value>, root: &str) {
        let mut state = self.handle.file_explorer.borrow_mut();
        state.project_root = root.into();
        state.file_tree.preferences = value
            .map(FileTreePreferences::from_value)
            .unwrap_or_default();
        state.file_tree.show_hidden = false;
        state.file_tree.error = None;
    }
    fn save_workspace(&self) -> Value {
        self.handle
            .file_explorer
            .borrow()
            .file_tree
            .preferences
            .to_value()
    }
    fn shutdown(&mut self, _: &mut ModuleServices<'_>) {
        // Replacing the finder drops its worker even if an embedding host keeps
        // a feature handle alive after module shutdown.
        let mut state = self.handle.file_explorer.borrow_mut();
        state.file_finder = file_finder::FileFinder::new();
        state.file_tree.file_info = Default::default();
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
}
impl ExplorerPanel {
    pub fn handle(&self) -> &ExplorerHandle {
        &self.handle
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
        if root.is_empty() {
            ui.text_disabled("Open a project to browse files");
            if ui.button("Open Folder") {
                self.handle
                    .actions
                    .borrow_mut()
                    .push(ExplorerAction::OpenProjectDialog);
            }
            return Ok(());
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
                     actions: &mut Vec<FileTreeAction>| {
            let context = TreeMenuContext {
                command: context_for_path(path, host),
                directory,
                background,
            };
            config.registry_menus.draw(ui, &context, actions);
            config.extensions.draw(ui, &context, host, actions);
        };
        let actions = {
            let mut state = self.handle.file_explorer.borrow_mut();
            state.file_tree.file_info.configure(
                config.workspace_identity,
                &root,
                documents.remote_client(),
            );
            let mut style = config.tree_style;
            style.rainbow_time = ui.time() as f32;
            state.file_tree.display_backend_actions_with_menu(
                ui,
                active_path,
                &style,
                Some(&config.icons),
                Some(&modified),
                remote,
                Some(&menus),
            )
        };
        self.handle.submit(actions, remote, requests)
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
            2
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
        let textures = HashMap::new();
        let host = HostContext {
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
                .iter()
                .any(|(id, _)| id == bed_plugin_csv::VIEWER_ID)
        );
    }
}
