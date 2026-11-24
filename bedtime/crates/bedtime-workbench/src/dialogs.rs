//! The unsaved-changes prompt, drawn in bedtime-ui's modal layer.
use crate::*;
use bedtime_ui::egui;
use bevy::prelude::*;

/// Ask what to do with unsaved documents before a close or quit goes ahead.
/// Drawn on the first workbench window, like error dialogs. Escape cancels.
pub(crate) fn draw_close_prompt(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
) -> bool {
    let ctx = &ui.ctx().clone();
    let surface = context.surface;
    let Some(prompt) = world.bedtime().close_prompt() else {
        return false;
    };
    let owner = world
        .query_filtered::<Entity, (With<LayoutSurface>, With<Window>)>()
        .iter(world)
        .next();
    if owner != Some(surface) {
        return false;
    }
    let many = prompt.documents.len() > 1;
    let failed = prompt
        .documents
        .iter()
        .any(|document| document.error.is_some());
    let response =
        egui::Modal::new(egui::Id::new(("workbench-close-prompt", surface))).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.0).clamp(0.0, 460.0));
            ui.heading(format!(
                "{} with unsaved changes?",
                prompt.exit.as_deref().unwrap_or("Close")
            ));
            ui.label(if failed {
                "Some changes couldn't be saved."
            } else if many {
                "These documents have unsaved changes:"
            } else {
                "This document has unsaved changes:"
            });
            for document in &prompt.documents {
                ui.label(egui::RichText::new(&document.title).strong());
                if let Some(error) = &document.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = match (failed, many) {
                    (true, _) => "Retry",
                    (false, true) => "Save all",
                    (false, false) => "Save",
                };
                let mut choice = None;
                if ui.button(save).clicked() {
                    choice = Some(crate::CloseChoice::Save);
                }
                if ui
                    .button(if many { "Discard all" } else { "Discard" })
                    .clicked()
                {
                    choice = Some(crate::CloseChoice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(crate::CloseChoice::Cancel);
                }
                choice
            })
            .inner
        });
    // As with error dialogs, keep these keys from reaching panels underneath.
    let escape = ctx.input_mut(|input| {
        let pressed = input.key_pressed(egui::Key::Escape);
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Escape | egui::Key::Enter,
                    ..
                }
            )
        });
        pressed
    });
    if let Some(choice) = response
        .inner
        .or(escape.then_some(crate::CloseChoice::Cancel))
    {
        world
            .bedtime_mut()
            .resolve_close(choice)
            .expect("A prompt was showing");
    }
    true
}

/// Save would write over an outside change: ask whether to. Overwrite keeps
/// this content and saves it; Reload takes the outside change instead.
pub(crate) fn draw_overwrite_question(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
) -> bool {
    let ctx = &ui.ctx().clone();
    let surface = context.surface;
    let Some(document) = world
        .resource::<crate::document_actions::OverwriteQuestion>()
        .0
    else {
        return false;
    };
    let Ok(info) = world.bedtime().inspect_document(document) else {
        world
            .resource_mut::<crate::document_actions::OverwriteQuestion>()
            .0 = None;
        return false;
    };
    let owner = world
        .query_filtered::<Entity, (With<LayoutSurface>, With<Window>)>()
        .iter(world)
        .next();
    if owner != Some(surface) {
        return false;
    }
    #[derive(Clone, Copy)]
    enum Answer {
        Overwrite,
        Reload,
        Cancel,
    }
    let response =
        egui::Modal::new(egui::Id::new(("workbench-overwrite", surface))).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 48.0).clamp(0.0, 460.0));
            ui.heading(format!("{} changed on disk", info.title));
            ui.label(
                "Something else changed it since it was opened here. Saving writes \
             your version over those changes.",
            );
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut answer = None;
                if ui.button("Overwrite").clicked() {
                    answer = Some(Answer::Overwrite);
                }
                if ui.button("Reload Theirs").clicked() {
                    answer = Some(Answer::Reload);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(Answer::Cancel);
                }
                answer
            })
            .inner
        });
    let escape = ctx.input_mut(|input| {
        let pressed = input.key_pressed(egui::Key::Escape);
        input.events.retain(|event| {
            !matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Escape | egui::Key::Enter,
                    ..
                }
            )
        });
        pressed
    });
    let Some(answer) = response.inner.or(escape.then_some(Answer::Cancel)) else {
        return true;
    };
    world
        .resource_mut::<crate::document_actions::OverwriteQuestion>()
        .0 = None;
    let result = match answer {
        Answer::Cancel => Ok(()),
        Answer::Reload => world
            .bedtime_mut()
            .settle_document_conflict(document, ConflictChoice::Reload)
            .map(|_| ()),
        Answer::Overwrite => world
            .bedtime_mut()
            .settle_document_conflict(document, ConflictChoice::KeepMine)
            .and_then(|_| world.bedtime_mut().save_document(document)),
    };
    if let Err(error) = result {
        crate::exit::report_error(
            world,
            "Save",
            "The document couldn't be saved.",
            Some(error.to_string()),
        );
    }
    true
}

/// A bar at the top of a panel whose document needs attention: it changed
/// on disk while it had unsaved changes, or it's gone from disk.
pub(crate) fn document_header(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
    panel: Entity,
) {
    let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
        return;
    };
    let Ok(info) = world.bedtime().inspect_document(document) else {
        return;
    };
    let (message, actions): (&str, &[(&str, HeaderAction)]) = if info.conflict {
        (
            "Changed on disk while you were editing it.",
            &[
                ("Keep Mine", HeaderAction::KeepMine),
                ("Reload", HeaderAction::Reload),
            ],
        )
    } else if info.missing {
        if info.writable {
            (
                "Deleted on disk. Save puts it back.",
                &[("Save", HeaderAction::Save)],
            )
        } else {
            ("Deleted on disk.", &[])
        }
    } else {
        return;
    };
    let palette = &context.theme.palette;
    let mut clicked = None;
    egui::Frame::NONE
        .fill(bedtime_ui::theme::color(palette.surface))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.colored_label(bedtime_ui::theme::color(palette.warning), message);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, action) in actions.iter().rev() {
                        if ui.small_button(*label).clicked() {
                            clicked = Some(*action);
                        }
                    }
                });
            });
        });
    let result = match clicked {
        None => return,
        Some(HeaderAction::KeepMine) => world
            .bedtime_mut()
            .settle_document_conflict(document, ConflictChoice::KeepMine)
            .map(|_| ()),
        Some(HeaderAction::Reload) => world
            .bedtime_mut()
            .settle_document_conflict(document, ConflictChoice::Reload)
            .map(|_| ()),
        Some(HeaderAction::Save) => world.bedtime_mut().save_document(document),
    };
    if let Err(error) = result {
        crate::exit::report_error(
            world,
            "Save",
            "The document couldn't be saved.",
            Some(error.to_string()),
        );
    }
}

#[derive(Clone, Copy)]
enum HeaderAction {
    KeepMine,
    Reload,
    Save,
}
