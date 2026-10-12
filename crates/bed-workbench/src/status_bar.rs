//! One status row. Features own content; this renderer owns sizing and requests.
use super::{UiLoan, WorkbenchLayout, WorkbenchRequests, WorkbenchViewport, api};
use api::{Action, StatusContext, StatusDrawContext, StatusDrawMode, StatusItem, StatusItems};
use bevy::prelude::*;
use dear_imgui_rs::{Condition, StyleColor, StyleVar, TextureId, Ui, WindowFlags, sys};
use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};

const PADDING: [f32; 2] = [10.0, 4.0];
const GAP: f32 = 12.0;
type Key = (Entity, &'static str);

/// Application-owned information is independent of the contribution collection.
pub struct StatusBarHost<'a> {
    pub summary: &'a str,
    pub details: &'a str,
}
pub type StatusIconResolver = fn(&World, &str) -> Option<TextureId>;

#[derive(Resource, Default)]
struct StatusBarState {
    failures: HashMap<Key, String>,
    popup: Option<(Key, StatusContext)>,
    overflow_contexts: HashMap<Key, StatusContext>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Representation {
    Inline,
    Icon,
    Overflow,
}

struct Entry {
    item: StatusItem,
    context: StatusContext,
    width: f32,
    icon: Option<TextureId>,
    representation: Representation,
}
impl Entry {
    fn key(&self) -> Key {
        (self.context.publisher, self.item.id)
    }
    fn shown_width(&self, icon_width: f32) -> f32 {
        match self.representation {
            Representation::Inline => self.width,
            Representation::Icon => icon_width,
            Representation::Overflow => 0.0,
        }
    }
}

pub fn status_bar_height(ui: &Ui) -> f32 {
    ui.text_line_height() + PADDING[1] * 2.0
}

fn context(world: &World, publisher: Entity, item_id: &'static str) -> StatusContext {
    let layout = world.resource::<WorkbenchLayout>();
    let owner = api::panel_owner(world, publisher);
    let area = owner
        .or(layout.focused)
        .and_then(|panel| world.get::<api::PanelPresentation>(panel))
        .and_then(|panel| layout.tiling.area_for(panel.id))
        .or(layout.focused_area)
        .unwrap_or(layout.tiling.layout.areas[0].id);
    StatusContext {
        publisher,
        item_id,
        focused_panel: layout.focused,
        focused_frontend: layout.focused.map(|panel| {
            world
                .get::<api::PanelFrontend>(panel)
                .map_or(panel, |front| front.0)
        }),
        source: owner.map(|_| publisher),
        area,
    }
}

/// Recover the entry scopes after each callback, preserving healthy producers.
fn callback<T>(ui: &Ui, call: impl FnOnce() -> T) -> Result<T, String> {
    let mut stacks = sys::ImGuiErrorRecoveryState::default();
    unsafe { sys::igErrorRecoveryStoreState(&mut stacks) };
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            unsafe {
                let io = sys::igGetIO_Nil();
                let assert = (*io).ConfigErrorRecoveryEnableAssert;
                (*io).ConfigErrorRecoveryEnableAssert = false;
                sys::igErrorRecoveryTryToRecoverState(&stacks);
                (*io).ConfigErrorRecoveryEnableAssert = assert;
            }
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-text panic");
            let _ = ui;
            Err(format!("Status widget panicked: {detail}"))
        }
    }
}

