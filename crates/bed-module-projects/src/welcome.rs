//! Local and SSH project picker with recent workspaces.
use bed_workbench_api::workspace::{WorkspaceSpec, WorkspaceTarget};
use dear_imgui_rs::{ChildFlags, StyleVar, TreeNodeFlags, Ui};
use std::path::PathBuf;

#[derive(Default, Debug)]
pub struct WelcomeAction {
    pub open_folder: bool,
    pub project: Option<PathBuf>,
    pub remove_recent: Option<PathBuf>,
    pub workspace: Option<WorkspaceSpec>,
    pub connect: Option<WorkspaceSpec>,
    pub remove_workspace: Option<WorkspaceSpec>,
    pub rename_workspace: Option<(WorkspaceSpec, String)>,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct Welcome {
    recent_workspaces: Vec<WorkspaceSpec>,
    workspace_actions: bool,
    ssh_host: String,
    ssh_root: String,
    rename: Option<WorkspaceSpec>,
    rename_name: String,
    #[cfg(test)]
    drawn_items: Vec<DrawnItem>,
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
    pub fn draw_body(&mut self, ui: &Ui) -> WelcomeAction {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let mut action = WelcomeAction::default();
        let fs = ui.current_font_size();
        let available = ui.content_region_avail();
        let outer_padding = (fs * 1.25).min(available[0] * 0.08);
        let content_width = (fs * 36.0).min(available[0] - outer_padding * 2.0).max(1.0);
        let origin = ui.cursor_pos();
        ui.set_cursor_pos([
            origin[0] + (available[0] - content_width).max(0.0) * 0.5,
            origin[1] + (fs * 1.25).min(available[1].max(0.0) * 0.08),
        ]);
        #[cfg(test)]
        self.drawn_items.clear();
        let mut begin_rename = false;
        // Horizontal padding is supplied by the centered column. Keeping this
        // child at zero horizontal padding also prevents native framed headers
        // from extending beyond the input and button edges.
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0, fs * 0.75]));
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.6, fs * 0.4]));
        let _frame = ui.push_style_var(StyleVar::FramePadding([fs * 0.6, fs * 0.35]));
        ui.child_window("##projects_content")
            .size([content_width, 0.0])
            .child_flags(ChildFlags::ALWAYS_USE_WINDOW_PADDING)
            .build(ui, || {
                #[cfg(test)]
                {
                    let min = ui.cursor_screen_pos();
                    let available = ui.content_region_avail();
                    self.drawn_items.push(DrawnItem {
                        name: "content",
                        min,
                        max: [min[0] + available[0], min[1] + available[1]],
                    });
                }
                ui.text("Projects");
                ui.spacing();
                if ui.button_with_size(
                    "Open Folder…",
                    [ui.content_region_avail()[0], ui.frame_height()],
                ) {
                    action.open_folder = true;
                }
                #[cfg(test)]
                record_item(&mut self.drawn_items, ui, "open_folder");
                ui.dummy([0.0, fs * 0.5]);
                let expanded =
                    ui.collapsing_header("Connect over SSH", TreeNodeFlags::DEFAULT_OPEN);
                #[cfg(test)]
                record_item(&mut self.drawn_items, ui, "ssh_header");
                if expanded {
                    ui.spacing();
                    ui.text("SSH host");
                    ui.set_next_item_width(ui.content_region_avail()[0]);
                    ui.input_text("##ssh_host", &mut self.ssh_host)
                        .hint("user@host or SSH alias")
                        .build();
                    #[cfg(test)]
                    record_item(&mut self.drawn_items, ui, "host_input");
                    ui.spacing();
                    ui.text("Project path");
                    ui.set_next_item_width(ui.content_region_avail()[0]);
                    ui.input_text("##ssh_root", &mut self.ssh_root)
                        .hint("~/Dev/project or /srv/project")
                        .build();
                    #[cfg(test)]
                    record_item(&mut self.drawn_items, ui, "path_input");
                    ui.spacing();
                    if ui.button_with_size(
                        "Connect",
                        [ui.content_region_avail()[0], ui.frame_height()],
                    ) {
                        match self.ssh_workspace() {
                            Ok(spec) => action.connect = Some(spec),
                            Err(error) => action.error = Some(error),
                        }
                    }
                    #[cfg(test)]
                    record_item(&mut self.drawn_items, ui, "connect");
                }
                ui.dummy([0.0, fs * 0.75]);
                ui.text_disabled("Recent workspaces");
                ui.spacing();
                if self.recent_workspaces.is_empty() {
                    ui.text_disabled("No recent workspaces");
                }
                let _recent_spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0, fs * 0.4]));
                for spec in &self.recent_workspaces {
                    let identity = spec.identity();
                    let _id = ui.push_id(identity.as_str());
                    let row = ui.cursor_pos();
                    if ui
                        .selectable_config(&spec.name)
                        .size([ui.content_region_avail()[0], fs * 2.2])
                        .build()
                    {
                        if matches!(spec.target, WorkspaceTarget::Local)
                            && !PathBuf::from(&spec.root).is_dir()
                        {
                            action.error =
                                Some(format!("Project folder is unavailable: {}", spec.root));
                        } else if self.workspace_actions {
                            action.workspace = Some(spec.clone());
                        } else {
                            action.project = Some(PathBuf::from(&spec.root));
                        }
                    }
                    #[cfg(test)]
                    record_item(&mut self.drawn_items, ui, "recent_workspace");
                    let next_row = ui.cursor_pos();
                    {
                        let _menu_style = bed_ui::util::popup_style::context_menu_style(ui);
                        if let Some(_menu) = ui.begin_popup_context_item() {
                            if self.workspace_actions && ui.menu_item("Rename Workspace…") {
                                self.rename = Some(spec.clone());
                                self.rename_name = spec.name.clone();
                                begin_rename = true;
                            }
                            if ui.menu_item("Remove from Recent Workspaces") {
                                if self.workspace_actions {
                                    action.remove_workspace = Some(spec.clone());
                                } else {
                                    action.remove_recent = Some(PathBuf::from(&spec.root));
                                }
                            }
                        }
                    }
                    ui.set_cursor_pos([row[0], row[1] + fs * 1.2]);
                    let location = spec.location();
                    ui.text_disabled(&location);
                    if ui.is_item_hovered() {
                        let _tooltip_padding =
                            ui.push_style_var(StyleVar::WindowPadding([fs * 0.6, fs * 0.4]));
                        ui.tooltip_text(&location);
                    }
                    ui.set_cursor_pos(next_row);
                }
                // Register the restored cursor after painting each row's second
                // line; ImGui requires an item after a forward cursor move.
                if !self.recent_workspaces.is_empty() {
                    ui.dummy([0.0; 2]);
                }
            });
        #[cfg(test)]
        record_item(&mut self.drawn_items, ui, "column");
        drop(_frame);
        drop(_spacing);
        drop(_padding);
        if begin_rename {
            ui.open_popup("Rename Workspace");
        }
        let _dialog = bed_ui::util::popup_style::dialog_style(ui);
        if let Some(_popup) = ui.begin_modal_popup("Rename Workspace") {
            ui.input_text("Name", &mut self.rename_name).build();
            if ui.button("Rename") {
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
            if ui.button("Cancel") {
                self.rename = None;
                ui.close_current_popup();
            }
        }
        action
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
                result.action = welcome.draw_body(ui);
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
    #[test]
    fn native_projects_column_bounds_two_fields_and_clickable_recent_rows() {
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
                ssh_host: "project-host".into(),
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
            assert!(column.min[0] >= layout.min[0]);
            assert!(column.max[0] <= layout.max[0]);
            assert!(column.max[0] - column.min[0] <= layout.font * 36.0 + 1.0);
            assert!(
                ((column.min[0] + column.max[0]) - (layout.min[0] + layout.max[0])).abs() <= 2.0,
                "native column must remain centered: {column:?}"
            );
            assert_eq!(
                welcome
                    .drawn_items
                    .iter()
                    .filter(|item| item.name.ends_with("_input"))
                    .count(),
                2
            );
            for name in [
                "open_folder",
                "ssh_header",
                "host_input",
                "path_input",
                "connect",
                "recent_workspace",
            ] {
                let drawn = item(&welcome, name);
                assert!(
                    (drawn.min[0] - content.min[0]).abs() <= 1.0,
                    "left edge of {drawn:?}"
                );
                assert!(
                    (drawn.max[0] - content.max[0]).abs() <= 1.0,
                    "right edge of {drawn:?}"
                );
                assert!(drawn.min[1] >= column.min[1] && drawn.max[1] <= column.max[1]);
            }
            let host = item(&welcome, "host_input");
            let path = item(&welcome, "path_input");
            let connect = item(&welcome, "connect");
            assert!(host.max[1] - host.min[1] >= layout.font * 1.5);
            assert!(((connect.max[1] - connect.min[1]) - (host.max[1] - host.min[1])).abs() <= 1.0);
            assert!(host.max[1] < path.min[1] && path.max[1] < connect.min[1]);
            let point = [
                (connect.min[0] + connect.max[0]) * 0.5,
                (connect.min[1] + connect.max[1]) * 0.5,
            ];
            context.io_mut().add_mouse_pos_event(point);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            frame(&mut context, &mut welcome, size);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            let action = frame(&mut context, &mut welcome, size).action;
            let connected = action
                .connect
                .expect("native Connect button must submit the two-field form");
            assert_eq!(connected.root, "~/Dev/project");
            assert!(connected.name.is_empty());
            let row = item(&welcome, "recent_workspace");
            assert!(
                row.max[1] - row.min[1] < layout.font * 2.8,
                "recent rows stay compact"
            );
            // Click the location line as well as the name's selectable region.
            context
                .io_mut()
                .add_mouse_pos_event([row.min[0] + 20.0, row.max[1] - 6.0]);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            frame(&mut context, &mut welcome, size);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            assert_eq!(
                frame(&mut context, &mut welcome, size).action.workspace,
                Some(recent)
            );
            assert_eq!(context.style().window_padding(), [9.0, 11.0]);
        }
    }
}
