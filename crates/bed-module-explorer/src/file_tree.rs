//! Translated from ned files/file_tree.{h,cpp}; see LICENSE and NOTICE.
use bed_files::tree_ignore::TreeIgnore;
use bed_ui::util::tree_animation::TreeAnimation;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::{collections::BTreeSet, fs, io, path::Path};

/// Personal, project-scoped tree preferences. Paths use project-relative `/` components.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTreePreferences {
    /// Finder scope is independent of tree visibility filters.
    pub include_ignored: bool,
    pub hide_gitignored: bool,
    pub hide_hidden: bool,
    pub hidden_paths: BTreeSet<String>,
}
impl FileTreePreferences {
    pub fn from_value(value: &Value) -> Self {
        Self {
            include_ignored: value["include_ignored"].as_bool().unwrap_or(false),
            hide_gitignored: value["hide_gitignored"].as_bool().unwrap_or(false),
            hide_hidden: value["hide_hidden"].as_bool().unwrap_or(false),
            hidden_paths: value["hidden_paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|path| valid_relative_path(path))
                .map(str::to_owned)
                .collect(),
        }
    }
    pub fn to_value(&self) -> Value {
        json!({"include_ignored":self.include_ignored,"hide_gitignored":self.hide_gitignored,"hide_hidden":self.hide_hidden,
            "hidden_paths":self.hidden_paths})
    }
    fn manually_hidden(&self, path: &str) -> bool {
        self.hidden_paths.iter().any(|hidden| {
            path == hidden
                || path
                    .strip_prefix(hidden)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }
}
fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn relative_path(root: &str, path: &str, remote: bool) -> Option<String> {
    let relative = if remote {
        path.strip_prefix(root.trim_end_matches('/'))?
            .strip_prefix('/')?
            .to_owned()
    } else {
        Path::new(path)
            .strip_prefix(root)
            .ok()?
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()?
            .join("/")
    };
    valid_relative_path(&relative).then_some(relative)
}
struct Visibility<'a> {
    root: &'a str,
    preferences: &'a FileTreePreferences,
    show_hidden: bool,
    remote: bool,
}
impl Visibility<'_> {
    fn hidden(&self, node: &FileNode) -> bool {
        let Some(relative) = relative_path(self.root, &node.full_path, self.remote) else {
            return false;
        };
        self.preferences.manually_hidden(&relative)
            || (self.preferences.hide_hidden
                && relative.split('/').any(|part| part.starts_with('.')))
            || (self.preferences.hide_gitignored && node.is_gitignored)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileNode {
    pub name: String,
    pub full_path: String,
    pub is_directory: bool,
    pub is_open: bool,
    pub is_gitignored: bool,
    pub children: Vec<FileNode>,
}
#[derive(Default)]
pub struct FileTree {
    pub root_node: FileNode,
    pub preferences: FileTreePreferences,
    pub show_hidden: bool,
    pub error: Option<String>,
    #[doc(hidden)]
    pub animations: FileTreeAnimations,
    #[doc(hidden)]
    pub file_info: super::file_info::FileInfoHover,
    /// The current panel sets this while drawing an accepted native file drag.
    pub external_drop_target: Option<String>,
    pub external_drag_position: Option<[f32; 2]>,
}
#[derive(Default)]
pub struct FileTreeAnimations {
    hosts: HashMap<u32, TreeHost>,
    drag: Option<TreeDrag>,
}
#[derive(Default)]
struct TreeHost {
    motion: TreeAnimation<String>,
    rows: HashMap<String, TreeRow>,
    root: String,
    preferences: FileTreePreferences,
    show_hidden: bool,
    last_frame: usize,
    interaction: FileTreeSelection,
    popup_selection: Vec<String>,
    drop_targets: Vec<FileTreeDropTarget>,
    hover_expand: Option<(String, f64)>,
    #[cfg(test)]
    labels: HashMap<String, [f32; 2]>,
}
/// Selection belongs to a Files panel, independently of the active document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTreeSelection {
    pub selected: BTreeSet<String>,
    pub focused: Option<String>,
    pub anchor: Option<String>,
}
impl FileTreeSelection {
    pub fn select(&mut self, path: &str, visible: &[String], toggle: bool, range: bool) {
        if range {
            let anchor = self.anchor.as_ref().or(self.focused.as_ref());
            let bounds = anchor.and_then(|anchor| {
                Some((
                    visible.iter().position(|p| p == anchor)?,
                    visible.iter().position(|p| p == path)?,
                ))
            });
            if let Some((start, end)) = bounds {
                if !toggle {
                    self.selected.clear();
                }
                self.selected
                    .extend(visible[start.min(end)..=start.max(end)].iter().cloned());
            } else {
                self.selected.clear();
                self.selected.insert(path.to_owned());
                self.anchor = Some(path.to_owned());
            }
        } else if toggle {
            if !self.selected.remove(path) {
                self.selected.insert(path.to_owned());
            }
            self.anchor = Some(path.to_owned());
        } else {
            self.selected.clear();
            self.selected.insert(path.to_owned());
            self.anchor = Some(path.to_owned());
        }
        self.focused = Some(path.to_owned());
    }
    pub fn context_selection(&mut self, path: &str) -> Vec<String> {
        if !self.selected.contains(path) {
            self.selected.clear();
            self.selected.insert(path.to_owned());
            self.anchor = Some(path.to_owned());
        }
        self.focused = Some(path.to_owned());
        self.selected.iter().cloned().collect()
    }
}
/// Screen-space drop rectangles are retained for native drags, whose pointer
/// updates do not necessarily pass through ImGui's mouse event queue.
#[derive(Clone, Debug)]
pub struct FileTreeDropTarget {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub destination: String,
}
#[derive(Clone, Debug)]
struct TreeDrag {
    root: String,
    paths: Vec<String>,
}
/// Remove the workspace root and descendants already represented by an ancestor.
pub fn mutation_paths(paths: &[String], root: &str) -> Vec<String> {
    let unique: BTreeSet<_> = paths
        .iter()
        .filter(|path| path.as_str() != root)
        .cloned()
        .collect();
    unique
        .iter()
        .filter(|path| {
            let mut parent = parent_directory(path);
            while !parent.is_empty() && parent != "/" {
                if unique.contains(&parent) {
                    return false;
                }
                parent = parent_directory(&parent);
            }
            true
        })
        .cloned()
        .collect()
}
pub fn valid_move_destination(paths: &[String], root: &str, destination: &str) -> bool {
    !paths.is_empty()
        && (destination == root || relative_path(root, destination, true).is_some())
        && paths.iter().all(|source| {
            source != root
                && destination != source
                && !destination
                    .strip_prefix(source.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
        })
}
fn parent_directory(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("")
        .to_owned()
}
#[derive(Clone)]
struct TreeRow {
    path: Vec<usize>,
    depth: i32,
    hidden: bool,
}
impl FileTree {
    /// Update a directory from its target without probing the local filesystem.
    pub fn apply_directory(&mut self, path: &str, entries: Vec<bed_remote::DirectoryEntry>) {
        fn find<'a>(node: &'a mut FileNode, path: &str) -> Option<&'a mut FileNode> {
            if node.full_path == path {
                return Some(node);
            }
            node.children.iter_mut().find_map(|child| find(child, path))
        }
        let Some(node) = find(&mut self.root_node, path) else {
            return;
        };
        let mut children = Vec::new();
        for entry in entries {
            let mut child = FileNode {
                name: entry.name,
                full_path: entry.path,
                is_directory: entry.is_directory,
                is_gitignored: entry.is_gitignored,
                ..FileNode::default()
            };
            if let Some(old) = node.children.iter_mut().find(|old| {
                old.full_path == child.full_path && old.is_directory == child.is_directory
            }) {
                child.is_open = old.is_open;
                child.children = std::mem::take(&mut old.children);
            }
            children.push(child);
        }
        children.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.cmp(&b.name))
        });
        node.children = children;
    }
    pub fn open_directories(&self) -> Vec<String> {
        fn collect(node: &FileNode, visibility: &Visibility<'_>, paths: &mut Vec<String>) {
            if !visibility.show_hidden && visibility.hidden(node) {
                return;
            }
            if node.is_directory && node.is_open {
                paths.push(node.full_path.clone());
                for child in &node.children {
                    collect(child, visibility, paths);
                }
            }
        }
        let mut paths = Vec::new();
        let visibility = Visibility {
            root: &self.root_node.full_path,
            preferences: &self.preferences,
            show_hidden: self.show_hidden,
            remote: true,
        };
        collect(&self.root_node, &visibility, &mut paths);
        paths
    }
    pub fn is_directory(&self, path: &str) -> Option<bool> {
        fn find(node: &FileNode, path: &str) -> Option<bool> {
            if node.full_path == path {
                return Some(node.is_directory);
            }
            node.children.iter().find_map(|child| find(child, path))
        }
        find(&self.root_node, path)
    }
    /// The active Files drag carries these paths independently of its ImGui payload.
    pub fn dragged_paths(&self) -> Option<&[String]> {
        self.animations
            .drag
            .as_ref()
            .map(|drag| drag.paths.as_slice())
    }
    pub fn build_file_tree(path: &Path, node: &mut FileNode) -> io::Result<()> {
        let root = path.to_str().unwrap_or_default();
        let preferences = FileTreePreferences::default();
        let visibility = Visibility {
            root,
            preferences: &preferences,
            show_hidden: false,
            remote: false,
        };
        Self::build_filtered(path, node, &visibility, &mut None)
    }
    fn build_filtered(
        path: &Path,
        node: &mut FileNode,
        visibility: &Visibility<'_>,
        reported_error: &mut Option<String>,
    ) -> io::Result<()> {
        let discovery = if visibility.preferences.hide_gitignored {
            TreeIgnore::discover(Path::new(visibility.root))
        } else {
            Ok(None)
        };
        let (ignore, mut warning) = match discovery {
            Ok(ignore) => (ignore, None),
            Err(error) => (None, Some(error)),
        };
        let result = Self::build_directory(path, node, visibility, ignore.as_ref(), &mut warning);
        if let Some(error) = warning {
            *reported_error = Some(format!("File tree: {error}"));
        }
        result
    }
    fn build_directory(
        path: &Path,
        node: &mut FileNode,
        visibility: &Visibility<'_>,
        ignore: Option<&TreeIgnore>,
        warning: &mut Option<io::Error>,
    ) -> io::Result<()> {
        if !node.is_open && !node.children.is_empty() {
            return Ok(());
        }
        let mut children = Vec::new();
        let result = (|| {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "File name is not UTF-8")
                })?;
                let child_path = entry.path();
                let full_path = child_path
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "File path is not UTF-8")
                    })?
                    .to_owned();
                let mut child = FileNode {
                    name,
                    full_path,
                    is_directory: fs::metadata(&child_path)?.is_dir(),
                    ..FileNode::default()
                };
                if let Some(old) = node
                    .children
                    .iter_mut()
                    .find(|old| old.full_path == child.full_path)
                {
                    child.is_open = old.is_open;
                    child.children = std::mem::take(&mut old.children);
                }
                children.push(child);
            }
            if let Some(ignore) = ignore {
                let paths: Vec<_> = children
                    .iter()
                    .map(|child| Path::new(&child.full_path).to_owned())
                    .collect();
                match ignore.classify(&paths) {
                    Ok(ignored) => {
                        for (child, ignored) in children.iter_mut().zip(ignored) {
                            child.is_gitignored = ignored;
                        }
                    }
                    Err(error) => *warning = Some(error),
                }
            }
            for child in &mut children {
                let child_path = Path::new(&child.full_path).to_owned();
                if child.is_directory
                    && child.is_open
                    && (visibility.show_hidden || !visibility.hidden(child))
                    && let Err(error) =
                        Self::build_directory(&child_path, child, visibility, ignore, warning)
                {
                    *warning = Some(error);
                }
            }
            Ok(())
        })();
        children.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.cmp(&b.name))
        });
        node.children = children;
        result
    }
    pub fn refresh_file_tree(&mut self, folder: &str) -> io::Result<()> {
        if folder.is_empty() {
            return Ok(());
        }
        let path = Path::new(folder);
        self.root_node.name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        self.root_node.full_path = folder.to_owned();
        self.root_node.is_directory = true;
        self.root_node.is_open = true;
        let visibility = Visibility {
            root: folder,
            preferences: &self.preferences,
            show_hidden: self.show_hidden,
            remote: false,
        };
        Self::build_filtered(path, &mut self.root_node, &visibility, &mut self.error)
    }
}

