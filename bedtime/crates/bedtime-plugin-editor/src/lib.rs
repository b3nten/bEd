//! Built-in text and byte panels registered by EditorPlugin.
//! Enable `language-services` to add the existing code editor and IDE tools.
//!
//! `bedtime_editor_widgets::EditorView` remains directly embeddable. This crate adds the desktop
//! panel lifecycle without making the workbench API depend on text editor types.
//! Preferences under `bedtime.editor` configure existing and new views, including cursor
//! movement, wrapping, minimap and rainbow text. With `language-services`, this plugin
//! also owns language completion, Git line markers, syntax highlighting and the
//! format-on-save preference. Applications supply defaults through the shared settings
//! service; they do not need to apply editor preferences themselves.
//! With application paths and a selected directory, language services use layered
//! `config_dir/lsp.json` overrides and the configured workspace directory's
//! `lsp.json` project overrides; explicitly supplied session configuration wins.
#[cfg(feature = "language-services")]
mod feature;
#[cfg(not(feature = "language-services"))]
#[path = "plain.rs"]
mod feature;
#[cfg(feature = "language-services")]
pub mod lsp;
#[cfg(feature = "language-services")]
pub mod presentation;
#[cfg(feature = "language-services")]
mod runtime;
#[cfg(feature = "language-services")]
mod status;
#[cfg(feature = "language-services")]
mod zoom;
use bedtime_api::{
    documents::DocumentKind,
    features::{FeatureRegistrar, HostAction, HostContext},
    workbench::{PanelAction, PanelPlacement},
};
use bedtime_documents::{DocumentId, DocumentSession};
use bedtime_editing::{text_commands::CursorReveal, text_view_state::Selection};
use bedtime_editor_widgets::{EditorView, EditorViewOptions, hex_editor::HexEditor};
use dear_imgui_rs::Ui;

fn unfocused_view(
    world: &bevy::prelude::World,
    entity: bevy::prelude::Entity,
) -> Option<bedtime_documents::ViewId> {
    let owner = bedtime_api::workbench::panel_owner(world, entity)?;
    let focused = world
        .get_resource::<bedtime_workbench::WorkbenchLayout>()?
        .focused?;
    if focused == owner {
        return None;
    }
    world
        .get::<bedtime_api::documents::PanelBinding>(entity)?
        .view
}

pub use feature::{
    EditorFrontend, EditorPanelState, EditorPlugin, EditorViews, close_panel, create_embedded,
    draw_embedded, focus_panel, input_is_empty, restore_workspace, save_embedded, save_workspace,
};
#[cfg(feature = "language-services")]
pub use runtime::{
    EditorAction, EditorMenuCommand, EditorRuntime, NavigationActivity, NavigationOutcome,
    NavigationOutcomeKind, NavigationStage, NavigationTiming,
};
use serde_json::{Value, json};
use std::{cell::RefCell, io, path::Path};

#[cfg(all(test, feature = "language-services"))]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, unix, feature = "language-services"))]
#[path = "../../../tests/support/rustup_fixture.rs"]
pub(crate) mod rustup_fixture;
#[cfg(test)]
#[allow(dead_code, clippy::duplicate_mod)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

pub const MODULE_ID: &str = "bedtime.editor";
#[cfg(test)]
mod native_clipboard_tests;
pub const TEXT_PANEL_TYPE: &str = "bedtime.editor.text-panel";
pub const HEX_PANEL_TYPE: &str = "bedtime.editor.hex-panel";
pub const TEXT_VIEWER: &str = "bedtime.editor.text";
pub const HEX_VIEWER: &str = "bedtime.editor.hex";
pub const DIAGNOSTICS_PANEL_TYPE: &str = "bedtime.editor.diagnostics-panel";
pub const NEW_DIAGNOSTICS_COMMAND: &str = "bedtime.editor.new_diagnostics";
pub const SHOW_DIAGNOSTICS_COMMAND: &str = "bedtime.editor.show_diagnostics";
pub const SHOW_LSP_COMMAND: &str = "bedtime.editor.show_language_servers";
pub const REFERENCES_PANEL_TYPE: &str = "bedtime.editor.references-panel";
pub const LSP_DASHBOARD_PANEL_TYPE: &str = "bedtime.editor.language-servers-panel";

