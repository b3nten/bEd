//! Whole-line commands (delete, copy, move, insert, join, indent), the
//! clipboard's whole lines when nothing is selected, and growing a selection
//! through the syntax tree. Each works at every caret, as one change.
//!
//! Line commands act on blocks: each caret's lines (a selection ending at a
//! line's start doesn't take that line), carets sharing lines together.
use super::carets::{Caret, Planned, indent_lines};
use super::*;

/// Lines `first..=last` and the carets (indices) on them.
pub(super) struct Block {
    pub first: usize,
    pub last: usize,
    pub carets: Vec<usize>,
}

/// A selection's first and last line.
fn selection_lines(text: &TextBuffer, range: Range<usize>) -> (usize, usize) {
    let first = text.line_of(range.start);
    let last = text.line_of(range.end);
    if range.end > range.start && range.end == text.line_start(last) && last > first {
        (first, last - 1)
    } else {
        (first, last)
    }
}

/// The carets' lines, joined where they share lines (and with `touching`,
/// where they're next to each other: they move as one).
pub(super) fn blocks(text: &TextBuffer, carets: &[Caret], touching: bool) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for (index, caret) in carets.iter().enumerate() {
        let (first, last) = selection_lines(text, caret.selection.range());
        if let Some(block) = blocks.last_mut()
            && (first <= block.last || touching && first == block.last + 1)
        {
            block.last = block.last.max(last);
            block.carets.push(index);
            continue;
        }
        blocks.push(Block {
            first,
            last,
            carets: vec![index],
        });
    }
    blocks
}

/// Lines `start` up to and including `end` as they are: from the first's
/// start to the start of the line after the last.
fn lines_range(text: &TextBuffer, first: usize, last: usize) -> Range<usize> {
    text.line_start(first)..text.line_start(last + 1)
}

/// A selection moved by `offset`.
fn shifted(selection: TextSelection, offset: isize) -> TextSelection {
    TextSelection {
        anchor: (selection.anchor as isize + offset) as usize,
        head: (selection.head as isize + offset) as usize,
    }
}

/// Undo of growing a selection: the selections before each step.
#[derive(Default)]
pub(super) struct Expansions {
    before: Vec<Vec<Caret>>,
    /// The selections the last step made, at `revision`: shrinking only
    /// steps back while they're unchanged.
    after: Vec<TextSelection>,
    revision: u64,
}

impl EditorView {
    /// Delete each block's lines. The caret keeps its column on the line
    /// that takes their place (the one before, for the last lines).
    pub(super) fn delete_lines(&self, text: &TextBuffer, carets: &[Caret]) -> Vec<Planned> {
        blocks(text, carets, false)
            .into_iter()
            .map(|block| {
                let lines = lines_range(text, block.first, block.last);
                let at_end = block.last + 1 >= text.line_count();
                let (range, row) = if at_end && block.first > 0 {
                    // No line after: take the newline before instead.
                    (
                        text.line_range(block.first - 1).end..lines.end,
                        Some(block.first - 1),
                    )
                } else if at_end {
                    (lines, None)
                } else {
                    (lines, Some(block.last + 1))
                };
                let owners = block
                    .carets
                    .iter()
                    .map(|&index| {
                        let column = text.column(carets[index].selection.head);
                        let at = match row {
                            Some(row) if row > block.last => {
                                text.byte_at_column(row, column) - range.len()
                            }
                            Some(row) => text.byte_at_column(row, column),
                            None => range.start,
                        };
                        (
                            index,
                            TextSelection {
                                anchor: at,
                                head: at,
                            },
                        )
                    })
                    .collect();
                Planned {
                    edit: BufferEdit::new(range, ""),
                    owners,
                }
            })
            .collect()
    }

