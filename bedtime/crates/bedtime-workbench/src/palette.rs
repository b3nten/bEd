//! The command palette (`ui` feature): a box listing every action that can
//! run now, by name, to run one. Add `CommandPalettePlugin`; its action
//! (`ShowCommandPalette`) has no keys of its own.
//!
//! The list is taken when the palette opens, for the entity actions run for
//! then (the focused panel): once its field has the keyboard, actions that
//! need their panel to have it (the editor's) would no longer count as
//! available. Choosing one closes the palette, gives the keyboard back to
//! whatever had it, and runs the action once it can, a frame or two later.
use crate::*;
use bedtime_ui::{
    UiContext, egui,
    picker::{self, Picker, PickerEvent, PickerRows, PickerState},
};
use bevy::prelude::*;

/// Show the command palette.
pub struct ShowCommandPalette;
impl Action for ShowCommandPalette {
    const KEY: &'static str = "workbench.command_palette";
    const LABEL: &'static str = "Command Palette…";
}

/// The command palette and the action that shows it.
pub struct CommandPalettePlugin;

impl Plugin for CommandPalettePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandPalette>()
            .add_action_handler::<ShowCommandPalette>(Handler::new(open))
            .add_menu_item::<ShowCommandPalette>(StandardMenu::View, "commands")
            .add_modal(draw)
            .add_systems(PreUpdate, run_chosen.before(ActionSystems::Dispatch));
    }
}

/// How many frames a chosen action waits to become available before it's
/// requested anyway (and fails, saying why).
const PATIENCE: u8 = 5;

/// How many recently run commands are remembered (for this session).
const RECENTS: usize = 20;

/// The palette, while open, the action chosen in it, until it runs, and
/// what was run from it lately.
#[derive(Resource, Default)]
pub struct CommandPalette {
    open: Option<Open>,
    closing: Option<Closing>,
    /// Most recent first.
    recent: Vec<ActionId>,
}

impl CommandPalette {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The commands listed, as filtered, best first.
    pub fn listed(&mut self) -> Vec<&Command> {
        let Some(open) = self.open.as_mut() else {
            return Vec::new();
        };
        open.state.filter(&mut open.commands);
        open.commands
            .shown
            .iter()
            .map(|(index, _)| &open.commands.all[*index])
            .collect()
    }

    /// Type `query` into the open palette, as if typed.
    pub fn set_query(&mut self, query: impl Into<String>) {
        if let Some(open) = self.open.as_mut() {
            open.state.query = query.into();
        }
    }

    /// Choose the listed command `row`, as Enter would.
    pub fn choose(&mut self, row: usize) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        open.state.filter(&mut open.commands);
        let Some(&(index, _)) = open.commands.shown.get(row) else {
            self.open = Some(open);
            return;
        };
        let action = open.commands.all[index].action.clone();
        self.recent.retain(|recent| *recent != action);
        self.recent.insert(0, action.clone());
        self.recent.truncate(RECENTS);
        let request = ActionRequest {
            action,
            context: open_context(open.target),
            source: None,
        };
        self.close(open, Some(request));
    }

    /// Close without running anything.
    pub fn dismiss(&mut self) {
        if let Some(open) = self.open.take() {
            self.close(open, None);
        }
    }

    fn close(&mut self, open: Open, request: Option<ActionRequest>) {
        let focus = open.focus_before.flatten();
        self.closing = Some(Closing {
            surface: open.surface,
            restored: focus.is_none(),
            focus,
            request,
            frames: 0,
        });
    }
}

fn open_context(target: Option<Entity>) -> InvocationContext {
    target.map_or(InvocationContext::Global, InvocationContext::Entity)
}

struct Open {
    /// The entity the actions run for.
    target: Option<Entity>,
    /// The window it's drawn in: the focused one when it opened, else the
    /// first to draw it.
    surface: Option<Entity>,
    /// What had the keyboard before, once drawn (`Some(None)`: nothing).
    focus_before: Option<Option<egui::Id>>,
    state: PickerState,
    commands: Commands,
}

/// After choosing: give the keyboard back, then run the request.
struct Closing {
    surface: Option<Entity>,
    focus: Option<egui::Id>,
    restored: bool,
    request: Option<ActionRequest>,
    frames: u8,
}

