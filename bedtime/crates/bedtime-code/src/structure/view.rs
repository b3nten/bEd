//! Drawing Structure: its toolbar (filter, pin, options) and its rows. What
//! the drawing asks of the editor comes back as `Intent`s, done by `apply`.
use super::{
    Reveal, StructurePanel, has_outline,
    symbols::{self, Category},
};
use crate::syntax::{AnalysisStatus, DocumentSyntax, Observation, ObservationKey};
use bedtime_ui::{BuiltinIcon, UiContext, egui, tree};
use bedtime_workbench::{DocumentReference, LayoutsWorldExt, PanelPresentation, WorkbenchWorldExt};
use bevy::prelude::*;
use std::ops::Range;

#[derive(Debug, PartialEq)]
pub(super) enum Intent {
    Reveal(u64, Reveal),
    /// Shade an entry's text in the editor this pass.
    Peek(Range<usize>, u64),
    /// Give the editor the keyboard back.
    FocusEditor,
    Pin(Option<Entity>),
}

pub(super) struct Intents {
    pub(super) editor: Option<(Entity, ObservationKey)>,
    pub(super) list: Vec<Intent>,
}

pub(super) fn apply(world: &mut World, state: &mut StructurePanel, intents: Intents) {
    for intent in intents.list {
        match intent {
            Intent::Pin(editor) => state.pinned = editor,
            Intent::FocusEditor => {
                if let Some((editor, _)) = intents.editor {
                    let _ = world.layouts_mut().focus_panel(editor);
                }
            }
            Intent::Reveal(item, how) => {
                if let Some((editor, key)) = intents.editor
                    && super::reveal_entry_as(world, editor, key, item, how)
                {
                    state.selected = Some(item);
                }
            }
            Intent::Peek(range, pass) => {
                if let Some((editor, key)) = intents.editor {
                    super::peek(world, editor, key, range, pass);
                }
            }
        }
    }
}

/// The entries shown, in the order they're shown, before any are collapsed:
/// without hidden kinds, and while filtering only matches and their parents.
pub(super) struct Arranged {
    order: Vec<usize>,
    has_children: Vec<bool>,
    /// While filtering: each entry's matched characters, `None` for a parent
    /// shown for its children.
    matches: Option<Vec<Option<Vec<usize>>>>,
}

pub(super) fn arrange(
    outline: &[crate::syntax::OutlineNode],
    hidden: u16,
    filter: &str,
    by_name: bool,
) -> Arranged {
    let count = outline.len();
    let mut excluded = vec![false; count];
    for (index, node) in outline.iter().enumerate() {
        excluded[index] = hidden & Category::of(&node.kind).bit() != 0
            || node.parent.is_some_and(|parent| excluded[parent]);
    }
    let filtering = !filter.trim().is_empty();
    let matches: Option<Vec<_>> = filtering.then(|| {
        outline
            .iter()
            .map(|node| bedtime_ui::picker::fuzzy(&node.label, filter).map(|(_, at)| at))
            .collect()
    });
    let mut kept: Vec<bool> = (0..count)
        .map(|index| {
            !excluded[index]
                && matches
                    .as_ref()
                    .is_none_or(|matches| matches[index].is_some())
        })
        .collect();
    if filtering {
        // Children come after parents: a kept child keeps its parent.
        for index in (0..count).rev() {
            if kept[index]
                && let Some(parent) = outline[index].parent
                && !excluded[parent]
            {
                kept[parent] = true;
            }
        }
    }
    let mut children = vec![Vec::new(); count];
    let mut roots = Vec::new();
    for index in (0..count).filter(|&index| kept[index]) {
        match outline[index].parent {
            Some(parent) => children[parent].push(index),
            None => roots.push(index),
        }
    }
    if by_name {
        let key = |&index: &usize| (outline[index].label.to_lowercase(), index);
        roots.sort_by_key(key);
        for list in &mut children {
            list.sort_by_key(key);
        }
    }
    let mut order = Vec::with_capacity(count);
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(index) = stack.pop() {
        order.push(index);
        stack.extend(children[index].iter().rev());
    }
    Arranged {
        order,
        has_children: children.iter().map(|list| !list.is_empty()).collect(),
        matches,
    }
}

