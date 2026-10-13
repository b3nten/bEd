//! Translated from ned lsp/lsp_dashboard.{h,cpp}; see LICENSE and NOTICE.
use crate::presentation::LspPresentationOptions;
use bed_lsp::{
    lsp_client::{LspClient, WorkDoneProgress},
    lsp_config::resolve_server_paths,
    rust_recovery::{RustInstallStatus, RustStartupIssue, RustStartupIssueKind},
    workspace_lsp::{ServerInstanceId, WorkspaceLsp},
};
use bed_ui::presentation::same_line_if_fits;
use bed_ui::util::popup_style::{controls_style, tooltip, tooltip_text};
use dear_imgui_rs::{
    ChildFlags, Condition, Key, MouseButton, StyleColor, StyleVar, TreeNodeFlags, Ui, WindowFlags,
    sys,
};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default)]
pub struct LspServerInfo {
    pub language: String,
    pub server: String,
    pub instance: Option<ServerInstanceId>,
    pub server_path: String,
    pub is_found: bool,
    pub is_active: bool,
    pub error: Option<String>,
    pub stderr: String,
    pub progress: Vec<WorkDoneProgress>,
    pub rust_issue: Option<RustStartupIssue>,
    pub startup_details: Option<String>,
    pub rust_install: Option<RustInstallStatus>,
    pub is_remote: bool,
}

#[derive(Clone, Debug)]
enum RestartRequest {
    Server(String),
    Instance(ServerInstanceId),
}

pub struct LspDashboard {
    show: bool,
    window_pos: [f32; 2],
    window_size: [f32; 2],
    server_infos: Vec<LspServerInfo>,
    notification: Option<(String, f32)>,
    restart_requested: Option<RestartRequest>,
    install_requested: Option<ServerInstanceId>,
}

impl Default for LspDashboard {
    fn default() -> Self {
        Self {
            show: false,
            window_pos: [300.0, 200.0],
            window_size: [800.0, 500.0],
            server_infos: Vec::new(),
            notification: None,
            restart_requested: None,
            install_requested: None,
        }
    }
}

impl LspDashboard {
    pub fn is_visible(&self) -> bool {
        self.show
    }

    pub fn toggle_show(&mut self) {
        self.show = !self.show;
    }

    pub fn set_show(&mut self, visible: bool, client: &LspClient) {
        self.show = visible;
        if visible {
            self.refresh_server_info(client);
        }
    }

    pub fn refresh_server_info(&mut self, client: &LspClient) {
        self.server_infos = client.dashboard_servers();
    }

    pub fn set_show_for_workspace(&mut self, visible: bool, workspace: &WorkspaceLsp) {
        self.show = visible;
        if visible {
            self.server_infos = workspace.dashboard_servers();
        }
    }

    pub fn take_restart_server(&mut self) -> Option<String> {
        if matches!(self.restart_requested, Some(RestartRequest::Server(_))) {
            match self.restart_requested.take().unwrap() {
                RestartRequest::Server(server) => Some(server),
                RestartRequest::Instance(_) => unreachable!(),
            }
        } else {
            None
        }
    }

    pub fn take_install_instance(&mut self) -> Option<ServerInstanceId> {
        self.install_requested.take()
    }

    pub fn take_restart_instance(&mut self) -> Option<ServerInstanceId> {
        if matches!(self.restart_requested, Some(RestartRequest::Instance(_))) {
            match self.restart_requested.take().unwrap() {
                RestartRequest::Instance(instance) => Some(instance),
                RestartRequest::Server(_) => unreachable!(),
            }
        } else {
            None
        }
    }

    pub fn render(
        &mut self,
        ui: &Ui,
        client: &mut LspClient,
        settings: &LspPresentationOptions,
    ) -> Option<PathBuf> {
        self.render_backend(ui, client, settings)
    }

    pub fn render_workspace(
        &mut self,
        ui: &Ui,
        workspace: &mut WorkspaceLsp,
        settings: &LspPresentationOptions,
    ) -> Option<PathBuf> {
        if self.show {
            self.server_infos = workspace.dashboard_servers();
        }
        self.render_backend(ui, workspace, settings)
    }

