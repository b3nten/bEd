//! Go to Symbol in File: a picker (`bedtime_ui::picker`) over the editor
//! listing its outline's entries, narrowed by what's typed (fuzzily, best
//! first). Moving through it shows each in the editor; Enter goes there,
//! Escape goes back.
use super::{
    Reveal, current_outline,
    symbols::{self, Category},
};
use crate::{
    editor::{EditorPanel, Navigation, Scroll, TextSelection, TextSnapshot, editor_id},
    syntax::Observation,
};
use bedtime_document::TextBuffer;
use bedtime_ui::{
    UiContext, egui,
    picker::{self, PickerEvent, PickerRows, PickerState},
};
use bedtime_workbench::{
    ActionInvocation, ActionResult, DocumentReference, LayoutsWorldExt, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::sync::Arc;

/// Go to one of the focused editor's outline entries, by name.
pub struct GoToSymbol;
impl bedtime_workbench::Action for GoToSymbol {
    const KEY: &'static str = "code.go_to_symbol";
    const LABEL: &'static str = "Go to Symbol in File…";
}

/// The open picker, if there is one.
#[derive(Resource, Default)]
pub(crate) struct Picking(Option<Picker>);

struct Picker {
    panel: Entity,
    /// Where the caret was, to go back to.
    before: (u64, TextSelection),
    state: PickerState,
    symbols: Symbols,
}

/// The outline's entries as picker rows.
struct Symbols {
    observation: Arc<Observation>,
    details: Arc<Vec<symbols::Detail>>,
    /// The entries matching the query, best first, with matched characters.
    matches: Vec<(usize, Vec<usize>)>,
    filtering: bool,
}

pub(crate) fn install(app: &mut App) {
    use bedtime_workbench::ActionAppExt;
    app.init_resource::<Picking>()
        .add_action_handler::<GoToSymbol>(bedtime_workbench::Handler::new(open).available_when(
            |world, panel| panel.is_some_and(|panel| world.get::<EditorPanel>(panel).is_some()),
        ));
}

fn open(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let panel = invocation.entity.ok_or("No editor is focused")?;
    let observation = current_outline(world, panel).ok_or("This file has no structure yet")?;
    if observation.outline.is_empty() {
        return Err("There are no declarations in this file".into());
    }
    let selection = world
        .get::<EditorPanel>(panel)
        .ok_or("No editor is focused")?
        .view
        .selection();
    let selected = symbols::entry_at(&observation.outline, selection.head()).unwrap_or(0);
    let details = symbols::Details::default().of(&observation);
    world.resource_mut::<Picking>().0 = Some(Picker {
        panel,
        before: (observation.key.revision, selection),
        state: PickerState::at(selected),
        symbols: Symbols {
            observation,
            details,
            matches: Vec::new(),
            filtering: false,
        },
    });
    Ok(())
}

impl PickerRows for Symbols {
    fn filter(&mut self, query: &str) {
        let outline = &self.observation.outline;
        self.filtering = !query.trim().is_empty();
        self.matches = if !self.filtering {
            (0..outline.len())
                .map(|index| (index, Vec::new()))
                .collect()
        } else {
            let mut scored: Vec<(i32, usize, Vec<usize>)> = outline
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    picker::fuzzy(&node.label, query).map(|(score, at)| (score, index, at))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored
                .into_iter()
                .map(|(_, index, at)| (index, at))
                .collect()
        };
    }

    fn len(&self) -> usize {
        self.matches.len()
    }

    fn label(&self, row: usize) -> String {
        self.observation.outline[self.matches[row].0].label.clone()
    }

    fn paint(&self, ui: &mut egui::Ui, context: &UiContext<'_>, rect: egui::Rect, row: usize) {
        let (index, matched) = &self.matches[row];
        let outline = &self.observation.outline;
        let node = &outline[*index];
        let indent = if self.filtering {
            0.0
        } else {
            node.depth as f32 * 14.0
        };
        paint_row(
            ui,
            context,
            rect,
            indent,
            node,
            matched,
            &self.details[*index],
            self.filtering.then(|| ancestry_label(outline, *index)),
        );
    }
}

/// While the editor `panel` has the picker open: draw it over `area` (the
/// editor's), and say so (the editor leaves the keyboard to it).
pub(crate) fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    area: egui::Rect,
) -> bool {
    let open = world
        .resource::<Picking>()
        .0
        .as_ref()
        .is_some_and(|picker| picker.panel == panel);
    if !open {
        return false;
    }
    if let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) {
        let id = editor_id(panel, document);
        if ui.memory(|memory| memory.has_focus(id)) {
            ui.memory_mut(|memory| memory.surrender_focus(id));
        }
    }
    let mut picking = world.resource_mut::<Picking>();
    let picker = picking.0.as_mut().unwrap();
    let width = (area.width() - 24.0).clamp(160.0, 560.0);
    let event = picker::Picker::new(
        egui::Id::new((panel, "go-to-symbol")),
        egui::pos2(area.center().x, area.top() + 8.0),
        width,
    )
    .hint("Go to symbol")
    .empty("No matching symbols")
    .show(ui, context, &mut picker.state, &mut picker.symbols);
    let key = picker.symbols.observation.key;
    let item = |picker: &Picker, row: usize| {
        picker
            .symbols
            .matches
            .get(row)
            .map(|(index, _)| picker.symbols.observation.outline[*index].id)
    };
    match event {
        Some(PickerEvent::Chosen(row)) => {
            if let Some(item) = item(picker, row) {
                picking.0 = None;
                super::reveal_entry_as(world, panel, key, item, Reveal::Focus);
                return false;
            }
        }
        Some(event @ (PickerEvent::Escaped | PickerEvent::ClickedOutside)) => {
            let before = picker.before;
            picking.0 = None;
            if event == PickerEvent::Escaped {
                go_back(world, panel, before);
            }
            let _ = world.layouts_mut().focus_panel(panel);
            return false;
        }
        Some(PickerEvent::Moved(row)) => {
            if let Some(item) = item(picker, row) {
                super::reveal_entry_as(world, panel, key, item, Reveal::Preview);
            }
        }
        None => {}
    }
    true
}

