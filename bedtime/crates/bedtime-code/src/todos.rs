//! TODOs: the TODO, FIXME, HACK, XXX and BUG comments in the workspace's
//! files, as a sortable list or as a tree of folders and files. Clicking one
//! opens its file there.
//!
//! The workspace (`Root`) is scanned on a worker (`scan`) when the panel
//! opens, when the root changes, after a document is saved, and when asked
//! (the refresh button). Unsaved edits aren't seen until they're saved.
use bedtime_ui::{UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentSaveFinished, ManagedTask, ManagedWork, Notification, Panel, PathOptions, Placement,
    Root, WorkbenchWorldExt,
};
use bevy::{ecs::message::MessageCursor, prelude::*};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub mod scan;
mod view;

pub use scan::{Found, Scan, TodoItem};

/// Open TODOs: a new one, in the focused area.
pub struct ShowTodos;
impl bedtime_workbench::Action for ShowTodos {
    const KEY: &'static str = "code.show_todos";
    const LABEL: &'static str = "TODOs";
}

/// Installed by `CodePlugin`.
pub(crate) fn install(app: &mut App) {
    use bedtime_workbench::{ActionAppExt, MenusAppExt};
    app.register_type::<TodoPanel>()
        .register_type::<TodoLayout>()
        .register_type::<TodoSort>()
        .add_action_handler::<ShowTodos>(bedtime_workbench::Handler::new(
            bedtime_workbench::open_new::<TodoPanel>,
        ))
        .add_menu_item::<ShowTodos>(bedtime_workbench::StandardMenu::View, "panels")
        .add_systems(Update, (notice_saves, start_scans, collect_scans).chain());
}

/// How the items are shown.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum TodoLayout {
    /// Under their folders and files, as Git shows changes.
    #[default]
    Tree,
    /// One list, sorted.
    List,
}

/// What the list is sorted by (in the tree, a file's items).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum TodoSort {
    /// Path, then line.
    #[default]
    File,
    /// `scan::TAGS`' order (FIXME first), then file.
    Tag,
    /// The comment's text.
    Text,
}

impl TodoSort {
    pub const ALL: [Self; 3] = [Self::File, Self::Tag, Self::Text];
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Tag => "Tag",
            Self::Text => "Text",
        }
    }
}

/// The TODOs panel. How it shows the items is saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(TodoState)]
pub struct TodoPanel {
    #[reflect(default)]
    pub layout: TodoLayout,
    #[reflect(default)]
    pub sort: TodoSort,
    #[reflect(default)]
    pub descending: bool,
    /// Tags not shown.
    #[reflect(default)]
    pub hidden_tags: Vec<String>,
}

/// A panel's scan and what's selected and collapsed. Not saved.
#[derive(Component, Default)]
pub struct TodoState {
    task: Option<ManagedTask<Scan>>,
    pub(crate) scan: Option<Arc<Scan>>,
    error: Option<String>,
    /// Scan again (the refresh button).
    asked: bool,
    /// A document was saved then: scan again once things settle.
    saved: Option<Instant>,
    /// Collapsed folders and files, by path.
    pub(crate) collapsed: HashSet<String>,
    /// The selected item: its file and line.
    pub(crate) selected: Option<(String, u32)>,
    pub(crate) filter: String,
    /// An item to scroll to.
    pub(crate) scroll_to: Option<(String, u32)>,
}

impl TodoState {
    /// Scan again soon.
    pub fn refresh(&mut self) {
        self.asked = true;
    }
}

/// How long after a save to scan again: saving several files at once scans
/// once.
const SETTLE: Duration = Duration::from_millis(400);

/// Note saves, so panels scan again.
fn notice_saves(world: &mut World, mut saves: Local<MessageCursor<DocumentSaveFinished>>) {
    let saved = saves
        .read(world.resource::<Messages<DocumentSaveFinished>>())
        .any(|save| save.result.is_ok());
    if !saved {
        return;
    }
    let now = Instant::now();
    for mut state in world.query::<&mut TodoState>().iter_mut(world) {
        state.saved = Some(now);
    }
}