use dear_imgui_rs::{StyleColor, StyleVar, Ui};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeAction {
    Command {
        command: String,
        context: bed_workbench_api::CommandContext,
    },
    Open(String),
    NewFile(String),
    NewFolder(String),
    Rename(String),
    Trash(String),
    OpenMany(Vec<String>),
    OpenManyFromMenu(Vec<String>),
    TrashMany(Vec<String>),
    Move {
        paths: Vec<String>,
        destination: String,
    },
    Copy(Vec<String>),
    Cut(Vec<String>),
    Paste(String),
    Duplicate(Vec<String>),
    Refresh,
    SetHideGitignored(bool),
    SetHideHidden(bool),
    SetShowHidden(bool),
    SetPathHidden {
        path: String,
        hidden: bool,
    },
}
/// Host-owned extension menu. The path belongs to the popup's source row.
pub type ExtensionMenu<'a> =
    dyn Fn(&Ui, &str, bool, bool, &[String], &mut Vec<FileTreeAction>) + 'a;
/// The host provides uploaded upstream icon textures; unavailable icons reserve their space.
use bed_ui::presentation::FileIcons;
#[derive(Clone, Copy, Debug)]
pub struct FileTreeStyle {
    pub text_color: [f32; 4],
    pub rainbow: bool,
    pub rainbow_time: f32,
    pub animations: bool,
}
impl Default for FileTreeStyle {
    fn default() -> Self {
        Self {
            text_color: [1.0; 4],
            rainbow: true,
            rainbow_time: 0.0,
            animations: true,
        }
    }
}
impl FileTree {
    fn visibility_menu(ui: &Ui, visibility: &Visibility<'_>, actions: &mut Vec<FileTreeAction>) {
        for (label, checked, action) in [
            (
                "Hide Gitignored Files",
                visibility.preferences.hide_gitignored,
                FileTreeAction::SetHideGitignored(!visibility.preferences.hide_gitignored),
            ),
            (
                "Hide Hidden Files",
                visibility.preferences.hide_hidden,
                FileTreeAction::SetHideHidden(!visibility.preferences.hide_hidden),
            ),
            (
                "Show Hidden Files",
                visibility.show_hidden,
                FileTreeAction::SetShowHidden(!visibility.show_hidden),
            ),
        ] {
            if ui.menu_item_enabled_selected_no_shortcut(label, checked, true) {
                actions.push(action);
            }
        }
    }
    /// Returns whether this is a visibility action; callers explicitly persist preferences.
    pub fn apply_visibility_action(&mut self, action: &FileTreeAction, remote: bool) -> bool {
        match action {
            FileTreeAction::SetHideGitignored(value) => self.preferences.hide_gitignored = *value,
            FileTreeAction::SetHideHidden(value) => self.preferences.hide_hidden = *value,
            FileTreeAction::SetShowHidden(value) => self.show_hidden = *value,
            FileTreeAction::SetPathHidden { path, hidden } => {
                if let Some(relative) = relative_path(&self.root_node.full_path, path, remote) {
                    if *hidden {
                        self.preferences.hidden_paths.insert(relative);
                    } else {
                        self.preferences.hidden_paths.remove(&relative);
                    }
                }
            }
            _ => return false,
        }
        true
    }
    pub fn display_file_tree(
        &mut self,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
    ) -> Option<String> {
        let actions = self.draw_rows(ui, current_path, style, icons, modified, false, false, None);
        actions.into_iter().find_map(|action| match action {
            FileTreeAction::Open(path) => Some(path),
            _ => None,
        })
    }
    pub fn display_actions(
        &mut self,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
    ) -> Vec<FileTreeAction> {
        self.display_backend_actions(ui, current_path, style, icons, modified, false)
    }
    pub fn display_backend_actions(
        &mut self,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
        remote: bool,
    ) -> Vec<FileTreeAction> {
        self.display_backend_actions_with_menu(
            ui,
            current_path,
            style,
            icons,
            modified,
            remote,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn display_backend_actions_with_menu(
        &mut self,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
        remote: bool,
        extensions: Option<&ExtensionMenu<'_>>,
    ) -> Vec<FileTreeAction> {
        let mut actions = self.draw_rows(
            ui,
            current_path,
            style,
            icons,
            modified,
            true,
            remote,
            extensions,
        );
        let root = self.root_node.full_path.clone();
        let visibility = Visibility {
            root: &root,
            preferences: &self.preferences,
            show_hidden: self.show_hidden,
            remote,
        };
        // NoOpenOverItems keeps a row's menu separate from the background menu.
        // SAFETY: This call is inside the live Ui context and its matching End
        // runs synchronously without retaining an ImGui pointer.
        {
            let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
            unsafe {
                if dear_imgui_rs::sys::igBeginPopupContextWindow(
                    c"##tree_background".as_ptr(),
                    dear_imgui_rs::sys::ImGuiPopupFlags_MouseButtonRight
                        | dear_imgui_rs::sys::ImGuiPopupFlags_NoOpenOverItems,
                ) {
                    if ui.menu_item("New File…") {
                        actions.push(FileTreeAction::NewFile(self.root_node.full_path.clone()));
                    }
                    if ui.menu_item("New Folder…") {
                        actions.push(FileTreeAction::NewFolder(self.root_node.full_path.clone()));
                    }
                    if ui.menu_item("Paste") {
                        actions.push(FileTreeAction::Paste(root.clone()));
                    }
                    if ui.menu_item("Refresh") {
                        actions.push(FileTreeAction::Refresh);
                    }
                    ui.separator();
                    Self::visibility_menu(ui, &visibility, &mut actions);
                    if let Some(extensions) = extensions {
                        extensions(ui, &root, true, true, &[], &mut actions);
                    }
                    dear_imgui_rs::sys::igEndPopup();
                }
            }
        }
        actions
    }
    pub fn drop_targets(&self, ui: &Ui) -> Vec<FileTreeDropTarget> {
        let host_id =
            ui.with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetCurrentWindow()).ID });
        self.animations
            .hosts
            .get(&host_id)
            .map(|host| host.drop_targets.clone())
            .unwrap_or_default()
    }
    fn keyboard_actions(
        ui: &Ui,
        root: &mut FileNode,
        visible: &[String],
        selection: &mut FileTreeSelection,
        actions: &mut Vec<FileTreeAction>,
    ) {
        use dear_imgui_rs::Key;
        if !ui.is_window_focused() || ui.io().want_text_input() {
            return;
        }
        fn find<'a>(node: &'a mut FileNode, path: &str) -> Option<&'a mut FileNode> {
            if node.full_path == path {
                return Some(node);
            }
            node.children.iter_mut().find_map(|node| find(node, path))
        }
        let primary = ui.io().key_ctrl() || ui.io().key_super();
        let focused = selection
            .focused
            .as_ref()
            .and_then(|path| visible.iter().position(|p| p == path));
        let next = if ui.is_key_pressed(Key::DownArrow) {
            Some(focused.map_or(0, |i| (i + 1).min(visible.len().saturating_sub(1))))
        } else if ui.is_key_pressed(Key::UpArrow) {
            Some(focused.unwrap_or(0).saturating_sub(1))
        } else if ui.is_key_pressed(Key::Home) {
            Some(0)
        } else if ui.is_key_pressed(Key::End) {
            Some(visible.len().saturating_sub(1))
        } else {
            None
        };
        if let Some(path) = next.and_then(|index| visible.get(index)) {
            if primary && !ui.io().key_shift() {
                selection.focused = Some(path.clone());
            } else {
                selection.select(path, visible, primary, ui.io().key_shift());
            }
        }
        if primary && ui.is_key_pressed_with_repeat(Key::A, false) {
            selection.selected = visible
                .iter()
                .filter(|path| *path != &root.full_path)
                .cloned()
                .collect();
        }
        if let Some(path) = selection.focused.clone() {
            let parent = parent_directory(&path);
            if let Some(node) = find(root, &path) {
                if ui.is_key_pressed(Key::RightArrow) && node.is_directory {
                    if !node.is_open {
                        node.is_open = true;
                    } else if let Some(child) = node.children.first() {
                        selection.select(&child.full_path, visible, false, false);
                    }
                } else if ui.is_key_pressed(Key::LeftArrow) {
                    if node.is_directory && node.is_open {
                        node.is_open = false;
                    } else if visible.contains(&parent) {
                        selection.select(&parent, visible, false, false);
                    }
                }
            }
        }
        let selected: Vec<_> = selection.selected.iter().cloned().collect();
        let paths = mutation_paths(&selected, &root.full_path);
        if ui.is_key_pressed_with_repeat(Key::Enter, false)
            || ui.is_key_pressed_with_repeat(Key::KeypadEnter, false)
        {
            let files = selected
                .iter()
                .filter(|path| find(root, path).is_some_and(|node| !node.is_directory))
                .cloned()
                .collect::<Vec<_>>();
            if !files.is_empty() {
                actions.push(FileTreeAction::OpenMany(files));
            }
        }
        if !paths.is_empty() {
            if primary && ui.is_key_pressed_with_repeat(Key::C, false) {
                actions.push(FileTreeAction::Copy(paths.clone()));
            }
            if primary && ui.is_key_pressed_with_repeat(Key::X, false) {
                actions.push(FileTreeAction::Cut(paths.clone()));
            }
            if primary && ui.is_key_pressed_with_repeat(Key::D, false) {
                actions.push(FileTreeAction::Duplicate(paths.clone()));
            }
            if ui.is_key_pressed_with_repeat(Key::Delete, false)
                || (primary && ui.is_key_pressed_with_repeat(Key::Backspace, false))
            {
                actions.push(FileTreeAction::TrashMany(paths.clone()));
            }
            if selected.len() == 1
                && paths.len() == 1
                && ui.is_key_pressed_with_repeat(Key::F2, false)
            {
                actions.push(FileTreeAction::Rename(paths[0].clone()));
            }
        }
        if primary && ui.is_key_pressed_with_repeat(Key::V, false) {
            let destination = selection
                .focused
                .as_ref()
                .and_then(|path| find(root, path))
                .map(|node| {
                    if node.is_directory {
                        node.full_path.clone()
                    } else {
                        parent_directory(&node.full_path)
                    }
                })
                .unwrap_or_else(|| root.full_path.clone());
            actions.push(FileTreeAction::Paste(destination));
        }
        if ui.is_key_pressed_with_repeat(Key::F5, false) {
            actions.push(FileTreeAction::Refresh);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn draw_rows(
        &mut self,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
        context_menu: bool,
        remote: bool,
        extensions: Option<&ExtensionMenu<'_>>,
    ) -> Vec<FileTreeAction> {
        fn collect(
            node: &FileNode,
            path: &mut Vec<usize>,
            inherited_hidden: bool,
            visibility: &Visibility<'_>,
            rows: &mut Vec<(String, TreeRow)>,
        ) {
            let hidden = !path.is_empty() && (inherited_hidden || visibility.hidden(node));
            if hidden && !visibility.show_hidden {
                return;
            }
            rows.push((
                node.full_path.clone(),
                TreeRow {
                    path: path.clone(),
                    depth: path.len() as i32,
                    hidden,
                },
            ));
            if node.is_directory && node.is_open {
                for (index, child) in node.children.iter().enumerate() {
                    path.push(index);
                    collect(child, path, hidden, visibility, rows);
                    path.pop();
                }
            }
        }
        fn node_at<'a>(mut node: &'a mut FileNode, path: &[usize]) -> Option<&'a mut FileNode> {
            for index in path {
                node = node.children.get_mut(*index)?;
            }
            Some(node)
        }
        fn valid_row(mut node: &FileNode, path: &[usize], key: &str) -> bool {
            for index in path {
                let Some(child) = node.children.get(*index) else {
                    return false;
                };
                node = child;
            }
            node.full_path == key
        }
        let root = self.root_node.full_path.clone();
        let visibility = Visibility {
            root: &root,
            preferences: &self.preferences,
            show_hidden: self.show_hidden,
            remote,
        };
        let mut rows = Vec::new();
        collect(
            &self.root_node,
            &mut Vec::new(),
            false,
            &visibility,
            &mut rows,
        );
        fn collect_loaded(
            node: &FileNode,
            paths: &mut BTreeSet<String>,
            files: &mut BTreeSet<String>,
        ) {
            paths.insert(node.full_path.clone());
            if !node.is_directory {
                files.insert(node.full_path.clone());
            }
            for child in &node.children {
                collect_loaded(child, paths, files);
            }
        }
        let mut loaded_paths = BTreeSet::new();
        let mut visible_files = BTreeSet::new();
        collect_loaded(&self.root_node, &mut loaded_paths, &mut visible_files);
        let desired: Vec<_> = rows.iter().map(|(key, _)| key.clone()).collect();
        let frame = ui.frame_count();
        let host_id =
            ui.with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetCurrentWindow()).ID });

        let host = self.animations.hosts.entry(host_id).or_default();
        let reset = host.root != root
            || host.preferences != self.preferences
            || host.show_hidden != self.show_hidden;
        if reset {
            host.rows.clear();
            if host.root != root {
                host.interaction = FileTreeSelection::default();
                host.popup_selection.clear();
            }
        }
        host.root = root.clone();
        host.preferences = self.preferences.clone();
        host.show_hidden = self.show_hidden;
        host.last_frame = frame;
        // Include this panel's context menus when deciding whether it lost focus.
        if !ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS) {
            host.interaction = FileTreeSelection::default();
        }
        host.interaction
            .selected
            .retain(|path| loaded_paths.contains(path));
        if host
            .interaction
            .focused
            .as_ref()
            .is_some_and(|path| !loaded_paths.contains(path))
        {
            host.interaction.focused = None;
        }
        host.rows.extend(rows);
        host.motion
            .update(&desired, ui.time(), frame, style.animations, false, reset);
        if host.motion.rows().len() != desired.len() {
            host.motion.retain(|key| {
                host.rows
                    .get(key)
                    .is_some_and(|row| valid_row(&self.root_node, &row.path, key))
            });
        }
        let row_height = (ui.current_font_size() * 1.32 + 2.0).max(ui.current_font_size() * 1.2)
            + ui.current_font_size() * 0.15;
        let layout = host.motion.layout(ui, row_height);
        let mut actions = Vec::new();
        let previous_focus = host.interaction.focused.clone();
        Self::keyboard_actions(
            ui,
            &mut self.root_node,
            &desired,
            &mut host.interaction,
            &mut actions,
        );
        if previous_focus != host.interaction.focused
            && let Some(index) = host
                .interaction
                .focused
                .as_ref()
                .and_then(|focused| host.motion.rows().iter().position(|path| path == focused))
            && !layout.visible.contains(&index)
        {
            ui.set_scroll_from_pos_y(layout.position(index)[1], 0.5);
        }
        host.drop_targets.clear();
        if ui.drag_drop_payload().is_none()
            && self.external_drop_target.is_none()
            && !ui.is_mouse_down(dear_imgui_rs::MouseButton::Left)
        {
            self.animations.drag = None;
            host.hover_expand = None;
        }
        #[cfg(test)]
        host.labels.clear();
        for index in layout.visible.clone() {
            if layout.height(index) <= 0.0 {
                continue;
            }
            let key = &host.motion.rows()[index];
            let row = &host.rows[key];
            let motion = host.motion.sample(key);
            let Some(node) = node_at(&mut self.root_node, &row.path) else {
                continue;
            };
            ui.set_cursor_pos(layout.position(index));
            let _clip = layout.row_clip(ui, index);
            let _alpha =
                ui.push_style_var(StyleVar::Alpha(ui.clone_style().alpha() * motion.alpha));
            let _disabled_alpha = ui.push_style_var(StyleVar::DisabledAlpha(1.0));
            let _disabled = ui.begin_disabled_with_cond(!motion.interactive);
            Self::draw_node_row(
                node,
                row.depth,
                ui,
                current_path,
                style,
                icons,
                modified,
                context_menu,
                row.path.is_empty(),
                remote,
                &visibility,
                row.hidden,
                &mut actions,
                extensions,
                &mut self.file_info,
                &desired,
                &visible_files,
                &mut host.interaction,
                &mut host.popup_selection,
                &mut host.drop_targets,
                &mut self.animations.drag,
                &mut host.hover_expand,
                self.external_drop_target.as_deref(),
            );
            #[cfg(test)]
            host.labels
                .insert(node.full_path.clone(), ui.item_rect_min());
        }
        if let Some(dragging) = self.animations.drag.as_ref()
            && dragging.root == root
            && valid_move_destination(&dragging.paths, &root, &root)
            && ui.is_window_hovered()
            && !host.drop_targets.iter().any(|target| {
                let mouse = ui.io().mouse_pos();
                mouse[0] >= target.min[0]
                    && mouse[0] < target.max[0]
                    && mouse[1] >= target.min[1]
                    && mouse[1] < target.max[1]
            })
        {
            // SAFETY: A custom ImGui target uses the current window's content
            // rectangle; each successful begin is paired immediately with end.
            ui.with_bound_context(|| unsafe {
                let window = &*dear_imgui_rs::sys::igGetCurrentWindow();
                if dear_imgui_rs::sys::igBeginDragDropTargetCustom(
                    window.InnerRect,
                    host_id.wrapping_add(1),
                ) {
                    let payload = dear_imgui_rs::sys::igAcceptDragDropPayload(
                        c"BED_FILES".as_ptr(),
                        dear_imgui_rs::sys::ImGuiDragDropFlags_AcceptBeforeDelivery,
                    );
                    if !payload.is_null() && (*payload).Delivery {
                        actions.push(FileTreeAction::Move {
                            paths: dragging.paths.clone(),
                            destination: root.clone(),
                        });
                    }
                    dear_imgui_rs::sys::igEndDragDropTarget();
                }
            });
        }
        layout.finish(ui);
        if self.animations.drag.is_some() || self.external_drop_target.is_some() {
            let pos = self
                .external_drag_position
                .unwrap_or_else(|| ui.io().mouse_pos());
            let window_pos = ui.window_pos();
            let window_size = ui.window_size();
            if pos[0] >= window_pos[0] && pos[0] <= window_pos[0] + window_size[0] {
                let edge = row_height * 1.5;
                let speed = row_height * 12.0 * ui.io().delta_time();
                if pos[1] >= window_pos[1] && pos[1] < window_pos[1] + edge {
                    ui.set_scroll_y((ui.scroll_y() - speed).max(0.0));
                } else if pos[1] <= window_pos[1] + window_size[1]
                    && pos[1] > window_pos[1] + window_size[1] - edge
                {
                    ui.set_scroll_y(ui.scroll_y() + speed);
                }
            }
        }
        host.rows.retain(|key, _| host.motion.contains(key));
        actions
    }
    #[allow(clippy::too_many_arguments)] // Recursive upstream row painter and its action output.
    fn draw_node_row(
        node: &mut FileNode,
        depth: i32,
        ui: &Ui,
        current_path: &str,
        style: &FileTreeStyle,
        icons: Option<&dyn FileIcons>,
        modified: Option<&dyn Fn(&str) -> bool>,
        context_menu: bool,
        root: bool,
        remote: bool,
        visibility: &Visibility<'_>,
        inherited_hidden: bool,
        actions: &mut Vec<FileTreeAction>,
        extensions: Option<&ExtensionMenu<'_>>,
        file_info: &mut super::file_info::FileInfoHover,
        visible: &[String],
        visible_files: &BTreeSet<String>,
        selection: &mut FileTreeSelection,
        popup_selection: &mut Vec<String>,
        drop_targets: &mut Vec<FileTreeDropTarget>,
        drag: &mut Option<TreeDrag>,
        hover_expand: &mut Option<(String, f64)>,
        external_drop_target: Option<&str>,
    ) {
        let hidden = !root && (inherited_hidden || visibility.hidden(node));
        if hidden && !visibility.show_hidden {
            return;
        }
        let font_size = ui.current_font_size();
        let _rounding = ui.push_style_var(StyleVar::FrameRounding(font_size * 0.3));
        let _padding =
            ui.push_style_var(StyleVar::FramePadding([font_size * 0.2, font_size * 0.1]));
        let _spacing =
            ui.push_style_var(StyleVar::ItemSpacing([font_size * 0.05, font_size * 0.15]));
        let _id = ui.push_id(node.full_path.as_str());
        let icon_size = font_size;
        let item_height = ui.frame_height().max(font_size * 1.32 + 2.0);
        let row_origin = ui.cursor_pos();
        let indent = depth as f32 * font_size * 0.45;
        let row_pad_x = font_size * 0.2;
        let icon_text_gap = font_size * 0.35;
        let icon = icons.and_then(|icons| {
            if node.is_directory {
                icons
                    .get(if node.is_open {
                        "folder-open"
                    } else {
                        "folder"
                    })
                    .or_else(|| icons.get("folder"))
                    .or_else(|| icons.get("default"))
            } else {
                icons.get_for_file(&node.name)
            }
        });
        let text_size = ui.calc_text_size(&node.name);
        let required_width =
            indent + row_pad_x + font_size * 0.75 + icon_size + icon_text_gap + text_size[0];
        let width = required_width.max(ui.content_region_avail()[0]);
        let clicked = {
            let selected = selection.selected.contains(&node.full_path);
            let target = external_drop_target == Some(node.full_path.as_str());
            let selection_color = ui.style_color(StyleColor::TextSelectedBg);
            let highlight = if target {
                bed_editing::util::color::blend(
                    ui.style_color(StyleColor::NavCursor),
                    selection_color,
                    0.30,
                )
            } else {
                selection_color
            };
            let _bg = ui.push_style_color(
                StyleColor::Button,
                if selected || target {
                    highlight
                } else {
                    [0.0; 4]
                },
            );
            let _hover = ui.push_style_color(
                StyleColor::ButtonHovered,
                if selected || target {
                    bed_editing::util::color::blend(style.text_color, highlight, 0.08)
                } else {
                    ui.style_color(StyleColor::HeaderHovered)
                },
            );
            let _active = ui.push_style_color(
                StyleColor::ButtonActive,
                if selected || target {
                    bed_editing::util::color::blend(style.text_color, highlight, 0.16)
                } else {
                    ui.style_color(StyleColor::HeaderActive)
                },
            );
            let _border = ui.push_style_var(StyleVar::FrameBorderSize(0.0));
            let _pad = ui.push_style_var(StyleVar::FramePadding([0.0; 2]));
            ui.button_with_size(format!("##{}", node.full_path), [width, item_height])
        };
        let rect_min = ui.item_rect_min();
        let rect_max = ui.item_rect_max();
        if selection.focused.as_deref() == Some(node.full_path.as_str()) {
            let mut color = style.text_color;
            color[3] *= 0.5;
            ui.get_window_draw_list()
                .add_rect(rect_min, rect_max, color)
                .rounding(font_size * 0.2)
                .build();
        }
        let directory = if node.is_directory {
            node.full_path.clone()
        } else {
            parent_directory(&node.full_path)
        };
        drop_targets.push(FileTreeDropTarget {
            min: rect_min,
            max: rect_max,
            destination: directory.clone(),
        });
        if clicked {
            let toggle = ui.io().key_ctrl() || ui.io().key_super();
            let range = ui.io().key_shift();
            selection.select(&node.full_path, visible, toggle, range);
            // Buttons activate on release, after MouseDoubleClicked resets.
            // The retained click count prevents a double-click from immediately
            // reversing a folder expansion. Files activate on both releases so
            // the second press cannot leave focus back in the tree.
            let repeated_click = ui.with_bound_context(|| unsafe {
                (*dear_imgui_rs::sys::igGetIO_Nil()).MouseClickedLastCount[0].is_multiple_of(2)
            });
            if !toggle && !range {
                if node.is_directory {
                    if !repeated_click {
                        node.is_open = !node.is_open;
                    }
                } else {
                    actions.push(FileTreeAction::Open(node.full_path.clone()));
                }
            }
        }
        if !root && let Some(_source) = ui.drag_drop_source_config("BED_FILES").begin_payload(0_u32)
        {
            if !selection.selected.contains(&node.full_path) {
                selection.select(&node.full_path, visible, false, false);
            }
            if drag.is_none() {
                *drag = Some(TreeDrag {
                    root: visibility.root.to_owned(),
                    paths: mutation_paths(
                        &selection.selected.iter().cloned().collect::<Vec<_>>(),
                        visibility.root,
                    ),
                });
            }
            if let Some(drag) = drag.as_ref() {
                ui.text(format!("Move {} item(s)", drag.paths.len()));
            }
        }
        if let Some(dragging) = drag.as_ref()
            && dragging.root == visibility.root
            && valid_move_destination(&dragging.paths, visibility.root, &directory)
            && let Some(target) = ui.drag_drop_target()
            && let Some(Ok(payload)) = target.accept_payload::<u32, _>(
                "BED_FILES",
                dear_imgui_rs::DragDropTargetFlags::BEFORE_DELIVERY,
            )
        {
            if payload.delivery {
                actions.push(FileTreeAction::Move {
                    paths: dragging.paths.clone(),
                    destination: directory.clone(),
                });
            }
            if node.is_directory && !node.is_open {
                match hover_expand {
                    Some((path, since)) if path == &node.full_path => {
                        if ui.time() - *since >= 0.65 {
                            node.is_open = true;
                        }
                    }
                    _ => *hover_expand = Some((node.full_path.clone(), ui.time())),
                }
            }
        }
        if external_drop_target == Some(node.full_path.as_str())
            && node.is_directory
            && !node.is_open
        {
            match hover_expand {
                Some((path, since)) if path == &node.full_path => {
                    if ui.time() - *since >= 0.65 {
                        node.is_open = true;
                    }
                }
                _ => *hover_expand = Some((node.full_path.clone(), ui.time())),
            }
        }
        if context_menu {
            if ui.is_item_clicked_with_button(dear_imgui_rs::MouseButton::Right) {
                *popup_selection = selection.context_selection(&node.full_path);
            }
            let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
            if let Some(_menu) = ui.begin_popup_context_item() {
                let captured = popup_selection.clone();
                let mutating = mutation_paths(&captured, visibility.root);
                let files: Vec<_> = captured
                    .iter()
                    .filter(|path| visible_files.contains(*path))
                    .cloned()
                    .collect();
                if !root
                    && ui.menu_item_enabled_selected_no_shortcut("Open", false, !files.is_empty())
                {
                    actions.push(FileTreeAction::OpenManyFromMenu(files));
                }
                if ui.menu_item("New File…") {
                    actions.push(FileTreeAction::NewFile(directory.clone()));
                }
                if ui.menu_item("New Folder…") {
                    actions.push(FileTreeAction::NewFolder(directory.clone()));
                }
                if ui.menu_item("Paste") {
                    actions.push(FileTreeAction::Paste(directory));
                }
                if !mutating.is_empty() {
                    ui.separator();
                    if ui.menu_item("Cut") {
                        actions.push(FileTreeAction::Cut(mutating.clone()));
                    }
                    if ui.menu_item("Copy") {
                        actions.push(FileTreeAction::Copy(mutating.clone()));
                    }
                    if ui.menu_item("Duplicate") {
                        actions.push(FileTreeAction::Duplicate(mutating.clone()));
                    }
                    if ui.menu_item_enabled_selected_no_shortcut(
                        "Rename…",
                        false,
                        captured.len() == 1,
                    ) {
                        actions.push(FileTreeAction::Rename(captured[0].clone()));
                    }
                    if ui.menu_item(if remote { "Delete…" } else { "Move to Trash" }) {
                        if mutating.len() == 1 {
                            actions.push(FileTreeAction::Trash(mutating[0].clone()));
                        } else {
                            actions.push(FileTreeAction::TrashMany(mutating));
                        }
                    }
                    if captured.len() == 1 {
                        let manually_hidden =
                            relative_path(visibility.root, &node.full_path, remote).is_some_and(
                                |path| visibility.preferences.hidden_paths.contains(&path),
                            );
                        if ui.menu_item(if manually_hidden {
                            "Unhide from File Tree"
                        } else {
                            "Hide from File Tree"
                        }) {
                            actions.push(FileTreeAction::SetPathHidden {
                                path: node.full_path.clone(),
                                hidden: !manually_hidden,
                            });
                        }
                    }
                }
                ui.separator();
                Self::visibility_menu(ui, visibility, actions);
                if let Some(extensions) = extensions {
                    extensions(
                        ui,
                        &node.full_path,
                        node.is_directory,
                        false,
                        &captured,
                        actions,
                    );
                }
            }
        }
        if ui.is_item_hovered_with_flags(
            dear_imgui_rs::ItemHoveredFlags::DELAY_NORMAL
                | dear_imgui_rs::ItemHoveredFlags::NO_SHARED_DELAY,
        ) && drag.is_none()
        {
            file_info.draw(ui, &node.full_path, visibility.root, remote);
        }
        // Dim labels and icons only; keep context-menu text fully legible.
        let _dim =
            hidden.then(|| ui.push_style_var(StyleVar::Alpha(ui.clone_style().alpha() * 0.45)));
        let center_y = row_origin[1] + item_height * 0.5;
        let icon_x = row_origin[0] + indent + row_pad_x + font_size * 0.75;
        let text_x = icon_x + icon_size + icon_text_gap;
        ui.set_cursor_pos([icon_x, center_y - icon_size * 0.5]);
        if let Some(icon) = icon {
            let tint = if node.is_directory {
                ui.style_color(StyleColor::Text)
            } else {
                icons.map_or([1.0; 4], |icons| {
                    icons.file_icon_tint(&node.name, ui.style_color(StyleColor::Text))
                })
            };
            ui.image_config(icon, [icon_size; 2])
                .tint_color(tint)
                .build();
        }
        ui.set_cursor_pos([text_x, center_y - ui.text_line_height() * 0.5]);
        let color = if !node.is_directory && node.full_path == current_path && style.rainbow {
            let t = style.rainbow_time * 2.0;
            [
                t.sin() * 0.5 + 0.5,
                (t + 2.0944).sin() * 0.5 + 0.5,
                (t + 4.1888).sin() * 0.5 + 0.5,
                1.0,
            ]
        } else if !node.is_directory
            && node.full_path != current_path
            && modified.is_some_and(|f| f(&node.full_path))
        {
            ui.style_color(StyleColor::TextDisabled)
        } else {
            style.text_color
        };
        let color = bed_editing::util::color::ensure_contrast(
            color,
            ui.style_color(StyleColor::ChildBg),
            4.5,
        );
        {
            let _text = ui.push_style_color(StyleColor::Text, color);
            ui.text(&node.name);
        }
        drop(_dim);
        if node.is_directory {
            let x = rect_min[0] + indent + row_pad_x + font_size * 0.3;
            let y = rect_min[1] + item_height * 0.5;
            let side = font_size * 0.7;
            if let Some(texture) = icons.and_then(|icons| {
                icons.get(if node.is_open {
                    "chevron-down"
                } else {
                    "chevron-right"
                })
            }) {
                ui.get_window_draw_list().add_image(
                    texture,
                    [x - side * 0.5, y - side * 0.5],
                    [x + side * 0.5, y + side * 0.5],
                    [0.0; 2],
                    [1.0; 2],
                    style.text_color,
                );
            } else {
                // Embedding hosts can omit textures; keep their expander visible.
                let r = font_size * 0.2;
                let points = if node.is_open {
                    [[x - r, y - r * 0.6], [x + r, y - r * 0.6], [x, y + r]]
                } else {
                    [[x - r * 0.6, y - r], [x - r * 0.6, y + r], [x + r, y]]
                };
                ui.get_window_draw_list()
                    .add_triangle(points[0], points[1], points[2], style.text_color)
                    .filled(true)
                    .build();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn selection_toggle_ranges_and_popup_capture_are_independent() {
        let visible: Vec<String> = ["/p/a", "/p/b", "/p/c", "/p/d"].map(str::to_owned).to_vec();
        let mut first = FileTreeSelection::default();
        let second = FileTreeSelection::default();
        first.select("/p/b", &visible, false, false);
        first.select("/p/d", &visible, false, true);
        assert_eq!(
            first.selected,
            BTreeSet::from(["/p/b".into(), "/p/c".into(), "/p/d".into()])
        );
        assert_eq!(first.anchor.as_deref(), Some("/p/b"));
        first.select("/p/c", &visible, true, false);
        assert!(!first.selected.contains("/p/c"));
        let captured = first.context_selection("/p/d");
        assert_eq!(captured, ["/p/b", "/p/d"]);
        first.select("/p/a", &visible, false, false);
        assert_eq!(
            captured,
            ["/p/b", "/p/d"],
            "popup keeps its originating selection"
        );
        assert!(
            second.selected.is_empty(),
            "another panel has independent selection"
        );
        assert_eq!(first.context_selection("/p/c"), ["/p/c"]);
    }
    #[test]
    fn ranges_use_visible_order_and_recover_from_a_hidden_anchor() {
        let visible = vec!["/p/a".into(), "/p/d".into(), "/p/f".into()];
        let mut selection = FileTreeSelection::default();
        selection.select("/p/a", &visible, false, false);
        selection.select("/p/f", &visible, false, true);
        assert_eq!(selection.selected.len(), 3);
        assert!(!selection.selected.contains("/p/b"));
        selection.anchor = Some("/p/hidden".into());
        selection.select("/p/d", &visible, false, true);
        assert_eq!(selection.selected, BTreeSet::from(["/p/d".into()]));
    }
    #[test]
    fn recursive_operations_deduplicate_descendants_and_reject_invalid_moves() {
        let paths: Vec<String> = [
            "/p",
            "/p/folder/child",
            "/p/folder",
            "/p/folder-other",
            "/p/folder",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            mutation_paths(&paths, "/p"),
            ["/p/folder", "/p/folder-other"]
        );
        let source = vec!["/p/folder".into()];
        assert!(valid_move_destination(&source, "/p", "/p"));
        assert!(valid_move_destination(&source, "/p", "/p/folder-other"));
        for destination in ["/p/folder", "/p/folder/child", "/p-other", "/p/../escape"] {
            assert!(
                !valid_move_destination(&source, "/p", destination),
                "invalid target: {destination}"
            );
        }
        assert!(!valid_move_destination(&["/p".into()], "/p", "/p/child"));
    }
    #[test]
    fn native_selection_keyboard_and_cross_panel_drag_use_complete_selection() {
        use dear_imgui_rs::{
            Condition, Context, FramePrepareOptions, Key, MouseButton, WindowFlags,
        };
        fn frame(context: &mut Context, tree: &mut FileTree) -> Vec<FileTreeAction> {
            context.prepare_frame(FramePrepareOptions::new([740.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut actions = Vec::new();
            for (title, left) in [("First Files", 0.0), ("Second Files", 370.0)] {
                ui.window(title)
                    .position([left, 0.0], Condition::Always)
                    .size([360.0, 300.0], Condition::Always)
                    .flags(
                        WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE,
                    )
                    .build(|| {
                        actions.extend(tree.display_backend_actions(
                            ui,
                            "",
                            &FileTreeStyle {
                                animations: false,
                                ..Default::default()
                            },
                            None,
                            None,
                            true,
                        ));
                    });
            }
            drop(context.render_legacy());
            actions
        }
        fn click(
            context: &mut Context,
            tree: &mut FileTree,
            point: [f32; 2],
        ) -> Vec<FileTreeAction> {
            let mut actions = Vec::new();
            context.io_mut().add_mouse_pos_event(point);
            for down in [true, false] {
                context
                    .io_mut()
                    .add_mouse_button_event(MouseButton::Left, down);
                actions.extend(frame(context, tree));
            }
            actions
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut tree = FileTree {
            root_node: FileNode {
                name: "project".into(),
                full_path: "/fixture".into(),
                is_directory: true,
                is_open: true,
                children: vec![
                    FileNode {
                        name: "target".into(),
                        full_path: "/fixture/target".into(),
                        is_directory: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "a".into(),
                        full_path: "/fixture/a".into(),
                        ..Default::default()
                    },
                    FileNode {
                        name: "b".into(),
                        full_path: "/fixture/b".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        frame(&mut context, &mut tree);
        frame(&mut context, &mut tree);
        let first_id = *tree
            .animations
            .hosts
            .iter()
            .min_by(|(_, a), (_, b)| {
                a.labels["/fixture/a"][0].total_cmp(&b.labels["/fixture/a"][0])
            })
            .unwrap()
            .0;
        let second_id = *tree
            .animations
            .hosts
            .keys()
            .find(|id| **id != first_id)
            .unwrap();
        let point = |tree: &FileTree, id: u32, path: &str| {
            let pos = tree.animations.hosts[&id].labels[path];
            [pos[0] + 2.0, pos[1] + 5.0]
        };
        let a = point(&tree, first_id, "/fixture/a");
        assert_eq!(
            click(&mut context, &mut tree, a),
            [FileTreeAction::Open("/fixture/a".into())],
            "single click selects and opens the file"
        );
        assert_eq!(
            click(&mut context, &mut tree, a),
            [FileTreeAction::Open("/fixture/a".into())],
            "the second click restores document focus through the host"
        );
        context.io_mut().add_key_event(Key::ModSuper, true);
        let b = point(&tree, first_id, "/fixture/b");
        assert!(click(&mut context, &mut tree, b).is_empty());
        context.io_mut().add_key_event(Key::ModSuper, false);
        frame(&mut context, &mut tree);
        assert_eq!(
            tree.animations.hosts[&first_id].interaction.selected,
            BTreeSet::from(["/fixture/a".into(), "/fixture/b".into()])
        );
        assert!(
            tree.animations.hosts[&second_id]
                .interaction
                .selected
                .is_empty()
        );
        context.io_mut().add_key_event(Key::Enter, true);
        assert_eq!(
            frame(&mut context, &mut tree),
            [FileTreeAction::OpenMany(vec![
                "/fixture/a".into(),
                "/fixture/b".into()
            ])]
        );
        context.io_mut().add_key_event(Key::Enter, false);
        frame(&mut context, &mut tree);
        // Begin from an already selected row and drag into the other Files panel.
        context.io_mut().add_mouse_pos_event(a);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut tree);
        let target = point(&tree, second_id, "/fixture/target");
        context.io_mut().add_mouse_pos_event(target);
        frame(&mut context, &mut tree);
        frame(&mut context, &mut tree);
        assert_eq!(
            tree.animations.drag.as_ref().unwrap().paths,
            ["/fixture/a", "/fixture/b"]
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        let actions = frame(&mut context, &mut tree);
        assert_eq!(
            actions,
            [FileTreeAction::Move {
                paths: vec!["/fixture/a".into(), "/fixture/b".into()],
                destination: "/fixture/target".into()
            }]
        );
        let folder = point(&tree, first_id, "/fixture/target");
        assert!(click(&mut context, &mut tree, folder).is_empty());
        assert!(
            tree.root_node.children[0].is_open,
            "folder label expands on single click"
        );
        assert_eq!(
            tree.animations.hosts[&first_id].interaction.selected,
            BTreeSet::from(["/fixture/target".into()])
        );
        assert!(click(&mut context, &mut tree, folder).is_empty());
        assert!(
            tree.root_node.children[0].is_open,
            "double click must not reverse expansion"
        );
        for _ in 0..24 {
            frame(&mut context, &mut tree);
        }
        context.io_mut().add_key_event(Key::ModSuper, true);
        assert!(click(&mut context, &mut tree, folder).is_empty());
        assert!(
            tree.root_node.children[0].is_open,
            "modifier clicks only change selection"
        );
        context.io_mut().add_key_event(Key::ModSuper, false);
        frame(&mut context, &mut tree);
        let font_size = context
            .binding()
            .with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFontSize() });
        let arrow = [folder[0] - font_size * 1.7, folder[1]];
        assert!(click(&mut context, &mut tree, arrow).is_empty());
        assert!(
            !tree.root_node.children[0].is_open,
            "disclosure click toggles and selects"
        );
        assert_eq!(
            tree.animations.hosts[&first_id].interaction.selected,
            BTreeSet::from(["/fixture/target".into()])
        );
        let other_file = point(&tree, second_id, "/fixture/b");
        assert_eq!(
            click(&mut context, &mut tree, other_file),
            [FileTreeAction::Open("/fixture/b".into())]
        );
        assert_eq!(
            tree.animations.hosts[&first_id].interaction,
            FileTreeSelection::default(),
            "moving focus to another panel clears selection and its range anchor"
        );
        assert_eq!(
            tree.animations.hosts[&second_id].interaction.selected,
            BTreeSet::from(["/fixture/b".into()])
        );
        assert!(click(&mut context, &mut tree, [100.0, 250.0]).is_empty());
        context.io_mut().add_key_event(Key::Enter, true);
        assert!(
            frame(&mut context, &mut tree).is_empty(),
            "returning focus must not reopen the previously selected file"
        );
        assert_eq!(
            tree.animations.hosts[&second_id].interaction,
            FileTreeSelection::default(),
            "the other panel also clears its selection when it loses focus"
        );
    }
    #[test]
    fn branch_motion_eases_height_keeps_current_hit_targets_and_reverses() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton, WindowFlags};
        fn frame(
            context: &mut Context,
            tree: &mut FileTree,
            dt: f32,
        ) -> (Vec<FileTreeAction>, f32, bool) {
            context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], dt));
            let ui = context.frame();
            let mut actions = Vec::new();
            let mut height = 0.0;
            let mut popup = false;
            ui.window("Animated Files")
                .position([0.0, 0.0], Condition::Always)
                .size([400.0, 300.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
                .build(|| {
                    actions = tree.display_backend_actions(
                        ui,
                        "",
                        &FileTreeStyle::default(),
                        None,
                        None,
                        true,
                    );
                    ui.with_bound_context(|| unsafe {
                        let window = &*dear_imgui_rs::sys::igGetCurrentWindow();
                        height = window.DC.CursorMaxPos.y - window.DC.CursorStartPos.y;
                        popup = (*dear_imgui_rs::sys::igGetCurrentContext())
                            .OpenPopupStack
                            .Size
                            > 0;
                    });
                });
            drop(context.render_legacy());
            (actions, height, popup)
        }
        fn click(
            context: &mut Context,
            tree: &mut FileTree,
            pos: [f32; 2],
            button: MouseButton,
        ) -> Vec<FileTreeAction> {
            let mut actions = Vec::new();
            for down in [true, false] {
                context.io_mut().add_mouse_pos_event(pos);
                context.io_mut().add_mouse_button_event(button, down);
                actions.extend(frame(context, tree, 0.001).0);
            }
            actions
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context.style_mut().set_window_padding([0.0; 2]);
        context.binding().with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetIO_Nil()).MouseDoubleClickTime = 0.001;
        });
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut tree = FileTree {
            root_node: FileNode {
                name: "project".into(),
                full_path: "/fixture".into(),
                is_directory: true,
                is_open: true,
                children: vec![
                    FileNode {
                        name: "branch".into(),
                        full_path: "/fixture/branch".into(),
                        is_directory: true,
                        is_open: true,
                        children: vec![FileNode {
                            name: "inside".into(),
                            full_path: "/fixture/branch/inside".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    FileNode {
                        name: "outside".into(),
                        full_path: "/fixture/outside".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        frame(&mut context, &mut tree, 1.0 / 60.0);
        let (_, expanded_height, _) = frame(&mut context, &mut tree, 1.0 / 60.0);
        let font_size = context
            .binding()
            .with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFontSize() });
        let host = tree.animations.hosts.values().next().unwrap();
        let inside = host.labels["/fixture/branch/inside"];
        let outside = host.labels["/fixture/outside"];
        assert!(
            (inside[0] - outside[0] - font_size * 0.45).abs() < 0.01,
            "file indentation must be half the original 0.9 font sizes"
        );
        let branch = host.labels["/fixture/branch"];
        let branch_click = [branch[0] - font_size * 1.7, branch[1] + font_size * 0.5];
        assert!(click(&mut context, &mut tree, branch_click, MouseButton::Left).is_empty());
        assert!(!tree.root_node.children[0].is_open);
        frame(&mut context, &mut tree, 0.001);
        let (_, halfway_height, _) = frame(&mut context, &mut tree, 0.084);
        let pitch = expanded_height / 4.0;
        assert!(
            (halfway_height - pitch * 3.5).abs() < 0.1,
            "sibling positions and scroll extent must follow the shrinking child height"
        );
        let host = tree.animations.hosts.values().next().unwrap();
        let closing = host.motion.sample(&"/fixture/branch/inside".into());
        assert!(!closing.interactive);
        let child_pos = [inside[0] + 2.0, pitch * 2.0 + 2.0];
        assert!(
            click(&mut context, &mut tree, child_pos, MouseButton::Left).is_empty(),
            "closing children cannot open files"
        );
        assert!(click(&mut context, &mut tree, child_pos, MouseButton::Right).is_empty());
        assert!(
            !frame(&mut context, &mut tree, 0.001).2,
            "closing children cannot open context menus"
        );
        click(&mut context, &mut tree, branch_click, MouseButton::Left);
        let before_reversal = tree
            .animations
            .hosts
            .values()
            .next()
            .unwrap()
            .motion
            .sample(&"/fixture/branch/inside".into())
            .height;
        frame(&mut context, &mut tree, 0.001);
        let after_reversal = tree
            .animations
            .hosts
            .values()
            .next()
            .unwrap()
            .motion
            .sample(&"/fixture/branch/inside".into());
        assert!(tree.root_node.children[0].is_open);
        assert!(after_reversal.interactive);
        assert!(
            (before_reversal - after_reversal.height).abs() < 0.02,
            "reopening must continue from the current row height"
        );
        let (_, settled_height, _) = frame(&mut context, &mut tree, 0.2);
        assert!((settled_height - expanded_height).abs() < 0.01);
    }
    #[test]
    fn local_tree_refresh_updates_ignore_metadata_and_reveals_ignored_folders() {
        let temp = TempDir::new();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(temp.root())
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(output.status.success());
        temp.write(".gitignore", b"ignored/\n*.log\n");
        temp.write("ignored/file", b"");
        temp.write("drop.log", b"");
        let root = temp.root().to_str().unwrap();
        let mut tree = FileTree::default();
        tree.preferences.hide_gitignored = true;
        tree.refresh_file_tree(root).unwrap();
        assert!(tree.error.is_none());
        let directory = tree
            .root_node
            .children
            .iter_mut()
            .find(|node| node.name == "ignored")
            .unwrap();
        assert!(directory.is_gitignored);
        directory.is_open = true;
        tree.refresh_file_tree(root).unwrap();
        assert!(
            tree.root_node
                .children
                .iter()
                .find(|node| node.name == "ignored")
                .unwrap()
                .children
                .is_empty()
        );
        tree.show_hidden = true;
        tree.refresh_file_tree(root).unwrap();
        let directory = tree
            .root_node
            .children
            .iter()
            .find(|node| node.name == "ignored")
            .unwrap();
        assert!(directory.is_open);
        assert_eq!(directory.children[0].name, "file");
        temp.write(".gitignore", b"");
        tree.refresh_file_tree(root).unwrap();
        assert!(
            tree.root_node
                .children
                .iter()
                .all(|node| !node.is_gitignored)
        );
    }
    #[test]
    fn filters_combine_without_discarding_nodes_or_expansion() {
        let temp = TempDir::new();
        for name in [
            ".secret/child",
            "manual/old",
            "manual-other/file",
            "ignored/file",
            "visible",
        ] {
            temp.write(name, b"");
        }
        let root = temp.root().to_str().unwrap();
        let mut tree = FileTree::default();
        tree.refresh_file_tree(root).unwrap();
        for node in &mut tree.root_node.children {
            if node.is_directory {
                node.is_open = true;
                FileTree::build_file_tree(Path::new(&node.full_path.clone()), node).unwrap();
            }
            node.is_gitignored = node.name == "ignored";
        }
        tree.apply_visibility_action(&FileTreeAction::SetHideHidden(true), false);
        tree.apply_visibility_action(&FileTreeAction::SetHideGitignored(true), false);
        tree.apply_visibility_action(
            &FileTreeAction::SetPathHidden {
                path: temp.path("manual").to_str().unwrap().into(),
                hidden: true,
            },
            false,
        );
        let visibility = Visibility {
            root,
            preferences: &tree.preferences,
            show_hidden: false,
            remote: false,
        };
        let visible: Vec<_> = tree
            .root_node
            .children
            .iter()
            .filter(|node| !visibility.hidden(node))
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(visible, ["manual-other", "visible"]);
        assert!(!visibility.hidden(&tree.root_node));
        if cfg!(unix) {
            let open = tree.open_directories();
            assert_eq!(open.len(), 2);
            assert!(open[1].ends_with("/manual-other"));
        }
        tree.apply_visibility_action(&FileTreeAction::SetShowHidden(true), false);
        assert_eq!(tree.open_directories().len(), 5);
        // Concealed directories are retained but not refreshed until revealed.
        tree.show_hidden = false;
        temp.write("manual/new", b"");
        tree.refresh_file_tree(root).unwrap();
        let manual = tree
            .root_node
            .children
            .iter()
            .find(|node| node.name == "manual")
            .unwrap();
        assert!(manual.is_open);
        assert_eq!(
            manual
                .children
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["old"]
        );
        tree.show_hidden = true;
        tree.refresh_file_tree(root).unwrap();
        let manual = tree
            .root_node
            .children
            .iter()
            .find(|node| node.name == "manual")
            .unwrap();
        assert_eq!(
            manual
                .children
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        tree.apply_visibility_action(
            &FileTreeAction::SetPathHidden {
                path: temp.path("manual").to_str().unwrap().into(),
                hidden: false,
            },
            false,
        );
        assert!(tree.preferences.hidden_paths.is_empty());
    }

    #[test]
    fn unhide_keeps_category_rules_and_root_cannot_be_manually_hidden() {
        let mut tree = FileTree::default();
        tree.root_node.full_path = "/remote/project".into();
        tree.preferences.hide_hidden = true;
        for path in [
            "/remote/project",
            "/remote/project-other/file",
            "/remote/project/../file",
        ] {
            tree.apply_visibility_action(
                &FileTreeAction::SetPathHidden {
                    path: path.into(),
                    hidden: true,
                },
                true,
            );
        }
        assert!(tree.preferences.hidden_paths.is_empty());
        let node = FileNode {
            full_path: "/remote/project/.secret".into(),
            ..Default::default()
        };
        for hidden in [true, false] {
            tree.apply_visibility_action(
                &FileTreeAction::SetPathHidden {
                    path: node.full_path.clone(),
                    hidden,
                },
                true,
            );
        }
        let visibility = Visibility {
            root: &tree.root_node.full_path,
            preferences: &tree.preferences,
            show_hidden: false,
            remote: true,
        };
        assert!(visibility.hidden(&node));
        assert!(tree.preferences.hidden_paths.is_empty());
        tree.preferences = FileTreePreferences::from_value(
            &json!({"hidden_paths":["", "../escape", "/abs", "dir", "dir", "a/b"]}),
        );
        assert_eq!(
            tree.preferences.hidden_paths,
            BTreeSet::from(["a/b".into(), "dir".into()])
        );
    }

    #[test]
    fn remote_directory_updates_preserve_expansion_without_local_access() {
        let mut tree = FileTree {
            root_node: FileNode {
                name: "remote".into(),
                full_path: "/unavailable/local/root".into(),
                is_directory: true,
                is_open: true,
                children: Vec::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        let entry = |name: &str, is_directory| bed_remote::DirectoryEntry {
            path: format!("/unavailable/local/root/{name}"),
            name: name.into(),
            is_directory,
            is_symlink: false,
            is_gitignored: false,
        };
        tree.apply_directory(
            "/unavailable/local/root",
            vec![
                entry("z.txt", false),
                entry("dir", true),
                entry(".DS_Store", false),
            ],
        );
        tree.root_node.children[0].is_open = true;
        tree.apply_directory(
            "/unavailable/local/root/dir",
            vec![bed_remote::DirectoryEntry {
                path: "/unavailable/local/root/dir/child".into(),
                name: "child".into(),
                is_directory: false,
                is_symlink: false,
                is_gitignored: false,
            }],
        );
        tree.apply_directory(
            "/unavailable/local/root",
            vec![entry("a.txt", false), entry("dir", true)],
        );
        assert_eq!(tree.root_node.children[0].name, "dir");
        assert!(tree.root_node.children[0].is_open);
        assert_eq!(tree.root_node.children[0].children[0].name, "child");
        assert_eq!(
            tree.open_directories(),
            vec!["/unavailable/local/root", "/unavailable/local/root/dir"]
        );
        assert_eq!(
            tree.is_directory("/unavailable/local/root/a.txt"),
            Some(false)
        );
        tree.apply_visibility_action(
            &FileTreeAction::SetPathHidden {
                path: "/unavailable/local/root/dir".into(),
                hidden: true,
            },
            true,
        );
        assert_eq!(tree.open_directories(), ["/unavailable/local/root"]);
        tree.apply_visibility_action(&FileTreeAction::SetShowHidden(true), true);
        assert_eq!(tree.open_directories().len(), 2);
    }
    #[test]
    fn directories_first_case_sensitive_names_and_no_unconditional_skips() {
        let temp = TempDir::new();
        fs::create_dir(temp.path("z-dir")).unwrap();
        fs::create_dir(temp.path("a-dir")).unwrap();
        temp.write(".git/config", b"");
        for name in ["B.txt", "a.txt", ".DS_Store", "thumbs.db"] {
            temp.write(name, b"");
        }
        let mut tree = FileTree::default();
        tree.refresh_file_tree(temp.root().to_str().unwrap())
            .unwrap();
        assert_eq!(
            tree.root_node
                .children
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                ".git",
                "a-dir",
                "z-dir",
                ".DS_Store",
                "B.txt",
                "a.txt",
                "thumbs.db"
            ]
        );
        assert!(tree.root_node.is_open);
        assert!(tree.root_node.children[..3].iter().all(|n| n.is_directory));
        assert!(tree.root_node.children[0].children.is_empty());
        // Names keep their original case even on case-insensitive volumes.
        fs::remove_file(temp.path("thumbs.db")).unwrap();
        temp.write("Thumbs.db", b"");
        tree.refresh_file_tree(temp.root().to_str().unwrap())
            .unwrap();
        assert!(
            tree.root_node
                .children
                .iter()
                .any(|node| node.name == "Thumbs.db")
        );
    }
    #[test]
    fn preserves_open_and_loaded_closed_subtrees_until_reopened() {
        let temp = TempDir::new();
        temp.write("dir/old", b"");
        let mut tree = FileTree::default();
        tree.refresh_file_tree(temp.root().to_str().unwrap())
            .unwrap();
        let node = &mut tree.root_node.children[0];
        node.is_open = true;
        FileTree::build_file_tree(Path::new(&node.full_path.clone()), node).unwrap();
        node.is_open = false;
        temp.write("dir/new", b"");
        tree.refresh_file_tree(temp.root().to_str().unwrap())
            .unwrap();
        let node = &mut tree.root_node.children[0];
        assert!(!node.is_open);
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].name, "old");
        node.is_open = true;
        FileTree::build_file_tree(Path::new(&node.full_path.clone()), node).unwrap();
        assert_eq!(
            node.children
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "old"]
        );
        tree.refresh_file_tree(temp.root().to_str().unwrap())
            .unwrap();
        assert!(tree.root_node.children[0].is_open);
        assert_eq!(tree.root_node.children[0].children.len(), 2);
    }

    #[test]
    fn row_and_background_context_menus_have_padding_without_changing_tree_style() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton, WindowFlags};
        use std::path::PathBuf;

        #[derive(Clone, Copy)]
        struct Popup {
            pos: [f32; 2],
            padding: [f32; 2],
            content_inset: [f32; 2],
            content_height: f32,
        }
        fn frame(
            context: &mut Context,
            tree: &mut FileTree,
        ) -> (Vec<FileTreeAction>, Option<Popup>) {
            context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], 1.0 / 60.0));
            let ui = context.frame();
            let before = ui.clone_style();
            let mut result = Vec::new();
            let mut popup = None;
            ui.window("Compact Files")
                .position([0.0, 0.0], Condition::Always)
                .size([400.0, 300.0], Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
                .build(|| {
                    result = tree.display_actions(ui, "", &FileTreeStyle::default(), None, None);
                    assert_eq!(ui.clone_style().window_padding(), before.window_padding());
                    assert_eq!(ui.clone_style().item_spacing(), before.item_spacing());
                    ui.with_bound_context(|| unsafe {
                        let stack = &(*dear_imgui_rs::sys::igGetCurrentContext()).OpenPopupStack;
                        if stack.Size > 0 {
                            let window = (*stack.Data.add(stack.Size as usize - 1)).Window;
                            if !window.is_null() {
                                popup = Some(Popup {
                                    pos: [(*window).Pos.x, (*window).Pos.y],
                                    padding: [(*window).WindowPadding.x, (*window).WindowPadding.y],
                                    content_inset: [
                                        (*window).DC.CursorStartPos.x - (*window).Pos.x,
                                        (*window).DC.CursorStartPos.y - (*window).Pos.y,
                                    ],
                                    content_height: (*window).ContentSize.y,
                                });
                            }
                        }
                    });
                });
            drop(context.render_legacy());
            (result, popup)
        }

        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        // Exercise an item menu from a zero-padding content window and a
        // background menu from the Files panel's compact two-pixel padding.
        for (mouse, inherited_padding, expected_items) in
            [([60.0, 24.0], [0.0; 2], 13), ([240.0, 180.0], [2.0; 2], 7)]
        {
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            context.style_mut().set_window_padding(inherited_padding);
            context.style_mut().set_item_spacing([1.0, 2.0]);
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let mut tree = FileTree {
                root_node: FileNode {
                    name: "project".into(),
                    full_path: "/fixture".into(),
                    is_directory: true,
                    is_open: true,
                    children: vec![FileNode {
                        name: "file.rs".into(),
                        full_path: "/fixture/file.rs".into(),
                        ..FileNode::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            };
            frame(&mut context, &mut tree);
            frame(&mut context, &mut tree);
            context.io_mut().add_mouse_pos_event(mouse);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Right, true);
            frame(&mut context, &mut tree);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Right, false);
            frame(&mut context, &mut tree);
            let (_, popup) = frame(&mut context, &mut tree);
            let popup = popup.expect("right-click must open the actual native menu");
            if expected_items == 13 {
                assert_eq!(
                    tree.animations
                        .hosts
                        .values()
                        .next()
                        .unwrap()
                        .interaction
                        .selected,
                    BTreeSet::from(["/fixture/file.rs".into()]),
                    "opening the panel's context menu keeps its selection"
                );
            }
            assert_eq!(popup.padding, [8.0, 6.0]);
            assert_eq!(popup.content_inset, [8.0, 6.0]);
            // Verify four-pixel menu row spacing and separator gaps with
            // visibility controls in both row and background menus.
            let font_size = context
                .binding()
                .with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFontSize() });
            let separators = if expected_items == 7 { 1.0 } else { 2.0 };
            let expected_height = expected_items as f32 * font_size
                + (expected_items - 1) as f32 * 4.0
                + separators * 5.0;
            assert!(
                (popup.content_height - expected_height).abs() < 0.1,
                "expected {expected_items} menu items, native height {} vs {expected_height}",
                popup.content_height
            );
            assert_eq!(context.style().window_padding(), inherited_padding);
            assert_eq!(context.style().item_spacing(), [1.0, 2.0]);
            context
                .io_mut()
                .add_mouse_pos_event([popup.pos[0] + 20.0, popup.pos[1] + 12.0]);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            frame(&mut context, &mut tree);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            let (actions, _) = frame(&mut context, &mut tree);
            let expected = if expected_items == 7 {
                FileTreeAction::NewFile("/fixture".into())
            } else {
                FileTreeAction::OpenManyFromMenu(vec!["/fixture/file.rs".into()])
            };
            assert_eq!(actions, vec![expected]);
        }
    }
}
