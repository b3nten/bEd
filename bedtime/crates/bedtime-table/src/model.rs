//! A table read from text: rows of text fields, how the text was written,
//! and where each row starts in it. Table edits (a cell, inserting and
//! deleting rows and columns) become edits of the text that touch only what
//! they change, so an editor showing the same text keeps its place. Clones
//! share the rows until one is changed.
use crate::csv::{self, Format};
use bedtime_document::{BufferEdit, TextBuffer};
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug, Default)]
pub struct Table {
    rows: Arc<Vec<Vec<String>>>,
    pub format: Format,
    /// Each row's first byte in the text.
    starts: Arc<Vec<usize>>,
}

impl PartialEq for Table {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && (Arc::ptr_eq(&self.rows, &other.rows) || self.rows == other.rows)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TableChange {
    SetCell {
        row: usize,
        column: usize,
        text: String,
    },
    InsertRow {
        at: usize,
        cells: Vec<String>,
    },
    DeleteRow {
        at: usize,
    },
    InsertColumn {
        at: usize,
        cells: Vec<String>,
    },
    DeleteColumn {
        at: usize,
    },
}

impl Table {
    /// The table in `text`, its delimiter sniffed.
    pub fn parse(text: &str) -> Self {
        let (rows, format, starts) = csv::parse(text, csv::sniff(text));
        Self {
            rows: Arc::new(rows),
            format,
            starts: Arc::new(starts),
        }
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// The widest row's length.
    pub fn columns(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub fn cell(&self, row: usize, column: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|cells| cells.get(column))
            .map_or("", String::as_str)
    }

    /// Make `change` in the table, and return the edit that makes it in
    /// `text` (the text the table was read from, as edited since by this
    /// table's edits only).
    pub fn edit(&mut self, text: &TextBuffer, change: &TableChange) -> Result<BufferEdit, String> {
        let (edit, starts) = self.text_edit(text, change)?;
        self.apply(change)?;
        self.starts = Arc::new(starts);
        Ok(edit)
    }

    /// The text edit for `change`, and where rows start after it.
    fn text_edit(
        &self,
        text: &TextBuffer,
        change: &TableChange,
    ) -> Result<(BufferEdit, Vec<usize>), String> {
        let delimiter = self.format.delimiter;
        let newline = if self.format.crlf { "\r\n" } else { "\n" };
        let mut starts = (*self.starts).clone();
        let shift = |starts: &mut [usize], edit: &BufferEdit| {
            let grown = edit.replacement().len() as isize - edit.range().len() as isize;
            for start in starts {
                *start = start.saturating_add_signed(grown);
            }
        };
        let rows = starts.len();
        let edit = match change {
            TableChange::SetCell {
                row,
                column,
                text: value,
            } => {
                let row = *row;
                if row >= rows {
                    return Err("There's no such row".into());
                }
                let line = self.row_range(text, row);
                let spans = csv::field_spans(&text.range_text(line.clone()), delimiter);
                let value = csv::quote(value, delimiter);
                let edit = match spans.get(*column) {
                    Some(span) => BufferEdit::new(
                        line.start + span.start..line.start + span.end,
                        value.as_ref(),
                    ),
                    // Past the row's end: pad it with empty fields.
                    None => {
                        let mut padded =
                            (delimiter as char).to_string().repeat(column + 1 - spans.len());
                        padded.push_str(&value);
                        BufferEdit::new(line.end..line.end, padded)
                    }
                };
                shift(&mut starts[row + 1..], &edit);
                edit
            }
            TableChange::InsertRow { at, cells } => {
                let at = *at;
                if at > rows {
                    return Err("There's no such row".into());
                }
                let line = csv::row_text(cells, delimiter);
                if at < rows {
                    let edit = BufferEdit::new(starts[at]..starts[at], line + newline);
                    starts.insert(at, starts[at]);
                    shift(&mut starts[at + 1..], &edit);
                    edit
                } else {
                    // After the last row, which may not end with a line break.
                    let end = text.len();
                    if end == text_start(text) || text.byte(end - 1) == Some(b'\n') {
                        starts.push(end);
                        BufferEdit::new(end..end, line + newline)
                    } else {
                        starts.push(end + newline.len());
                        BufferEdit::new(end..end, format!("{newline}{line}"))
                    }
                }
            }
            TableChange::DeleteRow { at } => {
                let at = *at;
                if at >= rows {
                    return Err("There's no such row".into());
                }
                let range = if at + 1 < rows {
                    starts[at]..starts[at + 1]
                } else if at > 0 && text.byte(text.len() - 1) != Some(b'\n') {
                    // The last row, with no line break after it: take the
                    // one before it instead, so none is left at the end.
                    self.row_range(text, at - 1).end..text.len()
                } else {
                    starts[at]..text.len()
                };
                let edit = BufferEdit::new(range, "");
                starts.remove(at);
                shift(&mut starts[at..], &edit);
                edit
            }
            TableChange::InsertColumn { .. } | TableChange::DeleteColumn { .. } => {
                // Every row changes: write them all again.
                let mut table = self.clone();
                table.apply(change)?;
                let begin = text_start(text);
                let mut out = String::new();
                starts.clear();
                for (index, row) in table.rows.iter().enumerate() {
                    starts.push(begin + out.len());
                    out.push_str(&csv::row_text(row, delimiter));
                    if index + 1 < table.rows.len() || self.format.final_newline {
                        out.push_str(newline);
                    }
                }
                BufferEdit::new(begin..text.len(), out)
            }
        };
        Ok((edit, starts))
    }

    /// A row's text, without its line break.
    fn row_range(&self, text: &TextBuffer, row: usize) -> Range<usize> {
        let start = self.starts[row];
        let mut end = self.starts.get(row + 1).copied().unwrap_or(text.len());
        if end > start && text.byte(end - 1) == Some(b'\n') {
            end -= 1;
            if end > start && text.byte(end - 1) == Some(b'\r') {
                end -= 1;
            }
        }
        start..end
    }

    /// Make `change` in the rows; returns the change that undoes it.
    pub fn apply(&mut self, change: &TableChange) -> Result<TableChange, String> {
        let columns = self.columns();
        let rows = Arc::make_mut(&mut self.rows);
        match change {
            TableChange::SetCell { row, column, text } => {
                let cells = rows.get_mut(*row).ok_or("There's no such row")?;
                if cells.len() <= *column {
                    cells.resize(column + 1, String::new());
                }
                let old = std::mem::replace(&mut cells[*column], text.clone());
                Ok(TableChange::SetCell {
                    row: *row,
                    column: *column,
                    text: old,
                })
            }
            TableChange::InsertRow { at, cells } => {
                if *at > rows.len() {
                    return Err("There's no such row".into());
                }
                rows.insert(*at, cells.clone());
                Ok(TableChange::DeleteRow { at: *at })
            }
            TableChange::DeleteRow { at } => {
                if *at >= rows.len() {
                    return Err("There's no such row".into());
                }
                let cells = rows.remove(*at);
                Ok(TableChange::InsertRow { at: *at, cells })
            }
            TableChange::InsertColumn { at, cells } => {
                if *at > columns {
                    return Err("There's no such column".into());
                }
                for (index, row) in rows.iter_mut().enumerate() {
                    if row.len() < *at {
                        row.resize(*at, String::new());
                    }
                    row.insert(*at, cells.get(index).cloned().unwrap_or_default());
                }
                Ok(TableChange::DeleteColumn { at: *at })
            }
            TableChange::DeleteColumn { at } => {
                if *at >= columns {
                    return Err("There's no such column".into());
                }
                let cells = rows
                    .iter_mut()
                    .map(|row| {
                        if *at < row.len() {
                            row.remove(*at)
                        } else {
                            String::new()
                        }
                    })
                    .collect();
                Ok(TableChange::InsertColumn { at: *at, cells })
            }
        }
    }
}

/// Where a text's rows begin: after a byte order mark.
fn text_start(text: &TextBuffer) -> usize {
    if text.len() >= 3 && text.range_text(0..3) == "\u{feff}" {
        3
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a,b\n1,2\n3\n";

    fn table() -> Table {
        Table::parse(TEXT)
    }

    #[test]
    fn changes_undo() {
        let original = table();
        for change in [
            TableChange::SetCell {
                row: 2,
                column: 1,
                text: "4".into(),
            },
            TableChange::InsertRow {
                at: 1,
                cells: vec!["x".into()],
            },
            TableChange::DeleteRow { at: 0 },
            TableChange::InsertColumn {
                at: 2,
                cells: vec!["c".into()],
            },
            TableChange::DeleteColumn { at: 0 },
        ] {
            let mut table = original.clone();
            let undo = table.apply(&change).unwrap();
            assert_ne!(table, original, "{change:?}");
            table.apply(&undo).unwrap();
            // Ragged rows come back padded.
            let trim = |rows: &[Vec<String>]| {
                rows.iter()
                    .map(|row| row.join(",").trim_end_matches(',').to_owned())
                    .collect::<Vec<_>>()
            };
            assert_eq!(trim(table.rows()), trim(original.rows()), "{change:?}");
        }
    }

    #[test]
    fn cells_past_a_rows_end_are_empty_and_settable() {
        let mut table = table();
        assert_eq!(table.cell(2, 1), "");
        assert_eq!(table.columns(), 2);
        table
            .apply(&TableChange::SetCell {
                row: 2,
                column: 3,
                text: "z".into(),
            })
            .unwrap();
        assert_eq!(table.cell(2, 3), "z");
        assert_eq!(table.columns(), 4);
        assert!(table.apply(&TableChange::DeleteRow { at: 9 }).is_err());
    }

    /// Each change, made in turn to the text, leaves text that reads back as
    /// the table, and only the text it changes is touched.
    #[test]
    fn edits_change_the_text_as_the_table() {
        let cases: &[(&str, &[TableChange], &str)] = &[
            (
                "a,b\n\"x, y\",2\n3",
                &[
                    TableChange::SetCell {
                        row: 1,
                        column: 0,
                        text: "z".into(),
                    },
                    TableChange::SetCell {
                        row: 2,
                        column: 2,
                        text: "q r ".into(),
                    },
                    TableChange::InsertRow {
                        at: 3,
                        cells: vec!["end".into()],
                    },
                    TableChange::DeleteRow { at: 0 },
                ],
                "z,2\n3,,\"q r \"\nend",
            ),
            (
                "\u{feff}a;b\r\n1;2\r\n",
                &[
                    TableChange::InsertRow {
                        at: 1,
                        cells: vec!["x".into(), "y".into()],
                    },
                    TableChange::InsertRow {
                        at: 3,
                        cells: vec!["last".into()],
                    },
                    TableChange::SetCell {
                        row: 2,
                        column: 1,
                        text: "two".into(),
                    },
                    TableChange::DeleteRow { at: 3 },
                ],
                "\u{feff}a;b\r\nx;y\r\n1;two\r\n",
            ),
            (
                "a,b\n1,2\n",
                &[
                    TableChange::InsertColumn {
                        at: 1,
                        cells: vec!["m".into()],
                    },
                    TableChange::SetCell {
                        row: 1,
                        column: 2,
                        text: "3".into(),
                    },
                    TableChange::DeleteColumn { at: 0 },
                    TableChange::DeleteRow { at: 1 },
                ],
                "m,b\n",
            ),
        ];
        for (before, changes, after) in cases {
            let mut text = TextBuffer::from(*before);
            let mut table = Table::parse(before);
            for change in *changes {
                let edit = table.edit(&text, change).unwrap();
                edit.apply(&mut text).unwrap();
                let read = Table::parse(&text.to_string());
                assert_eq!(read.rows(), table.rows(), "{change:?} in {before:?}");
                assert_eq!(read.starts, table.starts, "{change:?} in {before:?}");
            }
            assert_eq!(text.to_string(), *after);
        }
    }
}
