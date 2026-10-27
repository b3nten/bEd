//! A small application-wide queue of human-readable error dialogs.
use crate::WorkbenchSurface;
use bedtime_ui::egui;
use bevy::prelude::*;
use std::collections::VecDeque;

const MAX_ERROR_DIALOGS: usize = 16;

/// An error explained to the user, with optional diagnostic information.
///
/// Callers supply a domain-specific message rather than putting raw I/O errors
/// in `message`. Those diagnostics belong in the initially collapsed `details`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorDialog {
    pub title: String,
    pub message: String,
    pub details: Option<String>,
}

struct QueuedDialog {
    dialog: ErrorDialog,
    additional_errors: Option<u64>,
}

impl QueuedDialog {
    fn overflow(count: u64) -> Self {
        let mut entry = Self {
            dialog: ErrorDialog {
                title: "More errors occurred".into(),
                message: String::new(),
                details: None,
            },
            additional_errors: Some(count),
        };
        entry.update_overflow_message();
        entry
    }

    fn update_overflow_message(&mut self) {
        let count = self.additional_errors.expect("Overflow error count");
        let at_least = if count == u64::MAX { "At least " } else { "" };
        self.dialog.message = format!(
            "{at_least}{count} additional errors occurred while the error dialog queue was full. \
             Their details were not retained. Dismiss this dialog before trying again."
        );
    }
}

/// Pending error dialogs, including the currently presented dialog.
///
/// Created lazily by [`request_error_dialog`], without requiring any installed
/// plugin, panel, window or egui context. Requests remain here until
/// a workbench surface can present them. At most 16 entries are retained.
#[derive(Resource, Default)]
pub struct ErrorDialogs {
    queue: VecDeque<QueuedDialog>,
    owner: Option<Entity>,
    presentation: u64,
}

impl ErrorDialogs {
    /// Number of queued dialogs, including the active one and any overflow summary.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The active dialog, or the next one if no surface is currently drawing.
    pub fn front(&self) -> Option<&ErrorDialog> {
        self.queue.front().map(|entry| &entry.dialog)
    }

    fn request(&mut self, dialog: ErrorDialog) {
        if self.queue.iter().any(|entry| entry.dialog == dialog) {
            return;
        }
        // Once overflow starts, retain one count instead of collecting more
        // diagnostics, even as earlier dialogs are dismissed. Omitted requests
        // cannot be deduplicated: their identities are deliberately not retained.
        if let Some(overflow) = self
            .queue
            .iter_mut()
            .find(|entry| entry.additional_errors.is_some())
        {
            let count = overflow.additional_errors.as_mut().unwrap();
            *count = count.saturating_add(1);
            overflow.update_overflow_message();
            return;
        }
        if self.queue.len() < MAX_ERROR_DIALOGS {
            self.queue.push_back(QueuedDialog {
                dialog,
                additional_errors: None,
            });
        } else {
            // Count both the displaced last request and the incoming request.
            // The active (front) dialog is never displaced.
            self.queue.pop_back();
            self.queue.push_back(QueuedDialog::overflow(2));
        }
    }

    fn dismiss(&mut self) {
        self.queue.pop_front();
        self.owner = None;
        // Each dialog starts with its own collapsed technical-details state.
        self.presentation = self.presentation.wrapping_add(1);
    }
}

/// Queue an error independently of its originating panel.
///
/// Identical active/pending dialogs are deduplicated. After dismissal an
/// identical request is accepted again, so callers must edge-detect persistent
/// errors rather than submitting them every frame. Overflow is coalesced into
/// a bounded summary counting omitted requests, not distinct error identities.
pub fn request_error_dialog(world: &mut World, dialog: ErrorDialog) {
    world.init_resource::<ErrorDialogs>();
    world.resource_mut::<ErrorDialogs>().request(dialog);
}

