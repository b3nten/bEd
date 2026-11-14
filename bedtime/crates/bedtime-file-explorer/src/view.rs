//! Drawing the explorer: the tree (here) or the columns (`columns.rs`), and
//! the footer under them. Drawing only reads; what the user asks for comes
//! back as intents, applied after.
use crate::{
    ExplorerView, FileExplorer, FileExplorerDefaults, Folders, Row, Showing, TreeFilter, ViewMode,
    refresh_columns, refresh_rows,
};
use bedtime_filesystem::{EntryKind, operations::Operation};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::color, tree};
use bedtime_workbench::{
    Notification, Panel, PanelPresentation, PathOptions, ViewerId, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

impl Panel for FileExplorer {
    const LABEL: &'static str = "Files";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw(ui, world, context, panel);
    }
}

/// Show hidden files in the focused tree, or stop.
pub struct ToggleHiddenFiles;
impl bedtime_workbench::Action for ToggleHiddenFiles {
    const KEY: &'static str = "files.toggle_hidden";
    const LABEL: &'static str = "Show Hidden Files";
}

/// Show files Git ignores in the focused tree, or stop.
pub struct ToggleIgnoredFiles;
impl bedtime_workbench::Action for ToggleIgnoredFiles {
    const KEY: &'static str = "files.toggle_ignored";
    const LABEL: &'static str = "Show Ignored Files";
}

/// Show the focused explorer's files as a tree.
pub struct ViewFilesAsTree;
impl bedtime_workbench::Action for ViewFilesAsTree {
    const KEY: &'static str = "files.view_tree";
    const LABEL: &'static str = "View Files as Tree";
}

/// Show the focused explorer's files in columns.
pub struct ViewFilesAsColumns;
impl bedtime_workbench::Action for ViewFilesAsColumns {
    const KEY: &'static str = "files.view_columns";
    const LABEL: &'static str = "View Files as Columns";
}

/// Show the focused explorer's files as a tree when it's tall, in columns
/// when it's wide.
pub struct ViewFilesAutomatically;
impl bedtime_workbench::Action for ViewFilesAutomatically {
    const KEY: &'static str = "files.view_auto";
    const LABEL: &'static str = "View Files by Panel Shape";
}

pub(crate) fn install_actions(app: &mut App) {
    use bedtime_workbench::{ActionAppExt, Handler};
    let tree = |world: &World, panel: Option<Entity>| {
        panel.is_some_and(|panel| world.get::<FileExplorer>(panel).is_some())
    };
    app.add_action_handler::<ToggleHiddenFiles>(
        Handler::new(|world, invocation| toggle(world, invocation.entity, Intent::ToggleHidden))
            .available_when(tree),
    )
    .add_action_handler::<ToggleIgnoredFiles>(
        Handler::new(|world, invocation| toggle(world, invocation.entity, Intent::ToggleIgnored))
            .available_when(tree),
    )
    .add_action_handler::<ViewFilesAsTree>(
        Handler::new(|world, invocation| {
            toggle(world, invocation.entity, Intent::SetMode(ViewMode::Tree))
        })
        .available_when(tree),
    )
    .add_action_handler::<ViewFilesAsColumns>(
        Handler::new(|world, invocation| {
            toggle(world, invocation.entity, Intent::SetMode(ViewMode::Columns))
        })
        .available_when(tree),
    )
    .add_action_handler::<ViewFilesAutomatically>(
        Handler::new(|world, invocation| {
            toggle(world, invocation.entity, Intent::SetMode(ViewMode::Auto))
        })
        .available_when(tree),
    );
}

fn toggle(
    world: &mut World,
    panel: Option<Entity>,
    intent: Intent,
) -> bedtime_workbench::ActionResult {
    let panel = panel
        .filter(|&panel| world.get::<FileExplorer>(panel).is_some())
        .ok_or("No file explorer is focused")?;
    apply(world, panel, Path::new(""), intent);
    Ok(())
}

/// What the user asked for.
#[derive(Debug)]
pub(crate) enum Intent {
    Toggle(PathBuf),
    Open {
        path: PathBuf,
        preview: bool,
        viewer: Option<ViewerId>,
    },
    Refresh,
    ToggleHidden,
    ToggleIgnored,
    Operation(Operation),
    Copy {
        paths: Vec<PathBuf>,
        cut: bool,
    },
    Paste(PathBuf),
    /// Select this, opening the folders above it in the tree.
    Reveal(PathBuf),
    SetMode(ViewMode),
}

/// A name being typed: for a new file or folder (in a folder), or a rename.
#[derive(Clone, Debug)]
pub(crate) struct Naming {
    kind: NameKind,
    text: String,
    /// Selected the name (its stem, for a rename) on the first frame.
    started: bool,
}

#[derive(Clone, Debug)]
enum NameKind {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
}

