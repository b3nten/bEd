//! Application operations requested by the editor feature. Editor rendering,
//! context menus and language-server presentation live in bed-module-editor.
use super::*;
#[cfg(test)]
use bed_editor_ui::EditorView;
#[cfg(test)]
use bed_module_editor::lsp::lsp_ui::LspAction;
use bed_module_editor::{EditorAction, EditorMenuCommand, EditorRuntime};

impl Workbench {
    /// Adapt existing command contributions to the editor's concrete menu API.
    /// The editor retains the captured selection when invoking a menu command.
    pub(super) fn prepare_editor_menus(&mut self) -> io::Result<()> {
        let runtime = self.modules.editor_runtime.clone();
        let views: Vec<_> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.panel.view_id())
            .collect();
        let descriptors: Vec<_> = self
            .modules
            .registry
            .menus
            .iter()
            .filter(|menu| menu.slot == bed_workbench_api::MenuSlot::TextSelection)
            .filter_map(|menu| self.modules.registry.command(menu.command).cloned())
            .collect();
        for view in views {
            let context = runtime
                .borrow()
                .context(view)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| EditorRuntime::selection_context(&self.session, view))?;
            let commands = descriptors
                .iter()
                .map(|command| EditorMenuCommand {
                    id: command.id.to_owned(),
                    label: command.label.to_owned(),
                    enabled: self.modules.enabled(command.id, &context),
                })
                .collect();
            runtime.borrow_mut().set_menu_commands(view, commands);
        }
        Ok(())
    }

    pub(super) fn process_editor_actions(
        &mut self,
        host_actions: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let actions = self.modules.editor_runtime.borrow_mut().take_actions();
        for action in actions {
            match action {
                EditorAction::Host { view, action } => {
                    if let Some(document) = self.session.document_for_view(view) {
                        self.active = Some(view);
                        self.last_document = Some(document);
                        host_actions.push(action);
                    }
                }
                EditorAction::Navigate { location, origin } => {
                    if let Some(origin) = origin {
                        if !self.session.accepts_lsp_origin(&origin) {
                            continue;
                        }
                        if let Some(index) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.panel.view_id() == Some(origin.view_id))
                        {
                            self.switch_to_tab(index);
                        }
                    }
                    self.navigate_file(&location.file, location.line, location.character, true)?;
                }
                EditorAction::Command { id, context } => self.run_module_command(&id, &context)?,
            }
        }
        self.process_plugin_requests()
    }

    // These small adapters retain the existing native/test entry points while
    // the feature owns request validation, UI state and captured selections.
    #[cfg(test)]
    pub(super) fn capture_editor_context(&mut self, view: &EditorView) -> io::Result<()> {
        self.modules
            .editor_runtime
            .borrow_mut()
            .capture_context(&self.session, view.id())?;
        Ok(())
    }
    pub(super) fn finish_navigation_request(&mut self) -> io::Result<()> {
        let runtime = self.modules.editor_runtime.clone();
        runtime
            .borrow_mut()
            .tick(&mut self.session, &mut self.modules.requests)?;
        let mut actions = Vec::new();
        self.process_editor_actions(&mut actions)?;
        for action in actions {
            self.handle_action(action)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn open_lsp_action(&mut self, action: LspAction) -> io::Result<()> {
        let runtime = self.modules.editor_runtime.clone();
        runtime.borrow_mut().open_lsp_action(
            &mut self.session,
            action,
            &mut self.modules.requests,
        )?;
        self.process_editor_actions(&mut Vec::new())
    }
}
