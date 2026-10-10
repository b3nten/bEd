//! Local and SSH project picker with recent workspaces.
use bed_scenes::{SceneFrame, SceneKind, SceneView};
use bed_ui::util::popup_style::tooltip_text;
use bed_workbench_api::{
    HostContext,
    gpu::Canvas,
    workspace::{WorkspaceSpec, WorkspaceTarget},
};
use dear_imgui_rs::{ChildFlags, StyleColor, StyleVar, Ui, WindowFlags};
use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct WelcomeAction {
    pub open_folder: bool,
    pub project: Option<PathBuf>,
    pub remove_recent: Option<PathBuf>,
    pub workspace: Option<WorkspaceSpec>,
    pub workspace_window: Option<WorkspaceSpec>,
    pub connect: Option<WorkspaceSpec>,
    pub remove_workspace: Option<WorkspaceSpec>,
    pub rename_workspace: Option<(WorkspaceSpec, String)>,
    pub error: Option<String>,
}
pub struct Welcome {
    pub(crate) scene: SceneView,
    recent_workspaces: Vec<WorkspaceSpec>,
    workspace_actions: bool,
    ssh_host: String,
    ssh_root: String,
    ssh_error: Option<String>,
    rename: Option<WorkspaceSpec>,
    rename_name: String,
    #[cfg(test)]
    drawn_items: Vec<DrawnItem>,
}
impl Default for Welcome {
    fn default() -> Self {
        Self {
            scene: SceneView::new(SceneKind::WelcomeBed),
            recent_workspaces: Vec::new(),
            workspace_actions: false,
            ssh_host: String::new(),
            ssh_root: String::new(),
            ssh_error: None,
            rename: None,
            rename_name: String::new(),
            #[cfg(test)]
            drawn_items: Vec::new(),
        }
    }
}
impl Welcome {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_recent_projects(&mut self, projects: impl IntoIterator<Item = PathBuf>) {
        self.workspace_actions = false;
        self.recent_workspaces = projects
            .into_iter()
            .filter_map(|path| path.to_str().map(WorkspaceSpec::local))
            .collect();
    }
    pub fn set_recent_workspaces(&mut self, workspaces: impl IntoIterator<Item = WorkspaceSpec>) {
        self.workspace_actions = true;
        self.recent_workspaces = workspaces.into_iter().collect();
    }
    fn ssh_workspace(&self) -> Result<WorkspaceSpec, String> {
        let host = self.ssh_host.trim();
        if host.is_empty()
            || host.starts_with('-')
            || host.contains('\0')
            || host.chars().any(char::is_whitespace)
        {
            return Err("Enter an SSH host or SSH configuration alias.".into());
        }
        if !(self.ssh_root.starts_with('/')
            || self.ssh_root == "~"
            || self.ssh_root.starts_with("~/"))
            || self.ssh_root.contains('\0')
        {
            return Err(
                "Enter an absolute project path or a path beginning with ~/ on the SSH host."
                    .into(),
            );
        }
        Ok(WorkspaceSpec {
            name: String::new(),
            root: self.ssh_root.clone(),
            target: WorkspaceTarget::Ssh { host: host.into() },
        })
    }
    /// Draw inside a caller-owned dockable Projects tab.
    pub fn draw_body(&mut self, ui: &Ui, host: &HostContext<'_>) -> WelcomeAction {
        // Use the same theme accent as the Bed panel, before the controls scope
        // adjusts its colors for button and checkbox contrast.
        let scene_accent = ui.style_color(StyleColor::CheckMark);
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let mut action = WelcomeAction::default();
        let fs = ui.current_font_size();
        let available = ui.content_region_avail();
        let outer_padding = (fs * 1.5).min(available[0] * 0.08);
        let content_width = (fs * 36.0).min(available[0] - outer_padding * 2.0).max(1.0);
        let origin = ui.cursor_pos();
        ui.set_cursor_pos([
            origin[0] + (available[0] - content_width).max(0.0) * 0.5,
            origin[1] + (available[1] * 0.07).min(fs * 2.0),
        ]);
        #[cfg(test)]
        self.drawn_items.clear();
        let mut begin_rename = false;
        let mut begin_remote = false;
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0, fs * 0.5]));
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.7, fs * 0.45]));
        let _frame = ui.push_style_var(StyleVar::FramePadding([fs * 0.8, fs * 0.5]));
        ui.child_window("##projects_content")
            .size([content_width, 0.0])
            .child_flags(ChildFlags::ALWAYS_USE_WINDOW_PADDING)
            .flags(WindowFlags::NO_BACKGROUND)
            .build(ui, || {
                let width = ui.content_region_avail()[0];
                #[cfg(test)]
                {
                    let min = ui.cursor_screen_pos();
                    self.drawn_items.push(DrawnItem {
                        name: "content",
                        min,
                        max: [min[0] + width, min[1] + ui.content_region_avail()[1]],
                    });
                }
                let scene_width = (fs * 18.0).min(width);
                let scene_height = (scene_width * 0.5).min(fs * 9.0);
                ui.set_cursor_pos([
                    ui.cursor_pos()[0] + (width - scene_width) * 0.5,
                    ui.cursor_pos()[1],
                ]);
                let _scene_padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
                ui.child_window("##welcome_scene")
                    .size([scene_width, scene_height])
                    .flags(
                        WindowFlags::NO_SCROLLBAR
                            | WindowFlags::NO_SCROLL_WITH_MOUSE
                            | WindowFlags::NO_BACKGROUND,
                    )
                    .build(ui, || {
                        let canvas = Canvas::show(ui, host, self.scene.handle(), "##bed");
                        self.scene.update(
                            canvas.pixels,
                            SceneFrame {
                                background: [0.0; 4],
                                accent: scene_accent,
                                animations: host.animations,
                                ..Default::default()
                            },
                        );
                    });
                drop(_scene_padding);
                #[cfg(test)]
                record_item(&mut self.drawn_items, ui, "scene");
                ui.dummy([0.0, fs * 0.6]);
                {
                    let _title = ui.push_font_with_size(None, fs * 1.8);
                    let text = "Welcome to bEd";
                    ui.set_cursor_pos([
                        ((width - ui.calc_text_size(text)[0]) * 0.5).max(0.0),
                        ui.cursor_pos()[1],
                    ]);
                    ui.text(text);
                }
                ui.set_cursor_pos([
                    ((width - ui.calc_text_size("Not for napping.")[0]) * 0.5).max(0.0),
                    ui.cursor_pos()[1],
                ]);
                ui.text_disabled("Not for napping.");
                ui.dummy([0.0, fs * 1.2]);
                // Stack only when the two labels would no longer fit comfortably.
                let side_by_side = width >= fs * 26.0;
                let button_width = if side_by_side {
                    (width - fs * 0.7) * 0.5
                } else {
                    width
                };
                {
                    let _accent =
                        ui.push_style_color(StyleColor::Button, ui.style_color(StyleColor::Header));
                    if ui.button_with_size("Open local folder", [button_width, fs * 2.9]) {
                        action.open_folder = true;
                    }
                }
                #[cfg(test)]
                record_item(&mut self.drawn_items, ui, "open_folder");
                if side_by_side {
                    ui.same_line();
                }
                if ui.button_with_size("Open remote folder", [button_width, fs * 2.9]) {
                    begin_remote = true;
                    self.ssh_error = None;
                }
                #[cfg(test)]
                record_item(&mut self.drawn_items, ui, "open_remote");
                ui.dummy([0.0, fs * 1.5]);
                ui.text("Recent workspaces");
                ui.spacing();
                ui.separator();
                ui.dummy([0.0, fs * 0.3]);
                if self.recent_workspaces.is_empty() {
                    ui.text_disabled("Your next project starts here.");
                    ui.text_disabled("Open a folder to get settled in.");
                }
                let _recent_spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, fs * 0.45]));
                for spec in &self.recent_workspaces {
                    let identity = spec.identity();
                    let _id = ui.push_id(identity.as_str());
                    if ui
                        .selectable_config("##workspace")
                        .size([width, fs * 3.6])
                        .build()
                    {
                        action = open_recent(spec, self.workspace_actions);
                    }
                    #[cfg(test)]
                    record_item(&mut self.drawn_items, ui, "recent_workspace");
                    let min = ui.item_rect_min();
                    let max = ui.item_rect_max();
                    let location = spec.location();
                    if ui.is_item_hovered() {
                        tooltip_text(ui, &location);
                    }
                    {
                        let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
                        if let Some(_menu) = ui.begin_popup_context_item() {
                            if ui.menu_item("Open") {
                                action = open_recent(spec, self.workspace_actions);
                            }
                            #[cfg(test)]
                            record_item(&mut self.drawn_items, ui, "menu_open");
                            if ui.menu_item("Open in new window") {
                                action.workspace_window = Some(spec.clone());
                            }
                            #[cfg(test)]
                            record_item(&mut self.drawn_items, ui, "menu_window");
                            if self.workspace_actions {
                                ui.separator();
                                if ui.menu_item("Rename…") {
                                    self.rename = Some(spec.clone());
                                    self.rename_name = spec.name.clone();
                                    begin_rename = true;
                                }
                                #[cfg(test)]
                                record_item(&mut self.drawn_items, ui, "menu_rename");
                            }
                            ui.separator();
                            if ui.menu_item("Delete") {
                                if self.workspace_actions {
                                    action.remove_workspace = Some(spec.clone());
                                } else {
                                    action.remove_recent = Some(PathBuf::from(&spec.root));
                                }
                            }
                            #[cfg(test)]
                            record_item(&mut self.drawn_items, ui, "menu_delete");
                            if ui.is_item_hovered() {
                                tooltip_text(
                                    ui,
                                    "Remove this saved workspace. Files stay on disk.",
                                );
                            }
                        }
                    }
                    // Both lines belong to the same selectable. Clip long names
                    // and locations to the row without introducing extra widgets.
                    let _clip = ui.push_clip_rect(min, max, true);
                    let draw = ui.get_window_draw_list();
                    draw.add_text(
                        [min[0] + fs * 0.7, min[1] + fs * 0.55],
                        ui.style_color(StyleColor::Text),
                        &spec.name,
                    );
                    draw.add_text(
                        [min[0] + fs * 0.7, min[1] + fs * 1.9],
                        ui.style_color(StyleColor::TextDisabled),
                        &location,
                    );
                }
            });
        #[cfg(test)]
        record_item(&mut self.drawn_items, ui, "column");
        drop(_frame);
        drop(_spacing);
        drop(_padding);
        if begin_remote {
            ui.open_popup("Open remote folder");
        }
        if begin_rename {
            ui.open_popup("Rename Workspace");
        }
        let _dialog = bed_ui::util::popup_style::dialog_style(ui);
        if let Some(_popup) = ui
            .begin_modal_popup_config("Open remote folder")
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            let dialog_width = (fs * 29.0)
                .min(ui.io().display_size()[0] - fs * 4.0)
                .max(fs * 12.0);
            ui.text_disabled("Connect to a workspace over SSH.");
            ui.dummy([0.0, fs * 0.4]);
            ui.text("SSH host");
            ui.set_next_item_width(dialog_width);
            if begin_remote {
                ui.set_keyboard_focus_here();
            }
            ui.input_text("##ssh_host", &mut self.ssh_host)
                .hint("user@host or SSH alias")
                .build();
            #[cfg(test)]
            record_item(&mut self.drawn_items, ui, "host_input");
            ui.spacing();
            ui.text("Folder path");
            ui.set_next_item_width(dialog_width);
            let submit = ui
                .input_text("##ssh_root", &mut self.ssh_root)
                .hint("~/Dev/project or /srv/project")
                .enter_returns_true(true)
                .build();
            #[cfg(test)]
            record_item(&mut self.drawn_items, ui, "path_input");
            if let Some(error) = &self.ssh_error {
                ui.text_wrapped(error);
            }
            ui.dummy([0.0, fs * 0.6]);
            let connect = ui.button_with_size("Connect", [fs * 7.0, 0.0]);
            #[cfg(test)]
            record_item(&mut self.drawn_items, ui, "connect");
            if connect || submit {
                match self.ssh_workspace() {
                    Ok(spec) => {
                        action.connect = Some(spec);
                        ui.close_current_popup();
                    }
                    Err(error) => self.ssh_error = Some(error),
                }
            }
            ui.same_line();
            if ui.button("Cancel") || ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
                ui.close_current_popup();
            }
        }
        if let Some(_popup) = ui
            .begin_modal_popup_config("Rename Workspace")
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            ui.text("Workspace name");
            ui.set_next_item_width(fs * 24.0);
            if begin_rename {
                ui.set_keyboard_focus_here();
            }
            let submit = ui
                .input_text("##name", &mut self.rename_name)
                .auto_select_all(true)
                .enter_returns_true(true)
                .build();
            let rename = ui.button("Rename");
            #[cfg(test)]
            record_item(&mut self.drawn_items, ui, "rename_submit");
            if rename || submit {
                if self.rename_name.trim().is_empty() {
                    action.error = Some("Workspace name cannot be empty".into());
                } else {
                    action.rename_workspace = self
                        .rename
                        .take()
                        .map(|spec| (spec, self.rename_name.trim().to_owned()));
                    ui.close_current_popup();
                }
            }
            ui.same_line();
            if ui.button("Cancel") || ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
                self.rename = None;
                ui.close_current_popup();
            }
        }
        action
    }
}