/// An action the palette lists.
#[derive(Clone, Debug)]
pub struct Command {
    pub action: ActionId,
    /// What's shown and matched: "View: Git" for an action in a menu (the
    /// menu, then its item's label), else the action's label.
    pub label: String,
    /// How many characters of `label` name the menu ("View: ").
    pub menu_chars: usize,
    /// The chord that runs it, resolved for this platform.
    pub shortcut: Option<KeyChord>,
}

struct Commands {
    /// In the order shown when nothing's typed: recently run, then as in
    /// the menus, then the rest as registered. Ties in score keep it.
    all: Vec<Command>,
    /// Into `all`, best first, with the label's matched characters.
    shown: Vec<(usize, Vec<usize>)>,
}

impl PickerRows for Commands {
    fn filter(&mut self, query: &str) {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                picker::fuzzy(&command.label, query).map(|(score, at)| (score, index, at))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.shown = scored
            .into_iter()
            .map(|(_, index, at)| (index, at))
            .collect();
    }

    fn len(&self) -> usize {
        self.shown.len()
    }

    fn label(&self, row: usize) -> String {
        self.all[self.shown[row].0].label.clone()
    }

    fn paint(&self, ui: &mut egui::Ui, context: &UiContext<'_>, rect: egui::Rect, row: usize) {
        let (index, matched) = &self.shown[row];
        let command = &self.all[*index];
        let palette = &context.theme.palette;
        let text = bedtime_ui::theme::color(palette.text);
        let accent = bedtime_ui::theme::color(palette.accent);
        let muted = bedtime_ui::theme::color(palette.muted);
        let font = egui::FontId::proportional(context.metrics.font_size);
        let shortcut = command.shortcut.map(|chord| {
            ui.fonts_mut(|fonts| fonts.layout_no_wrap(chord.label(), font.clone(), muted))
        });
        let shortcut_width = shortcut
            .as_ref()
            .map_or(0.0, |galley| galley.size().x + 16.0);
        // The menu's name is quieter than the command's.
        let ink = |at| if at < command.menu_chars { muted } else { text };
        let mut job = picker::highlighted_with(&command.label, matched, font, ink, accent);
        job.wrap.max_width = (rect.width() - 16.0 - shortcut_width).max(0.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let painter = ui.painter();
        painter.galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            text,
        );
        if let Some(galley) = shortcut {
            painter.galley(
                egui::pos2(
                    rect.right() - 8.0 - galley.size().x,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                muted,
            );
        }
    }
}

/// Open the palette with the actions available for the invocation's entity.
fn open(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let target = invocation.entity;
    let context = open_context(target);
    let own = ActionId::of::<ShowCommandPalette>();
    let ids: Vec<(ActionId, &'static str)> = world
        .resource::<ActionRegistry>()
        .actions()
        .filter(|(id, _)| **id != own)
        .map(|(id, label)| (id.clone(), label))
        .collect();
    let menus = menu_names(world, context);
    let recent = world.resource::<CommandPalette>().recent.clone();
    let actions = world.actions();
    let mut all: Vec<(usize, usize, Command)> = ids
        .into_iter()
        .enumerate()
        .filter(|(_, (id, _))| actions.available(id, context))
        .map(|(registered, (action, label))| {
            let menu = menus.iter().position(|(id, ..)| *id == action);
            let (menu_chars, label) = match menu.map(|at| &menus[at]) {
                Some((_, menu, name)) => {
                    let name = name.as_deref().unwrap_or(label);
                    (menu.chars().count() + 2, format!("{menu}: {name}"))
                }
                None => (0, label.to_owned()),
            };
            let rank = recent.iter().position(|id| *id == action);
            let order = match (rank, menu) {
                (Some(rank), _) => rank,
                (None, Some(menu)) => RECENTS + menu,
                (None, None) => RECENTS + menus.len() + registered,
            };
            let command = Command {
                shortcut: actions.shortcut_for(&action, context),
                label,
                menu_chars,
                action,
            };
            (order, registered, command)
        })
        .collect();
    all.sort_by_key(|(order, registered, _)| (*order, *registered));
    let all = all.into_iter().map(|(.., command)| command).collect();
    let surface = world
        .query_filtered::<(Entity, &Window), With<LayoutSurface>>()
        .iter(world)
        .find(|(_, window)| window.focused)
        .map(|(surface, _)| surface);
    let mut palette = world.resource_mut::<CommandPalette>();
    palette.closing = None;
    palette.open = Some(Open {
        target,
        surface,
        focus_before: None,
        state: PickerState::default(),
        commands: Commands {
            all,
            shown: Vec::new(),
        },
    });
    Ok(())
}

/// Each action in the menu bar, in menu order, with the top menu's label
/// and its item's label (`None` when several items run it: the action's own
/// label says it better). Entries left out of the menus now are left out.
fn menu_names(
    world: &mut World,
    context: InvocationContext,
) -> Vec<(ActionId, String, Option<String>)> {
    fn walk(nodes: &[MenuNode], top: Option<&str>, found: &mut Vec<(ActionId, String, String)>) {
        for node in nodes {
            match node {
                MenuNode::Menu { label, entries, .. } => {
                    walk(entries, Some(top.unwrap_or(label)), found)
                }
                MenuNode::Item {
                    label,
                    target: Some(action),
                    ..
                } => found.push((
                    action.clone(),
                    top.unwrap_or_default().to_owned(),
                    label.clone(),
                )),
                _ => {}
            }
        }
    }
    let Some(bar) = world
        .query_filtered::<Entity, With<MenuBar>>()
        .iter(world)
        .next()
    else {
        return Vec::new();
    };
    let Ok(snapshot) = world.menus().snapshot(bar, context) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&snapshot.entries, None, &mut found);
    let mut names: Vec<(ActionId, String, Option<String>)> = Vec::new();
    for (action, menu, item) in &found {
        if names.iter().any(|(id, ..)| id == action) {
            continue;
        }
        let items = found.iter().filter(|(id, ..)| id == action).count();
        names.push((
            action.clone(),
            menu.clone(),
            (items == 1).then(|| item.clone()),
        ));
    }
    names
}

/// The modal: the palette on the window it belongs to. The frame after it
/// closes, the keyboard goes back to what had it.
fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) -> bool {
    let mut palette = world.resource_mut::<CommandPalette>();
    if let Some(closing) = palette.closing.as_mut()
        && !closing.restored
        && closing
            .surface
            .is_none_or(|surface| surface == context.surface)
    {
        if let Some(id) = closing.focus {
            ui.ctx().memory_mut(|memory| memory.request_focus(id));
        }
        closing.restored = true;
    }
    let Some(open) = palette.open.as_mut() else {
        return false;
    };
    if *open.surface.get_or_insert(context.surface) != context.surface {
        return false;
    }
    let ctx = ui.ctx().clone();
    if open.focus_before.is_none() {
        open.focus_before = Some(ctx.memory(|memory| memory.focused()));
    }
    let id = egui::Id::new(("command-palette", context.surface));
    let screen = ctx.content_rect();
    let width = (screen.width() - 48.0).clamp(200.0, 600.0);
    let area = egui::Modal::default_area(id.with("area")).anchor(
        egui::Align2::CENTER_TOP,
        egui::vec2(0.0, screen.height() * 0.12),
    );
    let shown = egui::Modal::new(id.with("modal"))
        .area(area)
        .backdrop_color(egui::Color32::from_black_alpha(48))
        .show(&ctx, |ui| {
            ui.set_width(width);
            Picker::new(id, egui::Pos2::ZERO, width)
                .hint("Run a command")
                .empty("No matching commands")
                .show_inside(ui, context, &mut open.state, &mut open.commands)
        });
    match shown.inner {
        Some(PickerEvent::Chosen(row)) => palette.choose(row),
        Some(PickerEvent::Escaped | PickerEvent::ClickedOutside) => palette.dismiss(),
        Some(PickerEvent::Moved(_)) | None => {
            if shown.should_close() {
                palette.dismiss();
            }
        }
    }
    // Keep the window disabled this frame too, closing or not.
    true
}

/// Run the chosen action once the keyboard is back and it's available (or
/// it has waited long enough: then it runs to say why it can't).
fn run_chosen(world: &mut World) {
    let Some(closing) = world.resource_mut::<CommandPalette>().closing.take() else {
        return;
    };
    let Some(request) = closing.request.clone() else {
        if !closing.restored {
            world.resource_mut::<CommandPalette>().closing = Some(closing);
        }
        return;
    };
    let frames = closing.frames + 1;
    let ready = closing.restored && world.actions().available(&request.action, request.context);
    if ready || frames > PATIENCE {
        world.resource_mut::<ActionRequests>().submit(request);
    } else {
        world.resource_mut::<CommandPalette>().closing = Some(Closing { frames, ..closing });
    }
}

#[cfg(test)]
mod tests;
