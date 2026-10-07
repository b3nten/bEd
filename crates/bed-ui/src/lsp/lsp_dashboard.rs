//! Translated from ned lsp/lsp_dashboard.{h,cpp}; see LICENSE and NOTICE.
use crate::presentation::LspPresentationOptions;
use bed_lsp::{
    lsp_client::{LspClient, WorkDoneProgress},
    lsp_config::resolve_server_paths,
    workspace_lsp::WorkspaceLsp,
};
use dear_imgui_rs::{
    Condition, Key, MouseButton, StyleColor, TableColumnFlags, TableColumnWidth, TableFlags, Ui,
    WindowFlags, sys,
};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default)]
pub struct LspServerInfo {
    pub language: String,
    pub server_path: String,
    pub is_found: bool,
    pub is_active: bool,
    pub error: Option<String>,
    pub stderr: String,
    pub progress: Vec<WorkDoneProgress>,
}
pub struct LspDashboard {
    show: bool,
    window_pos: [f32; 2],
    window_size: [f32; 2],
    server_infos: Vec<LspServerInfo>,
    notification: Option<(String, f32)>,
    restart_requested: Option<String>,
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
    pub fn take_restart_language(&mut self) -> Option<String> {
        std::mem::take(&mut self.restart_requested)
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
            let mut bg = bed_core::util::color::blend(ui.style_color(StyleColor::Text), bg, 0.055);
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
        self.render_notification(ui, settings);
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
            info.progress = client.dashboard_progress(&info.language);
        }
        let mut action = None;
        ui.text("Language Server Protocol Dashboard");
        ui.separator();
        if ui.button("Refresh Server Status") {
            self.server_infos = client.dashboard_servers();
        }
        ui.same_line();
        if ui.button("Reload LSP.json") {
            if let Err(error) = client.reload_dashboard_config() {
                eprintln!("LSP: configuration reload failed: {error}");
            }
            self.server_infos = client.dashboard_servers();
            self.notification = Some((format!("LSP Servers: {}", self.server_infos.len()), 2.0));
        }
        ui.same_line();
        if ui.button("Open LSP.json") {
            let path = client.dashboard_config_path();
            if path.exists() {
                action = Some(path.to_owned());
                self.show = false;
            } else {
                eprintln!(
                    "[LSP Dashboard] LSP.json file not found at: {}",
                    path.display()
                );
            }
        }
        ui.text(format!("{} servers configured", self.server_infos.len()));
        if !client.dashboard_current_language().is_empty() {
            ui.same_line();
            if ui.button("Restart Server") {
                self.restart_requested = Some(client.dashboard_current_language().to_owned());
            }
        }
        if let Some(error) = client.dashboard_error() {
            ui.text_colored(
                crate::presentation::readable_color(ui, [1.0, 0.4, 0.3, 1.0]),
                error,
            );
        }
        for info in &self.server_infos {
            for job in &info.progress {
                let mut text = format!("{}: {}", info.language, job.title);
                if let Some(message) = &job.message
                    && !message.is_empty()
                    && message != &job.title
                {
                    text.push_str(" — ");
                    text.push_str(message);
                }
                if let Some(percentage) = job.percentage {
                    text.push_str(&format!(" ({percentage}%)"));
                }
                if job.finished {
                    ui.text_disabled(format!("Completed: {text}"));
                } else {
                    ui.text_colored(
                        crate::presentation::readable_color(ui, [0.9, 0.8, 0.4, 1.0]),
                        &text,
                    );
                }
            }
            if let Some(error) = &info.error {
                ui.text_colored(
                    crate::presentation::readable_color(ui, [1.0, 0.4, 0.3, 1.0]),
                    format!("{}: {error}", info.language),
                );
            }
            if !info.stderr.is_empty()
                && ui.collapsing_header(
                    format!("{} stderr", info.language),
                    dear_imgui_rs::TreeNodeFlags::empty(),
                )
            {
                ui.text_wrapped(&info.stderr);
            }
        }
        ui.spacing();
        self.render_server_list(ui);
        action
    }
    fn render_server_list(&mut self, ui: &Ui) {
        ui.child_window("ServerList")
            .size([0.0, -ui.frame_height_with_spacing()])
            .border(true)
            .build(ui, || {
                if self.server_infos.is_empty() {
                    ui.text_disabled("No LSP servers configured");
                } else if let Some(_table) = ui.begin_table_with_flags(
                    "ServerTable",
                    4,
                    TableFlags::ROW_BG | TableFlags::SCROLL_Y | TableFlags::RESIZABLE,
                ) {
                    ui.table_setup_column(
                        "Language",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Fixed(120.0)),
                    );
                    ui.table_setup_column(
                        "Server Path",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Stretch(0.0)),
                    );
                    ui.table_setup_column(
                        "Found",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Fixed(140.0)),
                    );
                    ui.table_setup_column(
                        "Status",
                        TableColumnFlags::NONE,
                        Some(TableColumnWidth::Fixed(180.0)),
                    );
                    ui.table_headers_row();
                    for (index, info) in self.server_infos.iter().enumerate() {
                        ui.table_next_row();
                        ui.table_set_column_index(0);
                        ui.text(&info.language);
                        ui.table_set_column_index(1);
                        ui.text(display_path(&info.server_path));
                        if ui.is_item_hovered() {
                            ui.tooltip_text(&info.server_path);
                        }
                        ui.table_set_column_index(2);
                        ui.text_colored(
                            crate::presentation::readable_color(
                                ui,
                                if info.is_found {
                                    [0.2, 0.8, 0.2, 1.0]
                                } else {
                                    [0.8, 0.2, 0.2, 1.0]
                                },
                            ),
                            if info.is_found {
                                "● Found"
                            } else {
                                "● Missing"
                            },
                        );
                        ui.table_set_column_index(3);
                        let (color, text) = if !info.is_found {
                            ([0.5, 0.5, 0.5, 1.0], "N/A")
                        } else if info.is_active {
                            ([0.2, 0.8, 0.2, 1.0], "● Active")
                        } else {
                            ([0.7, 0.7, 0.7, 1.0], "● Inactive")
                        };
                        ui.text_colored(crate::presentation::readable_color(ui, color), text);
                        ui.same_line();
                        if ui.button(format!("Restart##{}-{index}", info.language)) {
                            self.restart_requested = Some(info.language.clone());
                        }
                    }
                }
            });
    }
    // LspPresentationOptions::renderNotification is immediate-mode upstream. Keep its same
    // foreground draw and 2-second timer local to this dashboard adapter.
    fn render_notification(&mut self, ui: &Ui, settings: &LspPresentationOptions) {
        let Some((text, timer)) = &mut self.notification else {
            return;
        };
        let viewport = ui.main_viewport();
        let pos = viewport.pos();
        let size = viewport.size();
        let max_width = size[0] * 0.8;
        let text_size = ui.with_bound_context(|| unsafe {
            let text = std::ffi::CString::new(text.as_str()).unwrap();
            sys::igCalcTextSize(text.as_ptr(), std::ptr::null(), false, max_width - 30.0)
        });
        let width = (text_size.x + 30.0).max(200.0).min(max_width);
        let height = (text_size.y + 30.0).max(50.0).min(size[1] * 0.4);
        let origin = [pos[0] + 20.0, pos[1] + size[1] - height - 20.0];
        let bg = settings.background_color();
        let color = (bg[0] * 255.0) as u32
            | (((bg[1] * 255.0) as u32) << 8)
            | (((bg[2] * 255.0) as u32) << 16)
            | (230 << 24);
        let draw = ui.get_foreground_draw_list();
        let max = [origin[0] + width, origin[1] + height];
        draw.add_rect(origin, max, color)
            .rounding(8.0)
            .filled(true)
            .build();
        draw.add_rect(origin, max, ui.style_color(StyleColor::Border))
            .rounding(8.0)
            .thickness(1.0)
            .build();
        draw.add_text(
            [origin[0] + 15.0, origin[1] + 15.0],
            ui.style_color(StyleColor::Text),
            text.as_str(),
        );
        *timer -= ui.io().delta_time();
        if *timer <= 0.0 {
            self.notification = None;
        }
    }
}
trait DashboardBackend {
    fn dashboard_servers(&self) -> Vec<LspServerInfo>;
    fn dashboard_progress(&self, language: &str) -> Vec<WorkDoneProgress>;
    fn dashboard_config_path(&self) -> &Path;
    fn reload_dashboard_config(&mut self) -> io::Result<()>;
    fn dashboard_current_language(&self) -> &str {
        ""
    }
    fn dashboard_error(&self) -> Option<&str>;
}
impl DashboardBackend for LspClient {
    fn dashboard_progress(&self, language: &str) -> Vec<WorkDoneProgress> {
        if self.current_language() == language {
            self.progress()
        } else {
            Vec::new()
        }
    }
    fn dashboard_servers(&self) -> Vec<LspServerInfo> {
        self.language_servers()
            .iter()
            .map(|config| {
                let path = resolve_server_paths(&config.server_paths);
                let current = self.current_language() == config.language;
                LspServerInfo {
                    language: config.language.clone(),
                    server_path: path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Not found".into()),
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
    fn dashboard_current_language(&self) -> &str {
        self.current_language()
    }
    fn dashboard_error(&self) -> Option<&str> {
        self.last_error()
    }
}
impl DashboardBackend for WorkspaceLsp {
    fn dashboard_progress(&self, language: &str) -> Vec<WorkDoneProgress> {
        self.client_for_language(language)
            .map(|client| client.borrow().progress())
            .unwrap_or_default()
    }
    fn dashboard_servers(&self) -> Vec<LspServerInfo> {
        self.server_statuses()
            .into_iter()
            .map(|status| LspServerInfo {
                language: status.language,
                server_path: status
                    .path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Not found".into()),
                is_found: status.path.is_some(),
                is_active: status.initialized,
                error: status.last_error,
                stderr: status.stderr,
                progress: status.progress,
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
}

fn display_path(path: &str) -> String {
    if path.len() <= 40 {
        return path.to_owned();
    }
    let mut start = path.len() - 37;
    while !path.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &path[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{presentation::LspPresentationOptions, test_support::TempDir};
    use dear_imgui_rs::{Context, FramePrepareOptions};
    use serde_json::json;
    #[test]
    fn dashboard_discovery_is_per_configuration_and_cached_until_refresh() {
        let directory = TempDir::new();
        let path = directory.write("lsp.json",serde_json::to_vec(&json!({"languages":[{"language_name":"same","language_file_extensions":[".a"],"language_server_paths":[concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),concat!(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),"/Cargo.toml")]},{"language_name":"same","language_file_extensions":[".b"],"language_server_paths":["/bed-no-server"]}]})).unwrap().as_slice());
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
        assert_eq!(
            display_path("abcdefghijklmnopqrstuvwxyz0123456789abcdef"),
            "...fghijklmnopqrstuvwxyz0123456789abcdef"
        );
        assert_eq!(
            display_path(&"é".repeat(30)),
            format!("...{}", "é".repeat(18))
        );
    }
    #[test]
    fn native_workspace_dashboard_body_keeps_docked_host_geometry_and_style() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let path = directory.write("lsp.json", b"{\"languages\":[]}");
        let mut pool = WorkspaceLsp::new(
            bed_session::editor_session::WorkspaceId(1),
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
    fn native_dashboard_table_escape_and_outside_dismissal() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let settings = LspPresentationOptions {
            embedded: false,
            ..Default::default()
        };
        let path = directory.write(
            "lsp.json",
            include_bytes!("../../../../resources/config/lsp.json"),
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
