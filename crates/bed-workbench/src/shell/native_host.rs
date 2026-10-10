//! Application services used by native feature modules. Feature rendering and
//! per-panel state live in modules; this adapter executes workspace mutations.
use super::*;
use bed_module_explorer::ExplorerAction;
use bed_module_projects::ProjectsAction;

impl Workbench {
    pub(super) fn file_explorer(&self) -> std::cell::RefMut<'_, FileExplorer> {
        self.modules.explorer.borrow_mut()
    }
    #[cfg(test)]
    pub(super) fn search_panel(
        &self,
        id: u64,
    ) -> Option<std::cell::RefMut<'_, bed_module_search::content_search::ContentSearch>> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)?
            .panel
            .instance
            .as_any()
            .downcast_ref::<bed_module_search::SearchPanel>()
            .map(|panel| panel.borrow_search())
    }
    pub(super) fn refresh_projects(&mut self) {
        let mut state = self.modules.projects.borrow_mut();
        state.recent_workspaces = self
            .store
            .as_ref()
            .map(|store| store.recent_workspaces())
            .unwrap_or_default();
        state.connecting = self.remote_ui.connecting();
        state.disconnected = self.session.is_remote() && !self.session.remote_connected();
    }
    pub(super) fn configure_native_modules(&mut self, ui: &Ui) {
        self.refresh_projects();
        self.modules.explorer.configure_presentation(
            bed_module_explorer::file_tree::FileTreeStyle {
                text_color: self.settings.text_color(),
                rainbow: self.settings.rainbow(),
                rainbow_time: ui.time() as f32,
                animations: self.settings.bool("ui_animations", true),
            },
            bed_module_explorer::file_finder::FileFinderStyle {
                background_color: self.settings.background_color(),
                embedded_pane: None,
            },
            &self.icons,
            self.workspace_spec
                .as_ref()
                .map(|spec| spec.identity())
                .unwrap_or_default(),
        );
        self.modules.explorer.update_menu_model(
            &self.modules.registry,
            &self.modules.frame.context(),
            |command, context| self.modules.enabled(command, context),
        );
        let mut config = self.modules.editor_config.borrow_mut();
        config.navigation_animations = self.settings.bool("ui_animations", true);
        config.options.rainbow_mode = self.settings.rainbow();
        config.options.minimap_enabled = self.settings.bool("minimap", true);
        config.options.background_color = Some(self.settings.background_color());
        config.options.line_jump_key = self.settings.keybinds.get_action_key("line_jump_key");
        config.options.block_input = self
            .file_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.visible)
            || self.reload_confirmation.is_some()
            || self.file_operations.modal_visible()
            || self.modules.explorer.finder_visible();
        config.lsp = bed_module_editor::presentation::LspPresentationOptions {
            background_color: self.settings.background_color(),
            embedded: self.settings.is_embedded,
            symbol_key: self.settings.keybinds.get_action_key("lsp_symbol_info"),
            definition_key: self.settings.keybinds.get_action_key("lsp_find_def"),
            references_key: self.settings.keybinds.get_action_key("lsp_find_ref"),
        };
    }
    pub(super) fn process_native_actions(
        &mut self,
        host_actions: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        for action in self.modules.explorer.take_actions() {
            let result = match action {
                ExplorerAction::Tree(action) => self.handle_tree_action(action),
                ExplorerAction::Import { paths, destination } => {
                    self.import_files(paths, destination)
                }
                ExplorerAction::OpenProjectDialog => {
                    self.dispatch(WindowCommand::OpenFolder).map(|_| ())
                }
                ExplorerAction::PreferencesChanged(preferences) => {
                    if let (Some(store), Some(spec)) = (&mut self.store, &self.workspace_spec) {
                        store.set_module_settings(
                            spec,
                            bed_module_explorer::MODULE_ID,
                            preferences.to_value(),
                        )
                    } else {
                        Ok(())
                    }
                }
                ExplorerAction::RefreshRemote { force } => {
                    if self.session.is_remote() {
                        self.queue_remote_directories(force)
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        let actions = self.modules.projects.borrow_mut().take_actions();
        for action in actions {
            let result = match action {
                ProjectsAction::OpenFolder => self.dispatch(WindowCommand::OpenFolder).map(|_| ()),
                ProjectsAction::OpenProject(path) => self.set_project(&path).map(|_| ()),
                ProjectsAction::OpenWorkspace(spec) => self.set_workspace(spec).map(|_| ()),
                ProjectsAction::OpenWorkspaceWindow(spec) => self.request_workspace_window(spec),
                ProjectsAction::Reconnect => self.reconnect_workspace().map(|_| ()),
                ProjectsAction::RemoveRecent(path) => self
                    .store
                    .as_mut()
                    .map_or(Ok(()), |store| store.forget_project(&path)),
                ProjectsAction::RemoveWorkspace(spec) => self
                    .store
                    .as_mut()
                    .map_or(Ok(()), |store| store.forget_workspace(&spec)),
                ProjectsAction::RenameWorkspace(spec, name) => {
                    if let Some(store) = &mut self.store {
                        store.rename_workspace(&spec, &name).map(|renamed| {
                            if self
                                .workspace_spec
                                .as_ref()
                                .is_some_and(|active| active.identity() == renamed.identity())
                            {
                                self.workspace_spec = Some(renamed);
                            }
                        })
                    } else {
                        Ok(())
                    }
                }
                ProjectsAction::Error(message) => {
                    self.error = Some(message);
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        self.refresh_projects();
        self.process_editor_actions(host_actions)
    }
}
