//! Extraction of schedule data from a running [`App`].
//!
//! A system's access, conditions and name are all at hand inside a
//! [`ScheduleBuildPass`](bevy::ecs::schedule::ScheduleBuildPass), with the ordering as it was
//! flattened, so [`pass::CapturePass`] records everything it can while a schedule is being built. [`refresh_snapshot`] then turns those raw
//! captures into an [`AppSnapshot`] once per change.

pub mod access;
pub mod pass;

use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

use bevy::log::{debug, warn};
use bevy::{
    app::{App, FixedMainScheduleOrder, Main, MainScheduleOrder, Plugin},
    ecs::{
        component::ComponentId,
        entity_disabling::DefaultQueryFilters,
        observer::On,
        query::FilteredAccessSet,
        resource::Resource,
        schedule::{
            ApplyDeferred, InternedScheduleLabel, IntoScheduleConfigs, NodeId, Schedule,
            ScheduleBuilt, Schedules, SystemKey, SystemSetKey,
        },
        system::{Res, System, SystemStateFlags},
        world::World,
    },
    utils::prelude::ShortName,
};

use crate::snapshot::{
    AppSnapshot, ConditionSnapshot, Conflict, FlatEdge, NodeRef, OrderKind, ScheduleSnapshot,
    SetIx, SetSnapshot, SysIx, SystemFlags, SystemSnapshot,
};
use access::{ComponentInterner, ConvertedAccess, convert_access, is_reserved};
use pass::CapturePass;

/// Settings for the extraction layer.
#[derive(Resource, Clone, Debug)]
pub struct ExtractConfig {
    /// Build every schedule on the first frame so that schedules which have not run yet
    /// (`OnEnter(..)`, `FixedUpdate`, ...) are captured too. Runs their parameters'
    /// `init_state` earlier than normal.
    pub eager_initialize: bool,
    /// Only capture schedules whose `Debug` name is in this list (`None` = all).
    pub schedule_filter: Option<Vec<String>>,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            eager_initialize: true,
            schedule_filter: None,
        }
    }
}

/// Captures schedule data without any viewer. Added automatically by
/// [`BevyGraphPlugin`](crate::BevyGraphPlugin); use it directly for headless tools and tests.
#[derive(Default)]
pub struct ScheduleExtractPlugin {
    pub config: ExtractConfig,
}

impl Plugin for ScheduleExtractPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.config.clone())
            .init_resource::<RawCaptures>()
            .init_resource::<AppSnapshot>()
            .add_observer(on_schedule_built)
            // Before the sub-schedules: eager initialisation and the first snapshot.
            // After them: pick up rebuilds that happened during this frame.
            .add_systems(
                Main,
                (
                    refresh_snapshot.before(Main::run_main),
                    refresh_snapshot_late.after(Main::run_main),
                ),
            );
    }

    /// Every plugin has been built but no schedule has been, so this is the moment to attach the
    /// capture pass to every schedule (including `Main`, which is otherwise never visible).
    fn finish(&self, app: &mut App) {
        let captures = app.world().resource::<RawCaptures>().clone();
        let mut schedules = app.world_mut().resource_mut::<Schedules>();
        let mut attached = Vec::new();
        for (_, schedule) in schedules.iter_mut() {
            let label = schedule.label();
            schedule.add_build_pass(CapturePass::new(label, captures.0.clone()));
            attached.push(label);
        }
        captures.lock().pass_attached.extend(attached);
    }
}

/// Shared sink written by [`CapturePass`] and the [`ScheduleBuilt`] observer.
#[derive(Resource, Clone, Default)]
pub struct RawCaptures(pub Arc<Mutex<CaptureMap>>);

