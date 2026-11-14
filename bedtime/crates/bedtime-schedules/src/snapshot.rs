//! Renderer-agnostic description of an app's schedules.
//!
//! Everything here is plain data indexed by small integers, so it can be shared between the
//! extraction layer, the layout engine, the viewer, tests and (optionally) serialised to disk.

use bevy::ecs::resource::Resource;

/// Index of a system within [`ScheduleSnapshot::systems`].
pub type SysIx = u32;
/// Index of a system set within [`ScheduleSnapshot::sets`].
pub type SetIx = u32;
/// Index of a component or resource within [`AppSnapshot::components`].
pub type CompIx = u32;

/// A snapshot of every captured schedule in the app.
#[derive(Resource, Default, Clone, Debug)]
pub struct AppSnapshot {
    /// Incremented every time the snapshot is rebuilt. Viewers use it to invalidate caches.
    pub generation: u64,
    /// Schedules ordered: `main_order` first, then `startup_order`, then `fixed_main_order`, then the
    /// remaining schedules alphabetically.
    pub schedules: Vec<ScheduleSnapshot>,
    /// Labels run by `Main` every frame, in order.
    pub main_order: Vec<String>,
    /// Labels run by `Main` once at startup, in order.
    pub startup_order: Vec<String>,
    /// Labels run by `FixedMain` per fixed tick, in order.
    pub fixed_main_order: Vec<String>,
    /// Every component/resource referenced by any access in any schedule.
    pub components: Vec<ComponentSnapshot>,
}

impl AppSnapshot {
    /// Finds a schedule by label.
    pub fn schedule(&self, label: &str) -> Option<&ScheduleSnapshot> {
        self.schedules.iter().find(|s| s.label == label)
    }
}

/// A component or resource type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentSnapshot {
    /// The `ComponentId` index in the world this snapshot was taken from.
    pub id: usize,
    /// Fully qualified type name.
    pub name: String,
    /// Type name with module paths stripped.
    pub short_name: String,
    /// `true` if this is a resource (accessed through `Res`/`ResMut`/`NonSend`...).
    pub is_resource: bool,
}

/// Everything captured about one schedule.
#[derive(Clone, Debug, Default)]
pub struct ScheduleSnapshot {
    /// `Debug` rendering of the schedule label, e.g. `Update` or `OnEnter(Playing)`.
    pub label: String,
    pub systems: Vec<SystemSnapshot>,
    pub sets: Vec<SetSnapshot>,
    /// `(parent set, child)` membership edges, already transitively reduced by Bevy.
    pub hierarchy: Vec<(NodeRef, NodeRef)>,
    /// Ordering constraints exactly as configured (between systems and/or sets).
    pub dependency: Vec<(NodeRef, NodeRef)>,
    /// System-to-system ordering edges after flattening sets, including auto-inserted sync points.
    /// Not transitively reduced.
    pub flat_edges: Vec<FlatEdge>,
    /// The order the executor uses to run the systems (a topological order of `flat_edges`).
    /// Falls back to a topological sort computed at build time when the executor order is not
    /// readable (e.g. for `Main`). Empty if neither is available.
    pub exec_order: Vec<SysIx>,
    /// Pairs of systems with conflicting access and no ordering between them.
    pub conflicts: Vec<Conflict>,
    /// `false` when the schedule was not present in the world at snapshot time (e.g. `Main`), so
    /// `conflicts` could not be read.
    pub conflicts_available: bool,
    /// Nodes configured with `ambiguous_with_all`.
    pub ambiguous_with_all: Vec<NodeRef>,
    /// Warnings reported by the last schedule build.
    pub warnings: Vec<String>,
    /// How many times this schedule has been built while captured.
    pub build_count: u32,
}

impl ScheduleSnapshot {
    /// Finds a system by full or short name.
    pub fn system_by_name(&self, name: &str) -> Option<(SysIx, &SystemSnapshot)> {
        self.systems
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name || s.short_name == name)
            .map(|(i, s)| (i as SysIx, s))
    }
}

/// Reference to a node (system or set) inside one schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeRef {
    System(SysIx),
    Set(SetIx),
}

