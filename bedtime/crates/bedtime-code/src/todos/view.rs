//! Drawing TODOs: the toolbar (filter, layout, refresh, options) and the rows.
//! Opening files comes back as `Intent`s, done by `apply` after drawing.
use super::{Row, Scan, TodoLayout, TodoPanel, TodoSort, TodoState, scan::TAGS};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme, tree};
use bedtime_workbench::PanelPresentation;
use bevy::prelude::*;
use std::sync::Arc;

#[derive(Debug, PartialEq)]
pub(super) enum Intent {
    /// Open item `index` of the scan; leave the keyboard here if `keep_focus`.
    Open {
        scan: Arc<Scan>,
        index: usize,
        keep_focus: bool,
    },
}

pub(super) fn apply(world: &mut World, panel: Entity, intents: Vec<Intent>) {
    for intent in intents {
        match intent {
            Intent::Open {
                scan,
                index,
                keep_focus,
            } => {
                if let Some(item) = scan.items.get(index) {
                    super::open(world, panel, item, keep_focus);
                }
            }
        }
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    world: &World,
    context: &UiContext<'_>,
    panel: Entity,
    shown: &mut TodoPanel,
    state: &mut TodoState,
) -> Vec<Intent> {
    let mut intents = Vec::new();
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    toolbar(ui, context, panel, shown, state);

    let Some(scan) = state.scan.clone() else {
        egui::Frame::NONE
            .inner_margin(14)
            .show(ui, |ui| match &state.error {
                Some(error) => {
                    ui.colored_label(theme::color(context.theme.palette.error), error);
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("Finding TODOs…");
                    });
                }
            });
        return intents;
    };
    let arranged = super::arrange(shown, &scan, &state.filter);
    let items = arranged
        .iter()
        .filter(|row| matches!(row, Row::Item { .. }))
        .count();
    let rows = super::visible(arranged, &state.collapsed);

    let quiet = !ui.ctx().text_edit_focused() && !ui.ctx().any_popup_open();
    if focused && quiet {
        keyboard(ui, &scan, &rows, state, &mut intents);
    }

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 2))
        .show(ui, |ui| {
            let files: std::collections::HashSet<&str> = scan
                .items
                .iter()
                .map(|item| item.relative.as_str())
                .collect();
            let mut summary = if items == scan.items.len() {
                format!("{} in {} files", items, files.len())
            } else {
                format!("{} of {} shown", items, scan.items.len())
            };
            if scan.limited {
                summary.push_str(&format!(" (stopped at {})", super::scan::MAX_ITEMS));
            }
            if let Some(error) = &state.error {
                summary = error.clone();
            }
            ui.weak(summary);
        });

    if rows.is_empty() {
        egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
            if scan.items.is_empty() {
                ui.weak("No TODO comments in this workspace");
            } else if !state.filter.trim().is_empty() {
                ui.weak(format!("Nothing matches “{}”", state.filter.trim()));
            } else {
                ui.weak("Every tag is hidden (see the options)");
            }
        });
        return intents;
    }
    draw_rows(ui, context, panel, shown, state, &scan, &rows, &mut intents);
    intents
}

