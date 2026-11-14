//! Builds the graph the viewer should draw: named sets become clusters, collapsed sets become
//! single nodes, hidden nodes are bypassed, and (optionally) transitive edges are dropped.

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
};

use super::{LayoutCluster, LayoutEdge, LayoutInput, LayoutNode};
use crate::snapshot::{NodeRef, OrderKind, ScheduleSnapshot, SetIx, SysIx};

/// What the viewer wants to see.
pub struct VisibleSpec<'a> {
    pub collapsed: &'a HashSet<SetIx>,
    pub hidden: &'a HashSet<NodeRef>,
    pub show_internal: bool,
    pub show_self: bool,
    pub show_sync: bool,
    /// When `Some`, only these systems (and sets containing them) are shown.
    pub only: Option<&'a HashSet<SysIx>>,
    pub transitive_reduce: bool,
}

/// Horizontal padding added around node labels.
pub const NODE_PAD_X: f32 = 16.0;

#[derive(Clone, Debug, Default)]
pub struct VisibleGraph {
    pub input: LayoutInput,
    /// Label to draw for each node.
    pub labels: Vec<String>,
    /// Systems represented by each node (one for a system, many for a collapsed set).
    pub members: Vec<Vec<SysIx>>,
    pub node_index: HashMap<NodeRef, usize>,
    /// Cluster index per named set that is drawn.
    pub cluster_of_set: HashMap<SetIx, usize>,
    /// Systems that were removed (hidden or filtered out).
    pub removed: Vec<SysIx>,
}

impl VisibleGraph {
    /// Hash of everything that affects the layout.
    pub fn fingerprint(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for node in &self.input.nodes {
            node.id.hash(&mut h);
            node.width.to_bits().hash(&mut h);
            node.cluster.hash(&mut h);
        }
        for edge in &self.input.edges {
            edge.hash(&mut h);
        }
        for cluster in &self.input.clusters {
            cluster.set.hash(&mut h);
            cluster.parent.hash(&mut h);
            cluster.label_width.to_bits().hash(&mut h);
        }
        h.finish()
    }
}

/// Named-set tree of a schedule, independent of the view state.
struct SetTree {
    /// Deepest named ancestor set (or self) of each system.
    primary_of_system: Vec<Option<SetIx>>,
    /// Deepest named proper ancestor of each named set.
    parent_of_set: Vec<Option<SetIx>>,
    /// Transitive system members of each named set.
    members: Vec<Vec<SysIx>>,
}

fn set_tree(schedule: &ScheduleSnapshot) -> SetTree {
    let set_count = schedule.sets.len();
    let mut set_parents: Vec<Vec<SetIx>> = vec![Vec::new(); set_count];
    for (parent, child) in &schedule.hierarchy {
        if let (NodeRef::Set(p), NodeRef::Set(c)) = (parent, child) {
            set_parents[*c as usize].push(*p);
        }
    }
    // Depth = longest chain of ancestors (hierarchy is acyclic).
    let mut depth = vec![None::<u32>; set_count];
    fn depth_of(s: usize, parents: &[Vec<SetIx>], depth: &mut [Option<u32>], guard: u32) -> u32 {
        if let Some(d) = depth[s] {
            return d;
        }
        let d = if guard > 64 {
            0
        } else {
            parents[s]
                .iter()
                .map(|p| depth_of(*p as usize, parents, depth, guard + 1) + 1)
                .max()
                .unwrap_or(0)
        };
        depth[s] = Some(d);
        d
    }
    for s in 0..set_count {
        depth_of(s, &set_parents, &mut depth, 0);
    }
    let depth: Vec<u32> = depth.into_iter().map(|d| d.unwrap_or(0)).collect();

    // All ancestors (including self) of a set.
    let ancestors = |start: &[SetIx]| -> Vec<SetIx> {
        let mut seen = HashSet::new();
        let mut stack: Vec<SetIx> = start.to_vec();
        let mut out = Vec::new();
        while let Some(s) = stack.pop() {
            if !seen.insert(s) {
                continue;
            }
            out.push(s);
            stack.extend(set_parents[s as usize].iter().copied());
        }
        out
    };
    let deepest_named = |candidates: &[SetIx]| -> Option<SetIx> {
        candidates
            .iter()
            .copied()
            .filter(|s| schedule.sets[*s as usize].is_named())
            .max_by_key(|s| (depth[*s as usize], std::cmp::Reverse(*s)))
    };

    let mut members: Vec<Vec<SysIx>> = vec![Vec::new(); set_count];
    let primary_of_system: Vec<Option<SetIx>> = schedule
        .systems
        .iter()
        .enumerate()
        .map(|(i, system)| {
            let all = ancestors(&system.sets);
            for s in &all {
                if schedule.sets[*s as usize].is_named() {
                    members[*s as usize].push(i as SysIx);
                }
            }
            deepest_named(&all)
        })
        .collect();
    let parent_of_set: Vec<Option<SetIx>> = (0..set_count)
        .map(|s| {
            let proper: Vec<SetIx> = ancestors(&set_parents[s]);
            deepest_named(&proper)
        })
        .collect();
    SetTree {
        primary_of_system,
        parent_of_set,
        members,
    }
}

