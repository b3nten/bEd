//! Toggling comments at every caret, with the comment tokens of the
//! language where each line is (a script inside HTML is JavaScript).
use super::carets::{Caret, Planned};
use super::lines::blocks;
use super::*;
use crate::syntax::comment_tokens;

impl EditorView {
    /// The language at `byte`: an analysis of this revision knows embedded
    /// ones; otherwise the text's.
    fn language_at(&self, revision: u64, byte: usize) -> Option<&'static str> {
        match &self.syntax {
            Some(syntax) if syntax.key.revision == revision => Some(syntax.language_at(byte)),
            _ => self.language,
        }
    }

    /// Comment each block's lines, or uncomment them if all already are.
    /// A language without line comments wraps the lines in a block comment.
    pub(super) fn toggle_line_comments(
        &self,
        snapshot: &TextSnapshot,
        carets: &[Caret],
    ) -> Vec<Planned> {
        let text = &snapshot.text;
        let mut planned = Vec::new();
        for block in blocks(text, carets, false) {
            let rows: Vec<usize> = (block.first..=block.last).collect();
            let filled: Vec<usize> = rows
                .iter()
                .copied()
                .filter(|&row| first_text(text, row) < text.line_range(row).end)
                .collect();
            // Blank lines don't count, unless they're all there is.
            let rows = if filled.is_empty() { rows } else { filled };
            let Some(tokens) = self
                .language_at(snapshot.revision, first_text(text, rows[0]))
                .and_then(comment_tokens)
            else {
                continue;
            };
            match (tokens.line, tokens.block) {
                (Some(token), _) => planned.extend(line_comment(text, &rows, token)),
                (None, Some((open, close))) => planned.extend(wrap_lines(text, &rows, open, close)),
                (None, None) => {}
            }
        }
        planned
    }

    /// Wrap each selection in a block comment (an empty one at a caret), or
    /// unwrap one that is exactly a block comment. The text inside stays
    /// selected.
    pub(super) fn toggle_block_comments(
        &self,
        snapshot: &TextSnapshot,
        carets: &[Caret],
    ) -> Vec<Planned> {
        let text = &snapshot.text;
        let mut planned = Vec::new();
        for (index, caret) in carets.iter().enumerate() {
            let range = caret.selection.range();
            let Some((open, close)) = self
                .language_at(snapshot.revision, range.start)
                .and_then(comment_tokens)
                .and_then(|tokens| tokens.block)
            else {
                continue;
            };
            if range.is_empty() {
                let head = range.start + open.len() + 1;
                planned.push(Planned {
                    edit: BufferEdit::new(range.clone(), format!("{open}  {close}")),
                    owners: vec![(index, TextSelection { anchor: head, head })],
                });
                continue;
            }
            let selected = text.slice(range.clone()).to_string();
            let (start, end) = (range.start, range.end);
            if selected.len() >= open.len() + close.len()
                && selected.starts_with(open)
                && selected.ends_with(close)
            {
                let inner = &selected[open.len()..selected.len() - close.len()];
                let before = open.len() + usize::from(inner.starts_with(' '));
                let after = close.len() + usize::from(inner.len() > 1 && inner.ends_with(' '));
                planned.push(Planned {
                    edit: BufferEdit::new(start..start + before, ""),
                    owners: Vec::new(),
                });
                // Placed by the last change, after the first's shift.
                let inner_end = end - after;
                planned.push(Planned {
                    edit: BufferEdit::new(inner_end..end, ""),
                    owners: vec![(index, oriented(caret.selection, start + before, inner_end))],
                });
            } else {
                planned.push(Planned {
                    edit: BufferEdit::new(start..start, format!("{open} ")),
                    owners: Vec::new(),
                });
                planned.push(Planned {
                    edit: BufferEdit::new(end..end, format!(" {close}")),
                    owners: vec![(index, oriented(caret.selection, start, end))],
                });
            }
        }
        planned
    }
}

/// `start..end` in `selection`'s direction.
fn oriented(selection: TextSelection, start: usize, end: usize) -> TextSelection {
    if selection.head >= selection.anchor {
        TextSelection {
            anchor: start,
            head: end,
        }
    } else {
        TextSelection {
            anchor: end,
            head: start,
        }
    }
}

/// Where a line's text starts (its end, if it's blank).
fn first_text(text: &TextBuffer, row: usize) -> usize {
    whitespace::leading_end(text, row, text.line_range(row).end)
}

/// Whether `text` at `at` reads `token`, within its line.
fn reads(text: &TextBuffer, at: usize, line_end: usize, token: &str) -> bool {
    let end = at + token.len();
    end <= line_end && text.is_char_boundary(end) && text.slice(at..end) == token
}

/// `token` and a space at the lines' least indentation, or each line's
/// `token` (and a space after it) away if all have one.
fn line_comment(text: &TextBuffer, rows: &[usize], token: &str) -> Vec<Planned> {
    let commented = rows.iter().all(|&row| {
        let line = text.line_range(row);
        reads(text, first_text(text, row), line.end, token)
    });
    if commented {
        return rows
            .iter()
            .map(|&row| {
                let line = text.line_range(row);
                let at = first_text(text, row);
                let mut end = at + token.len();
                if end < line.end && text.byte(end) == Some(b' ') {
                    end += 1;
                }
                Planned {
                    edit: BufferEdit::new(at..end, ""),
                    owners: Vec::new(),
                }
            })
            .collect();
    }
    let column = rows
        .iter()
        .map(|&row| text.column(first_text(text, row)))
        .min()
        .unwrap_or(0);
    rows.iter()
        .map(|&row| {
            let at = text.byte_at_column(row, column);
            Planned {
                edit: BufferEdit::new(at..at, format!("{token} ")),
                owners: Vec::new(),
            }
        })
        .collect()
}

/// The lines' text (from the first's text to the last's end) wrapped in
/// `open` and `close`, or unwrapped if it already is.
fn wrap_lines(text: &TextBuffer, rows: &[usize], open: &str, close: &str) -> Vec<Planned> {
    let start = first_text(text, rows[0]);
    let last = *rows.last().unwrap();
    let end = text
        .trailing_whitespace_start(text.line_range(last))
        .max(start);
    let content = text.slice(start..end).to_string();
    let edits = if content.len() >= open.len() + close.len()
        && content.starts_with(open)
        && content.ends_with(close)
    {
        let inner = &content[open.len()..content.len() - close.len()];
        let before = open.len() + usize::from(inner.starts_with(' '));
        let after = close.len() + usize::from(inner.len() > 1 && inner.ends_with(' '));
        [
            BufferEdit::new(start..start + before, ""),
            BufferEdit::new(end - after..end, ""),
        ]
    } else {
        [
            BufferEdit::new(start..start, format!("{open} ")),
            BufferEdit::new(end..end, format!(" {close}")),
        ]
    };
    edits
        .into_iter()
        .map(|edit| Planned {
            edit,
            owners: Vec::new(),
        })
        .collect()
}
