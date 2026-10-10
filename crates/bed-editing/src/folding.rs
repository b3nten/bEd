//! Whole-line folds belong to a view; document positions never change when folded.
use crate::editor_events::DocumentChange;

/// The opening line stays visible, and `start_line + 1..=end_line` is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub start_line: i32,
    pub end_line: i32,
}

impl FoldRange {
    /// Keep nested or disjoint ranges with one control per opening line.
    /// Line projection can overlap ranges whose syntax nodes are disjoint.
    pub fn normalize(mut ranges: Vec<Self>) -> Vec<Self> {
        ranges.retain(|range| range.start_line >= 0 && range.end_line > range.start_line);
        ranges.sort_by_key(|range| (range.start_line, std::cmp::Reverse(range.end_line)));
        ranges.dedup_by_key(|range| range.start_line);
        let mut enclosing_ends = Vec::new();
        ranges.retain(|range| {
            while enclosing_ends
                .last()
                .is_some_and(|&end| end < range.start_line)
            {
                enclosing_ends.pop();
            }
            if enclosing_ends
                .last()
                .is_some_and(|&end| range.end_line > end)
            {
                return false;
            }
            enclosing_ends.push(range.end_line);
            true
        });
        ranges
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldState {
    ranges: Vec<FoldRange>,
    collapsed: Vec<FoldRange>,
    generation: u64,
    source: Option<(u64, String, String)>,
}

impl FoldState {
    pub fn ranges(&self) -> &[FoldRange] {
        &self.ranges
    }

    pub fn collapsed_ranges(&self) -> &[FoldRange] {
        &self.collapsed
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A view can be dormant while another view changes the document's language
    /// or identity. Clear its old candidates before pending parser work returns.
    /// The first stamp adopts existing ranges, including caller-supplied folds.
    pub fn set_source(&mut self, generation: u64, path: &str, language_id: &str) -> bool {
        let matches = self.source.as_ref().is_some_and(|source| {
            source.0 == generation && source.1 == path && source.2 == language_id
        });
        let changed = self.source.is_some() && !matches;
        if !matches {
            self.source = Some((generation, path.to_owned(), language_id.to_owned()));
        }
        if changed {
            self.set_ranges(Vec::new());
        }
        changed
    }

    /// Normalize parser output into nested or disjoint folds with one control
    /// per opening line. Only unchanged ranges retain their collapsed state.
    pub fn set_ranges(&mut self, ranges: Vec<FoldRange>) -> bool {
        if self.ranges == ranges {
            return false;
        }
        let normalized = FoldRange::normalize(ranges);
        if self.ranges == normalized {
            return false;
        }
        self.collapsed.retain(|r| normalized.contains(r));
        self.ranges = normalized;
        self.generation += 1;
        true
    }

    /// A header selects its own fold; otherwise select the innermost fold
    /// containing the requested document line.
    pub fn toggle(&mut self, row: i32) -> bool {
        let range = self
            .ranges
            .iter()
            .find(|r| r.start_line == row)
            .copied()
            .or_else(|| {
                self.ranges
                    .iter()
                    .rev()
                    .find(|r| r.start_line < row && row <= r.end_line)
                    .copied()
            });
        let Some(range) = range else {
            return false;
        };
        if let Some(index) = self.collapsed.iter().position(|r| *r == range) {
            self.collapsed.remove(index);
        } else {
            self.collapsed.push(range);
            self.collapsed.sort_by_key(|r| r.start_line);
        }
        self.generation += 1;
        true
    }

    pub fn fold_all(&mut self) -> bool {
        if self.collapsed == self.ranges {
            return false;
        }
        self.collapsed.clone_from(&self.ranges);
        self.generation += 1;
        true
    }

    pub fn unfold_all(&mut self) -> bool {
        if self.collapsed.is_empty() {
            return false;
        }
        self.collapsed.clear();
        self.generation += 1;
        true
    }

    /// Reveal every enclosing fold, retaining unrelated nested fold choices.
    pub fn reveal(&mut self, row: i32) -> bool {
        let old_len = self.collapsed.len();
        self.collapsed
            .retain(|r| !(r.start_line < row && row <= r.end_line));
        let changed = old_len != self.collapsed.len();
        if changed {
            self.generation += 1;
        }
        changed
    }

    pub fn is_hidden(&self, row: i32) -> bool {
        self.collapsed
            .iter()
            .any(|r| r.start_line < row && row <= r.end_line)
    }

    /// Changes are ordered, and each range refers to the document before that
    /// change. Edits inside a fold reveal it; edits touching either structural
    /// boundary invalidate it until the syntax parser supplies fresh ranges.
    pub fn apply_changes(&mut self, changes: &[DocumentChange]) -> bool {
        let previous_ranges = self.ranges.clone();
        let previous_collapsed = self.collapsed.clone();
        for change in changes {
            let mut inserted_lines = 0;
            let mut bytes = change.text.iter().peekable();
            while let Some(&byte) = bytes.next() {
                if byte == b'\r' {
                    inserted_lines += 1;
                    if bytes.peek() == Some(&&b'\n') {
                        bytes.next();
                    }
                } else if byte == b'\n' {
                    inserted_lines += 1;
                }
            }
            let delta = inserted_lines - (change.end_line - change.start_line);
            let transform = |range: &mut FoldRange| {
                if change.end_line < range.start_line {
                    range.start_line += delta;
                    range.end_line += delta;
                    Some(false)
                } else if change.start_line > range.end_line {
                    Some(false)
                } else if change.start_line > range.start_line && change.end_line < range.end_line {
                    range.end_line += delta;
                    Some(true)
                } else {
                    None
                }
            };
            self.ranges.retain_mut(|range| transform(range).is_some());
            self.collapsed
                .retain_mut(|range| transform(range) == Some(false));
        }
        let changed = self.ranges != previous_ranges || self.collapsed != previous_collapsed;
        if changed {
            self.generation += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start_line: i32, end_line: i32) -> FoldRange {
        FoldRange {
            start_line,
            end_line,
        }
    }

    fn change(start_line: i32, end_line: i32, text: &[u8]) -> DocumentChange {
        DocumentChange {
            start_line,
            end_line,
            text: text.to_vec(),
            ..DocumentChange::default()
        }
    }

    #[test]
    fn source_changes_clear_dormant_views_without_clearing_sequential_edits() {
        let mut folds = FoldState::default();
        folds.set_ranges(vec![range(0, 3)]);
        folds.fold_all();
        assert!(!folds.set_source(1, "file.rs", "rs"));
        assert!(!folds.set_source(1, "file.rs", "rs"));
        assert!(folds.is_hidden(1));
        assert!(folds.set_source(1, "file.rs", "json"));
        assert!(folds.ranges().is_empty() && folds.collapsed_ranges().is_empty());
        folds.set_ranges(vec![range(1, 4)]);
        folds.fold_all();
        assert!(folds.set_source(2, "file.rs", "json"));
        assert!(folds.ranges().is_empty() && folds.collapsed_ranges().is_empty());
    }

    #[test]
    fn nested_choices_survive_toggling_a_parent_and_reveal_only_enclosing_folds() {
        let mut folds = FoldState::default();
        folds.set_ranges(vec![range(0, 9), range(2, 4), range(6, 8)]);
        folds.toggle(3); // A body line chooses the innermost range.
        folds.toggle(0);
        assert!(!folds.is_hidden(0));
        assert!(folds.is_hidden(9));
        folds.toggle(0);
        assert_eq!(folds.collapsed_ranges(), &[range(2, 4)]);
        folds.fold_all();
        folds.reveal(3);
        assert!(!folds.is_hidden(3));
        assert!(folds.is_hidden(7));
        assert_eq!(folds.collapsed_ranges(), &[range(6, 8)]);
    }

    #[test]
    fn reparsing_retains_only_matching_folds_and_rejects_crossing_ranges() {
        let mut folds = FoldState::default();
        folds.set_ranges(vec![range(0, 9), range(2, 4), range(6, 8)]);
        folds.fold_all();
        folds.set_ranges(vec![
            range(6, 8),
            range(0, 9),
            range(2, 5),
            range(8, 12),
            range(2, 3),
        ]);
        assert_eq!(folds.ranges(), &[range(0, 9), range(2, 5), range(6, 8)]);
        assert_eq!(folds.collapsed_ranges(), &[range(0, 9), range(6, 8)]);
        let generation = folds.generation();
        assert!(!folds.set_ranges(folds.ranges().to_vec()));
        assert_eq!(folds.generation(), generation);
    }

    #[test]
    fn edits_shift_later_folds_reveal_changed_bodies_and_invalidate_boundaries() {
        let mut folds = FoldState::default();
        folds.set_ranges(vec![range(2, 12), range(4, 6), range(8, 10), range(14, 16)]);
        folds.fold_all();
        folds.apply_changes(&[change(0, 0, b"prefix\r\n")]);
        assert_eq!(
            folds.collapsed_ranges(),
            &[range(3, 13), range(5, 7), range(9, 11), range(15, 17)]
        );
        folds.apply_changes(&[change(6, 6, b"new\n")]);
        assert_eq!(
            folds.ranges(),
            &[range(3, 14), range(5, 8), range(10, 12), range(16, 18)]
        );
        assert_eq!(folds.collapsed_ranges(), &[range(10, 12), range(16, 18)]);
        folds.apply_changes(&[change(10, 10, b"changed header")]);
        assert_eq!(folds.ranges(), &[range(3, 14), range(5, 8), range(16, 18)]);
        assert_eq!(folds.collapsed_ranges(), &[range(16, 18)]);
    }
}