pub(super) fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    state: &mut StructurePanel,
) -> Intents {
    let mut intents = Intents {
        editor: state
            .shown
            .as_ref()
            .map(|(editor, observation)| (*editor, observation.key)),
        list: Vec::new(),
    };
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    let ctx = ui.ctx().clone();
    let quiet = ui.is_enabled()
        && !ctx.any_popup_open()
        && ctx.memory(|memory| memory.top_modal_layer().is_none());
    let filter_id = egui::Id::new((panel, "structure-filter"));
    let filter_focused = ctx.memory(|memory| memory.has_focus(filter_id)) || state.filter_had_focus;
    // The list has the keys while the panel's focused and nothing's typed into.
    let list_keys = focused && quiet && !ctx.text_edit_focused() && !filter_focused;
    let field_keys = quiet && filter_focused;

    let shown = state.shown.clone();
    let arrange_now = |state: &StructurePanel| {
        shown.as_ref().map(|(_, observation)| {
            arrange(
                &observation.outline,
                state.hidden,
                &state.filter,
                state.sort_by_name,
            )
        })
    };
    // Keys first: the filter field would take Enter and Escape.
    if (list_keys || field_keys)
        && let (Some((_, observation)), Some(arranged)) = (&shown, arrange_now(state))
    {
        let page = (ui.available_height() / tree::row_height(context)).max(1.0) as usize;
        keyboard(
            ui,
            page,
            state,
            observation,
            &arranged,
            list_keys,
            filter_id,
            &mut intents,
        );
    }
    let collapse_all = toolbar(ui, world, context, state, filter_id, &mut intents);
    state.filter_had_focus = ctx.memory(|memory| memory.has_focus(filter_id));
    let arranged = arrange_now(state);
    if let (Some(collapse), Some((_, observation)), Some(arranged)) =
        (collapse_all, &shown, &arranged)
    {
        if collapse {
            state.collapsed = arranged
                .order
                .iter()
                .filter(|&&index| arranged.has_children[index])
                .map(|&index| observation.outline[index].id)
                .collect();
        } else {
            state.collapsed.clear();
        }
    }

    let (Some((editor, observation)), Some(arranged)) = (shown, arranged) else {
        let message = empty_message(world, state);
        egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
            ui.weak(message);
        });
        return intents;
    };
    draw_rows(
        ui,
        context,
        panel,
        state,
        editor,
        &observation,
        &arranged,
        &mut intents,
    );
    intents
}

/// The filter field, the pin and the options menu. Returns Collapse All
/// (`true`) or Expand All (`false`) when chosen.
fn toolbar(
    ui: &mut egui::Ui,
    world: &World,
    context: &UiContext<'_>,
    state: &mut StructurePanel,
    filter_id: egui::Id,
    intents: &mut Intents,
) -> Option<bool> {
    let mut collapse = None;
    let height = tree::row_height(context);
    let categories: Vec<Category> = state.shown.as_ref().map_or_else(Vec::new, |(_, o)| {
        let present: std::collections::HashSet<Category> = o
            .outline
            .iter()
            .map(|node| Category::of(&node.kind))
            .collect();
        Category::ALL
            .into_iter()
            .filter(|category| present.contains(category) || state.hidden & category.bit() != 0)
            .collect()
    });
    let pinned_title = state.pinned.and_then(|editor| title_of(world, editor));
    let following = world.resource::<super::StructureSource>().0;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 8,
            right: 6,
            top: 4,
            bottom: 4,
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
                    context.frame(ui).tooltip(&options, "Structure options");
                    egui::Popup::menu(&options)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.set_min_width(220.0);
                            let mut follow = !state.ignore_caret;
                            ui.checkbox(&mut follow, "Follow the caret");
                            state.ignore_caret = !follow;
                            ui.separator();
                            ui.weak("Sort by");
                            ui.radio_value(&mut state.sort_by_name, false, "Position");
                            ui.radio_value(&mut state.sort_by_name, true, "Name");
                            if !categories.is_empty() {
                                ui.separator();
                                ui.weak("Show");
                                for category in &categories {
                                    let mut show = state.hidden & category.bit() == 0;
                                    if ui.checkbox(&mut show, category.label()).changed() {
                                        state.hidden ^= category.bit();
                                    }
                                }
                            }
                            ui.separator();
                            if ui.button("Collapse All").clicked() {
                                collapse = Some(true);
                                ui.close();
                            }
                            if ui.button("Expand All").clicked() {
                                collapse = Some(false);
                                ui.close();
                            }
                        });

                    let pin = context.frame(ui).icon_button_sized(BuiltinIcon::Pin, 16.0);
                    if let Some(title) = &pinned_title {
                        ui.painter().rect_stroke(
                            pin.rect,
                            4.0,
                            egui::Stroke::new(
                                1.0,
                                bedtime_ui::theme::color(context.theme.palette.accent),
                            ),
                            egui::StrokeKind::Inside,
                        );
                        context.frame(ui).tooltip(
                            &pin,
                            format!("Showing {title} — click to follow the focused editor"),
                        );
                    } else {
                        context
                            .frame(ui)
                            .tooltip(&pin, "Keep showing this editor's structure");
                    }
                    if pin.clicked() {
                        intents.list.push(Intent::Pin(if state.pinned.is_some() {
                            None
                        } else {
                            state
                                .shown
                                .as_ref()
                                .map(|(editor, _)| *editor)
                                .or(following)
                        }));
                    }

                    if !state.filter.is_empty() {
                        let clear = context
                            .frame(ui)
                            .icon_button_sized(BuiltinIcon::Close, 14.0);
                        context.frame(ui).tooltip(&clear, "Clear the filter");
                        if clear.clicked() {
                            state.filter.clear();
                        }
                    }
                    let width = ui.available_width();
                    let field = context.frame(ui).text_edit(
                        egui::TextEdit::singleline(&mut state.filter)
                            .id(filter_id)
                            .hint_text("Filter")
                            .desired_width(width),
                    );
                    field.response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Filter")
                    });
                    if std::mem::take(&mut state.focus_filter) {
                        field.response.request_focus();
                        // Typing started in the list: the caret goes at the end.
                        let end = egui::text::CCursor::new(state.filter.chars().count());
                        let mut text_state = field.state;
                        text_state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(end)));
                        text_state.store(ui.ctx(), filter_id);
                    }
                },
            );
        });
    collapse
}

