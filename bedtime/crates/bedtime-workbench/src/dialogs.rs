//! The unsaved-changes prompt, drawn in bedtime-ui's modal layer.
use crate::*;
use bedtime_ui::egui;
use bevy::prelude::*;

/// Ask what to do with unsaved documents before a close or quit goes ahead.
/// Drawn on the first workbench window, like error dialogs. Escape cancels.
pub(crate) fn draw_close_prompt(ctx: &egui::Context, world: &mut World, surface: Entity) -> bool {
    let Some(prompt) = world.workbench().close_prompt() else {
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
            .workbench_mut()
            .resolve_close(choice)
            .expect("A prompt was showing");
    }
    true
}
