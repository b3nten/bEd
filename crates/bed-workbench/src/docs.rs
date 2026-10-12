//! Contextual help is an ordinary panel. Features own the notes it reads.
use crate::{PanelFocusHistory, PanelRecord, WorkbenchLayout, api};
use api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelDocumentation,
    PanelResult, ShortcutInput,
};
use bed_keybindings::{Bindings, RESET_ZOOM, Scope, ZOOM_IN, ZOOM_OUT};
use bevy::prelude::*;
use dear_imgui_rs::{PopupQueryFlags, StyleVar, TreeNodeFlags, Ui, WindowFlags};
use serde_json::{Value, json};

pub const DOCS_PANEL_ID: &str = "bed.docs.panel";
pub const SHOW_DOCS_COMMAND: &str = "bed.docs.show";

pub struct DocsPlugin;
impl Plugin for DocsPlugin {
    fn build(&self, app: &mut App) {
        api::register(app.world_mut(), "bed.docs", |r| {
            r.panel(DOCS_PANEL_ID, "Docs", create);
            r.command(SHOW_DOCS_COMMAND, "Show Docs", show);
            r.menu(SHOW_DOCS_COMMAND);
        })
        .expect("DocsPlugin must be installed once");
    }
}

fn show(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::ShowPanel {
        panel_type: DOCS_PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert((
        api::PanelPadding([10.0, 8.0]),
        PanelDocumentation::new(&[("Reading", "Focus another panel to see its usage notes and shortcuts. Collapse sections to save space."),
            ("Zoom", "Use the common zoom shortcuts while Docs is focused, or pinch over it to change text size. Reset Zoom restores normal text size.")])
            .shortcut_actions(&[ZOOM_IN, ZOOM_OUT, RESET_ZOOM]),
    ));
    Ok(Box::new(DocsPanel::from_state(&creation.state)))
}

/// The existing transition owner records focus. Reading that history also works
/// when Docs was hidden while other panels were used; there is no second tracker.
fn target(world: &World) -> Option<Entity> {
    let eligible = |entity: Entity| {
        api::panel_active(world, entity)
            && world
                .get::<PanelRecord>(entity)
                .is_some_and(|r| r.panel_type != DOCS_PANEL_ID)
    };
    world
        .resource::<WorkbenchLayout>()
        .focused
        .filter(|entity| eligible(*entity))
        .or_else(|| {
            world
                .resource::<PanelFocusHistory>()
                .0
                .iter()
                .rev()
                .copied()
                .find(|entity| eligible(*entity))
        })
        .map(|owner| {
            world
                .get::<api::PanelFrontend>(owner)
                .map_or(owner, |front| front.0)
        })
}

#[derive(Debug, PartialEq)]
struct KeyRow {
    label: String,
    keys: String,
}

fn key_rows(
    bindings: &Bindings,
    panel_type: Option<&str>,
    docs: Option<&PanelDocumentation>,
    global: bool,
) -> Vec<KeyRow> {
    let mut scopes = panel_type.into_iter().map(Scope::panel).collect::<Vec<_>>();
    if let Some(docs) = docs {
        scopes.extend(docs.shortcut_scopes.iter().cloned().map(Scope::panel));
    }
    bindings
        .actions
        .iter()
        .filter_map(|action| {
            let included = if global {
                action.scope == Scope::Global
            } else {
                action.scope != Scope::Global
                    && (scopes.contains(&action.scope)
                        || docs.is_some_and(|docs| docs.shortcut_actions.contains(&action.id)))
            };
            if !included {
                return None;
            }
            let keys = bindings
                .for_action(&action.id)
                .into_iter()
                .filter(|binding| {
                    if global {
                        bindings
                            .winner(&Scope::Global, binding.shortcut)
                            .is_some_and(|winner| winner.action == action.id)
                    } else {
                        let relevant = if action.scope == Scope::Focused {
                            scopes.as_slice()
                        } else {
                            std::slice::from_ref(&action.scope)
                        };
                        relevant.iter().any(|scope| {
                            bindings
                                .active_winner(scope, binding.shortcut)
                                .is_some_and(|winner| winner.action == action.id)
                        })
                    }
                })
                .map(|binding| binding.shortcut.label(bindings.platform))
                .collect::<Vec<_>>();
            (!keys.is_empty()).then(|| KeyRow {
                label: action.label.clone(),
                keys: keys.join(" / "),
            })
        })
        .collect()
}

struct DocsPanel {
    zoom: f32,
    expanded: [bool; 3],
    source: Option<Entity>,
    scroll: f32,
    pinch: bool,
    frame: Option<usize>,
}
impl DocsPanel {
    fn from_state(state: &Value) -> Self {
        let zoom = state["zoom"]
            .as_f64()
            .filter(|value| value.is_finite())
            .unwrap_or(1.0) as f32;
        let mut panel = Self {
            zoom: zoom.clamp(0.5, 2.0),
            expanded: [true, true, false],
            source: None,
            scroll: 0.0,
            pinch: false,
            frame: None,
        };
        for (index, expanded) in panel.expanded.iter_mut().enumerate() {
            if let Some(saved) = state["expanded"][index].as_bool() {
                *expanded = saved;
            }
        }
        panel
    }
    fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.5, 2.0);
    }
}

