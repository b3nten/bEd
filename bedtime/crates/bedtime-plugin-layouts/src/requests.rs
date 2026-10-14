//! One FIFO owns admitted switches and their completion, including save barriers.
use super::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayoutRequestId(pub u64);

struct Request<Key> {
    id: LayoutRequestId,
    action: Action<Key>,
}
enum Action<Key> {
    Switch { target: Key, snapshot: Value },
    Rearrange(LayoutDefinition),
}

#[derive(Resource)]
struct Requests<Key: LayoutKey> {
    pending: VecDeque<Request<Key>>,
    results: HashMap<LayoutRequestId, Result<(), String>>,
}
impl<Key: LayoutKey> Default for Requests<Key> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            results: HashMap::new(),
        }
    }
}

/// Capture the fallback definition now. Retained or saved instances still take
/// precedence at execution. Switches are application-wide, not panel-scoped.
pub fn submit_transition<Key: LayoutKey, Integration: LayoutIntegration>(
    world: &mut World,
    target: Key,
    snapshot: Value,
) -> LayoutRequestId {
    world.init_resource::<Requests<Key>>();
    let id = LayoutRequestId(bedtime_api::features::next_request_sequence());
    world
        .resource_mut::<Requests<Key>>()
        .pending
        .push_back(Request {
            id,
            action: Action::Switch { target, snapshot },
        });
    poll::<Key, Integration>(world);
    id
}

pub fn submit_rearrange<Key: LayoutKey, Integration: LayoutIntegration>(
    world: &mut World,
    definition: LayoutDefinition,
) -> LayoutRequestId {
    world.init_resource::<Requests<Key>>();
    let id = LayoutRequestId(bedtime_api::features::next_request_sequence());
    world
        .resource_mut::<Requests<Key>>()
        .pending
        .push_back(Request {
            id,
            action: Action::Rearrange(definition),
        });
    poll::<Key, Integration>(world);
    id
}

/// Call between frames in the same controlled stage as other context requests.
pub fn poll<Key: LayoutKey, Integration: LayoutIntegration>(world: &mut World) {
    world.init_resource::<Requests<Key>>();
    while Integration::ready(world) {
        let Some(request) = world.resource_mut::<Requests<Key>>().pending.pop_front() else {
            break;
        };
        let result = match request.action {
            Action::Switch { target, snapshot } => {
                transition::<Key, Integration>(world, target, &snapshot)
            }
            Action::Rearrange(definition) => rearrange::<Integration>(world, &definition),
        }
        .map_err(|error| error.to_string());
        world
            .resource_mut::<Requests<Key>>()
            .results
            .insert(request.id, result);
    }
}

pub fn take_result<Key: LayoutKey>(
    world: &mut World,
    id: LayoutRequestId,
) -> Option<Result<(), String>> {
    world
        .get_resource_mut::<Requests<Key>>()?
        .results
        .remove(&id)
}

/// App-owned operations such as writing a default definition can share the same
/// receipt API as transitions without creating a second completion store.
pub fn completed_request<Key: LayoutKey>(
    world: &mut World,
    result: Result<(), String>,
) -> LayoutRequestId {
    world.init_resource::<Requests<Key>>();
    let id = LayoutRequestId(bedtime_api::features::next_request_sequence());
    world
        .resource_mut::<Requests<Key>>()
        .results
        .insert(id, result);
    id
}

/// A replaced workspace cancels intentions admitted against its former scope.
pub fn cancel_requests<Key: LayoutKey>(world: &mut World, reason: &str) {
    if let Some(mut requests) = world.get_resource_mut::<Requests<Key>>() {
        let pending = std::mem::take(&mut requests.pending);
        for request in pending {
            requests.results.insert(request.id, Err(reason.to_owned()));
        }
    }
}
