//! The editor's breadcrumbs: the entries around its caret, outermost first,
//! in a strip below the text. Clicking one lists it and its siblings to go to.
use super::{
    Reveal, current_outline,
    symbols::{self, Category},
};
use crate::{
    editor::EditorPanel,
    syntax::{DocumentSyntax, Observation},
};
use bedtime_ui::{Breadcrumbs, MenuItem, UiContext, egui};
use bedtime_workbench::{BedtimeWorldExt, DocumentReference};
use bevy::prelude::*;
use std::sync::Arc;

/// On an editor: the trail last drawn, from an analysis of its text as it
/// was, kept while the text is reanalysed so it doesn't flicker.
#[derive(Component, Clone)]
pub(crate) struct Trail {
    observation: Arc<Observation>,
    /// Entry indices, outermost first.
    path: Vec<usize>,
}

#[cfg(test)]
impl Trail {
    pub(crate) fn labels(&self) -> Vec<String> {
        self.path
            .iter()
            .map(|&index| self.observation.outline[index].label.clone())
            .collect()
    }
}

/// The strip's height.
pub(crate) fn height(context: &UiContext<'_>) -> f32 {
    (context.metrics.font_size + 8.0).max(22.0)
}

/// Draw `panel`'s breadcrumbs at the top of `ui` (the strip below the
/// text), taking their height.
pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    update(world, panel);
    let title = world
        .get::<DocumentReference>(panel)
        .and_then(|reference| world.bedtime().inspect_document(reference.document()).ok())
        .map(|info| info.title)
        .unwrap_or_default();
    let trail = world.get::<Trail>(panel).cloned();
    let height = height(context);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let palette = &context.theme.palette;
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        egui::Stroke::new(
            1.0,
            bedtime_ui::theme::color(palette.border).gamma_multiply(0.6),
        ),
    );
    let mut chosen = None;
    let mut strip = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    strip.set_clip_rect(rect.intersect(ui.clip_rect()));
    let mut names = vec![title];
    let mut crumbs = Breadcrumbs::new(Vec::<String>::new());
    if let Some(trail) = &trail {
        let outline = &trail.observation.outline;
        for (at, &index) in trail.path.iter().enumerate() {
            let node = &outline[index];
            let category = Category::of(&node.kind);
            names.push(node.label.clone());
            crumbs = crumbs.icon(
                at + 1,
                category.icon(),
                bedtime_ui::theme::color(category.ink(&palette.syntax)),
            );
        }
    }
    let crumbs = crumbs.names(names).plain(0);
    // Each one opens a menu of what's beside it; the file's name does nothing.
    crumbs.show_with(&mut strip, |ui, at, crumb| {
        let Some(trail) = trail.as_ref().filter(|_| at > 0) else {
            return;
        };
        let outline = &trail.observation.outline;
        let index = trail.path[at - 1];
        let node = &outline[index];
        egui::Popup::menu(crumb)
            .align(egui::RectAlign::TOP_START)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(180.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for sibling in outline
                            .iter()
                            .enumerate()
                            .filter(|(_, other)| other.parent == node.parent)
                        {
                            let label = sibling.1.label.as_str();
                            let response = ui.add(MenuItem::new(label).checked(sibling.0 == index));
                            if sibling.0 == index && response.rect.is_positive() {
                                // Open at the current one.
                                if ui
                                    .ctx()
                                    .memory(|m| m.data.get_temp::<bool>(crumb.id.with("opened")))
                                    != Some(true)
                                {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                    ui.ctx().memory_mut(|m| {
                                        m.data.insert_temp(crumb.id.with("opened"), true)
                                    });
                                }
                            }
                            if response.clicked() {
                                chosen = Some((trail.observation.key, sibling.1.id));
                                ui.close();
                            }
                        }
                    });
            });
        if !egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(crumb)) {
            ui.ctx()
                .memory_mut(|m| m.data.remove::<bool>(crumb.id.with("opened")));
        }
    });
    if let Some((key, item)) = chosen {
        super::reveal_entry_as(world, panel, key, item, Reveal::Focus);
    }
}

/// Work the trail out again from the current analysis, if there is one.
pub(super) fn update(world: &mut World, panel: Entity) {
    let Some(observation) = current_outline(world, panel) else {
        // Being reanalysed: keep the trail, unless the text is another one.
        let same = world.get::<Trail>(panel).is_some_and(|trail| {
            world
                .get::<DocumentReference>(panel)
                .and_then(|reference| world.get::<DocumentSyntax>(reference.document().entity()))
                .and_then(|syntax| syntax.session.key())
                .is_some_and(|key| {
                    key.source == trail.observation.key.source
                        && key.language_generation == trail.observation.key.language_generation
                })
        });
        if !same {
            world.entity_mut(panel).remove::<Trail>();
        }
        return;
    };
    let Some(head) = world
        .get::<EditorPanel>(panel)
        .map(|editor| editor.view.selection().head())
    else {
        return;
    };
    let mut path: Vec<usize> = symbols::entry_at(&observation.outline, head)
        .map(|index| symbols::ancestry(&observation.outline, index).collect())
        .unwrap_or_default();
    path.reverse();
    let unchanged = world
        .get::<Trail>(panel)
        .is_some_and(|trail| trail.observation.key == observation.key && trail.path == path);
    if !unchanged {
        world.entity_mut(panel).insert(Trail { observation, path });
    }
}
