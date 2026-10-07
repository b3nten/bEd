//! Stable tree row transitions with variable-height clipping and native hit testing.
use dear_imgui_rs::{ClipRectToken, Ui};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::Hash,
    ops::Range,
    rc::Rc,
};

const DURATION: f64 = 0.168;
const STAGGER: f64 = 0.012;

#[derive(Clone, Copy, Debug)]
struct Transition {
    from: f32,
    to: f32,
    began: f64,
}
impl Transition {
    fn value(self, now: f64) -> f32 {
        let t = ((now - self.began) / DURATION).clamp(0.0, 1.0) as f32;
        let eased = t * t * (3.0 - 2.0 * t);
        self.from + (self.to - self.from) * eased
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RowMotion {
    pub alpha: f32,
    pub height: f32,
    pub interactive: bool,
}
impl Default for RowMotion {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            height: 1.0,
            interactive: true,
        }
    }
}

pub(crate) struct TreeAnimation<K> {
    rows: Vec<K>,
    desired: Vec<K>,
    transitions: HashMap<K, Transition>,
    now: f64,
    last_frame: Option<usize>,
    animating: bool,
    offsets: RefCell<Option<(f32, Rc<Vec<f32>>)>>,
}
impl<K> Default for TreeAnimation<K> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            desired: Vec::new(),
            transitions: HashMap::new(),
            now: 0.0,
            last_frame: None,
            animating: false,
            offsets: RefCell::new(None),
        }
    }
}
impl<K: Clone + Eq + Hash> TreeAnimation<K> {
    /// Closing rows retain their order while their height eases to zero. Reversals
    /// start at the current height; wall time prevents duplicate draws advancing twice.
    pub fn update(
        &mut self,
        desired: &[K],
        now: f64,
        frame: usize,
        enabled: bool,
        animate_mount: bool,
        reset: bool,
    ) {
        let mounting = reset
            || self
                .last_frame
                .is_none_or(|last| frame > last.saturating_add(1));
        self.now = now;
        self.last_frame = Some(frame);
        let unchanged = self.desired == desired;
        if !mounting && unchanged && (!self.animating || enabled) {
            if self.animating {
                self.advance();
            }
            return;
        }
        if !unchanged {
            self.desired = desired.to_vec();
        }
        if !enabled || (mounting && !animate_mount) {
            self.rows = desired.to_vec();
            self.transitions = desired
                .iter()
                .cloned()
                .map(|key| {
                    (
                        key,
                        Transition {
                            from: 1.0,
                            to: 1.0,
                            began: now,
                        },
                    )
                })
                .collect();
            self.animating = false;
            return;
        }
        if mounting {
            self.rows.clear();
            self.transitions.clear();
        }
        self.transitions
            .retain(|_, state| state.to > 0.0 || state.value(now) > 0.0001);
        self.rows.retain(|key| self.transitions.contains_key(key));
        let wanted: HashSet<_> = desired.iter().cloned().collect();
        for (key, state) in &mut self.transitions {
            let target = if wanted.contains(key) { 1.0 } else { 0.0 };
            if state.to != target {
                *state = Transition {
                    from: state.value(now),
                    to: target,
                    began: now,
                };
            }
        }
        let mut entering = 0;
        for key in desired {
            self.transitions.entry(key.clone()).or_insert_with(|| {
                let began = now + STAGGER * entering.min(5) as f64;
                entering += 1;
                Transition {
                    from: 0.0,
                    to: 1.0,
                    began,
                }
            });
        }
        // Attach closing runs to the next surviving row, preserving order even
        // when an asynchronous directory/outline update inserts new children.
        let mut anchor = None;
        let mut closing: HashMap<Option<K>, Vec<K>> = HashMap::new();
        for key in self.rows.iter().rev() {
            if wanted.contains(key) {
                anchor = Some(key.clone());
            } else if self
                .transitions
                .get(key)
                .is_some_and(|state| state.value(now) > 0.0001)
            {
                closing.entry(anchor.clone()).or_default().push(key.clone());
            }
        }
        self.rows.clear();
        for key in desired {
            if let Some(run) = closing.remove(&Some(key.clone())) {
                self.rows.extend(run.into_iter().rev());
            }
            self.rows.push(key.clone());
        }
        if let Some(run) = closing.remove(&None) {
            self.rows.extend(run.into_iter().rev());
        }
        self.advance();
    }
    fn advance(&mut self) {
        self.transitions
            .retain(|_, state| state.to > 0.0 || state.value(self.now) > 0.0001);
        self.rows.retain(|key| self.transitions.contains_key(key));
        self.animating = self
            .transitions
            .values()
            .any(|state| state.from != state.to && self.now < state.began + DURATION);
    }
    pub fn rows(&self) -> &[K] {
        &self.rows
    }
    pub fn contains(&self, key: &K) -> bool {
        self.transitions.contains_key(key)
    }
    pub fn sample(&self, key: &K) -> RowMotion {
        self.transitions
            .get(key)
            .map_or_else(RowMotion::default, |state| {
                let value = state.value(self.now);
                RowMotion {
                    alpha: value,
                    height: value,
                    interactive: state.to > 0.0 && value > 0.05,
                }
            })
    }
    /// Remove invalid or newly filtered nodes immediately instead of displaying stale rows.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.rows.retain(|key| keep(key));
        let remaining: HashSet<_> = self.rows.iter().cloned().collect();
        self.desired.retain(|key| remaining.contains(key));
        self.transitions.retain(|key, _| remaining.contains(key));
    }
    pub fn layout(&self, ui: &Ui, row_height: f32) -> TreeLayout {
        let cached = self.offsets.borrow();
        let offsets = if !self.animating
            && cached.as_ref().is_some_and(|(height, offsets)| {
                *height == row_height && offsets.len() == self.rows.len() + 1
            }) {
            Rc::clone(&cached.as_ref().unwrap().1)
        } else {
            let mut offsets = Vec::with_capacity(self.rows.len() + 1);
            offsets.push(0.0);
            for key in &self.rows {
                let height = if self.animating {
                    self.sample(key).height
                } else {
                    1.0
                };
                offsets.push(offsets.last().copied().unwrap() + height * row_height);
            }
            Rc::new(offsets)
        };
        drop(cached);
        if !self.animating {
            *self.offsets.borrow_mut() = Some((row_height, Rc::clone(&offsets)));
        }
        TreeLayout::new(ui, offsets)
    }
}

