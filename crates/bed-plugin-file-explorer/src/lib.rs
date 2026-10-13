//! Files tree widgets, finder, and workspace discovery.
//! Filesystem mutations and workspace navigation are requested from the host.
mod file_content;
pub mod file_finder;
pub mod file_info;
pub mod file_tree;
pub mod files;
pub mod operations;

use self::{
    file_content::{ContentStatus, FileContent},
    file_finder::{FileFinderAction, FileFinderStyle},
    file_tree::{FileNode, FileTreeAction, FileTreePreferences, FileTreeStyle},
    files::FileExplorer,
};
use bed_api::{
    documents::CommandContext,
    features::{FeatureRegistry, HostAction, HostContext, MenuSlot},
    workbench::PanelPlacement,
};
#[cfg(test)]
use bed_remote::RemoteClient;
use bed_ui::presentation::FileIcons;
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, io};

pub const MODULE_ID: &str = "bed.explorer";
pub const PANEL_ID: &str = "bed.explorer.panel";
pub const SHOW_COMMAND: &str = "bed.explorer.show";
pub const NEW_COMMAND: &str = "bed.explorer.new";
pub const FIND_COMMAND: &str = "bed.explorer.find_file";

#[cfg(all(test, unix))]
pub(crate) fn test_remote_client() -> RemoteClient {
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bed-remote-agent");
    if helper.is_file() {
        RemoteClient::launch_local(helper).unwrap()
    } else {
        let mut command = std::process::Command::new(env!("CARGO"));
        command
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .args([
                "run",
                "--quiet",
                "--offline",
                "-p",
                "bed-remote-agent",
                "--",
                "--stdio",
            ]);
        RemoteClient::launch_command(command).unwrap()
    }
}

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
    viewers: FeatureRegistry,
}

impl RegistryMenus {
    fn content_status(
        &self,
        path: &str,
        host: &HostContext<'_>,
        content: &RefCell<&mut FileContent>,
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
        content: &RefCell<&mut FileContent>,
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
                        actions.push(FileTreeAction::OpenWith {
                            path: path.into(),
                            viewer: viewer.id.into(),
                        });
                    }
                }
                match &status {
                    ContentStatus::Loading => ui.text_disabled("Checking file type…"),
                    ContentStatus::Unavailable(error) => {
                        ui.text_disabled("File type unavailable");
                        if ui.is_item_hovered() {
                            bed_ui::components::tooltip_text(
                                ui,
                                &bed_ui::UiFrameContext::for_ui(ui),
                                error,
                            );
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
    pub workspace_identity: String,
    registry_menus: RegistryMenus,
}

/// Work requiring application services is drained after drawing, never while a
/// tree or its popup holds mutable widget state.
pub enum ExplorerAction {
    Tree(FileTreeAction),
    ChooseDirectory,
    Import {
        paths: Vec<std::path::PathBuf>,
        destination: String,
    },
    PreferencesChanged(FileTreePreferences),
    RefreshRemote {
        force: bool,
    },
}

pub struct ExplorerState {
    pub file_explorer: FileExplorer,
    pub config: ExplorerConfig,
    actions: Vec<ExplorerAction>,
    file_content: FileContent,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            file_explorer: FileExplorer::new(),
            config: ExplorerConfig::default(),
            actions: Vec::new(),
            file_content: FileContent::default(),
        }
    }
}

impl ExplorerState {
    pub fn explorer(&self) -> &FileExplorer {
        &self.file_explorer
    }

    pub fn explorer_mut(&mut self) -> &mut FileExplorer {
        &mut self.file_explorer
    }

    pub fn config(&mut self) -> &mut ExplorerConfig {
        &mut self.config
    }

    pub fn take_actions(&mut self) -> Vec<ExplorerAction> {
        std::mem::take(&mut self.actions)
    }

    /// Lifecycle consumers drain the same stream used by tree/finder discovery.
    pub fn take_filesystem_updates(&mut self) -> Vec<bed_remote::WorkspaceUpdate> {
        let state = &mut self.file_explorer;
        state.file_finder.poll();
        let updates = state.file_finder.take_workspace_updates();
        let content = &mut self.file_content;
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
        self.file_explorer.file_finder.show_ff_window
    }

    pub fn toggle_finder(&mut self) {
        self.file_explorer.file_finder.toggle_window();
    }

    pub fn reset_finder(&mut self) {
        let state = &mut self.file_explorer;
        state.file_finder.cancel_and_close();
        state.file_finder.set_query("");
    }