impl RawCaptures {
    pub fn lock(&self) -> MutexGuard<'_, CaptureMap> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Raw per-schedule captures keyed by label.
#[derive(Default)]
pub struct CaptureMap {
    pub schedules: HashMap<InternedScheduleLabel, RawSchedule>,
    /// Labels whose schedule has a [`CapturePass`] installed.
    pub pass_attached: HashSet<InternedScheduleLabel>,
    /// Set whenever a capture changes; cleared when the snapshot is rebuilt.
    pub dirty: bool,
}

/// Everything captured about one schedule, still keyed by Bevy's slotmap keys.
pub struct RawSchedule {
    pub label: InternedScheduleLabel,
    pub build_count: u32,
    /// `false` when the schedule was built before the pass was installed and only names/flags
    /// could be recovered.
    pub captured_by_pass: bool,
    pub systems: Vec<RawSystem>,
    pub sets: Vec<RawSet>,
    pub hierarchy: Vec<(NodeId, NodeId)>,
    pub dependency: Vec<(NodeId, NodeId)>,
    pub flat: Vec<(SystemKey, SystemKey)>,
    pub toposort: Vec<SystemKey>,
    pub ambiguous_with_all: Vec<NodeId>,
    pub pass_added_edges: HashSet<(SystemKey, SystemKey)>,
    pub warnings: Vec<String>,
}

impl RawSchedule {
    pub fn empty(label: InternedScheduleLabel) -> Self {
        Self {
            label,
            build_count: 0,
            captured_by_pass: false,
            systems: Vec::new(),
            sets: Vec::new(),
            hierarchy: Vec::new(),
            dependency: Vec::new(),
            flat: Vec::new(),
            toposort: Vec::new(),
            ambiguous_with_all: Vec::new(),
            pass_added_edges: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    /// Recovers what can still be read from a schedule that was built without the pass.
    fn refresh_from_live(&mut self, schedule: &Schedule) {
        let graph = schedule.graph();
        if let Ok(systems) = schedule.systems_with_access() {
            self.systems = systems
                .map(|(key, system)| RawSystem {
                    key,
                    name: system.name().to_string(),
                    type_id: system.system_type(),
                    flags: system.flags(),
                    exclusive: system.access().is_exclusive(),
                    apply_deferred: system.system_type() == TypeId::of::<ApplyDeferred>(),
                    access: pass::shared(system.access()),
                    conditions: Vec::new(),
                })
                .collect();
            self.toposort = self.systems.iter().map(|s| s.key).collect();
        }
        self.sets = graph
            .system_sets
            .iter()
            .map(|(key, set, _)| RawSet {
                key,
                name: format!("{set:?}"),
                anonymous: set.is_anonymous(),
                system_type: set.system_type(),
                conditions: Vec::new(),
            })
            .collect();
        self.hierarchy = graph.hierarchy().graph().all_edges().collect();
        self.dependency = graph.dependency().graph().all_edges().collect();
        self.ambiguous_with_all = graph.ambiguous_with_all.iter().copied().collect();
    }
}

pub struct RawSystem {
    pub key: SystemKey,
    pub name: String,
    pub type_id: TypeId,
    pub flags: SystemStateFlags,
    /// Whether it takes the whole world.
    pub exclusive: bool,
    pub apply_deferred: bool,
    /// `None` for exclusive systems and sync points.
    pub access: Option<FilteredAccessSet>,
    pub conditions: Vec<RawCondition>,
}

pub struct RawCondition {
    pub name: String,
    /// `None` for a condition taking the whole world.
    pub access: Option<FilteredAccessSet>,
}

pub struct RawSet {
    pub key: SystemSetKey,
    pub name: String,
    pub anonymous: bool,
    pub system_type: Option<TypeId>,
    pub conditions: Vec<RawCondition>,
}

/// Records sync-point edges and warnings, which only exist in the build metadata.
fn on_schedule_built(event: On<ScheduleBuilt>, captures: Res<RawCaptures>) {
    let mut map = captures.lock();
    let label = event.label;
    let entry = map
        .schedules
        .entry(label)
        .or_insert_with(|| RawSchedule::empty(label));
    entry.pass_added_edges = event
        .build_metadata
        .edges_added_by_build_passes
        .iter()
        .copied()
        .collect();
    entry.warnings = event
        .build_metadata
        .warnings
        .iter()
        .map(|warning| format!("{warning:?}"))
        .collect();
    map.dirty = true;
}

/// Runs in `Main` before the sub-schedules: eagerly builds uncaptured schedules and rebuilds the
/// [`AppSnapshot`] when any capture changed.
pub fn refresh_snapshot(world: &mut World) {
    let config = world.resource::<ExtractConfig>().clone();
    let captures = world.resource::<RawCaptures>().clone();
    if config.eager_initialize {
        eager_initialize(world, &captures, &config);
    }
    if !captures.lock().dirty {
        return;
    }
    let generation = world.resource::<AppSnapshot>().generation + 1;
    let snapshot = build_snapshot(world, &captures, generation);
    *world.resource_mut::<AppSnapshot>() = snapshot;
    captures.lock().dirty = false;
    debug!("schedules: snapshot rebuilt (generation {generation})");
}

/// Same as [`refresh_snapshot`], scheduled after the sub-schedules ran.
fn refresh_snapshot_late(world: &mut World) {
    refresh_snapshot(world);
}

/// Takes a fresh [`AppSnapshot`] of the world right now, building any schedule that has not been
/// built yet. Intended for tools that don't run the app loop.
pub fn snapshot_now(world: &mut World) -> AppSnapshot {
    let captures = world
        .get_resource::<RawCaptures>()
        .cloned()
        .unwrap_or_default();
    let config = world
        .get_resource::<ExtractConfig>()
        .cloned()
        .unwrap_or_default();
    eager_initialize(world, &captures, &config);
    let generation = world
        .get_resource::<AppSnapshot>()
        .map(|s| s.generation + 1)
        .unwrap_or(1);
    let snapshot = build_snapshot(world, &captures, generation);
    captures.lock().dirty = false;
    snapshot
}

/// Whether a system name belongs to Bevy itself: every crate mentioned at the top level of the
/// (possibly composed, e.g. `Pipe(a::x, b::y)`) name starts with `bevy_`. Paths inside generic
/// arguments are ignored, so `bevy_state::...::apply_state_transition<my_game::State>` is internal.
pub fn is_internal_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut crates = 0;
    let mut all_bevy = true;
    let mut angle_depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                angle_depth += 1;
                i += 1;
                continue;
            }
            b'>' if i > 0 && bytes[i - 1] == b'-' => {
                i += 1;
                continue;
            }
            b'>' => {
                angle_depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        let at_path_start =
            i == 0 || matches!(bytes[i - 1], b'(' | b'<' | b',' | b' ' | b'[' | b'&');
        if at_path_start && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if angle_depth == 0 && name[j..].starts_with("::") {
                crates += 1;
                if !name[i..j].starts_with("bevy_") {
                    all_bevy = false;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    crates > 0 && all_bevy
}

fn label_name(label: InternedScheduleLabel) -> String {
    format!("{label:?}")
}

fn label_selected(filter: &Option<Vec<String>>, label: InternedScheduleLabel) -> bool {
    match filter {
        None => true,
        Some(list) => {
            let name = label_name(label);
            list.contains(&name)
        }
    }
}

fn eager_initialize(world: &mut World, captures: &RawCaptures, config: &ExtractConfig) {
    let labels: Vec<InternedScheduleLabel> = world
        .resource::<Schedules>()
        .iter()
        .map(|(_, schedule)| schedule.label())
        .collect();
    for label in labels {
        if !label_selected(&config.schedule_filter, label) {
            continue;
        }
        let already_captured = {
            let map = captures.lock();
            map.schedules
                .get(&label)
                .is_some_and(|raw| raw.captured_by_pass)
        };
        if already_captured {
            continue;
        }
        let sink = captures.0.clone();
        let needs_pass = !captures.lock().pass_attached.contains(&label);
        let result = world.try_schedule_scope(label, |world, schedule| {
            if needs_pass {
                schedule.add_build_pass(CapturePass::new(label, sink));
            }
            schedule.initialize(world)
        });
        match result {
            Ok(Ok(_)) => {
                let mut map = captures.lock();
                map.pass_attached.insert(label);
                if let std::collections::hash_map::Entry::Vacant(entry) = map.schedules.entry(label)
                {
                    // Built before the pass existed: register a fallback entry.
                    entry.insert(RawSchedule::empty(label));
                    map.dirty = true;
                }
            }
            Ok(Err(error)) => warn!("schedules: failed to build schedule {label:?}: {error:?}"),
            // The schedule is currently running; try again next frame.
            Err(_) => {}
        }
    }
}

fn build_snapshot(world: &mut World, captures: &RawCaptures, generation: u64) -> AppSnapshot {
    let world: &World = world;
    let mut map = captures.lock();
    let schedules = world.resource::<Schedules>();
    // Refresh fallback entries from the live schedules.
    for raw in map.schedules.values_mut() {
        if !raw.captured_by_pass
            && let Some(live) = schedules.get(raw.label)
        {
            raw.refresh_from_live(live);
        }
    }
    let default_filters: HashSet<ComponentId> = world
        .get_resource::<DefaultQueryFilters>()
        .map(|filters| filters.disabling_ids().collect())
        .unwrap_or_default();
    let mut interner = ComponentInterner::new(world.components(), default_filters);
    let mut out: Vec<ScheduleSnapshot> = map
        .schedules
        .values()
        .map(|raw| convert_schedule(raw, schedules.get(raw.label), &mut interner))
        .collect();

    let names = |labels: &[InternedScheduleLabel]| -> Vec<String> {
        labels.iter().map(|l| label_name(*l)).collect()
    };
    let (main_order, startup_order) = world
        .get_resource::<MainScheduleOrder>()
        .map(|order| (names(&order.labels), names(&order.startup_labels)))
        .unwrap_or_default();
    let fixed_main_order = world
        .get_resource::<FixedMainScheduleOrder>()
        .map(|order| names(&order.labels))
        .unwrap_or_default();
    let rank = |label: &str| -> (usize, usize, String) {
        if let Some(i) = main_order.iter().position(|l| l == label) {
            (0, i, String::new())
        } else if let Some(i) = startup_order.iter().position(|l| l == label) {
            (1, i, String::new())
        } else if let Some(i) = fixed_main_order.iter().position(|l| l == label) {
            (2, i, String::new())
        } else {
            (3, 0, label.to_string())
        }
    };
    out.sort_by_cached_key(|s| rank(&s.label));

    AppSnapshot {
        generation,
        schedules: out,
        main_order,
        startup_order,
        fixed_main_order,
        components: interner.finish(),
    }
}

fn convert_condition(raw: &RawCondition, interner: &mut ComponentInterner) -> ConditionSnapshot {
    let ConvertedAccess {
        queries, resources, ..
    } = raw
        .access
        .as_ref()
        .map(|access| convert_access(access, interner))
        .unwrap_or_default();
    ConditionSnapshot {
        short_name: ShortName(&raw.name).to_string(),
        name: raw.name.clone(),
        queries,
        resources,
    }
}

fn convert_schedule(
    raw: &RawSchedule,
    live: Option<&Schedule>,
    interner: &mut ComponentInterner,
) -> ScheduleSnapshot {
    let sys_index: HashMap<SystemKey, SysIx> = raw
        .systems
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key, i as SysIx))
        .collect();
    let set_index: HashMap<SystemSetKey, SetIx> = raw
        .sets
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key, i as SetIx))
        .collect();
    let node_ref = |node: NodeId| -> Option<NodeRef> {
        match node {
            NodeId::System(key) => sys_index.get(&key).map(|i| NodeRef::System(*i)),
            NodeId::Set(key) => set_index.get(&key).map(|i| NodeRef::Set(*i)),
        }
    };
    let map_edges = |edges: &[(NodeId, NodeId)]| -> Vec<(NodeRef, NodeRef)> {
        edges
            .iter()
            .filter_map(|(a, b)| Some((node_ref(*a)?, node_ref(*b)?)))
            .collect()
    };
    let hierarchy = map_edges(&raw.hierarchy);
    let dependency = map_edges(&raw.dependency);

