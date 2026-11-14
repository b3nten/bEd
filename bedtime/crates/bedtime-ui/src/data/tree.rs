//! `DataTree`: a parsed JSON or TOML `Value` as rows that open and close.
//! Arrays and tables show their size; scalars their value, coloured by kind.
//! Right-click copies a value (as JSON) or its path, or opens or closes all.
use super::{Key, Kind, Value, path_text};
use crate::{UiContext, egui, theme, theme::SyntaxPalette};
use std::{collections::HashSet, sync::Arc};

/// Tables with more entries than this start closed.
const OPEN_AT_START: usize = 100;

/// A value's tree: which containers are open, which row is selected.
pub struct DataTree {
    value: Arc<Value>,
    /// Open arrays and tables, by `path_text`.
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    /// Rows for `value` and `expanded`; `None` after either changes.
    rows: Option<Vec<Row>>,
}

impl DataTree {
    /// `value` with its first level open (unless it's large).
    pub fn new(value: Arc<Value>) -> Self {
        let mut expanded = HashSet::new();
        open_first_level(&value, &mut expanded);
        Self::with_expanded(value, expanded)
    }

    /// `value` with these containers open (as saved, say).
    pub fn with_expanded(value: Arc<Value>, expanded: HashSet<String>) -> Self {
        Self {
            value,
            expanded,
            selected: None,
            rows: None,
        }
    }

    pub fn value(&self) -> &Arc<Value> {
        &self.value
    }

    /// Show `value` instead, keeping what's open.
    pub fn set_value(&mut self, value: Arc<Value>) {
        self.value = value;
        self.rows = None;
    }

    /// Draw it filling the space left (it scrolls). Returns whether
    /// `expanded` changed.
    pub fn show(&mut self, ui: &mut egui::Ui, context: &UiContext<'_>) -> bool {
        draw(ui, context, self)
    }
}

#[derive(Debug)]
struct Row {
    depth: usize,
    /// Child positions from the root.
    indices: Vec<usize>,
    path: String,
    key: String,
    index: bool,
}

fn open_first_level(value: &Value, expanded: &mut HashSet<String>) {
    let Some(children) = value.children() else {
        return;
    };
    if children.len() > OPEN_AT_START {
        return;
    }
    for (key, child) in children {
        if child.children().is_some() {
            expanded.insert(path_text(&[key]));
        }
    }
}

/// The visible rows: the root's entries, and those of open containers.
fn rows(value: &Value, expanded: &HashSet<String>) -> Vec<Row> {
    fn visit<'v>(
        value: &'v Value,
        keys: &mut Vec<Key<'v>>,
        indices: &mut Vec<usize>,
        expanded: &HashSet<String>,
        rows: &mut Vec<Row>,
    ) {
        let Some(children) = value.children() else {
            return;
        };
        for (i, (key, child)) in children.into_iter().enumerate() {
            keys.push(key);
            indices.push(i);
            let path = path_text(keys);
            let open = expanded.contains(&path);
            rows.push(Row {
                depth: keys.len() - 1,
                indices: indices.clone(),
                key: match key {
                    Key::Index(i) => i.to_string(),
                    Key::Name(name) => name.to_owned(),
                },
                index: matches!(key, Key::Index(_)),
                path,
            });
            if open {
                visit(child, keys, indices, expanded, rows);
            }
            keys.pop();
            indices.pop();
        }
    }
    let mut rows = Vec::new();
    visit(value, &mut Vec::new(), &mut Vec::new(), expanded, &mut rows);
    rows
}

fn at<'a>(value: &'a Value, indices: &[usize]) -> Option<&'a Value> {
    indices.iter().try_fold(value, |value, &i| match value {
        Value::Array(items) => items.get(i),
        Value::Table(entries) => entries.get(i).map(|(_, v)| v),
        Value::Scalar(..) => None,
    })
}

/// Every container's path, to open them all.
fn all_paths<'v>(value: &'v Value, keys: &mut Vec<Key<'v>>, out: &mut HashSet<String>) {
    let Some(children) = value.children() else {
        return;
    };
    for (key, child) in children {
        if child.children().is_some() {
            keys.push(key);
            out.insert(path_text(keys));
            all_paths(child, keys, out);
            keys.pop();
        }
    }
}

enum Command {
    Toggle(String),
    Select(String),
    ExpandAll,
    CollapseAll,
}