/// Kind of a flattened ordering edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OrderKind {
    /// Configured by the user (directly or through sets).
    Explicit,
    /// Added by a build pass, i.e. an auto-inserted `ApplyDeferred` sync point.
    SyncPoint,
}

/// A flattened system-to-system ordering edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlatEdge {
    pub from: SysIx,
    pub to: SysIx,
    pub kind: OrderKind,
}

/// Two systems whose accesses conflict without an ordering between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub a: SysIx,
    pub b: SysIx,
    /// Conflicting components; empty means the conflict is on the whole `World`.
    pub components: Vec<CompIx>,
}

/// One system in a schedule.
#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    pub name: String,
    pub short_name: String,
    /// Direct set memberships (named, anonymous and system-type sets alike).
    pub sets: Vec<SetIx>,
    /// One entry per non-resource parameter that accesses components (`Query`, `EntityMut`...).
    pub queries: Vec<QueryAccess>,
    /// One entry per resource parameter (`Res`, `ResMut`, `NonSend`...).
    pub resources: Vec<ResourceAccess>,
    /// Run conditions attached directly to this system.
    pub conditions: Vec<ConditionSnapshot>,
    pub flags: SystemFlags,
}

impl SystemSnapshot {
    /// Whether the system writes component `c` through any query.
    pub fn writes_component(&self, c: CompIx) -> bool {
        self.queries.iter().any(|q| q.writes.contains(&c))
    }

    /// Whether the system reads (or writes) component `c` through any query.
    pub fn reads_component(&self, c: CompIx) -> bool {
        self.queries
            .iter()
            .any(|q| q.reads.contains(&c) || q.writes.contains(&c))
    }
}

/// Boolean facts about a system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemFlags {
    /// Takes `&mut World`; runs alone.
    pub exclusive: bool,
    /// Has deferred buffers (`Commands` etc.) to apply.
    pub deferred: bool,
    /// Must run on the main thread.
    pub non_send: bool,
    /// Is an `ApplyDeferred` sync point.
    pub apply_deferred: bool,
    /// Reads every component (e.g. `&World`, `EntityRef`).
    pub read_all: bool,
    /// Writes every component (e.g. `EntityMut`).
    pub write_all: bool,
    /// `false` when the access could not be captured (exclusive systems, or schedules built
    /// before the capture pass was installed).
    pub access_available: bool,
    /// Name starts with `bevy_`.
    pub internal: bool,
    /// Belongs to this crate.
    pub is_self: bool,
}

/// Component access of one parameter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryAccess {
    /// Components read (writes are not repeated here).
    pub reads: Vec<CompIx>,
    pub writes: Vec<CompIx>,
    /// Components that only affect which entities match (`Has<T>`).
    pub archetypal: Vec<CompIx>,
    /// Filter clauses combined with `Or`; each clause is `With` all / `Without` all.
    pub filters: Vec<FilterGroup>,
    pub read_all: bool,
    pub write_all: bool,
}

/// One `With`/`Without` clause.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterGroup {
    pub with: Vec<CompIx>,
    pub without: Vec<CompIx>,
}

/// Access to one resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceAccess {
    pub comp: CompIx,
    pub write: bool,
}

/// A run condition and what it accesses.
#[derive(Clone, Debug, Default)]
pub struct ConditionSnapshot {
    pub name: String,
    pub short_name: String,
    pub queries: Vec<QueryAccess>,
    pub resources: Vec<ResourceAccess>,
}

/// A system set in a schedule.
#[derive(Clone, Debug, Default)]
pub struct SetSnapshot {
    /// `Debug` rendering of the set.
    pub name: String,
    /// Created implicitly by a tuple/chain configuration; has no user-facing name.
    pub anonymous: bool,
    /// The implicit per-system-type set that every system belongs to.
    pub system_type: bool,
    pub conditions: Vec<ConditionSnapshot>,
}

impl SetSnapshot {
    /// Sets worth showing to a user as clusters.
    pub fn is_named(&self) -> bool {
        !self.anonymous && !self.system_type
    }
}