/// "Parent.Child", for an entry shown out of place (best match first).
fn ancestry_label(outline: &[crate::syntax::OutlineNode], index: usize) -> String {
    let mut names: Vec<&str> = symbols::ancestry(outline, index)
        .skip(1)
        .map(|at| outline[at].label.as_str())
        .collect();
    names.reverse();
    names.join(" › ")
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    indent: f32,
    node: &crate::syntax::OutlineNode,
    matched: &[usize],
    detail: &symbols::Detail,
    within: Option<String>,
) {
    let palette = &context.theme.palette;
    let category = Category::of(&node.kind);
    let side = context.metrics.font_size.min(16.0);
    let x = rect.left() + 8.0 + indent;
    let mut theme = context.theme.clone();
    theme.palette.text = category.ink(&palette.syntax);
    // Icons draw through a frame; this one paints and doesn't lay out.
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let mut frame = context.frame(&mut child);
    frame.theme = &theme;
    frame.icon_at(category.icon(), [x, rect.center().y - side / 2.0], side);
    let font = egui::FontId::proportional(context.metrics.font_size);
    let text = bedtime_ui::theme::color(palette.text);
    let accent = bedtime_ui::theme::color(palette.accent);
    let muted = bedtime_ui::theme::color(palette.muted);
    let mut job = picker::highlighted(&node.label, matched, font, text, accent);
    let small = egui::FontId::proportional((context.metrics.font_size - 1.0).max(9.0));
    if !detail.text.is_empty() {
        job.append(
            &detail.text,
            8.0,
            egui::TextFormat::simple(small.clone(), muted),
        );
    }
    if let Some(within) = within.filter(|w| !w.is_empty()) {
        job.append(
            &format!("  {within}"),
            0.0,
            egui::TextFormat::simple(small.clone(), muted),
        );
    }
    let line = format!("{}", detail.line + 1);
    let line_galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(line, small, muted));
    let name_x = x + side + 6.0;
    job.wrap.max_width = (rect.right() - 24.0 - line_galley.size().x - name_x).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let painter = ui.painter();
    painter.galley(
        egui::pos2(name_x, rect.center().y - galley.size().y / 2.0),
        galley,
        text,
    );
    painter.galley(
        egui::pos2(
            rect.right() - 8.0 - line_galley.size().x,
            rect.center().y - line_galley.size().y / 2.0,
        ),
        line_galley,
        muted,
    );
}

/// Put the caret back where it was before the picker showed entries.
pub(super) fn go_back(
    world: &mut World,
    panel: Entity,
    (revision, selection): (u64, TextSelection),
) {
    let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
        return;
    };
    let workbench = world.workbench();
    let Some(text) = workbench
        .cast_document::<TextBuffer>(document)
        .and_then(|handle| workbench.read_document(handle).cloned())
        .ok()
    else {
        return;
    };
    let snapshot = TextSnapshot { revision, text };
    if world
        .workbench()
        .inspect_document(document)
        .is_ok_and(|info| info.revision != revision)
    {
        return;
    }
    let navigation = Navigation {
        revision,
        range: selection.range(),
        selection: Some(selection),
        reveal: Scroll::Center,
        focus: false,
    };
    if let Some(mut editor) = world.get_mut::<EditorPanel>(panel) {
        let _ = editor.view.request_navigation(&snapshot, navigation);
    }
}

/// The picker's entries, for tests: labels, best first.
#[cfg(test)]
pub(crate) fn listed(world: &mut World) -> Option<Vec<String>> {
    let mut picking = world.resource_mut::<Picking>();
    let picker = picking.0.as_mut()?;
    picker.state.filter(&mut picker.symbols);
    Some(
        (0..picker.symbols.len())
            .map(|row| picker.symbols.label(row))
            .collect(),
    )
}

/// Type `query` into the open picker, for tests.
#[cfg(test)]
pub(crate) fn type_query(world: &mut World, query: &str) {
    if let Some(picker) = world.resource_mut::<Picking>().0.as_mut() {
        picker.state.query = query.into();
    }
}

#[cfg(test)]
pub(crate) fn opened(world: &World) -> Option<(u64, TextSelection)> {
    world.resource::<Picking>().0.as_ref().map(|p| p.before)
}