fn collect(
    world: &mut World,
    ui: &Ui,
    icons: StatusIconResolver,
    state: &mut StatusBarState,
) -> Vec<Entry> {
    // Copy descriptors before lending the world to a producer's callback.
    let publishers = world
        .query::<(Entity, &StatusItems)>()
        .iter(world)
        .map(|(entity, items)| (entity, items.0.clone()))
        .collect::<Vec<_>>();
    let mut alive = HashSet::new();
    let mut entries = Vec::new();
    for (publisher, items) in publishers {
        let mut ids = HashSet::new();
        for item in items {
            let key = (publisher, item.id);
            alive.insert(key);
            if !ids.insert(item.id) {
                state
                    .failures
                    .insert(key, format!("Duplicate status item ID: {}", item.id));
                continue;
            }
            let context = context(world, publisher, item.id);
            let measured = if state.failures.contains_key(&key) {
                Some(ui.calc_text_size(format!("{}: unavailable", item.label))[0])
            } else {
                match callback(ui, || (item.measure)(world, ui, context)) {
                    Ok(Some(width)) if !width.is_finite() || width < 0.0 => {
                        state
                            .failures
                            .insert(key, "Status width must be finite and nonnegative".into());
                        Some(ui.calc_text_size(format!("{}: unavailable", item.label))[0])
                    }
                    Ok(width) => width,
                    Err(error) => {
                        state.failures.insert(key, error);
                        Some(ui.calc_text_size(format!("{}: unavailable", item.label))[0])
                    }
                }
            };
            if let Some(width) = measured {
                let icon = item.icon.and_then(|key| icons(world, key));
                entries.push(Entry {
                    item,
                    context,
                    width: width.max(1.0),
                    icon,
                    representation: if item.prefer_icon && icon.is_some() {
                        Representation::Icon
                    } else {
                        Representation::Inline
                    },
                });
            }
        }
    }
    state.failures.retain(|key, _| alive.contains(key));
    entries.sort_by(|a, b| {
        b.item
            .priority
            .cmp(&a.item.priority)
            .then_with(|| a.item.id.cmp(b.item.id))
            .then_with(|| {
                a.context
                    .publisher
                    .to_bits()
                    .cmp(&b.context.publisher.to_bits())
            })
    });
    entries
}

fn total_width(entries: &[Entry], icon_width: f32, overflow_width: f32) -> f32 {
    let visible = entries
        .iter()
        .filter(|entry| entry.representation != Representation::Overflow)
        .count();
    let overflow = entries
        .iter()
        .any(|entry| entry.representation == Representation::Overflow);
    let count = visible + usize::from(overflow);
    entries
        .iter()
        .map(|entry| entry.shown_width(icon_width))
        .sum::<f32>()
        + if overflow { overflow_width } else { 0.0 }
        + count.saturating_sub(1) as f32 * GAP
}

fn allocate(entries: &mut [Entry], budget: f32, icon_width: f32, overflow_width: f32) {
    // Exhaust a lower-priority item's representations before shrinking a
    // higher-priority widget. Iconless items follow the same ordering.
    for index in (0..entries.len()).rev() {
        if total_width(entries, icon_width, overflow_width) <= budget {
            break;
        }
        if entries[index].representation == Representation::Inline
            && entries[index].icon.is_some()
            && entries[index].width > icon_width
        {
            entries[index].representation = Representation::Icon;
        }
        if total_width(entries, icon_width, overflow_width) > budget {
            entries[index].representation = Representation::Overflow;
        }
    }
}

fn fit_text(ui: &Ui, text: &str, width: f32) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if ui.calc_text_size(&text)[0] <= width {
        return text;
    }
    if ui.calc_text_size("…")[0] > width {
        return String::new();
    }
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let (mut low, mut high) = (0, boundaries.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if ui.calc_text_size(format!("{}…", &text[..boundaries[mid]]))[0] <= width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("{}…", &text[..boundaries[low]])
}

fn draw_entry(
    world: &mut World,
    ui: &Ui,
    entry: &Entry,
    ctx: StatusDrawContext,
    state: &mut StatusBarState,
    actions: &mut Vec<Action>,
) {
    if let Some(error) = state.failures.get(&entry.key()) {
        ui.text_wrapped(format!("{}: unavailable\n{error}", entry.item.label));
        return;
    }
    let mut produced = Vec::new();
    match callback(ui, || (entry.item.draw)(world, ui, ctx, &mut produced)) {
        Ok(()) => actions.extend(produced),
        Err(error) => {
            state.failures.insert(entry.key(), error);
        }
    }
}

fn tooltip(world: &World, ui: &Ui, entry: &Entry, state: &mut StatusBarState) {
    ui.tooltip(|| {
        if let Some(error) = state.failures.get(&entry.key()) {
            ui.text_wrapped(error);
        } else if let Err(error) = callback(ui, || (entry.item.tooltip)(world, ui, entry.context)) {
            state.failures.insert(entry.key(), error);
        }
    });
}

