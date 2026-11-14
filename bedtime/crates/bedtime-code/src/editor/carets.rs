//! Several carets. The view's `selection` stays the primary one: completion,
//! search, language tools and text composition act there alone. The others
//! move and edit with it, and each edit at all of them is one document
//! change (one revision, one undo step).
//!
//! The others hold the revision they're positioned at. Before they're used,
//! they follow the document's journal to the current revision, so edits made
//! only at the primary (a completion, a replacement, another view, undo)
//! carry them along.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Other {
    pub selection: TextSelection,
    /// Where vertical movement aims, as for the primary.
    pub column: Option<usize>,
}

#[derive(Default)]
pub(super) struct Others {
    pub list: Vec<Other>,
    pub revision: u64,
}

/// One caret with what moving it needs.
#[derive(Clone, Copy, Debug)]
pub(super) struct Caret {
    pub selection: TextSelection,
    pub column: Option<usize>,
    pub upstream: bool,
    pub primary: bool,
}

/// A change of the text, in the coordinates before any of a command's
/// changes, and where the caret that asked for it goes (in the coordinates
/// after only this change).
pub(super) struct Planned {
    pub edit: BufferEdit,
    pub owners: Vec<(usize, TextSelection)>,
}

/// Selecting every occurrence stops here.
const MAX_CARETS: usize = 10_000;

impl EditorView {
    /// Every selection, the primary included, in text order.
    pub fn selections(&self) -> Vec<TextSelection> {
        self.carets()
            .into_iter()
            .map(|caret| caret.selection)
            .collect()
    }

    pub(super) fn has_others(&self) -> bool {
        !self.others.list.is_empty()
    }

    pub(super) fn clear_others(&mut self) {
        self.others.list.clear();
    }

    /// Every caret, in text order.
    pub(super) fn carets(&self) -> Vec<Caret> {
        let mut carets: Vec<_> = self
            .others
            .list
            .iter()
            .map(|other| Caret {
                selection: other.selection,
                column: other.column,
                upstream: false,
                primary: false,
            })
            .collect();
        carets.push(Caret {
            selection: self.selection,
            column: self.preferred_column,
            upstream: self.upstream,
            primary: true,
        });
        carets.sort_by_key(|caret| (caret.selection.range().start, caret.selection.range().end));
        carets
    }

    /// Replace every caret, merging those that overlap, at `revision`.
    pub(super) fn set_carets(&mut self, mut carets: Vec<Caret>, revision: u64) {
        carets.sort_by_key(|caret| (caret.selection.range().start, caret.selection.range().end));
        let mut merged: Vec<Caret> = Vec::with_capacity(carets.len());
        for caret in carets {
            if let Some(last) = merged.last_mut() {
                let (a, b) = (last.selection.range(), caret.selection.range());
                if b.start < a.end || b.start == a.start {
                    let range = a.start..a.end.max(b.end);
                    let primary = last.primary || caret.primary;
                    // Keep the primary's direction (else the first's).
                    let forward = if caret.primary {
                        caret.selection.head >= caret.selection.anchor
                    } else {
                        last.selection.head >= last.selection.anchor
                    };
                    let keep = if caret.primary { caret } else { *last };
                    *last = Caret {
                        selection: if forward {
                            TextSelection {
                                anchor: range.start,
                                head: range.end,
                            }
                        } else {
                            TextSelection {
                                anchor: range.end,
                                head: range.start,
                            }
                        },
                        primary,
                        ..keep
                    };
                    continue;
                }
            }
            merged.push(caret);
        }
        let primary = merged.iter().position(|caret| caret.primary).unwrap_or(0);
        let caret = merged.remove(primary);
        self.selection = caret.selection;
        self.preferred_column = caret.column;
        self.upstream = caret.upstream;
        self.others = Others {
            list: merged
                .into_iter()
                .map(|caret| Other {
                    selection: caret.selection,
                    column: caret.column,
                })
                .collect(),
            revision,
        };
    }