impl Naming {
    fn new(kind: NameKind) -> Self {
        let text = match &kind {
            NameKind::Rename(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            _ => String::new(),
        };
        Self {
            kind,
            text,
            started: false,
        }
    }

    /// The row it's typed at: the renamed one, or the folder it goes in.
    pub(crate) fn row(&self) -> &Path {
        match &self.kind {
            NameKind::NewFile(folder) | NameKind::NewFolder(folder) => folder,
            NameKind::Rename(path) => path,
        }
    }

    /// Something new goes in `row()` (rather than renaming it).
    pub(crate) fn goes_in(&self) -> bool {
        !matches!(self.kind, NameKind::Rename(_))
    }

    fn operation(&self) -> Option<Operation> {
        let name = self.text.trim();
        if name.is_empty() {
            return None;
        }
        let name = std::ffi::OsString::from(name);
        Some(match &self.kind {
            NameKind::NewFile(directory) => Operation::CreateFile {
                directory: directory.clone(),
                name,
            },
            NameKind::NewFolder(directory) => Operation::CreateDirectory {
                directory: directory.clone(),
                name,
            },
            NameKind::Rename(path) => Operation::Rename {
                path: path.clone(),
                name,
            },
        })
    }
}

/// What a drag from the tree carries, besides opening a file where it's
/// dropped on the layout.
pub(crate) struct TreeDrag {
    pub(crate) paths: Vec<PathBuf>,
}

/// The folder something goes in, from the row it's at: the folder itself,
/// or a file's folder.
pub(crate) fn folder_of(row: &Row) -> PathBuf {
    if row.kind == EntryKind::Directory {
        row.path.clone()
    } else {
        row.path
            .parent()
            .map_or_else(|| row.path.clone(), Path::to_owned)
    }
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let root = match world.resource::<Folders>().root() {
        Some(Ok(root)) => root.to_owned(),
        problem => {
            egui::Frame::NONE
                .inner_margin(14)
                .show(ui, |ui| match problem {
                    Some(Err(error)) => ui.label(format!("The folder can't be shown: {error}")),
                    _ => ui.weak("No folder is open."),
                });
            return;
        }
    };
    let Some(mut view) = world.entity_mut(panel).take::<ExplorerView>() else {
        return;
    };
    let defaults = world
        .get_resource::<FileExplorerDefaults>()
        .copied()
        .unwrap_or_default();
    let (mode, filter) = world
        .get::<FileExplorer>(panel)
        .map_or((ViewMode::Tree, TreeFilter::ALL), |explorer| {
            (explorer.mode, explorer.filter(&defaults))
        });
    let mut intents = Vec::new();

    // The footer at the bottom; the tree or the columns above it.
    let whole = ui.available_rect_before_wrap();
    let footer_height = tree::row_height(context) + 6.0;
    let (body, footer_rect) = whole.split_top_bottom_at_y(whole.bottom() - footer_height);
    let showing = mode.showing(Vec2::new(whole.width(), whole.height()));
    if view.showing.is_some_and(|before| before != showing)
        && let Some(selected) = view.selected.clone()
    {
        // One selection for both views: the tree opens the folders down to
        // it; the columns follow it anyway.
        if showing == Showing::Tree {
            intents.push(Intent::Reveal(selected));
        } else {
            view.reveal = Some(selected);
        }
    }
    view.showing = Some(showing);
    match showing {
        Showing::Tree => {
            if let Some(tree) = world.get::<FileExplorer>(panel) {
                refresh_rows(&root, tree, world.resource::<Folders>(), filter, &mut view);
            }
        }
        Showing::Columns => refresh_columns(&root, world.resource::<Folders>(), filter, &mut view),
    }
    let ctx = ui.ctx().clone();
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let keys = focused
        && ui.is_enabled()
        && !ctx.text_edit_focused()
        && !ctx.any_popup_open()
        && ctx.memory(|memory| memory.top_modal_layer().is_none());

    let mut body_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("body")
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    body_ui.set_clip_rect(body.intersect(ui.clip_rect()));
    if let Some(error) = world.resource::<Folders>().watch_error() {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(&mut body_ui, |ui| {
                ui.weak("Not watching for changes")
                    .on_hover_text(error.to_owned());
            });
    }
    let shown = Shown {
        panel,
        root: &root,
        filter,
        mode,
        keys,
    };
    match showing {
        Showing::Tree => draw_tree(
            &mut body_ui,
            world,
            context,
            &shown,
            &mut view,
            &mut intents,
        ),
        Showing::Columns => crate::columns::draw(
            &mut body_ui,
            world,
            context,
            &shown,
            &mut view,
            &mut intents,
        ),
    }
    footer(
        ui,
        context,
        footer_rect,
        &shown,
        showing,
        &mut view,
        &mut intents,
    );
    ui.advance_cursor_after_rect(whole);
    world.entity_mut(panel).insert(view);
    // Applied in `Update`: opening can close panels the layout is drawing.
    world
        .resource_mut::<Intents>()
        .0
        .extend(intents.into_iter().map(|intent| (panel, intent)));
}

/// What both views draw from.
pub(crate) struct Shown<'a> {
    pub(crate) panel: Entity,
    pub(crate) root: &'a Path,
    pub(crate) filter: TreeFilter,
    pub(crate) mode: ViewMode,
    /// The panel takes keys.
    pub(crate) keys: bool,
}

/// The selected row, in what's showing.
pub(crate) fn selected_row(view: &ExplorerView) -> Option<Row> {
    let selected = view.selected.as_ref()?;
    match view.showing {
        Some(Showing::Columns) => view
            .columns
            .iter()
            .flat_map(|column| &column.rows)
            .find(|row| &row.path == selected)
            .cloned(),
        _ => view.rows.iter().find(|row| &row.path == selected).cloned(),
    }
}

