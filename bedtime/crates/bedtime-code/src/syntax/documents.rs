//! Analysis for the documents editors (and `SyntaxViewer`s) show: one
//! session and worker per document, owned by its entity, fed each new
//! revision.
use super::*;
use crate::editor::EditorPanel;
use bedtime_document::TextBuffer;
use bedtime_workbench::{DocumentReference, DocumentStatus, ManagedWork, WorkbenchWorldExt};
use bevy::prelude::*;
use std::collections::HashSet;

/// A panel, other than an editor, that reads the `DocumentSyntax` of the text
/// document it shows (a Markdown preview colouring its code).
#[derive(Component, Default)]
pub struct SyntaxViewer;

/// A text document's syntax analysis, on the document's entity while an editor
/// or `SyntaxViewer` shows it. The language comes from the document's title
/// (its file name).
#[derive(Component)]
pub struct DocumentSyntax {
    pub session: AnalysisSession,
}

impl DocumentSyntax {
    /// The analysis of `revision`, once it's done.
    pub fn at(&self, revision: u64) -> Option<&Arc<Observation>> {
        self.session
            .observation()
            .filter(|observation| observation.key.revision == revision)
    }
}

/// Start, feed and stop analysis, in `Update`.
pub(crate) fn update_syntax(world: &mut World) {
    let shown: HashSet<Entity> = world
        .query_filtered::<&DocumentReference, Or<(With<EditorPanel>, With<SyntaxViewer>)>>()
        .iter(world)
        .map(|reference| reference.document().entity())
        .collect();
    // Dropping a session stops its worker.
    let unshown: Vec<Entity> = world
        .query_filtered::<Entity, With<DocumentSyntax>>()
        .iter(world)
        .filter(|document| !shown.contains(document))
        .collect();
    for document in unshown {
        world.entity_mut(document).remove::<DocumentSyntax>();
    }
    for document in shown {
        let id = bedtime_workbench::DocumentId(document);
        let workbench = world.workbench();
        let (Ok(handle), Ok(info)) = (
            workbench.cast_document::<TextBuffer>(id),
            workbench.inspect_document(id),
        ) else {
            continue;
        };
        let Some(language) = language_name(&info.title) else {
            continue;
        };
        if info.status != DocumentStatus::Ready {
            continue;
        }
        if world.get::<DocumentSyntax>(document).is_none() {
            let (session, worker) = AnalysisSession::with_worker();
            let task = world.resource_mut::<ManagedWork>().spawn_task(
                Some(document),
                format!("Analysing {}", info.title),
                move |cancel| worker.run(|| cancel.is_cancelled()),
            );
            if task.is_err() {
                continue;
            }
            world
                .entity_mut(document)
                .insert(DocumentSyntax { session });
        }
        let workbench = world.workbench();
        let text = workbench.read_document(handle).unwrap().clone();
        // Reparse incrementally from the last analysis only while every change
        // since is known; otherwise from scratch.
        let base = world
            .get::<DocumentSyntax>(document)
            .unwrap()
            .session
            .key()
            .and_then(|key| {
                workbench
                    .document_changes_since(handle, key.revision)
                    .ok()
                    .flatten()
                    .filter(|commits| commits.iter().all(|commit| commit.changes.is_some()))
                    .map(|_| key.revision)
            });
        let mut syntax = world.get_mut::<DocumentSyntax>(document).unwrap();
        syntax
            .session
            .request(Arc::new(text), info.revision, language, base);
        syntax.session.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CodePlugin;
    use bedtime_document::{BufferEdit, MemorySource};
    use bedtime_workbench::{EditOptions, LayoutsWorldExt, OpenOptions, WorkbenchCorePlugins};

    fn until(app: &mut App, done: impl Fn(&World) -> bool) {
        for _ in 0..1000 {
            app.update();
            if done(app.world()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("never got there");
    }

    #[test]
    fn documents_editors_show_are_analysed_as_they_change() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, CodePlugin));
        let world = app.world_mut();
        let layout = world.layouts_mut().create("Test").unwrap();
        world.layouts_mut().activate(layout).unwrap();
        let open = |world: &mut World, name: &str| {
            world
                .workbench_mut()
                .open_document(
                    MemorySource::new(name, TextBuffer::from("fn main() {}\n")),
                    OpenOptions::default(),
                )
                .unwrap()
        };
        let panel = open(world, "main.rs");
        // Plain text has no language, so no analysis.
        let notes = open(world, "notes.txt");
        let document = world.get::<DocumentReference>(panel).unwrap().document();
        let entity = document.entity();
        let analysed = move |world: &World, revision: u64| {
            world
                .get::<DocumentSyntax>(entity)
                .and_then(|syntax| syntax.at(revision))
                .is_some_and(|observation| !observation.highlights.is_empty())
        };
        until(&mut app, |world| analysed(world, 0));
        let notes_document = app
            .world()
            .get::<DocumentReference>(notes)
            .unwrap()
            .document();
        assert!(
            app.world()
                .get::<DocumentSyntax>(notes_document.entity())
                .is_none()
        );

        let world = app.world_mut();
        let handle = world
            .workbench()
            .cast_document::<TextBuffer>(document)
            .unwrap();
        world
            .workbench_mut()
            .edit_document(
                handle,
                0,
                BufferEdit::new(0..0, "// hi\n"),
                EditOptions::new("Typing"),
            )
            .unwrap();
        until(&mut app, |world| analysed(world, 1));
        let world = app.world();
        let syntax = world.get::<DocumentSyntax>(entity).unwrap().at(1).unwrap();
        assert!(syntax.highlights.iter().any(|h| &*h.capture == "comment"));

        // No editor shows it: analysis stops.
        app.world_mut().layouts_mut().close_panel(panel).unwrap();
        until(&mut app, |world| {
            world.get::<DocumentSyntax>(entity).is_none()
        });
    }
}