mod settings_view;

fn register_settings(world: &mut bevy::prelude::World) {
    use bedtime_api::settings::SettingDefinition;
    let definitions = vec![
        SettingDefinition::new("word_wrap", "Wrap long lines", true),
        SettingDefinition::new("minimap", "Show minimap", true),
        SettingDefinition::new("rainbow", "Rainbow text", true),
        SettingDefinition::new("cursor_smear", "Animate cursor movement", true),
    ];
    #[cfg(feature = "language-services")]
    let definitions = definitions
        .into_iter()
        .chain([
            SettingDefinition::new("lsp_autocomplete", "Language server autocomplete", true),
            SettingDefinition::new("lsp_completion_delay_ms", "Completion delay (ms)", 150.0)
                .validated_by(completion_delay),
            SettingDefinition::new("lsp_format_on_save", "Format on save", true),
            SettingDefinition::new("git_changed_lines", "Show changed lines", true),
            SettingDefinition::new("treesitter", "Syntax highlighting", true),
        ])
        .collect();
    bedtime_api::settings::register(world, MODULE_ID, definitions)
        .expect("Editor preferences install once");
}

#[cfg(feature = "language-services")]
fn completion_delay(value: &Value) -> Result<(), String> {
    if value
        .as_f64()
        .is_some_and(|value| (0.0..u64::MAX as f64).contains(&value))
    {
        Ok(())
    } else {
        Err("Completion delay must be nonnegative and fit in milliseconds".into())
    }
}

fn apply_settings(config: &mut EditorConfig, snapshot: &bedtime_api::settings::SettingsSnapshot) {
    let settings = snapshot.for_owner(MODULE_ID);
    if let Some(value) = settings["word_wrap"].as_bool() {
        config.options.soft_wrap = value;
    }
    if let Some(value) = settings["minimap"].as_bool() {
        config.options.minimap_enabled = value;
    }
    if let Some(value) = settings["rainbow"].as_bool() {
        config.options.rainbow_mode = value;
    }
    if let Some(value) = settings["cursor_smear"].as_bool() {
        config.options.cursor_smear = value;
    }
    if let Some(value) = snapshot
        .get("bedtime.ui", "ui_animations")
        .and_then(Value::as_bool)
    {
        config.navigation_animations = value;
    }
    #[cfg(feature = "language-services")]
    {
        if let Some(value) = settings["lsp_autocomplete"].as_bool() {
            config.lsp.autocomplete = value;
        }
        if let Some(value) = settings["lsp_completion_delay_ms"].as_f64() {
            config.lsp.completion_delay_ms = value as u64;
        }
    }
}

/// Apply editor preferences independently of an application's settings frontend.
fn configure_preferences(world: &mut bevy::prelude::World) {
    let snapshot = world
        .get_resource::<bedtime_api::settings::SettingsSnapshot>()
        .cloned()
        .unwrap_or_default();
    apply_settings(&mut world.non_send_mut::<EditorConfig>(), &snapshot);
    #[cfg(feature = "language-services")]
    {
        let default_lsp_config = world
            .get_resource::<bedtime_api::launch::ApplicationPaths>()
            .map(|paths| {
                (
                    paths.config_dir.join("lsp.json"),
                    std::path::PathBuf::from(&paths.workspace_directory_name).join("lsp.json"),
                )
            });
        let mut session = world.non_send_mut::<DocumentSession>();
        let mut options = session.options().clone();
        if options.lsp_config.is_none()
            && options.project_root.is_some()
            && let Some((path, project_config)) = default_lsp_config
        {
            options.lsp_config = Some(path);
            options.lsp_project_config.get_or_insert(project_config);
            options.lsp_config_mode = bedtime_documents::LspConfigMode::Layered;
            options.lsp_file_observations = true;
        }
        if let Some(value) = snapshot
            .get(MODULE_ID, "git_changed_lines")
            .and_then(Value::as_bool)
        {
            options.git = value;
        }
        if let Some(value) = snapshot
            .get(MODULE_ID, "treesitter")
            .and_then(Value::as_bool)
        {
            options.highlighting = value;
        }
        if session.options() != &options
            && let Err(error) = session.configure(options)
        {
            eprintln!("Editor preferences: {error}");
        }
    }
}