fn draw_tree(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    shown: &Shown<'_>,
    view: &mut ExplorerView,
    intents: &mut Vec<Intent>,
) {
    let Shown {
        panel,
        root,
        filter,
        mode,
        ..
    } = *shown;
    let ctx = ui.ctx().clone();
    if shown.keys {
        keyboard(ui, view, tree::row_height(context), intents);
        editing_keys(ui, view, root, intents);
    }

    let height = tree::row_height(context);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt((panel, "files"))
        .auto_shrink([false, false]);
    // The row to reveal may not be there yet (its folder still being read).
    let reveal = view
        .reveal
        .as_ref()
        .and_then(|path| view.rows.iter().position(|row| &row.path == path));
    if reveal.is_some() || !view.rows.iter().any(|row| row.loading) {
        view.reveal = None;
    }
    if let Some(index) = reveal {
        let id = egui::Id::new((panel, "files-scroll"));
        let (offset, visible) =
            ctx.data(|data| data.get_temp::<(f32, f32)>(id).unwrap_or_default());
        let top = index as f32 * height;
        if top < offset {
            scroll = scroll.vertical_scroll_offset(top);
        } else if top + height > offset + visible {
            scroll = scroll.vertical_scroll_offset(top + height - visible);
        }
    }
    let rows = view.rows.clone();
    // The empty space (under rows, which take clicks first): a menu for the root.
    let whole = ui.available_rect_before_wrap();
    let background = ui.interact(
        whole,
        egui::Id::new((panel, "background")),
        egui::Sense::click(),
    );
    background.context_menu(|ui| {
        folder_menu(ui, root, &mut view.naming, intents);
        filter_menu(ui, filter, mode, intents);
    });
    let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();
    let mut item_rects: Vec<(usize, egui::Rect)> = Vec::new();
    ui.spacing_mut().item_spacing.y = 0.0;
    let now = ui.input(|i| i.time);
    view.motions.update(&rows, height, now, context.motion);
    let output = scroll.show_viewport(ui, |ui, viewport| {
        let width = ui.available_width();
        ui.set_min_height(rows.len() as f32 * height);
        ui.set_min_width(width);
        let top = ui.max_rect().top();
        let left = ui.max_rect().left();
        // The rows in view, and any moving into or out of it.
        let first = ((viewport.min.y / height).floor().max(0.0) as usize).saturating_sub(1);
        let last = ((viewport.max.y / height).ceil().max(0.0) as usize + 1).min(rows.len());
        let mut shown: Vec<usize> = (first.min(last)..last).collect();
        shown.extend(
            view.motions
                .moving(&rows)
                .filter(|&i| i < first || i >= last),
        );
        // Rows on their way out, under the rest.
        for (row, y, alpha, floor) in view.motions.leaving(now, context.motion) {
            let rect =
                egui::Rect::from_min_size(egui::pos2(left, top + y), egui::vec2(width, height));
            if rect.intersects(ui.clip_rect()) {
                let item = tree::item_rect(
                    ui,
                    context,
                    &tree_row(context, row, view),
                    rect,
                    tree::INDENT,
                );
                let response = ui.interact(
                    item,
                    egui::Id::new((panel, "leaving", &row.path)),
                    egui::Sense::hover(),
                );
                below(ui, floor.map(|f| top + f), |ui| {
                    paint(ui, context, row, view, rect, &response, alpha)
                });
            }
        }
        for index in shown {
            let row = &rows[index];
            let (y, alpha, floor) = view.motions.sample(row, index, height, now, context.motion);
            let rect =
                egui::Rect::from_min_size(egui::pos2(left, top + y), egui::vec2(width, height));
            if !rect.intersects(ui.clip_rect()) {
                continue;
            }
            // Only the item takes the pointer, not the empty space after it.
            let item = tree::item_rect(
                ui,
                context,
                &tree_row(context, row, view),
                rect,
                tree::INDENT,
            );
            let response = ui.interact(
                item,
                egui::Id::new((panel, &row.path)),
                egui::Sense::click_and_drag(),
            );
            row_rects.push((index, rect));
            item_rects.push((index, item));
            // Drag onto a folder in the tree to move it there; onto the layout
            // to open a file there (an area, beside a tab, a new split).
            if response.drag_started() && row.depth > 0 {
                let opens = row.kind != EntryKind::Directory
                    && !world.workbench().viewers_for(&row.path).is_empty();
                let factory = if opens {
                    open_factory(row.path.clone())
                } else {
                    bedtime_workbench::PanelFactory::new("File", |_, _| Entity::PLACEHOLDER)
                };
                response.dnd_set_drag_payload(
                    bedtime_workbench::PanelFactoryPayload::with_content(
                        factory,
                        TreeDrag {
                            paths: vec![row.path.clone()],
                        },
                    ),
                );
            }
            let selected = view.selected.as_ref() == Some(&row.path);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    &row.name,
                )
            });
            below(ui, floor.map(|f| top + f), |ui| {
                paint(ui, context, row, view, rect, &response, alpha)
            });
            if tree::rested(ui.ctx(), &response) {
                let details = details(world.resource::<Folders>(), view, &row.path);
                context.frame(ui).tooltip_ui(&response, |frame| {
                    describe(frame, context, row, details.as_ref())
                });
            }
            // egui counts a click on another row just before as part of a
            // triple click.
            if response.double_clicked() || response.triple_clicked() {
                if row.kind != EntryKind::Directory {
                    intents.push(Intent::Open {
                        path: row.path.clone(),
                        preview: false,
                        viewer: None,
                    });
                }
            } else if response.clicked() {
                view.selected = Some(row.path.clone());
                intents.push(if row.kind == EntryKind::Directory {
                    Intent::Toggle(row.path.clone())
                } else {
                    Intent::Open {
                        path: row.path.clone(),
                        preview: true,
                        viewer: None,
                    }
                });
            }
            if response.secondary_clicked() {
                view.selected = Some(row.path.clone());
                let viewers = if row.kind == EntryKind::Directory {
                    Vec::new()
                } else {
                    world.workbench().viewers_for(&row.path)
                };
                view.menu = Some((row.path.clone(), viewers));
            }
            response.context_menu(|ui| {
                let viewers = view
                    .menu
                    .as_ref()
                    .filter(|(path, _)| path == &row.path)
                    .map(|(_, viewers)| viewers.clone())
                    .unwrap_or_default();
                menu(ui, row, &viewers, &mut view.naming, intents);
                filter_menu(ui, filter, mode, intents);
            });
        }
    });
    if view.motions.settle(now, context.motion) {
        ui.ctx().request_repaint();
    }
    let items: Vec<(&Row, egui::Rect)> = item_rects
        .iter()
        .map(|&(index, rect)| (&rows[index], rect))
        .collect();
    drop_into(ui, &ctx, &items, &[], whole, root, context, intents);
    if let Some(naming) = &view.naming {
        let anchor = row_rects
            .iter()
            .find(|(index, _)| rows[*index].path == naming.row())
            .map(|(_, rect)| *rect);
        draw_naming(&ctx, panel, view, anchor, whole, intents);
    }
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new((panel, "files-scroll")),
            (output.state.offset.y, output.inner_rect.height()),
        )
    });
}

/// What trees were asked for while drawing.
#[derive(Resource, Default)]
pub(crate) struct Intents(Vec<(Entity, Intent)>);

pub(crate) fn apply_intents(world: &mut World) {
    let intents = std::mem::take(&mut world.resource_mut::<Intents>().0);
    let Some(Ok(root)) = world
        .resource::<Folders>()
        .root()
        .map(|r| r.map(Path::to_owned))
    else {
        return;
    };
    for (panel, intent) in intents {
        if world.get_entity(panel).is_ok() {
            apply(world, panel, &root, intent);
        }
    }
}

/// Draw with what's above `floor` hidden: a row coming out of (or going
/// into) its folder's row passes under it.
fn below(ui: &mut egui::Ui, floor: Option<f32>, draw: impl FnOnce(&mut egui::Ui)) {
    let Some(floor) = floor else {
        return draw(ui);
    };
    let mut clip = ui.clip_rect();
    clip.min.y = clip.min.y.max(floor);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()));
    child.set_clip_rect(clip);
    draw(&mut child);
}