    let mut systems: Vec<SystemSnapshot> = raw
        .systems
        .iter()
        .map(|s| {
            let converted = s.access.as_ref().map(|a| convert_access(a, interner));
            let (queries, resources, read_all, write_all) = match converted {
                Some(ConvertedAccess {
                    queries,
                    resources,
                    read_all,
                    write_all,
                }) => (queries, resources, read_all, write_all),
                None => (Vec::new(), Vec::new(), false, false),
            };
            SystemSnapshot {
                short_name: ShortName(&s.name).to_string(),
                name: s.name.clone(),
                sets: Vec::new(),
                queries,
                resources,
                conditions: s
                    .conditions
                    .iter()
                    .map(|c| convert_condition(c, interner))
                    .collect(),
                flags: SystemFlags {
                    exclusive: s.exclusive,
                    deferred: s.flags.contains(SystemStateFlags::DEFERRED),
                    non_send: s.flags.contains(SystemStateFlags::NON_SEND),
                    apply_deferred: s.apply_deferred,
                    read_all,
                    write_all,
                    access_available: s.access.is_some() || s.apply_deferred,
                    internal: is_internal_name(&s.name),
                    is_self: s.name.starts_with(concat!(env!("CARGO_CRATE_NAME"), "::")),
                },
            }
        })
        .collect();
    for (parent, child) in &hierarchy {
        if let (NodeRef::Set(set), NodeRef::System(sys)) = (parent, child) {
            systems[*sys as usize].sets.push(*set);
        }
    }

