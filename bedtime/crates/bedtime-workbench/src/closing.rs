//! Closing panels and ending the session without losing unsaved work.
//!
//! A referenced document lives while something holds a `DocumentReference` to it.
//! Closing panels that would release an unsaved document either saves it first
//! (autosave on) or asks the user: Save, Discard or Cancel. Exiting (quitting, or
//! an application's own exit such as switching project) asks the same about
//! every unsaved document, then finishes background work and runs the exit's
//! last step: `AppExit` for a quit.
//! A failed save never closes anything; the prompt shows the error instead.
//!
//! The flow is driven headlessly through `close_prompt` and `resolve_close`;
//! the `ui` feature draws the prompt.
use crate::*;
use crate::{Workbench, WorkbenchRef, WorkbenchWorldExt};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// An unsaved document the user is being asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsavedDocument {
    pub id: DocumentId,
    pub title: String,
    /// Why the last attempt to save it failed, if it did.
    pub error: Option<String>,
}

/// What the user is being asked, while a close waits on a decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosePrompt {
    /// The exit being asked about ("Quit", "Switch workspace"), or `None` when
    /// closing panels.
    pub exit: Option<String>,
    pub documents: Vec<UnsavedDocument>,
}

/// An exit's last step, once unsaved work is settled and background work has
/// finished.
type ExitStep = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// The close in progress, if any. There is at most one at a time.
#[derive(Resource, Default)]
pub(crate) struct PendingClose(Option<Close>);

struct Close {
    target: Target,
    documents: Vec<DocumentId>,
    stage: Stage,
}

enum Target {
    Panels {
        layout: Entity,
        edit: LayoutEdit,
    },
    Exit {
        name: String,
        /// Taken when it runs.
        then: Option<ExitStep>,
    },
}

enum Stage {
    /// Saves are running. Errors from saves that couldn't even start are kept here.
    Saving(Vec<(DocumentId, String)>),
    Asking(Vec<(DocumentId, String)>),
    /// Exit only: waiting for in-flight saves, then for managed work to finish.
    ShuttingDown,
}

/// Released documents marked this way are dropped even though they're unsaved.
#[derive(Component)]
struct Discarded;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PendingClose>()
        .init_resource::<DocumentAutosave>()
        .add_systems(
            Update,
            (crate::documents::autosave, request_quit, advance).chain(),
        )
        .add_systems(Last, release_unviewed.before(SessionSystems::Close));
}

impl WorkbenchRef {
    /// The question waiting on the user, if any.
    pub fn close_prompt(&self) -> Option<ClosePrompt> {
        let world = &self.world;
        let close = world.resource::<PendingClose>().0.as_ref()?;
        let Stage::Asking(errors) = &close.stage else {
            return None;
        };
        let documents = close
            .documents
            .iter()
            .filter_map(|&id| {
                Some(UnsavedDocument {
                    id,
                    title: world.workbench().inspect_document(id).ok()?.title,
                    error: errors
                        .iter()
                        .find(|(e, _)| *e == id)
                        .map(|(_, e)| e.clone()),
                })
            })
            .collect();
        Some(ClosePrompt {
            exit: match &close.target {
                Target::Exit { name, .. } => Some(name.clone()),
                Target::Panels { .. } => None,
            },
            documents,
        })
    }
}

impl Workbench {
    /// Answer the current prompt.
    pub fn resolve_close(&mut self, choice: CloseChoice) -> Result<(), String> {
        let world = &mut self.world;
        let mut pending = world.resource_mut::<PendingClose>();
        if !matches!(
            pending.0,
            Some(Close {
                stage: Stage::Asking(_),
                ..
            })
        ) {
            return Err("No close is waiting for a decision".into());
        }
        let mut close = pending.0.take().unwrap();
        match choice {
            CloseChoice::Cancel => {}
            CloseChoice::Save => {
                close.stage = Stage::Saving(start_saves(world, &close.documents));
                world.resource_mut::<PendingClose>().0 = Some(close);
            }
            CloseChoice::Discard => {
                let documents = close.documents.clone();
                if let Some(close) = finish(world, close) {
                    world.resource_mut::<PendingClose>().0 = Some(close);
                }
                // Mark only after the close went through, so a refused close can't
                // leave still-viewed documents silently discardable later.
                for id in documents {
                    if world.get_entity(id.0).is_ok() {
                        world.entity_mut(id.0).insert(Discarded);
                    }
                }
            }
        }
        Ok(())
    }
}