/// How `row` looks in the tree.
fn tree_row<'a>(context: &UiContext<'_>, row: &'a Row, view: &ExplorerView) -> tree::Row<'a> {
    let palette = &context.theme.palette;
    let icon = match row.kind {
        EntryKind::Directory if row.expanded => BuiltinIcon::FolderOpen,
        EntryKind::Directory => BuiltinIcon::Folder,
        EntryKind::Symlink => BuiltinIcon::Extension,
        _ => file_icon(&row.path),
    };
    let ink = if row.error.is_some() {
        palette.error
    } else if row.hidden || row.ignored {
        palette.muted
    } else {
        palette.text
    };
    tree::Row {
        label: Some(&row.name),
        depth: row.depth,
        icon: Some(icon),
        expanded: (row.kind == EntryKind::Directory).then_some(row.expanded),
        selected: view.selected.as_ref() == Some(&row.path),
        ink: color(ink),
        icon_ink: palette.text,
    }
}

fn paint(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    row: &Row,
    view: &ExplorerView,
    rect: egui::Rect,
    response: &egui::Response,
    alpha: f32,
) {
    tree::paint_row(
        ui,
        context,
        tree_row(context, row, view),
        rect,
        Some(response),
        alpha * if row.loading { 0.6 } else { 1.0 },
        tree::INDENT,
    );
}

/// The hovered entry's metadata, read once while its tooltip shows.
pub(crate) fn details(
    folders: &Folders,
    view: &mut ExplorerView,
    path: &Path,
) -> Option<bedtime_filesystem::Metadata> {
    if view.details.as_ref().is_none_or(|(at, _)| at != path) {
        let metadata = folders.filesystem.source(path).entry_metadata().ok();
        view.details = Some((path.to_owned(), metadata));
    }
    view.details
        .as_ref()
        .and_then(|(_, metadata)| metadata.clone())
}

/// A row's tooltip: what it is, its size, when it changed, and why it's
/// dimmed or red.
pub(crate) fn describe(
    frame: bedtime_ui::UiFrame<'_>,
    context: &UiContext<'_>,
    row: &Row,
    metadata: Option<&bedtime_filesystem::Metadata>,
) {
    let palette = &context.theme.palette;
    let ui = frame.ui;
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.label(egui::RichText::new(&row.name).strong());
    let mut kind = match row.kind {
        EntryKind::Directory => "Folder".to_owned(),
        EntryKind::Symlink => "Symbolic link".to_owned(),
        _ => file_kind(&row.path),
    };
    if let Some(metadata) = metadata.filter(|m| m.kind == EntryKind::File) {
        kind = format!("{kind} · {}", size(metadata.len));
    }
    ui.label(kind);
    let muted = color(palette.muted);
    if let Some(modified) = metadata.and_then(|m| m.modified) {
        ui.colored_label(muted, format!("Modified {}", ago(modified)));
    }
    if row.hidden {
        ui.colored_label(muted, "Hidden");
    }
    if row.ignored {
        ui.colored_label(muted, "Ignored by Git");
    }
    if let Some(error) = &row.error {
        ui.colored_label(color(palette.error), error);
    }
}

/// "1.2 MB": decimal units, as Finder and Files show them.
fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    let mut value = bytes as f64;
    for unit in ["KB", "MB", "GB", "TB"] {
        value /= 1000.0;
        if value < 1000.0 || unit == "TB" {
            return if value < 10.0 {
                format!("{value:.1} {unit}")
            } else {
                format!("{value:.0} {unit}")
            };
        }
    }
    unreachable!()
}

/// "3 minutes ago", "yesterday", "2 years ago".
fn ago(time: std::time::SystemTime) -> String {
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(time) else {
        return "just now".into();
    };
    let seconds = elapsed.as_secs();
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match seconds {
        0..60 => "just now".into(),
        60..3600 => plural(seconds / 60, "minute"),
        3600..86400 => plural(seconds / 3600, "hour"),
        86400..172800 => "yesterday".into(),
        172800..2592000 => plural(seconds / 86400, "day"),
        2592000..31536000 => plural(seconds / 2592000, "month"),
        _ => plural(seconds / 31536000, "year"),
    }
}

/// What kind of file `path` is, by its extension ("PNG image"). Does no I/O.
pub fn file_kind(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();
    let of = |kind: &str| format!("{extension} {kind}");
    match file_icon(path) {
        BuiltinIcon::Rust => "Rust source".into(),
        BuiltinIcon::Python => "Python source".into(),
        BuiltinIcon::Javascript => "JavaScript".into(),
        BuiltinIcon::Typescript => "TypeScript".into(),
        BuiltinIcon::Json => "JSON".into(),
        BuiltinIcon::Markdown => "Markdown".into(),
        BuiltinIcon::Bash => "Shell script".into(),
        BuiltinIcon::Text => "Plain text".into(),
        BuiltinIcon::Config => of("configuration"),
        BuiltinIcon::Image => of("image"),
        BuiltinIcon::Audio => of("audio"),
        BuiltinIcon::Video => of("video"),
        BuiltinIcon::Font => of("font"),
        BuiltinIcon::Spreadsheet => of("table"),
        BuiltinIcon::Box => of("3D model"),
        _ if extension.is_empty() => "File".into(),
        _ => of("file"),
    }
}

