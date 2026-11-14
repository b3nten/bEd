//! Derived data-flow edges: a system that writes a component feeds every system that reads or
//! writes it later in the same schedule.

use std::collections::HashMap;

use crate::snapshot::{CompIx, ScheduleSnapshot, SysIx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlowKind {
    /// Writer -> reader.
    WriteRead,
    /// Writer -> later writer.
    WriteWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlowEdge {
    pub from: SysIx,
    pub to: SysIx,
    pub comp: CompIx,
    pub kind: FlowKind,
    /// The consumer only touches the component from a run condition.
    pub via_condition: bool,
}

/// Bitset reachability over the flattened ordering graph: `reach[a]` has bit `b` set when `a`
/// runs before `b` (transitively).
pub fn reachability(schedule: &ScheduleSnapshot) -> Vec<Vec<u64>> {
    let n = schedule.systems.len();
    let words = n.div_ceil(64).max(1);
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for e in &schedule.flat_edges {
        let (a, b) = (e.from as usize, e.to as usize);
        if a < n && b < n && a != b {
            out[a].push(b);
            indeg[b] += 1;
        }
    }
    // Kahn's algorithm; cycles are impossible in a built schedule but keep it safe.
    let mut order = Vec::with_capacity(n);
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    while let Some(u) = queue.pop() {
        order.push(u);
        for &v in &out[u] {
            indeg[v] -= 1;
            if indeg[v] == 0 {
                queue.push(v);
            }
        }
    }
    let mut reach = vec![vec![0u64; words]; n];
    for &u in order.iter().rev() {
        let mut acc = vec![0u64; words];
        for &v in &out[u] {
            acc[v / 64] |= 1 << (v % 64);
            for (a, r) in acc.iter_mut().zip(reach[v].iter()) {
                *a |= r;
            }
        }
        reach[u] = acc;
    }
    reach
}

pub fn reaches(reach: &[Vec<u64>], a: SysIx, b: SysIx) -> bool {
    let b = b as usize;
    reach
        .get(a as usize)
        .and_then(|row| row.get(b / 64))
        .is_some_and(|w| (w >> (b % 64)) & 1 == 1)
}

/// Every component/resource each system touches, split into writers and readers.
#[derive(Default, Debug, Clone)]
pub struct AccessIndex {
    /// Per component: systems that write it (`via_condition` is always `false` for writers).
    pub writers: HashMap<CompIx, Vec<SysIx>>,
    /// Per component: `(system, via_condition)` that read it.
    pub readers: HashMap<CompIx, Vec<(SysIx, bool)>>,
}

pub fn access_index(schedule: &ScheduleSnapshot) -> AccessIndex {
    let mut index = AccessIndex::default();
    for (i, system) in schedule.systems.iter().enumerate() {
        let sys = i as SysIx;
        let mut add = |comp: CompIx, write: bool, via_condition: bool| {
            if write {
                index.writers.entry(comp).or_default().push(sys);
            } else {
                index
                    .readers
                    .entry(comp)
                    .or_default()
                    .push((sys, via_condition));
            }
        };
        for q in &system.queries {
            for c in &q.writes {
                add(*c, true, false);
            }
            for c in &q.reads {
                add(*c, false, false);
            }
        }
        for r in &system.resources {
            add(r.comp, r.write, false);
        }
        for condition in &system.conditions {
            for q in &condition.queries {
                for c in &q.writes {
                    add(*c, true, true);
                }
                for c in &q.reads {
                    add(*c, false, true);
                }
            }
            for r in &condition.resources {
                add(r.comp, r.write, true);
            }
        }
    }
    for list in index.writers.values_mut() {
        list.sort_unstable();
        list.dedup();
    }
    for list in index.readers.values_mut() {
        list.sort_unstable();
        list.dedup_by_key(|(sys, _)| *sys);
    }
    index
}

/// All data-flow edges of a schedule, sorted by component.
pub fn flow_edges(schedule: &ScheduleSnapshot) -> Vec<FlowEdge> {
    let reach = reachability(schedule);
    let index = access_index(schedule);
    let mut edges = Vec::new();
    for (comp, writers) in &index.writers {
        for &w in writers {
            if let Some(readers) = index.readers.get(comp) {
                for &(r, via_condition) in readers {
                    if r != w && reaches(&reach, w, r) {
                        edges.push(FlowEdge {
                            from: w,
                            to: r,
                            comp: *comp,
                            kind: FlowKind::WriteRead,
                            via_condition,
                        });
                    }
                }
            }
            for &w2 in writers {
                if w2 != w && reaches(&reach, w, w2) {
                    edges.push(FlowEdge {
                        from: w,
                        to: w2,
                        comp: *comp,
                        kind: FlowKind::WriteWrite,
                        via_condition: false,
                    });
                }
            }
        }
    }
    edges.sort_by_key(|e| (e.comp, e.from, e.to, e.kind as u8));
    edges.dedup();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{FlatEdge, OrderKind, QueryAccess, ResourceAccess, SystemSnapshot};

    fn schedule() -> ScheduleSnapshot {
        let sys =
            |writes: Vec<CompIx>, reads: Vec<CompIx>, res: Vec<(CompIx, bool)>| SystemSnapshot {
                queries: vec![QueryAccess {
                    writes,
                    reads,
                    ..Default::default()
                }],
                resources: res
                    .into_iter()
                    .map(|(comp, write)| ResourceAccess { comp, write })
                    .collect(),
                ..Default::default()
            };
        ScheduleSnapshot {
            systems: vec![
                sys(vec![0], vec![], vec![(2, true)]),  // 0: writes A, writes R
                sys(vec![], vec![0], vec![]),           // 1: reads A
                sys(vec![], vec![0], vec![(2, false)]), // 2: reads A, reads R (unordered)
                sys(vec![0], vec![], vec![]),           // 3: writes A after 1
            ],
            flat_edges: vec![
                FlatEdge {
                    from: 0,
                    to: 1,
                    kind: OrderKind::Explicit,
                },
                FlatEdge {
                    from: 1,
                    to: 3,
                    kind: OrderKind::Explicit,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn edges_follow_reachability() {
        let edges = flow_edges(&schedule());
        let has = |from, to, comp, kind| {
            edges
                .iter()
                .any(|e| e.from == from && e.to == to && e.comp == comp && e.kind == kind)
        };
        assert!(has(0, 1, 0, FlowKind::WriteRead));
        assert!(has(0, 3, 0, FlowKind::WriteWrite), "{edges:?}");
        assert!(
            !has(0, 2, 0, FlowKind::WriteRead),
            "unordered pair must not flow"
        );
        assert!(
            !has(0, 2, 2, FlowKind::WriteRead),
            "resource flow needs ordering too"
        );
        assert!(!has(3, 1, 0, FlowKind::WriteRead), "no backwards flow");
    }

    #[test]
    fn reachability_is_transitive() {
        let reach = reachability(&schedule());
        assert!(reaches(&reach, 0, 3));
        assert!(!reaches(&reach, 3, 0));
        assert!(!reaches(&reach, 0, 2));
    }
}