/// For every system: the chain of named sets it primarily belongs to, outermost first.
pub fn primary_chains(schedule: &ScheduleSnapshot) -> Vec<Vec<SetIx>> {
    let tree = set_tree(schedule);
    (0..schedule.systems.len())
        .map(|sys| {
            let mut chain = Vec::new();
            let mut current = tree.primary_of_system[sys];
            while let Some(s) = current {
                chain.push(s);
                current = tree.parent_of_set[s as usize];
            }
            chain.reverse();
            chain
        })
        .collect()
}

pub fn build_visible(
    schedule: &ScheduleSnapshot,
    spec: &VisibleSpec,
    mut measure: impl FnMut(&str) -> f32,
) -> VisibleGraph {
    let tree = set_tree(schedule);

    // Clusters: named sets with members.
    let mut cluster_of_set: HashMap<SetIx, usize> = HashMap::new();
    let mut clusters: Vec<LayoutCluster> = Vec::new();
    for (s, set) in schedule.sets.iter().enumerate() {
        if set.is_named() && !tree.members[s].is_empty() {
            cluster_of_set.insert(s as SetIx, clusters.len());
            clusters.push(LayoutCluster {
                set: s as SetIx,
                parent: None,
                name: set.name.clone(),
                label_width: measure(&set.name),
            });
        }
    }
    for cluster in &mut clusters {
        cluster.parent =
            tree.parent_of_set[cluster.set as usize].and_then(|p| cluster_of_set.get(&p).copied());
    }

    // Representative of each system after collapsing.
    let chain = |sys: SysIx| -> Vec<SetIx> {
        let mut out = Vec::new();
        let mut current = tree.primary_of_system[sys as usize];
        while let Some(s) = current {
            out.push(s);
            current = tree.parent_of_set[s as usize];
        }
        out
    };
    let rep = |sys: SysIx| -> NodeRef {
        chain(sys)
            .into_iter()
            .rev()
            .find(|s| spec.collapsed.contains(s))
            .map_or(NodeRef::System(sys), NodeRef::Set)
    };
    let system_visible = |sys: SysIx| -> bool {
        let system = &schedule.systems[sys as usize];
        if spec.hidden.contains(&NodeRef::System(sys)) {
            return false;
        }
        if system.flags.internal && !spec.show_internal {
            return false;
        }
        if system.flags.is_self && !spec.show_self {
            return false;
        }
        if system.flags.apply_deferred && !spec.show_sync {
            return false;
        }
        if let Some(only) = spec.only
            && !only.contains(&sys)
        {
            return false;
        }
        true
    };

    // All representatives (visible or not), so hidden ones can be bypassed.
    let mut all_index: HashMap<NodeRef, usize> = HashMap::new();
    let mut all_reps: Vec<NodeRef> = Vec::new();
    let mut all_members: Vec<Vec<SysIx>> = Vec::new();
    let mut rep_of: Vec<usize> = Vec::with_capacity(schedule.systems.len());
    for sys in 0..schedule.systems.len() as SysIx {
        let r = rep(sys);
        let idx = *all_index.entry(r).or_insert_with(|| {
            all_reps.push(r);
            all_members.push(Vec::new());
            all_reps.len() - 1
        });
        all_members[idx].push(sys);
        rep_of.push(idx);
    }
    let visible: Vec<bool> = all_reps
        .iter()
        .enumerate()
        .map(|(i, r)| match r {
            NodeRef::System(sys) => system_visible(*sys),
            NodeRef::Set(set) => {
                !spec.hidden.contains(&NodeRef::Set(*set))
                    && all_members[i].iter().any(|s| system_visible(*s))
            }
        })
        .collect();

    // Edges between representatives.
    let mut edges: HashMap<(usize, usize), (OrderKind, bool)> = HashMap::new();
    for edge in &schedule.flat_edges {
        let a = rep_of[edge.from as usize];
        let b = rep_of[edge.to as usize];
        if a == b {
            continue;
        }
        edges
            .entry((a, b))
            .and_modify(|(kind, _)| {
                if edge.kind == OrderKind::Explicit {
                    *kind = OrderKind::Explicit;
                }
            })
            .or_insert((edge.kind, false));
    }
    // Bypass hidden representatives one at a time.
    for (h, _) in visible.iter().enumerate().filter(|(_, v)| !**v) {
        let preds: Vec<(usize, OrderKind)> = edges
            .iter()
            .filter(|((_, b), _)| *b == h)
            .map(|((a, _), (kind, _))| (*a, *kind))
            .collect();
        let succs: Vec<(usize, OrderKind)> = edges
            .iter()
            .filter(|((a, _), _)| *a == h)
            .map(|((_, b), (kind, _))| (*b, *kind))
            .collect();
        edges.retain(|(a, b), _| *a != h && *b != h);
        for (p, pk) in &preds {
            for (s, sk) in &succs {
                if p == s {
                    continue;
                }
                let kind = if *pk == OrderKind::SyncPoint || *sk == OrderKind::SyncPoint {
                    OrderKind::SyncPoint
                } else {
                    OrderKind::Explicit
                };
                edges.entry((*p, *s)).or_insert((kind, true));
            }
        }
    }

    // Compact to visible nodes.
    let mut node_index: HashMap<NodeRef, usize> = HashMap::new();
    let mut nodes = Vec::new();
    let mut labels = Vec::new();
    let mut members = Vec::new();
    let mut compact = vec![usize::MAX; all_reps.len()];
    for (i, r) in all_reps.iter().enumerate() {
        if !visible[i] {
            continue;
        }
        let (label, cluster) = match r {
            NodeRef::System(sys) => {
                let system = &schedule.systems[*sys as usize];
                let label = if system.flags.apply_deferred {
                    "⏸".to_string()
                } else {
                    system.short_name.clone()
                };
                let cluster = tree.primary_of_system[*sys as usize]
                    .and_then(|s| cluster_of_set.get(&s).copied());
                (label, cluster)
            }
            NodeRef::Set(set) => {
                let count = all_members[i]
                    .iter()
                    .filter(|s| system_visible(**s))
                    .count();
                let label = format!("▸ {} ({count})", schedule.sets[*set as usize].name);
                let cluster =
                    tree.parent_of_set[*set as usize].and_then(|p| cluster_of_set.get(&p).copied());
                (label, cluster)
            }
        };
        compact[i] = nodes.len();
        node_index.insert(*r, nodes.len());
        nodes.push(LayoutNode {
            id: *r,
            width: measure(&label) + NODE_PAD_X,
            cluster,
        });
        labels.push(label);
        members.push(all_members[i].clone());
    }
    let mut layout_edges: Vec<LayoutEdge> = edges
        .into_iter()
        .filter_map(|((a, b), (kind, via_hidden))| {
            let (from, to) = (compact[a], compact[b]);
            (from != usize::MAX && to != usize::MAX).then_some(LayoutEdge {
                from,
                to,
                kind,
                via_hidden,
            })
        })
        .collect();
    layout_edges.sort_by_key(|e| (e.from, e.to));

    if spec.transitive_reduce {
        layout_edges = transitive_reduction(nodes.len(), layout_edges);
    }

    // Drop clusters that ended up empty (all members hidden) and remap indices.
    let mut used = vec![false; clusters.len()];
    for node in &nodes {
        let mut current = node.cluster;
        while let Some(c) = current {
            used[c] = true;
            current = clusters[c].parent;
        }
    }
    let mut remap = vec![None; clusters.len()];
    let mut kept = Vec::new();
    for (c, cluster) in clusters.iter().enumerate() {
        if used[c] {
            remap[c] = Some(kept.len());
            kept.push(cluster.clone());
        }
    }
    for cluster in &mut kept {
        cluster.parent = cluster.parent.and_then(|p| remap[p]);
    }
    for node in &mut nodes {
        node.cluster = node.cluster.and_then(|c| remap[c]);
    }
    let cluster_of_set = cluster_of_set
        .into_iter()
        .filter_map(|(set, c)| remap[c].map(|c| (set, c)))
        .collect();
    let removed = (0..schedule.systems.len() as SysIx)
        .filter(|sys| !visible[rep_of[*sys as usize]])
        .collect();

    VisibleGraph {
        input: LayoutInput {
            nodes,
            edges: layout_edges,
            clusters: kept,
        },
        labels,
        members,
        node_index,
        cluster_of_set,
        removed,
    }
}