    fn render_backend(
        &mut self,
        ui: &Ui,
        client: &mut impl DashboardBackend,
        settings: &LspPresentationOptions,
    ) -> Option<PathBuf> {
        let mut action = None;
        if self.show {
            let fs = ui.current_font_size();
            if self.window_size[0] < fs * 20.0 {
                self.window_size = [fs * 40.0, fs * 25.0];
            }
            let push_bg = !settings.embedded;
            let bg = settings.background_color();
            let mut bg = bed_ui::color::blend(ui.style_color(StyleColor::Text), bg, 0.055);
            bg[3] = 1.0;
            let _bg = push_bg.then(|| ui.push_style_color(StyleColor::WindowBg, bg));
            let _child_bg = push_bg.then(|| ui.push_style_color(StyleColor::ChildBg, bg));
            let mut open = true;
            ui.window("LSP Server Dashboard")
                .opened(&mut open)
                .position(self.window_pos, Condition::FirstUseEver)
                .size(self.window_size, Condition::FirstUseEver)
                .flags(WindowFlags::NO_COLLAPSE)
                .build(|| {
                    self.window_pos = ui.window_pos();
                    self.window_size = ui.window_size();
                    action = self.render_controls(ui, client);
                    if action.is_some() {
                        self.show = false;
                    }
                    if ui.is_key_pressed(Key::Escape) {
                        self.show = false;
                    }
                    if ui.is_mouse_clicked(MouseButton::Left) {
                        let mouse = ui.io().mouse_pos();
                        let pos = ui.window_pos();
                        let size = ui.window_size();
                        let outside = mouse[0] < pos[0]
                            || mouse[0] > pos[0] + size[0]
                            || mouse[1] < pos[1]
                            || mouse[1] > pos[1] + size[1];
                        let popup_open = ui.with_bound_context(|| unsafe {
                            sys::igIsPopupOpen_Str(
                                std::ptr::null(),
                                sys::ImGuiPopupFlags_AnyPopupId,
                            )
                        });
                        if outside && !popup_open {
                            self.show = false;
                        }
                    }
                });
            if !open {
                self.show = false;
            }
        }
        action
    }

    /// Draw inside a host-owned dock window. This does not change window
    /// geometry, visibility, input blocking, or the host's window colors.
    pub fn render_workspace_body(
        &mut self,
        ui: &Ui,
        workspace: &mut WorkspaceLsp,
    ) -> Option<PathBuf> {
        self.server_infos = workspace.dashboard_servers();
        self.render_controls(ui, workspace)
    }