/// The context menu of `row`.
pub(crate) fn menu(
    ui: &mut egui::Ui,
    row: &Row,
    viewers: &[(ViewerId, &'static str)],
    naming: &mut Option<Naming>,
    intents: &mut Vec<Intent>,
) {
    let root = row.depth == 0;
    if row.kind != EntryKind::Directory {
        if ui.button("Open").clicked() {
            intents.push(Intent::Open {
                path: row.path.clone(),
                preview: false,
                viewer: None,
            });
        }
        if !viewers.is_empty() {
            ui.menu_button("Open With", |ui| {
                for &(viewer, label) in viewers {
                    if ui.button(label).clicked() {
                        intents.push(Intent::Open {
                            path: row.path.clone(),
                            preview: false,
                            viewer: Some(viewer),
                        });
                    }
                }
            });
        }
        ui.separator();
    }
    let folder = folder_of(row);
    if ui.button("New File…").clicked() {
        *naming = Some(Naming::new(NameKind::NewFile(folder.clone())));
    }
    if ui.button("New Folder…").clicked() {
        *naming = Some(Naming::new(NameKind::NewFolder(folder.clone())));
    }
    ui.separator();
    if ui
        .add_enabled(!root, egui::Button::new("Rename…"))
        .clicked()
    {
        *naming = Some(Naming::new(NameKind::Rename(row.path.clone())));
    }
    if ui
        .add_enabled(!root, egui::Button::new("Duplicate"))
        .clicked()
    {
        intents.push(Intent::Operation(Operation::Duplicate {
            sources: vec![row.path.clone()],
        }));
    }
    if ui
        .add_enabled(!root, egui::Button::new("Move to Trash"))
        .clicked()
    {
        intents.push(Intent::Operation(Operation::Trash {
            sources: vec![row.path.clone()],
        }));
    }
    ui.separator();
    if ui.add_enabled(!root, egui::Button::new("Copy")).clicked() {
        intents.push(Intent::Copy {
            paths: vec![row.path.clone()],
            cut: false,
        });
    }
    if ui.add_enabled(!root, egui::Button::new("Cut")).clicked() {
        intents.push(Intent::Copy {
            paths: vec![row.path.clone()],
            cut: true,
        });
    }
    if ui.button("Paste").clicked() {
        intents.push(Intent::Paste(folder));
    }
    ui.separator();
    if ui.button("Copy Path").clicked() {
        ui.ctx().copy_text(row.path.display().to_string());
    }
    if ui.button("Refresh").clicked() {
        intents.push(Intent::Refresh);
    }
}

/// The menu of the empty space: the root folder.
pub(crate) fn folder_menu(
    ui: &mut egui::Ui,
    folder: &Path,
    naming: &mut Option<Naming>,
    intents: &mut Vec<Intent>,
) {
    if ui.button("New File…").clicked() {
        *naming = Some(Naming::new(NameKind::NewFile(folder.to_owned())));
    }
    if ui.button("New Folder…").clicked() {
        *naming = Some(Naming::new(NameKind::NewFolder(folder.to_owned())));
    }
    if ui.button("Paste").clicked() {
        intents.push(Intent::Paste(folder.to_owned()));
    }
    ui.separator();
    if ui.button("Refresh").clicked() {
        intents.push(Intent::Refresh);
    }
}

/// How files show (hidden and ignored ones, tree or columns), at the end of
/// each menu.
pub(crate) fn filter_menu(
    ui: &mut egui::Ui,
    filter: TreeFilter,
    mode: ViewMode,
    intents: &mut Vec<Intent>,
) {
    ui.separator();
    let mut hidden = filter.hidden;
    if ui.checkbox(&mut hidden, "Show Hidden Files").clicked() {
        intents.push(Intent::ToggleHidden);
        ui.close();
    }
    let mut ignored = filter.ignored;
    if ui.checkbox(&mut ignored, "Show Ignored Files").clicked() {
        intents.push(Intent::ToggleIgnored);
        ui.close();
    }
    ui.menu_button("View As", |ui| {
        for (choice, label) in MODES {
            if ui.radio(mode == choice, label).clicked() {
                intents.push(Intent::SetMode(choice));
                ui.close();
            }
        }
    });
}

/// The view modes, as the footer and menus name them.
const MODES: [(ViewMode, &str); 3] = [
    (ViewMode::Tree, "Tree"),
    (ViewMode::Columns, "Columns"),
    (ViewMode::Auto, "Auto"),
];

/// The strip under the files: the folders down to the selection's (each
/// selects itself when clicked), and the view mode.
fn footer(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    shown: &Shown<'_>,
    showing: Showing,
    view: &mut ExplorerView,
    intents: &mut Vec<Intent>,
) {
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("footer")
            .max_rect(rect.shrink2(egui::vec2(6.0, 0.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    let line = ui.painter().clone();
    let palette = &context.theme.palette;
    let small = (context.metrics.font_size - 1.0).max(10.0);

    // The mode: a segmented switch (Tree, Columns, Auto), or in a narrow
    // panel a menu of them.
    let font = egui::FontId::proportional(small);
    let width = |ui: &egui::Ui, text: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        }) + 4.0
    };
    let tip = |choice: ViewMode| match choice {
        ViewMode::Tree => "Always show a tree".to_owned(),
        ViewMode::Columns => "Always show columns".to_owned(),
        ViewMode::Auto => format!(
            "A tree when the panel is tall, columns when it's wide (now {})",
            match showing {
                Showing::Tree => "a tree",
                Showing::Columns => "columns",
            }
        ),
    };
    let segmented: f32 = MODES
        .iter()
        .map(|(_, label)| width(&ui, label) + 14.0)
        .sum();
    let switch = if rect.width() >= segmented + 80.0 {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding = egui::vec2(6.0, 1.0);
            for (choice, label) in MODES.into_iter().rev() {
                let on = shown.mode == choice;
                let response = ui
                    .selectable_label(on, egui::RichText::new(label).size(small))
                    .on_hover_text(tip(choice));
                if response.clicked() && !on {
                    intents.push(Intent::SetMode(choice));
                }
            }
        })
        .response
        .rect
    } else {
        let (_, label) = MODES
            .into_iter()
            .find(|(choice, _)| *choice == shown.mode)
            .unwrap_or(MODES[2]);
        ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
        let menu = ui.menu_button(
            egui::RichText::new(format!("{label} ▾")).size(small),
            |ui| {
                for (choice, label) in MODES {
                    let response = ui
                        .radio(shown.mode == choice, label)
                        .on_hover_text(tip(choice));
                    if response.clicked() {
                        if shown.mode != choice {
                            intents.push(Intent::SetMode(choice));
                        }
                        ui.close();
                    }
                }
            },
        );
        menu.response.on_hover_text("How files show").rect
    };
    // The breadcrumb gets what's left of the switch.
    let crumbs_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 6.0, rect.top()),
        egui::pos2((switch.left() - 8.0).max(rect.left() + 6.0), rect.bottom()),
    );

    // Where the selection is: the root's name, then each folder down to the
    // selection's (its own name shows where it's selected).
    let mut segments = vec![(
        shown.root.file_name().map_or_else(
            || shown.root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        shown.root.to_owned(),
    )];
    if let Some(relative) = view
        .selected
        .as_deref()
        .and_then(|selected| selected.parent()?.strip_prefix(shown.root).ok())
    {
        let mut at = shown.root.to_owned();
        for part in relative.components() {
            at.push(part);
            segments.push((part.as_os_str().to_string_lossy().into_owned(), at.clone()));
        }
    }
    // What doesn't fit goes, from the root down.
    let separator = width(&ui, " › ");
    let room = crumbs_rect.width();
    let mut needed: f32 = segments
        .iter()
        .map(|(name, _)| width(&ui, name) + separator)
        .sum();
    let mut skipped = false;
    while needed > room && segments.len() > 1 {
        needed -= width(&ui, &segments[0].0) + separator;
        segments.remove(0);
        skipped = true;
    }
    let muted = color(palette.muted);
    let mut crumbs = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("crumbs")
            .max_rect(crumbs_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    crumbs.set_clip_rect(crumbs_rect.intersect(ui.clip_rect()));
    crumbs.spacing_mut().item_spacing.x = 0.0;
    crumbs.spacing_mut().button_padding = egui::vec2(2.0, 1.0);
    if skipped {
        crumbs.label(egui::RichText::new("… › ").size(small).color(muted));
    }
    let last = segments.len() - 1;
    for (index, (name, path)) in segments.into_iter().enumerate() {
        let ink = if index == last {
            color(palette.text)
        } else {
            muted
        };
        let response = crumbs
            .add(egui::Button::new(egui::RichText::new(name).size(small).color(ink)).frame(false));
        if response.clicked() {
            view.selected = Some(path.clone());
            view.reveal = Some(path);
        }
        if index < last {
            crumbs.label(egui::RichText::new(" › ").size(small).color(muted));
        }
    }
    line.hline(rect.x_range(), rect.top() + 0.5, stroke);
}

/// The box a name is typed in, under `at` (or at the top). Enter does it,
/// Escape (or clicking away) doesn't.
pub(crate) fn draw_naming(
    ctx: &egui::Context,
    panel: Entity,
    view: &mut ExplorerView,
    at: Option<egui::Rect>,
    whole: egui::Rect,
    intents: &mut Vec<Intent>,
) {
    let Some(naming) = view.naming.as_mut() else {
        return;
    };
    let anchor = at.map_or(whole.left_top(), |rect| rect.left_bottom());
    let id = egui::Id::new((panel, "naming"));
    let label = match naming.kind {
        NameKind::NewFile(_) => "New file",
        NameKind::NewFolder(_) => "New folder",
        NameKind::Rename(_) => "New name",
    };
    let mut done = None;
    egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor + egui::vec2(tree::INSET, 0.0))
        .constrain_to(ctx.content_rect().shrink(4.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let output = egui::TextEdit::singleline(&mut naming.text)
                    .id(id)
                    .hint_text(label)
                    .desired_width((whole.width() - 32.0).clamp(120.0, 320.0))
                    .show(ui);
                if !naming.started {
                    naming.started = true;
                    output.response.request_focus();
                    // A rename selects the name without its extension.
                    let stem = match naming.text.rfind('.') {
                        Some(dot) if dot > 0 => naming.text[..dot].chars().count(),
                        _ => naming.text.chars().count(),
                    };
                    let mut state = output.state.clone();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(stem),
                        )));
                    state.store(ui.ctx(), id);
                }
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                if escape {
                    done = Some(None);
                } else if output.response.lost_focus() {
                    done = Some(enter.then(|| naming.operation()).flatten());
                }
            });
        });
    if let Some(operation) = done {
        view.naming = None;
        if let Some(operation) = operation {
            intents.push(Intent::Operation(operation));
        }
    }
}

