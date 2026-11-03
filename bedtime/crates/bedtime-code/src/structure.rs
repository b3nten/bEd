//! Structure: the outline of the focused editor's text, or of the last editor
//! focused while another kind of panel (Structure itself, say) has focus.
//! Clicking an entry selects its name in that editor and focuses it.
use crate::{
    editor::{EditorPanel, Navigation, Reveal, TextSelection, TextSnapshot},
    syntax::{AnalysisStatus, DocumentSyntax, Observation, ObservationKey},
};
use bedtime_document::TextBuffer;
use bedtime_ui::{UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentReference, LayoutsWorldExt, Panel, PanelInstance, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::{collections::HashSet, sync::Arc};

/// The editor panel Structure shows.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructureSource(pub Option<Entity>);

/// Follow the focused editor, in `Update`.
pub(crate) fn update_source(world: &mut World) {
    let focused = world
        .layouts()
        .active()
        .and_then(|layout| world.layouts().focused_panel(layout))
        .filter(|&panel| has_outline(world, panel));
    let remembered = world
        .resource::<StructureSource>()
        .0
        .filter(|&panel| has_outline(world, panel));
    let source = StructureSource(focused.or(remembered));
    if *world.resource::<StructureSource>() != source {
        world.insert_resource(source);
    }
}

/// An editor showing a document with a language.
fn has_outline(world: &World, panel: Entity) -> bool {
    world.get::<EditorPanel>(panel).is_some()
        && world
            .get::<DocumentReference>(panel)
            .and_then(|reference| {
                world
                    .workbench()
                    .inspect_document(reference.document())
                    .ok()
            })
            .is_some_and(|info| crate::syntax::language_name(&info.title).is_some())
}

/// The current analysis of the text `panel` shows: at its current revision.
pub fn current_outline(world: &World, panel: Entity) -> Option<Arc<Observation>> {
    world.get::<EditorPanel>(panel)?;
    let document = world.get::<DocumentReference>(panel)?.document();
    let revision = world.workbench().inspect_document(document).ok()?.revision;
    world
        .get::<DocumentSyntax>(document.entity())?
        .at(revision)
        .cloned()
}

/// Select outline entry `item` of analysis `key` in `editor`, and focus it.
/// An entry from an older analysis, or an editor that's gone or composing
/// text, does nothing (returns false).
pub fn reveal_entry(world: &mut World, editor: Entity, key: ObservationKey, item: u64) -> bool {
    let Some(observation) = current_outline(world, editor).filter(|o| o.key == key) else {
        return false;
    };
    let Some(node) = observation.outline.iter().find(|node| node.id == item) else {
        return false;
    };
    let Some(document) = world.get::<DocumentReference>(editor).map(|r| r.document()) else {
        return false;
    };
    let workbench = world.workbench();
    let Some(text) = workbench
        .cast_document::<TextBuffer>(document)
        .and_then(|handle| workbench.read_document(handle).cloned())
        .ok()
    else {
        return false;
    };
    let snapshot = TextSnapshot {
        revision: key.revision,
        text,
    };
    let Ok(selection) =
        TextSelection::new(&snapshot.text, node.name_range.start, node.name_range.end)
    else {
        return false;
    };
    let navigation = Navigation {
        revision: key.revision,
        range: node.name_range.clone(),
        selection: Some(selection),
        reveal: Reveal::Center,
        focus: false,
    };
    let accepted = world
        .get_mut::<EditorPanel>(editor)
        .is_some_and(|mut panel| panel.view.request_navigation(&snapshot, navigation).is_ok());
    accepted && world.layouts_mut().focus_panel(editor).is_ok()
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct StructurePanel {
    /// What's shown: the editor and its analysis. Kept while a newer analysis
    /// of the same text runs, so the outline doesn't flicker as you type.
    #[reflect(ignore)]
    shown: Option<(Entity, Arc<Observation>)>,
    #[reflect(ignore)]
    collapsed: HashSet<u64>,
    #[reflect(ignore)]
    selected: Option<u64>,
}

impl StructurePanel {
    fn refresh(&mut self, world: &World) {
        let source = world.resource::<StructureSource>().0;
        let shown = source.and_then(|editor| {
            if let Some(observation) = current_outline(world, editor) {
                return Some((editor, observation));
            }
            // Keep the last outline of this text while it's reanalysed.
            let (old_editor, old) = self.shown.as_ref()?;
            let document = world.get::<DocumentReference>(editor)?.document();
            let session = &world.get::<DocumentSyntax>(document.entity())?.session;
            let key = session.key()?;
            (*old_editor == editor
                && key.source == old.key.source
                && key.language_generation == old.key.language_generation
                && matches!(
                    session.status(),
                    AnalysisStatus::Pending | AnalysisStatus::Ready
                ))
            .then(|| (editor, old.clone()))
        });
        let identity = |shown: &Option<(Entity, Arc<Observation>)>| {
            shown.as_ref().map(|(editor, observation)| {
                (
                    *editor,
                    observation.key.source,
                    observation.key.language_generation,
                )
            })
        };
        if identity(&self.shown) != identity(&shown) {
            self.collapsed.clear();
            self.selected = None;
        }
        self.shown = shown;
    }
}

impl Panel for StructurePanel {
    const LABEL: &'static str = "Structure";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut state) = world.entity_mut(panel).take::<Self>() else {
            return;
        };
        state.refresh(world);
        let mut reveal = None;
        if let Some((editor, observation)) = state.shown.clone() {
            reveal = draw_outline(ui, context, panel, &mut state, editor, &observation);
        } else {
            egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                ui.weak("No structure available");
            });
        }
        if let Some((editor, key, item)) = reveal
            && reveal_entry(world, editor, key, item)
        {
            state.selected = Some(item);
        }
        world.entity_mut(panel).insert(state);
    }
}