    fn render_controls(&mut self, ui: &Ui, client: &mut impl DashboardBackend) -> Option<PathBuf> {
        for info in &mut self.server_infos {
            info.progress = client.dashboard_progress(info);
        }
        let _controls = controls_style(ui);
        let fs = ui.current_font_size();
        let _padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.7, fs * 0.6]));
        let _rounding = ui.push_style_var(StyleVar::ChildRounding(fs * 0.45));
        let _border = ui.push_style_var(StyleVar::ChildBorderSize(1.0));
        let mut action = None;
        ui.text("Language Servers");
        {
            let _wrap = ui.push_text_wrap_pos(0.0);
            ui.text_disabled(format!(
                "{} configured · {} active · {} missing",
                self.server_infos.len(),
                self.server_infos
                    .iter()
                    .filter(|info| info.is_active)
                    .count(),
                self.server_infos
                    .iter()
                    .filter(|info| !info.is_found)
                    .count(),
            ));
        }
        if ui.button("Refresh") {
            self.server_infos = client.dashboard_servers();
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Refresh language server status");
        }
        same_line_if_fits(ui, button_width(ui, "Reload"));
        if ui.button("Reload") {
            match client.reload_dashboard_config() {
                Ok(()) => {
                    let message = if client.dashboard_is_discovering() {
                        "Reloading LSP configuration..."
                    } else {
                        "LSP configuration reloaded"
                    };
                    self.notification = Some((message.into(), 2.0));
                }
                Err(error) => {
                    self.notification = Some((format!("LSP configuration: {error}"), 8.0));
                }
            }
            self.server_infos = client.dashboard_servers();
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Reload the language server configuration");
        }
        same_line_if_fits(ui, button_width(ui, "Open config"));
        if ui.button("Open config") {
            let path = client.dashboard_config_path();
            if path.exists() {
                action = Some(path.to_owned());
                self.show = false;
            } else {
                self.notification = Some((
                    format!("Configuration file not found: {}", path.display()),
                    8.0,
                ));
            }
        }
        if ui.is_item_hovered() {
            tooltip_text(ui, "Open the language server configuration file");
        }
        ui.spacing();
        // One scroll region keeps long diagnostics and configuration details
        // from crowding the server list out of a short docked panel.
        ui.child_window("ServerList").build(ui, || {
            let _wrap = ui.push_text_wrap_pos(0.0);
            if let Some((message, timer)) = &mut self.notification {
                ui.text_wrapped(message.as_str());
                *timer -= ui.io().delta_time();
                if *timer <= 0.0 {
                    self.notification = None;
                }
                ui.spacing();
            }
            if client.dashboard_is_discovering() {
                ui.text_disabled("Discovering language servers and project roots…");
                ui.spacing();
            }
            if let Some(error) = client.dashboard_error()
                && !self
                    .server_infos
                    .iter()
                    .any(|info| info.error.as_deref() == Some(error))
            {
                ui.text_colored(
                    bed_ui::presentation::readable_color(ui, [1.0, 0.4, 0.3, 1.0]),
                    error,
                );
                ui.spacing();
            }
            if ui.collapsing_header("Configuration", TreeNodeFlags::empty()) {
                ui.child_window("ConfigurationDetails")
                    .child_flags(ChildFlags::AUTO_RESIZE_Y | ChildFlags::BORDERS)
                    .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
                    .build(ui, || {
                        ui.text_disabled("Later configuration files override earlier ones.");
                        for source in client.dashboard_config_sources() {
                            path_text(ui, &source.to_string_lossy());
                        }
                    });
            }
            ui.spacing();
            self.render_server_list(ui);
        });
        action
    }

    fn render_server_list(&mut self, ui: &Ui) {
        self.server_infos.sort_by_key(|info| {
            !(info.is_active
                || info.error.is_some()
                || info.progress.iter().any(|job| !job.finished))
        });
        let _surface = ui.push_style_color(
            StyleColor::ChildBg,
            bed_ui::color::blend(
                ui.style_color(StyleColor::Text),
                ui.style_color(StyleColor::WindowBg),
                0.025,
            ),
        );
        if self.server_infos.is_empty() {
            ui.child_window("NoServers")
                .child_flags(ChildFlags::AUTO_RESIZE_Y | ChildFlags::BORDERS)
                .build(ui, || {
                    ui.text("No language servers configured");
                    ui.text_disabled("Open the configuration to add a language server.");
                });
            return;
        }
        for info in &self.server_infos {
            let _id = ui.push_id(&info.server);
            let _root_id = info
                .instance
                .as_ref()
                .map(|instance| ui.push_id(instance.root.to_string_lossy().as_ref()));
            ui.child_window("Server")
                .child_flags(ChildFlags::AUTO_RESIZE_Y | ChildFlags::BORDERS)
                .flags(WindowFlags::NO_SCROLLBAR | WindowFlags::NO_SCROLL_WITH_MOUSE)
                .build(ui, || {
                    let _wrap = ui.push_text_wrap_pos(0.0);
                    ui.text_wrapped(&info.server);
                    if !info.language.is_empty() && info.language != info.server {
                        ui.text_disabled(&info.language);
                    }
                    let (color, status) = if !info.is_found {
                        ([1.0, 0.6, 0.3, 1.0], "Missing executable")
                    } else if info.error.is_some() {
                        ([1.0, 0.4, 0.3, 1.0], "Error")
                    } else if info.is_active {
                        ([0.2, 0.8, 0.2, 1.0], "Active")
                    } else {
                        (ui.style_color(StyleColor::TextDisabled), "Inactive")
                    };
                    ui.align_text_to_frame_padding();
                    ui.text_colored(bed_ui::presentation::readable_color(ui, color), status);
                    same_line_if_fits(ui, button_width(ui, "Restart"));
                    let restart_disabled = ui.begin_disabled_with_cond(
                        info.rust_install
                            .as_ref()
                            .is_some_and(|install| install.running),
                    );
                    if ui.button("Restart") {
                        self.restart_requested = Some(if let Some(instance) = &info.instance {
                            RestartRequest::Instance(instance.clone())
                        } else {
                            RestartRequest::Server(info.server.clone())
                        });
                    }
                    if ui.is_item_hovered() {
                        tooltip_text(ui, "Restart this language server");
                    }
                    drop(restart_disabled);
                    ui.spacing();
                    let label_width = ui.calc_text_size("Executable")[0];
                    let gap = ui.clone_style().item_spacing()[0];
                    let value_column = ui.cursor_pos()[0] + label_width + gap;
                    let inline_paths = ui.content_region_avail()[0]
                        >= label_width + gap + ui.current_font_size() * 9.0;
                    ui.text_disabled("Project");
                    if inline_paths {
                        ui.same_line_with_pos(value_column);
                    }
                    if let Some(instance) = &info.instance {
                        path_text(ui, &instance.root.to_string_lossy());
                    } else {
                        ui.text_disabled("Standalone client");
                    }
                    ui.text_disabled("Executable");
                    if inline_paths {
                        ui.same_line_with_pos(value_column);
                    }
                    path_text(ui, &info.server_path);

                    if !info.progress.is_empty() || info.error.is_some() || !info.stderr.is_empty()
                    {
                        ui.spacing();
                        ui.separator();
                    }
                    for job in &info.progress {
                        let text = if job.finished {
                            format!("Completed: {}", job.title)
                        } else {
                            job.title.clone()
                        };
                        if job.finished {
                            ui.text_disabled(text);
                        } else {
                            ui.text_colored(
                                bed_ui::presentation::readable_color(ui, [0.9, 0.8, 0.4, 1.0]),
                                text,
                            );
                        }
                        if let Some(message) = &job.message
                            && !message.is_empty()
                            && message != &job.title
                        {
                            ui.text_disabled(message);
                        }
                        if let Some(percentage) = job.percentage
                            && !job.finished
                        {
                            ui.progress_bar(percentage as f32 / 100.0)
                                .overlay_text(format!("{percentage}%"))
                                .build();
                        }
                    }
                    if let Some(error) = &info.error {
                        ui.text_colored(
                            bed_ui::presentation::readable_color(ui, [1.0, 0.4, 0.3, 1.0]),
                            error,
                        );
                    }
                    if let Some(issue) = &info.rust_issue {
                        ui.text_wrapped(issue.command());
                        if info.is_remote {
                            ui.text_disabled("Run this command on the remote host.");
                        }
                        if ui.button("Copy command") {
                            let command = std::ffi::CString::new(issue.command()).unwrap();
                            ui.with_bound_context(|| unsafe {
                                sys::igSetClipboardText(command.as_ptr())
                            });
                        }
                        if let Some(instance) = &info.instance
                            && !info.is_remote
                        {
                            let label = match issue.kind {
                                RustStartupIssueKind::MissingComponents => {
                                    "Install Rust components"
                                }
                                RustStartupIssueKind::MissingToolchain => "Install Rust toolchain",
                            };
                            same_line_if_fits(ui, button_width(ui, label));
                            let _disabled = ui.begin_disabled_with_cond(!issue.install_available);
                            if ui.button(label) {
                                self.install_requested = Some(instance.clone());
                            }
                        }
                        ui.text_disabled("Includes rust-src for standard-library analysis.");
                        if let Some(details) = &info.startup_details
                            && ui.collapsing_header("Error details", TreeNodeFlags::empty())
                        {
                            ui.text_wrapped(details);
                        }
                    }
                    if let Some(install) = &info.rust_install {
                        ui.text_wrapped(&install.message);
                        if !install.output.is_empty()
                            && ui.collapsing_header(
                                "Installation output",
                                TreeNodeFlags::DEFAULT_OPEN,
                            )
                        {
                            ui.text_wrapped(&install.output);
                        }
                    }
                    if !info.stderr.is_empty()
                        && ui.collapsing_header("Server output", TreeNodeFlags::empty())
                    {
                        ui.text_wrapped(&info.stderr);
                    }
                });
            ui.spacing();
        }
    }
}

