//! Existing integration coverage, consolidated into one executable.
mod fixtures;
#[path = "../support/headless_host.rs"]
mod headless_host;
#[path = "../support/rustup_fixture.rs"]
mod rustup_fixture;
use rustup_fixture::temp_dir;
// ImGui owns process-global state. Tests using a context share this lock.
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
// The three LSP layers include the same fixture source in this process.
static LSP_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
mod core;
mod debugger;
mod documents;
mod editing;
mod editor_widgets;
mod files;
mod git;
mod highlight;
mod launch;
mod lsp;
mod plugin_csv_table;
mod plugin_editor;
mod plugin_markdown_preview;
mod remote_agent;
mod terminal;
mod workbench;