    pub fn refresh_finder(&mut self) {
        let state = &mut self.file_explorer;
        let root = state.project_root.clone();
        state.file_finder.set_project_dir(&root);
    }

    pub fn configure_presentation(
        &mut self,
        tree_style: FileTreeStyle,
        finder_style: FileFinderStyle,
        workspace_identity: String,
    ) {
        let config = &mut self.config;
        config.tree_style = tree_style;
        config.finder_style = finder_style;
        config.workspace_identity = workspace_identity;
    }

    /// Replace a transport only at workspace activation/reconnection. Replacing
    /// it cancels the old generation of discovery work.
    pub fn set_file_services(&mut self, services: bed_files::FileServices) {
        self.file_content.reset();
        let state = &mut self.file_explorer;
        state.file_finder.set_project_dir("");
        state.file_finder.set_file_services(services);
        let root = state.project_root.clone();
        state.file_finder.set_project_dir(&root);
    }

    pub fn apply_directory(&mut self, path: &str, entries: Vec<bed_remote::DirectoryEntry>) {
        self.file_explorer.file_tree.apply_directory(path, entries);
    }

    pub fn open_directories(&self) -> Vec<String> {
        self.file_explorer.file_tree.open_directories()
    }

    /// Expand loaded ancestors. The host's normal remote-directory service
    /// supplies any missing children on its next pass.
    pub fn reveal(&mut self, path: &str) {
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
        expand(&mut self.file_explorer.file_tree.root_node, path);
        self.actions
            .push(ExplorerAction::RefreshRemote { force: false });
    }

    /// Adapt registry contributions into a Files-owned menu model. The host
    /// resolves enabled states against the captured path; drawing requires no
    /// reference to the workbench or module runtime.
    pub fn update_menu_model(
        &mut self,
        registry: &FeatureRegistry,
        definitions: &bed_api::workbench::WorkbenchRegistry,
        facts: Option<&bed_api::workbench::CommandPresentations>,
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
        paths(&self.file_explorer.file_tree.root_node, &mut loaded_paths);
        let contexts: Vec<_> = loaded_paths
            .iter()
            .map(|path| context_for_path(path, host))
            .collect();
        let commands = registry
            .menus()
            .iter()
            .filter(|item| {
                matches!(
                    item.slot,
                    MenuSlot::File | MenuSlot::Folder | MenuSlot::TreeBackground
                )
            })
            .filter_map(|item| {
                let descriptor = definitions.command(item.command)?.presentation(facts);
                if !descriptor.visible {
                    return None;
                }
                Some(MenuCommand {
                    id: descriptor.id.into(),
                    label: descriptor.label.into(),
                    slot: item.slot,
                    enabled: contexts
                        .iter()
                        .filter_map(|context| {
                            Some((
                                context.path.clone()?,
                                descriptor.enabled && enabled(descriptor.id, context),
                            ))
                        })
                        .collect(),
                })
            })
            .collect();
        self.config.registry_menus = RegistryMenus {
            commands,
            viewers: registry.clone(),
        };
    }