    /// Copy each block's lines above or below them; the selections go with
    /// the lower copy when copying down, and stay when copying up.
    fn copy_lines(&self, text: &TextBuffer, carets: &[Caret], down: bool) -> Vec<Planned> {
        let eol = whitespace::ending(text);
        blocks(text, carets, true)
            .into_iter()
            .map(|block| {
                let range = lines_range(text, block.first, block.last);
                let lines = text.slice(range.clone()).to_string();
                let ends_line = range.end > text.line_range(block.last).end;
                let (at, insert) = match (down, ends_line) {
                    (true, true) => (range.end, lines),
                    (true, false) => (range.end, format!("{eol}{lines}")),
                    (false, true) => (range.start, lines),
                    (false, false) => (range.start, format!("{lines}{eol}")),
                };
                let offset = if down { insert.len() as isize } else { 0 };
                let owners = block
                    .carets
                    .iter()
                    .map(|&index| (index, shifted(carets[index].selection, offset)))
                    .collect();
                Planned {
                    edit: BufferEdit::new(at..at, insert),
                    owners,
                }
            })
            .collect()
    }

    /// Swap each block with the line below (or above) it. Line endings stay
    /// where they were, so a last line without one stays without.
    fn move_lines(&self, text: &TextBuffer, carets: &[Caret], down: bool) -> Vec<Planned> {
        blocks(text, carets, true)
            .into_iter()
            .filter_map(|block| {
                let (first, last) = if down {
                    (block.first, block.last + 1)
                } else {
                    (block.first.checked_sub(1)?, block.last)
                };
                if last >= text.line_count() {
                    return None;
                }
                let order: Vec<usize> = if down {
                    std::iter::once(last).chain(first..last).collect()
                } else {
                    (block.first..=last).chain([first]).collect()
                };
                let region = lines_range(text, first, last);
                let mut replacement = String::with_capacity(region.len());
                // Where each line (by its row) starts in the replacement.
                let mut starts = vec![0; last - first + 1];
                for (slot, &row) in order.iter().enumerate() {
                    starts[row - first] = replacement.len();
                    replacement.push_str(&text.slice(text.line_range(row)).to_string());
                    let separator = first + slot;
                    let ending = text.line_range(separator).end..text.line_start(separator + 1);
                    replacement.push_str(&text.slice(ending).to_string());
                }
                let place = |byte: usize| {
                    let row = text.line_of(byte);
                    // A selection's end at the next line's start: after the block.
                    if byte >= region.end || row > block.last {
                        return region.start + replacement.len();
                    }
                    let line = text.line_range(row);
                    region.start + starts[row - first] + (byte - line.start).min(line.len())
                };
                let owners = block
                    .carets
                    .iter()
                    .map(|&index| {
                        let selection = carets[index].selection;
                        (
                            index,
                            TextSelection {
                                anchor: place(selection.anchor),
                                head: place(selection.head),
                            },
                        )
                    })
                    .collect();
                Some(Planned {
                    edit: BufferEdit::new(region, replacement),
                    owners,
                })
            })
            .collect()
    }

    /// A new line below (or above) each caret's, indented like it.
    fn insert_lines(&self, text: &TextBuffer, carets: &[Caret], below: bool) -> Vec<Planned> {
        let style = self.settings.indentation;
        let smart = self.settings.smart_indent;
        let mut planned: Vec<Planned> = Vec::new();
        for (index, caret) in carets.iter().enumerate() {
            let (first, last) = selection_lines(text, caret.selection.range());
            let row = if below { last } else { first };
            if let Some(previous) = planned.last_mut()
                && text.line_of(previous.edit.range().start) == row
            {
                let placed = previous.owners[0].1;
                previous.owners.push((index, placed));
                continue;
            }
            let (edit, head) = if below {
                let end = text.line_range(row).end;
                whitespace::newline(
                    text,
                    TextSelection {
                        anchor: end,
                        head: end,
                    },
                    style,
                    smart,
                )
            } else {
                let start = text.line_start(row);
                let indent_end = whitespace::leading_end(text, row, text.line_range(row).end);
                let indent = text.slice(start..indent_end).to_string();
                let head = start + indent.len();
                (
                    BufferEdit::new(
                        start..start,
                        format!("{indent}{}", whitespace::ending(text)),
                    ),
                    head,
                )
            };
            planned.push(Planned {
                edit,
                owners: vec![(index, TextSelection { anchor: head, head })],
            });
        }
        planned
    }