impl Workbench {
    /// Close panels on the user's behalf (closing a tab or area, or merging areas).
    /// Closes immediately unless that would release an unsaved document.
    pub fn request_panel_close(&mut self, layout: Entity, edit: LayoutEdit) -> Result<(), String> {
        if self.world.resource::<PendingClose>().0.is_some() {
            return Err("Another close is waiting for a decision".into());
        }
        let panels = self.close_targets(layout, &edit)?;
        crate::layout::check_panel_drafts(&self.world, panels.iter().copied())?;
        let documents = released_unsaved(&self.world, &panels);
        if documents.is_empty() {
            return self.edit(layout, &edit);
        }
        begin(&mut self.world, Target::Panels { layout, edit }, documents);
        Ok(())
    }
}

/// Documents that would lose unsaved work because only `panels` (or entities
/// inside them, which close with them) reference them.
fn released_unsaved(world: &World, panels: &[Entity]) -> Vec<DocumentId> {
    let mut closing = panels.to_vec();
    let mut index = 0;
    while let Some(&entity) = closing.get(index) {
        if let Some(children) = world.get::<Children>(entity) {
            closing.extend(children.iter());
        }
        index += 1;
    }
    let mut documents: Vec<_> = closing
        .iter()
        .filter_map(|&holder| world.get::<DocumentReference>(holder))
        .map(DocumentReference::document)
        .filter(|id| {
            world
                .get::<DocumentReferences>(id.0)
                .is_none_or(|references| references.holders().iter().all(|h| closing.contains(h)))
        })
        .filter(|&id| needs_decision(world, id))
        .collect();
    documents.sort();
    documents.dedup();
    documents
}

fn needs_decision(world: &World, id: DocumentId) -> bool {
    world
        .workbench()
        .inspect_document(id)
        .is_ok_and(|info| info.dirty || info.conflict)
}

fn begin(world: &mut World, target: Target, documents: Vec<DocumentId>) {
    let stage = if world.resource::<DocumentAutosave>().enabled() {
        Stage::Saving(start_saves(world, &documents))
    } else {
        Stage::Asking(Vec::new())
    };
    world.resource_mut::<PendingClose>().0 = Some(Close {
        target,
        documents,
        stage,
    });
}

fn start_saves(world: &mut World, documents: &[DocumentId]) -> Vec<(DocumentId, String)> {
    let mut errors = Vec::new();
    for &id in documents {
        if world
            .workbench()
            .inspect_document(id)
            .is_ok_and(|info| info.saving)
        {
            continue;
        }
        if let Err(error) = world.workbench_mut().save_document(id) {
            errors.push((id, error.to_string()));
        }
    }
    errors
}

/// The decision is made: close the panels, or start shutting down.
/// Returns the close if it still has work to do.
fn finish(world: &mut World, close: Close) -> Option<Close> {
    match close.target {
        Target::Panels { layout, edit } => {
            if let Err(error) = world.workbench_mut().edit(layout, &edit) {
                report_error(
                    world,
                    "Close panel",
                    "The layout changed before the close finished, so nothing was closed.",
                    Some(error),
                );
            }
            None
        }
        Target::Exit { .. } => Some(Close {
            stage: Stage::ShuttingDown,
            ..close
        }),
    }
}

fn request_quit(
    world: &mut World,
    mut cursor: Local<bevy::ecs::message::MessageCursor<CloseRequested>>,
) {
    let requested = cursor
        .read(world.resource::<Messages<CloseRequested>>())
        .count()
        > 0;
    if !requested || exiting(world) {
        return;
    }
    if let Err(error) = world.workbench_mut().request_exit("Quit", |world| {
        world.write_message(AppExit::Success);
    }) {
        report_error(world, "Quit", &error, None);
    }
}

fn exiting(world: &World) -> bool {
    world
        .resource::<PendingClose>()
        .0
        .as_ref()
        .is_some_and(|close| matches!(close.target, Target::Exit { .. }))
}