    fn submit(
        &mut self,
        actions: Vec<FileTreeAction>,
        remote: bool,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        for action in actions {
            let state = &mut self.file_explorer;
            if state.file_tree.apply_visibility_action(&action, remote) {
                if !matches!(action, FileTreeAction::SetShowHidden(_)) {
                    self.actions.push(ExplorerAction::PreferencesChanged(
                        state.file_tree.preferences.clone(),
                    ));
                }
                let directories = state.file_tree.open_directories();
                state.file_finder.refresh_directories(directories);
                requests.push(HostAction::Invalidate);
            } else {
                self.actions.push(ExplorerAction::Tree(action));
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

pub struct ExplorerPanel {
    drop_targets: Vec<file_tree::FileTreeDropTarget>,
    drop_root: String,
    panel_bounds: Option<([f32; 2], [f32; 2])>,
    external_destination: Option<String>,
    external_position: Option<[f32; 2]>,
}

impl ExplorerPanel {
    pub fn drop_destination_at(&self, root: &str, position: [f32; 2]) -> Option<String> {
        let contains = |min: [f32; 2], max: [f32; 2]| {
            position[0] >= min[0]
                && position[0] < max[0]
                && position[1] >= min[1]
                && position[1] < max[1]
        };
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
        Some(root.to_owned())
    }

    pub fn set_external_drop_position(&mut self, root: &str, position: Option<[f32; 2]>) {
        self.external_position = position;
        self.external_destination =
            position.and_then(|position| self.drop_destination_at(root, position));
    }
}

impl ExplorerPanel {}

impl ExplorerState {
    pub fn tick(&mut self, target: &ExplorerTarget, requests: &mut Vec<HostAction>) {
        let state = &mut self.file_explorer;
        {
            let content = &mut self.file_content;
            content.configure(
                &self.config.workspace_identity,
                &state.project_root,
                target.services.is_remote(),
                target.services.clone(),
            );
            if content.poll() {
                requests.push(HostAction::Invalidate);
            }
        }
        let include_ignored = state.file_tree.preferences.include_ignored;
        state.file_finder.set_include_ignored(include_ignored);
        if state.file_finder.poll() {
            requests.push(HostAction::Invalidate);
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
            requests.push(HostAction::Invalidate);
        }
        if let Some(error) = state.file_tree.error.take() {
            requests.push(HostAction::Notify { message: error });
        }
    }

    pub fn draw_popups(
        &mut self,
        ui: &Ui,
        icons: Option<&dyn FileIcons>,
        requests: &mut Vec<HostAction>,
    ) {
        let config = &self.config;
        let action = self
            .file_explorer
            .file_finder
            .render_window(ui, &config.finder_style, icons);
        {
            let state = &mut self.file_explorer;
            let include_ignored = state.file_finder.include_ignored();
            if state.file_tree.preferences.include_ignored != include_ignored {
                state.file_tree.preferences.include_ignored = include_ignored;
                self.actions.push(ExplorerAction::PreferencesChanged(
                    state.file_tree.preferences.clone(),
                ));
            }
        }
        match action {
            FileFinderAction::Open(path) => {
                requests.push(HostAction::OpenFile { path, viewer: None })
            }
            FileFinderAction::Close => requests.push(HostAction::Invalidate),
            FileFinderAction::None => {}
        }
    }
}

impl Default for ExplorerPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ExplorerPanel {
    pub fn new() -> Self {
        Self {
            drop_targets: Vec::new(),
            drop_root: String::new(),
            panel_bounds: None,
            external_destination: None,
            external_position: None,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The Files frontend names its filesystem target independently of document presentation"
    )]
    pub fn draw_body(
        &mut self,
        controller: &mut ExplorerState,
        ui: &Ui,
        frame: &bed_api::features::FrontendSnapshot,
        target: &ExplorerTarget,
        documents: &mut bed_documents::DocumentSession,
        icons: Option<&dyn FileIcons>,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let mut host = frame.context();
        host.remote = target.services.is_remote();
        let document_provider = operations::provider_matches(&target.services, documents);
        if !document_provider {
            host.documents = &[];
            host.active_document = None;
        }
        let root = controller.file_explorer.project_root.clone();
        self.drop_root = root.clone();
        self.panel_bounds = Some(ui.with_bound_context(|| unsafe {
            let rect = (*dear_imgui_rs::sys::igGetCurrentWindow()).InnerClipRect;
            ([rect.Min.x, rect.Min.y], [rect.Max.x, rect.Max.y])
        }));
        if root.is_empty() {
            self.drop_targets.clear();
            ui.text_disabled("Open a folder to browse files");
            if ui.button("Open Folder") {
                controller.actions.push(ExplorerAction::ChooseDirectory);
            }
            return Ok(());
        }
        bed_ui::presentation::directory_label(ui, &root);
        let config = controller.config.clone();
        let active_path = host
            .active()
            .map(|document| document.path.as_str())
            .unwrap_or_default();
        let remote = target.services.is_remote();

        fn modified_paths(
            node: &FileNode,
            documents: &mut bed_documents::DocumentSession,
            remote: bool,
            paths: &mut std::collections::HashSet<String>,
        ) {
            let modified = if remote {
                documents.remote_file_modified(&node.full_path)
            } else {
                documents.git_file_modified(&node.full_path)
            };
            if modified {
                paths.insert(node.full_path.clone());
            }
            for child in &node.children {
                modified_paths(child, documents, remote, paths);
            }
        }
        let mut modified_files = std::collections::HashSet::new();
        if document_provider {
            modified_paths(
                &controller.file_explorer.file_tree.root_node,
                documents,
                remote,
                &mut modified_files,
            );
        }
        let modified = |path: &str| modified_files.contains(path);
        let content = RefCell::new(&mut controller.file_content);
        let menus = |ui: &Ui,
                     path: &str,
                     directory: bool,
                     background: bool,
                     selected_paths: &[String],
                     actions: &mut Vec<FileTreeAction>| {
            let context = TreeMenuContext {
                command: context_for_path(path, &host),
                selected_paths: selected_paths.to_vec(),
                directory,
                background,
            };
            config
                .registry_menus
                .draw(ui, &context, &host, &content, actions);
        };
        let actions = {
            let state = &mut controller.file_explorer;
            if let Some(status) = &state.file_finder.discovery_status {
                ui.text_disabled("File watching degraded — periodic refresh active");
                if ui.is_item_hovered() {
                    bed_ui::components::tooltip_text(
                        ui,
                        &bed_ui::UiFrameContext::for_ui(ui),
                        status,
                    );
                }
            }
            state.file_tree.file_info.configure(
                config.workspace_identity,
                &root,
                target.services.clone(),
            );
            let mut style = config.tree_style;
            style.rainbow_time = ui.time() as f32;
            state.file_tree.external_drop_target = self.external_destination.clone();
            state.file_tree.external_drag_position = self.external_position;
            let actions = state.file_tree.display_backend_actions_with_menu(
                ui,
                active_path,
                &style,
                icons,
                Some(&modified),
                remote,
                Some(&menus),
            );
            self.drop_targets = state.file_tree.drop_targets(ui);
            state.file_tree.external_drop_target = None;
            state.file_tree.external_drag_position = None;
            actions
        };
        controller.submit(actions, remote, requests)
    }
}

impl ExplorerPanel {
    pub fn external_files(
        &mut self,
        controller: &mut ExplorerState,
        event: &bed_api::workbench::ExternalFileDrag,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<bed_api::workbench::ExternalFileDropResponse> {
        use bed_api::workbench::{ExternalFileDragPhase, ExternalFileDropResponse};
        if event.phase == ExternalFileDragPhase::Cancel {
            self.set_external_drop_position(&controller.file_explorer.project_root, None);
            requests.push(HostAction::Invalidate);
            return Ok(ExternalFileDropResponse::Accepted);
        }
        let Some(destination) =
            self.drop_destination_at(&controller.file_explorer.project_root, event.position)
        else {
            self.set_external_drop_position(&controller.file_explorer.project_root, None);
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
                controller.actions.push(ExplorerAction::Import {
                    paths: event.paths.clone(),
                    destination,
                });
            }
            ExternalFileDragPhase::Cancel => unreachable!(),
        }
        requests.push(HostAction::Invalidate);
        Ok(ExternalFileDropResponse::Accepted)
    }
}

mod feature;
pub use feature::{
    ExplorerDefaults, ExplorerPanels, ExplorerPresentation, ExplorerTarget, FileExplorerPlugin,
    draw_popups,
};

#[cfg(test)]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

pub(crate) fn shortcut_actions() -> Vec<bed_keybindings::ShortcutAction> {
    vec![
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.move_first",
            "Move to First Item",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Home"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.move_last",
            "Move to Last Item",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["End"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.select_first",
            "Extend Selection to First",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+Home"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.select_last",
            "Extend Selection to Last",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Shift+End"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.open",
            "Open Selected Files",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Enter", "KeypadEnter"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.duplicate",
            "Duplicate Files",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &[],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.trash",
            "Move to Trash",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["Delete"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.rename",
            "Rename",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &["F2"],
        ),
        bed_keybindings::ShortcutAction::new(
            "bed.explorer.refresh",
            "Refresh",
            MODULE_ID,
            bed_keybindings::Scope::panel(PANEL_ID),
            &[],
        ),
    ]
}

pub(crate) fn default_shortcuts() -> bed_keybindings::ShortcutInput {
    bed_keybindings::ShortcutInput::new(bed_keybindings::Bindings::new(
        bed_keybindings::common_actions()
            .into_iter()
            .chain(shortcut_actions())
            .collect(),
        &Default::default(),
        bed_keybindings::Platform::current(),
    ))
    .with_scope(bed_keybindings::Scope::panel(PANEL_ID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_drop_destinations_use_pointer_rectangles_and_reject_stale_workspaces() {
        let mut handle = ExplorerState::default();
        handle.explorer_mut().project_root = "/project".into();
        let mut panel = ExplorerPanel {
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
            panel
                .drop_destination_at(&handle.file_explorer.project_root, [50.0, 30.0])
                .as_deref(),
            Some("/project/src")
        );
        assert_eq!(
            panel
                .drop_destination_at(&handle.file_explorer.project_root, [150.0, 150.0])
                .as_deref(),
            Some("/project")
        );
        assert_eq!(
            panel.drop_destination_at(&handle.file_explorer.project_root, [5.0, 30.0]),
            None,
            "clipped rows cannot capture native drops outside their panel"
        );
        panel.set_external_drop_position(&handle.file_explorer.project_root, Some([50.0, 30.0]));
        assert_eq!(panel.external_destination.as_deref(), Some("/project/src"));
        assert_eq!(panel.external_position, Some([50.0, 30.0]));
        handle.explorer_mut().project_root = "/different".into();
        assert_eq!(
            panel.drop_destination_at(&handle.file_explorer.project_root, [50.0, 30.0]),
            None,
            "an old panel rectangle cannot mutate the previous project"
        );
        handle.explorer_mut().project_root.clear();
        assert_eq!(
            panel.drop_destination_at(&handle.file_explorer.project_root, [50.0, 30.0]),
            None
        );
        panel.set_external_drop_position(&handle.file_explorer.project_root, None);
        assert!(panel.external_destination.is_none());
    }

    #[test]
    fn reveal_expands_only_ancestors_without_changing_preferences() {
        let mut handle = ExplorerState::default();
        handle.explorer_mut().file_tree.root_node = FileNode {
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
        let state = handle.explorer();
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
        let mut handle = ExplorerState::default();
        let remote_entry = FileNode {
            name: "remote-only.txt".into(),
            full_path: format!("{root}/remote-only.txt"),
            ..Default::default()
        };
        {
            let state = handle.explorer_mut();
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
        let state = handle.explorer();
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
                .filter(|request| matches!(request, HostAction::Invalidate))
                .count(),
            2
        );
    }

    #[test]
    fn registry_menu_model_keeps_enabled_state_for_each_originating_path() {
        let mut handle = ExplorerState::default();
        handle.explorer_mut().file_tree.root_node = FileNode {
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
        let mut registry = FeatureRegistry::default();
        registry
            .register_with("example", |registrar| {
                registrar.command_ref("example.inspect");
                registrar.menu(MenuSlot::File, "example.inspect");
                registrar.panel_ref("example.csv");
                registrar.viewer(
                    "example.csv-viewer",
                    "CSV",
                    "example.csv",
                    &["csv"],
                    bed_api::documents::DocumentKind::Text,
                );
            })
            .unwrap();
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
            session_id: 0,
            diagnostics: &Value::Null,
        };
        let mut world = bevy::prelude::World::new();
        bed_api::workbench::register(&mut world, "example", |registrar| {
            registrar.command(
                "example.inspect",
                "Inspect",
                |_: bevy::prelude::In<bed_api::workbench::CommandInvocation>| Ok(Vec::new()),
            );
        })
        .unwrap();
        handle.update_menu_model(
            &registry,
            world.resource::<bed_api::workbench::WorkbenchRegistry>(),
            None,
            &host,
            |command, context| {
                command == "example.inspect"
                    && context
                        .path
                        .as_deref()
                        .is_some_and(|path| path.ends_with(".csv"))
            },
        );
        let config = &handle.config;
        let command = config
            .registry_menus
            .commands
            .iter()
            .find(|command| command.id == "example.inspect")
            .unwrap();
        assert!(command.enabled["/project/data.csv"]);
        assert!(!command.enabled["/project/notes.txt"]);
        assert!(
            config
                .registry_menus
                .viewers
                .viewer("example.csv-viewer")
                .is_some()
        );
    }

    #[test]
    fn viewer_menu_uses_unsaved_content_without_probing_disk() {
        use bed_api::documents::{DocumentKind, PluginDocument};
        let mut registry = FeatureRegistry::default();
        registry
            .register_with("example", |registrar| {
                registrar.panel_ref("example.csv");
                registrar.viewer(
                    "example.csv-viewer",
                    "CSV",
                    "example.csv",
                    &["csv"],
                    DocumentKind::Text,
                );
            })
            .unwrap();
        assert!(
            !registry
                .menus()
                .iter()
                .any(|item| item.slot == MenuSlot::File)
        );
        let menus = RegistryMenus {
            commands: Vec::new(),
            viewers: registry,
        };
        let mut file_content = FileContent::default();
        let content = RefCell::new(&mut file_content);
        let textures = HashMap::new();
        let mut document = PluginDocument {
            id: bed_documents::DocumentId::next(),
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
                session_id: 0,
                diagnostics: &Value::Null,
            };
            let status = menus.content_status(&document.path, &host, &content);
            assert_eq!(status, ContentStatus::Ready(expected));
            assert_eq!(
                menus
                    .viewers
                    .compatible_viewers(&document.path, status.kind())
                    .iter()
                    .any(|viewer| viewer.id == "example.csv-viewer"),
                expected == DocumentKind::Text,
            );
        }
    }
}