/// Why nothing's shown.
fn empty_message(world: &World, state: &StructurePanel) -> String {
    let editor = state
        .pinned
        .or(world.resource::<super::StructureSource>().0);
    if let Some(editor) = editor {
        let status = world
            .get::<DocumentReference>(editor)
            .and_then(|reference| world.get::<DocumentSyntax>(reference.document().entity()))
            .map(|syntax| syntax.session.status());
        return match status {
            Some(AnalysisStatus::Unsupported) => "No structure for this kind of file".into(),
            Some(AnalysisStatus::Limited(why)) => format!("No structure: {why}"),
            Some(AnalysisStatus::Failed(why)) => format!("The structure couldn't be read: {why}"),
            _ => "Reading the structure…".into(),
        };
    }
    let focused_editor = world
        .layouts()
        .active()
        .and_then(|layout| world.layouts().focused_panel(layout))
        .filter(|&panel| world.get::<crate::editor::EditorPanel>(panel).is_some());
    match focused_editor {
        Some(panel) if !has_outline(world, panel) => "No structure for this kind of file".into(),
        _ => "Open a file to see its structure".into(),
    }
}

fn title_of(world: &World, editor: Entity) -> Option<String> {
    let document = world.get::<DocumentReference>(editor)?.document();
    Some(world.workbench().inspect_document(document).ok()?.title)
}

/// The rows the keyboard moves through: shown, under open parents.
fn reachable(state: &StructurePanel, observation: &Observation, arranged: &Arranged) -> Vec<usize> {
    let filtering = arranged.matches.is_some();
    let mut rows = Vec::with_capacity(arranged.order.len());
    // The depth below which rows are inside a closed parent.
    let mut closed_at: Option<usize> = None;
    for &index in &arranged.order {
        let node = &observation.outline[index];
        if closed_at.is_some_and(|depth| node.depth > depth) {
            continue;
        }
        closed_at = None;
        rows.push(index);
        if !filtering && arranged.has_children[index] && state.collapsed.contains(&node.id) {
            closed_at = Some(node.depth);
        }
    }
    rows
}