fn draw(ui: &mut egui::Ui, context: &UiContext<'_>, tree: &mut DataTree) -> bool {
    let id = ui.id().with("bedtime.datatree");
    let value = tree.value.clone();
    let rows = tree
        .rows
        .get_or_insert_with(|| rows(&value, &tree.expanded));
    let palette = SyntaxPalette::from_theme(context.theme);
    let p = &context.theme.palette;
    let muted = theme::color(p.muted);
    let row_height = crate::tree::row_height(context);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut command = None;
    if rows.is_empty() {
        // A document that's a single scalar, or an empty container.
        egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
            ui.label(match &*value {
                Value::Scalar(..) => value.summary(),
                _ => format!("Empty ({})", value.summary()),
            });
        });
    } else {
        egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .auto_shrink(false)
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for row in &rows[range] {
                    let Some(item) = at(&value, &row.indices) else {
                        continue;
                    };
                    let open = tree.expanded.contains(&row.path);
                    let selected = tree.selected.as_ref() == Some(&row.path);
                    let colors = RowColors {
                        key: if row.index { muted } else { palette.property },
                        muted,
                        palette: &palette,
                    };
                    let response = draw_row(
                        ui, context, row, item, open, selected, &font, row_height, &colors,
                    );
                    if response.clicked() {
                        command = Some(if item.children().is_some() {
                            Command::Toggle(row.path.clone())
                        } else {
                            Command::Select(row.path.clone())
                        });
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy Value").clicked() {
                            ui.ctx().copy_text(item.to_json());
                        }
                        if ui.button("Copy Path").clicked() {
                            ui.ctx().copy_text(row.path.clone());
                        }
                        ui.separator();
                        if ui.button("Expand All").clicked() {
                            command = Some(Command::ExpandAll);
                        }
                        if ui.button("Collapse All").clicked() {
                            command = Some(Command::CollapseAll);
                        }
                    });
                }
            });
    }
    let changed = !matches!(command, None | Some(Command::Select(_)));
    match command {
        Some(Command::Toggle(path)) => {
            if !tree.expanded.remove(&path) {
                tree.expanded.insert(path.clone());
            }
            tree.selected = Some(path);
            tree.rows = None;
        }
        Some(Command::Select(path)) => tree.selected = Some(path),
        Some(Command::ExpandAll) => {
            all_paths(&value, &mut Vec::new(), &mut tree.expanded);
            tree.rows = None;
        }
        Some(Command::CollapseAll) => {
            tree.expanded.clear();
            tree.rows = None;
        }
        None => {}
    }
    changed
}

struct RowColors<'a> {
    key: egui::Color32,
    muted: egui::Color32,
    palette: &'a SyntaxPalette,
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    row: &Row,
    item: &Value,
    open: bool,
    selected: bool,
    font: &egui::FontId,
    height: f32,
    colors: &RowColors<'_>,
) -> egui::Response {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    // "name: 3 keys", for screen readers and scripts.
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            format!("{}: {}", row.key, item.summary()),
        )
    });
    let visuals = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(2.0, 1.0)),
            visuals.corner_radius,
            visuals.bg_fill,
        );
    }
    let x = rect.left() + crate::tree::INSET + row.depth as f32 * crate::tree::INDENT;
    if item.children().is_some() {
        let openness =
            crate::animate_expansion(ui.ctx(), response.id.with("arrow"), open, context.motion);
        crate::paint_chevron(
            ui.painter(),
            egui::pos2(x + 7.0, rect.center().y),
            openness,
            colors.muted,
        );
    }
    let format = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&row.key, 0.0, format(colors.key));
    job.append("  ", 0.0, format(colors.muted));
    match item {
        Value::Scalar(kind, text) => {
            let color = match kind {
                Kind::String => colors.palette.string,
                Kind::Number => colors.palette.number,
                Kind::Bool | Kind::Null => colors.palette.keyword,
                Kind::Date => colors.palette.special,
            };
            // One line: the rest is in Copy Value.
            let line = text.lines().next().unwrap_or("");
            let shown: String = line.chars().take(400).collect();
            let more = shown.len() < text.len();
            match kind {
                Kind::String => job.append(
                    &format!("\"{shown}{}\"", if more { "…" } else { "" }),
                    0.0,
                    format(color),
                ),
                _ => job.append(&shown, 0.0, format(color)),
            }
        }
        Value::Array(_) => job.append(
            &format!("[ {} ]", item.summary()),
            0.0,
            format(colors.muted),
        ),
        Value::Table(_) => job.append(
            &format!("{{ {} }}", item.summary()),
            0.0,
            format(colors.muted),
        ),
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let text_x = x + 18.0;
    // Wide values widen the row, so the area scrolls sideways to them.
    ui.expand_to_include_x(text_x + galley.size().x + 8.0);
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y - galley.size().y / 2.0),
        galley,
        colors.muted,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Format, parse};

    #[test]
    fn rows_follow_what_is_open() {
        let value = parse(Format::Json, r#"{"a": {"b": [1, 2]}, "c": 3}"#).unwrap();
        let mut expanded = HashSet::new();
        open_first_level(&value, &mut expanded);
        let paths: Vec<_> = rows(&value, &expanded)
            .into_iter()
            .map(|r| r.path)
            .collect();
        assert_eq!(paths, ["a", "a.b", "c"]);
        expanded.insert("a.b".into());
        let rows = rows(&value, &expanded);
        let paths: Vec<_> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["a", "a.b", "a.b[0]", "a.b[1]", "c"]);
        assert_eq!(
            at(&value, &rows[3].indices),
            Some(&Value::Scalar(Kind::Number, "2".into()))
        );
        let mut all = HashSet::new();
        all_paths(&value, &mut Vec::new(), &mut all);
        assert_eq!(all, HashSet::from(["a".to_string(), "a.b".to_string()]));
    }
}
