//! Existing integration coverage, consolidated into one executable.
#[path = "../support/application_requests.rs"]
mod application_requests;
#[path = "../support/editor_host.rs"]
mod editor_host;
mod fixtures;
#[path = "../../../bedtime/tests/support/temp_dir.rs"]
mod temp_dir;
// ImGui owns process-global state. Tests using a context share this lock.
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
mod app_remote_editor;
mod bevy_regressions;
mod debug_workbench;
mod docs_panel;
mod lsp_actions_rust_analyzer;
mod lsp_rust_analyzer;
mod startup_layout;
mod viewer_menus;
mod workbench_focus;