/// Called by the workbench root before drawing panels, including empty/hidden
/// workbenches. The dialog retains its window until dismissed or that window's
/// workbench surface is removed; requests survive the absence of any window.
pub(crate) fn draw(ctx: &egui::Context, world: &mut World, surface: Entity) -> bool {
    let Some(dialogs) = world.get_resource::<ErrorDialogs>() else {
        return false;
    };
    if dialogs.is_empty() {
        return false;
    }
    let owner = dialogs.owner.filter(|&owner| {
        world.get::<Window>(owner).is_some() && world.get::<WorkbenchSurface>(owner).is_some()
    });
    let owner = owner.or_else(|| {
        world
            .query_filtered::<Entity, (With<WorkbenchSurface>, With<Window>)>()
            .iter(world)
            .next()
    });
    let mut dialogs = world.resource_mut::<ErrorDialogs>();
    dialogs.owner = owner;
    if owner != Some(surface) {
        return false;
    }
    let dialog = dialogs.front().unwrap();
    let id = egui::Id::new(("workbench-error-dialog", surface));
    let presentation = dialogs.presentation;
    let response = egui::Modal::new(id).show(ctx, |ui| {
        let width = (ctx.content_rect().width() - 48.0).clamp(0.0, 460.0);
        ui.set_width(width);
        ui.heading(&dialog.title);
        ui.add(egui::Label::new(&dialog.message).wrap());
        if let Some(details) = &dialog.details {
            egui::CollapsingHeader::new("Technical details")
                .id_salt(presentation)
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(details).monospace())
                                    .selectable(true)
                                    .wrap(),
                            );
                        });
                });
        }
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.button("OK").clicked()
        })
        .inner
    });
    // Raw egui key reads in panel gestures are not automatically blocked by a
    // modal layer. Remove both dismissal keys before panels run, including on
    // the closing frame. Ignore backdrop clicks: acknowledgment is explicit.
    let dismiss_key = ctx.input_mut(|input| {
        let pressed = input.key_pressed(egui::Key::Escape) || input.key_pressed(egui::Key::Enter);
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
    // egui's top-modal observation describes the previous frame. The first
    // presentation must also accept dismissal without leaking the key beneath.
    let can_dismiss =
        response.is_top_modal || ctx.memory(|memory| memory.top_modal_layer().is_none());
    if response.inner || can_dismiss && dismiss_key {
        dialogs.dismiss();
    }
    true
}

/// Ask what to do with unsaved documents before a close or quit goes ahead.
/// Drawn on the first workbench window, like error dialogs. Escape cancels.
pub(crate) fn draw_close_prompt(ctx: &egui::Context, world: &mut World, surface: Entity) -> bool {
    let Some(prompt) = crate::close_prompt(world) else {
        return false;
    };
    let owner = world
        .query_filtered::<Entity, (With<WorkbenchSurface>, With<Window>)>()
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
            ui.heading(if prompt.quitting {
                "Quit with unsaved changes?"
            } else {
                "Close with unsaved changes?"
            });
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
        crate::resolve_close(world, choice).expect("A prompt was showing");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(name: &str) -> ErrorDialog {
        ErrorDialog {
            title: "Could not complete the operation".into(),
            message: name.into(),
            details: Some(format!("diagnostic for {name}")),
        }
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            events,
            ..default()
        }
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn render(
        ctx: &egui::Context,
        world: &mut World,
        surface: Entity,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(input(events), |ui| {
            draw(ui.ctx(), world, surface);
        });
        // These headless tests inspect shapes but intentionally do not upload textures.
        output.textures_delta.clear();
        output
    }

    #[test]
    fn requests_are_lazy_headless_and_outlive_their_origin() {
        let mut world = World::new();
        let ctx = egui::Context::default();
        assert!(!world.contains_resource::<ErrorDialogs>());
        let mut output = ctx.run_ui(input(vec![]), |ui| {
            assert!(!draw(ui.ctx(), &mut world, Entity::PLACEHOLDER));
        });
        output.textures_delta.clear();
        assert!(!world.contains_resource::<ErrorDialogs>());

        let origin = world.spawn_empty().id();
        request_error_dialog(&mut world, error("Refresh Files and try again."));
        world.despawn(origin);
        let mut output = ctx.run_ui(input(vec![]), |ui| {
            assert!(!draw(ui.ctx(), &mut world, Entity::PLACEHOLDER));
        });
        output.textures_delta.clear();
        let dialogs = world.resource::<ErrorDialogs>();
        assert_eq!(dialogs.len(), 1);
        assert!(!dialogs.is_empty());
        assert_eq!(
            dialogs.front(),
            Some(&error("Refresh Files and try again."))
        );
    }

    #[test]
    fn retained_duplicates_are_deduplicated_in_order_but_dismissed_errors_can_retry() {
        let mut world = World::new();
        let surface = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let ctx = egui::Context::default();
        let first = error("First operation failed.");
        let second = error("Second operation failed.");
        request_error_dialog(&mut world, first.clone());
        request_error_dialog(&mut world, first.clone());
        request_error_dialog(&mut world, second.clone());
        request_error_dialog(&mut world, second.clone());
        let _ = render(&ctx, &mut world, surface, vec![]);
        request_error_dialog(&mut world, first.clone()); // Also deduplicate the active dialog.
        assert_eq!(world.resource::<ErrorDialogs>().len(), 2);
        assert_eq!(world.resource::<ErrorDialogs>().front(), Some(&first));

        let _ = render(&ctx, &mut world, surface, vec![key(egui::Key::Enter)]);
        assert_eq!(world.resource::<ErrorDialogs>().front(), Some(&second));
        request_error_dialog(&mut world, first.clone());
        assert_eq!(world.resource::<ErrorDialogs>().len(), 2);
        let _ = render(&ctx, &mut world, surface, vec![key(egui::Key::Escape)]);
        assert_eq!(world.resource::<ErrorDialogs>().front(), Some(&first));
        let _ = render(&ctx, &mut world, surface, vec![key(egui::Key::Enter)]);
        assert!(world.resource::<ErrorDialogs>().is_empty());
    }

    #[test]
    fn different_diagnostics_are_not_silently_deduplicated() {
        let mut world = World::new();
        let first = error("Could not save the file.");
        let mut second = first.clone();
        second.details = Some("A different file or cause".into());
        request_error_dialog(&mut world, first.clone());
        request_error_dialog(&mut world, second.clone());
        assert_eq!(world.resource::<ErrorDialogs>().len(), 2);
        world.resource_mut::<ErrorDialogs>().dismiss();
        assert_eq!(world.resource::<ErrorDialogs>().front(), Some(&second));
    }

    #[test]
    fn overflow_stays_bounded_and_counts_displaced_and_later_requests_honestly() {
        let mut world = World::new();
        for index in 0..200 {
            request_error_dialog(&mut world, error(&format!("Error {index}")));
            assert!(world.resource::<ErrorDialogs>().len() <= MAX_ERROR_DIALOGS);
        }
        assert_eq!(world.resource::<ErrorDialogs>().len(), 16);
        request_error_dialog(&mut world, error("Error 0"));
        world.resource_mut::<ErrorDialogs>().dismiss();
        // Freed slots do not restart collecting details while overflow remains.
        request_error_dialog(&mut world, error("Another attempt"));
        assert_eq!(world.resource::<ErrorDialogs>().len(), 15);
        for index in 1..15 {
            assert_eq!(
                world.resource::<ErrorDialogs>().front(),
                Some(&error(&format!("Error {index}")))
            );
            world.resource_mut::<ErrorDialogs>().dismiss();
        }
        let dialogs = world.resource::<ErrorDialogs>();
        assert_eq!(dialogs.len(), 1);
        let summary = dialogs.front().unwrap();
        assert!(summary.message.starts_with("186 additional errors"));
        assert!(summary.message.contains("details were not retained"));
        assert_eq!(summary.details, None);
        assert!(summary.message.len() < 256);
        world.resource_mut::<ErrorDialogs>().dismiss();
        request_error_dialog(&mut world, error("Another attempt"));
        assert_eq!(
            world.resource::<ErrorDialogs>().front(),
            Some(&error("Another attempt"))
        );
    }

    #[test]
    fn saturated_overflow_count_is_reported_as_a_lower_bound() {
        let mut dialogs = ErrorDialogs::default();
        dialogs
            .queue
            .push_back(QueuedDialog::overflow(u64::MAX - 1));
        dialogs.request(error("One more"));
        dialogs.request(error("And another"));
        assert_eq!(dialogs.len(), 1);
        assert!(
            dialogs
                .front()
                .unwrap()
                .message
                .starts_with(&format!("At least {} additional errors", u64::MAX))
        );
    }

    #[test]
    fn dismissal_keys_do_not_reach_underlying_gestures_even_on_the_first_frame() {
        for dismiss in [egui::Key::Enter, egui::Key::Escape] {
            let mut world = World::new();
            let surface = world
                .spawn((Window::default(), WorkbenchSurface::default()))
                .id();
            let ctx = egui::Context::default();
            request_error_dialog(&mut world, error("The operation failed."));
            let mut output = ctx.run_ui(input(vec![key(dismiss)]), |ui| {
                assert!(draw(ui.ctx(), &mut world, surface));
                assert!(!ui.input(|input| input.key_pressed(egui::Key::Enter)));
                assert!(!ui.input(|input| input.key_pressed(egui::Key::Escape)));
            });
            output.textures_delta.clear();
            assert!(world.resource::<ErrorDialogs>().is_empty());
            assert!(ctx.memory(|memory| memory.top_modal_layer().is_some()));
        }
    }

    #[test]
    fn simultaneous_dismissal_keys_acknowledge_only_one_error() {
        let mut world = World::new();
        let surface = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let ctx = egui::Context::default();
        request_error_dialog(&mut world, error("First failure"));
        request_error_dialog(&mut world, error("Second failure"));
        let _ = render(
            &ctx,
            &mut world,
            surface,
            vec![key(egui::Key::Enter), key(egui::Key::Escape)],
        );
        assert_eq!(world.resource::<ErrorDialogs>().len(), 1);
        assert_eq!(
            world.resource::<ErrorDialogs>().front(),
            Some(&error("Second failure"))
        );
    }

    #[test]
    fn presentation_keeps_its_owner_and_rehomes_when_that_surface_closes() {
        let mut world = World::new();
        let first = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let second = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let ctx = egui::Context::default();
        request_error_dialog(&mut world, error("The operation failed."));
        let mut output = ctx.run_ui(input(vec![]), |ui| {
            assert!(!draw(ui.ctx(), &mut world, second));
            assert!(draw(ui.ctx(), &mut world, first));
        });
        output.textures_delta.clear();
        assert_eq!(world.resource::<ErrorDialogs>().owner, Some(first));
        world.get_mut::<Window>(first).unwrap().focused = false;
        let _ = render(&ctx, &mut world, second, vec![key(egui::Key::Enter)]);
        assert_eq!(world.resource::<ErrorDialogs>().len(), 1);
        assert_eq!(world.resource::<ErrorDialogs>().owner, Some(first));

        world.entity_mut(first).remove::<WorkbenchSurface>();
        let _ = render(&ctx, &mut world, second, vec![]);
        assert_eq!(world.resource::<ErrorDialogs>().owner, Some(second));
        assert_eq!(world.resource::<ErrorDialogs>().len(), 1);
        world.despawn(second);
        let _ = render(&ctx, &mut world, first, vec![]);
        assert_eq!(world.resource::<ErrorDialogs>().owner, None);
        assert_eq!(world.resource::<ErrorDialogs>().len(), 1);
        world.entity_mut(first).insert(WorkbenchSurface::default());
        let _ = render(&ctx, &mut world, first, vec![]);
        assert_eq!(world.resource::<ErrorDialogs>().owner, Some(first));
    }

    fn painted_text(output: &egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn diagnostics_start_collapsed_and_the_details_control_is_optional() {
        let mut world = World::new();
        let surface = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let ctx = egui::Context::default();
        request_error_dialog(&mut world, error("Refresh Files and try again."));
        let _ = render(&ctx, &mut world, surface, vec![]); // egui area sizing pass.
        let text = painted_text(&render(&ctx, &mut world, surface, vec![]));
        assert!(
            text.iter()
                .any(|text| text == "Refresh Files and try again.")
        );
        assert!(text.iter().any(|text| text == "Technical details"));
        assert!(!text.iter().any(|text| text.contains("diagnostic for")));

        let _ = render(&ctx, &mut world, surface, vec![key(egui::Key::Enter)]);
        request_error_dialog(
            &mut world,
            ErrorDialog {
                title: "Could not open the file".into(),
                message: "Choose an existing file and try again.".into(),
                details: None,
            },
        );
        let _ = render(&ctx, &mut world, surface, vec![]);
        let text = painted_text(&render(&ctx, &mut world, surface, vec![]));
        assert!(
            text.iter()
                .any(|text| text == "Choose an existing file and try again.")
        );
        assert!(!text.iter().any(|text| text == "Technical details"));
    }

    #[test]
    fn surface_registers_the_error_aware_root_instead_of_the_panel_only_renderer() {
        let mut app = App::new();
        app.add_plugins(crate::WorkbenchUiPlugin);
        let surface = app
            .world_mut()
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        app.world_mut().spawn((Camera2d, crate::WorkbenchCamera));
        app.update();
        let roots = app
            .world()
            .get::<bedtime_ui::SurfaceViews>(surface)
            .unwrap();
        assert!(roots.iter().any(|root| {
            app.world()
                .get::<bedtime_ui::UiView>(root)
                .is_some_and(|view| {
                    std::ptr::fn_addr_eq(view.draw, crate::ui::draw_active as bedtime_ui::UiDraw)
                })
        }));
    }
}
