//! Drawing the column view: a column per folder from the root down to the
//! selection, each listing its folder's entries, the folder that's open in
//! the next column marked. Everything comes from the selection, so it
//! follows whatever selects (the tree, the footer, a finished operation).
use crate::view::{
    Intent, Shown, TreeDrag, describe, details, draw_naming, drop_into, editing_keys, filter_menu,
    folder_menu, menu, open_factory, type_to_select,
};
use crate::{COLUMN_WIDTH, Column, ExplorerView, Folders, Row, file_icon};
use bedtime_filesystem::EntryKind;
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::color, tree};
use bedtime_workbench::WorkbenchWorldExt;
use bevy::prelude::*;
use std::path::Path;

pub(crate) fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    shown: &Shown<'_>,
    view: &mut ExplorerView,
    intents: &mut Vec<Intent>,
) {
    let Shown {
        panel, root, keys, ..
    } = *shown;
    let height = tree::row_height(context);
    if keys {
        keyboard(ui, view, height, intents);
        editing_keys(ui, view, root, intents);
    }
    let ctx = ui.ctx().clone();
    let columns = view.columns.clone();
    let whole = ui.available_rect_before_wrap();

    // The selection may not be in a column yet (its folder being read).
    let reveal = view.reveal.clone().filter(|path| {
        columns
            .iter()
            .any(|column| column.rows.iter().any(|row| &row.path == path))
    });
    if reveal.is_some() || !columns.iter().any(|column| column.loading) {
        view.reveal = None;
    }

    let total = columns.len() as f32 * COLUMN_WIDTH;
    let mut scroll = egui::ScrollArea::horizontal()
        .id_salt((panel, "columns"))
        .auto_shrink([false, false]);
    if reveal.is_some() {
        // The last column, where the selection leads.
        scroll = scroll.horizontal_scroll_offset((total - whole.width()).max(0.0));
    }
    let mut items: Vec<(Row, egui::Rect)> = Vec::new();
    let mut areas: Vec<(&Path, egui::Rect)> = Vec::new();
    scroll.show(ui, |ui| {
        let origin = ui.max_rect().min;
        let column_height = ui.available_height();
        ui.set_min_size(egui::vec2(total, column_height));
        let separator = ui.visuals().widgets.noninteractive.bg_stroke;
        for (index, column) in columns.iter().enumerate() {
            let rect = egui::Rect::from_min_size(
                origin + egui::vec2(index as f32 * COLUMN_WIDTH, 0.0),
                egui::vec2(COLUMN_WIDTH, column_height),
            );
            areas.push((&column.folder, rect));
            ui.painter()
                .vline(rect.right() - 0.5, rect.y_range(), separator);
            let mut column_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("column", &column.folder))
                    .max_rect(rect.shrink2(egui::vec2(0.0, 2.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            column_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            draw_column(
                &mut column_ui,
                world,
                context,
                shown,
                view,
                column,
                reveal.as_deref(),
                &mut items,
                intents,
            );
        }
    });

    let item_refs: Vec<(&Row, egui::Rect)> = items.iter().map(|(row, rect)| (row, *rect)).collect();
    drop_into(ui, &ctx, &item_refs, &areas, whole, root, context, intents);
    if let Some(naming) = &view.naming {
        // Something new goes at the top of its folder's column; a rename
        // under its row.
        let area = || {
            areas
                .iter()
                .find(|(path, _)| *path == naming.row())
                .map(|(_, rect)| egui::Rect::from_min_size(rect.min, egui::Vec2::ZERO))
        };
        let row = || {
            items
                .iter()
                .find(|(row, _)| row.path == naming.row())
                .map(|(_, rect)| *rect)
        };
        let anchor = if naming.goes_in() {
            area().or_else(row)
        } else {
            row()
        };
        draw_naming(&ctx, panel, view, anchor, whole, intents);
    }
}

/// One column: its rows (only those in view), or why there are none. Its
/// empty space has the folder's menu.
#[allow(clippy::too_many_arguments)]
fn draw_column(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    shown: &Shown<'_>,
    view: &mut ExplorerView,
    column: &Column,
    reveal: Option<&Path>,
    items: &mut Vec<(Row, egui::Rect)>,
    intents: &mut Vec<Intent>,
) {
    let panel = shown.panel;
    let height = tree::row_height(context);
    let background = ui.interact(
        ui.max_rect(),
        egui::Id::new((panel, "column", &column.folder)),
        egui::Sense::click(),
    );
    background.context_menu(|ui| {
        folder_menu(ui, &column.folder, &mut view.naming, intents);
        filter_menu(ui, shown.filter, shown.mode, intents);
    });
    if column.rows.is_empty() {
        let palette = &context.theme.palette;
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| match &column.error {
                Some(error) => {
                    ui.colored_label(color(palette.error), error);
                }
                None if column.loading => {
                    ui.weak("Reading…");
                }
                None => {
                    ui.weak("Empty");
                }
            });
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt((panel, "column-scroll", &column.folder))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let width = ui.available_width();
            let rows = &column.rows;
            ui.set_min_height(rows.len() as f32 * height);
            ui.set_min_width(width);
            let top = ui.max_rect().top();
            let left = ui.max_rect().left();
            let rect_of = |index: usize| {
                egui::Rect::from_min_size(
                    egui::pos2(left, top + index as f32 * height),
                    egui::vec2(width, height),
                )
            };
            // Reveal the selection, or the folder on the way to it.
            if let Some(reveal) = reveal
                && let Some(index) = rows
                    .iter()
                    .position(|row| row.path == reveal || reveal.starts_with(&row.path))
            {
                ui.scroll_to_rect(rect_of(index), None);
            }
            let first = (viewport.min.y / height).floor().max(0.0) as usize;
            let last = ((viewport.max.y / height).ceil().max(0.0) as usize + 1).min(rows.len());
            for (index, row) in rows.iter().enumerate().take(last).skip(first.min(last)) {
                let rect = rect_of(index);
                let response = ui.interact(
                    rect,
                    egui::Id::new((panel, "column-row", &row.path)),
                    egui::Sense::click_and_drag(),
                );
                items.push((row.clone(), rect));
                interact(ui, world, context, view, row, &response, shown, intents);
                paint(ui, context, view, row, rect, &response);
            }
        });
}

