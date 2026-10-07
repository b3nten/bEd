//! Translated from ned files/file_tree.{h,cpp}; see LICENSE and NOTICE.
use crate::util::tree_animation::TreeAnimation;
use bed_files::tree_ignore::TreeIgnore;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::{collections::BTreeSet, fs, io, path::Path};

/// Personal, project-scoped tree preferences. Paths use project-relative `/` components.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileTreePreferences {
    pub hide_gitignored: bool,
    pub hide_hidden: bool,
    pub hidden_paths: BTreeSet<String>,
}
impl FileTreePreferences {
    pub fn from_value(value: &Value) -> Self {
        Self {
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
        json!({"hide_gitignored":self.hide_gitignored,"hide_hidden":self.hide_hidden,
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
}
#[derive(Default)]
pub struct FileTreeAnimations {
    hosts: HashMap<u32, TreeHost>,
}
#[derive(Default)]
struct TreeHost {
    motion: TreeAnimation<String>,
    rows: HashMap<String, TreeRow>,
    root: String,
    preferences: FileTreePreferences,
    show_hidden: bool,
    last_frame: usize,
    #[cfg(test)]
    labels: HashMap<String, [f32; 2]>,
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
        context: bed_plugin::CommandContext,
    },
    Open(String),
    NewFile(String),
    NewFolder(String),
    Rename(String),
    Trash(String),
    SetHideGitignored(bool),
    SetHideHidden(bool),
    SetShowHidden(bool),
    SetPathHidden {
        path: String,
        hidden: bool,
    },
}
/// Host-owned extension menu. The path belongs to the popup's source row.
pub type ExtensionMenu<'a> = dyn Fn(&Ui, &str, bool, bool, &mut Vec<FileTreeAction>) + 'a;
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
            let _menu_style = crate::util::context_menu_style(ui);
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
                    ui.separator();
                    Self::visibility_menu(ui, &visibility, &mut actions);
                    if let Some(extensions) = extensions {
                        extensions(ui, &root, true, true, &mut actions);
                    }
                    dear_imgui_rs::sys::igEndPopup();
                }
            }
        }
        actions
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
        let desired: Vec<_> = rows.iter().map(|(key, _)| key.clone()).collect();
        let frame = ui.frame_count();
        let host_id =
            ui.with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetCurrentWindow()).ID });
        self.animations
            .hosts
            .retain(|_, host| frame <= host.last_frame.saturating_add(120));
        let host = self.animations.hosts.entry(host_id).or_default();
        let reset = host.root != root
            || host.preferences != self.preferences
            || host.show_hidden != self.show_hidden;
        if reset {
            host.rows.clear();
        }
        host.root = root.clone();
        host.preferences = self.preferences.clone();
        host.show_hidden = self.show_hidden;
        host.last_frame = frame;
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
                &mut self.error,
                &mut actions,
                extensions,
            );
            #[cfg(test)]
            host.labels
                .insert(node.full_path.clone(), ui.item_rect_min());
        }
        layout.finish(ui);
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
        error: &mut Option<String>,
        actions: &mut Vec<FileTreeAction>,
        extensions: Option<&ExtensionMenu<'_>>,
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
        let base_icon = font_size * if node.is_directory { 0.8 } else { 1.2 };
        let icon_size = base_icon * 1.1;
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
        let required_width = indent + row_pad_x + icon_size + icon_text_gap + text_size[0];
        let width = required_width.max(ui.content_region_avail()[0]);
        let clicked = {
            let _bg = ui.push_style_color(StyleColor::Button, [0.0; 4]);
            let text = style.text_color;
            let _hover =
                ui.push_style_color(StyleColor::ButtonHovered, [text[0], text[1], text[2], 0.13]);
            let _active =
                ui.push_style_color(StyleColor::ButtonActive, [text[0], text[1], text[2], 0.20]);
            let _border = ui.push_style_var(StyleVar::FrameBorderSize(0.0));
            let _pad = ui.push_style_var(StyleVar::FramePadding([0.0; 2]));
            ui.button_with_size(format!("##{}", node.full_path), [width, item_height])
        };
        if context_menu {
            let _menu_style = crate::util::context_menu_style(ui);
            if let Some(_menu) = ui.begin_popup_context_item() {
                let directory = if node.is_directory {
                    node.full_path.clone()
                } else if remote {
                    node.full_path
                        .rsplit_once('/')
                        .map(|(parent, _)| {
                            if parent.is_empty() {
                                "/".to_owned()
                            } else {
                                parent.to_owned()
                            }
                        })
                        .unwrap_or_default()
                } else {
                    Path::new(&node.full_path)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .to_string_lossy()
                        .into_owned()
                };
                if ui.menu_item("New File…") {
                    actions.push(FileTreeAction::NewFile(directory.clone()));
                }
                if ui.menu_item("New Folder…") {
                    actions.push(FileTreeAction::NewFolder(directory));
                }
                if !root {
                    ui.separator();
                    if ui.menu_item("Rename…") {
                        actions.push(FileTreeAction::Rename(node.full_path.clone()));
                    }
                    if ui.menu_item(if remote { "Delete…" } else { "Move to Trash" }) {
                        actions.push(FileTreeAction::Trash(node.full_path.clone()));
                    }
                    let manually_hidden = relative_path(visibility.root, &node.full_path, remote)
                        .is_some_and(|path| visibility.preferences.hidden_paths.contains(&path));
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
                ui.separator();
                Self::visibility_menu(ui, visibility, actions);
                if let Some(extensions) = extensions {
                    extensions(ui, &node.full_path, node.is_directory, false, actions);
                }
            }
        }
        // Dim labels and icons only; keep context-menu text fully legible.
        let _dim =
            hidden.then(|| ui.push_style_var(StyleVar::Alpha(ui.clone_style().alpha() * 0.45)));
        let center_y = row_origin[1] + item_height * 0.5;
        let icon_x = row_origin[0] + indent + row_pad_x;
        let text_x = icon_x + icon_size + icon_text_gap;
        ui.set_cursor_pos([icon_x, center_y - icon_size * 0.5]);
        if let Some(icon) = icon {
            let tint = if node.is_directory
                || icons.is_some_and(|icons| Some(icon) == icons.get("default"))
            {
                ui.style_color(StyleColor::Text)
            } else {
                [1.0; 4]
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
        let color =
            bed_core::util::color::ensure_contrast(color, ui.style_color(StyleColor::ChildBg), 4.5);
        {
            let _text = ui.push_style_color(StyleColor::Text, color);
            ui.text(&node.name);
        }
        drop(_dim);
        if node.is_directory {
            if clicked {
                node.is_open = !node.is_open;
                if node.is_open && !remote {
                    let path = node.full_path.clone();
                    if let Err(failure) =
                        Self::build_filtered(Path::new(&path), node, visibility, error)
                    {
                        *error = Some(failure.to_string());
                    }
                }
            }
        } else if clicked {
            actions.push(FileTreeAction::Open(node.full_path.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::test_support::TempDir;
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
        let branch_click = [branch[0] + 2.0, branch[1] + font_size * 0.5];
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
            [([60.0, 24.0], [0.0; 2], 8), ([240.0, 180.0], [2.0; 2], 5)]
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
            assert_eq!(popup.padding, [8.0, 6.0]);
            assert_eq!(popup.content_inset, [8.0, 6.0]);
            // Verify four-pixel menu row spacing and separator gaps with
            // visibility controls in both row and background menus.
            let font_size = context
                .binding()
                .with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFontSize() });
            let expected_height = if expected_items == 5 {
                5.0 * font_size + 16.0 + 5.0
            } else {
                8.0 * font_size + 28.0 + 10.0
            };
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
            assert_eq!(actions, vec![FileTreeAction::NewFile("/fixture".into())]);
        }
    }
}
