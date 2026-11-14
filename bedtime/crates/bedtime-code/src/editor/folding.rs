//! Sparse hidden-row intervals over the same wrapping geometry used everywhere.
//! Cost depends on collapsed regions, never the number of document lines.
use crate::editor::{EditorError, EditorView, TextBuffer, TextDelta, TextSnapshot};
use std::ops::Range;

#[derive(Default)]
pub(crate) struct Folding {
    pub collapsed: Vec<Range<usize>>,
    revision: Option<u64>,
    pub map: FoldMap,
    pub display_map: FoldMap,
    visual_targets: Vec<Range<usize>>,
    motions: std::collections::BTreeMap<(usize, usize), FoldMotion>,
    map_key: Option<(Option<u64>, Option<(usize, usize)>, bool)>,
}
#[derive(Default)]
pub(crate) struct FoldMap {
    gaps: Vec<Gap>,
    removed: usize,
    trimmed: f32,
}
struct Gap {
    rows: Range<usize>,
    before: usize,
    trim: f32,
    trimmed_before: f32,
}
struct FoldMotion {
    range: Range<usize>,
    from: f32,
    to: f32,
    started: f64,
}
impl FoldMotion {
    fn value(&self, now: f64) -> f32 {
        let duration = 0.16 * f64::from((self.to - self.from).abs());
        self.from
            + (self.to - self.from) * bedtime_ui::ease(((now - self.started) / duration) as f32)
    }
}
impl FoldMap {
    pub fn height(&self, rows: usize) -> f32 {
        self.rows(rows) as f32 - self.trimmed
    }
    pub fn row_y(&self, row: usize) -> f32 {
        let index = self
            .gaps
            .partition_point(|g| g.rows.start - g.before <= row);
        row as f32
            - if index == 0 {
                0.0
            } else {
                let gap = &self.gaps[index - 1];
                gap.trimmed_before + gap.trim
            }
    }
    pub fn row_at(&self, y: f32) -> usize {
        let index = self
            .gaps
            .partition_point(|g| (g.rows.start - g.before) as f32 - g.trimmed_before - g.trim <= y);
        (y + if index == 0 {
            0.0
        } else {
            let gap = &self.gaps[index - 1];
            gap.trimmed_before + gap.trim
        })
        .floor()
        .max(0.0) as usize
    }
    pub fn row_height(&self, row: usize) -> f32 {
        self.row_y(row + 1) - self.row_y(row)
    }