/// Draw after panel surfaces. Consequential actions run after all UI scopes end.
pub fn render_status_bar(
    world: &mut World,
    ui: &Ui,
    host: StatusBarHost<'_>,
    icons: StatusIconResolver,
) {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "status frontend is not nested"
    );
    world.insert_resource(UiLoan);
    let result = catch_unwind(AssertUnwindSafe(|| draw(world, ui, host, icons)));
    world.remove_resource::<UiLoan>().unwrap();
    match result {
        Ok(actions) => {
            for action in actions {
                world.resource_mut::<WorkbenchRequests>().push(action);
            }
        }
        Err(panic) => resume_unwind(panic),
    }
}

fn draw(
    world: &mut World,
    ui: &Ui,
    host: StatusBarHost<'_>,
    icons: StatusIconResolver,
) -> Vec<Action> {
    let viewport = *world.resource::<WorkbenchViewport>();
    let main = ui.main_viewport();
    let (origin, size) = viewport
        .bounds
        .unwrap_or((main.work_pos(), main.work_size()));
    let available = (size[1] - viewport.top_inset.clamp(0.0, size[1])).max(0.0);
    let height = viewport.bottom_inset.clamp(0.0, available);
    if height <= 0.0 || size[0] <= 0.0 {
        return Vec::new();
    }
    let mut state = world
        .remove_resource::<StatusBarState>()
        .unwrap_or_default();
    let mut entries = collect(world, ui, icons, &mut state);
    let mut actions = Vec::new();
    let position = [origin[0], origin[1] + size[1] - height];
    let _padding = ui.push_style_var(StyleVar::WindowPadding(PADDING));
    let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
    let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
    let _minimum = ui.push_style_var(StyleVar::WindowMinSize([0.0; 2]));
    let _spacing = ui.push_style_var(StyleVar::ItemSpacing([GAP, 0.0]));
    let _background =
        ui.push_style_color(StyleColor::WindowBg, ui.style_color(StyleColor::MenuBarBg));
    ui.set_next_window_viewport(main.id());
    ui.window("##bed_status_bar")
        .position(position, Condition::Always)
        .size([size[0], height], Condition::Always)
        .flags(
            WindowFlags::NO_DECORATION
                | WindowFlags::NO_DOCKING
                | WindowFlags::NO_SCROLLBAR
                | WindowFlags::NO_SCROLL_WITH_MOUSE
                | WindowFlags::NO_FOCUS_ON_APPEARING
                | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
                | WindowFlags::NO_NAV,
        )
        .build(|| {
            ui.get_window_draw_list()
                .add_line(
                    position,
                    [position[0] + size[0], position[1]],
                    ui.style_color(StyleColor::Separator),
                )
                .build();
            let width = ui.content_region_avail()[0].max(0.0);
            let host_width = ui.calc_text_size(host.summary)[0].min(width * 0.4);
            let left_width = (width - host_width - GAP).max(0.0);
            let side = ui.text_line_height();
            let overflow_width = ui.calc_text_size("…")[0] + 8.0;
            allocate(&mut entries, left_width, side, overflow_width);
            let start = ui.cursor_screen_pos();
            let mut x = start[0];
            let mut open_widget = false;
            for entry in entries
                .iter()
                .filter(|entry| entry.representation != Representation::Overflow)
            {
                let slot_width = entry
                    .shown_width(side)
                    .min((start[0] + left_width - x).max(0.0));
                if slot_width <= 0.0 {
                    continue;
                }
                ui.set_cursor_screen_pos([x, start[1]]);
                let id = format!("{}/{}", entry.context.publisher.to_bits(), entry.item.id);
                let _id = ui.push_id(&id);
                let hovered;
                {
                    let _clip =
                        ui.push_clip_rect([x, start[1]], [x + slot_width, start[1] + side], true);
                    if entry.representation == Representation::Icon {
                        if ui.invisible_button("compact", [slot_width, side]) {
                            state.popup = Some((entry.key(), entry.context));
                            open_widget = true;
                            // Open at the footer scope, independent of the item's representation.
                        }
                        if let Some(icon) = entry.icon {
                            ui.get_window_draw_list().add_image(
                                icon,
                                [x, start[1]],
                                [x + side, start[1] + side],
                                [0.0; 2],
                                [1.0; 2],
                                ui.style_color(StyleColor::TextDisabled),
                            );
                        }
                        hovered = ui.is_item_hovered();
                    } else {
                        // Ordinary buttons fit the same single-line slot as text.
                        let _frame_padding = ui.push_style_var(StyleVar::FramePadding([
                            ui.clone_style().frame_padding()[0],
                            0.0,
                        ]));
                        draw_entry(
                            world,
                            ui,
                            entry,
                            StatusDrawContext {
                                context: entry.context,
                                mode: StatusDrawMode::Inline,
                                width: slot_width,
                            },
                            &mut state,
                            &mut actions,
                        );
                        hovered = ui.is_mouse_hovering_rect(
                            [x, start[1]],
                            [x + slot_width, start[1] + side],
                        );
                    }
                }
                if hovered {
                    tooltip(world, ui, entry, &mut state);
                }
                x += slot_width + GAP;
            }
            let hidden = entries
                .iter()
                .filter(|entry| entry.representation == Representation::Overflow)
                .collect::<Vec<_>>();
            if !hidden.is_empty() && left_width > 0.0 {
                ui.set_cursor_screen_pos([
                    x.min(start[0] + (left_width - overflow_width).max(0.0)),
                    start[1],
                ]);
                let point = ui.cursor_screen_pos();
                if ui.invisible_button("overflow", [overflow_width.min(left_width), side]) {
                    state.overflow_contexts = hidden
                        .iter()
                        .map(|entry| (entry.key(), entry.context))
                        .collect();
                    ui.open_popup("status-overflow");
                }
                ui.get_window_draw_list().add_text(
                    point,
                    ui.style_color(StyleColor::TextDisabled),
                    "…",
                );
                if ui.is_item_hovered() {
                    ui.tooltip(|| ui.text(format!("{} more status items", hidden.len())));
                }
            }
            ui.set_cursor_screen_pos([start[0] + width - host_width, start[1]]);
            if host_width > 0.0 {
                ui.invisible_button("host", [host_width, side]);
                ui.get_window_draw_list().add_text(
                    [start[0] + width - host_width, start[1]],
                    ui.style_color(StyleColor::TextDisabled),
                    fit_text(ui, host.summary, host_width),
                );
                if ui.is_item_hovered() {
                    ui.tooltip(|| ui.text_wrapped(host.details));
                }
            }
            // A single popup identity survives icon/inline/overflow layout changes.
            if open_widget {
                ui.open_popup("status-widget");
            }
            popup_constraints(size);
            if let Some(_popup) = ui.begin_popup("status-widget") {
                if let Some((key, captured)) = state.popup
                    && let Some(entry) = entries.iter().find(|entry| entry.key() == key)
                {
                    let id = format!("{}/{}", key.0.to_bits(), key.1);
                    let _id = ui.push_id(&id);
                    draw_entry(
                        world,
                        ui,
                        entry,
                        StatusDrawContext {
                            context: captured,
                            mode: StatusDrawMode::Popup,
                            width: ui.content_region_avail()[0]
                                .max(entry.width)
                                .min(size[0] * 0.85),
                        },
                        &mut state,
                        &mut actions,
                    );
                } else {
                    ui.close_current_popup();
                    state.popup = None;
                }
            } else if !open_widget {
                state.popup = None;
            }
            popup_constraints(size);
            if let Some(_popup) = ui.begin_popup("status-overflow") {
                if hidden.is_empty() {
                    ui.close_current_popup();
                }
                for entry in hidden {
                    let id = format!("{}/{}", entry.context.publisher.to_bits(), entry.item.id);
                    let _id = ui.push_id(&id);
                    ui.text(entry.item.label);
                    draw_entry(
                        world,
                        ui,
                        entry,
                        StatusDrawContext {
                            context: state
                                .overflow_contexts
                                .get(&entry.key())
                                .copied()
                                .unwrap_or(entry.context),
                            mode: StatusDrawMode::Popup,
                            width: ui.content_region_avail()[0]
                                .max(entry.width)
                                .min(size[0] * 0.85),
                        },
                        &mut state,
                        &mut actions,
                    );
                    ui.separator();
                }
            } else {
                state.overflow_contexts.clear();
            }
        });
    world.insert_resource(state);
    actions
}

fn popup_constraints(size: [f32; 2]) {
    let width = (size[0] * 0.9).max(1.0);
    let height = (size[1] * 0.8).max(1.0);
    unsafe {
        sys::igSetNextWindowSizeConstraints(
            sys::ImVec2 {
                x: width.min(180.0),
                y: 0.0,
            },
            sys::ImVec2 {
                x: width,
                y: height,
            },
            None,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(test)]
mod tests;
