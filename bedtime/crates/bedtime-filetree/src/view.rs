//! Drawing the tree. Drawing only reads; what the user asks for comes back as
//! intents, applied after.
use crate::{FileTree, Folders, Row, TreeView, refresh_rows};
use bedtime_filesystem::{EntryKind, operations::Operation};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::color, tree};
use bedtime_workbench::{
    LayoutsWorldExt, Notification, OpenerId, Panel, PanelPresentation, PathOptions,
    WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

impl Panel for FileTree {
    const LABEL: &'static str = "Files";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw(ui, world, context, panel);
    }
}

/// What the user asked for.
#[derive(Debug)]
pub(crate) enum Intent {
    Toggle(PathBuf),
    Open {
        path: PathBuf,
        preview: bool,
        opener: Option<OpenerId>,
    },
    Refresh,
    Operation(Operation),
    Copy {
        paths: Vec<PathBuf>,
        cut: bool,
    },
    Paste(PathBuf),
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
    fn row(&self) -> &Path {
        match &self.kind {
            NameKind::NewFile(folder) | NameKind::NewFolder(folder) => folder,
            NameKind::Rename(path) => path,
        }
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
struct TreeDrag {
    paths: Vec<PathBuf>,
}

/// The folder something goes in, from the row it's at: the folder itself,
/// or a file's folder.
fn folder_of(row: &Row) -> PathBuf {
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
    let Some(mut view) = world.entity_mut(panel).take::<TreeView>() else {
        return;
    };
    if let Some(tree) = world.get::<FileTree>(panel) {
        refresh_rows(&root, tree, world.resource::<Folders>(), &mut view);
    }
    let mut intents = Vec::new();
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let ctx = ui.ctx().clone();
    if focused
        && ui.is_enabled()
        && !ctx.text_edit_focused()
        && !ctx.any_popup_open()
        && ctx.memory(|memory| memory.top_modal_layer().is_none())
    {
        keyboard(ui, &mut view, tree::row_height(context), &mut intents);
        editing_keys(ui, &mut view, &root, &mut intents);
    }

    if let Some(error) = world.resource::<Folders>().watch_error() {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.weak("Not watching for changes")
                    .on_hover_text(error.to_owned());
            });
    }

    let height = tree::row_height(context);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt((panel, "files"))
        .auto_shrink([false, false]);
    if let Some(path) = view.reveal.take()
        && let Some(index) = view.rows.iter().position(|row| row.path == path)
    {
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
        folder_menu(ui, &root, &mut view.naming, &mut intents);
    });
    let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();
    ui.spacing_mut().item_spacing.y = 0.0;
    let output = scroll.show_rows(ui, height, rows.len(), |ui, range| {
        let first = range.start;
        for (offset, row) in rows[range].iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            let response = ui.interact(
                rect,
                egui::Id::new((panel, &row.path)),
                egui::Sense::click_and_drag(),
            );
            row_rects.push((first + offset, rect));
            // Drag onto a folder in the tree to move it there; onto the layout
            // to open a file there (an area, beside a tab, a new split).
            if response.drag_started() && row.depth > 0 {
                let opens = row.kind != EntryKind::Directory
                    && !world.workbench().openers_for(&row.path).is_empty();
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
            paint(ui, context, row, &view, rect, &response);
            // egui counts a click on another row just before as part of a
            // triple click.
            if response.double_clicked() || response.triple_clicked() {
                if row.kind != EntryKind::Directory {
                    intents.push(Intent::Open {
                        path: row.path.clone(),
                        preview: false,
                        opener: None,
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
                        opener: None,
                    }
                });
            }
            if response.secondary_clicked() {
                view.selected = Some(row.path.clone());
                let openers = if row.kind == EntryKind::Directory {
                    Vec::new()
                } else {
                    world.workbench().openers_for(&row.path)
                };
                view.menu = Some((row.path.clone(), openers));
            }
            response.context_menu(|ui| {
                let openers = view
                    .menu
                    .as_ref()
                    .filter(|(path, _)| path == &row.path)
                    .map(|(_, openers)| openers.clone())
                    .unwrap_or_default();
                menu(ui, row, &openers, &mut view.naming, &mut intents);
            });
        }
    });
    drop_into_tree(
        ui,
        &ctx,
        &rows,
        &row_rects,
        whole,
        &root,
        context,
        &mut intents,
    );
    if view.naming.is_some() {
        draw_naming(ui, &ctx, panel, &mut view, &row_rects, whole, &mut intents);
    }
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new((panel, "files-scroll")),
            (output.state.offset.y, output.inner_rect.height()),
        )
    });
    world.entity_mut(panel).insert(view);
    // Applied in `Update`: opening can close panels the layout is drawing.
    world
        .resource_mut::<Intents>()
        .0
        .extend(intents.into_iter().map(|intent| (panel, intent)));
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

fn paint(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    row: &Row,
    view: &TreeView,
    rect: egui::Rect,
    response: &egui::Response,
) {
    let palette = &context.theme.palette;
    let directory = row.kind == EntryKind::Directory;
    let icon = match row.kind {
        EntryKind::Directory if row.expanded => BuiltinIcon::FolderOpen,
        EntryKind::Directory => BuiltinIcon::Folder,
        EntryKind::Symlink => BuiltinIcon::Extension,
        _ => file_icon(&row.path),
    };
    let ink = if row.error.is_some() {
        palette.error
    } else if row.hidden {
        palette.muted
    } else {
        palette.text
    };
    tree::paint_row(
        ui,
        context,
        tree::Row {
            label: Some(&row.name),
            depth: row.depth,
            icon,
            expanded: directory.then_some(row.expanded),
            selected: view.selected.as_ref() == Some(&row.path),
            ink: color(ink),
            icon_ink: palette.text,
        },
        rect,
        Some(response),
        if row.loading { 0.6 } else { 1.0 },
        tree::INDENT,
    );
    if let Some(error) = &row.error {
        response.clone().on_hover_text(error);
    }
}