fn start_scans(world: &mut World) {
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let now = Instant::now();
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<TodoPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let state = world.get::<TodoState>(panel).unwrap();
        if state.task.is_some() {
            continue;
        }
        let first = state.scan.is_none() && state.error.is_none();
        let moved = state
            .scan
            .as_ref()
            .is_some_and(|scan| Some(&scan.root) != root.as_ref());
        let saved = state.saved.is_some_and(|at| now - at >= SETTLE);
        if !(first || moved || saved || state.asked) {
            continue;
        }
        start(world, panel, root.clone());
    }
}

fn start(world: &mut World, panel: Entity, root: Option<PathBuf>) {
    let mut state = world.get_mut::<TodoState>(panel).unwrap();
    state.asked = false;
    state.saved = None;
    let Some(root) = root else {
        state.error = Some("No folder is open".into());
        return;
    };
    let spawned = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Finding TODOs",
        move |cancel| scan::scan(&root, &|| cancel.is_cancelled()),
    );
    let mut state = world.get_mut::<TodoState>(panel).unwrap();
    match spawned {
        Ok(task) => state.task = Some(task),
        Err(error) => state.error = Some(error),
    }
}

fn collect_scans(mut states: Query<&mut TodoState>) {
    for mut state in &mut states {
        let Some(task) = &state.task else {
            continue;
        };
        let result = match task.try_recv() {
            Ok(scan) => Ok(scan),
            Err(mpsc::TryRecvError::Empty) => continue,
            Err(mpsc::TryRecvError::Disconnected) => Err("Finding TODOs stopped".to_owned()),
        };
        state.task = None;
        match result {
            Ok(scan) if scan.cancelled => {}
            Ok(scan) => {
                state.error = None;
                state.scan = Some(Arc::new(scan));
            }
            Err(error) => state.error = Some(error),
        }
    }
}

/// A row shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A folder (or a chain of folders with nothing else in them, as
    /// `a/b`): its path and name.
    Folder {
        path: String,
        label: String,
        depth: usize,
    },
    /// A file: its path, name and how many items it has.
    File {
        path: String,
        label: String,
        depth: usize,
        count: usize,
    },
    /// An item, by index in the scan.
    Item { index: usize, depth: usize },
}

impl Row {
    pub(crate) fn depth(&self) -> usize {
        match self {
            Self::Folder { depth, .. } | Self::File { depth, .. } | Self::Item { depth, .. } => {
                *depth
            }
        }
    }
}

/// Whether `item` is shown: its tag isn't hidden, and it has the filter's
/// text (in its comment, tag, author or path).
fn shown(item: &TodoItem, hidden: &[String], filter: &str) -> bool {
    if hidden.iter().any(|tag| tag == item.found.tag) {
        return false;
    }
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || item.found.text.to_lowercase().contains(&filter)
        || item.found.tag.to_lowercase().contains(&filter)
        || item.relative.to_lowercase().contains(&filter)
        || item
            .found
            .author
            .as_ref()
            .is_some_and(|author| author.to_lowercase().contains(&filter))
}

fn tag_rank(tag: &str) -> usize {
    scan::TAGS
        .iter()
        .position(|t| *t == tag)
        .unwrap_or(usize::MAX)
}

/// `indices` (into `items`) in `sort` order.
fn sort(items: &[TodoItem], indices: &mut [usize], sort: TodoSort, descending: bool) {
    let by_file = |&a: &usize, &b: &usize| {
        let (a, b) = (&items[a], &items[b]);
        (&a.relative, a.found.line).cmp(&(&b.relative, b.found.line))
    };
    match sort {
        TodoSort::File => indices.sort_by(by_file),
        TodoSort::Tag => indices.sort_by(|a, b| {
            tag_rank(items[*a].found.tag)
                .cmp(&tag_rank(items[*b].found.tag))
                .then_with(|| by_file(a, b))
        }),
        TodoSort::Text => indices.sort_by(|a, b| {
            let key = |i: usize| items[i].found.text.to_lowercase();
            key(*a).cmp(&key(*b)).then_with(|| by_file(a, b))
        }),
    }
    if descending {
        indices.reverse();
    }
}

/// The rows to show of `scan`, before collapsing (`visible` does that).
pub(crate) fn arrange(panel: &TodoPanel, scan: &Scan, filter: &str) -> Vec<Row> {
    let mut indices: Vec<usize> = (0..scan.items.len())
        .filter(|&i| shown(&scan.items[i], &panel.hidden_tags, filter))
        .collect();
    sort(&scan.items, &mut indices, panel.sort, panel.descending);
    match panel.layout {
        TodoLayout::List => indices
            .into_iter()
            .map(|index| Row::Item { index, depth: 0 })
            .collect(),
        TodoLayout::Tree => tree(
            scan,
            &indices,
            panel.descending && panel.sort == TodoSort::File,
        ),
    }
}