trait DashboardBackend {
    fn dashboard_servers(&self) -> Vec<LspServerInfo>;

    fn dashboard_progress(&self, server: &LspServerInfo) -> Vec<WorkDoneProgress>;

    fn dashboard_config_path(&self) -> &Path;

    fn reload_dashboard_config(&mut self) -> io::Result<()>;

    fn dashboard_error(&self) -> Option<&str>;

    fn dashboard_config_sources(&self) -> Vec<PathBuf> {
        vec![self.dashboard_config_path().to_owned()]
    }

    fn dashboard_is_discovering(&self) -> bool {
        false
    }
}

impl DashboardBackend for LspClient {
    fn dashboard_progress(&self, server: &LspServerInfo) -> Vec<WorkDoneProgress> {
        if self.current_server() == server.server {
            self.progress()
        } else {
            Vec::new()
        }
    }

    fn dashboard_servers(&self) -> Vec<LspServerInfo> {
        self.language_servers()
            .iter()
            .map(|(name, config)| {
                let path = resolve_server_paths(&config.command);
                let current = self.current_server() == name.as_str();
                LspServerInfo {
                    language: self
                        .config()
                        .languages
                        .iter()
                        .filter(|language| {
                            language.enabled && language.language_server.as_ref() == Some(name)
                        })
                        .map(|language| language.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    server: name.clone(),
                    instance: None,
                    server_path: path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| config.command.join(", ")),
                    is_found: path.is_some(),
                    is_active: path.is_some() && self.is_initialized() && current,
                    error: current
                        .then(|| self.last_error().map(str::to_owned))
                        .flatten(),
                    stderr: if current {
                        self.stderr_text()
                    } else {
                        String::new()
                    },
                    progress: if current { self.progress() } else { Vec::new() },
                    rust_issue: current
                        .then(|| self.rust_startup_issue().cloned())
                        .flatten(),
                    startup_details: current.then(|| self.startup_error_details()).flatten(),
                    rust_install: None,
                    is_remote: self.is_remote(),
                }
            })
            .collect()
    }