    let sets: Vec<SetSnapshot> = raw
        .sets
        .iter()
        .map(|s| SetSnapshot {
            name: s.name.clone(),
            anonymous: s.anonymous,
            system_type: s.system_type.is_some(),
            conditions: s
                .conditions
                .iter()
                .map(|c| convert_condition(c, interner))
                .collect(),
        })
        .collect();

    let is_sync = |key: &SystemKey| {
        sys_index
            .get(key)
            .is_some_and(|i| systems[*i as usize].flags.apply_deferred)
    };
    let flat_edges: Vec<FlatEdge> = raw
        .flat
        .iter()
        .filter_map(|(a, b)| {
            let kind = if raw.pass_added_edges.contains(&(*a, *b)) || is_sync(a) || is_sync(b) {
                OrderKind::SyncPoint
            } else {
                OrderKind::Explicit
            };
            Some(FlatEdge {
                from: *sys_index.get(a)?,
                to: *sys_index.get(b)?,
                kind,
            })
        })
        .collect();

    let live_order: Option<Vec<SysIx>> = live.and_then(|s| s.systems().ok()).map(|iter| {
        iter.filter_map(|(key, _)| sys_index.get(&key).copied())
            .collect()
    });
    let exec_order = live_order.unwrap_or_else(|| {
        raw.toposort
            .iter()
            .filter_map(|key| sys_index.get(key).copied())
            .collect()
    });

