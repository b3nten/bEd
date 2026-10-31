//! Drawing the tree. Drawing only reads; what the user asks for comes back as
//! intents, applied after.
use crate::{FileTree, Folders, Row, TreeView, refresh_rows};
use bedtime_filesystem::EntryKind;
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
#[derive(Debug, PartialEq)]
pub(crate) enum Intent {
    Toggle(PathBuf),
    Open {
        path: PathBuf,
        preview: bool,
        opener: Option<OpenerId>,
    },
    Refresh,
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
    ui.spacing_mut().item_spacing.y = 0.0;
    let output = scroll.show_rows(ui, height, rows.len(), |ui, range| {
        for row in &rows[range] {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            let response = ui.interact(
                rect,
                egui::Id::new((panel, &row.path)),
                egui::Sense::click_and_drag(),
            );
            // Drag a file onto the layout to open it there (an area, beside
            // a tab, a new split), if something here can open it.
            if response.drag_started()
                && row.kind != EntryKind::Directory
                && !world.workbench().openers_for(&row.path).is_empty()
            {
                bedtime_workbench::panel_drag_source(&response, open_factory(row.path.clone()));
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
            response.context_menu(|ui| menu(ui, row, &view, &mut intents));
        }
    });
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
fn menu(ui: &mut egui::Ui, row: &Row, view: &TreeView, intents: &mut Vec<Intent>) {
    if row.kind != EntryKind::Directory {
        if ui.button("Open").clicked() {
            intents.push(Intent::Open {
                path: row.path.clone(),
                preview: false,
                opener: None,
            });
        }
        let openers = view
            .menu
            .as_ref()
            .filter(|(path, _)| path == &row.path)
            .map_or(&[][..], |(_, openers)| openers);
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
    if ui.button("Copy Path").clicked() {
        ui.ctx().copy_text(row.path.display().to_string());
    }
    if ui.button("Refresh").clicked() {
        intents.push(Intent::Refresh);
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
    }
}

/// Opens `path` in the area it's dropped in (or does nothing if it can't).
fn open_factory(path: PathBuf) -> bedtime_workbench::PanelFactory {
    bedtime_workbench::PanelFactory::new("File", move |world, area| {
        let options = PathOptions {
            target: bedtime_workbench::Target::New { area: Some(area) },
            ..default()
        };
        match world.workbench_mut().open_path(&path, options) {
            Ok(Some(panel)) => panel,
            Ok(None) => Entity::PLACEHOLDER,
            Err(error) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).display();
                world.workbench_mut().notify(Notification::error(format!(
                    "Couldn't open {name}: {error}"
                )));
                Entity::PLACEHOLDER
            }
        }
    })
}

/// The icon for a file, by its extension. Does no I/O.
pub fn file_icon(path: &Path) -> BuiltinIcon {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "rs" => BuiltinIcon::Rust,
        "py" => BuiltinIcon::Python,
        "js" | "mjs" => BuiltinIcon::Javascript,
        "ts" => BuiltinIcon::Typescript,
        "json" => BuiltinIcon::Json,
        "toml" | "ini" | "ron" => BuiltinIcon::Config,
        "md" => BuiltinIcon::Markdown,
        "png" | "jpg" | "jpeg" | "svg" | "webp" => BuiltinIcon::Image,
        "sh" => BuiltinIcon::Bash,
        "txt" => BuiltinIcon::Text,
        _ => BuiltinIcon::Default,
    }
}
