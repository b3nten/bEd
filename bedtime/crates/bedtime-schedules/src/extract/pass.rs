//! The [`ScheduleBuildPass`] that captures systems while they are still reachable.
//!
//! Bevy moves every system (and its conditions) out of the [`ScheduleGraph`] into the private
//! executable as the last step of a build, so this pass, which runs just before that, is the only
//! public hook where names, flags, access sets and conditions can all be read together.

use std::{
    any::TypeId,
    collections::HashSet,
    fmt,
    sync::{Arc, Mutex},
};

use bevy::{
    ecs::{
        query::FilteredAccessSet,
        schedule::{
            ApplyDeferred, FlattenedDependencies, InternedScheduleLabel, NodeId,
            ScheduleBuildError, ScheduleBuildPass, ScheduleGraph, SystemKey, SystemSetKey,
            graph::DiGraph,
        },
        system::{System, SystemAccess},
        world::World,
    },
    platform::hash::FixedHasher,
};
use indexmap::IndexSet;

use super::{CaptureMap, RawCondition, RawSchedule, RawSet, RawSystem};

/// Build pass installed on every schedule by the extract plugin.
pub struct CapturePass {
    label: InternedScheduleLabel,
    sink: Arc<Mutex<CaptureMap>>,
}

impl CapturePass {
    pub fn new(label: InternedScheduleLabel, sink: Arc<Mutex<CaptureMap>>) -> Self {
        Self { label, sink }
    }
}

impl fmt::Debug for CapturePass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturePass")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// The components and resources an access names: `None` for the whole world.
pub(super) fn shared(access: &SystemAccess) -> Option<FilteredAccessSet> {
    match access {
        SystemAccess::None => Some(FilteredAccessSet::new()),
        SystemAccess::Shared(set) => Some(set.clone()),
        SystemAccess::Exclusive => None,
    }
}

fn raw_conditions(conditions: &[bevy::ecs::schedule::ConditionWithAccess]) -> Vec<RawCondition> {
    conditions
        .iter()
        .map(|c| RawCondition {
            name: c.condition.name().to_string(),
            access: shared(&c.access),
        })
        .collect()
}

impl ScheduleBuildPass for CapturePass {
    type EdgeOptions = ();

    fn add_dependency(&mut self, _from: NodeId, _to: NodeId, _options: Option<&Self::EdgeOptions>) {
    }

    fn collapse_set(
        &mut self,
        _set: SystemSetKey,
        _systems: &IndexSet<SystemKey, FixedHasher>,
        _dependency_flattening: &DiGraph<NodeId>,
    ) -> impl Iterator<Item = (NodeId, NodeId)> {
        std::iter::empty()
    }

    fn build(
        &mut self,
        _world: &mut World,
        graph: &mut ScheduleGraph,
        mut flat: FlattenedDependencies<'_>,
    ) -> Result<(), ScheduleBuildError> {
        // Collect keys first: `get_mut` needs exclusive access to the container.
        let keys: Vec<SystemKey> = graph.systems.iter().map(|(key, _, _)| key).collect();
        let mut systems = Vec::with_capacity(keys.len());
        for key in keys {
            let conditions = graph
                .systems
                .get_conditions(key)
                .map(raw_conditions)
                .unwrap_or_default();
            let Some(system) = graph.systems.get(key) else {
                continue;
            };
            let flags = system.flags();
            let type_id = system.system_type();
            let apply_deferred = type_id == TypeId::of::<ApplyDeferred>();
            // The graph initialised every system before running its passes, and kept the
            // access each returned: nothing is initialised again here.
            let exclusive = system.access().is_exclusive();
            let access = if apply_deferred {
                None
            } else {
                shared(system.access())
            };
            systems.push(RawSystem {
                key,
                name: system.name().to_string(),
                type_id,
                flags,
                exclusive,
                apply_deferred,
                access,
                conditions,
            });
        }

        let sets = graph
            .system_sets
            .iter()
            .map(|(key, set, conditions)| RawSet {
                key,
                name: format!("{set:?}"),
                anonymous: set.is_anonymous(),
                system_type: set.system_type(),
                conditions: raw_conditions(conditions),
            })
            .collect();
        let hierarchy = graph.hierarchy().graph().all_edges().collect();
        let dependency = graph.dependency().graph().all_edges().collect();
        let flat_edges: Vec<(SystemKey, SystemKey)> = flat.all_edges().collect();
        let toposort = flat
            .toposort()
            .map(|order| order.to_vec())
            .unwrap_or_default();
        let ambiguous_with_all = graph.ambiguous_with_all.iter().copied().collect();

        let mut map = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let build_count = map
            .schedules
            .get(&self.label)
            .map_or(0, |previous| previous.build_count)
            + 1;
        map.schedules.insert(
            self.label,
            RawSchedule {
                label: self.label,
                build_count,
                captured_by_pass: true,
                systems,
                sets,
                hierarchy,
                dependency,
                flat: flat_edges,
                toposort,
                ambiguous_with_all,
                // Filled in by the `ScheduleBuilt` observer, which fires after this pass.
                pass_added_edges: HashSet::new(),
                warnings: Vec::new(),
            },
        );
        map.dirty = true;
        Ok(())
    }
}
