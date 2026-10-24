//! Application-owned code Structure viewer. One remembered source per workspace;
//! activation directly controls that exact open editor through its public API.
use bedtime_editor2::{
    EditorError, Navigation, Reveal, TextBuffer, TextSelection, TextSnapshot,
    workbench::{
        CodeLanguage, DocumentAnalysis, analysis_for_panel, code_document_for_panel,
        navigate_editor, update_document_analysis,
    },
};
use bedtime_treesitter2::{AnalysisStatus, Observation, ObservationKey};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{collections::HashSet, sync::Arc};

pub struct StructurePlugin;
impl Plugin for StructurePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, refresh_sources.after(update_document_analysis));
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceBinding {
    pub panel: Entity,
    pub document: DocumentId,
}
#[derive(Component, Default)]
pub struct StructureSource {
    pub source: Option<SourceBinding>,
}
#[derive(Clone, Copy, Debug)]
pub struct Activation {
    pub source: SourceBinding,
    pub observation: ObservationKey,
    pub item: u64,
}
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct StructurePanel {
    outline: Option<(SourceBinding, Arc<Observation>)>,
    collapsed: HashSet<u64>,
    selected: Option<u64>,
}

impl StructurePanel {
    /// Retain presentation during a same-source refresh. Activation still resolves
    /// through analysis_for_panel and never navigates with cached byte ranges.
    fn refresh_outline(
        &mut self,
        world: &World,
        source: Option<SourceBinding>,
    ) -> Option<(SourceBinding, Arc<Observation>)> {
        let outline = source.and_then(|source| {
            if code_document_for_panel(world, source.panel) != Some(source.document) {
                return None;
            }
            if let Some(observation) = analysis_for_panel(world, source.panel) {
                return Some((source, observation));
            }
            let (old_source, old) = self.outline.as_ref()?;
            let analysis = world.get::<DocumentAnalysis>(source.document.0)?;
            let language = world.get::<CodeLanguage>(source.document.0)?;
            let key = analysis.session.key()?;
            // Ready can still refer to the previous revision on the edit frame,
            // before the next Update submits its replacement to the worker.
            (source == *old_source
                && bedtime_treesitter2::language_name(&language.0) == Some(old.language)
                && key.source == old.key.source
                && key.language_generation == old.key.language_generation
                && matches!(
                    analysis.session.status(),
                    AnalysisStatus::Pending | AnalysisStatus::Ready
                ))
            .then(|| (source, old.clone()))
        });
        let identity = |outline: &Option<(SourceBinding, Arc<Observation>)>| {
            outline.as_ref().map(|(source, observation)| {
                (
                    *source,
                    observation.key.source,
                    observation.key.language_generation,
                )
            })
        };
        if identity(&self.outline) != identity(&outline) {
            self.collapsed.clear();
            self.selected = None;
        }
        self.outline = outline.clone();
        outline
    }
}

