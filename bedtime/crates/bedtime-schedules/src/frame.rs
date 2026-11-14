//! The whole frame as one sequence: the schedules `Main` runs, in order, flattened into the
//! systems they execute, plus every component/resource access as marks on component "lanes".
//!
//! This is the model behind the frame timeline views. Order inside a schedule is the executor's
//! order; systems in the same *wave* (topological level) may run in parallel.

use std::collections::{HashMap, HashSet};

use crate::{
    layout::ranks_of,
    snapshot::{AppSnapshot, CompIx, ScheduleSnapshot, SysIx, SystemSnapshot},
};

/// Where a schedule sits in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandKind {
    Startup,
    Main,
    /// Runs zero or more times per frame inside `RunFixedMainLoop`.
    Fixed,
    /// `OnEnter`/`OnExit`/`OnTransition`, run by `StateTransition` when a state changes.
    Transition,
    /// Not reachable from `Main` (e.g. sub-app or custom schedules).
    Other,
}

#[derive(Clone, Debug)]
pub struct FrameBand {
    /// Index into `AppSnapshot::schedules`.
    pub schedule: usize,
    pub label: String,
    pub kind: BandKind,
    /// Column range `[first, last)` in the sequence; empty when no system was included.
    pub first: usize,
    pub last: usize,
    /// Nesting depth (fixed and transition schedules are nested).
    pub depth: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameSystem {
    pub schedule: usize,
    pub system: SysIx,
    /// Position in the sequence.
    pub column: usize,
    /// Topological level inside the schedule.
    pub wave: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkKind {
    Write,
    Read,
    /// Only used as a `With`/`Without` filter.
    Filter,
    /// Archetypal access (`Has<T>`).
    Archetypal,
    /// Read from a run condition.
    ConditionRead,
    ConditionWrite,
}

impl MarkKind {
    pub fn is_write(self) -> bool {
        matches!(self, MarkKind::Write | MarkKind::ConditionWrite)
    }
    pub fn is_read(self) -> bool {
        matches!(self, MarkKind::Read | MarkKind::ConditionRead)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub column: usize,
    pub comp: CompIx,
    pub kind: MarkKind,
}

/// A write followed by the reads (and the next write) it feeds, on one lane.
#[derive(Clone, Debug)]
pub struct Handoff {
    pub comp: CompIx,
    pub from: usize,
    pub to: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct FrameSequence {
    pub systems: Vec<FrameSystem>,
    pub bands: Vec<FrameBand>,
    pub marks: Vec<Mark>,
    /// Lanes (components) in order of first access.
    pub lanes: Vec<CompIx>,
    pub handoffs: Vec<Handoff>,
}

impl FrameSequence {
    /// Drops every column whose system touches none of the current [`lanes`](Self::lanes), and
    /// renumbers the columns that remain.
    ///
    /// Lane filtering only decides which *rows* are drawn; on its own it leaves the frame's full
    /// width in place, so a search for one component still shows every system in the frame. This
    /// narrows the sequence to the systems that actually read or write a shown lane.
    ///
    /// Marks of hidden lanes are kept on the columns that survive, so a system's tooltip still
    /// reports everything it touches.
    pub fn retain_columns_touching_lanes(&mut self) {
        let shown: HashSet<CompIx> = self.lanes.iter().copied().collect();
        let mut keep = vec![false; self.systems.len()];
        for m in &self.marks {
            if shown.contains(&m.comp) {
                keep[m.column] = true;
            }
        }
        // `new_column[old] == None` for a dropped column; the running count is also what every
        // band boundary (an index into the columns) has to be remapped to.
        let mut new_column = Vec::with_capacity(keep.len() + 1);
        let mut kept = 0;
        for k in &keep {
            new_column.push(kept);
            kept += usize::from(*k);
        }
        new_column.push(kept); // one past the end, for `band.last`

        self.systems.retain(|fs| keep[fs.column]);
        for fs in &mut self.systems {
            fs.column = new_column[fs.column];
        }
        self.marks.retain(|m| keep[m.column]);
        for m in &mut self.marks {
            m.column = new_column[m.column];
        }
        self.handoffs.retain_mut(|h| {
            h.to.retain(|c| keep[*c]);
            if !keep[h.from] || h.to.is_empty() {
                return false;
            }
            h.from = new_column[h.from];
            for c in &mut h.to {
                *c = new_column[*c];
            }
            true
        });
        for band in &mut self.bands {
            band.first = new_column[band.first];
            band.last = new_column[band.last];
        }
    }
}

pub struct FrameOptions<'a> {
    pub include_startup: bool,
    pub include_transitions: bool,
    pub include_other: bool,
    /// Which systems to include (e.g. hide Bevy internals).
    pub include_system: &'a dyn Fn(&SystemSnapshot) -> bool,
    /// Lanes are only created for components with at least one mark from an included system;
    /// this additionally drops lanes that have no write at all (pure reads of e.g. `Time`).
    pub require_write: bool,
}

impl Default for FrameOptions<'static> {
    fn default() -> Self {
        Self {
            include_startup: false,
            include_transitions: true,
            include_other: false,
            include_system: &|s| !s.flags.internal && !s.flags.is_self,
            require_write: false,
        }
    }
}

fn kind_of(label: &str, snapshot: &AppSnapshot) -> BandKind {
    if snapshot.main_order.iter().any(|l| l == label) {
        BandKind::Main
    } else if snapshot.startup_order.iter().any(|l| l == label) {
        BandKind::Startup
    } else if snapshot.fixed_main_order.iter().any(|l| l == label) {
        BandKind::Fixed
    } else if label.starts_with("OnEnter(")
        || label.starts_with("OnExit(")
        || label.starts_with("OnTransition")
    {
        BandKind::Transition
    } else {
        BandKind::Other
    }
}

/// Builds the frame sequence.
pub fn frame_sequence(snapshot: &AppSnapshot, options: &FrameOptions) -> FrameSequence {
    let index_of: HashMap<&str, usize> = snapshot
        .schedules
        .iter()
        .enumerate()
        .map(|(i, s)| (s.label.as_str(), i))
        .collect();
    let mut seq = FrameSequence::default();

    let push_schedule = |seq: &mut FrameSequence, label: &str, kind: BandKind, depth: u8| {
        let Some(&si) = index_of.get(label) else {
            return;
        };
        let schedule = &snapshot.schedules[si];
        let ranks = ranks_of(schedule);
        let first = seq.systems.len();
        let order: Vec<SysIx> = if schedule.exec_order.is_empty() {
            (0..schedule.systems.len() as SysIx).collect()
        } else {
            schedule.exec_order.clone()
        };
        for sys in order {
            let system = &schedule.systems[sys as usize];
            if !(options.include_system)(system) {
                continue;
            }
            seq.systems.push(FrameSystem {
                schedule: si,
                system: sys,
                column: seq.systems.len(),
                wave: ranks.get(sys as usize).copied().unwrap_or(0),
            });
        }
        seq.bands.push(FrameBand {
            schedule: si,
            label: label.to_string(),
            kind,
            first,
            last: seq.systems.len(),
            depth,
        });
    };

    if options.include_startup {
        for label in &snapshot.startup_order {
            push_schedule(&mut seq, label, BandKind::Startup, 0);
        }
    }
    let transitions: Vec<&str> = snapshot
        .schedules
        .iter()
        .map(|s| s.label.as_str())
        .filter(|l| kind_of(l, snapshot) == BandKind::Transition)
        .collect();
    for label in &snapshot.main_order {
        push_schedule(&mut seq, label, BandKind::Main, 0);
        if label == "RunFixedMainLoop" {
            for fixed in &snapshot.fixed_main_order {
                push_schedule(&mut seq, fixed, BandKind::Fixed, 1);
            }
        }
        if label == "StateTransition" && options.include_transitions {
            for t in &transitions {
                push_schedule(&mut seq, t, BandKind::Transition, 1);
            }
        }
    }
    if options.include_other {
        for schedule in &snapshot.schedules {
            if kind_of(&schedule.label, snapshot) == BandKind::Other {
                push_schedule(&mut seq, &schedule.label, BandKind::Other, 0);
            }
        }
    }

    // Marks.
    for fs in &seq.systems {
        let system = &snapshot.schedules[fs.schedule].systems[fs.system as usize];
        let mut add = |comp: CompIx, kind: MarkKind| {
            seq.marks.push(Mark {
                column: fs.column,
                comp,
                kind,
            });
        };
        for q in &system.queries {
            for c in &q.writes {
                add(*c, MarkKind::Write);
            }
            for c in &q.reads {
                add(*c, MarkKind::Read);
            }
            for c in &q.archetypal {
                add(*c, MarkKind::Archetypal);
            }
            for f in &q.filters {
                for c in f.with.iter().chain(&f.without) {
                    add(*c, MarkKind::Filter);
                }
            }
        }
        for r in &system.resources {
            add(
                r.comp,
                if r.write {
                    MarkKind::Write
                } else {
                    MarkKind::Read
                },
            );
        }
        for cond in &system.conditions {
            for q in &cond.queries {
                for c in &q.writes {
                    add(*c, MarkKind::ConditionWrite);
                }
                for c in &q.reads {
                    add(*c, MarkKind::ConditionRead);
                }
            }
            for r in &cond.resources {
                add(
                    r.comp,
                    if r.write {
                        MarkKind::ConditionWrite
                    } else {
                        MarkKind::ConditionRead
                    },
                );
            }
        }
    }
    // Strongest mark per (column, comp).
    seq.marks.sort_by_key(|m| (m.column, m.comp, m.kind));
    seq.marks.dedup_by_key(|m| (m.column, m.comp));

    // Lanes by first access, optionally only lanes with a write.
    let mut first_seen: HashMap<CompIx, usize> = HashMap::new();
    let mut has_write: HashMap<CompIx, bool> = HashMap::new();
    for m in &seq.marks {
        first_seen.entry(m.comp).or_insert(m.column);
        *has_write.entry(m.comp).or_default() |= m.kind.is_write();
    }
    let mut lanes: Vec<CompIx> = first_seen
        .keys()
        .copied()
        .filter(|c| !options.require_write || has_write[c])
        .collect();
    lanes.sort_by_key(|c| (first_seen[c], *c));
    seq.lanes = lanes;

    // Hand-offs: each write feeds the following reads up to and including the next write.
    let mut by_lane: HashMap<CompIx, Vec<&Mark>> = HashMap::new();
    for m in &seq.marks {
        by_lane.entry(m.comp).or_default().push(m);
    }
    for comp in &seq.lanes {
        let marks = &by_lane[comp];
        let mut current: Option<Handoff> = None;
        for m in marks {
            if m.kind.is_write() {
                if let Some(h) = current.take() {
                    let mut h = h;
                    h.to.push(m.column);
                    seq.handoffs.push(h);
                }
                current = Some(Handoff {
                    comp: *comp,
                    from: m.column,
                    to: Vec::new(),
                });
            } else if m.kind.is_read()
                && let Some(h) = current.as_mut()
            {
                h.to.push(m.column);
            }
        }
        if let Some(h) = current.take()
            && !h.to.is_empty()
        {
            seq.handoffs.push(h);
        }
    }
    seq
}

/// Convenience: the system snapshot behind a frame column.
pub fn system_at<'a>(
    snapshot: &'a AppSnapshot,
    seq: &FrameSequence,
    column: usize,
) -> &'a SystemSnapshot {
    let fs = &seq.systems[column];
    &snapshot.schedules[fs.schedule].systems[fs.system as usize]
}

/// Convenience: the schedule snapshot behind a frame column.
pub fn schedule_at<'a>(
    snapshot: &'a AppSnapshot,
    seq: &FrameSequence,
    column: usize,
) -> &'a ScheduleSnapshot {
    &snapshot.schedules[seq.systems[column].schedule]
}