/// Application presentation settings shared by existing and newly created views.
/// Editor-specific feature contributions are registered in `options.extensions`.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    pub options: EditorViewOptions,
    /// Animate navigation, zoom resets, and completion selection/documentation.
    pub navigation_animations: bool,
    #[cfg(feature = "language-services")]
    pub lsp: crate::presentation::LspPresentationOptions,
}

impl Default for EditorConfig {
    fn default() -> Self {
        let options = EditorViewOptions::default();
        #[cfg(feature = "language-services")]
        let mut lsp = crate::presentation::LspPresentationOptions::default();
        #[cfg(feature = "language-services")]
        {
            lsp.shortcuts = options.shortcuts.clone();
        }
        Self {
            options,
            navigation_animations: true,
            #[cfg(feature = "language-services")]
            lsp,
        }
    }
}

#[cfg(feature = "language-services")]
fn document_title(host: &HostContext<'_>, document: DocumentId, hex: bool) -> String {
    let Some(document) = host.document(document) else {
        return "Closed".into();
    };
    let name = if document.path.is_empty() {
        "Untitled".to_owned()
    } else {
        Path::new(&document.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    if hex { format!("{name} [Hex]") } else { name }
}

// Once a buffer has been used, undoing its contents must not make it available
// for replacement. The initial revision also protects clean empty byte editors.
fn pristine_revision(
    session: &DocumentSession,
    document: DocumentId,
) -> Option<bedtime_api::documents::Revision> {
    if session
        .with_document(document, |state| {
            state.path.is_empty()
                && state.byte_size() == 0
                && !state.dirty
                && state.version == 0
                && !state.read_only
        })
        .ok()?
        && session.original_path(document).ok()?.is_none()
    {
        session.document_revision(document).ok()
    } else {
        None
    }
}

fn input_is_pristine(
    host: &HostContext<'_>,
    document: DocumentId,
    initial_revision: Option<bedtime_api::documents::Revision>,
) -> bool {
    host.document(document).is_some_and(|document| {
        initial_revision == Some(document.revision)
            && document.path.is_empty()
            && document.bytes.is_empty()
            && !document.dirty
    })
}

fn restore_text_state(
    session: &mut DocumentSession,
    view: bedtime_documents::ViewId,
    state: &Value,
) -> io::Result<()> {
    if let Some(values) = state.get("selections").and_then(Value::as_array) {
        let selections = values
            .iter()
            .filter_map(|value| {
                let value = value.as_array()?;
                Some(Selection {
                    head_row: value.first()?.as_i64()? as i32,
                    head_column: value.get(1)?.as_i64()? as i32,
                    anchor_row: value.get(2)?.as_i64()? as i32,
                    anchor_column: value.get(3)?.as_i64()? as i32,
                    preferred_column: 0,
                })
            })
            .collect();
        session.with_commands(view, |commands| {
            commands.set_selections(
                selections,
                state.get("primary").and_then(Value::as_u64).unwrap_or(0) as usize,
                CursorReveal::Ensure,
            )
        })?;
    }
    if let Some(scroll) = state
        .get("scroll")
        .and_then(Value::as_array)
        .filter(|scroll| scroll.len() == 2)
    {
        session.set_scroll(
            view,
            scroll[0].as_f64().unwrap_or(0.0) as f32,
            scroll[1].as_f64().unwrap_or(0.0) as f32,
        )?;
    }
    Ok(())
}