/// The context menu of `row`.
fn menu(
    ui: &mut egui::Ui,
    row: &Row,
    openers: &[(OpenerId, &'static str)],
    naming: &mut Option<Naming>,
    intents: &mut Vec<Intent>,
) {
    let root = row.depth == 0;
    if row.kind != EntryKind::Directory {
        if ui.button("Open").clicked() {
            intents.push(Intent::Open {
                path: row.path.clone(),
                preview: false,
                opener: None,
            });
        }
        if !openers.is_empty() {
            ui.menu_button("Open With", |ui| {
                for &(opener, label) in openers {
                    if ui.button(label).clicked() {
                        intents.push(Intent::Open {
                            path: row.path.clone(),
                            preview: false,
                            opener: Some(opener),
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
fn folder_menu(
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

/// The box a name is typed in, under its row (or at the top). Enter does it,
/// Escape (or clicking away) doesn't.
fn draw_naming(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    panel: Entity,
    view: &mut TreeView,
    row_rects: &[(usize, egui::Rect)],
    whole: egui::Rect,
    intents: &mut Vec<Intent>,
) {
    let Some(naming) = view.naming.as_mut() else {
        return;
    };
    let anchor = view
        .rows
        .iter()
        .position(|row| row.path == naming.row())
        .and_then(|index| row_rects.iter().find(|(at, _)| *at == index))
        .map_or(whole.left_top(), |(_, rect)| rect.left_bottom());
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
    let _ = ui;
    if let Some(operation) = done {
        view.naming = None;
        if let Some(operation) = operation {
            intents.push(Intent::Operation(operation));
        }
    }
}

/// While a tree drag (or files from outside the app) is over the tree: the
/// folder it would go into is highlighted (a file's folder, or the root over
/// empty space), and letting go moves it there (Alt/Option or Cmd/Ctrl
/// copies); files from outside are copied (Alt/Option or Cmd/Ctrl moves). Dropping it on itself,
/// where it already is, or inside itself does nothing.
#[allow(clippy::too_many_arguments)]
fn drop_into_tree(
    ui: &egui::Ui,
    ctx: &egui::Context,
    rows: &[Row],
    row_rects: &[(usize, egui::Rect)],
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
    let hovered = row_rects
        .iter()
        .find(|(_, rect)| rect.y_range().contains(pointer.y))
        .map(|(index, rect)| (&rows[*index], *rect));
    let folder = hovered.map_or_else(|| root.to_owned(), |(row, _)| folder_of(row));
    let fits = paths
        .iter()
        .all(|path| path.parent() != Some(folder.as_path()) && !folder.starts_with(path));
    // The tree takes the drop (the layout doesn't make a panel of it).
    bedtime_workbench::PanelFactoryPayload::reserve_for_content(ctx);
    if !fits {
        return;
    }
    let highlight = rows
        .iter()
        .position(|row| row.path == folder)
        .and_then(|index| row_rects.iter().find(|(at, _)| *at == index))
        .map_or(whole, |(_, rect)| *rect);
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
fn editing_keys(ui: &mut egui::Ui, view: &mut TreeView, root: &Path, intents: &mut Vec<Intent>) {
    let Some(selected) = view.selected.clone() else {
        return;
    };
    let Some(row) = view.rows.iter().find(|row| row.path == selected).cloned() else {
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
fn keyboard(ui: &mut egui::Ui, view: &mut TreeView, height: f32, intents: &mut Vec<Intent>) {
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
                    opener: None,
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
fn type_to_select(
    ui: &mut egui::Ui,
    view: &mut TreeView,
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
            if let (Ok(relative), Some(mut tree)) =
                (path.strip_prefix(root), world.get_mut::<FileTree>(panel))
            {
                let open = tree.is_expanded(relative);
                tree.set_expanded(relative, !open);
            }
        }
        Intent::Open {
            path,
            preview,
            opener,
        } => {
            let options = PathOptions {
                preview,
                opener,
                ..default()
            };
            match world.workbench_mut().open_path(&path, options) {
                // A preview leaves the tree focused, to keep browsing.
                Ok(Some(opened)) if preview => {
                    let area = |panel| world.get::<bedtime_workbench::InArea>(panel).map(|a| a.0);
                    if area(opened) != area(panel) {
                        let _ = world.layouts_mut().focus_panel(panel);
                    }
                }
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
    }
}

/// Opens `path` in the area it's dropped in (or does nothing if it can't).
fn open_factory(path: PathBuf) -> bedtime_workbench::PanelFactory {
    open_files_factory(vec![path])
}

/// A factory opening `paths` in the area it's given (the last one shows),
/// for dropping files on a layout. Errors are notifications.
pub fn open_files_factory(paths: Vec<PathBuf>) -> bedtime_workbench::PanelFactory {
    bedtime_workbench::PanelFactory::new("File", move |world, area| {
        let mut shown = Entity::PLACEHOLDER;
        for path in &paths {
            let options = PathOptions {
                target: bedtime_workbench::Target::New { area: Some(area) },
                ..default()
            };
            match world.workbench_mut().open_path(path, options) {
                Ok(Some(panel)) => shown = panel,
                Ok(None) => {}
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
            expanded: false,
            loading: false,
            error: None,
        }
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
