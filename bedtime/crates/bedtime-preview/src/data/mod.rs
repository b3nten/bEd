//! Tree View: a JSON or TOML document as rows that open and close. Arrays
//! and tables show their size; scalars their value, coloured by kind. It
//! parses each revision on a worker and keeps showing the last one that
//! parsed, with the error, while the text doesn't.
use bedtime_code::editor::syntax::SyntaxPalette;
use bedtime_document::{TextBuffer, TextOpenState};
use bedtime_ui::{UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentHandle, DocumentReference, ManagedTask, ManagedWork, Panel, Viewer, WorkbenchWorldExt,
    viewer_document,
};
use bevy::prelude::*;
use std::{collections::HashSet, sync::Arc, time::Duration};

mod value;
pub use value::{Format, Key, Kind, Value, parse, path_text};

/// Tables with more entries than this start closed.
const OPEN_AT_START: usize = 100;

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct TreePanel {
    /// Open arrays and tables, by `path_text`.
    expanded: HashSet<String>,
    #[reflect(ignore)]
    state: State,
}

#[derive(Default)]
struct State {
    shown: Option<Parsed>,
    /// The last revision that didn't parse, and why.
    error: Option<(u64, String)>,
    parsing: Option<(u64, ManagedTask<Result<Value, String>>)>,
    /// Rows for `shown` and `expanded`; `None` after either changes.
    rows: Option<Vec<Row>>,
    selected: Option<String>,
}

struct Parsed {
    revision: u64,
    value: Arc<Value>,
}

struct Row {
    depth: usize,
    /// Child positions from the root.
    indices: Vec<usize>,
    path: String,
    key: String,
    index: bool,
}

impl Viewer<TextBuffer> for TreePanel {
    fn new(_: DocumentHandle<TextBuffer>) -> Self {
        Self::default()
    }
    fn apply(_: &mut World, _: Entity, _: &TextOpenState) {}
}

impl Panel for TreePanel {
    const LABEL: &'static str = "Tree View";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<TextBuffer>(ui, world, panel) else {
            return;
        };
        let title = world
            .get::<DocumentReference>(panel)
            .and_then(|reference| {
                world
                    .workbench()
                    .inspect_document(reference.document())
                    .ok()
            })
            .map(|info| info.title)
            .unwrap_or_default();
        let Some(format) = Format::of(&title) else {
            ui.label(format!("{title} isn't JSON or TOML"));
            return;
        };
        update(world, panel, format, &document.model, document.revision);
        if world
            .get::<TreePanel>(panel)
            .is_some_and(|tree| tree.state.parsing.is_some())
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let Some(mut tree) = world.get_mut::<TreePanel>(panel) else {
            return;
        };
        draw(ui, context, panel, &mut tree);
    }
}

/// Collect a finished parse, and start one for a newer revision.
fn update(world: &mut World, panel: Entity, format: Format, text: &TextBuffer, revision: u64) {
    let Some(mut tree) = world.get_mut::<TreePanel>(panel) else {
        return;
    };
    let tree = &mut *tree;
    if let Some((parsed, task)) = &tree.state.parsing {
        match task.try_recv() {
            Ok(Ok(value)) => {
                if tree.state.shown.is_none() && tree.expanded.is_empty() {
                    open_first_level(&value, &mut tree.expanded);
                }
                tree.state.shown = Some(Parsed {
                    revision: *parsed,
                    value: Arc::new(value),
                });
                tree.state.error = None;
                tree.state.rows = None;
                tree.state.parsing = None;
            }
            Ok(Err(error)) => {
                tree.state.error = Some((*parsed, error));
                tree.state.parsing = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => tree.state.parsing = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
    }
    let done = tree.state.shown.as_ref().map(|shown| shown.revision);
    let failed = tree.state.error.as_ref().map(|(revision, _)| *revision);
    if done == Some(revision) || failed == Some(revision) {
        return;
    }
    let text = text.clone();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Reading the tree",
        move |_| parse(format, &text.range_text(0..text.len())),
    );
    if let (Ok(task), Some(mut tree)) = (task, world.get_mut::<TreePanel>(panel)) {
        tree.state.parsing = Some((revision, task));
    }
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

fn draw(ui: &mut egui::Ui, context: &UiContext<'_>, panel: Entity, tree: &mut TreePanel) {
    let id = egui::Id::new(("bedtime.preview.tree", panel));
    let Some(shown) = tree.state.shown.as_ref() else {
        match &tree.state.error {
            Some((_, error)) => {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    ui.label(format!("This doesn't parse: {error}"));
                });
            }
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
        }
        return;
    };
    let value = shown.value.clone();
    let rows = tree
        .state
        .rows
        .get_or_insert_with(|| rows(&value, &tree.expanded));
    let palette = SyntaxPalette::from_theme(context.theme);
    let p = &context.theme.palette;
    let muted = theme::color(p.muted);
    let row_height = bedtime_ui::tree::row_height(context);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    // The text doesn't parse now: say why, above the last tree that did.
    if let Some((_, error)) = &tree.state.error {
        egui::Frame::NONE
            .fill(theme::color(theme::mix(
                p.warning,
                p.panel_background,
                0.82,
            )))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("Showing the last version that parsed. {error}"))
                        .color(theme::color(p.text)),
                );
            });
    }
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
                    let selected = tree.state.selected.as_ref() == Some(&row.path);
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
    match command {
        Some(Command::Toggle(path)) => {
            if !tree.expanded.remove(&path) {
                tree.expanded.insert(path.clone());
            }
            tree.state.selected = Some(path);
            tree.state.rows = None;
        }
        Some(Command::Select(path)) => tree.state.selected = Some(path),
        Some(Command::ExpandAll) => {
            all_paths(&value, &mut Vec::new(), &mut tree.expanded);
            tree.state.rows = None;
        }
        Some(Command::CollapseAll) => {
            tree.expanded.clear();
            tree.state.rows = None;
        }
        None => {}
    }
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
    let x = rect.left() + bedtime_ui::tree::INSET + row.depth as f32 * bedtime_ui::tree::INDENT;
    if item.children().is_some() {
        let openness = bedtime_ui::animate_expansion(
            ui.ctx(),
            response.id.with("arrow"),
            open,
            context.motion,
        );
        bedtime_ui::paint_chevron(
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