    /// Join each caret's line with the next (a selection's lines with each
    /// other): their whitespace between becomes one space.
    fn join_lines(&self, text: &TextBuffer, carets: &[Caret]) -> Vec<Planned> {
        let mut joins: Vec<(usize, Vec<usize>)> = Vec::new();
        for (index, caret) in carets.iter().enumerate() {
            let (first, last) = selection_lines(text, caret.selection.range());
            let rows = if first == last {
                first..first + 1
            } else {
                first..last
            };
            for row in rows.filter(|&row| row + 1 < text.line_count()) {
                // Only carets go to the join; selections follow the text.
                let owner = (first == last).then_some(index);
                match joins.iter_mut().find(|(r, _)| *r == row) {
                    Some((_, owners)) => owners.extend(owner),
                    None => joins.push((row, owner.into_iter().collect())),
                }
            }
        }
        joins.sort_by_key(|(row, _)| *row);
        // Whether text precedes the join, through empty lines joined before.
        let mut text_before: Option<(usize, bool)> = None;
        joins
            .into_iter()
            .map(|(row, owners)| {
                let line = text.line_range(row);
                let next = text.line_range(row + 1);
                let start = text.trailing_whitespace_start(line.clone());
                let end = whitespace::leading_end(text, row + 1, next.end);
                let left = start > line.start
                    || text_before.is_some_and(|(previous, any)| previous + 1 == row && any);
                text_before = Some((row, left));
                let space = left && end < next.end;
                Planned {
                    edit: BufferEdit::new(start..end, if space { " " } else { "" }),
                    owners: owners
                        .into_iter()
                        .map(|index| {
                            (
                                index,
                                TextSelection {
                                    anchor: start,
                                    head: start,
                                },
                            )
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// Run a line command: plan it from the text, then apply it as one
    /// change and one undo step.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_line_command(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        command: EditorCommand,
        now: f64,
    ) -> Result<(), EditorError> {
        let carets = self.carets();
        let text = &snapshot.text;
        let style = self.settings.indentation;
        let planned = match command {
            EditorCommand::DeleteLine => self.delete_lines(text, &carets),
            EditorCommand::CopyLineUp => self.copy_lines(text, &carets, false),
            EditorCommand::CopyLineDown => self.copy_lines(text, &carets, true),
            EditorCommand::MoveLineUp => self.move_lines(text, &carets, false),
            EditorCommand::MoveLineDown => self.move_lines(text, &carets, true),
            EditorCommand::InsertLineAbove => self.insert_lines(text, &carets, false),
            EditorCommand::InsertLineBelow => self.insert_lines(text, &carets, true),
            EditorCommand::JoinLines => self.join_lines(text, &carets),
            EditorCommand::IndentLines => indent_lines(text, &carets, style, false),
            EditorCommand::OutdentLines => indent_lines(text, &carets, style, true),
            EditorCommand::ToggleLineComment => self.toggle_line_comments(snapshot, &carets),
            EditorCommand::ToggleBlockComment => self.toggle_block_comments(snapshot, &carets),
            _ => return Ok(()),
        };
        session.end_undo_group();
        self.commit_carets(id, session, snapshot, now, carets, planned)?;
        session.end_undo_group();
        Ok(())
    }

    /// Copy and Cut with nothing selected take each caret's whole line, and
    /// pasting those lines puts them above the caret's line (one each, with
    /// as many carets as lines).
    pub(super) fn line_clipboard_event(
        &mut self,
        ui: &egui::Ui,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        event: &egui::Event,
        now: f64,
    ) -> Result<Option<bool>, EditorError> {
        if !matches!(
            event,
            egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
        ) {
            return Ok(None);
        }
        let carets = self.carets();
        if carets
            .iter()
            .any(|caret| !caret.selection.range().is_empty())
        {
            return Ok(None);
        }
        let text = &snapshot.text;
        match event {
            egui::Event::Copy | egui::Event::Cut => {
                let eol = whitespace::ending(text);
                let mut copied = String::new();
                for block in blocks(text, &carets, false) {
                    let range = lines_range(text, block.first, block.last);
                    let ends_line = range.end > text.line_range(block.last).end;
                    copied.push_str(&text.slice(range).to_string());
                    if !ends_line {
                        copied.push_str(eol);
                    }
                }
                ui.ctx().copy_text(copied.clone());
                self.line_clipboard = Some(copied);
                if matches!(event, egui::Event::Cut) && !self.settings.read_only {
                    let planned = self.delete_lines(text, &carets);
                    session.end_undo_group();
                    self.commit_carets(id, session, snapshot, now, carets, planned)?;
                    session.end_undo_group();
                }
                Ok(Some(true))
            }
            egui::Event::Paste(pasted)
                if !self.settings.read_only
                    && self.line_clipboard.as_deref() == Some(pasted.as_str()) =>
            {
                let lines: Vec<&str> = pasted.split_inclusive('\n').collect();
                let spread = carets.len() > 1 && lines.len() == carets.len();
                let mut planned: Vec<Planned> = Vec::new();
                for (index, caret) in carets.iter().enumerate() {
                    let start = text.line_start(text.line_of(caret.selection.head));
                    let insert = if spread {
                        lines[index]
                    } else {
                        pasted.as_str()
                    };
                    match planned.last_mut() {
                        Some(previous) if previous.edit.range().start == start && !spread => {}
                        _ => planned.push(Planned {
                            edit: BufferEdit::new(start..start, insert),
                            owners: Vec::new(),
                        }),
                    }
                }
                session.end_undo_group();
                self.commit_carets(id, session, snapshot, now, carets, planned)?;
                session.end_undo_group();
                Ok(Some(true))
            }
            _ => Ok(None),
        }
    }

    /// Grow every selection to the next larger piece of syntax: a caret's
    /// word first, then the nodes around it. Without an analysis of this
    /// revision: the word, the line, the whole text.
    pub(super) fn expand_selection(&mut self, snapshot: &TextSnapshot) {
        let text = &snapshot.text;
        let carets = self.carets();
        let current: Vec<_> = carets.iter().map(|caret| caret.selection).collect();
        if self.expansions.revision != snapshot.revision || self.expansions.after != current {
            self.expansions.before.clear();
        }
        let tree = self
            .syntax
            .as_ref()
            .filter(|syntax| syntax.key.revision == snapshot.revision)
            .map(|syntax| &syntax.tree);
        let grown: Vec<Caret> = carets
            .iter()
            .map(|caret| {
                let range = grow(text, tree, caret.selection.range());
                Caret {
                    selection: TextSelection {
                        anchor: range.start,
                        head: range.end,
                    },
                    column: None,
                    upstream: false,
                    ..*caret
                }
            })
            .collect();
        self.expansions.before.push(carets);
        self.set_carets(grown, snapshot.revision);
        self.expansions.after = self.selections();
        self.expansions.revision = snapshot.revision;
    }

    /// Step back through what growing did, while its selections stand.
    pub(super) fn shrink_selection(&mut self, snapshot: &TextSnapshot) {
        if self.expansions.revision != snapshot.revision
            || self.expansions.after != self.selections()
        {
            self.expansions.before.clear();
            return;
        }
        if let Some(carets) = self.expansions.before.pop() {
            self.set_carets(carets, snapshot.revision);
            self.expansions.after = self.selections();
        }
    }
}

/// The next larger range around `range`.
fn grow(
    text: &TextBuffer,
    tree: Option<&crate::syntax::tree_sitter::Tree>,
    range: Range<usize>,
) -> Range<usize> {
    if range.is_empty() {
        let at = if class(text, range.start) != 1 && range.start > 0 {
            text.previous_grapheme(range.start)
        } else {
            range.start
        };
        if class(text, at) == 1 {
            return word_at(text, at);
        }
    }
    if let Some(tree) = tree {
        let mut node = tree
            .root_node()
            .descendant_for_byte_range(range.start, range.end);
        while let Some(current) = node {
            let found = current.byte_range();
            if found.start <= range.start && found.end >= range.end && found != range {
                return found;
            }
            node = current.parent();
        }
        return 0..text.len();
    }
    let (first, last) = selection_lines(text, range.clone());
    let lines = text.line_start(first)..text.line_range(last).end;
    if lines.start <= range.start && lines.end >= range.end && lines != range {
        lines
    } else {
        0..text.len()
    }
}