impl Panel for DocsPanel {
    fn title(&self, _: &World) -> String {
        "Docs".into()
    }
    fn save(&self, _: &World) -> Value {
        json!({"zoom": self.zoom, "expanded": self.expanded})
    }
    fn draw(&mut self, ui: &Ui, world: &mut World, _: &mut Vec<Action>, input: &ShortcutInput) {
        let source = target(world);
        let changed = self.source != source;
        self.source = source;
        if changed {
            self.scroll = 0.0;
        }
        let record = source.and_then(|entity| world.get::<PanelRecord>(entity));
        let docs = source.and_then(|entity| world.get::<PanelDocumentation>(entity));
        let context = record
            .and_then(|r| world.resource::<api::Registry>().panel(&r.panel_type))
            .map(|p| p.label)
            .or_else(|| record.map(|r| r.panel_type.as_str()))
            .unwrap_or("Docs");
        let before_zoom = self.zoom;
        let blocked = ui.is_popup_open_with_flags(
            "",
            PopupQueryFlags::ANY_POPUP_ID | PopupQueryFlags::ANY_POPUP_LEVEL,
        ) || input.suppressed(ui);
        if !blocked
            && !ui.io().want_text_input()
            && ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::ROOT_AND_CHILD_WINDOWS)
        {
            if input.take(ui, ZOOM_IN) {
                self.zoom_by(1.1);
            }
            if input.take(ui, ZOOM_OUT) {
                self.zoom_by(1.0 / 1.1);
            }
            if input.take(ui, RESET_ZOOM) {
                self.zoom = 1.0;
            }
        }
        // Each presentation reads the same frame's native facts. Hovered root
        // ownership makes the captured targets exclusive, even across focus changes.
        if blocked || self.frame.is_none_or(|frame| frame + 1 != ui.frame_count()) {
            self.pinch = false;
        }
        self.frame = Some(ui.frame_count());
        for event in &world.resource::<api::PinchEvents>().0 {
            if event.phase == api::PinchPhase::Started {
                self.pinch = !blocked
                    && event.position.iter().all(|value| value.is_finite())
                    && ui.with_bound_context(|| unsafe {
                        let mut hovered = std::ptr::null_mut();
                        dear_imgui_rs::sys::igFindHoveredWindowEx(
                            event.position.into(),
                            true,
                            &mut hovered,
                            std::ptr::null_mut(),
                        );
                        !hovered.is_null()
                            && (*hovered).RootWindow
                                == (*dear_imgui_rs::sys::igGetCurrentWindowRead()).RootWindow
                    });
            }
            let factor = (1.0 + event.delta) as f32;
            if self.pinch
                && event.phase != api::PinchPhase::Cancelled
                && factor.is_finite()
                && factor > 0.0
            {
                self.zoom_by(factor);
            }
            if matches!(
                event.phase,
                api::PinchPhase::Ended | api::PinchPhase::Cancelled
            ) {
                self.pinch = false;
            }
        }