/// Arrows, Home/End and Page Up/Down move the selection, showing it in the
/// editor; Right and Left open and close; Enter goes to the editor; typing
/// filters. From the filter field: Up/Down, Page Up/Down, Enter and Escape.
#[allow(clippy::too_many_arguments)]
pub(super) fn keyboard(
    ui: &mut egui::Ui,
    page: usize,
    state: &mut StructurePanel,
    observation: &Observation,
    arranged: &Arranged,
    list: bool,
    filter_id: egui::Id,
    intents: &mut Intents,
) {
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return;
    }
    let pressed = |ui: &mut egui::Ui, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    let rows = reachable(state, observation, arranged);
    let outline = &observation.outline;
    let current = state
        .selected
        .and_then(|id| rows.iter().position(|&index| outline[index].id == id));
    let at = current.unwrap_or(0);
    let last = rows.len().saturating_sub(1);

    if pressed(ui, egui::Key::Escape) {
        if !list && !state.filter.is_empty() {
            state.filter.clear();
        } else {
            ui.memory_mut(|memory| memory.surrender_focus(filter_id));
            intents.list.push(Intent::FocusEditor);
        }
        return;
    }
    if rows.is_empty() {
        return;
    }
    if pressed(ui, egui::Key::Enter) {
        let index = rows[at];
        intents
            .list
            .push(Intent::Reveal(outline[index].id, Reveal::Focus));
        return;
    }
    let moved = if pressed(ui, egui::Key::ArrowDown) {
        Some(current.map_or(0, |at| (at + 1).min(last)))
    } else if pressed(ui, egui::Key::ArrowUp) {
        Some(at.saturating_sub(1))
    } else if pressed(ui, egui::Key::PageDown) {
        Some((at + page).min(last))
    } else if pressed(ui, egui::Key::PageUp) {
        Some(at.saturating_sub(page))
    } else if list && pressed(ui, egui::Key::Home) {
        Some(0)
    } else if list && pressed(ui, egui::Key::End) {
        Some(last)
    } else {
        None
    };
    let target = if moved.is_some() || !list {
        moved
    } else {
        let index = rows[at];
        let node = &outline[index];
        let parent_row = || {
            node.parent
                .and_then(|parent| rows.iter().position(|&row| row == parent))
        };
        let open = arranged.has_children[index]
            && (arranged.matches.is_some() || !state.collapsed.contains(&node.id));
        if pressed(ui, egui::Key::ArrowRight) {
            if arranged.has_children[index] && !open {
                state.collapsed.remove(&node.id);
                None
            } else {
                rows.get(at + 1)
                    .filter(|&&next| open && outline[next].parent == Some(index))
                    .map(|_| at + 1)
            }
        } else if pressed(ui, egui::Key::ArrowLeft) {
            if open && arranged.matches.is_none() {
                state.collapsed.insert(node.id);
                None
            } else {
                parent_row()
            }
        } else {
            type_to_filter(ui, state);
            None
        }
    };
    if let Some(row) = target {
        let id = outline[rows[row]].id;
        state.selected = Some(id);
        state.scroll_to = Some(id);
        intents.list.push(Intent::Reveal(id, Reveal::Preview));
    }
}

/// Letters typed in the list start a filter, in the filter field.
fn type_to_filter(ui: &mut egui::Ui, state: &mut StructurePanel) {
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
    if typed.is_empty() || typed.chars().any(char::is_control) || typed.trim().is_empty() {
        return;
    }
    ui.input_mut(|input| {
        input
            .events
            .retain(|event| !matches!(event, egui::Event::Text(_)))
    });
    state.filter.push_str(&typed);
    state.focus_filter = true;
}