    /// Bring the others to `snapshot`'s revision.
    pub(super) fn sync_others(
        &mut self,
        session: &dyn TextSession,
        snapshot: &TextSnapshot,
    ) -> Result<(), EditorError> {
        if self.others.revision == snapshot.revision {
            return Ok(());
        }
        if self.has_others() {
            let selections: Vec<_> = self.others.list.iter().map(|o| o.selection).collect();
            let mapped = map_selections(session, self.others.revision, snapshot, &selections)?;
            for (other, selection) in self.others.list.iter_mut().zip(mapped) {
                other.selection = selection;
            }
            let carets = self.carets();
            self.set_carets(carets, snapshot.revision);
        }
        self.others.revision = snapshot.revision;
        Ok(())
    }

    /// Change the text at every caret: `plan` gives a caret's change, and
    /// where it goes, from the text before any of them.
    pub(super) fn edit_carets(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        now: f64,
        mut plan: impl FnMut(&TextBuffer, TextSelection) -> Option<(BufferEdit, TextSelection)>,
    ) -> Result<(), EditorError> {
        let carets = self.carets();
        let planned = carets
            .iter()
            .enumerate()
            .filter_map(|(index, caret)| {
                plan(&snapshot.text, caret.selection).map(|(edit, placed)| Planned {
                    edit,
                    owners: vec![(index, placed)],
                })
            })
            .collect();
        self.commit_carets(id, session, snapshot, now, carets, planned)
    }

    /// Apply `planned` as one change and put each caret where its change
    /// says, or where the changes move it.
    pub(super) fn commit_carets(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        now: f64,
        mut carets: Vec<Caret>,
        mut planned: Vec<Planned>,
    ) -> Result<(), EditorError> {
        planned.sort_by_key(|p| (p.edit.range().start, p.edit.range().end));
        // Overlapping deletions (Backspace by words at neighbouring carets)
        // join; any other overlap keeps the first change.
        let mut changes: Vec<Planned> = Vec::with_capacity(planned.len());
        for p in planned {
            if let Some(last) = changes.last_mut()
                && p.edit.range().start < last.edit.range().end
            {
                if p.edit.replacement().is_empty() && last.edit.replacement().is_empty() {
                    let start = last.edit.range().start;
                    let end = last.edit.range().end.max(p.edit.range().end);
                    last.edit = BufferEdit::new(start..end, "");
                    let at = TextSelection {
                        anchor: start,
                        head: start,
                    };
                    last.owners
                        .extend(p.owners.iter().map(|&(index, _)| (index, at)));
                    for (_, placed) in &mut last.owners {
                        *placed = at;
                    }
                }
                continue;
            }
            changes.push(p);
        }
        let deltas: Vec<(Range<usize>, usize)> = changes
            .iter()
            .map(|p| (p.edit.range(), p.edit.replacement().len()))
            .collect();
        let mut placed = vec![None; carets.len()];
        let mut shift: isize = 0;
        for (p, (range, inserted)) in changes.iter().zip(&deltas) {
            for &(index, at) in &p.owners {
                let offset = |byte: usize| (byte as isize + shift) as usize;
                placed[index] = Some(TextSelection {
                    anchor: offset(at.anchor),
                    head: offset(at.head),
                });
            }
            shift += *inserted as isize - range.len() as isize;
        }
        for (caret, placed) in carets.iter_mut().zip(placed) {
            caret.selection = placed.unwrap_or(TextSelection {
                anchor: map_through(&deltas, caret.selection.anchor),
                head: map_through(&deltas, caret.selection.head),
            });
            caret.column = None;
            caret.upstream = false;
        }
        if !changes.is_empty() {
            session.set_edit_owner(id);
            // Last first: each range is still where it was.
            session.edit_buffers(
                snapshot.revision,
                changes.into_iter().rev().map(|p| p.edit).collect(),
            )?;
            *snapshot = session.snapshot()?;
            self.last_edit = Some(now);
        }
        for caret in &mut carets {
            caret.selection =
                TextSelection::new(&snapshot.text, caret.selection.anchor, caret.selection.head)?;
        }
        self.set_carets(carets, snapshot.revision);
        Ok(())
    }