impl Workbench {
    /// End the session, as quitting does: unsaved documents are saved or asked
    /// about (`name` titles the prompt, "Quit" or "Switch workspace"), background
    /// work finishes, then `then` runs. Quitting's `then` sends `AppExit`. If the
    /// user cancels, nothing happens and the session goes on. Refused while
    /// another exit is under way, or while an editor has uncommitted input.
    pub fn request_exit(
        &mut self,
        name: impl Into<String>,
        then: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let name = name.into();
        let world = &mut self.world;
        if exiting(world) {
            return Err("Already exiting".into());
        }
        let ids = world.workbench().document_ids();
        if let Some(info) = ids
            .iter()
            .filter_map(|&id| world.workbench().inspect_document(id).ok())
            .find(|info| info.has_drafts)
        {
            return Err(format!("Finish typing in {:?} first.", info.title));
        }
        // Exiting supersedes a pending panel close.
        world.resource_mut::<PendingClose>().0 = None;
        let documents: Vec<_> = ids
            .into_iter()
            .filter(|&id| needs_decision(world, id))
            .collect();
        let target = Target::Exit {
            name,
            then: Some(Box::new(then)),
        };
        if documents.is_empty() {
            world.resource_mut::<PendingClose>().0 = Some(Close {
                target,
                documents,
                stage: Stage::ShuttingDown,
            });
        } else {
            begin(world, target, documents);
        }
        Ok(())
    }
}

fn advance(world: &mut World) {
    let Some(mut close) = world.resource_mut::<PendingClose>().0.take() else {
        return;
    };
    let saving = |world: &World, ids: &[DocumentId]| {
        ids.iter().any(|&id| {
            world
                .workbench()
                .inspect_document(id)
                .is_ok_and(|info| info.saving)
        })
    };
    match &mut close.stage {
        Stage::Asking(_) => {}
        Stage::Saving(errors) => {
            if !saving(world, &close.documents) {
                let mut errors = std::mem::take(errors);
                close.documents.retain(|&id| needs_decision(world, id));
                for &id in &close.documents {
                    let failed = world
                        .workbench()
                        .inspect_document(id)
                        .ok()
                        .and_then(|info| info.error);
                    if let Some(error) = failed
                        && !errors.iter().any(|(e, _)| *e == id)
                    {
                        errors.push((id, error));
                    }
                }
                errors.retain(|(id, _)| close.documents.contains(id));
                if close.documents.is_empty() {
                    let Some(next) = finish(world, close) else {
                        return;
                    };
                    close = next;
                } else {
                    close.stage = Stage::Asking(errors);
                }
            }
        }
        Stage::ShuttingDown => {
            let ids = world.workbench().document_ids();
            if !world.resource::<ManagedWork>().is_closing() && !saving(world, &ids) {
                world.resource_mut::<ManagedWork>().shutdown();
            }
            if world.resource::<ManagedWork>().is_closing()
                && world.resource::<ManagedWork>().is_finished()
                && let Target::Exit { then, .. } = &mut close.target
                && let Some(then) = then.take()
            {
                then(world);
            }
        }
    }
    world.resource_mut::<PendingClose>().0 = Some(close);
}

/// Drop documents whose last reference went away this frame (or earlier). Running
/// at the end of the frame lets a viewer be replaced without reloading its document.
/// Unsaved documents are only dropped once saved or discarded; otherwise they
/// stay until quit, which asks about them.
fn release_unviewed(world: &mut World) {
    let candidates: Vec<_> = world
        .query_filtered::<(Entity, Option<&DocumentReferences>), (With<Document>, With<crate::documents::Referenced>)>()
        .iter(world)
        .filter(|(_, references)| references.is_none_or(|r| r.holders().is_empty()))
        .map(|(entity, _)| DocumentId(entity))
        .collect();
    for id in candidates {
        let Ok(info) = world.workbench().inspect_document(id) else {
            continue;
        };
        let discarded = world.get::<Discarded>(id.0).is_some();
        if !info.saving && (discarded || !(info.dirty || info.conflict)) {
            world.despawn(id.0);
        }
    }
}