/// The outline's rows, with animated collapsing.
#[allow(clippy::too_many_arguments)]
fn draw_rows(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    state: &mut StructurePanel,
    editor: Entity,
    observation: &Observation,
    arranged: &Arranged,
    intents: &mut Intents,
) {
    let outline = &observation.outline;
    let details = state.details.of(observation);
    let filtering = arranged.matches.is_some();
    let height = tree::row_height(context);
    // Where each visible row goes, as its ancestors open and close: work it
    // out once, then draw only the rows in view.
    let mut rows = Vec::new();
    let mut ancestors: Vec<(usize, f32)> = Vec::new();
    let mut top = 0.0;
    for &index in &arranged.order {
        let node = &outline[index];
        while ancestors
            .last()
            .is_some_and(|(depth, _)| *depth >= node.depth)
        {
            ancestors.pop();
        }
        let visibility = ancestors.last().map_or(1.0, |(_, visibility)| *visibility);
        let openness = if arranged.has_children[index] {
            bedtime_ui::animate_expansion(
                ui.ctx(),
                egui::Id::new((panel, editor, node.id, "structure-expansion")),
                filtering || !state.collapsed.contains(&node.id),
                context.motion,
            )
        } else {
            1.0
        };
        if visibility > 0.0 {
            rows.push((index, top, height * visibility));
        }
        top += height * visibility;
        ancestors.push((node.depth, visibility * openness));
    }
    let mut toggle = None;
    let pass = ui.ctx().cumulative_pass_nr();
    let scroll_to = state.scroll_to.take();
    let text_ink = bedtime_ui::theme::color(context.theme.palette.text);
    let muted_ink = bedtime_ui::theme::color(context.theme.palette.muted);
    let accent = bedtime_ui::theme::color(context.theme.palette.accent);
    // Rows as the file tree draws them: a long name is cut short at the
    // panel's edge, rather than the panel scrolling sideways.
    egui::ScrollArea::vertical()
        .id_salt((panel, "structure"))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            if rows.is_empty() {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    if filtering {
                        ui.weak(format!("Nothing matches “{}”", state.filter.trim()));
                    } else if outline.is_empty() {
                        ui.weak("No declarations");
                    } else {
                        ui.weak("Everything here is hidden (see the options)");
                    }
                });
            }
            let origin = ui.cursor().min;
            ui.set_min_size(egui::vec2(viewport.width(), top));
            if let Some(target) = scroll_to
                && let Some((_, y, row_height)) = rows
                    .iter()
                    .find(|(index, _, _)| outline[*index].id == target)
            {
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, *y),
                    egui::vec2(viewport.width(), *row_height),
                );
                ui.scroll_to_rect(rect, None);
            }
            let first = rows.partition_point(|(_, y, h)| y + h < viewport.top());
            for &(index, y, row_height) in &rows[first..] {
                if y > viewport.bottom() {
                    break;
                }
                let node = &outline[index];
                let category = Category::of(&node.kind);
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, y),
                    egui::vec2(viewport.width(), row_height),
                );
                let id = egui::Id::new((panel, editor, node.id, "structure-row"));
                // The chevron toggles; the rest of the row reveals.
                let x = rect.left() + tree::INSET + node.depth as f32 * tree::INDENT;
                let arrow = egui::Rect::from_min_max(
                    egui::pos2(x, rect.top()),
                    egui::pos2(x + 16.0, rect.bottom()),
                );
                let children = arranged.has_children[index];
                let open = filtering || !state.collapsed.contains(&node.id);
                let selected = state.selected == Some(node.id);
                let matched = arranged
                    .matches
                    .as_ref()
                    .map(|matches| matches[index].as_deref());
                let row = tree::Row {
                    label: Some(&node.label),
                    depth: node.depth,
                    icon: Some(category.icon()),
                    expanded: children.then_some(open),
                    selected,
                    // A parent shown only for its matching children is dimmed.
                    ink: if matched.is_some_and(|m| m.is_none()) {
                        muted_ink
                    } else {
                        text_ink
                    },
                    icon_ink: category.ink(&context.theme.palette.syntax),
                };
                // Only the item takes the pointer, not the empty space after
                // it. The item first: the chevron, registered after, gets its
                // clicks.
                let item = tree::item_rect(ui, context, &row, rect, tree::INDENT);
                let response = ui.interact(item, id, egui::Sense::click());
                let arrow_response = (children && !filtering).then(|| {
                    let response = ui.interact(arrow, id.with("arrow"), egui::Sense::click());
                    let verb = if open { "Collapse" } else { "Expand" };
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            format!("{verb} {}", node.label),
                        )
                    });
                    response
                });
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        &node.label,
                    )
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(rect));
                    let depth = row.depth;
                    tree::paint_row(ui, context, row, rect, Some(&response), 1.0, tree::INDENT);
                    paint_matches(
                        ui,
                        context,
                        rect,
                        depth,
                        &node.label,
                        matched.flatten(),
                        accent,
                    );
                });
                if arrow_response.as_ref().is_some_and(|arrow| arrow.clicked()) {
                    toggle = Some(node.id);
                } else if response.clicked() {
                    intents.list.push(Intent::Reveal(node.id, Reveal::Focus));
                }
                if response.hovered() || arrow_response.as_ref().is_some_and(|a| a.hovered()) {
                    intents.list.push(Intent::Peek(node.range.clone(), pass));
                }
                row_menu(observation, index, &response, intents);
                let detail = &details[index];
                let mut tip = format!("{} · line {}", node.kind, detail.line + 1);
                if !detail.text.is_empty() {
                    tip = format!("{}{}\n{tip}", node.label, detail.text);
                }
                tree::tooltip(ui, context, &response, tip);
            }
        });
    if let Some(id) = toggle
        && !state.collapsed.remove(&id)
    {
        state.collapsed.insert(id);
    }
}

