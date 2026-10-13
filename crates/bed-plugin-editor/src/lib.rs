//! Built-in text and byte panels registered by EditorPlugin.
//! Enable `language-services` to add the existing code editor and IDE tools.
//!
//! `bed_editor_widgets::EditorView` remains directly embeddable. This crate adds the desktop
//! panel lifecycle without making the workbench API depend on text editor types.
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
use bed_api::{
    documents::DocumentKind,
    features::{FeatureRegistrar, HostAction, HostContext},
    workbench::{PanelAction, PanelPlacement},
};
use bed_documents::{DocumentId, DocumentSession};
use bed_editing::{text_commands::CursorReveal, text_view_state::Selection};
use bed_editor_widgets::{EditorView, EditorViewOptions, hex_editor::HexEditor};
use dear_imgui_rs::Ui;
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

pub const MODULE_ID: &str = "bed.editor";
pub const TEXT_PANEL_TYPE: &str = "bed.editor.text-panel";
pub const HEX_PANEL_TYPE: &str = "bed.editor.hex-panel";
pub const TEXT_VIEWER: &str = "bed.editor.text";
pub const HEX_VIEWER: &str = "bed.editor.hex";
pub const DIAGNOSTICS_PANEL_TYPE: &str = "bed.editor.diagnostics-panel";
pub const NEW_DIAGNOSTICS_COMMAND: &str = "bed.editor.new_diagnostics";
pub const SHOW_DIAGNOSTICS_COMMAND: &str = "bed.editor.show_diagnostics";
pub const SHOW_LSP_COMMAND: &str = "bed.editor.show_language_servers";
pub const REFERENCES_PANEL_TYPE: &str = "bed.editor.references-panel";
pub const LSP_DASHBOARD_PANEL_TYPE: &str = "bed.editor.language-servers-panel";

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
) -> Option<bed_api::documents::Revision> {
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
    initial_revision: Option<bed_api::documents::Revision>,
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
    view: bed_documents::ViewId,
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