fn open_recent(spec: &WorkspaceSpec, workspace_actions: bool) -> WelcomeAction {
    if matches!(spec.target, WorkspaceTarget::Local) && !PathBuf::from(&spec.root).is_dir() {
        WelcomeAction {
            error: Some(format!("Project folder is unavailable: {}", spec.root)),
            ..Default::default()
        }
    } else if workspace_actions {
        WelcomeAction {
            workspace: Some(spec.clone()),
            ..Default::default()
        }
    } else {
        WelcomeAction {
            project: Some(PathBuf::from(&spec.root)),
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct DrawnItem {
    name: &'static str,
    min: [f32; 2],
    max: [f32; 2],
}
#[cfg(test)]
fn record_item(items: &mut Vec<DrawnItem>, ui: &Ui, name: &'static str) {
    items.push(DrawnItem {
        name,
        min: ui.item_rect_min(),
        max: ui.item_rect_max(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench_api::{HostContext, ModulePanel};
    use serde_json::Value;
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    #[test]
    fn projects_panel_keeps_the_picker_centered_and_controls_clickable() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let state = Rc::new(RefCell::new(crate::ProjectsState::default()));
        let mut panel = crate::ProjectsPanel {
            state: state.clone(),
            welcome: Welcome::new(),
        };
        let textures = HashMap::new();
        let host = HostContext {
            remote: false,
            documents: &[],
            active_document: None,
            settings: &Value::Null,
            textures: &textures,
            animations: false,
            workspace: 0,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        let draw = |context: &mut Context, panel: &mut crate::ProjectsPanel, size: [f32; 2]| {
            context.prepare_frame(FramePrepareOptions::new(
                [size[0] + 80.0, size[1] + 80.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            let font = ui.push_font_with_size(None, 20.0);
            let mut bounds = ([0.0; 2], [0.0; 2]);
            ui.window("Projects panel")
                .position([40.0, 20.0], Condition::Always)
                .size(size, Condition::Always)
                .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
                .build(|| {
                    let min = ui.cursor_screen_pos();
                    let available = ui.content_region_avail();
                    bounds = (min, [min[0] + available[0], min[1] + available[1]]);
                    panel.draw(ui, &host, &mut Vec::new());
                });
            drop(font);
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
            bounds
        };
        for size in [[1200.0, 800.0], [600.0, 800.0]] {
            draw(&mut context, &mut panel, size);
            let (min, max) = draw(&mut context, &mut panel, size);
            let column = item(&panel.welcome, "column");
            assert!(
                ((column.min[0] + column.max[0]) - (min[0] + max[0])).abs() < 3.0,
                "picker remains centered at every width"
            );
            let open = item(&panel.welcome, "open_folder");
            assert!(
                open.max[0] - open.min[0] > 200.0,
                "controls remain comfortably wide"
            );
            context.io_mut().add_mouse_pos_event([
                (open.min[0] + open.max[0]) * 0.5,
                (open.min[1] + open.max[1]) * 0.5,
            ]);
            draw(&mut context, &mut panel, size);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            draw(&mut context, &mut panel, size);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            draw(&mut context, &mut panel, size);
            assert!(matches!(
                state.borrow_mut().take_actions().as_slice(),
                [crate::ProjectsAction::OpenFolder]
            ));
        }
    }
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton, WindowFlags};

    #[test]
    fn ssh_form_preserves_absolute_and_home_paths_and_defers_workspace_name() {
        for root in [
            "/srv/project with spaces",
            "~",
            "~/Dev/project",
            "~/literal $HOME; $(project)/*",
        ] {
            let welcome = Welcome {
                ssh_host: "  project-host  ".into(),
                ssh_root: root.into(),
                ..Welcome::default()
            };
            let spec = welcome.ssh_workspace().unwrap();
            assert_eq!(spec.root, root);
            assert!(
                spec.name.is_empty(),
                "name comes from the canonical remote root"
            );
            assert_eq!(
                spec.target,
                WorkspaceTarget::Ssh {
                    host: "project-host".into(),
                }
            );
        }
    }
    #[test]
    fn ssh_form_rejects_invalid_hosts_relative_paths_and_other_users_homes() {
        let mut welcome = Welcome::default();
        assert!(welcome.ssh_workspace().is_err());
        welcome.ssh_host = "host".into();
        for root in [
            "",
            "relative",
            "~otheruser",
            "~otheruser/project",
            "~/invalid\0path",
            "/invalid\0path",
        ] {
            welcome.ssh_root = root.into();
            assert!(welcome.ssh_workspace().is_err(), "must reject {root:?}");
        }
        welcome.ssh_root = "~/Dev/project".into();
        for host in ["", "-oProxyCommand=bad", "host alias", "host\0alias"] {
            welcome.ssh_host = host.into();
            assert!(welcome.ssh_workspace().is_err(), "must reject {host:?}");
        }
    }

    struct Frame {
        action: WelcomeAction,
        min: [f32; 2],
        max: [f32; 2],
        font: f32,
    }
    fn frame(context: &mut Context, welcome: &mut Welcome, size: [f32; 2]) -> Frame {
        context.prepare_frame(FramePrepareOptions::new(
            [size[0] + 80.0, size[1] + 80.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        let mut result = Frame {
            action: WelcomeAction::default(),
            min: [0.0; 2],
            max: [0.0; 2],
            font: ui.current_font_size(),
        };
        ui.window("Projects layout")
            .position([40.0, 20.0], Condition::Always)
            .size(size, Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
            .build(|| {
                result.min = ui.cursor_screen_pos();
                let available = ui.content_region_avail();
                result.max = [result.min[0] + available[0], result.min[1] + available[1]];
                let textures = HashMap::new();
                let host = HostContext {
                    remote: false,
                    documents: &[],
                    active_document: None,
                    settings: &Value::Null,
                    textures: &textures,
                    animations: false,
                    workspace: 0,
                    diagnostics: &Value::Null,
                    viewer_menu: None,
                    default_viewers: &Value::Null,
                };
                result.action = welcome.draw_body(ui, &host);
            });
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        result
    }
    fn item(welcome: &Welcome, name: &str) -> DrawnItem {
        *welcome
            .drawn_items
            .iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("native item {name} must be drawn"))
    }
    fn click(
        context: &mut Context,
        welcome: &mut Welcome,
        size: [f32; 2],
        name: &str,
        button: MouseButton,
    ) -> WelcomeAction {
        let target = item(welcome, name);
        context.io_mut().add_mouse_pos_event([
            (target.min[0] + target.max[0]) * 0.5,
            (target.min[1] + target.max[1]) * 0.5,
        ]);
        frame(context, welcome, size);
        context.io_mut().add_mouse_button_event(button, true);
        frame(context, welcome, size);
        context.io_mut().add_mouse_button_event(button, false);
        frame(context, welcome, size).action
    }
    #[test]
    fn welcome_buttons_remote_dialog_and_recent_menu_work_at_wide_and_narrow_sizes() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        for size in [[1100.0, 650.0], [300.0, 650.0]] {
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            context.style_mut().set_window_padding([9.0, 11.0]);
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let mut welcome = Welcome {
                ssh_host: String::new(),
                ssh_root: "~/Dev/project".into(),
                ..Welcome::default()
            };
            let recent = WorkspaceSpec {
                name: "Recent project".into(),
                root: "/srv/a project".into(),
                target: WorkspaceTarget::Ssh {
                    host: "project-host".into(),
                },
            };
            welcome.set_recent_workspaces([recent.clone()]);
            frame(&mut context, &mut welcome, size);
            let layout = frame(&mut context, &mut welcome, size);
            let column = item(&welcome, "column");
            let content = item(&welcome, "content");
            assert!(column.min[0] >= layout.min[0] && column.max[0] <= layout.max[0]);
            assert!(column.max[0] - column.min[0] <= layout.font * 36.0 + 1.0);
            assert!(
                ((column.min[0] + column.max[0]) - (layout.min[0] + layout.max[0])).abs() <= 2.0
            );
            assert!(
                !welcome
                    .drawn_items
                    .iter()
                    .any(|item| item.name.ends_with("_input"))
            );
            let scene = item(&welcome, "scene");
            assert!(scene.max[1] <= item(&welcome, "open_folder").min[1]);
            for name in ["open_folder", "open_remote", "recent_workspace"] {
                let drawn = item(&welcome, name);
                assert!(
                    drawn.min[0] >= content.min[0] - 1.0 && drawn.max[0] <= content.max[0] + 1.0,
                    "{name}: {drawn:?} outside {content:?}"
                );
                assert!(drawn.min[1] >= column.min[1] && drawn.max[1] <= column.max[1]);
            }
            assert!(
                click(
                    &mut context,
                    &mut welcome,
                    size,
                    "open_folder",
                    MouseButton::Left
                )
                .open_folder
            );
            click(
                &mut context,
                &mut welcome,
                size,
                "open_remote",
                MouseButton::Left,
            );
            frame(&mut context, &mut welcome, size);
            let host = item(&welcome, "host_input");
            let path = item(&welcome, "path_input");
            assert!(host.max[1] < path.min[1]);
            let invalid = click(
                &mut context,
                &mut welcome,
                size,
                "connect",
                MouseButton::Left,
            );
            assert!(invalid.connect.is_none());
            assert!(welcome.ssh_error.is_some());
            welcome.ssh_host = "project-host".into();
            frame(&mut context, &mut welcome, size);
            let action = click(
                &mut context,
                &mut welcome,
                size,
                "connect",
                MouseButton::Left,
            );
            let connected = action.connect.expect("Connect submits the remote dialog");
            assert_eq!(connected.root, "~/Dev/project");
            assert!(connected.name.is_empty());
            frame(&mut context, &mut welcome, size);
            assert_eq!(
                click(
                    &mut context,
                    &mut welcome,
                    size,
                    "recent_workspace",
                    MouseButton::Left
                )
                .workspace,
                Some(recent.clone())
            );
            for menu in ["menu_open", "menu_window", "menu_rename", "menu_delete"] {
                click(
                    &mut context,
                    &mut welcome,
                    size,
                    "recent_workspace",
                    MouseButton::Right,
                );
                frame(&mut context, &mut welcome, size);
                let action = click(&mut context, &mut welcome, size, menu, MouseButton::Left);
                match menu {
                    "menu_open" => assert_eq!(action.workspace, Some(recent.clone())),
                    "menu_window" => assert_eq!(action.workspace_window, Some(recent.clone())),
                    "menu_delete" => assert_eq!(action.remove_workspace, Some(recent.clone())),
                    "menu_rename" => {
                        assert_eq!(welcome.rename, Some(recent.clone()));
                        frame(&mut context, &mut welcome, size);
                        for character in "New label".chars() {
                            context.io_mut().add_input_character(character);
                        }
                        frame(&mut context, &mut welcome, size);
                        let renamed = click(
                            &mut context,
                            &mut welcome,
                            size,
                            "rename_submit",
                            MouseButton::Left,
                        );
                        assert_eq!(
                            renamed.rename_workspace,
                            Some((recent.clone(), "New label".into()))
                        );
                    }
                    _ => unreachable!(),
                }
                frame(&mut context, &mut welcome, size);
            }
            assert_eq!(context.style().window_padding(), [9.0, 11.0]);
        }
    }
}