/// While a drag from the explorer (or files from outside the app) is over
/// it: the folder it would go into is highlighted (an item's folder, a
/// column's folder over its empty space, else the root), and letting go
/// moves it there (Alt/Option or Cmd/Ctrl copies); files from outside are
/// copied (Alt/Option or Cmd/Ctrl moves). Dropping it on itself, where it
/// already is, or inside itself does nothing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drop_into(
    ui: &egui::Ui,
    ctx: &egui::Context,
    items: &[(&Row, egui::Rect)],
    areas: &[(&Path, egui::Rect)],
    whole: egui::Rect,
    root: &Path,
    context: &UiContext<'_>,
    intents: &mut Vec<Intent>,
) {
    // A drag within the tree moves (Alt/Cmd copies); files from outside the
    // app are copied in (Cmd moves them).
    let (paths, outside, dropped) =
        match egui::DragAndDrop::payload::<bedtime_workbench::PanelFactoryPayload>(ctx)
            .and_then(|payload| payload.content::<TreeDrag>())
        {
            Some(drag) => (drag.paths.clone(), false, false),
            None => {
                let (hovered, dropped) = ctx.input(|i| {
                    let path = |p: Option<&Path>| p.map(Path::to_owned);
                    (
                        i.raw
                            .hovered_files
                            .iter()
                            .filter_map(|file| path(file.path.as_deref()))
                            .collect::<Vec<_>>(),
                        i.raw
                            .dropped_files
                            .iter()
                            .map(|file| file.path().to_owned())
                            .collect::<Vec<_>>(),
                    )
                });
                if !dropped.is_empty() {
                    (dropped, true, true)
                } else if !hovered.is_empty() {
                    (hovered, true, false)
                } else {
                    return;
                }
            }
        };
    let Some(pointer) = ctx.pointer_latest_pos().filter(|p| whole.contains(*p)) else {
        return;
    };
    // Onto a folder's (or a file's) item; onto an area's folder; elsewhere,
    // the root.
    let folder = items
        .iter()
        .find(|(_, rect)| rect.contains(pointer))
        .map(|(row, _)| folder_of(row))
        .or_else(|| {
            areas
                .iter()
                .find(|(_, rect)| rect.contains(pointer))
                .map(|(path, _)| path.to_path_buf())
        })
        .unwrap_or_else(|| root.to_owned());
    let fits = paths
        .iter()
        .all(|path| path.parent() != Some(folder.as_path()) && !folder.starts_with(path));
    // The tree takes the drop (the layout doesn't make a panel of it).
    bedtime_workbench::PanelFactoryPayload::reserve_for_content(ctx);
    if !fits {
        return;
    }
    let highlight = items
        .iter()
        .find(|(row, _)| row.path == folder)
        .map(|(_, rect)| *rect)
        .or_else(|| {
            areas
                .iter()
                .find(|(path, _)| *path == folder)
                .map(|(_, rect)| *rect)
        })
        .unwrap_or(whole);
    let ink = color(context.theme.palette.accent);
    ui.painter().rect_stroke(
        highlight.shrink(1.0),
        4.0,
        egui::Stroke::new(1.5, ink),
        egui::StrokeKind::Inside,
    );
    let (released, modified) = ui.input(|input| {
        (
            input.pointer.primary_released(),
            input.modifiers.alt || input.modifiers.command,
        )
    });
    let copy = if outside { !modified } else { modified };
    if (outside && dropped) || (!outside && released) {
        egui::DragAndDrop::clear_payload(ctx);
        let sources = paths;
        intents.push(Intent::Operation(if copy {
            Operation::Copy {
                sources,
                directory: folder,
            }
        } else {
            Operation::Move {
                sources,
                directory: folder,
            }
        }));
    }
}