pub(crate) struct TreeLayout {
    origin: [f32; 2],
    screen: [f32; 2],
    clip_min: [f32; 2],
    clip_max: [f32; 2],
    offsets: Rc<Vec<f32>>,
    previous_max_y: f32,
    pub visible: Range<usize>,
}
impl TreeLayout {
    fn new(ui: &Ui, offsets: Rc<Vec<f32>>) -> Self {
        let origin = ui.cursor_pos();
        let screen = ui.cursor_screen_pos();
        let draw = ui.get_window_draw_list();
        let clip_min = draw.clip_rect_min();
        let clip_max = draw.clip_rect_max();
        let previous_max_y = ui.with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetCurrentWindow())
                .DC
                .CursorMaxPos
                .y
        });
        let top = clip_min[1] - screen[1];
        let bottom = clip_max[1] - screen[1];
        let count = offsets.len() - 1;
        let first = offsets[1..].partition_point(|end| *end <= top).min(count);
        let last = offsets[..count]
            .partition_point(|start| *start < bottom)
            .max(first);
        Self {
            origin,
            screen,
            clip_min,
            clip_max,
            offsets,
            previous_max_y,
            visible: first..last,
        }
    }
    pub fn position(&self, index: usize) -> [f32; 2] {
        [self.origin[0], self.origin[1] + self.offsets[index]]
    }
    pub fn height(&self, index: usize) -> f32 {
        self.offsets[index + 1] - self.offsets[index]
    }
    pub fn row_clip<'ui>(&self, ui: &'ui Ui, index: usize) -> ClipRectToken<'ui> {
        ui.push_clip_rect(
            [self.clip_min[0], self.screen[1] + self.offsets[index]],
            [self.clip_max[0], self.screen[1] + self.offsets[index + 1]],
            true,
        )
    }
    pub fn finish(&self, ui: &Ui) {
        ui.set_cursor_pos(self.origin);
        ui.dummy([0.0, self.offsets.last().copied().unwrap_or(0.0)]);
        // Widgets submit their full text height even while their row is shrinking.
        // Keep the scroll extent tied to the animated list, including its final row.
        ui.with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetCurrentWindow())
                .DC
                .CursorMaxPos
                .y = self
                .previous_max_y
                .max(self.screen[1] + self.offsets.last().copied().unwrap_or(0.0));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapsing_height_moves_following_rows_continuously_and_reverses() {
        let mut motion = TreeAnimation::default();
        motion.update(&["root", "child", "sibling"], 0.0, 1, true, false, false);
        motion.update(&["root", "sibling"], 1.0, 2, true, false, false);
        motion.update(&["root", "sibling"], 1.084, 3, true, false, false);
        assert_eq!(motion.rows(), &["root", "child", "sibling"]);
        let closing = motion.sample(&"child");
        assert!((closing.height - 0.5).abs() < 0.001);
        assert!(!closing.interactive);
        let sibling_y = (motion.sample(&"root").height + closing.height) * 20.0;
        assert!((sibling_y - 30.0).abs() < 0.001);
        motion.update(&["root", "child", "sibling"], 1.084, 3, true, false, false);
        assert!((motion.sample(&"child").height - closing.height).abs() < 0.001);
        assert!(motion.sample(&"child").interactive);
        motion.update(&["root", "child", "sibling"], 1.168, 4, true, false, false);
        assert!((motion.sample(&"child").height - 0.75).abs() < 0.001);
    }
    #[test]
    fn toggle_off_snaps_to_target_and_finished_closures_leave_no_gap() {
        let mut motion = TreeAnimation::default();
        motion.update(&[1, 2, 3], 0.0, 1, true, false, false);
        motion.update(&[1, 3], 1.0, 2, true, false, false);
        motion.update(&[1, 3], 1.02, 3, false, false, false);
        assert_eq!(motion.rows(), &[1, 3]);
        assert_eq!(motion.sample(&1).height, 1.0);
        motion.update(&[1, 2, 3], 1.03, 4, true, false, false);
        assert_eq!(motion.sample(&2).height, 0.0);
        motion.update(&[1, 3], 1.1, 5, true, false, false);
        motion.update(&[1, 3], 1.3, 6, true, false, false);
        assert_eq!(motion.rows(), &[1, 3]);
    }
    #[test]
    fn mounts_stagger_and_missing_nodes_can_be_removed_immediately() {
        let mut motion = TreeAnimation::default();
        motion.update(&[1, 2, 3], 0.0, 1, true, true, false);
        motion.update(&[1, 2, 3], 0.07, 2, true, true, false);
        assert!(motion.sample(&1).alpha > motion.sample(&2).alpha);
        motion.retain(|key| *key != 2);
        assert_eq!(motion.rows(), &[1, 3]);
        motion.update(&[1, 3], 1.0, 9, true, true, false);
        assert_eq!(motion.sample(&1).height, 0.0);
    }
}