/// The matched characters of a name, in the accent color over it. The text
/// after the name (its parameters, its type) shows only in the tooltip.
fn paint_matches(
    ui: &egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    depth: usize,
    label: &str,
    matched: Option<&[usize]>,
    accent: egui::Color32,
) {
    let Some(matched) = matched.filter(|m| !m.is_empty()) else {
        return;
    };
    // Laid out as `tree::paint_row` lays out a name after an icon, so the
    // characters land on its.
    let x = rect.left() + tree::INSET + depth as f32 * tree::INDENT;
    let side = context.metrics.font_size.min(18.0);
    let name_x = x + 18.0 + side + 5.0;
    let font = egui::FontId::proportional(context.metrics.font_size);
    let mut job = egui::text::LayoutJob::default();
    let marked: std::collections::HashSet<usize> = matched.iter().copied().collect();
    for (at, c) in label.chars().enumerate() {
        let color = if marked.contains(&at) {
            accent
        } else {
            egui::Color32::TRANSPARENT
        };
        let mut buffer = [0; 4];
        job.append(
            c.encode_utf8(&mut buffer),
            0.0,
            egui::TextFormat::simple(font.clone(), color),
        );
    }
    job.wrap.max_width = (rect.right() - x - 42.0).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter().galley(
        egui::pos2(name_x, rect.center().y - galley.size().y / 2.0),
        galley,
        accent,
    );
}

/// An entry's right-click menu.
fn row_menu(
    observation: &Observation,
    index: usize,
    response: &egui::Response,
    intents: &mut Intents,
) {
    let outline = &observation.outline;
    let node = &outline[index];
    response.context_menu(|ui| {
        if ui.button("Go to").clicked() {
            intents.list.push(Intent::Reveal(node.id, Reveal::Focus));
            ui.close();
        }
        if ui.button("Select").clicked() {
            intents
                .list
                .push(Intent::Reveal(node.id, Reveal::SelectAll));
            ui.close();
        }
        if ui.button("Fold").clicked() {
            intents.list.push(Intent::Reveal(node.id, Reveal::Fold));
            ui.close();
        }
        ui.separator();
        if ui.button("Copy Name").clicked() {
            ui.ctx().copy_text(node.label.clone());
            ui.close();
        }
        if ui.button("Copy Path").clicked() {
            let mut names: Vec<&str> = symbols::ancestry(outline, index)
                .map(|at| outline[at].label.as_str())
                .collect();
            names.reverse();
            ui.ctx()
                .copy_text(names.join(symbols::path_separator(observation.language)));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::OutlineNode;

    fn node(label: &str, kind: &str, parent: Option<usize>) -> OutlineNode {
        OutlineNode {
            id: label.len() as u64,
            label: label.into(),
            kind: kind.into(),
            range: 0..0,
            name_range: 0..0,
            parent,
            depth: parent.map_or(0, |_| 1),
        }
    }

    #[test]
    fn filtering_keeps_matches_and_their_parents() {
        let outline = vec![
            node("Cat", "type", None),
            node("meow", "method", Some(0)),
            node("age", "field", Some(0)),
            node("dog", "function", None),
        ];
        let all = arrange(&outline, 0, "", false);
        assert_eq!(all.order, vec![0, 1, 2, 3]);
        assert!(all.has_children[0] && !all.has_children[3]);
        let meow = arrange(&outline, 0, "mw", false);
        assert_eq!(meow.order, vec![0, 1]);
        let matches = meow.matches.unwrap();
        assert!(matches[0].is_none() && matches[1].is_some());
        let by_name = arrange(&outline, 0, "", true);
        assert_eq!(by_name.order, vec![0, 2, 1, 3]);
        let no_fields = arrange(&outline, Category::Field.bit(), "", false);
        assert_eq!(no_fields.order, vec![0, 1, 3]);
        let no_types = arrange(&outline, Category::Type.bit(), "", false);
        assert_eq!(no_types.order, vec![3]);
    }
}