/// F2 renames; Cmd/Ctrl+Backspace (or Delete) moves to the Trash; Copy, Cut
/// and Paste work on the selected row.
pub(crate) fn editing_keys(
    ui: &mut egui::Ui,
    view: &mut ExplorerView,
    root: &Path,
    intents: &mut Vec<Intent>,
) {
    let Some(row) = selected_row(view) else {
        return;
    };
    let not_root = row.path != root;
    let (rename, trash, events) = ui.input_mut(|input| {
        let rename = input.consume_key(egui::Modifiers::NONE, egui::Key::F2);
        let trash = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Backspace)
            || input.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
        let events: Vec<egui::Event> = input
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                )
            })
            .cloned()
            .collect();
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
            )
        });
        (rename, trash, events)
    });
    if rename && not_root {
        view.naming = Some(Naming::new(NameKind::Rename(row.path.clone())));
    }
    if trash && not_root {
        intents.push(Intent::Operation(Operation::Trash {
            sources: vec![row.path.clone()],
        }));
    }
    for event in events {
        match event {
            egui::Event::Copy | egui::Event::Cut if not_root => intents.push(Intent::Copy {
                paths: vec![row.path.clone()],
                cut: matches!(event, egui::Event::Cut),
            }),
            egui::Event::Paste(_) => intents.push(Intent::Paste(folder_of(&row))),
            _ => {}
        }
    }
}

/// Arrow keys, Home/End and Page Up/Down move the selection; Right and Left
/// open and close folders; Enter opens; typing selects by name.
fn keyboard(ui: &mut egui::Ui, view: &mut ExplorerView, height: f32, intents: &mut Vec<Intent>) {
    let rows = view.rows.clone();
    if rows.is_empty() {
        return;
    }
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return;
    }
    let current = view
        .selected
        .as_ref()
        .and_then(|selected| rows.iter().position(|row| &row.path == selected));
    let at = current.unwrap_or(0);
    let last = rows.len() - 1;
    let page = (ui.available_height() / height).max(1.0) as usize;
    let pressed = |ui: &mut egui::Ui, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    let moved = if pressed(ui, egui::Key::ArrowDown) {
        Some(current.map_or(0, |at| (at + 1).min(last)))
    } else if pressed(ui, egui::Key::ArrowUp) {
        Some(at.saturating_sub(1))
    } else if pressed(ui, egui::Key::Home) {
        Some(0)
    } else if pressed(ui, egui::Key::End) {
        Some(last)
    } else if pressed(ui, egui::Key::PageDown) {
        Some((at + page).min(last))
    } else if pressed(ui, egui::Key::PageUp) {
        Some(at.saturating_sub(page))
    } else {
        None
    };
    let row = &rows[at];
    let directory = row.kind == EntryKind::Directory;
    let target = if moved.is_some() {
        moved
    } else if pressed(ui, egui::Key::ArrowRight) {
        if directory && !row.expanded {
            intents.push(Intent::Toggle(row.path.clone()));
            None
        } else {
            rows.get(at + 1)
                .filter(|child| directory && child.depth > row.depth)
                .map(|_| at + 1)
        }
    } else if pressed(ui, egui::Key::ArrowLeft) {
        if directory && row.expanded {
            intents.push(Intent::Toggle(row.path.clone()));
            None
        } else {
            rows[..at]
                .iter()
                .rposition(|parent| parent.depth < row.depth)
        }
    } else {
        if current.is_some() && pressed(ui, egui::Key::Enter) {
            intents.push(if directory {
                Intent::Toggle(row.path.clone())
            } else {
                Intent::Open {
                    path: row.path.clone(),
                    preview: false,
                    viewer: None,
                }
            });
        }
        type_to_select(ui, view, &rows, at)
    };
    if let Some(index) = target {
        view.selected = Some(rows[index].path.clone());
        view.reveal = view.selected.clone();
    }
}

/// Typed letters select the next row whose name starts with them.
pub(crate) fn type_to_select(
    ui: &mut egui::Ui,
    view: &mut ExplorerView,
    rows: &[Row],
    at: usize,
) -> Option<usize> {
    let typed: String = ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    });
    if typed.is_empty() || typed.chars().any(char::is_control) {
        return None;
    }
    ui.input_mut(|input| {
        input
            .events
            .retain(|event| !matches!(event, egui::Event::Text(_)))
    });
    let now = ui.input(|input| input.time);
    if now - view.typed_at > 0.8 {
        view.typed.clear();
    }
    view.typed_at = now;
    view.typed.push_str(&typed.to_lowercase());
    // Typing more of the selected row's name keeps it selected.
    let start = if view.typed.chars().count() > 1 {
        at
    } else {
        at + 1
    };
    (0..rows.len())
        .map(|offset| (start + offset) % rows.len())
        .find(|&index| rows[index].name.to_lowercase().starts_with(&view.typed))
}

