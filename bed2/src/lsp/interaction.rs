//! Per-view interaction state. Worker results are accepted before UI drawing;
//! document edits are queued into Editor2 and checked again at application.
use super::*;
use bedtime_editor2::{EditorOutput, SymbolHit, workbench::WorkbenchEditor};
use bedtime_lsp2::{
    Completions, Hover, Intelligence, LanguageQuery, QueryKind, RequestOrigin, completion_context,
};

#[derive(Component, Default)]
struct Interaction {
    hover_target: Option<(u64, SymbolHit)>,
    hover_origin: Option<RequestOrigin>,
    hover: Option<Hover>,
    hover_rect: Option<egui::Rect>,
    hover_capable: bool,
    hover_armed_at: Option<f64>,
    hover_epoch: u64,
    documentation: Option<super::documentation::Documentation>,
    completion: Option<CompletionSession>,
    error: Option<String>,
    completion_rect: Option<egui::Rect>,
    documentation_rect: Option<egui::Rect>,
}
struct CompletionSession {
    origin: RequestOrigin,
    byte: usize,
    list: Option<Completions>,
    resolving: Option<(RequestOrigin, usize, bool)>,
    resolved: std::collections::HashSet<usize>,
}
pub(super) fn receive(world: &mut World) {
    for reply in take_intelligence(world) {
        if !query_is_current(world, &reply.origin, reply.kind) {
            continue;
        }
        let Some(panel) = Entity::try_from_bits(reply.origin.panel) else {
            continue;
        };
        let Some(mut state) = world.get_mut::<Interaction>(panel) else {
            continue;
        };
        if reply.result.is_ok() {
            state.error = None;
        }
        match reply.result {
            Ok(Intelligence::Hover(hover)) if state.hover_origin == Some(reply.origin) => {
                state.hover = hover;
            }
            Ok(Intelligence::Completion(list)) => {
                if let Some(session) = &mut state.completion {
                    if session.origin == reply.origin {
                        let choices = list.choices.clone();
                        let revision = reply.origin.revision;
                        session.list = Some(list);
                        if let Some(mut editor) = world.get_mut::<WorkbenchEditor>(panel) {
                            editor.view.set_completions(revision, choices);
                        }
                    }
                }
            }
            Ok(Intelligence::Resolved(entry)) => {
                let mut accept = None;
                if let Some(session) = &mut state.completion {
                    if let Some((origin, index, accepting)) = session.resolving {
                        if origin == reply.origin {
                            if let Some(list) = &mut session.list {
                                if let Some(item) = list.items.get_mut(index) {
                                    *item = entry;
                                    if accepting {
                                        accept = Some(item.edit.clone());
                                    }
                                }
                            }
                            session.resolved.insert(index);
                            session.resolving = None;
                        }
                    }
                }
                if let Some(edit) = accept {
                    state.completion = None;
                    if let Some(mut editor) = world.get_mut::<WorkbenchEditor>(panel) {
                        editor.view.queue_completion(reply.origin.revision, edit);
                    }
                }
            }
            Err(error) => {
                // Optional hover/completion failures stay local to the interaction;
                // they do not turn a healthy synchronized server into an error.
                state.error = Some(error);
                if reply.kind == QueryKind::Resolve {
                    if let Some(session) = &mut state.completion {
                        if session
                            .resolving
                            .is_some_and(|(origin, _, _)| origin == reply.origin)
                        {
                            session.resolving = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
fn clear_completion(world: &mut World, panel: Entity, state: &mut Interaction) {
    if state.completion.take().is_none() {
        return;
    }
    cancel_query(world, panel, QueryKind::Completion);
    cancel_query(world, panel, QueryKind::Resolve);
    if let Some(mut editor) = world.get_mut::<WorkbenchEditor>(panel) {
        editor.view.clear_completions();
    }
}
fn resolve(
    world: &mut World,
    panel: Entity,
    state: &mut Interaction,
    index: usize,
    accepting: bool,
) {
    let Some(session) = &mut state.completion else {
        return;
    };
    let Some(item) = session.list.as_ref().and_then(|l| l.items.get(index)) else {
        return;
    };
    let supports = language_activity(world, panel).is_some_and(|a| a.supports_completion_resolve);
    if session.resolved.contains(&index) || !supports {
        if accepting {
            let edit = item.edit.clone();
            let revision = session.origin.revision;
            clear_completion(world, panel, state);
            if let Some(mut editor) = world.get_mut::<WorkbenchEditor>(panel) {
                editor.view.queue_completion(revision, edit);
            }
        }
        return;
    }
    if let Some((_, selected, accept)) = &mut session.resolving {
        if *selected == index {
            *accept |= accepting;
            return;
        }
    }
    match request_query(
        world,
        panel,
        LanguageQuery::Resolve {
            byte: session.byte,
            item: item.raw.clone(),
        },
    ) {
        Ok(origin) => session.resolving = Some((origin, index, accepting)),
        Err(error) => state.error = Some(error),
    }
}
pub(super) fn draw(ui: &mut egui::Ui, world: &mut World, panel: Entity, output: &EditorOutput) {
    let mut state = world
        .entity_mut(panel)
        .take::<Interaction>()
        .unwrap_or_default();
    state.completion_rect = output.completion_rect;
    state.documentation_rect = None;
    let activity = language_activity(world, panel);
    let eligible = activity.is_some() && !output.composing;
    if let Some(byte) = output.definition_clicked {
        clear_completion(world, panel, &mut state);
        if let Err(error) = request_definition_at(world, panel, byte) {
            super::show_error(world, panel, error);
        }
    }
    let focused = output.response.has_focus();
    let moved = state
        .completion
        .as_ref()
        .is_some_and(|s| s.origin.revision != output.revision || s.byte != output.selection.head());
    let incomplete = state
        .completion
        .as_ref()
        .and_then(|s| s.list.as_ref())
        .is_some_and(|l| l.incomplete);
    if moved || !eligible || !focused || output.completion_dismissed {
        clear_completion(world, panel, &mut state);
    }
    if eligible && focused {
        let trigger = output.typed.as_deref().and_then(|typed| {
            activity
                .as_ref()
                .unwrap()
                .completion_triggers
                .iter()
                .find(|t| typed.ends_with(t.as_str()))
        });
        let typed_identifier = output
            .typed
            .as_deref()
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_'));
        let automatic =
            output.selection.range().is_empty() && (typed_identifier || trigger.is_some());
        if activity.as_ref().unwrap().supports_completion
            && (output.completion_requested || output.completion_refresh || automatic)
        {
            clear_completion(world, panel, &mut state);
            let byte = output.selection.head();
            let context = completion_context(
                trigger.map(String::as_str),
                incomplete && !output.completion_requested,
            );
            match request_query(world, panel, LanguageQuery::Completion { byte, context }) {
                Ok(origin) => {
                    state.completion = Some(CompletionSession {
                        origin,
                        byte,
                        list: None,
                        resolving: None,
                        resolved: Default::default(),
                    })
                }
                Err(error) => state.error = Some(error),
            }
        }
        if let Some(index) = output.completion_accepted {
            resolve(world, panel, &mut state, index, true);
        } else if let Some(index) = world
            .get::<WorkbenchEditor>(panel)
            .and_then(|e| e.view.selected_completion())
        {
            resolve(world, panel, &mut state, index, false);
        }
    }
    // A popup remains readable while the pointer crosses its small attachment gap.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let inside_hover =
        pointer.is_some_and(|p| state.hover_rect.is_some_and(|r| r.expand(5.0).contains(p)));
    let diagnostic_hit = output.hovered_text.as_ref().filter(|hit| {
        world
            .get::<DocumentView>(panel)
            .and_then(|v| world.get::<DocumentLsp>(v.document().0))
            .and_then(|s| s.diagnostics.as_ref())
            .is_some_and(|d| {
                d.revision == output.revision && d.intersecting(hit.range.clone()).next().is_some()
            })
    });
    let target = if inside_hover {
        state.hover_target.clone()
    } else {
        output
            .hovered_symbol
            .clone()
            .or_else(|| diagnostic_hit.cloned())
            .map(|hit| (output.revision, hit))
    };
    let capable = activity.as_ref().is_some_and(|a| a.supports_hover);
    let epoch = activity.as_ref().map_or(0, |a| a.analysis_epoch);
    let refreshed_empty =
        state.hover_origin.is_some() && state.hover.is_none() && epoch != state.hover_epoch;
    let changed = refreshed_empty
        || capable != state.hover_capable
        || match (&target, &state.hover_target) {
            (Some((rev, a)), Some((old, b))) => rev != old || a.range != b.range,
            (None, None) => false,
            _ => true,
        };
    state.hover_capable = capable;
    state.hover_epoch = epoch;
    let now = ui.input(|i| i.time);
    if changed || !eligible || output.completion_dismissed || state.completion.is_some() {
        if state.hover_origin.is_some() {
            cancel_query(world, panel, QueryKind::Hover);
        }
        state.hover = None;
        state.hover_origin = None;
        state.hover_rect = None;
        state.hover_target = target.clone();
        state.hover_armed_at = (eligible
            && state.completion.is_none()
            && !output.completion_dismissed
            && target.is_some())
        .then_some(now);
    } else if let Some(target) = target {
        state.hover_target = Some(target);
    }
    let hover_ready = state.hover_armed_at.is_some_and(|armed| now - armed >= 0.2);
    if let Some(armed) = state.hover_armed_at {
        if !hover_ready {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64((0.2 - (now - armed)).max(0.0)));
        }
    }
    if hover_ready && state.hover_origin.is_none() && capable {
        if let Some((_, hit)) = &state.hover_target {
            if let Ok(origin) = request_query(world, panel, LanguageQuery::Hover { byte: hit.byte })
            {
                state.hover_origin = Some(origin);
            }
        }
    }
    if let Some(origin) = state.hover_origin {
        if !query_is_current(world, &origin, QueryKind::Hover) {
            state.hover = None;
            state.hover_target = None;
            state.hover_rect = None;
            state.hover_origin = None;
        }
    }
    let palette = world
        .get::<WorkbenchEditor>(panel)
        .map(|e| e.view.syntax_palette.clone())
        .unwrap_or_default();
    let language = world
        .get::<DocumentView>(panel)
        .and_then(|v| world.get::<LspDocument>(v.document().0))
        .map(|s| s.language.clone())
        .unwrap_or_default();
    if state.completion.is_none() {
        if let Some((revision, hit)) = &state.hover_target {
            if *revision == output.revision && eligible {
                let diagnostics = world
                    .get::<DocumentView>(panel)
                    .and_then(|v| world.get::<DocumentLsp>(v.document().0))
                    .and_then(|s| s.diagnostics.as_ref())
                    .filter(|d| d.revision == output.revision)
                    .map(|d| {
                        d.intersecting(hit.range.clone())
                            .take(16)
                            .map(|d| d.message.to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if hover_ready && (state.hover.is_some() || !diagnostics.is_empty()) {
                    if let Some(hover) = &state.hover {
                        super::documentation::prepare(
                            world,
                            panel,
                            &mut state.documentation,
                            &hover.contents,
                            &language,
                        );
                    } else {
                        state.documentation = None;
                    }
                    let result = egui::Area::new(egui::Id::new(("lsp.hover", panel)))
                        .order(egui::Order::Foreground)
                        .fixed_pos(hit.rect.left_bottom())
                        .constrain_to(ui.ctx().content_rect().shrink(8.0))
                        .show(ui.ctx(), |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                super::documentation::style(ui);
                                ui.set_width((ui.ctx().content_rect().width() - 40.0).min(460.0));
                                egui::ScrollArea::vertical()
                                    .max_height(320.0)
                                    .show(ui, |ui| {
                                        if let Some(documentation) = &mut state.documentation {
                                            documentation.show(ui, &palette);
                                        }
                                        for message in &diagnostics {
                                            ui.separator();
                                            ui.label(message);
                                        }
                                    });
                            });
                        });
                    state.hover_rect = Some(result.response.rect);
                }
            }
        }
    }
    if let Some(session) = &state.completion {
        if let Some(index) = world
            .get::<WorkbenchEditor>(panel)
            .and_then(|e| e.view.selected_completion())
        {
            if let Some(item) = session.list.as_ref().and_then(|list| list.items.get(index)) {
                if !item.detail.is_empty() || !item.documentation.text.is_empty() {
                    super::documentation::prepare(
                        world,
                        panel,
                        &mut state.documentation,
                        &item.documentation,
                        &language,
                    );
                    if let Some(menu) = output.completion_rect {
                        let screen = ui.ctx().content_rect().shrink(8.0);
                        if let Some(region) = documentation_region(screen, menu) {
                            let width = region.width().min(400.0) - 16.0;
                            let position = if region.right() <= menu.left() {
                                egui::pos2(region.right() - width - 16.0, menu.top())
                            } else if region.bottom() <= menu.top() {
                                egui::pos2(
                                    menu.left(),
                                    region.bottom() - region.height().min(300.0),
                                )
                            } else {
                                egui::pos2(region.left(), menu.top().max(region.top()))
                            };
                            let result =
                                egui::Area::new(egui::Id::new(("lsp.completion.docs", panel)))
                                    .order(egui::Order::Foreground)
                                    .fixed_pos(position)
                                    .constrain_to(region)
                                    .show(ui.ctx(), |ui| {
                                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                                            super::documentation::style(ui);
                                            ui.set_width(width.max(100.0));
                                            egui::ScrollArea::vertical()
                                                .max_height((region.height() - 24.0).min(300.0))
                                                .show(ui, |ui| {
                                                    if !item.detail.is_empty() {
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(&item.detail)
                                                                    .monospace(),
                                                            )
                                                            .wrap()
                                                            .selectable(true),
                                                        );
                                                    }
                                                    if let Some(documentation) =
                                                        &mut state.documentation
                                                    {
                                                        documentation.show(ui, &palette);
                                                    }
                                                });
                                        });
                                    });
                            state.documentation_rect = Some(result.response.rect);
                        }
                    }
                }
            }
        }
    }
    if state.hover_rect.is_none() && state.documentation_rect.is_none() {
        state.documentation = None;
    }
    world.entity_mut(panel).insert(state);
}
// Constrain documentation to a region that excludes the suggestion list,
// including narrow windows where it must go above or below the list.
fn documentation_region(screen: egui::Rect, menu: egui::Rect) -> Option<egui::Rect> {
    let right = egui::Rect::from_min_max(egui::pos2(menu.right() + 8.0, screen.top()), screen.max);
    let left = egui::Rect::from_min_max(screen.min, egui::pos2(menu.left() - 8.0, screen.bottom()));
    if right.width() >= 220.0 {
        return Some(right);
    }
    if left.width() >= 220.0 {
        return Some(left);
    }
    let below =
        egui::Rect::from_min_max(egui::pos2(screen.left(), menu.bottom() + 8.0), screen.max);
    let above = egui::Rect::from_min_max(screen.min, egui::pos2(screen.right(), menu.top() - 8.0));
    let region = if below.height() >= above.height() {
        below
    } else {
        above
    };
    (region.height() >= 80.0).then_some(region)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documentation_placement_stays_outside_completion_list_at_window_edges() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960.0, 640.0));
        for (position, size) in [
            (egui::pos2(590.0, 150.0), egui::vec2(360.0, 240.0)),
            (egui::pos2(10.0, 150.0), egui::vec2(360.0, 240.0)),
            (egui::pos2(100.0, 400.0), egui::vec2(760.0, 230.0)),
        ] {
            let menu = egui::Rect::from_min_size(position, size);
            let region = documentation_region(screen, menu).unwrap();
            assert!(screen.contains_rect(region));
            assert!(!region.intersects(menu));
        }
    }
}
#[cfg(feature = "automation")]
pub(super) fn inspect(
    world: &mut World,
    _: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let panels: Vec<_> = world.query::<(Entity,&Interaction)>().iter(world).map(|(panel,state)| {
        json!({"panel":panel.to_bits().to_string(),"hover_capable":state.hover_capable,"hover_requested":state.hover_origin.is_some(),"hover_target":state.hover_target.as_ref().map(|(rev,h)|json!({"revision":rev,"byte":h.byte,"start":h.range.start,"end":h.range.end})),"hover":state.hover.as_ref().map(|h| &h.contents.text),"hover_rect":state.hover_rect.map(|r| json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),
            "syntax_highlights":state.documentation.as_ref().map(|d| d.highlight_count()),"completion_rect":state.completion_rect.map(|r|json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"documentation_rect":state.documentation_rect.map(|r|json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"completion":state.completion.as_ref().map(|s| json!({"revision":s.origin.revision,"byte":s.byte,"items":s.list.as_ref().map(|l| l.items.iter().map(|i| json!({"label":i.label,"detail":i.detail,"documentation":i.documentation.text})).collect::<Vec<_>>()),"resolving":s.resolving.is_some()})),"error":state.error})
    }).collect();
    Ok(json!({"panels":panels}))
}