    pub fn rows(&self, total: usize) -> usize {
        total.saturating_sub(self.removed).max(1)
    }
    pub fn source_row(&self, visible: usize) -> usize {
        let index = self
            .gaps
            .partition_point(|g| g.rows.start - g.before <= visible);
        visible
            + if index == 0 {
                0
            } else {
                let g = &self.gaps[index - 1];
                g.before + g.rows.len()
            }
    }
    pub fn visible_row(&self, source: usize) -> usize {
        let index = self.gaps.partition_point(|g| g.rows.start <= source);
        if index == 0 {
            source
        } else {
            let g = &self.gaps[index - 1];
            source.saturating_sub(g.before + (source - g.rows.start + 1).min(g.rows.len()))
        }
    }
}
impl Folding {
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }
    pub fn reconcile(&mut self, snapshot: &TextSnapshot, changes: Option<&[TextDelta]>) {
        if self.revision == Some(snapshot.revision) {
            return;
        }
        if self.revision.is_some() {
            if let Some(changes) = changes {
                for change in changes {
                    self.collapsed.retain_mut(|fold| {
                        // Any edit touching the declaration expands it. Exact
                        // identity cannot be inferred from byte lengths alone.
                        if change.range.start <= fold.end && change.range.end >= fold.start {
                            return false;
                        }
                        fold.start = change.map_position(fold.start);
                        fold.end = change.map_position(fold.end);
                        true
                    });
                }
            } else {
                self.collapsed.clear();
            }
        }
        self.collapsed.retain(|r| {
            r.start < r.end
                && snapshot.text.is_char_boundary(r.start)
                && snapshot.text.is_char_boundary(r.end)
        });
        self.motions.clear();
        self.visual_targets = self.collapsed.clone();
        self.revision = Some(snapshot.revision);
        self.map_key = None;
    }
    pub fn prepare(
        &mut self,
        text: &TextBuffer,
        wrap: Option<&crate::editor::wrapping::WrapIndex>,
        active: bool,
    ) {
        let key = (self.revision, wrap.map(|w| (w.width, w.rows())), active);
        if self.map_key == Some(key) {
            return;
        }
        self.map_key = Some(key);
        self.map = FoldMap::default();
        if !active {
            return;
        }
        for fold in &self.collapsed {
            let start_line = text.line_of(fold.start) + 1;
            let end_line = text.line_of(fold.end.saturating_sub(1)) + 1;
            let row = |line| {
                if line >= text.line_count() {
                    wrap.map_or(text.line_count(), |w| w.rows())
                } else {
                    wrap.map_or(line, |w| w.visual_row(text.line_start(line)))
                }
            };
            let rows = row(start_line)..row(end_line);
            if rows.is_empty() || self.map.gaps.last().is_some_and(|g| g.rows.end >= rows.end) {
                continue;
            }
            if let Some(last) = self.map.gaps.last_mut()
                && rows.start <= last.rows.end
            {
                self.map.removed += rows.end - last.rows.end;
                last.rows.end = rows.end;
            } else {
                self.map.gaps.push(Gap {
                    before: self.map.removed,
                    trim: 0.0,
                    trimmed_before: 0.0,
                    rows: rows.clone(),
                });
                self.map.removed += rows.len();
            }
        }
    }
    /// Target folds stay immediate; only this sparse drawing projection is animated.
    /// Bodies crop at normal font size, with fractional last rows shared by hit testing.
    pub fn animate(
        &mut self,
        text: &TextBuffer,
        wrap: Option<&crate::editor::wrapping::WrapIndex>,
        active: bool,
        now: f64,
        enabled: bool,
        ctx: &bedtime_ui::egui::Context,
    ) {
        self.motions.retain(|_, m| m.value(now) != m.to);
        if !enabled || !active {
            self.motions.clear();
            self.visual_targets = self.collapsed.clone();
        } else if self.visual_targets != self.collapsed {
            let mut ranges = self.visual_targets.clone();
            ranges.extend(self.collapsed.iter().cloned());
            ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
            ranges.dedup();
            for range in ranges {
                let key = (range.start, std::cmp::Reverse(range.end));
                let was = self
                    .visual_targets
                    .binary_search_by_key(&key, |r| (r.start, std::cmp::Reverse(r.end)))
                    .is_ok();
                let closed = self
                    .collapsed
                    .binary_search_by_key(&key, |r| (r.start, std::cmp::Reverse(r.end)))
                    .is_ok();
                if was == closed {
                    continue;
                }
                let from = self
                    .motions
                    .get(&(range.start, range.end))
                    .map_or(if was { 0.0 } else { 1.0 }, |m| m.value(now));
                self.motions.remove(&(range.start, range.end));
                let to = if closed { 0.0 } else { 1.0 };
                if from != to {
                    self.motions.insert(
                        (range.start, range.end),
                        FoldMotion {
                            range,
                            from,
                            to,
                            started: now,
                        },
                    );
                }
            }
            self.visual_targets = self.collapsed.clone();
        }
        if !self.motions.is_empty() {
            ctx.request_repaint();
        }
        let mut visuals: Vec<_> = self
            .collapsed
            .iter()
            .cloned()
            .map(|range| {
                let value = self
                    .motions
                    .get(&(range.start, range.end))
                    .map_or(0.0, |m| m.value(now));
                (range, value)
            })
            .collect();
        visuals.extend(
            self.motions
                .values()
                .filter(|m| m.to == 1.0)
                .map(|m| (m.range.clone(), m.value(now))),
        );
        visuals.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
        self.display_map = FoldMap::default();
        if !active {
            return;
        }
        let row = |line| {
            if line >= text.line_count() {
                wrap.map_or(text.line_count(), |w| w.rows())
            } else {
                wrap.map_or(line, |w| w.visual_row(text.line_start(line)))
            }
        };
        // Union cropped tails, including nested folds. An opening parent must
        // preserve a child's independent collapse rather than reveal it briefly.
        let mut hidden = Vec::new();
        for (range, openness) in visuals {
            let start = row(text.line_of(range.start) + 1);
            let end = row(text.line_of(range.end.saturating_sub(1)) + 1);
            if start >= end || openness >= 1.0 {
                continue;
            }
            hidden.push((start as f32 + (end - start) as f32 * openness, end));
        }
        hidden.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, usize)> = Vec::new();
        for (start, end) in hidden {
            if let Some(last) = merged.last_mut()
                && start <= last.1 as f32
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        for (start, end) in merged {
            let first_hidden = start.ceil() as usize;
            let trim = first_hidden as f32 - start;
            let map = &mut self.display_map;
            map.gaps.push(Gap {
                rows: first_hidden..end,
                before: map.removed,
                trim,
                trimmed_before: map.trimmed,
            });
            map.removed += end - first_hidden;
            map.trimmed += trim;
        }
    }

    pub fn openness(&self, text: &TextBuffer, line: usize, now: f64) -> f32 {
        self.motions
            .values()
            .find(|m| text.line_of(m.range.start) == line)
            .map_or(
                if self.collapsed_at(text, line) {
                    0.0
                } else {
                    1.0
                },
                |m| m.value(now),
            )
    }
    pub fn reveal(&mut self, text: &TextBuffer, target: &Range<usize>) {
        let before = self.collapsed.len();
        self.collapsed.retain(|r| {
            let last = text.line_of(r.end.saturating_sub(1));
            let hidden = text.line_start(text.line_of(r.start) + 1)..text.line_start(last + 1);
            // The end of its last line is shown after its " …": a caret
            // there doesn't open it.
            let shown = text.line_range(last).end;
            let opens = |at: usize| hidden.contains(&at) && !(target.is_empty() && at == shown);
            !(opens(target.start)
                || opens(target.end.saturating_sub(usize::from(!target.is_empty()))))
        });
        if self.collapsed.len() != before {
            self.map_key = None;
        }
    }
    /// Where a selection ending at `byte` really ends: a collapsed fold's
    /// first line ends visually with its " …", so reaching that end selects
    /// what the fold hides too, to the end of its last line (where the caret
    /// is drawn after the " …").
    pub fn through_collapsed(&self, text: &TextBuffer, byte: usize) -> usize {
        let line = text.line_of(byte);
        if self.collapsed.is_empty() || byte != text.line_range(line).end {
            return byte;
        }
        self.collapsed
            .iter()
            .filter(|r| text.line_of(r.start) == line)
            .map(|r| text.line_range(text.line_of(r.end.saturating_sub(1))).end)
            .max()
            .map_or(byte, |end| end.max(byte))
    }
    /// Each collapsed fold from the end of its first line to the end of its
    /// last: the two places beside it a caret can be.
    pub(super) fn spans<'a>(
        &'a self,
        text: &'a TextBuffer,
    ) -> impl Iterator<Item = Range<usize>> + 'a {
        self.collapsed.iter().map(|r| {
            text.line_range(text.line_of(r.start)).end
                ..text.line_range(text.line_of(r.end.saturating_sub(1))).end
        })
    }
    /// The outermost collapsed fold hiding `byte` (strictly inside its span).
    pub fn hidden_span(&self, text: &TextBuffer, byte: usize) -> Option<Range<usize>> {
        self.spans(text)
            .filter(|span| span.start < byte && byte < span.end)
            .max_by_key(|span| span.len())
    }
    /// The line a caret at `byte` is drawn on: a fold's first line for what
    /// it hides, the end of its last line included.
    pub fn visible_line(&self, text: &TextBuffer, byte: usize) -> usize {
        self.spans(text)
            .filter(|span| span.start < byte && byte <= span.end)
            .max_by_key(|span| span.len())
            .map_or_else(|| text.line_of(byte), |span| text.line_of(span.start))
    }
    pub fn collapsed_at(&self, text: &TextBuffer, line: usize) -> bool {
        if self.collapsed.is_empty() {
            return false;
        }
        let end = text.line_start(line + 1);
        let index = self.collapsed.partition_point(|r| r.start < end);
        index > 0 && text.line_of(self.collapsed[index - 1].start) == line
    }
}
impl EditorView {
    pub fn unfold_all(&mut self) {
        self.folding.collapsed.clear();
        self.folding.map_key = None;
    }
    pub fn collapsed_regions(&self) -> &[Range<usize>] {
        &self.folding.collapsed
    }
    /// A selection made with the pointer that reaches the end of a folded
    /// line takes the folded lines with it, as copying or deleting expects.
    pub(crate) fn select_through_folds(
        &self,
        text: &TextBuffer,
        selection: crate::editor::TextSelection,
    ) -> crate::editor::TextSelection {
        let range = selection.range();
        if range.is_empty() || self.diff_active() {
            return selection;
        }
        let end = self.folding.through_collapsed(text, range.end);
        let mut selection = selection;
        if selection.head >= selection.anchor {
            selection.head = end;
        } else {
            selection.anchor = end;
        }
        selection
    }
    pub fn toggle_fold(&mut self, snapshot: &TextSnapshot, line: usize) -> Result<(), EditorError> {
        if self.is_composing() {
            return Err(EditorError("Finish composition before folding".into()));
        }
        self.folding.map_key = None;
        if let Some(index) = self
            .folding
            .collapsed
            .iter()
            .position(|r| snapshot.text.line_of(r.start) == line)
        {
            self.folding.collapsed.remove(index);
            return Ok(());
        }
        let region = self
            .syntax
            .as_ref()
            .filter(|o| o.key.revision == snapshot.revision)
            .and_then(|o| {
                o.folds
                    .binary_search_by_key(&line, |f| f.start_line)
                    .ok()
                    .map(|index| &o.folds[index])
            })
            .ok_or_else(|| EditorError("No current fold at this line".into()))?;
        self.folding.collapsed.push(region.range.clone());
        self.folding
            .collapsed
            .sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        if snapshot.text.line_of(self.selection.head) > line
            && self.selection.head < snapshot.text.line_start(region.end_line + 1)
        {
            self.selection = crate::editor::TextSelection::caret(
                &snapshot.text,
                snapshot.text.line_range(line).end,
            )?;
        }
        Ok(())
    }
    pub fn fold_all(&mut self, snapshot: &TextSnapshot) -> Result<(), EditorError> {
        if self.is_composing() {
            return Err(EditorError("Finish composition before folding".into()));
        }
        let observation = self
            .syntax
            .as_ref()
            .filter(|o| o.key.revision == snapshot.revision)
            .ok_or_else(|| EditorError("No current syntax observation".into()))?;
        let head = self.selection.head;
        let containing = observation.folds.iter().find(|f| {
            snapshot.text.line_of(head) > f.start_line
                && head < snapshot.text.line_start(f.end_line + 1)
        });
        if let Some(region) = containing {
            self.selection = crate::editor::TextSelection::caret(
                &snapshot.text,
                snapshot.text.line_range(region.start_line).end,
            )?;
        }
        self.folding.collapsed = observation.folds.iter().map(|f| f.range.clone()).collect();
        self.folding.map_key = None;
        Ok(())
    }
}