    /// Text, deletion, Enter, Tab and the clipboard at every caret. `None`
    /// for what one caret handles the same as many.
    pub(super) fn multi_caret_event(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        event: &egui::Event,
        now: f64,
    ) -> Result<Option<bool>, EditorError> {
        let read_only = self.settings.read_only;
        match event {
            egui::Event::Text(text) | egui::Event::Paste(text)
                if !matches!(text.as_str(), "\n" | "\r" | "\t") =>
            {
                if read_only {
                    return Ok(Some(true));
                }
                let paste = matches!(event, egui::Event::Paste(_));
                // As many lines as carets: one each.
                let count = self.others.list.len() + 1;
                let lines: Vec<&str> = text.lines().collect();
                let spread = paste && lines.len() == count;
                let mut index = 0;
                if paste {
                    session.end_undo_group();
                }
                self.edit_carets(id, session, snapshot, now, |_, selection| {
                    let insert = if spread { lines[index] } else { text.as_str() };
                    index += 1;
                    let start = selection.range().start;
                    let head = start + insert.len();
                    Some((
                        BufferEdit::new(selection.range(), insert),
                        TextSelection { anchor: head, head },
                    ))
                })?;
                if paste {
                    session.end_undo_group();
                }
                Ok(Some(true))
            }
            egui::Event::Copy | egui::Event::Cut => {
                let selected: Vec<String> = self
                    .carets()
                    .iter()
                    .map(|caret| caret.selection.range())
                    .filter(|range| !range.is_empty())
                    .map(|range| snapshot.text.slice(range).to_string())
                    .collect();
                if selected.is_empty() {
                    return Ok(Some(true));
                }
                ui.ctx().copy_text(selected.join("\n"));
                if matches!(event, egui::Event::Cut) && !read_only {
                    session.end_undo_group();
                    self.edit_carets(id, session, snapshot, now, |_, selection| {
                        let start = selection.range().start;
                        (!selection.range().is_empty()).then(|| {
                            (
                                BufferEdit::new(selection.range(), ""),
                                TextSelection {
                                    anchor: start,
                                    head: start,
                                },
                            )
                        })
                    })?;
                    session.end_undo_group();
                }
                Ok(Some(true))
            }
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => {
                let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
                let primary = modifiers.command;
                let word = modifiers.alt || (!mac && modifiers.ctrl);
                let style = self.settings.indentation;
                let smart = self.settings.smart_indent;
                match key {
                    egui::Key::Escape => {
                        self.clear_others();
                        session.end_undo_group();
                        Ok(Some(true))
                    }
                    egui::Key::Backspace | egui::Key::Delete => {
                        if read_only {
                            return Ok(Some(true));
                        }
                        let backspace = *key == egui::Key::Backspace;
                        let folding = &self.folding;
                        let mut carets = self.carets();
                        // Resolve each deletion's range first; folds only read.
                        for caret in &mut carets {
                            let selection = caret.selection;
                            if !selection.range().is_empty() {
                                continue;
                            }
                            let text = &snapshot.text;
                            let head = selection.head;
                            let body = text.line_range(folding.visible_line(text, head));
                            let target = if backspace {
                                if mac && primary {
                                    body.start
                                } else if word {
                                    word_boundary(text, head, false)
                                } else if smart {
                                    whitespace::backspace(text, head, style)
                                } else {
                                    text.previous_grapheme(head)
                                }
                            } else if mac && primary {
                                body.end
                            } else if word {
                                word_boundary(text, head, true)
                            } else {
                                text.next_grapheme(head)
                            };
                            caret.selection.anchor = target;
                        }
                        let planned = carets
                            .iter()
                            .enumerate()
                            .filter(|(_, caret)| !caret.selection.range().is_empty())
                            .map(|(index, caret)| {
                                let start = caret.selection.range().start;
                                Planned {
                                    edit: BufferEdit::new(caret.selection.range(), ""),
                                    owners: vec![(
                                        index,
                                        TextSelection {
                                            anchor: start,
                                            head: start,
                                        },
                                    )],
                                }
                            })
                            .collect();
                        self.commit_carets(id, session, snapshot, now, carets, planned)?;
                        Ok(Some(true))
                    }
                    egui::Key::Enter if !primary && !modifiers.alt => {
                        if read_only {
                            return Ok(Some(true));
                        }
                        session.end_undo_group();
                        self.edit_carets(id, session, snapshot, now, |text, selection| {
                            let (edit, head) = whitespace::newline(text, selection, style, smart);
                            Some((edit, TextSelection { anchor: head, head }))
                        })?;
                        session.end_undo_group();
                        Ok(Some(true))
                    }
                    egui::Key::Tab if !primary && !modifiers.alt => {
                        if read_only {
                            return Ok(Some(true));
                        }
                        let carets = self.carets();
                        session.end_undo_group();
                        if modifiers.shift || carets.iter().any(|c| !c.selection.range().is_empty())
                        {
                            let planned =
                                indent_lines(&snapshot.text, &carets, style, modifiers.shift);
                            self.commit_carets(id, session, snapshot, now, carets, planned)?;
                        } else {
                            self.edit_carets(id, session, snapshot, now, |text, selection| {
                                let head = selection.head;
                                let insert = if style == Indentation::Tabs {
                                    "\t".to_owned()
                                } else {
                                    " ".repeat(style.width() - text.column(head) % style.width())
                                };
                                let end = head + insert.len();
                                Some((
                                    BufferEdit::new(head..head, insert),
                                    TextSelection {
                                        anchor: end,
                                        head: end,
                                    },
                                ))
                            })?;
                        }
                        session.end_undo_group();
                        Ok(Some(true))
                    }
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// Where `key` moves `caret` (extending it with Shift), if it's a key
    /// that moves carets.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn move_caret(
        &self,
        ui: &egui::Ui,
        text: &TextBuffer,
        revision: u64,
        caret: Caret,
        key: egui::Key,
        modifiers: egui::Modifiers,
        page: usize,
    ) -> Option<Caret> {
        let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
        let primary = modifiers.command;
        let word = modifiers.alt || (!mac && modifiers.ctrl);
        let head = caret.selection.head;
        // The line the caret is drawn on (a fold's first, beside it).
        let row = self.folding.visible_line(text, head);
        let body = text.line_range(row);
        let selection = caret.selection.range();
        let mut upstream = matches!(key, egui::Key::ArrowLeft | egui::Key::End);
        let mut column = caret.column;
        let wrapped_segment = self
            .wrapping
            .index
            .as_ref()
            .filter(|index| self.settings.wrap && index.snapshot.revision == revision)
            .map(|index| {
                let mut visual = index.visual_row(head);
                if caret.upstream && visual > 0 && index.segment(visual).bytes.start == head {
                    visual -= 1;
                }
                index.segment(visual)
            });
        let vertical = matches!(
            key,
            egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::PageUp | egui::Key::PageDown
        );
        let target = match key {
            egui::Key::ArrowLeft => {
                if !modifiers.shift && !selection.is_empty() {
                    selection.start
                } else if mac && primary {
                    body.start
                } else if word {
                    word_boundary(text, head, false)
                } else {
                    text.previous_grapheme(head)
                }
            }
            egui::Key::ArrowRight => {
                if !modifiers.shift && !selection.is_empty() {
                    selection.end
                } else if mac && primary {
                    body.end
                } else if word {
                    word_boundary(text, head, true)
                } else {
                    text.next_grapheme(head)
                }
            }
            egui::Key::Home => {
                if primary {
                    0
                } else {
                    let first = whitespace::leading_end(text, row, body.end);
                    match &wrapped_segment {
                        Some(segment) if segment.part > 0 => segment.bytes.start,
                        _ if head == first => body.start,
                        _ => first,
                    }
                }
            }
            egui::Key::End => {
                if primary {
                    text.len()
                } else {
                    wrapped_segment
                        .as_ref()
                        .map_or(body.end, |segment| segment.bytes.end)
                }
            }
            _ if vertical && mac && primary && key == egui::Key::ArrowUp => 0,
            _ if vertical && mac && primary && key == egui::Key::ArrowDown => text.len(),
            _ if vertical => {
                let wrap = self
                    .wrapping
                    .index
                    .as_ref()
                    .filter(|index| self.settings.wrap && index.snapshot.revision == revision);
                let geometry = layout::Geometry {
                    text,
                    metrics: &layout::Metrics::new(ui),
                    wrap,
                    folds: &self.folding.map,
                    diff: self.diff.current(revision, wrap),
                };
                let aim = *column.get_or_insert_with(|| geometry.column_for(head, caret.upstream));
                let step = if matches!(key, egui::Key::PageUp | egui::Key::PageDown) {
                    page
                } else {
                    1
                } as isize;
                let step = if matches!(key, egui::Key::ArrowUp | egui::Key::PageUp) {
                    -step
                } else {
                    step
                };
                let (target, after) = geometry.vertical(head, aim, step, caret.upstream);
                upstream = after;
                target
            }
            _ => return None,
        };
        // A collapsed fold is one step: from the end of its first line to
        // the end of its last (drawn after its " …"), never inside.
        let target = match (key, self.folding.hidden_span(text, target)) {
            (egui::Key::ArrowLeft | egui::Key::Home, Some(span)) => span.start,
            (_, Some(span)) => span.end,
            _ => target,
        };
        if !vertical {
            column = None;
        }
        Some(Caret {
            selection: TextSelection {
                anchor: if modifiers.shift {
                    caret.selection.anchor
                } else {
                    target
                },
                head: target,
            },
            column,
            upstream,
            primary: caret.primary,
        })
    }

    /// Move every caret by `key`; `false` if it doesn't move carets.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn move_carets(
        &mut self,
        ui: &egui::Ui,
        snapshot: &TextSnapshot,
        key: egui::Key,
        modifiers: egui::Modifiers,
        page: usize,
    ) -> bool {
        let carets = self.carets();
        let mut moved = Vec::with_capacity(carets.len());
        for caret in carets {
            match self.move_caret(
                ui,
                &snapshot.text,
                snapshot.revision,
                caret,
                key,
                modifiers,
                page,
            ) {
                Some(caret) => moved.push(caret),
                None => return false,
            }
        }
        self.set_carets(moved, snapshot.revision);
        true
    }

    /// A caret on the line above or below the primary, which it becomes.
    pub(super) fn add_caret_vertically(
        &mut self,
        ui: &egui::Ui,
        snapshot: &TextSnapshot,
        down: bool,
    ) {
        let primary = Caret {
            selection: TextSelection {
                anchor: self.selection.head,
                head: self.selection.head,
            },
            column: self.preferred_column,
            upstream: self.upstream,
            primary: true,
        };
        let key = if down {
            egui::Key::ArrowDown
        } else {
            egui::Key::ArrowUp
        };
        let Some(added) = self.move_caret(
            ui,
            &snapshot.text,
            snapshot.revision,
            primary,
            key,
            egui::Modifiers::NONE,
            1,
        ) else {
            return;
        };
        if added.selection.head == primary.selection.head {
            return;
        }
        let mut carets = self.carets();
        for caret in &mut carets {
            caret.primary = false;
        }
        carets.push(added);
        self.set_carets(carets, snapshot.revision);
    }

    /// A box from `anchor` to `to` (lines and columns): on each line, the
    /// text between the two columns (up to its end); lines a fold hides
    /// have none. The line at `to` has the primary.
    pub(super) fn select_box(
        &mut self,
        snapshot: &TextSnapshot,
        anchor: (usize, usize),
        to: (usize, usize),
    ) {
        let text = &snapshot.text;
        let rows = anchor.0.min(to.0)..=anchor.0.max(to.0);
        let carets = rows
            .filter(|&row| {
                row == to.0
                    || self
                        .folding
                        .hidden_span(text, text.line_start(row))
                        .is_none()
            })
            .map(|row| Caret {
                selection: TextSelection {
                    anchor: text.byte_at_column(row, anchor.1),
                    head: text.byte_at_column(row, to.1),
                },
                column: Some(to.1),
                upstream: false,
                primary: row == to.0,
            })
            .collect();
        self.set_carets(carets, snapshot.revision);
    }

    /// Alt-click: a caret at `byte`, or none if one is there already.
    pub(super) fn toggle_caret(&mut self, revision: u64, byte: usize) {
        let mut carets = self.carets();
        if let Some(index) = carets
            .iter()
            .position(|caret| caret.selection.range() == (byte..byte))
            && carets.len() > 1
        {
            let removed = carets.remove(index);
            if removed.primary {
                carets.last_mut().unwrap().primary = true;
            }
        } else {
            for caret in &mut carets {
                caret.primary = false;
            }
            carets.push(Caret {
                selection: TextSelection {
                    anchor: byte,
                    head: byte,
                },
                column: None,
                upstream: false,
                primary: true,
            });
        }
        self.set_carets(carets, revision);
    }

    /// What Cmd/Ctrl+D and Select All Occurrences look for: the primary's
    /// text (whole words if it's a word), or the word at its caret, which
    /// they select first.
    fn occurrence_query(&mut self, text: &TextBuffer) -> Option<(search::SearchQuery, bool)> {
        let range = self.selection.range();
        let (range, selected_now) = if range.is_empty() {
            // The word at the caret, or just before it.
            let at = if class(text, range.start) != 1 && range.start > 0 {
                text.previous_grapheme(range.start)
            } else {
                range.start
            };
            let word = word_at(text, at);
            if word.is_empty() || class(text, word.start) != 1 {
                return None;
            }
            self.selection = TextSelection {
                anchor: word.start,
                head: word.end,
            };
            (word, true)
        } else {
            (range, false)
        };
        let whole_word = class(text, range.start) == 1 && word_at(text, range.start) == range;
        Some((
            search::SearchQuery {
                text: text.slice(range).to_string(),
                match_case: true,
                whole_word,
            },
            selected_now,
        ))
    }

    /// Cmd/Ctrl+D: select the next occurrence of the primary's text too
    /// (first, the word at a caret).
    pub(super) fn add_next_occurrence(&mut self, snapshot: &TextSnapshot) {
        let text = &snapshot.text;
        let Some((query, selected_now)) = self.occurrence_query(text) else {
            return;
        };
        if selected_now {
            return;
        }
        let selected: Vec<_> = self.carets().iter().map(|c| c.selection.range()).collect();
        let after = self.selection.range().end;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut found = None;
        for range in [after..text.len(), 0..after] {
            let _ = search::scan_range(text, &query, range, &cancel, |hit| {
                if selected.contains(&hit) {
                    return Ok(());
                }
                found = Some(hit);
                Err(EditorError(String::new()))
            });
            if found.is_some() {
                break;
            }
        }
        let Some(hit) = found else { return };
        let mut carets = self.carets();
        for caret in &mut carets {
            caret.primary = false;
        }
        carets.push(Caret {
            selection: TextSelection {
                anchor: hit.start,
                head: hit.end,
            },
            column: None,
            upstream: false,
            primary: true,
        });
        self.set_carets(carets, snapshot.revision);
    }

    /// Cmd/Ctrl+Shift+L: select every occurrence of the primary's text.
    pub(super) fn select_all_occurrences(&mut self, snapshot: &TextSnapshot) {
        let text = &snapshot.text;
        let Some((query, _)) = self.occurrence_query(text) else {
            return;
        };
        let primary = self.selection.range();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut carets = Vec::new();
        let _ = search::scan(text, &query, &cancel, |hit| {
            carets.push(Caret {
                primary: hit == primary,
                selection: TextSelection {
                    anchor: hit.start,
                    head: hit.end,
                },
                column: None,
                upstream: false,
            });
            if carets.len() >= MAX_CARETS {
                return Err(EditorError(String::new()));
            }
            Ok(())
        });
        if carets.is_empty() {
            return;
        }
        if !carets.iter().any(|caret| caret.primary) {
            carets.push(Caret {
                selection: self.selection,
                column: None,
                upstream: false,
                primary: true,
            });
        }
        self.set_carets(carets, snapshot.revision);
    }

    /// A caret at the end of each line of each selection.
    pub(super) fn carets_at_line_ends(&mut self, snapshot: &TextSnapshot) {
        let text = &snapshot.text;
        let mut carets = Vec::new();
        for caret in self.carets() {
            let range = caret.selection.range();
            if range.is_empty() {
                carets.push(Caret {
                    primary: false,
                    ..caret
                });
                continue;
            }
            let (first, last) = (text.line_of(range.start), text.line_of(range.end));
            for row in first..=last {
                let end = if row == last {
                    range.end
                } else {
                    text.line_range(row).end
                };
                if row == last && row > first && end == text.line_start(row) {
                    continue;
                }
                carets.push(Caret {
                    selection: TextSelection {
                        anchor: end,
                        head: end,
                    },
                    column: None,
                    upstream: false,
                    primary: false,
                });
            }
        }
        // The last becomes the primary.
        if let Some(last) = (0..carets.len()).max_by_key(|&i| carets[i].selection.head) {
            carets[last].primary = true;
        }
        self.set_carets(carets, snapshot.revision);
    }
}

/// Where `byte` goes when `deltas` (in order, from the text before any of
/// them) are applied, as `TextDelta::map_position` maps one.
fn map_through(deltas: &[(Range<usize>, usize)], byte: usize) -> usize {
    let mut shift: isize = 0;
    for (range, inserted) in deltas {
        if byte < range.start {
            break;
        }
        if byte <= range.end {
            return (range.start as isize + shift) as usize + inserted;
        }
        shift += *inserted as isize - range.len() as isize;
    }
    (byte as isize + shift) as usize
}

/// Tab and Shift+Tab with several carets: each line of any selection, once.
pub(super) fn indent_lines(
    text: &TextBuffer,
    carets: &[Caret],
    style: Indentation,
    outdent: bool,
) -> Vec<Planned> {
    let mut rows = Vec::new();
    for caret in carets {
        let range = caret.selection.range();
        let first = text.line_of(range.start);
        let last = text.line_of(range.end);
        let last = if range.end > range.start && range.end == text.line_start(last) {
            last.saturating_sub(1)
        } else {
            last
        };
        rows.extend(first..=last);
    }
    rows.sort_unstable();
    rows.dedup();
    rows.into_iter()
        .filter_map(|row| {
            let at = text.line_start(row);
            if outdent {
                let remove = if text.byte(at) == Some(b'\t') {
                    1
                } else {
                    (whitespace::leading_end(text, row, at.saturating_add(style.width())) - at)
                        .min(style.width())
                };
                (remove > 0).then(|| Planned {
                    edit: BufferEdit::new(at..at + remove, ""),
                    owners: Vec::new(),
                })
            } else {
                Some(Planned {
                    edit: BufferEdit::new(at..at, style.unit()),
                    owners: Vec::new(),
                })
            }
        })
        .collect()
}
