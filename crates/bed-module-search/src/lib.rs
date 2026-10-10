//! Project search module. Every panel owns its query, results and search worker.
pub mod content_search;
mod replacement;
use replacement::ReplacementJob;

use bed_remote::RemoteClient;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, Module, ModulePanel, PanelAction, PanelPlacement,
    Registrar,
};
use content_search::{ContentSearch, ContentSearchAction};
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    any::Any,
    cell::{RefCell, RefMut},
    collections::HashMap,
    rc::{Rc, Weak},
};

pub const MODULE_ID: &str = "bed.search";
pub const PANEL_ID: &str = "bed.search.panel";
pub const SHOW_COMMAND: &str = "bed.search.show";
pub const NEW_COMMAND: &str = "bed.search.new";

#[derive(Default)]
struct Shared {
    next: u64,
    replacement: Option<ReplacementJob>,
    remote: Option<RemoteClient>,
    panels: HashMap<u64, Weak<RefCell<ContentSearch>>>,
}
#[derive(Clone, Default)]
pub struct SearchHandle {
    shared: Rc<RefCell<Shared>>,
}
impl SearchHandle {
    pub fn states(&self) -> Vec<Rc<RefCell<ContentSearch>>> {
        self.shared
            .borrow()
            .panels
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }
    pub fn set_remote_client(&self, remote: Option<RemoteClient>) {
        {
            let mut shared = self.shared.borrow_mut();
            shared.replacement = None;
            shared.remote = remote.clone();
        }
        for search in self.states() {
            let mut search = search.borrow_mut();
            search.set_remote_client(remote.clone());
            search.invalidate();
        }
    }
    pub fn cancel_all(&self) {
        self.shared.borrow_mut().replacement = None;
        for search in self.states() {
            search.borrow_mut().cancel();
        }
    }
    pub fn is_searching(&self) -> bool {
        self.shared.borrow().replacement.is_some()
            || self
                .states()
                .iter()
                .any(|search| search.borrow().needs_tick())
    }
    pub fn panel_count(&self) -> usize {
        self.shared.borrow().panels.len()
    }
    fn unregister(&self, id: u64) {
        if let Some(search) = self
            .shared
            .borrow_mut()
            .panels
            .remove(&id)
            .and_then(|entry| entry.upgrade())
        {
            search.borrow_mut().dismiss();
        }
    }
}