/// What a click, double click, right click or drag on `row` does: as in the
/// tree, except a click on a folder selects it (its column follows) rather
/// than opening it.
#[allow(clippy::too_many_arguments)]
fn interact(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    view: &mut ExplorerView,
    row: &Row,
    response: &egui::Response,
    shown: &Shown<'_>,
    intents: &mut Vec<Intent>,
) {
    if response.drag_started() {
        let opens = row.kind != EntryKind::Directory
            && !world.workbench().viewers_for(&row.path).is_empty();
        let factory = if opens {
            open_factory(row.path.clone())
        } else {
            bedtime_workbench::PanelFactory::new("File", |_, _| Entity::PLACEHOLDER)
        };
        response.dnd_set_drag_payload(bedtime_workbench::PanelFactoryPayload::with_content(
            factory,
            TreeDrag {
                paths: vec![row.path.clone()],
            },
        ));
    }
    let selected = view.selected.as_ref() == Some(&row.path);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &row.name)
    });
    if tree::rested(ui.ctx(), response) {
        let details = details(world.resource::<Folders>(), view, &row.path);
        context.frame(ui).tooltip_ui(response, |frame| {
            describe(frame, context, row, details.as_ref())
        });
    }
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
        // Scroll to the column it leads to.
        view.reveal = Some(row.path.clone());
        if row.kind != EntryKind::Directory {
            intents.push(Intent::Open {
                path: row.path.clone(),
                preview: true,
                viewer: None,
            });
        }
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
        filter_menu(ui, shown.filter, shown.mode, intents);
    });
}

