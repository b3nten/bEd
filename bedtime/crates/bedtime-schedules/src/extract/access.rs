//! Conversion of Bevy's access structures into snapshot types.

use std::collections::{HashMap, HashSet};

use bevy::ecs::{
    component::{ComponentId, Components},
    query::{ComponentAccessKind, FilteredAccess, FilteredAccessSet},
    resource::IS_RESOURCE,
};

use crate::snapshot::{CompIx, ComponentSnapshot, FilterGroup, QueryAccess, ResourceAccess};

/// Component ids below this value are reserved by Bevy for internal markers
/// (`ADD`, `INSERT`, `DISCARD`, `REMOVE`, `DESPAWN`, `IS_RESOURCE`).
pub const RESERVED_COMPONENT_IDS: usize = 6;

/// Whether `id` is one of Bevy's reserved marker components.
pub fn is_reserved(id: ComponentId) -> bool {
    id.index() < RESERVED_COMPONENT_IDS
}

/// Interns `ComponentId`s into snapshot indices and resolves their names.
pub struct ComponentInterner<'a> {
    components: &'a Components,
    index: HashMap<ComponentId, CompIx>,
    out: Vec<ComponentSnapshot>,
    /// Components every query implicitly filters out (`Disabled` & co); hidden from filters.
    default_filters: HashSet<ComponentId>,
}

impl<'a> ComponentInterner<'a> {
    pub fn new(components: &'a Components, default_filters: HashSet<ComponentId>) -> Self {
        Self {
            components,
            index: HashMap::new(),
            out: Vec::new(),
            default_filters,
        }
    }

    fn is_hidden_filter(&self, id: ComponentId) -> bool {
        is_reserved(id) || self.default_filters.contains(&id)
    }

    /// Returns the snapshot index for `id`, registering it on first sight.
    ///
    /// `resource_hint` is `true` when the access was tagged as a resource access.
    pub fn intern(&mut self, id: ComponentId, resource_hint: bool) -> CompIx {
        if let Some(&ix) = self.index.get(&id) {
            if resource_hint {
                self.out[ix as usize].is_resource = true;
            }
            return ix;
        }
        let (name, short_name) = match self.components.get_info(id) {
            Some(info) => {
                let name = info.name();
                (name.to_string(), name.shortname().to_string())
            }
            None => {
                let name = format!("ComponentId({})", id.index());
                (name.clone(), name)
            }
        };
        // Resources are ordinary components; the only reliable tell is the
        // `IS_RESOURCE` filter on the access that referenced them.
        let is_resource = resource_hint;
        let ix = self.out.len() as CompIx;
        self.out.push(ComponentSnapshot {
            id: id.index(),
            name,
            short_name,
            is_resource,
        });
        self.index.insert(id, ix);
        ix
    }

    pub fn finish(self) -> Vec<ComponentSnapshot> {
        self.out
    }
}

/// The snapshot form of one `FilteredAccessSet`.
#[derive(Default, Debug)]
pub struct ConvertedAccess {
    pub queries: Vec<QueryAccess>,
    pub resources: Vec<ResourceAccess>,
    pub read_all: bool,
    pub write_all: bool,
}

/// Converts a system's or condition's access set.
pub fn convert_access(
    set: &FilteredAccessSet,
    interner: &mut ComponentInterner,
) -> ConvertedAccess {
    let mut out = ConvertedAccess::default();
    for fa in set.filtered_accesses() {
        if is_resource_access(fa) {
            convert_resource(fa, interner, &mut out.resources);
        } else if let Some(query) = convert_query(fa, interner) {
            out.queries.push(query);
        }
    }
    let combined = set.combined_access();
    out.read_all = combined.has_read_all();
    out.write_all = combined.has_write_all();
    out
}

/// `Res`/`ResMut`/`NonSend` parameters register a `With<IS_RESOURCE>` filter.
fn is_resource_access(fa: &FilteredAccess) -> bool {
    fa.filter_sets()
        .iter()
        .any(|filter| filter.with().contains(IS_RESOURCE))
}

fn convert_resource(
    fa: &FilteredAccess,
    interner: &mut ComponentInterner,
    out: &mut Vec<ResourceAccess>,
) {
    let Ok(iter) = fa.access().try_iter_access() else {
        return;
    };
    for kind in iter {
        match kind {
            ComponentAccessKind::Shared(id) if !is_reserved(id) => out.push(ResourceAccess {
                comp: interner.intern(id, true),
                write: false,
            }),
            ComponentAccessKind::Exclusive(id) if !is_reserved(id) => out.push(ResourceAccess {
                comp: interner.intern(id, true),
                write: true,
            }),
            _ => {}
        }
    }
}

fn convert_query(fa: &FilteredAccess, interner: &mut ComponentInterner) -> Option<QueryAccess> {
    let access = fa.access();
    let mut query = QueryAccess::default();
    let mut accessed: HashSet<ComponentId> = HashSet::new();
    match access.try_iter_access() {
        Ok(iter) => {
            for kind in iter {
                match kind {
                    ComponentAccessKind::Archetypal(id) if !is_reserved(id) => {
                        accessed.insert(id);
                        query.archetypal.push(interner.intern(id, false));
                    }
                    ComponentAccessKind::Shared(id) if !is_reserved(id) => {
                        accessed.insert(id);
                        query.reads.push(interner.intern(id, false));
                    }
                    ComponentAccessKind::Exclusive(id) if !is_reserved(id) => {
                        accessed.insert(id);
                        query.writes.push(interner.intern(id, false));
                    }
                    _ => {}
                }
            }
        }
        Err(_) => {
            query.read_all = access.has_read_all();
            query.write_all = access.has_write_all();
        }
    }
    // Bevy adds every accessed component to the `with` filter as well; only keep the
    // user-visible `With`/`Without` clauses.
    for filter in fa.filter_sets() {
        let with_ids: Vec<ComponentId> = filter
            .with()
            .iter()
            .filter(|id| !is_reserved(*id) && !accessed.contains(id))
            .collect();
        let without_ids: Vec<ComponentId> = filter
            .without()
            .iter()
            .filter(|id| !interner.is_hidden_filter(*id))
            .collect();
        let with: Vec<CompIx> = with_ids
            .into_iter()
            .map(|id| interner.intern(id, false))
            .collect();
        let without: Vec<CompIx> = without_ids
            .into_iter()
            .map(|id| interner.intern(id, false))
            .collect();
        if with.is_empty() && without.is_empty() {
            continue;
        }
        let group = FilterGroup { with, without };
        if !query.filters.contains(&group) {
            query.filters.push(group);
        }
    }
    let empty = query.reads.is_empty()
        && query.writes.is_empty()
        && query.archetypal.is_empty()
        && query.filters.is_empty()
        && !query.read_all
        && !query.write_all;
    (!empty).then_some(query)
}