pub(crate) fn apply(world: &mut World, panel: Entity, root: &Path, intent: Intent) {
    match intent {
        Intent::Toggle(path) => {
            if let (Ok(relative), Some(mut tree)) = (
                path.strip_prefix(root),
                world.get_mut::<FileExplorer>(panel),
            ) {
                let open = tree.is_expanded(relative);
                tree.set_expanded(relative, !open);
            }
        }
        Intent::Open {
            path,
            preview,
            viewer,
        } => {
            // Somewhere other than the explorer (`Placement::From`). A
            // preview leaves it focused, to keep browsing.
            let options = PathOptions {
                preview,
                keep_focus: preview,
                viewer,
                place: Some(bedtime_workbench::Placement::From(panel)),
                ..default()
            };
            match world.workbench_mut().open_path(&path, options) {
                Ok(_) => {}
                Err(error) => {
                    let name = path.file_name().unwrap_or(path.as_os_str()).display();
                    world.workbench_mut().notify(Notification::error(format!(
                        "Couldn't open {name}: {error}"
                    )));
                }
            }
        }
        Intent::Refresh => world.resource_mut::<Folders>().refresh(),
        Intent::ToggleHidden | Intent::ToggleIgnored => {
            let defaults = world
                .get_resource::<FileExplorerDefaults>()
                .copied()
                .unwrap_or_default();
            if let Some(mut tree) = world.get_mut::<FileExplorer>(panel) {
                let shown = tree.filter(&defaults);
                if matches!(intent, Intent::ToggleHidden) {
                    tree.show_hidden = Some(!shown.hidden);
                } else {
                    tree.show_ignored = Some(!shown.ignored);
                }
            }
        }
        Intent::Operation(operation) => {
            if let Err(error) = crate::ops::run_file_operation(world, operation, Some(panel)) {
                world.workbench_mut().notify(Notification::error(error));
            }
        }
        Intent::Copy { paths, cut } => crate::ops::copy_files(world, paths, cut),
        Intent::Paste(folder) => {
            if let Err(error) = crate::ops::paste_files(world, folder, Some(panel)) {
                world.workbench_mut().notify(Notification::warning(error));
            }
        }
        Intent::Reveal(path) => crate::ops::reveal(world, panel, &path),
        Intent::SetMode(mode) => {
            if let Some(mut explorer) = world.get_mut::<FileExplorer>(panel)
                && explorer.mode != mode
            {
                explorer.mode = mode;
            }
        }
    }
}

/// Opens `path` in the area it's dropped in (or does nothing if it can't).
pub(crate) fn open_factory(path: PathBuf) -> bedtime_workbench::PanelFactory {
    open_files_factory(vec![path])
}

/// A factory opening `paths` in the area it's given (the last one shows),
/// for dropping files on a layout. Errors are notifications.
pub fn open_files_factory(paths: Vec<PathBuf>) -> bedtime_workbench::PanelFactory {
    bedtime_workbench::PanelFactory::new("File", move |world, area| {
        let mut shown = Entity::PLACEHOLDER;
        for path in &paths {
            let options = PathOptions {
                target: bedtime_workbench::Target::New,
                place: Some(bedtime_workbench::Placement::In(area)),
                ..default()
            };
            match world.workbench_mut().open_path(path, options) {
                Ok(panel) => shown = panel,
                Err(error) => {
                    let name = path.file_name().unwrap_or(path.as_os_str()).display();
                    world.workbench_mut().notify(Notification::error(format!(
                        "Couldn't open {name}: {error}"
                    )));
                }
            }
        }
        shown
    })
}

/// The icon for a file, by its extension. Does no I/O.
pub fn file_icon(path: &Path) -> BuiltinIcon {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => BuiltinIcon::Rust,
        "py" => BuiltinIcon::Python,
        "js" | "mjs" => BuiltinIcon::Javascript,
        "ts" => BuiltinIcon::Typescript,
        "json" => BuiltinIcon::Json,
        "toml" | "ini" | "ron" => BuiltinIcon::Config,
        "md" => BuiltinIcon::Markdown,
        "png" | "apng" | "jpg" | "jpeg" | "jfif" | "svg" | "webp" | "gif" | "bmp" | "ico"
        | "tif" | "tiff" | "tga" | "qoi" | "dds" | "hdr" | "exr" | "pnm" | "ppm" | "pgm"
        | "pbm" => BuiltinIcon::Image,
        "mp3" | "wav" | "wave" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "aif" | "aiff"
        | "caf" => BuiltinIcon::Audio,
        "mp4" | "m4v" | "mov" | "qt" | "3gp" | "mkv" | "webm" | "avi" => BuiltinIcon::Video,
        "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2" => BuiltinIcon::Font,
        "csv" | "tsv" | "tab" => BuiltinIcon::Spreadsheet,
        "gltf" | "glb" | "stl" | "obj" | "fbx" => BuiltinIcon::Box,
        "sh" => BuiltinIcon::Bash,
        "txt" => BuiltinIcon::Text,
        _ => BuiltinIcon::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, kind: EntryKind) -> Row {
        Row {
            path: PathBuf::from(path),
            name: String::new(),
            depth: 1,
            kind,
            hidden: false,
            ignored: false,
            expanded: false,
            loading: false,
            error: None,
        }
    }

    #[test]
    fn tooltips_say_what_a_file_is_how_big_and_how_old() {
        assert_eq!(file_kind(Path::new("a/photo.PNG")), "PNG image");
        assert_eq!(file_kind(Path::new("main.rs")), "Rust source");
        assert_eq!(file_kind(Path::new("data.xyz")), "XYZ file");
        assert_eq!(file_kind(Path::new("Makefile")), "File");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(4_200), "4.2 KB");
        assert_eq!(size(42_000_000), "42 MB");
        let now = std::time::SystemTime::now();
        let before = |seconds| now - std::time::Duration::from_secs(seconds);
        assert_eq!(ago(before(5)), "just now");
        assert_eq!(ago(before(60)), "1 minute ago");
        assert_eq!(ago(before(3 * 3600 + 5)), "3 hours ago");
        assert_eq!(ago(before(90_000)), "yesterday");
        assert_eq!(ago(before(800 * 86_400)), "2 years ago");
    }

    #[test]
    fn things_go_in_a_folder_or_a_files_folder() {
        assert_eq!(
            folder_of(&row("/r/src", EntryKind::Directory)),
            PathBuf::from("/r/src")
        );
        assert_eq!(
            folder_of(&row("/r/src/a.rs", EntryKind::File)),
            PathBuf::from("/r/src")
        );
    }

    #[test]
    fn names_make_operations() {
        let mut naming = Naming::new(NameKind::NewFile("/r".into()));
        assert!(naming.operation().is_none(), "an empty name does nothing");
        naming.text = "  new.rs ".into();
        assert!(matches!(
            naming.operation(),
            Some(Operation::CreateFile { directory, name }) if directory == Path::new("/r") && name == "new.rs"
        ));
        let rename = Naming::new(NameKind::Rename("/r/old.txt".into()));
        assert_eq!(rename.text, "old.txt", "a rename starts from the name");
        assert_eq!(rename.row(), Path::new("/r/old.txt"));
        assert!(matches!(
            Naming::new(NameKind::NewFolder("/r".into())).kind,
            NameKind::NewFolder(_)
        ));
    }
}