/// The outline's rows, with animated collapsing. Returns the entry clicked.
fn draw_outline(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    state: &mut StructurePanel,
    editor: Entity,
    observation: &Observation,
) -> Option<(Entity, ObservationKey, u64)> {
    let outline = &observation.outline;
    let height = bedtime_ui::tree::row_height(context);
    let has_children = |index: usize| {
        outline
            .get(index + 1)
            .is_some_and(|n| n.parent == Some(index))
    };
    // Where each visible row goes, as its ancestors open and close: work it
    // out once, then draw only the rows in view.
    let mut rows = Vec::new();
    let mut ancestors: Vec<(usize, f32)> = Vec::new();
    let mut top = 0.0;
    for (index, node) in outline.iter().enumerate() {
        while ancestors
            .last()
            .is_some_and(|(depth, _)| *depth >= node.depth)
        {
            ancestors.pop();
        }
        let visibility = ancestors.last().map_or(1.0, |(_, visibility)| *visibility);
        let openness = if has_children(index) {
            bedtime_ui::animate_expansion(
                ui.ctx(),
                egui::Id::new((panel, editor, node.id, "structure-expansion")),
                !state.collapsed.contains(&node.id),
                context.motion,
            )
        } else {
            1.0
        };
        if visibility > 0.0 {
            rows.push((index, top, height * visibility, openness));
        }
        top += height * visibility;
        ancestors.push((node.depth, visibility * openness));
    }
    let mut toggle = None;
    let mut clicked = None;
    // Rows as the file tree draws them: a long name is cut short at the
    // panel's edge, rather than the panel scrolling sideways.
    egui::ScrollArea::vertical()
        .id_salt((panel, "structure"))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            if outline.is_empty() {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    ui.weak("No declarations");
                });
            }
            let origin = ui.cursor().min;
            ui.set_min_size(egui::vec2(viewport.width(), top));
            let first = rows.partition_point(|(_, y, h, _)| y + h < viewport.top());
            for &(index, y, row_height, _) in &rows[first..] {
                if y > viewport.bottom() {
                    break;
                }
                let node = &outline[index];
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, y),
                    egui::vec2(viewport.width(), row_height),
                );
                let id = egui::Id::new((panel, editor, node.id, "structure-row"));
                // The chevron toggles; the rest of the row reveals.
                let x = rect.left()
                    + bedtime_ui::tree::INSET
                    + node.depth as f32 * bedtime_ui::tree::INDENT;
                let arrow = egui::Rect::from_min_max(
                    egui::pos2(x, rect.top()),
                    egui::pos2(x + 16.0, rect.bottom()),
                );
                let children = has_children(index);
                let arrow_response =
                    children.then(|| ui.interact(arrow, id.with("arrow"), egui::Sense::click()));
                let response = ui.interact(rect, id, egui::Sense::click());
                let selected = state.selected == Some(node.id);
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
                    bedtime_ui::tree::paint_row(
                        ui,
                        context,
                        bedtime_ui::tree::Row {
                            label: Some(&node.label),
                            depth: node.depth,
                            icon: None,
                            expanded: children.then_some(!state.collapsed.contains(&node.id)),
                            selected,
                            ink: bedtime_ui::theme::color(context.theme.palette.text),
                            icon_ink: context.theme.palette.text,
                        },
                        rect,
                        Some(&response),
                        1.0,
                        bedtime_ui::tree::INDENT,
                    );
                });
                if arrow_response.is_some_and(|arrow| arrow.clicked()) {
                    toggle = Some(node.id);
                } else if response.clicked() {
                    clicked = Some((editor, observation.key, node.id));
                }
                response.on_hover_text(&node.kind);
            }
        });
    if let Some(id) = toggle
        && !state.collapsed.remove(&id)
    {
        state.collapsed.insert(id);
    }
    clicked
}

#[cfg(test)]
mod tests;
