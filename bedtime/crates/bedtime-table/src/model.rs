//! A table as a document: rows of text fields, and how its file was
//! written. Edits change cells, rows and columns; each one's inverse undoes
//! it. Clones share the rows until one is changed.
use crate::csv::{self, Format};
use bedtime_document::DocumentModel;
use bedtime_filesystem::FileModel;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct Table {
    rows: Arc<Vec<Vec<String>>>,
    pub format: Format,
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
    pub fn new(rows: Vec<Vec<String>>, format: Format) -> Self {
        Self {
            rows: Arc::new(rows),
            format,
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
}

impl DocumentModel for Table {
    type Change = TableChange;
    type OpenState = ();

    fn apply(&mut self, change: &TableChange) -> Result<TableChange, String> {
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

impl FileModel for Table {
    const MAX_LEN: u64 = 128 * 1024 * 1024;
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "It isn't UTF-8 text".to_owned())?;
        let (rows, format) = csv::parse(text, csv::sniff(text));
        Ok(Self::new(rows, format))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        csv::write(&self.rows, self.format, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        Table::decode(b"a,b\n1,2\n3\n").unwrap()
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
            // Ragged rows come back padded, which writes the same.
            let (mut a, mut b) = (Vec::new(), Vec::new());
            table.encode(&mut a).unwrap();
            original.encode(&mut b).unwrap();
            let trim = |s: Vec<u8>| String::from_utf8(s).unwrap().replace(",\n", "\n");
            assert_eq!(trim(a), trim(b), "{change:?}");
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
}