/// Explain a failure with an error dialog when there is UI, otherwise log it.
pub(crate) fn report_error(world: &mut World, title: &str, message: &str, details: Option<String>) {
    #[cfg(feature = "ui")]
    world.workbench_mut().request_error_dialog(ErrorDialog {
        title: title.into(),
        message: message.into(),
        details,
    });
    #[cfg(not(feature = "ui"))]
    {
        let _ = world;
        error!("{title}: {message} {}", details.unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::tests::{TestSource, app, open, panel, type_text};
    use std::time::{Duration, Instant};

    /// A viewed document with unsaved edits, whose saves succeed or fail as asked.
    fn unsaved(app: &mut App, saves: bool) -> (DocumentHandle<TextBuffer>, Entity) {
        let source = TestSource::new("base");
        if !saves {
            source.fail_saves(Some("disk full"));
        }
        let document = open(app, source);
        let world = app.world_mut();
        let viewer = panel(world);
        world
            .entity_mut(viewer)
            .insert(DocumentReference::new(document.id()));
        type_text(world, document, 4, " edited");
        (document, viewer)
    }

    fn clean(app: &mut App, name: &str) -> DocumentHandle<TextBuffer> {
        open(app, MemorySource::new(name, TextBuffer::default()))
    }

    fn close(app: &mut App, panel: Entity) {
        let workbench = app.world_mut().workbench_mut();
        let layout = workbench.active_layout().unwrap();
        workbench
            .request_panel_close(layout, LayoutEdit::ClosePanels(vec![panel]))
            .unwrap();
        app.update();
    }

    fn update_until(app: &mut App, done: impl Fn(&mut App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(app) {
            assert!(Instant::now() < deadline, "Timed out");
            app.update();
            std::thread::yield_now();
        }
    }

    fn exists(app: &App, entity: Entity) -> bool {
        app.world().get_entity(entity).is_ok()
    }

    #[test]
    fn closing_one_of_several_views_never_asks() {
        let mut app = app();
        let (document, first) = unsaved(&mut app, true);
        let second = panel(app.world_mut());
        app.world_mut()
            .entity_mut(second)
            .insert(DocumentReference::new(document.id()));
        close(&mut app, first);
        assert!(!exists(&app, first));
        assert!(app.world().workbench().close_prompt().is_none());
        assert!(exists(&app, document.id().entity()));
    }

    #[test]
    fn closing_the_last_view_of_an_unsaved_document_asks_and_cancel_keeps_it() {
        let mut app = app();
        let (document, viewer) = unsaved(&mut app, true);
        close(&mut app, viewer);
        let prompt = app.world().workbench().close_prompt().unwrap();
        assert_eq!(prompt.exit, None);
        assert_eq!(prompt.documents[0].title, "notes.txt");
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Cancel)
            .unwrap();
        app.update();
        assert!(exists(&app, viewer));
        assert!(
            app.world()
                .workbench()
                .inspect_document(document.id())
                .unwrap()
                .dirty
        );
    }

    #[test]
    fn discard_closes_the_panel_and_releases_the_document() {
        let mut app = app();
        let (document, viewer) = unsaved(&mut app, true);
        close(&mut app, viewer);
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Discard)
            .unwrap();
        app.update();
        assert!(!exists(&app, viewer));
        assert!(!exists(&app, document.id().entity()));
    }

    #[test]
    fn save_closes_once_the_save_succeeds() {
        let mut app = app();
        let (document, viewer) = unsaved(&mut app, true);
        close(&mut app, viewer);
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Save)
            .unwrap();
        update_until(&mut app, |app| !exists(app, viewer));
        app.update();
        assert!(!exists(&app, document.id().entity()));
    }

    #[test]
    fn autosave_saves_instead_of_asking() {
        let mut app = app();
        app.insert_resource(DocumentAutosave(DocumentAutosavePolicy::OnFocusChange));
        let (document, viewer) = unsaved(&mut app, true);
        close(&mut app, viewer);
        assert!(app.world().workbench().close_prompt().is_none());
        update_until(&mut app, |app| !exists(app, viewer));
        app.update();
        assert!(!exists(&app, document.id().entity()));
    }

    #[test]
    fn a_failed_save_keeps_the_panel_and_asks_with_the_error() {
        let mut app = app();
        app.insert_resource(DocumentAutosave(DocumentAutosavePolicy::OnFocusChange));
        let (_, viewer) = unsaved(&mut app, false);
        close(&mut app, viewer);
        update_until(&mut app, |app| {
            app.world().workbench().close_prompt().is_some()
        });
        let prompt = app.world().workbench().close_prompt().unwrap();
        assert!(
            prompt.documents[0]
                .error
                .as_ref()
                .unwrap()
                .contains("disk full")
        );
        assert!(exists(&app, viewer));
    }

    #[test]
    fn clean_documents_are_released_with_their_last_reference_but_unreferenced_ones_are_kept() {
        let mut app = app();
        let viewed = clean(&mut app, "a");
        let unviewed = clean(&mut app, "b");
        let world = app.world_mut();
        let viewer = panel(world);
        world
            .entity_mut(viewer)
            .insert(DocumentReference::new(viewed.id()));
        close(&mut app, viewer);
        assert!(!exists(&app, viewed.id().entity()));
        assert!(exists(&app, unviewed.id().entity()));
    }

    #[test]
    fn replacing_a_view_in_the_same_frame_keeps_the_document() {
        let mut app = app();
        let document = clean(&mut app, "a");
        let world = app.world_mut();
        let old = panel(world);
        world
            .entity_mut(old)
            .insert(DocumentReference::new(document.id()));
        world.despawn(old);
        let new = panel(world);
        world
            .entity_mut(new)
            .insert(DocumentReference::new(document.id()));
        app.update();
        assert!(exists(&app, document.id().entity()));
    }

    #[test]
    fn any_entity_can_hold_a_document_open() {
        let mut app = app();
        let document = clean(&mut app, "a");
        let world = app.world_mut();
        let holder = world.spawn(DocumentReference::new(document.id())).id();
        app.update();
        assert!(exists(&app, document.id().entity()));
        app.world_mut().despawn(holder);
        app.update();
        assert!(!exists(&app, document.id().entity()));
    }

    #[test]
    fn references_inside_a_closing_panel_close_with_it() {
        let mut app = app();
        let (document, viewer) = unsaved(&mut app, true);
        // e.g. one side of a diff: a child of a panel referencing another document.
        let world = app.world_mut();
        world.entity_mut(viewer).remove::<DocumentReference>();
        let host = panel(world);
        world.spawn((DocumentReference::new(document.id()), ChildOf(host)));
        close(&mut app, host);
        assert_eq!(
            app.world().workbench().close_prompt().unwrap().documents[0].id,
            document.id()
        );
    }

    #[test]
    fn quitting_asks_about_unsaved_documents_then_exits() {
        let mut app = app();
        unsaved(&mut app, true);
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert_eq!(
            app.world()
                .workbench()
                .close_prompt()
                .unwrap()
                .exit
                .as_deref(),
            Some("Quit")
        );
        assert!(app.should_exit().is_none());
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Discard)
            .unwrap();
        update_until(&mut app, |app| app.should_exit().is_some());
    }

    #[test]
    fn an_application_exit_asks_like_quit_then_runs_its_last_step() {
        #[derive(Resource)]
        struct Switched;
        let mut app = app();
        unsaved(&mut app, true);
        let switch = |world: &mut World| {
            world
                .workbench_mut()
                .request_exit("Switch workspace", |world| world.insert_resource(Switched))
        };
        switch(app.world_mut()).unwrap();
        app.update();
        assert_eq!(
            app.world()
                .workbench()
                .close_prompt()
                .unwrap()
                .exit
                .as_deref(),
            Some("Switch workspace")
        );
        assert!(switch(app.world_mut()).is_err(), "one exit at a time");
        // Cancelling leaves the session as it was, and a later exit can go ahead.
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Cancel)
            .unwrap();
        app.update();
        assert!(app.world().workbench().close_prompt().is_none());
        assert!(!app.world().resource::<ManagedWork>().is_closing());
        switch(app.world_mut()).unwrap();
        app.update();
        app.world_mut()
            .workbench_mut()
            .resolve_close(CloseChoice::Save)
            .unwrap();
        update_until(&mut app, |app| app.world().contains_resource::<Switched>());
        assert!(
            app.should_exit().is_none(),
            "the last step replaces AppExit"
        );
    }

    #[test]
    fn quitting_with_nothing_unsaved_exits_without_asking() {
        let mut app = app();
        app.world_mut()
            .write_message(CloseRequested { window: None });
        update_until(&mut app, |app| app.should_exit().is_some());
    }
}
