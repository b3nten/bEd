//! Folding changes only visible geometry; the document and target state stay fixed.
use bed_editing::folding::FoldRange;
use std::cmp::Reverse;

const DURATION: f64 = 0.16;

fn range_key(range: &FoldRange) -> (i32, Reverse<i32>) {
    (range.start_line, Reverse(range.end_line))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FoldVisual {
    pub range: FoldRange,
    /// Zero hides the body; one displays its full height at normal text size.
    pub openness: f32,
}

#[derive(Clone, Copy, Debug)]
struct Transition {
    range: FoldRange,
    from: f32,
    to: f32,
    started: f64,
    duration: f64,
}
impl Transition {
    fn openness(self, now: f64) -> f32 {
        let t = ((now - self.started) / self.duration).clamp(0.0, 1.0) as f32;
        let eased = t * t * (3.0 - 2.0 * t);
        self.from + (self.to - self.from) * eased
    }
    fn active(self, now: f64) -> bool {
        now < self.started + self.duration
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FoldAnimation {
    // Targets use FoldState's normalized order; transitions retain that order
    // so bulk toggles and frame sampling can look up ranges with binary search.
    collapsed: Vec<FoldRange>,
    transitions: Vec<Transition>,
}
impl FoldAnimation {
    pub fn snap(&mut self, collapsed: &[FoldRange]) {
        self.collapsed = collapsed.to_vec();
        self.transitions.clear();
    }

    /// Capture the current visible height before changing direction. Unchanged
    /// nested ranges keep their own progress instead of restarting together.
    pub fn retarget(&mut self, collapsed: &[FoldRange], now: f64, animate: bool) {
        if !animate {
            self.snap(collapsed);
            return;
        }
        self.transitions.retain(|transition| transition.active(now));
        if self.collapsed == collapsed {
            return;
        }
        // Unchanged targets retain their original start time. Reversing ranges
        // capture their current openness when rebuilt below.
        let mut transitions: Vec<_> = self
            .transitions
            .iter()
            .filter(|transition| {
                let key = range_key(&transition.range);
                self.collapsed.binary_search_by_key(&key, range_key).is_ok()
                    == collapsed.binary_search_by_key(&key, range_key).is_ok()
            })
            .copied()
            .collect();
        let mut ranges = self.collapsed.clone();
        ranges.extend_from_slice(collapsed);
        ranges.sort_by_key(range_key);
        ranges.dedup();
        for range in ranges {
            let key = range_key(&range);
            let was_closed = self.collapsed.binary_search_by_key(&key, range_key).is_ok();
            let is_closed = collapsed.binary_search_by_key(&key, range_key).is_ok();
            if was_closed == is_closed {
                continue;
            }
            let from = self
                .transitions
                .binary_search_by_key(&key, |transition| range_key(&transition.range))
                .map_or(if was_closed { 0.0 } else { 1.0 }, |index| {
                    self.transitions[index].openness(now)
                });
            let to = if is_closed { 0.0 } else { 1.0 };
            if from != to {
                transitions.push(Transition {
                    range,
                    from,
                    to,
                    started: now,
                    duration: DURATION * f64::from((to - from).abs()),
                });
            }
        }
        transitions.sort_by_key(|transition| range_key(&transition.range));
        self.transitions = transitions;
        self.collapsed = collapsed.to_vec();
    }

    pub fn sample(&self, now: f64) -> Vec<FoldVisual> {
        let mut visual: Vec<_> = self
            .collapsed
            .iter()
            .map(|range| FoldVisual {
                range: *range,
                openness: self
                    .transitions
                    .binary_search_by_key(&range_key(range), |transition| {
                        range_key(&transition.range)
                    })
                    .map_or(0.0, |index| self.transitions[index].openness(now)),
            })
            .collect();
        // Opening ranges have left the collapsed targets, but their bodies
        // remain in the projection until they reach full height.
        for transition in self
            .transitions
            .iter()
            .filter(|transition| transition.to == 1.0)
        {
            let openness = transition.openness(now);
            if openness < 1.0 {
                visual.push(FoldVisual {
                    range: transition.range,
                    openness,
                });
            }
        }
        visual.sort_by_key(|fold| range_key(&fold.range));
        visual
    }

    pub fn is_active(&self, now: f64) -> bool {
        self.transitions
            .iter()
            .any(|transition| transition.active(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OUTER: FoldRange = FoldRange {
        start_line: 1,
        end_line: 8,
    };
    const INNER: FoldRange = FoldRange {
        start_line: 3,
        end_line: 5,
    };

    #[test]
    fn closing_eases_from_full_height_to_a_collapsed_fold() {
        let mut animation = FoldAnimation::default();
        animation.retarget(&[OUTER], 1.0, true);
        assert_eq!(animation.sample(1.0)[0].openness, 1.0);
        assert!((animation.sample(1.08)[0].openness - 0.5).abs() < 0.001);
        assert_eq!(animation.sample(1.16)[0].openness, 0.0);
        assert!(!animation.is_active(1.16));
    }

    #[test]
    fn completed_transitions_are_released_while_targets_stay_unchanged() {
        let mut animation = FoldAnimation::default();
        animation.retarget(&[OUTER, INNER], 0.0, true);
        animation.retarget(&[OUTER, INNER], 0.2, true);
        // Retaining completed Fold All transitions would make every idle sample
        // revisit every completed transition and scan all collapsed ranges.
        assert!(animation.transitions.is_empty());
        assert_eq!(
            animation.sample(0.2),
            vec![
                FoldVisual {
                    range: OUTER,
                    openness: 0.0
                },
                FoldVisual {
                    range: INNER,
                    openness: 0.0
                },
            ]
        );
        animation.retarget(&[], 0.2, true);
        animation.retarget(&[], 0.4, true);
        assert!(animation.transitions.is_empty());
        assert!(animation.sample(0.4).is_empty());
    }

    #[test]
    fn reversing_preserves_current_height_and_finishes_in_proportion_to_distance() {
        let mut animation = FoldAnimation::default();
        animation.retarget(&[OUTER], 0.0, true);
        let before = animation.sample(0.08)[0].openness;
        animation.retarget(&[], 0.08, true);
        assert_eq!(animation.sample(0.08)[0].openness, before);
        assert!((animation.sample(0.12)[0].openness - 0.75).abs() < 0.001);
        assert!(animation.sample(0.16).is_empty());
        assert!(!animation.is_active(0.16));
    }

    #[test]
    fn thousands_of_folds_preserve_progress_when_half_reverse_together() {
        let ranges: Vec<_> = (0..4_000)
            .map(|index| FoldRange {
                start_line: index * 3,
                end_line: index * 3 + 2,
            })
            .collect();
        let alternating: Vec<_> = ranges.iter().step_by(2).copied().collect();
        let mut animation = FoldAnimation::default();
        animation.retarget(&ranges, 0.0, true);
        assert!(
            animation
                .sample(0.08)
                .iter()
                .all(|fold| fold.openness == 0.5)
        );
        animation.retarget(&alternating, 0.08, true);
        let sample = animation.sample(0.12);
        assert_eq!(sample.len(), ranges.len());
        for (index, fold) in sample.iter().enumerate() {
            assert_eq!(fold.range, ranges[index]);
            let expected = if index % 2 == 0 { 0.15625 } else { 0.75 };
            assert!((fold.openness - expected).abs() < 0.001);
        }
        animation.retarget(&alternating, 0.2, true);
        let resting = animation.sample(0.2);
        assert_eq!(resting.len(), alternating.len());
        assert!(resting.iter().all(|fold| fold.openness == 0.0));
        animation.retarget(&ranges, 0.2, true);
        let sample = animation.sample(0.28);
        assert_eq!(sample.len(), ranges.len());
        for (index, fold) in sample.iter().enumerate() {
            let expected = if index % 2 == 0 { 0.0 } else { 0.5 };
            assert!((fold.openness - expected).abs() < 0.001);
        }
    }

    #[test]
    fn opening_parent_preserves_nested_fold_and_disabled_animation_snaps() {
        let mut animation = FoldAnimation::default();
        animation.snap(&[OUTER, INNER]);
        animation.retarget(&[INNER], 0.0, true);
        let sample = animation.sample(0.08);
        assert_eq!(sample.len(), 2);
        assert_eq!(sample[0].openness, 0.5);
        assert_eq!(sample[1].openness, 0.0);
        animation.retarget(&[], 0.08, false);
        assert!(animation.sample(0.08).is_empty());
        assert!(!animation.is_active(0.08));
    }
}
