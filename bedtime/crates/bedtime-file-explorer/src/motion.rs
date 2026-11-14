//! Rows moving when the tree changes: rows that appear slide out from under
//! the nearest folder already shown and fade in; rows that go (a folder
//! closed) slide back under the nearest one still shown and fade out; the
//! rest slide to their new places. The first rows a tree shows don't move.
use crate::Row;
use bedtime_ui::{MotionPolicy, tree::RowMotion};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// A row's motion, when it last changed course, and for one coming out of or
/// going into a folder, the bottom of that folder's row (it's hidden above).
struct Moving {
    motion: RowMotion,
    since: f64,
    floor: Option<f32>,
}

#[derive(Default)]
pub(crate) struct Motions {
    /// The rows the motions are for.
    shown: Option<Arc<[Row]>>,
    moving: HashMap<PathBuf, Moving>,
    /// Rows no longer in the tree, until they've faded.
    leaving: Vec<(Row, Moving)>,
}

/// The y of `path`'s nearest ancestor among `rows` (an index by path).
fn ancestor_y(path: &Path, rows: &HashMap<PathBuf, usize>, height: f32) -> Option<f32> {
    path.ancestors()
        .skip(1)
        .find_map(|ancestor| rows.get(ancestor))
        .map(|&index| index as f32 * height)
}

impl Motions {
    /// Start moving what changed between the rows shown and `rows`.
    pub fn update(&mut self, rows: &Arc<[Row]>, height: f32, now: f64, policy: MotionPolicy) {
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, rows))
        {
            return;
        }
        let Some(old) = self.shown.replace(rows.clone()) else {
            return;
        };
        let index = |rows: &'_ [Row]| -> HashMap<PathBuf, usize> {
            rows.iter()
                .enumerate()
                .map(|(i, row)| (row.path.clone(), i))
                .collect()
        };
        let (old_index, new_index) = (index(&old), index(rows));
        for (index, row) in old.iter().enumerate() {
            if new_index.contains_key(&row.path) {
                continue;
            }
            let from = index as f32 * height;
            let mut motion = self.moving.remove(&row.path).map_or_else(
                || RowMotion::new(from, from, 1.0, now),
                |moving| moving.motion,
            );
            let folder = ancestor_y(&row.path, &new_index, height);
            motion.retarget(folder.unwrap_or(from), 0.0, now, policy);
            self.leaving.push((
                row.clone(),
                Moving {
                    motion,
                    since: now,
                    floor: folder.map(|y| y + height),
                },
            ));
        }
        for (index, row) in rows.iter().enumerate() {
            let target = index as f32 * height;
            if let Some(moving) = self.moving.get_mut(&row.path) {
                moving.motion.retarget(target, 1.0, now, policy);
                moving.since = now;
            } else if let Some(at) = self.leaving.iter().position(|(r, _)| r.path == row.path) {
                // Back before it had gone: from where it's drawn.
                let (_, mut moving) = self.leaving.remove(at);
                moving.motion.retarget(target, 1.0, now, policy);
                moving.since = now;
                self.moving.insert(row.path.clone(), moving);
            } else if let Some(&before) = old_index.get(&row.path) {
                if before != index {
                    let from = before as f32 * height;
                    let motion = RowMotion::new(from, target, 1.0, now);
                    let moving = Moving {
                        motion,
                        since: now,
                        floor: None,
                    };
                    self.moving.insert(row.path.clone(), moving);
                }
            } else {
                let folder = ancestor_y(&row.path, &old_index, height);
                let motion = RowMotion::new(folder.unwrap_or(target), target, 0.0, now);
                let moving = Moving {
                    motion,
                    since: now,
                    floor: folder.map(|y| y + height),
                };
                self.moving.insert(row.path.clone(), moving);
            }
        }
        // A burst of changes mustn't keep too many ghosts.
        let cap = rows.len().max(old.len()).max(64);
        if self.leaving.len() > cap {
            self.leaving.drain(..self.leaving.len() - cap);
        }
    }

    /// Where the row at `index` is drawn, how opaque, and above what it's
    /// hidden (its folder's row, while it comes out of it).
    pub fn sample(
        &self,
        row: &Row,
        index: usize,
        height: f32,
        now: f64,
        policy: MotionPolicy,
    ) -> (f32, f32, Option<f32>) {
        self.moving
            .get(&row.path)
            .map_or((index as f32 * height, 1.0, None), |moving| {
                let (y, alpha) = moving.motion.sample(now, policy);
                (y, alpha, moving.floor)
            })
    }

    /// The rows that are moving, by index in `rows`: drawn even when their
    /// place is out of view.
    pub fn moving<'a>(&'a self, rows: &'a [Row]) -> impl Iterator<Item = usize> + 'a {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| self.moving.contains_key(&row.path))
            .map(|(index, _)| index)
    }

    /// The rows on their way out: where they're drawn, how opaque, and above
    /// what they're hidden.
    pub fn leaving(
        &self,
        now: f64,
        policy: MotionPolicy,
    ) -> impl Iterator<Item = (&Row, f32, f32, Option<f32>)> {
        self.leaving.iter().map(move |(row, moving)| {
            let (y, alpha) = moving.motion.sample(now, policy);
            (row, y, alpha, moving.floor)
        })
    }

    /// Let go of finished motions. Whether any are still running.
    pub fn settle(&mut self, now: f64, policy: MotionPolicy) -> bool {
        let done = |since: f64| !policy.enabled || now - since >= f64::from(policy.seconds);
        self.moving.retain(|_, moving| !done(moving.since));
        self.leaving.retain(|(_, moving)| !done(moving.since));
        !self.moving.is_empty() || !self.leaving.is_empty()
    }
}