#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    /// Each file's name, path and items.
    files: BTreeMap<&'a str, (&'a str, Vec<usize>)>,
}

/// Folders first, then files, by name (backwards if `reversed`); each
/// file's items in `indices`' order.
fn tree(scan: &Scan, indices: &[usize], reversed: bool) -> Vec<Row> {
    let mut top = Folder::default();
    for &index in indices {
        let relative = scan.items[index].relative.as_str();
        let mut folder = &mut top;
        let mut names: Vec<&str> = relative.split('/').collect();
        let name = names.pop().unwrap_or(relative);
        for part in names {
            folder = folder.folders.entry(part).or_default();
        }
        folder
            .files
            .entry(name)
            .or_insert_with(|| (relative, Vec::new()))
            .1
            .push(index);
    }
    let mut rows = Vec::new();
    flatten(&top, "", 0, reversed, &mut rows);
    rows
}

fn flatten(folder: &Folder, prefix: &str, depth: usize, reversed: bool, rows: &mut Vec<Row>) {
    let mut folders: Vec<_> = folder.folders.iter().collect();
    let mut files: Vec<_> = folder.files.iter().collect();
    if reversed {
        folders.reverse();
        files.reverse();
    }
    for (name, mut inner) in folders {
        // A folder holding only one folder shows as one row: `a/b`.
        let mut label = (*name).to_owned();
        while inner.files.is_empty() && inner.folders.len() == 1 {
            let (next, deeper) = inner.folders.iter().next().unwrap();
            label = format!("{label}/{next}");
            inner = deeper;
        }
        let path = if prefix.is_empty() {
            label.clone()
        } else {
            format!("{prefix}/{label}")
        };
        rows.push(Row::Folder {
            path: path.clone(),
            label,
            depth,
        });
        flatten(inner, &path, depth + 1, reversed, rows);
    }
    for (name, (path, items)) in files {
        rows.push(Row::File {
            path: (*path).to_owned(),
            label: (*name).to_owned(),
            depth,
            count: items.len(),
        });
        rows.extend(items.iter().map(|&index| Row::Item {
            index,
            depth: depth + 1,
        }));
    }
}

/// `rows` without those inside collapsed folders and files.
pub(crate) fn visible(rows: Vec<Row>, collapsed: &HashSet<String>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    // The depth below which rows are inside a collapsed row.
    let mut closed_at: Option<usize> = None;
    for row in rows {
        if closed_at.is_some_and(|depth| row.depth() > depth) {
            continue;
        }
        closed_at = None;
        if let Row::Folder { path, depth, .. } | Row::File { path, depth, .. } = &row
            && collapsed.contains(path)
        {
            closed_at = Some(*depth);
        }
        out.push(row);
    }
    out
}

/// Open `item`'s file with its tag selected, somewhere other than `panel`.
pub(crate) fn open(world: &mut World, panel: Entity, item: &TodoItem, keep_focus: bool) {
    let found = &item.found;
    let end = found.column + found.tag.chars().count() as u32;
    let options = PathOptions {
        line: Some((found.line, found.column)),
        select_to: Some((found.line, end)),
        preview: true,
        keep_focus,
        place: Some(Placement::From(panel)),
        ..default()
    };
    if let Err(error) = world.workbench_mut().open_path(&item.path, options) {
        world.workbench_mut().notify(Notification::error(format!(
            "Couldn't open {}: {error}",
            item.relative
        )));
    }
}

impl Panel for TodoPanel {
    const LABEL: &'static str = "TODOs";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut shown) = world.entity_mut(panel).take::<Self>() else {
            return;
        };
        let Some(mut state) = world.entity_mut(panel).take::<TodoState>() else {
            world.entity_mut(panel).insert(shown);
            return;
        };
        let intents = view::draw(ui, world, context, panel, &mut shown, &mut state);
        world.entity_mut(panel).insert((shown, state));
        view::apply(world, panel, intents);
    }
}

#[cfg(test)]
mod tests;