pub struct SearchModule {
    handle: SearchHandle,
}
impl SearchModule {
    pub fn new(handle: SearchHandle) -> Self {
        Self { handle }
    }
    pub fn handle(&self) -> &SearchHandle {
        &self.handle
    }
}
impl Module for SearchModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.command(SHOW_COMMAND, "Find in Project", None);
        registrar.command(NEW_COMMAND, "New Search Panel", None);
        registrar.panel_options(
            PANEL_ID,
            "Search",
            false,
            PanelPlacement::Center,
            Some("search"),
        );
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        match command {
            SHOW_COMMAND => requests.push(HostRequest::ShowPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
                action: Some(PanelAction::Find),
            }),
            NEW_COMMAND => requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            }),
            _ => {}
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        _: Option<bed_document_session::DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown project search panel: {panel_type}"));
        }
        let mut search = ContentSearch::new_lazy();
        search.set_remote_client(self.handle.shared.borrow().remote.clone());
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
        let search = Rc::new(RefCell::new(search));
        let id = {
            let mut shared = self.handle.shared.borrow_mut();
            shared.next += 1;
            let id = shared.next;
            shared.panels.insert(id, Rc::downgrade(&search));
            id
        };
        Ok(Box::new(SearchPanel {
            id,
            search,
            handle: self.handle.clone(),
        }))
    }
    fn tick_with_services(
        &mut self,
        _: &HostContext<'_>,
        services: &mut bed_workbench_api::ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> std::io::Result<()> {
        for search in self.handle.states() {
            if self
                .handle
                .shared
                .borrow()
                .replacement
                .as_ref()
                .is_some_and(|job| job.owns(&search))
            {
                continue;
            }
            let mut search = search.borrow_mut();
            let root = if services.project_root.is_empty() {
                search.root.clone()
            } else {
                services.project_root.to_owned()
            };
            if search.tick(&root, services.documents) {
                requests.push(HostRequest::Invalidate);
            }
        }
        let mut shared = self.handle.shared.borrow_mut();
        if let Some(job) = &mut shared.replacement {
            if job.tick(services.documents, requests) {
                shared.replacement = None;
            }
            requests.push(HostRequest::Invalidate);
        }
        Ok(())
    }
    fn create_panel_with_services(
        &mut self,
        kind: &str,
        input: bed_workbench_api::PanelInput,
        state: &Value,
        services: &mut bed_workbench_api::ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if input.local_file().is_some() || input.document().is_some() {
            return Err("Folder search does not attach to files".into());
        }
        let panel = self.create_panel(kind, None, state)?;
        panel
            .as_any()
            .downcast_ref::<SearchPanel>()
            .unwrap()
            .search
            .borrow_mut()
            .root = services.working_directory.into();
        Ok(panel)
    }
    fn document_events(
        &mut self,
        _: &bed_document_session::EditorSession,
        events: &[bed_document_session::SessionEvent],
    ) -> std::io::Result<()> {
        if events.iter().any(|event| {
            matches!(
                event,
                bed_document_session::SessionEvent::Edited { .. }
                    | bed_document_session::SessionEvent::Reloaded { .. }
                    | bed_document_session::SessionEvent::Removed { .. }
                    | bed_document_session::SessionEvent::PathChanged { .. }
                    | bed_document_session::SessionEvent::Closed { .. }
            )
        }) {
            for search in self.handle.states() {
                if !self
                    .handle
                    .shared
                    .borrow()
                    .replacement
                    .as_ref()
                    .is_some_and(|job| job.owns(&search))
                {
                    search.borrow_mut().invalidate();
                }
            }
        }
        Ok(())
    }
    fn save_result(
        &mut self,
        token: bed_workbench_api::SaveToken,
        result: Result<Vec<bed_workbench_api::SavedDocument>, String>,
    ) {
        if let Some(job) = &mut self.handle.shared.borrow_mut().replacement {
            job.save_result(token, result);
        }
    }
    fn edit_result(
        &mut self,
        token: bed_workbench_api::EditToken,
        result: Result<bed_workbench_api::Revision, String>,
    ) {
        if let Some(job) = &mut self.handle.shared.borrow_mut().replacement {
            job.edit_result(token, result);
        }
    }
    fn shutdown(&mut self, _: &mut bed_workbench_api::ModuleServices<'_>) {
        self.handle.cancel_all();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct SearchPanel {
    id: u64,
    search: Rc<RefCell<ContentSearch>>,
    handle: SearchHandle,
}
impl SearchPanel {
    pub fn search(&self) -> Rc<RefCell<ContentSearch>> {
        self.search.clone()
    }
    pub fn borrow_search(&self) -> RefMut<'_, ContentSearch> {
        self.search.borrow_mut()
    }
    pub fn open(&self) {
        self.search.borrow_mut().open();
    }
}
impl Drop for SearchPanel {
    fn drop(&mut self) {
        self.handle.unregister(self.id);
    }
}
impl ModulePanel for SearchPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Search".into()
    }
    fn draw(&mut self, _: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {}
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut bed_workbench_api::ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> std::io::Result<()> {
        let replacing = self.handle.shared.borrow().replacement.is_some();
        let icons = services.resources.take::<bed_ui::icons::Icons>();
        let root = self.search.borrow().root.clone();
        let root = if services.project_root.is_empty() {
            &root
        } else {
            services.project_root
        };
        if services.project_root.is_empty() {
            bed_ui::presentation::directory_label(ui, root);
        }
        let action = self
            .search
            .borrow_mut()
            .draw_body(ui, root, replacing, icons.as_deref());
        match action {
            ContentSearchAction::Open(found) => requests.push(HostRequest::RevealSource {
                path: found.file.full_path,
                row: found.row,
                column: found.column,
                center: false,
            }),
            ContentSearchAction::ReplaceNext | ContentSearchAction::ReplaceAll if !replacing => {
                match ReplacementJob::new(
                    self.search.clone(),
                    action == ContentSearchAction::ReplaceAll,
                ) {
                    Ok(job) => self.handle.shared.borrow_mut().replacement = Some(job),
                    Err(error) => self.search.borrow_mut().message = Some(error),
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn action(
        &mut self,
        action: PanelAction,
        _: &HostContext<'_>,
        _: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        if action == PanelAction::Find {
            self.open();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn save_state(&self) -> Value {
        let search = self.search.borrow();
        json!({"query":search.query,"case_sensitive":search.case_sensitive,"include_ignored":search.include_ignored,
            "regex":search.regex,"whole_words":search.whole_words,"include":search.include,"exclude":search.exclude,
            "replacement":search.replacement,"show_replace":search.show_replace,"show_filters":search.show_filters})
    }
    fn close(&mut self, _: &mut Vec<HostRequest>) {
        self.handle.unregister(self.id);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_panel_retains_its_own_query_and_closing_one_cancels_only_its_worker() {
        let handle = SearchHandle::default();
        let mut module = SearchModule::new(handle.clone());
        let first = module
            .create_panel(PANEL_ID, None, &json!({"query":"first"}))
            .unwrap();
        let second = module
            .create_panel(
                PANEL_ID,
                None,
                &json!({"query":"second","case_sensitive":true,"regex":true,"whole_words":true,
                    "include":"*.rs","exclude":"vendor/","replacement":"$1","show_replace":true,"show_filters":true}),
            )
            .unwrap();
        assert_eq!(handle.panel_count(), 2);
        drop(first);
        assert_eq!(handle.panel_count(), 1);
        let second = second.as_any().downcast_ref::<SearchPanel>().unwrap();
        assert_eq!(second.borrow_search().query, "second");
        assert!(second.borrow_search().case_sensitive);
        assert_eq!(second.save_state()["query"], "second");
        let restored = module
            .create_panel(PANEL_ID, None, &second.save_state())
            .unwrap();
        let restored = restored.as_any().downcast_ref::<SearchPanel>().unwrap();
        let restored = restored.borrow_search();
        assert!(
            restored.regex
                && restored.whole_words
                && restored.show_replace
                && restored.show_filters
        );
        assert_eq!(restored.include, "*.rs");
        assert_eq!(restored.exclude, "vendor/");
        assert_eq!(restored.replacement, "$1");
    }
}

#[cfg(test)]
#[path = "../../../tests/support/temp_dir.rs"]
mod test_support;