fn toolbar(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    shown: &mut TodoPanel,
    state: &mut TodoState,
) {
    let height = tree::row_height(context);
    let scanning = state.task.is_some();
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 8,
            right: 6,
            top: 4,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().button_padding = egui::vec2(3.0, 3.0);
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let options = context
                        .frame(ui)
                        .icon_button_sized(BuiltinIcon::Sliders, 16.0);
                    context.frame(ui).tooltip(&options, "TODO options");
                    options.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "TODO options")
                    });
                    egui::Popup::menu(&options)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| options_menu(ui, shown, state));

                    let tree_on = shown.layout == TodoLayout::Tree;
                    let layout = context
                        .frame(ui)
                        .icon_button_sized(BuiltinIcon::Folder, 16.0);
                    if tree_on {
                        ui.painter().rect_stroke(
                            layout.rect,
                            4.0,
                            egui::Stroke::new(1.0, theme::color(context.theme.palette.accent)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let label = if tree_on {
                        "Show as a list"
                    } else {
                        "Show as a tree"
                    };
                    context.frame(ui).tooltip(&layout, label);
                    layout.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::Checkbox,
                            true,
                            tree_on,
                            "Tree",
                        )
                    });
                    if layout.clicked() {
                        shown.layout = if tree_on {
                            TodoLayout::List
                        } else {
                            TodoLayout::Tree
                        };
                    }

                    if scanning {
                        ui.add(egui::Spinner::new().size(14.0));
                    } else {
                        let refresh = context
                            .frame(ui)
                            .icon_button_sized(BuiltinIcon::RefreshCw, 16.0);
                        context.frame(ui).tooltip(&refresh, "Find TODOs again");
                        refresh.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Refresh")
                        });
                        if refresh.clicked() {
                            state.refresh();
                        }
                    }

                    let width = ui.available_width();
                    let field = context.frame(ui).text_edit(
                        egui::TextEdit::singleline(&mut state.filter)
                            .id(egui::Id::new((panel, "todos-filter")))
                            .hint_text("Filter")
                            .desired_width(width),
                    );
                    field.response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Filter")
                    });
                },
            );
        });
}

fn options_menu(ui: &mut egui::Ui, shown: &mut TodoPanel, state: &mut TodoState) {
    ui.set_min_width(200.0);
    ui.weak("Show as");
    ui.radio_value(&mut shown.layout, TodoLayout::Tree, "Tree");
    ui.radio_value(&mut shown.layout, TodoLayout::List, "List");
    ui.separator();
    ui.weak("Sort by");
    for sort in TodoSort::ALL {
        ui.radio_value(&mut shown.sort, sort, sort.label());
    }
    ui.checkbox(&mut shown.descending, "Descending");
    ui.separator();
    ui.weak("Show");
    for tag in TAGS {
        let mut show = !shown.hidden_tags.iter().any(|t| t == tag);
        if ui.checkbox(&mut show, tag).changed() {
            if show {
                shown.hidden_tags.retain(|t| t != tag);
            } else {
                shown.hidden_tags.push(tag.to_owned());
            }
        }
    }
    if shown.layout == TodoLayout::Tree {
        ui.separator();
        if ui.button("Collapse All").clicked() {
            if let Some(scan) = &state.scan {
                let rows = super::arrange(shown, scan, &state.filter);
                state.collapsed = rows
                    .into_iter()
                    .filter_map(|row| match row {
                        Row::Folder { path, .. } | Row::File { path, .. } => Some(path),
                        Row::Item { .. } => None,
                    })
                    .collect();
            }
            ui.close();
        }
        if ui.button("Expand All").clicked() {
            state.collapsed.clear();
            ui.close();
        }
    }
}

/// Up and Down move through the items, opening each (the keyboard stays
/// here); Enter opens the selected one and goes to it. In the tree, Left and
/// Right close and open the selected item's file.
fn keyboard(
    ui: &mut egui::Ui,
    scan: &Arc<Scan>,
    rows: &[Row],
    state: &mut TodoState,
    intents: &mut Vec<Intent>,
) {
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return;
    }
    let items: Vec<usize> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Item { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    if items.is_empty() {
        return;
    }
    let key = |index: usize| {
        let item = &scan.items[index];
        (item.relative.clone(), item.found.line)
    };
    let at = state
        .selected
        .as_ref()
        .and_then(|selected| items.iter().position(|&i| key(i) == *selected));
    let pressed = |ui: &mut egui::Ui, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    if pressed(ui, egui::Key::Enter) {
        if let Some(at) = at {
            intents.push(Intent::Open {
                scan: scan.clone(),
                index: items[at],
                keep_focus: false,
            });
        }
        return;
    }
    let last = items.len() - 1;
    let to = if pressed(ui, egui::Key::ArrowDown) {
        Some(at.map_or(0, |at| (at + 1).min(last)))
    } else if pressed(ui, egui::Key::ArrowUp) {
        Some(at.map_or(0, |at| at.saturating_sub(1)))
    } else if pressed(ui, egui::Key::Home) {
        Some(0)
    } else if pressed(ui, egui::Key::End) {
        Some(last)
    } else {
        None
    };
    if let Some(to) = to
        && Some(to) != at
    {
        let selected = key(items[to]);
        state.selected = Some(selected.clone());
        state.scroll_to = Some(selected);
        intents.push(Intent::Open {
            scan: scan.clone(),
            index: items[to],
            keep_focus: true,
        });
    }
}

