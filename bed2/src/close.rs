//! Application-owned quit decisions and session worker shutdown.
use bedtime_ui2::egui;
use bedtime_workbench2::*;
use bevy::{ecs::message::MessageCursor, prelude::*};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseSystems {
    Update,
}

pub struct ClosePlugin;

impl Plugin for ClosePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppClose>()
            .init_resource::<PanelCloseInbox>()
            .init_resource::<CloseInbox>()
            .add_systems(
                Update,
                (request_close, close_requested_panels, advance_close)
                    .chain()
                    .in_set(CloseSystems::Update)
                    // Drain file-operation outcomes/source rebinding before
                    // deciding documents are clean or flushing layout recipes.
                    .after(crate::files::FilesSystems::Update),
            );
    }
}

#[derive(Resource, Default)]
struct PanelCloseInbox(MessageCursor<PanelCloseRequested>);

/// Closing a view retains its canonical document, including dirty content and
/// admitted writes. Only viewer-local uncommitted composition would be lost.
fn close_requested_panels(world: &mut World) {
    if !world.contains_resource::<Messages<PanelCloseRequested>>() {
        return;
    }

    world.resource_scope(|world, mut inbox: Mut<PanelCloseInbox>| {
        let requests: Vec<_> = inbox.0.read(world.resource::<Messages<PanelCloseRequested>>()).cloned().collect();
        for request in requests {
            if !matches!(world.resource::<AppClose>().phase, ClosePhase::Idle) { continue; }
            let result = (|| {
                let panels = Workbench::new(world).close_targets(&request)?;
                if panels.iter().any(|&panel| world.get::<DocumentDraft>(panel).is_some()) {
                    return Err("Finish or cancel text composition before closing this view. Your document and edits remain open.".into());
                }
                Workbench::new(world).commit_close(&request)
            })();
            if let Err(error) = result {
                request_error_dialog(world, ErrorDialog {
                    title: "Close panel".into(),
                    message: "This view couldn't be closed. Nothing was discarded.".into(),
                    details: Some(error),
                });
            }
        }
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CloseSnapshot {
    revisions: Vec<(DocumentId, u64)>,
}

#[derive(Clone, Default)]
enum ClosePhase {
    #[default]
    Idle,
    Prompt,
    Saving {
        snapshot: CloseSnapshot,
        awaited: Vec<DocumentId>,
    },
    Flushing {
        mode: SessionCloseMode,
    },
    FlushError {
        mode: SessionCloseMode,
    },
    ShuttingDown,
    ExitSent,
}

#[derive(Resource, Default)]
struct AppClose {
    phase: ClosePhase,
    error: Option<String>,
}

/// Native menus and asynchronous application actions share the same modal gate
/// as the egui surface. Completed folder choices wait here until cancellation.
pub fn accepts_user_actions(world: &World) -> bool {
    world
        .get_resource::<AppClose>()
        .is_none_or(|close| matches!(close.phase, ClosePhase::Idle))
}

pub fn shutting_down(world: &World) -> bool {
    world
        .get_resource::<AppClose>()
        .is_some_and(|close| matches!(close.phase, ClosePhase::ShuttingDown | ClosePhase::ExitSent))
}

#[derive(Clone, Copy)]
enum CloseChoice {
    SaveAll,
    Discard,
    Cancel,
}

#[derive(Resource, Default)]
struct CloseInbox {
    requests: MessageCursor<CloseRequested>,
    pending: bool,
}

fn request_close(world: &mut World) {
    let requested = world.resource_scope(|world, mut inbox: Mut<CloseInbox>| {
        let pending = std::mem::take(&mut inbox.pending);
        let requested = inbox
            .requests
            .read(world.resource::<Messages<CloseRequested>>())
            .next()
            .is_some()
            || pending;
        // Native editing actions admitted after drawing and canonical command
        // follow-ups take effect on the next pass. Observe them before deciding
        // whether documents are clean, rather than abandoning an admitted edit.
        let commands_pending = world
            .get_resource::<ContextCommandRequests>()
            .is_some_and(|queue| !queue.is_empty())
            || world
                .query::<&ContextCommandRequests>()
                .iter(world)
                .any(|queue| !queue.is_empty());
        if requested && (crate::menus::has_pending_edits(world) || commands_pending) {
            inbox.pending = true;
            false
        } else {
            requested
        }
    });
    if !requested || !matches!(world.resource::<AppClose>().phase, ClosePhase::Idle) {
        return;
    }
    let result = (|| {
        let snapshot = close_snapshot(world)?;
        let mut saving = false;
        for (id, _) in snapshot.revisions {
            let info = inspect_document(world, id).map_err(|e| e.to_string())?;
            if info.dirty || info.conflict || info.requires_source_refresh || info.has_drafts {
                let mut close = world.resource_mut::<AppClose>();
                close.phase = ClosePhase::Prompt;
                close.error = None;
                return Ok(());
            }
            saving |= info.saving;
        }
        // A clean application needs no decision. Already admitted document saves
        // still drain through the ordinary Save All gate; no write is cancelled.
        if saving {
            begin_save_all(world)
        } else {
            begin_shutdown(world, SessionCloseMode::Saved)
        }
    })();
    if let Err(error) = result {
        close_failed(world, error);
    }
}

fn close_snapshot(world: &mut World) -> Result<CloseSnapshot, String> {
    let mut revisions = Vec::new();
    for id in document_ids(world) {
        revisions.push((
            id,
            inspect_document(world, id)
                .map_err(|e| e.to_string())?
                .revision,
        ));
    }
    revisions.sort_by_key(|entry| entry.0);
    Ok(CloseSnapshot { revisions })
}

fn close_failed(world: &mut World, error: String) {
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::Prompt;
    close.error = Some(error);
}

fn begin_save_all(world: &mut World) -> Result<(), String> {
    let snapshot = close_snapshot(world)?;
    let mut infos = Vec::new();
    // Validate the complete application decision before admitting any writes.
    // A failed write admission later may leave earlier saves running, never a
    // partly closed application or cancelled backend writes.
    for (id, _) in &snapshot.revisions {
        let info = inspect_document(world, *id).map_err(|e| e.to_string())?;
        if info.has_drafts {
            return Err(format!(
                "Commit or cancel drafts for {:?} first",
                info.title
            ));
        }
        if info.conflict || info.requires_source_refresh {
            return Err(format!(
                "Cancel quit and observe/resolve the source conflict for {:?} first",
                info.title
            ));
        }
        if info.dirty && !info.has_save_binding {
            return Err(format!(
                "Document {:?} has no save binding; cancel quit or explicitly discard",
                info.title
            ));
        }
        infos.push(info);
    }
    let mut awaited = Vec::new();
    for info in infos {
        if info.saving {
            awaited.push(info.id);
        } else if info.dirty {
            save_document(world, info.id)
                .map_err(|e| format!("Cannot save {:?}: {e}", info.title))?;
            awaited.push(info.id);
        }
    }
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::Saving { snapshot, awaited };
    close.error = None;
    Ok(())
}

fn begin_shutdown(world: &mut World, mode: SessionCloseMode) -> Result<(), String> {
    preflight_session_close(world, mode)?;
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::Flushing { mode };
    close.error = None;
    Ok(())
}

fn apply_close_choice(world: &mut World, choice: CloseChoice) {
    if matches!(
        world.resource::<AppClose>().phase,
        ClosePhase::ShuttingDown | ClosePhase::ExitSent
    ) {
        return;
    }
    let result = match choice {
        CloseChoice::SaveAll => begin_save_all(world),
        CloseChoice::Discard => begin_shutdown(world, SessionCloseMode::Discard),
        CloseChoice::Cancel => {
            // Admitted saves continue normally. Cancellation only abandons the
            // quit intention; live documents, viewers and workers are retained.
            let mut close = world.resource_mut::<AppClose>();
            close.phase = ClosePhase::Idle;
            close.error = None;
            Ok(())
        }
    };
    if let Err(error) = result {
        close_failed(world, error);
    }
}

fn advance_close(world: &mut World) {
    let phase = world.resource::<AppClose>().phase.clone();
    match phase {
        ClosePhase::Saving { snapshot, awaited } => {
            let result = (|| {
                if close_snapshot(world)? != snapshot {
                    return Err("Documents or layouts changed while Save All was pending. Nothing was closed. Cancel quit to review, or choose Save All again.".into());
                }
                let infos: Vec<_> = snapshot
                    .revisions
                    .iter()
                    .map(|(id, _)| inspect_document(world, *id).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?;
                if infos.iter().any(|info| info.saving) {
                    return Ok(());
                }
                for info in &infos {
                    if awaited.contains(&info.id)
                        && let Some(error) = &info.last_save_error
                    {
                        return Err(format!(
                            "Save failed for {:?}: {error}. Nothing was closed; cancel quit to fix it or explicitly discard.",
                            info.title
                        ));
                    }
                }
                // The completed snapshots, not merely save admission, must pass
                // the same core gate that protects normal session shutdown.
                begin_shutdown(world, SessionCloseMode::Saved)
            })();
            if let Err(error) = result {
                close_failed(world, error);
            }
        }
        ClosePhase::Flushing { mode } => {
            let result = (|| {
                // Poll every store even while another is pending. Session flush
                // captures final mutations and retains final session writes.
                let files = crate::files::flush_preferences(world);
                let settings = crate::settings::flush(world);
                let session = crate::session::flush(world);
                if !files? || !settings? || !session? {
                    return Ok(());
                }
                // Revalidate after draining writes/outcomes, before any cancellation.
                preflight_session_close(world, mode)?;
                shutdown_work(world)?;
                world.resource_mut::<AppClose>().phase = ClosePhase::ShuttingDown;
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                // Persistence failures are not document-discard decisions. Keep
                // workers/live work intact, with explicit retry or cancellation.
                let mut close = world.resource_mut::<AppClose>();
                close.phase = ClosePhase::FlushError { mode };
                close.error = Some(error);
            }
        }
        ClosePhase::ShuttingDown => {
            if work_finished(world) {
                world.write_message(AppExit::Success);
                world.resource_mut::<AppClose>().phase = ClosePhase::ExitSent;
            }
        }
        _ => {}
    }
}

/// Drawn before the workbench panels from the app-owned titlebar callback. The
/// foreground modal blocks the entire one-window UI, including during final
/// worker shutdown; normal updates continue so processes and saves can finish.
pub fn draw_close_dialog(ui: &mut egui::Ui, world: &mut World) {
    let Some(close) = world.get_resource::<AppClose>() else {
        return;
    };
    if matches!(close.phase, ClosePhase::Idle) {
        return;
    }
    let phase = close.phase.clone();
    let error = close.error.clone();
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::new("bed.application-close")).show(ui.ctx(), |ui| {
        ui.set_max_width(480.0);
        ui.heading("Quit bEd 2?");
        if matches!(phase, ClosePhase::Prompt | ClosePhase::Saving { .. }) {
            ui.label("Modified documents include documents with no open viewers. Layouts save automatically.");
        }
        if let Some(error) = &error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        match &phase {
            ClosePhase::Prompt => {
                if let Ok(snapshot) = close_snapshot(world) {
                    let decisions = snapshot.revisions.iter().filter(|(id, _)|
                        inspect_document(world, *id).is_ok_and(|info|
                            info.dirty || info.conflict || info.requires_source_refresh || info.has_drafts)).count();
                    ui.label(format!("{decisions} modified, conflicted or composing document(s) need a decision."));
                }
                ui.horizontal(|ui| {
                    if ui.button("Save All and quit").clicked() { choice = Some(CloseChoice::SaveAll); }
                    if ui.button("Discard and quit").clicked() { choice = Some(CloseChoice::Discard); }
                    if ui.button("Cancel").clicked() { choice = Some(CloseChoice::Cancel); }
                });
            }
            ClosePhase::Saving { .. } => {
                ui.spinner();
                ui.label("Waiting for admitted saves… Edits are paused. Save failures or changed documents return to this decision, never an automatic save loop.");
                if ui.button("Cancel quit (saves keep running)").clicked() { choice = Some(CloseChoice::Cancel); }
            }
            ClosePhase::Flushing { .. } => {
                ui.spinner();
                ui.label("Finishing file operations and saving application settings…");
                if ui.button("Cancel quit").clicked() { choice = Some(CloseChoice::Cancel); }
            }
            ClosePhase::FlushError { mode } => {
                ui.label("Application state couldn't be saved or the close preflight changed. Nothing was discarded. Cancel quit to repair settings/documents, or retry after fixing the reported problem.");
                if ui.button("Retry quit").clicked() {
                    crate::session::retry_failed(world);
                    crate::storage::retry_failed(world);
                    let mut close = world.resource_mut::<AppClose>();
                    close.phase = ClosePhase::Flushing { mode: *mode };
                    close.error = None;
                }
                if ui.button("Cancel quit").clicked() { choice = Some(CloseChoice::Cancel); }
            }
            ClosePhase::ShuttingDown | ClosePhase::ExitSent => {
                ui.spinner();
                ui.label("Stopping tasks and processes…");
            }
            ClosePhase::Idle => {}
        }
    });
    if !matches!(phase, ClosePhase::ShuttingDown | ClosePhase::ExitSent) && response.should_close()
    {
        choice = Some(CloseChoice::Cancel);
    }
    if let Some(choice) = choice {
        apply_close_choice(world, choice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn host() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, ClosePlugin))
            .add_message::<AppExit>();
        app
    }

    fn workspace(app: &mut App, _name: &str) -> (Entity, Entity) {
        let layout = crate::test_layout(app.world_mut());
        (layout, layout)
    }

    fn viewer(world: &mut World, layout: Entity) -> Entity {
        let area = Workbench::for_entity(world, layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        world
            .spawn((crate::settings::SettingsPanel::default(), InArea(area)))
            .id()
    }

    fn text_document(world: &mut World) -> DocumentId {
        let handle = open_document(world, String::new(), DocumentOptions::loaded("Test")).unwrap();
        bind_document_save(world, handle, DocumentSaveBinding::new(None, |_| Ok(None))).unwrap();
        handle.id()
    }

    fn change_text(world: &mut World, id: DocumentId, text: &str) {
        let handle = cast_document::<String>(world, id).unwrap();
        let revision = inspect_document(world, id).unwrap().revision;
        let len = read_document(world, handle).unwrap().len();
        edit_document(
            world,
            handle,
            revision,
            TextChange {
                range: 0..len,
                replacement: text.into(),
            },
            EditOptions::new("Test edit"),
        )
        .unwrap();
    }

    fn eventually(app: &mut App, condition: impl Fn(&mut World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(app.world_mut()) {
            assert!(Instant::now() < deadline, "Document operation timed out");
            app.update();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn quit_observes_admitted_command_followup_edits_before_deciding_documents_are_clean() {
        struct QueueEdit;
        impl ContextCommand for QueueEdit {
            const KEY: &'static str = "test.queue-edit";
            const LABEL: &'static str = "Queue edit";
        }
        struct CompleteEdit;
        impl ContextCommand for CompleteEdit {
            const KEY: &'static str = "test.complete-edit";
            const LABEL: &'static str = "Complete edit";
        }
        let mut app = host();
        let (_owner, _) = workspace(&mut app, "Default");
        let id = text_document(app.world_mut());
        register_context_command::<QueueEdit>(app.world_mut()).unwrap();
        register_context_command::<CompleteEdit>(app.world_mut()).unwrap();
        install_global_handler::<QueueEdit, _>(
            app.world_mut(),
            |_: In<ContextCommandInvocation>, world: &mut World| {
                world
                    .resource_mut::<ContextCommandRequests>()
                    .invoke::<CompleteEdit>();
                Ok(())
            },
        )
        .unwrap();
        install_global_handler::<CompleteEdit, _>(
            app.world_mut(),
            move |_: In<ContextCommandInvocation>, world: &mut World| {
                change_text(world, id, "Admitted before quit");
                Ok(())
            },
        )
        .unwrap();
        app.world_mut()
            .resource_mut::<ContextCommandRequests>()
            .invoke::<QueueEdit>();
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(accepts_user_actions(app.world()));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        app.update();
        assert!(inspect_document(app.world(), id).unwrap().dirty);
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn accepted_folder_results_wait_behind_quit_and_resume_on_cancellation() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let mut app = host();
        app.add_plugins(crate::files::FilesPlugin);
        workspace(&mut app, "Default");
        app.insert_resource(crate::files::ProjectRoot(root.clone()));
        app.insert_resource(crate::files::ProjectFolder);
        let result = crate::opening::open_folder(app.world_mut(), root.clone()).unwrap();
        app.world_mut().resource_mut::<AppClose>().phase = ClosePhase::Prompt;
        for _ in 0..3 {
            app.update();
        }
        assert!(result.lock().unwrap().try_recv().is_err());
        assert!(crate::opening::open_folder(app.world_mut(), root.clone()).is_err());
        apply_close_choice(app.world_mut(), CloseChoice::Cancel);
        eventually(&mut app, |_| result.lock().unwrap().try_recv().is_ok());
        assert_eq!(app.world().resource::<crate::files::ProjectRoot>().0, root);
        shutdown_work(app.world_mut()).unwrap();
        eventually(&mut app, |world| work_finished(world));
    }

    #[test]
    fn clean_startup_quits_without_a_document_prompt() {
        for clean_document in [false, true] {
            let mut app = host();
            let (_owner, _) = workspace(&mut app, "Memory workspace");
            if clean_document {
                text_document(app.world_mut());
            }
            app.world_mut()
                .write_message(CloseRequested { window: None });
            app.update();
            assert!(!matches!(
                app.world().resource::<AppClose>().phase,
                ClosePhase::Prompt
            ));
            eventually(&mut app, |world| {
                matches!(world.resource::<AppClose>().phase, ClosePhase::ExitSent)
            });
            assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
        }
    }

    #[test]
    fn dirty_document_requests_a_decision_even_without_an_open_viewer() {
        let mut app = host();
        let (_owner, _) = workspace(&mut app, "Project");
        let id = text_document(app.world_mut());
        change_text(app.world_mut(), id, "Modified");
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        assert!(inspect_document(app.world(), id).unwrap().dirty);
    }

    #[test]
    fn application_approves_join_and_dock_intentions_using_core_close_targets() {
        for dock in [false, true] {
            let mut app = host();
            let (owner, layout) = workspace(&mut app, "Layout policy");
            let mut workbench = Workbench::for_entity(app.world_mut(), owner).unwrap();
            let source = workbench.destination_area(layout).unwrap();
            let target = workbench
                .split_area(source, SplitAxis::Horizontal, 5000, [100, 100])
                .unwrap();
            let retained = workbench
                .world_mut()
                .spawn((crate::settings::SettingsPanel, InArea(source)))
                .id();
            let retired = workbench
                .world_mut()
                .spawn((crate::settings::SettingsPanel, InArea(target)))
                .id();
            let edit = if dock {
                LayoutEdit::DockArea {
                    source,
                    target,
                    placement: AreaPlacement::Replace,
                    minimum: [100, 100],
                }
            } else {
                LayoutEdit::JoinAreas {
                    source,
                    target,
                    minimum: [100, 100],
                }
            };
            workbench
                .request_panel_close(workbench.stamp(), edit)
                .unwrap();
            assert!(workbench.world().get_entity(retired).is_ok());
            app.update();
            assert!(app.world().get_entity(retained).is_ok());
            assert!(
                app.world().get_entity(retired).is_err(),
                "dock={dock}, error={:?}",
                app.world()
                    .get_resource::<ErrorDialogs>()
                    .and_then(|dialogs| dialogs.front())
            );
            assert!(app.world().get_entity(source).is_ok());
        }
    }

    #[test]
    fn close_cancel_preserves_live_documents_and_never_emits_exit() {
        let mut app = host();
        let (_owner, layout) = workspace(&mut app, "Project");
        let id = text_document(app.world_mut());
        change_text(app.world_mut(), id, "Unsaved edit");
        let viewer = viewer(app.world_mut(), layout);
        let handle = cast_document::<String>(app.world(), id).unwrap();
        bind_document_view(app.world_mut(), viewer, handle).unwrap();
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        apply_close_choice(app.world_mut(), CloseChoice::Cancel);
        app.update();
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Idle
        ));
        assert!(app.world().get::<DocumentView>(viewer).is_some());
        assert!(inspect_document(app.world(), id).is_ok());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn save_all_failure_leaves_the_app_editable_and_does_not_exit() {
        let mut app = host();
        let (_owner, _) = workspace(&mut app, "Project");
        let id = text_document(app.world_mut());
        change_text(app.world_mut(), id, "Local edit");
        let handle = cast_document::<String>(app.world(), id).unwrap();
        bind_document_save(
            app.world_mut(),
            handle,
            DocumentSaveBinding::new(None, |_| {
                Err(DocumentSaveError::Failed("Test save failure".into()))
            }),
        )
        .unwrap();
        apply_close_choice(app.world_mut(), CloseChoice::SaveAll);
        eventually(&mut app, |world| {
            matches!(world.resource::<AppClose>().phase, ClosePhase::Prompt)
        });
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("Save failed")
        );
        assert!(inspect_document(app.world(), id).unwrap().dirty);
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        apply_close_choice(app.world_mut(), CloseChoice::Cancel);
        change_text(app.world_mut(), id, "Still editable after failed quit");
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Idle
        ));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn discard_preflights_inactive_workspace_drafts_before_shutting_down_any_work() {
        let mut app = host();
        let (_first, _) = workspace(&mut app, "Active");
        let (_inactive, layout) = workspace(&mut app, "Inactive");
        let id = text_document(app.world_mut());
        let viewer = viewer(app.world_mut(), layout);
        let handle = cast_document::<String>(app.world(), id).unwrap();
        bind_document_view(app.world_mut(), viewer, handle).unwrap();
        app.world_mut().entity_mut(viewer).insert(DocumentDraft);
        apply_close_choice(app.world_mut(), CloseChoice::Discard);
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("draft")
        );
        assert!(app.world().get::<DocumentView>(viewer).is_some());
        assert!(inspect_document(app.world(), id).is_ok());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        let _task = app
            .world_mut()
            .get_resource_mut::<ManagedWork>()
            .unwrap()
            .spawn_task(None, "Still accepts work after rejected close", |_| 1_u8)
            .unwrap();
    }

    #[test]
    fn successful_quit_waits_for_workspace_work_before_emitting_exit() {
        let mut app = host();
        let (_owner, _) = workspace(&mut app, "Project");
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let _task = app
            .world_mut()
            .get_resource_mut::<ManagedWork>()
            .unwrap()
            .spawn_task(None, "Pending cleanup", move |_| {
                let _ = wait.recv_timeout(Duration::from_secs(3));
            })
            .unwrap();
        apply_close_choice(app.world_mut(), CloseChoice::Discard);
        advance_close(app.world_mut());
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::ShuttingDown
        ));
        advance_close(app.world_mut());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        release.send(()).unwrap();
        eventually(&mut app, |world| {
            matches!(world.resource::<AppClose>().phase, ClosePhase::ExitSent)
        });
        assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn save_all_does_not_loop_when_a_document_changes_while_saving() {
        let mut app = host();
        let (_owner, _) = workspace(&mut app, "Project");
        let id = text_document(app.world_mut());
        change_text(app.world_mut(), id, "Save this version");
        apply_close_choice(app.world_mut(), CloseChoice::SaveAll);
        change_text(app.world_mut(), id, "A newer version arrived");
        advance_close(app.world_mut());
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("changed")
        );
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        eventually(&mut app, |world| {
            !inspect_document(world, id).unwrap().saving
        });
        assert!(inspect_document(app.world(), id).unwrap().dirty);
        let handle = cast_document::<String>(app.world(), id).unwrap();
        assert_eq!(
            read_document(app.world(), handle).unwrap(),
            "A newer version arrived"
        );
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }
}