        let panel_keys = key_rows(
            &input.bindings,
            record.map(|r| r.panel_type.as_str()),
            docs,
            false,
        );
        let global_keys = key_rows(&input.bindings, None, None, true);
        let _font = ui.push_font_with_size(None, ui.current_font_size() * self.zoom);
        ui.text_wrapped(context);
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([6.0 * self.zoom, 4.0 * self.zoom]));
        let _padding =
            ui.push_style_var(StyleVar::FramePadding([3.0 * self.zoom, 2.0 * self.zoom]));
        // Reserve the scrollbar width even as zoom changes the content height,
        // so wrapped text never acquires a horizontal scroll range for a frame.
        let flags = WindowFlags::ALWAYS_VERTICAL_SCROLLBAR;
        if changed || before_zoom != self.zoom {
            ui.with_bound_context(|| unsafe {
                dear_imgui_rs::sys::igSetNextWindowScroll([0.0, self.scroll].into());
            });
        }
        ui.child_window("docs_content")
            .size([0.0, 0.0])
            .flags(flags)
            .build(ui, || {
                for (index, label) in ["Usage", "Panel keybinds", "Global keybinds"]
                    .into_iter()
                    .enumerate()
                {
                    ui.set_next_item_open(self.expanded[index]);
                    let width =
                        (ui.content_region_avail()[0] - ui.current_font_size() - 6.0 * self.zoom)
                            .max(0.0);
                    let short = ["Usage", "Keybinds", "Global"][index];
                    let heading = if ui.calc_text_size(label)[0] <= width {
                        label
                    } else {
                        short
                    };
                    let heading = bed_ui::presentation::fit_text(ui, heading, width);
                    self.expanded[index] = ui.collapsing_header(
                        format!("{heading}###docs_section_{index}"),
                        TreeNodeFlags::empty(),
                    );
                    if ui.is_item_hovered() {
                        ui.tooltip_text(label);
                    }
                    if !self.expanded[index] {
                        continue;
                    }
                    if index == 0 {
                        if source.is_none() {
                            ui.text_wrapped("Focus a panel to see its docs");
                        } else if let Some(docs) = docs.filter(|docs| !docs.sections.is_empty()) {
                            for section in &docs.sections {
                                if !section.title.is_empty() {
                                    let _color = ui.push_style_color(
                                        dear_imgui_rs::StyleColor::Text,
                                        ui.style_color(dear_imgui_rs::StyleColor::TextDisabled),
                                    );
                                    ui.text_wrapped(&section.title);
                                }
                                ui.text_wrapped(&section.body);
                            }
                        } else {
                            ui.text_wrapped("No usage notes published for this panel.");
                        }
                    } else {
                        let rows = if index == 1 {
                            &panel_keys
                        } else {
                            &global_keys
                        };
                        if rows.is_empty() {
                            ui.text_wrapped("No bound shortcuts.");
                        }
                        for row in rows {
                            draw_key_row(ui, row, world.get_non_send::<bed_ui::icons::Icons>());
                        }
                    }
                }
                self.scroll = ui.scroll_y();
            });
    }
}

fn draw_key_row(ui: &Ui, row: &KeyRow, icons: Option<&bed_ui::icons::Icons>) {
    ui.group(|| {
        let right = ui.cursor_screen_pos()[0] + ui.content_region_avail()[0];
        let side = ui.current_font_size();
        let gap = side * 0.15;
        let ink = ui.style_color(dear_imgui_rs::StyleColor::TextDisabled);
        let mut first = true;
        for (index, shortcut) in row.keys.split(" / ").enumerate() {
            // Alternatives share the same list item. Wrap individual keys when
            // a long chord cannot fit the panel at the current zoom level.
            let parts = (index > 0)
                .then_some("/")
                .into_iter()
                .chain(shortcut.split('+'));
            for part in parts {
                let key = match part {
                    "Command" => Some("key-command"),
                    "Shift" => Some("key-shift"),
                    "Alt" => Some("key-option"),
                    "Ctrl" => Some("key-control"),
                    _ => None,
                };
                let texture = key.and_then(|key| icons?.textures.get(key).copied());
                let width = texture.map_or_else(|| ui.calc_text_size(part)[0], |_| side);
                if !first && ui.item_rect_max()[0] + gap + width <= right {
                    ui.same_line_with_spacing(0.0, gap);
                }
                first = false;
                if let Some(texture) = texture {
                    ui.image_config(texture, [side.min(ui.content_region_avail()[0]); 2])
                        .tint_color(ink)
                        .build();
                    if ui.is_item_hovered() {
                        ui.tooltip_text(if part == "Alt" { "Option / Alt" } else { part });
                    }
                } else {
                    let _color = ui.push_style_color(dear_imgui_rs::StyleColor::Text, ink);
                    ui.text_wrapped(part);
                }
            }
        }
        let action_gap = side * 0.4;
        let word = row.label.split_whitespace().next().unwrap_or("");
        if ui.item_rect_max()[0] + action_gap + ui.calc_text_size(word)[0] <= right {
            ui.same_line_with_spacing(0.0, action_gap);
        }
        ui.text_wrapped(&row.label);
    });
}

#[cfg(test)]
mod tests;