fn tag_ink(context: &UiContext<'_>, tag: &str) -> egui::Color32 {
    let p = &context.theme.palette;
    theme::color(match tag {
        "FIXME" | "BUG" => p.error,
        "HACK" | "XXX" => p.warning,
        _ => p.accent,
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_rows(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    shown: &TodoPanel,
    state: &mut TodoState,
    scan: &Arc<Scan>,
    rows: &[Row],
    intents: &mut Vec<Intent>,
) {
    let height = tree::row_height(context);
    let p = &context.theme.palette;
    let text_ink = theme::color(p.text);
    let muted_ink = theme::color(p.muted);
    let list = shown.layout == TodoLayout::List;
    let scroll_to = state.scroll_to.take().and_then(|target| {
        rows.iter().position(|row| match row {
            Row::Item { index, .. } => {
                let item = &scan.items[*index];
                item.relative == target.0 && item.found.line == target.1
            }
            _ => false,
        })
    });
    let mut toggle = None;
    egui::ScrollArea::vertical()
        .id_salt((panel, "todos"))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min;
            let width = viewport.width();
            ui.set_min_size(egui::vec2(width, rows.len() as f32 * height + tree::INSET));
            if let Some(at) = scroll_to {
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, at as f32 * height),
                    egui::vec2(width, height),
                );
                ui.scroll_to_rect(rect, None);
            }
            let first = (viewport.top() / height).floor().max(0.0) as usize;
            let last = ((viewport.bottom() / height).ceil() as usize + 1).min(rows.len());
            for (at, row) in rows.iter().enumerate().take(last).skip(first) {
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, at as f32 * height),
                    egui::vec2(width, height),
                );
                match row {
                    Row::Folder { path, label, depth }
                    | Row::File {
                        path, label, depth, ..
                    } => {
                        let open = !state.collapsed.contains(path);
                        let icon = match row {
                            Row::Folder { .. } if open => BuiltinIcon::FolderOpen,
                            Row::Folder { .. } => BuiltinIcon::Folder,
                            _ => bedtime_file_explorer::file_icon(std::path::Path::new(label)),
                        };
                        let tree_row = tree::Row {
                            label: Some(label),
                            depth: *depth,
                            icon: Some(icon),
                            expanded: Some(open),
                            selected: false,
                            ink: text_ink,
                            icon_ink: p.muted,
                        };
                        let item = tree::item_rect(ui, context, &tree_row, rect, tree::INDENT);
                        let id = egui::Id::new((panel, "todos-row", path));
                        let response = ui.interact(item, id, egui::Sense::click());
                        let verb = if open { "Collapse" } else { "Expand" };
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                format!("{verb} {path}"),
                            )
                        });
                        tree::paint_row(
                            ui,
                            context,
                            tree_row,
                            rect,
                            Some(&response),
                            1.0,
                            tree::INDENT,
                        );
                        if let Row::File { count, .. } = row {
                            // A pill, so it isn't read as a line number.
                            let galley = ui.painter().layout_no_wrap(
                                count.to_string(),
                                egui::FontId::proportional(context.metrics.font_size * 0.75),
                                muted_ink,
                            );
                            let pill = egui::Rect::from_center_size(
                                egui::pos2(
                                    rect.right() - 10.0 - (galley.size().x + 12.0) / 2.0,
                                    rect.center().y,
                                ),
                                egui::vec2(galley.size().x + 12.0, galley.size().y + 2.0),
                            );
                            ui.painter().rect_filled(
                                pill,
                                pill.height() / 2.0,
                                theme::color(theme::mix(p.panel_background, p.text, 0.1)),
                            );
                            ui.painter().galley(
                                pill.center() - galley.size() / 2.0,
                                galley,
                                muted_ink,
                            );
                        }
                        if response.clicked() {
                            toggle = Some(path.clone());
                        }
                    }
                    Row::Item { index, depth } => {
                        let item = &scan.items[*index];
                        let key = (item.relative.clone(), item.found.line);
                        let selected = state.selected.as_ref() == Some(&key);
                        let id = egui::Id::new((panel, "todos-item", &key));
                        let hit = rect.shrink2(egui::vec2(3.0, 1.0));
                        let response = ui.interact(hit, id, egui::Sense::click());
                        let name = format!(
                            "{} {} — {}:{}",
                            item.found.tag,
                            item.found.text,
                            item.relative,
                            item.found.line + 1
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                selected,
                                &name,
                            )
                        });
                        tree::paint_highlight(ui, ui.painter(), context, hit, &response, selected);
                        paint_item(
                            ui, context, rect, *depth, item, list, selected, text_ink, muted_ink,
                        );
                        if response.clicked() || response.double_clicked() {
                            state.selected = Some(key);
                            intents.push(Intent::Open {
                                scan: scan.clone(),
                                index: *index,
                                keep_focus: !response.double_clicked(),
                            });
                        }
                        if tree::rested(ui.ctx(), &response) {
                            let tip = match &item.found.author {
                                Some(author) => {
                                    format!("{}:{} ({author})", item.relative, item.found.line + 1)
                                }
                                None => format!("{}:{}", item.relative, item.found.line + 1),
                            };
                            tree::tooltip(ui, context, &response, tip);
                        }
                    }
                }
            }
        });
    if let Some(path) = toggle
        && !state.collapsed.remove(&path)
    {
        state.collapsed.insert(path);
    }
}

