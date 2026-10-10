//! Workspace HTTP client. Only explicit Run actions send requests.
mod model;
mod panel;
mod worker;

use bed_editing::identity::DocumentId;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, ModuleServices,
    PanelPlacement, Registrar,
};
use model::{Request, ResolvedRequest, SavedWorkspace};
use serde_json::Value;
use std::{any::Any, cell::RefCell, collections::HashMap, rc::Rc};

pub use panel::HttpPanel;

pub const MODULE_ID: &str = "bed.http";
pub const PANEL_ID: &str = "bed.http.panel";
pub const OPEN_COMMAND: &str = "bed.http.open";

#[derive(Default)]
struct Shared {
    requests: Vec<Request>,
    selected: Option<u64>,
    results: HashMap<u64, RunResult>,
    active: Option<ActiveRun>,
    actions: Vec<Action>,
    error: Option<String>,
}

struct RunResult {
    run: u64,
    request: ResolvedRequest,
    outcome: Result<worker::Response, String>,
}

struct ActiveRun {
    run: u64,
    id: u64,
    request: ResolvedRequest,
}

enum Action {
    Create,
    Select { id: u64 },
    Update { request: Request },
    Delete { id: u64 },
    Run { id: u64 },
    Cancel,
}

// The same catalog transition builds live state and a save snapshot of queued UI edits.
// Run and Cancel have no catalog effect, so saving can never start network work.
fn apply_edit(
    action: &Action,
    requests: &mut Vec<Request>,
    selected: &mut Option<u64>,
    next_request: &mut u64,
) -> Result<bool, String> {
    match action {
        Action::Create => {
            let id = next_request
                .checked_add(1)
                .ok_or("Cannot create another HTTP request: ID limit reached")?;
            *next_request = id;
            requests.push(Request::new(id));
            *selected = Some(id);
        }
        Action::Select { id } => {
            if !requests.iter().any(|request| request.id == *id) {
                return Ok(false);
            }
            *selected = Some(*id);
        }
        Action::Update { request } => {
            let Some(current) = requests.iter_mut().find(|current| current.id == request.id) else {
                return Ok(false);
            };
            *current = request.clone();
        }
        Action::Delete { id } => {
            let Some(index) = requests.iter().position(|request| request.id == *id) else {
                return Ok(false);
            };
            requests.remove(index);
            if *selected == Some(*id) {
                *selected = requests
                    .get(index)
                    .or_else(|| requests.last())
                    .map(|request| request.id);
            }
        }
        Action::Run { .. } | Action::Cancel => return Ok(false),
    }
    Ok(true)
}

/// Owns saved definitions and execution independently of the panel's lifetime.
pub struct HttpModule {
    shared: Rc<RefCell<Shared>>,
    worker: worker::Worker,
    next_request: u64,
    next_run: u64,
    // Preserve unsupported/corrupt saved data until an explicit new catalog is created.
    unrestored: Option<Value>,
}

impl Default for HttpModule {
    fn default() -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared::default())),
            worker: worker::Worker::new(),
            next_request: 0,
            next_run: 0,
            unrestored: None,
        }
    }
}