    fn dashboard_config_path(&self) -> &Path {
        self.config_path()
    }

    fn reload_dashboard_config(&mut self) -> io::Result<()> {
        self.reload_config()
    }

    fn dashboard_error(&self) -> Option<&str> {
        self.last_error()
    }
}

impl DashboardBackend for WorkspaceLsp {
    fn dashboard_progress(&self, server: &LspServerInfo) -> Vec<WorkDoneProgress> {
        server
            .instance
            .as_ref()
            .and_then(|instance| self.client_for_instance(instance))
            .map(|client| client.borrow().progress())
            .unwrap_or_default()
    }

    fn dashboard_servers(&self) -> Vec<LspServerInfo> {
        self.server_statuses()
            .into_iter()
            .map(|status| LspServerInfo {
                language: self
                    .config()
                    .languages
                    .iter()
                    .filter(|language| {
                        language.enabled
                            && language.language_server.as_ref() == Some(&status.instance.server)
                    })
                    .map(|language| language.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                server: status.instance.server.clone(),
                server_path: status
                    .path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| {
                        self.config()
                            .language_servers
                            .get(&status.instance.server)
                            .map(|server| server.command.join(", "))
                            .unwrap_or_else(|| "Not found".into())
                    }),
                is_found: status.path.is_some(),
                is_active: status.initialized,
                error: status.last_error,
                stderr: status.stderr,
                progress: status.progress,
                rust_issue: status.rust_issue,
                startup_details: status.startup_details,
                rust_install: status.rust_install,
                is_remote: self.ssh_target().is_some(),
                instance: Some(status.instance),
            })
            .collect()
    }