pub fn refresh_sources(world: &mut World) {
    let workspaces: Vec<Entity> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    for workspace in workspaces {
        refresh_source(world, workspace);
    }
}
pub fn refresh_source(world: &mut World, workspace: Entity) -> Option<SourceBinding> {
    if world.get::<Workspace>(workspace).is_none() {
        return None;
    }
    let focused = Workbench::for_entity(world, workspace).ok().and_then(|w| {
        let layout = active_layout(w.world(), workspace)?;
        w.focused_panel(layout)
    });
    let current = focused.and_then(|panel| {
        code_document_for_panel(world, panel).map(|document| SourceBinding { panel, document })
    });
    let remembered = world
        .get::<StructureSource>(workspace)
        .and_then(|state| state.source)
        .filter(|source| {
            code_document_for_panel(world, source.panel) == Some(source.document)
                && inspect_document(world, source.document)
                    .is_ok_and(|info| info.workspace == workspace)
        });
    let source = current.or(remembered);
    world
        .entity_mut(workspace)
        .insert(StructureSource { source });
    source
}
/// Reject obsolete clicks before changing layout, focus or editor selection.
/// No other viewer is substituted, and closed panels are never reopened.
pub fn activate(world: &mut World, activation: Activation) -> Result<bool, EditorError> {
    let source = activation.source;
    if code_document_for_panel(world, source.panel) != Some(source.document) {
        return Ok(false);
    }
    let Some(observation) =
        analysis_for_panel(world, source.panel).filter(|o| o.key == activation.observation)
    else {
        return Ok(false);
    };
    let Some(item) = observation.outline.iter().find(|n| n.id == activation.item) else {
        return Ok(false);
    };
    let handle = cast_document::<TextBuffer>(world, source.document)
        .map_err(|e| EditorError(e.to_string()))?;
    let snapshot = TextSnapshot {
        revision: observation.key.revision,
        text: read_document(world, handle)
            .map_err(|e| EditorError(e.to_string()))?
            .clone(),
    };
    let navigation = Navigation {
        revision: snapshot.revision,
        range: item.name_range.clone(),
        selection: Some(TextSelection::new(
            &snapshot.text,
            item.name_range.start,
            item.name_range.end,
        )?),
        reveal: Reveal::Center,
        focus: true,
    };
    match navigate_editor(world, source.panel, navigation) {
        Ok(()) => Ok(true),
        // Composition/retirement/rebinding races have no activation effects.
        Err(_) => Ok(false),
    }
}
impl Panel for StructurePanel {
    const KEY: &'static str = "bed.structure";
    const LABEL: &'static str = "Structure";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Ok(workbench) = Workbench::for_entity(world, panel) else {
            return;
        };
        let workspace = workbench.workspace_id().entity();
        let source = refresh_source(world, workspace);
        let mut state = world.entity_mut(panel).take::<Self>().unwrap_or_default();
        let mut activation = None;
        if let Some((source, observation)) = state.refresh_outline(world, source) {
            let height = (context.theme.metrics.font_size + 10.0).max(24.0);
            ui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
            ui.spacing_mut().interact_size.y = height - 2.0;
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            let mut toggle = None;
            // Descendants retain their order while each closing branch loses height.
            // Compute positions once, then allocate and paint only viewport rows.
            let mut rows = Vec::new();
            let mut ancestors: Vec<(usize, f32)> = Vec::new();
            let mut top = 0.0;
            let stride = height;
            for (index, node) in observation.outline.iter().enumerate() {
                while ancestors
                    .last()
                    .is_some_and(|(depth, _)| *depth >= node.depth)
                {
                    ancestors.pop();
                }
                let visibility = ancestors.last().map_or(1.0, |(_, visibility)| *visibility);
                let children = observation
                    .outline
                    .get(index + 1)
                    .is_some_and(|n| n.parent == Some(index));
                let openness = if children {
                    bedtime_ui2::animate_expansion(
                        ui.ctx(),
                        egui::Id::new((panel, source.panel, node.id, "structure-expansion")),
                        !state.collapsed.contains(&node.id),
                        context.motion,
                    )
                } else {
                    1.0
                };
                if visibility > 0.0 {
                    rows.push((index, top, stride * visibility, openness));
                }
                top += stride * visibility;
                ancestors.push((node.depth, visibility * openness));
            }
            egui::ScrollArea::both()
                .id_salt(("code-structure", panel))
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    if observation.outline.is_empty() {
                        egui::Frame::new().inner_margin(14).show(ui, |ui| {
                            ui.weak("No declarations");
                        });
                    }
                    let origin = ui.cursor().min;
                    ui.set_min_size(egui::vec2(viewport.width(), top));
                    let first = rows.partition_point(|(_, y, h, _)| y + h < viewport.top());
                    for &(index, y, row_height, openness) in &rows[first..] {
                        if y > viewport.bottom() {
                            break;
                        }
                        let node = &observation.outline[index];
                        let children = observation
                            .outline
                            .get(index + 1)
                            .is_some_and(|n| n.parent == Some(index));
                        let rect = egui::Rect::from_min_size(
                            origin + egui::vec2(0.0, y),
                            egui::vec2(viewport.width(), row_height),
                        );
                        ui.scope_builder(
                            egui::UiBuilder::new().id_salt(node.id).max_rect(rect),
                            |ui| {
                                ui.set_clip_rect(ui.clip_rect().intersect(rect));
                                let x = rect.left() + 8.0 + node.depth.min(24) as f32 * 16.0;
                                if children {
                                    let arrow = egui::Rect::from_min_size(
                                        egui::pos2(x, rect.top()),
                                        egui::vec2(14.0, height),
                                    );
                                    let response = ui.interact(
                                        arrow,
                                        ui.id().with("arrow"),
                                        egui::Sense::click(),
                                    );
                                    bedtime_ui2::paint_chevron(
                                        ui.painter(),
                                        arrow.center(),
                                        openness,
                                        ui.visuals().text_color(),
                                    );
                                    if response.clicked() {
                                        toggle = Some(node.id);
                                    }
                                }
                                let selected = state.selected == Some(node.id);
                                let ink = if selected {
                                    bedtime_ui2::rainbow_ink(ui, context.motion)
                                } else {
                                    ui.visuals().text_color()
                                };
                                let font = egui::TextStyle::Body.resolve(ui.style());
                                let text =
                                    ui.painter().layout_no_wrap(node.label.clone(), font, ink);
                                let label = egui::Rect::from_center_size(
                                    egui::pos2(x + 18.0 + text.size().x / 2.0, rect.center().y),
                                    egui::vec2(
                                        text.size().x + 2.0 * ui.spacing().button_padding.x,
                                        height - 2.0,
                                    ),
                                );
                                let response = ui.put(
                                    label,
                                    egui::Button::selectable(
                                        selected,
                                        egui::RichText::new(&node.label).color(ink),
                                    ),
                                );
                                if response.clicked() {
                                    activation = Some(Activation {
                                        source,
                                        observation: observation.key,
                                        item: node.id,
                                    });
                                }
                                response.on_hover_text(&node.kind);
                            },
                        );
                    }
                });
            if let Some(id) = toggle {
                if !state.collapsed.remove(&id) {
                    state.collapsed.insert(id);
                }
            }
        } else {
            egui::Frame::new().inner_margin(14).show(ui, |ui| {
                ui.weak("No structure available");
            });
        }
        if let Some(activation) = activation
            && activate(world, activation).unwrap_or(false)
        {
            state.selected = Some(activation.item);
        }
        world.entity_mut(panel).insert(state);
    }
}

#[cfg(test)]
mod tests;
