use super::Text;
use crate::tokens::*;

/// A small table of text: a line of headings, then rows whose cells line up
/// in columns as wide as their widest cell. For a few dozen rows (a table's
/// columns, a response's headers), not a data grid.
///
/// ```ignore
/// Table::new("columns").headers(["Column", "Type"]).show(ui, |table| {
///     for column in &columns {
///         table.row(|ui| {
///             ui.add(Text::new(&column.name));
///             ui.add(Text::new(&column.declared));
///         });
///     }
/// });
/// ```
#[must_use = "a table does nothing until it's shown"]
pub struct Table {
    id: egui::Id,
    headers: Vec<String>,
    striped: bool,
    gap: Space,
    columns: Option<usize>,
}

/// A table being filled: add its rows.
pub struct TableRows<'a> {
    ui: &'a mut egui::Ui,
}

impl Table {
    /// `id` tells it from the other tables beside it.
    pub fn new(id: impl egui::AsId) -> Self {
        Self {
            id: egui::Id::new(id),
            headers: Vec::new(),
            striped: false,
            gap: Space::Lg,
            columns: None,
        }
    }

    /// A first row of headings, one for each column.
    pub fn headers<H: Into<String>>(mut self, headers: impl IntoIterator<Item = H>) -> Self {
        self.headers = headers.into_iter().map(Into::into).collect();
        self
    }
    /// How many columns it has (as many as its headings, unless set). Known,
    /// its last column takes the width that's left, so text wrapped in it
    /// (`Text::wrap`) has room.
    pub fn columns(mut self, columns: usize) -> Self {
        self.columns = Some(columns);
        self
    }
    /// Every other row shaded.
    pub fn striped(mut self) -> Self {
        self.striped = true;
        self
    }
    /// The space between columns (`Space::Lg`, unless set).
    pub fn gap(mut self, gap: Space) -> Self {
        self.gap = gap;
        self
    }

    pub fn show(self, ui: &mut egui::Ui, rows: impl FnOnce(&mut TableRows<'_>)) -> egui::Response {
        let spacing = egui::vec2(self.gap.px(ui), Space::Xs.px(ui));
        let mut grid = egui::Grid::new(self.id)
            .striped(self.striped)
            .spacing(spacing);
        let columns = self
            .columns
            .or((!self.headers.is_empty()).then_some(self.headers.len()));
        if let Some(columns) = columns {
            grid = grid.num_columns(columns);
        }
        grid.show(ui, |ui| {
            if !self.headers.is_empty() {
                for header in self.headers {
                    ui.add(Text::new(header).muted());
                }
                ui.end_row();
            }
            rows(&mut TableRows { ui });
        })
        .response
    }
}

impl TableRows<'_> {
    /// A row: what `cells` adds, each thing a cell.
    pub fn row(&mut self, cells: impl FnOnce(&mut egui::Ui)) {
        cells(self.ui);
        self.ui.end_row();
    }
}