    let (conflicts, conflicts_available) = match live {
        Some(schedule) => {
            let conflicts = schedule
                .graph()
                .conflicting_systems()
                .iter()
                .filter_map(|(a, b, components)| {
                    Some(Conflict {
                        a: *sys_index.get(a)?,
                        b: *sys_index.get(b)?,
                        components: components
                            .iter()
                            .filter(|id| !is_reserved(**id))
                            .map(|id| interner.intern(*id, false))
                            .collect(),
                    })
                })
                .collect();
            (conflicts, true)
        }
        None => (Vec::new(), false),
    };

    ScheduleSnapshot {
        label: label_name(raw.label),
        systems,
        sets,
        hierarchy,
        dependency,
        flat_edges,
        exec_order,
        conflicts,
        conflicts_available,
        ambiguous_with_all: raw
            .ambiguous_with_all
            .iter()
            .filter_map(|n| node_ref(*n))
            .collect(),
        warnings: raw.warnings.clone(),
        build_count: raw.build_count,
    }
}

#[cfg(test)]
mod tests {
    use super::is_internal_name;

    #[test]
    fn internal_names() {
        assert!(is_internal_name(
            "bevy_transform::systems::propagate_parent_transforms"
        ));
        assert!(is_internal_name(
            "Pipe(bevy_state::state::transitions::last_transition<demo::game::GameState>, bevy_state::state::transitions::run_exit<demo::game::GameState>)"
        ));
        assert!(is_internal_name(
            "bevy_state::state::transitions::apply_state_transition<demo::game::GameState>"
        ));
        assert!(is_internal_name("bevy_ui::update::update_clipping_system"));
        assert!(!is_internal_name("demo::game::read_player_input"));
        assert!(!is_internal_name(
            "my_game::systems::foo<bevy_ecs::entity::Entity>"
        ));
        assert!(!is_internal_name("Pipe(demo::a, bevy_state::b)"));
        assert!(!is_internal_name(
            "<Enable the debug feature to see the name>"
        ));
    }
}
