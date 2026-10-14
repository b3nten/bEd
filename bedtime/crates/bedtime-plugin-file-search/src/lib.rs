//! Project query widgets and one revision-checked workspace replacement job.
pub mod content_search;
mod replacement;
use bedtime_api::{
    features::HostAction,
    workbench::{PanelAction, PanelPlacement},
};
use bedtime_files::FileServices;
use bevy::prelude::Entity;
use content_search::{ContentSearch, ContentSearchAction};
use dear_imgui_rs::Ui;
use replacement::ReplacementJob;
use serde_json::{Value, json};
use std::collections::HashMap;
pub const MODULE_ID: &str = "bedtime.search";
pub const PANEL_ID: &str = "bedtime.search.panel";
pub const SHOW_COMMAND: &str = "bedtime.search.show";
pub const NEW_COMMAND: &str = "bedtime.search.new";
pub const CANCEL_COMMAND: &str = "bedtime.search.cancel";

#[derive(Default)]
pub struct SearchState {
    replacement: Option<ReplacementJob>,
    services: FileServices,
    context_identity: Option<(
        bedtime_files::FileProviderId,
        std::path::PathBuf,
        u64,
        bedtime_documents::SessionId,
    )>,
}

impl SearchState {
    pub fn is_replacing(&self) -> bool {
        self.replacement.is_some()
    }
}

pub struct SearchPanel {
    pub search: ContentSearch,
}

impl Drop for SearchPanel {
    fn drop(&mut self) {
        self.search.dismiss();
    }
}

impl SearchPanel {
    pub fn new(services: FileServices, state: &Value) -> Self {
        let mut search = ContentSearch::new_lazy();
        search.set_file_services(services);
        search.query = state["query"].as_str().unwrap_or_default().into();
        search.case_sensitive = state["case_sensitive"].as_bool().unwrap_or_default();
        search.include_ignored = state["include_ignored"].as_bool().unwrap_or_default();
        search.regex = state["regex"].as_bool().unwrap_or_default();
        search.whole_words = state["whole_words"].as_bool().unwrap_or_default();
        search.include = state["include"].as_str().unwrap_or_default().into();
        search.exclude = state["exclude"].as_str().unwrap_or_default().into();
        search.replacement = state["replacement"].as_str().unwrap_or_default().into();
        search.show_replace = state["show_replace"].as_bool().unwrap_or_default();
        search.show_filters = state["show_filters"].as_bool().unwrap_or_default();
        search.open();
        Self { search }
    }

    pub fn open(&mut self) {
        self.search.open();
    }

    pub fn draw_body(
        &mut self,
        entity: Entity,
        state: &mut SearchState,
        ui: &Ui,
        project_root: &str,
        icons: Option<&bedtime_ui::icons::Icons>,
        requests: &mut Vec<HostAction>,
    ) -> std::io::Result<()> {
        let replacing = state.replacement.is_some();
        let root = if project_root.is_empty() {
            self.search.root.clone()
        } else {
            project_root.to_owned()
        };
        let action = self.search.draw_body(ui, &root, replacing, icons);
        match action {
            ContentSearchAction::Cancel => requests.push(HostAction::Command {
                command: CANCEL_COMMAND.into(),
                context: Default::default(),
            }),
            ContentSearchAction::Open(found) => requests.push(HostAction::RevealSource {
                path: found.file.full_path,
                row: found.row,
                column: found.column,
                center: false,
            }),
            ContentSearchAction::ReplaceNext | ContentSearchAction::ReplaceAll if !replacing => {
                match ReplacementJob::new(
                    entity,
                    &self.search,
                    action == ContentSearchAction::ReplaceAll,
                ) {
                    Ok(job) => {
                        self.search.message = None;
                        state.replacement = Some(job);
                    }
                    Err(error) => self.search.message = Some(error),
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn state(&self) -> Value {
        let search = &self.search;
        json!({"query":search.query,"case_sensitive":search.case_sensitive,"include_ignored":search.include_ignored,"regex":search.regex,"whole_words":search.whole_words,"include":search.include,"exclude":search.exclude,"replacement":search.replacement,"show_replace":search.show_replace,"show_filters":search.show_filters})
    }
}
mod feature;
pub use feature::{FileSearchPlugin, SearchPanels};
#[cfg(test)]
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;

pub(crate) fn shortcut_actions() -> Vec<bedtime_keybindings::ShortcutAction> {
    vec![
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.search.next_result",
            "Next Result",
            MODULE_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["F3"],
        ),
        bedtime_keybindings::ShortcutAction::new(
            "bedtime.search.previous_result",
            "Previous Result",
            MODULE_ID,
            bedtime_keybindings::Scope::panel(PANEL_ID),
            &["Shift+F3"],
        ),
    ]
}

pub(crate) fn default_shortcuts() -> bedtime_keybindings::ShortcutInput {
    bedtime_keybindings::ShortcutInput::new(bedtime_keybindings::Bindings::new(
        shortcut_actions(),
        &Default::default(),
        bedtime_keybindings::Platform::current(),
    ))
    .with_scope(bedtime_keybindings::Scope::panel(PANEL_ID))
}