/// An item's row: its tag, its text, then where it is (the file and line in
/// the list; the line in the tree, under its file).
#[allow(clippy::too_many_arguments)]
fn paint_item(
    ui: &egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    depth: usize,
    item: &super::TodoItem,
    list: bool,
    selected: bool,
    text_ink: egui::Color32,
    muted_ink: egui::Color32,
) {
    let painter = ui.painter_at(rect);
    let size = context.metrics.font_size;
    let mut x =
        rect.left() + tree::INSET + depth as f32 * tree::INDENT + if list { 4.0 } else { 18.0 };
    let tag = painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        item.found.tag,
        egui::FontId::monospace(size * 0.78),
        tag_ink(context, item.found.tag),
    );
    x = tag.right() + 8.0;

    let place = if list {
        format!("{}:{}", item.relative, item.found.line + 1)
    } else {
        format!("Ln {}", item.found.line + 1)
    };
    let place_font = egui::FontId::proportional(size * 0.85);
    // The place gets up to half the row; the text what's left.
    let mut job = egui::text::LayoutJob::single_section(
        place,
        egui::TextFormat::simple(place_font, muted_ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: ((rect.right() - x) * 0.5).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let place = painter.layout_job(job);
    let place_x = rect.right() - 10.0 - place.size().x;
    painter.galley(
        egui::pos2(place_x, rect.center().y - place.size().y / 2.0),
        place,
        muted_ink,
    );

    let ink = if selected {
        bedtime_ui::rainbow_ink(ui, context.motion)
    } else {
        text_ink
    };
    let (text, ink) = if item.found.text.is_empty() {
        ("(no text)", muted_ink)
    } else {
        (item.found.text.as_str(), ink)
    };
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(size), ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (place_x - 10.0 - x).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
}