/// Removes edges implied by longer paths.
fn transitive_reduction(n: usize, edges: Vec<LayoutEdge>) -> Vec<LayoutEdge> {
    let words = n.div_ceil(64).max(1);
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &edges {
        out[e.from].push(e.to);
    }
    // Reverse topological order via DFS post-order.
    let mut order = Vec::with_capacity(n);
    let mut state = vec![0u8; n];
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state[start] = 1;
        while let Some((node, i)) = stack.last_mut() {
            if *i < out[*node].len() {
                let next = out[*node][*i];
                *i += 1;
                if state[next] == 0 {
                    state[next] = 1;
                    stack.push((next, 0));
                }
            } else {
                state[*node] = 2;
                order.push(*node);
                stack.pop();
            }
        }
    }
    let mut reach = vec![vec![0u64; words]; n];
    for &node in &order {
        for &succ in &out[node] {
            let (mut mine, theirs) = (std::mem::take(&mut reach[node]), reach[succ].clone());
            for (m, t) in mine.iter_mut().zip(theirs.iter()) {
                *m |= t;
            }
            mine[succ / 64] |= 1 << (succ % 64);
            reach[node] = mine;
        }
    }
    edges
        .into_iter()
        .filter(|e| {
            !out[e.from]
                .iter()
                .any(|&w| w != e.to && (reach[w][e.to / 64] >> (e.to % 64)) & 1 == 1)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{FlatEdge, SetSnapshot, SystemSnapshot};

    fn schedule() -> ScheduleSnapshot {
        // sets: 0 = Named "A", 1 = anonymous inside A, 2 = Named "B"
        // systems: a0, a1 in anon set (in A); b0 in B; c free; sync point s
        let mut s = ScheduleSnapshot {
            label: "T".into(),
            ..Default::default()
        };
        s.sets = vec![
            SetSnapshot {
                name: "A".into(),
                ..Default::default()
            },
            SetSnapshot {
                name: "anon".into(),
                anonymous: true,
                ..Default::default()
            },
            SetSnapshot {
                name: "B".into(),
                ..Default::default()
            },
        ];
        let sys = |name: &str, sets: Vec<SetIx>| SystemSnapshot {
            name: name.into(),
            short_name: name.into(),
            sets,
            ..Default::default()
        };
        s.systems = vec![
            sys("a0", vec![1]),
            sys("a1", vec![1]),
            sys("b0", vec![2]),
            sys("c", vec![]),
            sys("sync", vec![]),
        ];
        s.systems[4].flags.apply_deferred = true;
        s.hierarchy = vec![
            (NodeRef::Set(0), NodeRef::Set(1)),
            (NodeRef::Set(1), NodeRef::System(0)),
            (NodeRef::Set(1), NodeRef::System(1)),
            (NodeRef::Set(2), NodeRef::System(2)),
        ];
        let e = |from, to, kind| FlatEdge { from, to, kind };
        s.flat_edges = vec![
            e(0, 1, OrderKind::Explicit),
            e(1, 4, OrderKind::SyncPoint),
            e(4, 2, OrderKind::SyncPoint),
            e(3, 0, OrderKind::Explicit),
            e(3, 2, OrderKind::Explicit),
        ];
        s
    }

    fn spec<'a>(
        collapsed: &'a HashSet<SetIx>,
        hidden: &'a HashSet<NodeRef>,
        show_sync: bool,
        reduce: bool,
    ) -> VisibleSpec<'a> {
        VisibleSpec {
            collapsed,
            hidden,
            show_internal: true,
            show_self: true,
            show_sync,
            only: None,
            transitive_reduce: reduce,
        }
    }

    #[test]
    fn anonymous_sets_are_transparent_and_named_sets_become_clusters() {
        let (collapsed, hidden) = (HashSet::new(), HashSet::new());
        let g = build_visible(&schedule(), &spec(&collapsed, &hidden, true, false), |_| {
            30.0
        });
        assert_eq!(g.input.clusters.len(), 2);
        let a = g.cluster_of_set[&0];
        let b = g.cluster_of_set[&2];
        assert_eq!(
            g.input.nodes[g.node_index[&NodeRef::System(0)]].cluster,
            Some(a)
        );
        assert_eq!(
            g.input.nodes[g.node_index[&NodeRef::System(2)]].cluster,
            Some(b)
        );
        assert_eq!(
            g.input.nodes[g.node_index[&NodeRef::System(3)]].cluster,
            None
        );
        assert_eq!(g.input.edges.len(), 5);
    }

    #[test]
    fn hiding_sync_points_bypasses_them() {
        let (collapsed, hidden) = (HashSet::new(), HashSet::new());
        let g = build_visible(
            &schedule(),
            &spec(&collapsed, &hidden, false, false),
            |_| 30.0,
        );
        assert!(!g.node_index.contains_key(&NodeRef::System(4)));
        let a1 = g.node_index[&NodeRef::System(1)];
        let b0 = g.node_index[&NodeRef::System(2)];
        let bypass = g
            .input
            .edges
            .iter()
            .find(|e| e.from == a1 && e.to == b0)
            .expect("a1 -> b0 bypass edge");
        assert!(bypass.via_hidden);
        assert_eq!(bypass.kind, OrderKind::SyncPoint);
        assert_eq!(g.removed, vec![4]);
    }

    #[test]
    fn collapsing_a_set_merges_members() {
        let collapsed: HashSet<SetIx> = [0].into_iter().collect();
        let hidden = HashSet::new();
        let g = build_visible(&schedule(), &spec(&collapsed, &hidden, true, false), |_| {
            30.0
        });
        let a = g.node_index[&NodeRef::Set(0)];
        assert_eq!(g.members[a], vec![0, 1]);
        assert!(g.labels[a].starts_with("▸ A (2)"));
        assert!(!g.node_index.contains_key(&NodeRef::System(0)));
        // c -> A and A -> sync; the internal a0 -> a1 edge disappears.
        let c = g.node_index[&NodeRef::System(3)];
        assert!(g.input.edges.iter().any(|e| e.from == c && e.to == a));
        assert!(!g.input.edges.iter().any(|e| e.from == a && e.to == a));
        // Collapsed set is no longer a cluster.
        assert!(!g.cluster_of_set.contains_key(&0));
    }

    #[test]
    fn transitive_reduction_drops_implied_edges() {
        let (collapsed, hidden) = (HashSet::new(), HashSet::new());
        let full = build_visible(&schedule(), &spec(&collapsed, &hidden, true, false), |_| {
            30.0
        });
        let reduced = build_visible(&schedule(), &spec(&collapsed, &hidden, true, true), |_| {
            30.0
        });
        let c = reduced.node_index[&NodeRef::System(3)];
        let b0 = reduced.node_index[&NodeRef::System(2)];
        assert!(full.input.edges.iter().any(|e| e.from == c && e.to == b0));
        assert!(
            !reduced
                .input
                .edges
                .iter()
                .any(|e| e.from == c && e.to == b0)
        );
        assert_eq!(reduced.input.edges.len(), full.input.edges.len() - 1);
    }
}