    fn dashboard_config_path(&self) -> &Path {
        self.config_path()
    }

    fn reload_dashboard_config(&mut self) -> io::Result<()> {
        self.reload_config()
    }

    fn dashboard_error(&self) -> Option<&str> {
        self.config_error()
    }

    fn dashboard_config_sources(&self) -> Vec<PathBuf> {
        self.configuration_sources()
    }

    fn dashboard_is_discovering(&self) -> bool {
        self.is_discovering()
    }
}

fn button_width(ui: &Ui, label: &str) -> f32 {
    ui.calc_text_size(label)[0] + ui.clone_style().frame_padding()[0] * 2.0
}

fn path_text(ui: &Ui, path: &str) {
    ui.text(display_path(ui, path, ui.content_region_avail()[0]));
    if ui.is_item_hovered() {
        tooltip(ui, || {
            let _wrap = ui.push_text_wrap_pos(ui.cursor_pos()[0] + ui.current_font_size() * 36.0);
            // text_wrapped overrides this width with the tooltip's initial right edge.
            ui.text(path);
        });
    }
}

// Keep the filename visible, sizing to the actual font and available space.
fn display_path(ui: &Ui, path: &str, width: f32) -> String {
    if ui.calc_text_size(path)[0] <= width {
        return path.to_owned();
    }
    let ellipsis = ui.calc_text_size("…")[0];
    if width < ellipsis {
        return String::new();
    }
    let mut start = path.len();
    for (index, _) in path.char_indices().rev() {
        if ui.calc_text_size(&path[index..])[0] > width - ellipsis {
            break;
        }
        start = index;
    }
    format!("…{}", &path[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{presentation::LspPresentationOptions, test_support::TempDir};
    use dear_imgui_rs::{Context, FramePrepareOptions};
    use serde_json::json;

    #[test]
    fn rust_recovery_card_wraps_in_narrow_dashboard_panels() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let issue = RustStartupIssue {
            toolchain: "1.99.0-aarch64-apple-darwin".into(),
            kind: RustStartupIssueKind::MissingComponents,
            install_available: true,
        };
        let mut dashboard = LspDashboard {
            server_infos: vec![LspServerInfo {
                language: "rust".into(), server: "rust-analyzer".into(),
                instance: Some(ServerInstanceId { server: "rust-analyzer".into(), root: "/project".into() }),
                is_found: true, error: Some(issue.message()), rust_issue: Some(issue),
                startup_details: Some("LSP header reached EOF\nerror: Unknown binary 'rust-analyzer' in official toolchain '1.99.0-aarch64-apple-darwin'.".into()),
                rust_install: Some(RustInstallStatus { running: true, message: "Installing Rust tooling…".into(), output: "info: downloading component rust-analyzer for 1.99.0-aarch64-apple-darwin".into() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for width in [240.0, 320.0, 800.0] {
            for frame in 0..3 {
                context.prepare_frame(FramePrepareOptions::new([1000.0, 900.0], 1.0 / 60.0));
                let ui = context.frame();
                ui.window("Rust recovery layout")
                    .position([0.0; 2], Condition::Always)
                    .size([width, 850.0], Condition::Always)
                    .build(|| {
                        dashboard.render_server_list(ui);
                        if frame > 0 {
                            ui.with_bound_context(|| unsafe {
                                let parent = sys::igGetCurrentWindow();
                                let child = *(*parent).DC.ChildWindows.Data;
                                assert!(
                                    (*child).ScrollMax.x <= 4.0,
                                    "Recovery overflow at {width}"
                                );
                            });
                        }
                    });
                assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
                assert!(
                    dashboard.take_install_instance().is_none(),
                    "Drawing must not start installation"
                );
                assert!(dashboard.take_restart_instance().is_none());
            }
        }
    }

    #[test]
    fn dashboard_discovery_is_per_configuration_and_cached_until_refresh() {
        let directory = TempDir::new();
        let path = directory.write(
            "lsp.json",
            &serde_json::to_vec(&json!({
                "languages": [
                    {"name":"a", "file_types":["a"], "language_server":"a-found"},
                    {"name":"b", "file_types":["b"], "language_server":"b-missing"}
                ],
                "language_servers": {
                    "a-found": {"command": [
                        concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
                        concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml")
                    ]},
                    "b-missing": {"command":"/bed-no-server"}
                }
            }))
            .unwrap(),
        );
        let mut client = LspClient::new(path.clone());
        let mut dashboard = LspDashboard::default();
        dashboard.set_show(true, &client);
        assert_eq!(dashboard.server_infos.len(), 2);
        assert!(dashboard.server_infos[0].is_found);
        assert!(!dashboard.server_infos[0].is_active);
        assert!(!dashboard.server_infos[1].is_found);
        std::fs::write(path, b"{\"languages\":[]}").unwrap();
        client.reload_config().unwrap();
        assert_eq!(dashboard.server_infos.len(), 2);
        dashboard.refresh_server_info(&client);
        assert!(dashboard.server_infos.is_empty());
    }

    #[test]
    fn native_workspace_dashboard_body_keeps_docked_host_geometry_and_style() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let path = directory.write("lsp.json", b"{\"languages\":[]}");
        let mut pool = WorkspaceLsp::new(
            bed_document_session::editor_session::WorkspaceId(1),
            path,
            directory.root().to_owned(),
        );
        let mut dashboard = LspDashboard::default();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([1000.0, 800.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Host dashboard")
                .position([80.0, 90.0], Condition::Always)
                .size([700.0, 500.0], Condition::Always)
                .build(|| {
                    let before = (
                        ui.window_pos(),
                        ui.window_size(),
                        ui.style_color(StyleColor::WindowBg),
                    );
                    assert!(dashboard.render_workspace_body(ui, &mut pool).is_none());
                    assert_eq!(
                        before,
                        (
                            ui.window_pos(),
                            ui.window_size(),
                            ui.style_color(StyleColor::WindowBg)
                        )
                    );
                });
            ui.with_bound_context(|| unsafe {
                assert!(sys::igFindWindowByName(c"LSP Server Dashboard".as_ptr()).is_null());
            });
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        }
        assert!(!dashboard.is_visible());
    }

    #[test]
    fn native_dashboard_layout_escape_and_outside_dismissal() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let settings = LspPresentationOptions {
            embedded: false,
            ..Default::default()
        };
        let path = directory.write(
            "lsp.json",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../resources/config/lsp.json"
            )),
        );
        let mut client = LspClient::new(path);
        let mut dashboard = LspDashboard::default();
        dashboard.set_show(true, &client);
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let draw = |context: &mut Context, dashboard: &mut LspDashboard, client: &mut LspClient| {
            context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
            assert!(
                dashboard
                    .render(context.frame(), client, &settings)
                    .is_none()
            );
            let draw = context.render_legacy();
            assert!(draw.draw_data().total_vtx_count() > 0);
        };
        draw(&mut context, &mut dashboard, &mut client);
        draw(&mut context, &mut dashboard, &mut client);
        context.io_mut().add_key_event(Key::Escape, true);
        draw(&mut context, &mut dashboard, &mut client);
        assert!(!dashboard.is_visible());
        context.io_mut().clear_input_keys();
        dashboard.set_show(true, &client);
        draw(&mut context, &mut dashboard, &mut client);
        context.io_mut().add_mouse_pos_event([0.0, 0.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut dashboard, &mut client);
        assert!(!dashboard.is_visible());
    }
}