/// Folding at every caret (Fold, Unfold, Toggle Fold).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FoldAction {
    Fold,
    Unfold,
    Toggle,
}

impl EditorView {
    /// Fold or unfold the region around each caret's line. Folding needs
    /// an analysis of this revision; carets it hides go to its first line.
    pub(super) fn fold_at_carets(&mut self, snapshot: &TextSnapshot, action: FoldAction) {
        if self.is_composing() {
            return;
        }
        let text = &snapshot.text;
        let regions: Vec<(Range<usize>, usize, usize)> = self
            .syntax
            .as_ref()
            .filter(|o| o.key.revision == snapshot.revision)
            .map(|o| {
                o.folds
                    .iter()
                    .map(|f| (f.range.clone(), f.start_line, f.end_line))
                    .collect()
            })
            .unwrap_or_default();
        let mut lines: Vec<usize> = self
            .carets()
            .iter()
            .map(|caret| text.line_of(caret.selection.head))
            .collect();
        lines.dedup();
        for line in lines {
            let header = self
                .folding
                .collapsed
                .iter()
                .position(|r| text.line_of(r.start) == line);
            let unfold = match action {
                FoldAction::Unfold => true,
                FoldAction::Toggle => header.is_some(),
                FoldAction::Fold => false,
            };
            if unfold {
                // The one this line heads, else the innermost around it.
                let index = header.or_else(|| {
                    (0..self.folding.collapsed.len())
                        .filter(|&i| {
                            let r = &self.folding.collapsed[i];
                            text.line_of(r.start) <= line
                                && line <= text.line_of(r.end.saturating_sub(1))
                        })
                        .max_by_key(|&i| self.folding.collapsed[i].start)
                });
                if let Some(index) = index {
                    self.folding.collapsed.remove(index);
                }
                continue;
            }
            let innermost = regions
                .iter()
                .filter(|(range, start, end)| {
                    *start <= line && line <= *end && !self.folding.collapsed.contains(range)
                })
                .max_by_key(|(_, start, _)| *start);
            if let Some((range, _, _)) = innermost {
                self.folding.collapsed.push(range.clone());
            }
        }
        self.folding
            .collapsed
            .sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        self.folding.map_key = None;
        self.reveal_carets(snapshot);
    }

    /// Fold every region; carets inside go to their region's first line.
    pub(super) fn fold_everything(&mut self, snapshot: &TextSnapshot) {
        if self.fold_all(snapshot).is_ok() {
            self.reveal_carets(snapshot);
        }
    }

    /// Carets a fold hides go beside it, at the end of its first line.
    fn reveal_carets(&mut self, snapshot: &TextSnapshot) {
        let text = &snapshot.text;
        let mut carets = self.carets();
        for caret in &mut carets {
            for end in [caret.selection.anchor, caret.selection.head] {
                if let Some(span) = self.folding.hidden_span(text, end) {
                    caret.selection = crate::editor::TextSelection {
                        anchor: span.start,
                        head: span.start,
                    };
                    caret.column = None;
                }
            }
        }
        self.set_carets(carets, snapshot.revision);
    }
}
