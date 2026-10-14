//! HTTP client with host-owned catalog persistence. Only explicit Run actions send requests.
mod ecs;
mod model;
mod panel;
mod status;
mod worker;
pub use ecs::{HttpClientPlugin, HttpPersistence, HttpRuntime, HttpSurface};

use bedtime_api::{
    features::{FeatureRegistrar, HostAction, MenuSlot},
    workbench::PanelPlacement,
};
use model::{Request, ResolvedRequest, SavedWorkspace};
use serde_json::Value;
use std::collections::HashMap;

pub use panel::HttpView;

pub const MODULE_ID: &str = "bedtime.http";
pub const PANEL_ID: &str = "bedtime.http.panel";
pub const OPEN_COMMAND: &str = "bedtime.http.open";

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_options(PANEL_ID, true, PanelPlacement::Center);
    registrar.command_ref(OPEN_COMMAND);
    registrar.menu(MenuSlot::Application, OPEN_COMMAND);
}

#[derive(Default)]
struct Shared {
    requests: Vec<Request>,
    selected: Option<u64>,
    results: HashMap<u64, RunResult>,
    active: Option<ActiveRun>,
    actions: Vec<HttpAction>,
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

enum HttpAction {
    Create,
    Select { id: u64 },
    Update { request: Request },
    Delete { id: u64 },
    Run { id: u64 },
    Cancel,
    CancelRun { run: u64 },
}

// The same catalog transition builds live state and a save snapshot of queued UI edits.
// Run and Cancel have no catalog effect, so saving can never start network work.
fn apply_edit(
    action: &HttpAction,
    requests: &mut Vec<Request>,
    selected: &mut Option<u64>,
    next_request: &mut u64,
) -> Result<bool, String> {
    match action {
        HttpAction::Create => {
            let id = next_request
                .checked_add(1)
                .ok_or("Cannot create another HTTP request: ID limit reached")?;
            *next_request = id;
            requests.push(Request::new(id));
            *selected = Some(id);
        }
        HttpAction::Select { id } => {
            if !requests.iter().any(|request| request.id == *id) {
                return Ok(false);
            }
            *selected = Some(*id);
        }
        HttpAction::Update { request } => {
            let Some(current) = requests.iter_mut().find(|current| current.id == request.id) else {
                return Ok(false);
            };
            *current = request.clone();
        }
        HttpAction::Delete { id } => {
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
        HttpAction::Run { .. } | HttpAction::Cancel | HttpAction::CancelRun { .. } => {
            return Ok(false);
        }
    }
    Ok(true)
}

fn restore_catalog(
    shared: &mut Shared,
    worker: &mut worker::Worker,
    next_request: &mut u64,
    unrestored: &mut Option<Value>,
    state: Option<&Value>,
) {
    // Disconnect old completions and cancel all old runs before restoring definitions.
    *worker = worker::Worker::new();
    *shared = Shared::default();
    *next_request = 0;
    *unrestored = None;
    match model::load(state) {
        Ok(saved) => {
            *next_request = saved
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
            *unrestored = state.cloned();
        }
    }
}

fn saved_catalog(shared: &Shared, next_request: u64, unrestored: Option<&Value>) -> Value {
    let mut saved = SavedWorkspace {
        version: 1,
        requests: shared.requests.clone(),
        selected: shared.selected,
    };
    let mut next_request = next_request;
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
        created |= applied && matches!(action, HttpAction::Create);
    }
    if !created && let Some(state) = unrestored {
        return state.clone();
    }
    serde_json::to_value(saved).expect("HTTP request definitions serialize")
}

fn update_catalog(
    shared: &mut Shared,
    worker: &mut worker::Worker,
    next_request: &mut u64,
    next_run: &mut u64,
    unrestored: &mut Option<Value>,
    requests: &mut Vec<HostAction>,
) {
    let mut changed = false;

    // One owner applies edits in order and captures the exact input sent by Run.
    for action in std::mem::take(&mut shared.actions) {
        changed = true;
        let applied = {
            let Shared {
                requests, selected, ..
            } = &mut *shared;
            apply_edit(&action, requests, selected, &mut *next_request)
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
            HttpAction::Create => {
                *unrestored = None;
            }
            HttpAction::Select { .. } | HttpAction::Update { .. } => {}
            HttpAction::Delete { id } => {
                if shared.active.as_ref().is_some_and(|active| active.id == id) {
                    worker.cancel(
                        shared
                            .active
                            .take()
                            .expect("active request was checked")
                            .run,
                    );
                }
                shared.results.remove(&id);
            }
            HttpAction::Run { id } => {
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
                *next_run = next_run.checked_add(1).expect("HTTP run ID overflow");
                match worker.start(*next_run, request.clone()) {
                    Ok(()) => {
                        shared.active = Some(ActiveRun {
                            run: *next_run,
                            id,
                            request,
                        });
                        shared.error = None;
                    }
                    Err(error) => shared.error = Some(error),
                }
            }
            HttpAction::Cancel | HttpAction::CancelRun { .. } => {
                if let HttpAction::CancelRun { run } = action
                    && !shared
                        .active
                        .as_ref()
                        .is_some_and(|active| active.run == run)
                {
                    continue;
                }
                if let Some(active) = shared.active.take() {
                    worker.cancel(active.run);
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
    for completion in worker.poll() {
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
        requests.push(HostAction::Invalidate);
    }
}

#[cfg(test)]
mod tests;
