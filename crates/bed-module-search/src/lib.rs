//! Project search module. Every panel owns its query, results and search worker.
pub mod content_search;

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
        self.shared.borrow_mut().remote = remote.clone();
        for search in self.states() {
            search.borrow_mut().set_remote_client(remote.clone());
        }
    }
    pub fn cancel_all(&self) {
        for search in self.states() {
            search.borrow_mut().cancel();
        }
    }
    pub fn is_searching(&self) -> bool {
        self.states().iter().any(|search| search.borrow().searching)
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
            search.borrow_mut().cancel();
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
    fn tick(&mut self, _: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        for search in self.handle.states() {
            if search.borrow_mut().poll() {
                requests.push(HostRequest::Invalidate);
            }
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
        if let ContentSearchAction::Open(found) = self
            .search
            .borrow_mut()
            .draw_body(ui, services.project_root)
        {
            requests.push(HostRequest::RevealSource {
                path: found.file.full_path,
                row: found.row,
                column: found.column,
                center: false,
            });
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
        json!({"query":search.query,"case_sensitive":search.case_sensitive,"include_ignored":search.include_ignored})
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
                &json!({"query":"second","case_sensitive":true}),
            )
            .unwrap();
        assert_eq!(handle.panel_count(), 2);
        drop(first);
        assert_eq!(handle.panel_count(), 1);
        let second = second.as_any().downcast_ref::<SearchPanel>().unwrap();
        assert_eq!(second.borrow_search().query, "second");
        assert!(second.borrow_search().case_sensitive);
        assert_eq!(second.save_state()["query"], "second");
    }
}