/// A row across its column: highlighted when selected, a quieter fill when
/// it's the folder open in the next column, and a chevron for a folder.
fn paint(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &ExplorerView,
    row: &Row,
    rect: egui::Rect,
    response: &egui::Response,
) {
    let palette = &context.theme.palette;
    let selected = view.selected.as_ref() == Some(&row.path);
    let fill = rect.shrink2(egui::vec2(4.0, 1.0));
    if row.expanded && !selected {
        let visuals = ui.visuals();
        ui.painter().rect_filled(
            fill,
            visuals.widgets.inactive.corner_radius,
            visuals.widgets.inactive.weak_bg_fill,
        );
    } else {
        tree::paint_highlight(ui, ui.painter(), context, fill, response, selected);
    }
    let icon = match row.kind {
        EntryKind::Directory if row.expanded => BuiltinIcon::FolderOpen,
        EntryKind::Directory => BuiltinIcon::Folder,
        EntryKind::Symlink => BuiltinIcon::Extension,
        _ => file_icon(&row.path),
    };
    let ink = if row.hidden || row.ignored {
        palette.muted
    } else {
        palette.text
    };
    // Rows here have no disclosure: start where a tree row's would be.
    let mut at = rect;
    at.min.x -= 14.0;
    tree::paint_row(
        ui,
        context,
        tree::Row {
            label: Some(&row.name),
            depth: 0,
            icon: Some(icon),
            expanded: None,
            selected,
            ink: color(ink),
            icon_ink: palette.text,
        },
        at,
        None,
        1.0,
        tree::INDENT,
    );
    if row.kind == EntryKind::Directory {
        bedtime_ui::paint_chevron(
            ui.painter(),
            egui::pos2(rect.right() - 12.0, rect.center().y),
            0.0,
            color(palette.muted),
        );
    }
}

/// Up/Down, Home/End and Page Up/Down move within the selection's column;
/// Right goes into a folder, Left back to its folder; Enter opens a file or
/// goes into a folder; typing selects by name in the column.
fn keyboard(ui: &mut egui::Ui, view: &mut ExplorerView, height: f32, intents: &mut Vec<Intent>) {
    let columns = view.columns.clone();
    let Some(first) = columns.first() else {
        return;
    };
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return;
    }
    let pressed = |ui: &mut egui::Ui, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    // Where the selection is: a column, and a row in it.
    let at = view.selected.as_ref().and_then(|selected| {
        columns.iter().enumerate().find_map(|(index, column)| {
            let row = column.rows.iter().position(|row| &row.path == selected)?;
            Some((index, row))
        })
    });
    let Some((index, at)) = at else {
        // Nothing selected (or the root): a key starts in the first column.
        let start = [
            egui::Key::ArrowDown,
            egui::Key::ArrowUp,
            egui::Key::ArrowRight,
            egui::Key::Home,
            egui::Key::End,
        ]
        .into_iter()
        .any(|key| pressed(ui, key));
        let target = if start {
            first.rows.first().map(|row| row.path.clone())
        } else {
            type_to_select(ui, view, &first.rows, first.rows.len().saturating_sub(1))
                .map(|index| first.rows[index].path.clone())
        };
        if let Some(path) = target {
            view.selected = Some(path.clone());
            view.reveal = Some(path);
        }
        return;
    };
    let column = &columns[index];
    let rows = &column.rows;
    let row = &rows[at];
    let last = rows.len() - 1;
    let page = (ui.available_height() / height).max(1.0) as usize;
    // Into a folder: its first entry, once it's read.
    let inside = |row: &Row| {
        columns
            .get(index + 1)
            .filter(|next| next.folder == row.path)
            .and_then(|next| next.rows.first())
            .map(|first| first.path.clone())
    };
    let target = if pressed(ui, egui::Key::ArrowDown) {
        Some(rows[(at + 1).min(last)].path.clone())
    } else if pressed(ui, egui::Key::ArrowUp) {
        Some(rows[at.saturating_sub(1)].path.clone())
    } else if pressed(ui, egui::Key::Home) {
        Some(rows[0].path.clone())
    } else if pressed(ui, egui::Key::End) {
        Some(rows[last].path.clone())
    } else if pressed(ui, egui::Key::PageDown) {
        Some(rows[(at + page).min(last)].path.clone())
    } else if pressed(ui, egui::Key::PageUp) {
        Some(rows[at.saturating_sub(page)].path.clone())
    } else if pressed(ui, egui::Key::ArrowRight) {
        inside(row)
    } else if pressed(ui, egui::Key::ArrowLeft) {
        (index > 0).then(|| column.folder.clone())
    } else if pressed(ui, egui::Key::Enter) {
        if row.kind == EntryKind::Directory {
            inside(row)
        } else {
            intents.push(Intent::Open {
                path: row.path.clone(),
                preview: false,
                viewer: None,
            });
            None
        }
    } else {
        type_to_select(ui, view, rows, at).map(|index| rows[index].path.clone())
    };
    if let Some(path) = target {
        view.selected = Some(path.clone());
        view.reveal = Some(path);
    }
}