impl Module for HttpModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }

    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(PANEL_ID, "HTTP Client", true, PanelPlacement::Center, None);
        registrar.command(OPEN_COMMAND, "Open HTTP Client", None);
        registrar.menu(MenuSlot::Application, OPEN_COMMAND);
    }

    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == OPEN_COMMAND {
            requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            });
        }
    }

    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID {
            return Err(format!("Unknown HTTP panel: {panel_type}"));
        }
        if document.is_some() {
            return Err("HTTP Client is a workspace panel and does not attach to files".into());
        }
        Ok(Box::new(HttpPanel::new(self.shared.clone(), state)))
    }

    fn restore_workspace(&mut self, state: Option<&Value>, _: &str) {
        // Disconnect old completions and cancel all old runs before restoring definitions.
        self.worker = worker::Worker::new();
        let mut shared = self.shared.borrow_mut();
        *shared = Shared::default();
        self.next_request = 0;
        self.unrestored = None;
        match model::load(state) {
            Ok(saved) => {
                self.next_request = saved
                    .requests
                    .iter()
                    .map(|request| request.id)
                    .max()
                    .unwrap_or(0);
                shared.requests = saved.requests;
                shared.selected = saved.selected;
            }
            Err(error) => {
                shared.error = Some(format!("Could not restore HTTP requests: {error}"));
                self.unrestored = state.cloned();
            }
        }
    }

    fn save_workspace(&self) -> Value {
        let shared = self.shared.borrow();
        let mut saved = SavedWorkspace {
            version: 1,
            requests: shared.requests.clone(),
            selected: shared.selected,
        };
        let mut next_request = self.next_request;
        let mut created = false;
        // A close may persist immediately after draw, before the next module tick.
        for action in &shared.actions {
            let applied = apply_edit(
                action,
                &mut saved.requests,
                &mut saved.selected,
                &mut next_request,
            )
            .unwrap_or(false);
            created |= applied && matches!(action, Action::Create);
        }
        if !created && let Some(state) = &self.unrestored {
            return state.clone();
        }
        serde_json::to_value(saved).expect("HTTP request definitions serialize")
    }

    fn tick(&mut self, _: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let mut shared = self.shared.borrow_mut();
        let mut changed = false;

        // One owner applies edits in order and captures the exact input sent by Run.
        for action in std::mem::take(&mut shared.actions) {
            changed = true;
            let applied = {
                let Shared {
                    requests, selected, ..
                } = &mut *shared;
                apply_edit(&action, requests, selected, &mut self.next_request)
            };
            match applied {
                Ok(true) => shared.error = None,
                Ok(false) => {}
                Err(error) => {
                    shared.error = Some(error);
                    continue;
                }
            }
            match action {
                Action::Create => {
                    self.unrestored = None;
                }
                Action::Select { .. } | Action::Update { .. } => {}
                Action::Delete { id } => {
                    if shared.active.as_ref().is_some_and(|active| active.id == id) {
                        self.worker.cancel(
                            shared
                                .active
                                .take()
                                .expect("active request was checked")
                                .run,
                        );
                    }
                    shared.results.remove(&id);
                }
                Action::Run { id } => {
                    if shared.active.is_some() {
                        continue;
                    }
                    let Some(definition) = shared.requests.iter().find(|request| request.id == id)
                    else {
                        continue;
                    };
                    let request = match model::resolve(definition) {
                        Ok(request) => request,
                        Err(error) => {
                            shared.error = Some(error);
                            continue;
                        }
                    };
                    self.next_run = self.next_run.checked_add(1).expect("HTTP run ID overflow");
                    match self.worker.start(self.next_run, request.clone()) {
                        Ok(()) => {
                            shared.active = Some(ActiveRun {
                                run: self.next_run,
                                id,
                                request,
                            });
                            shared.error = None;
                        }
                        Err(error) => shared.error = Some(error),
                    }
                }
                Action::Cancel => {
                    if let Some(active) = shared.active.take() {
                        self.worker.cancel(active.run);
                        shared.results.insert(
                            active.id,
                            RunResult {
                                run: active.run,
                                request: active.request,
                                outcome: Err("Request cancelled".into()),
                            },
                        );
                    }
                }
            }
        }

        // Completion identity belongs to the captured request, never current selection.
        for completion in self.worker.poll() {
            if shared
                .active
                .as_ref()
                .is_some_and(|active| active.run == completion.run)
            {
                let active = shared
                    .active
                    .take()
                    .expect("completion matched an active run");
                shared.results.insert(
                    active.id,
                    RunResult {
                        run: active.run,
                        request: active.request,
                        outcome: completion.result,
                    },
                );
                changed = true;
            }
        }
        if changed {
            requests.push(HostRequest::Invalidate);
        }
    }

    fn shutdown(&mut self, _: &mut ModuleServices<'_>) {
        self.worker = worker::Worker::new();
        *self.shared.borrow_mut() = Shared::default();
        self.unrestored = None;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
