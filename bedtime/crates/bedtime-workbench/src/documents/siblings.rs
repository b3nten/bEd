//! Siblings: documents of one source held by different models (an SVG open
//! as text and as an image, a file as text and in hex). Each keeps the form
//! that makes its own edits cheap; content passes between them as bytes, in
//! memory, through their models' byte forms (`ByteModel`), never through the
//! file.
//!
//! When one sibling has changed since content last passed between them, and
//! it has been idle a moment or the layout's focus moved, its content is
//! encoded and each other sibling takes it in place of its own, as one undo
//! step ("Change in Another View"). A sibling that can't decode it (half-typed
//! JSON, for a glTF) keeps what it has and shows why. Undo is each
//! document's own: undoing in one view is a change the others then take.
//!
//! If two siblings changed since they were last in step, nothing passes and
//! the user is told: saving one brings the other up to date, through the
//! file. A sibling that opens later takes the unsaved content of one already
//! open. When one saves, the others that hold the same content take that as
//! their saved state too, so they aren't saved again.
use super::*;
use crate::Notification;
use bedtime_document::SourceId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// How long a document is left alone before its siblings take its content.
const SYNC_IDLE: Duration = Duration::from_millis(300);

#[derive(Default)]
pub(super) struct SyncState {
    /// Each document's revision, and when it last changed.
    changed: HashMap<Entity, (u64, Instant)>,
    /// The focused panel last frame.
    focused: Option<Entity>,
    /// Sources already reported as changed in two views.
    warned: HashSet<SourceId>,
}

pub(super) fn sync_siblings(
    world: &mut World,
    mut state: Local<SyncState>,
    mut saves: Local<bevy::ecs::message::MessageCursor<DocumentSaveFinished>>,
) {
    let now = Instant::now();
    let focused = world
        .layouts()
        .active()
        .and_then(|layout| world.layouts().focused_panel(layout));
    let focus_moved = focused != state.focused;
    state.focused = focused;
    let saved: Vec<DocumentId> = saves
        .read(world.resource::<Messages<DocumentSaveFinished>>())
        .filter(|finished| finished.result.is_ok())
        .map(|finished| finished.document)
        .collect();

    // Ready documents with a byte form, by source.
    let mut groups: HashMap<SourceId, Vec<Entity>> = HashMap::new();
    let mut documents = world.query::<(Entity, &Document)>();
    for (entity, document) in documents.iter(world) {
        let info = document.store.info();
        if !document.store.has_codec() || info.status != DocumentStatus::Ready {
            continue;
        }
        let changed = state.changed.entry(entity).or_insert((info.revision, now));
        if changed.0 != info.revision {
            *changed = (info.revision, now);
        }
        groups
            .entry(document.store.source_id())
            .or_default()
            .push(entity);
    }
    state
        .changed
        .retain(|entity, _| world.get_entity(*entity).is_ok());

    for (source, group) in groups {
        if group.len() < 2 {
            state.warned.remove(&source);
            continue;
        }
        join(world, &group);
        let changed: Vec<Entity> = group
            .iter()
            .copied()
            .filter(|&entity| store(world, entity).changed_since_sync())
            .collect();
        match changed.as_slice() {
            [] => {
                state.warned.remove(&source);
            }
            &[from] => {
                let idle = state
                    .changed
                    .get(&from)
                    .is_some_and(|(_, at)| now.duration_since(*at) >= SYNC_IDLE);
                // A save needs its siblings in step first, to share it.
                let saving = saved.contains(&DocumentId(from));
                if idle || focus_moved || saving {
                    pass(world, from, &group);
                }
            }
            _ => {
                if state.warned.insert(source) {
                    let title = store(world, changed[0]).title();
                    world.workbench_mut().notify(Notification::warning(format!(
                        "{title} was changed in two views. Save one to bring the other up to date."
                    )));
                }
            }
        }
        // A save shared: siblings holding the same content have it saved too.
        for &document in saved.iter().filter(|id| group.contains(&id.0)) {
            if store(world, document.0).changed_since_sync() {
                continue;
            }
            let version = store(world, document.0).saved_version();
            for &other in group.iter().filter(|&&other| other != document.0) {
                let mut sibling = world.get_mut::<Document>(other).unwrap();
                if !sibling.store.changed_since_sync() {
                    sibling.store.adopt_save(version.clone());
                }
            }
        }
    }
}

/// Bring siblings that haven't been in step yet (just opened) into it: they
/// take the unsaved content of one already open, if any has some.
fn join(world: &mut World, group: &[Entity]) {
    let new: Vec<Entity> = group
        .iter()
        .copied()
        .filter(|&entity| !store(world, entity).in_step())
        .collect();
    if new.is_empty() {
        return;
    }
    let from = group
        .iter()
        .copied()
        .filter(|entity| !new.contains(entity))
        .chain(new.iter().copied())
        .find(|&entity| store(world, entity).info().dirty);
    if let Some(from) = from {
        pass(world, from, group);
    }
    for entity in new {
        world
            .get_mut::<Document>(entity)
            .unwrap()
            .store
            .mark_synced();
    }
}

/// Give `from`'s content to the rest of `group`.
fn pass(world: &mut World, from: Entity, group: &[Entity]) {
    let bytes = match world.get::<Document>(from).unwrap().store.encode() {
        Some(Ok(bytes)) => bytes,
        Some(Err(error)) => {
            let title = world.get::<Document>(from).unwrap().store.title();
            world.workbench_mut().notify(Notification::warning(format!(
                "{title} couldn't be passed to its other views: {error}"
            )));
            return;
        }
        None => return,
    };
    for &other in group.iter().filter(|&&other| other != from) {
        let mut sibling = world.get_mut::<Document>(other).unwrap();
        // One that can't take it says why itself (its document's error).
        let _ = sibling.store.receive(&bytes);
    }
    world.get_mut::<Document>(from).unwrap().store.mark_synced();
}

/// A ready document's store.
fn store(world: &World, entity: Entity) -> &dyn Store {
    world.get::<Document>(entity).unwrap().store.as_ref()
}

#[cfg(all(test, feature = "files"))]
mod tests {
    use crate::documents::tests::{type_text, update_until};
    use crate::tests::layout;
    use crate::*;
    use bedtime_document::{ByteModel, TextBuffer};

    /// A file's bytes, editable as a whole: a second model for a text file.
    #[derive(Clone, PartialEq, Default)]
    struct Raw(Vec<u8>);

    impl DocumentModel for Raw {
        type Change = Vec<u8>;
        type OpenState = ();
        fn apply(&mut self, change: &Vec<u8>) -> Result<Vec<u8>, String> {
            Ok(std::mem::replace(&mut self.0, change.clone()))
        }
    }

    impl ByteModel for Raw {
        fn decode(bytes: &[u8]) -> Result<Self, String> {
            Ok(Self(bytes.to_vec()))
        }
        fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
            out.write_all(&self.0)
        }
    }

    impl bedtime_filesystem::FileModel for Raw {}

    #[bedtime_ui::panel]
    #[derive(Default)]
    struct TextView;
    impl Panel for TextView {
        const LABEL: &'static str = "Text";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    #[bedtime_ui::panel]
    #[derive(Default)]
    struct RawView;
    impl Panel for RawView {
        const LABEL: &'static str = "Raw";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    fn document(world: &World, panel: Entity) -> DocumentId {
        world.get::<DocumentReference>(panel).unwrap().document()
    }

    fn info(world: &World, id: DocumentId) -> DocumentInfo {
        world.workbench().inspect_document(id).unwrap()
    }

    /// A text file open as text and as raw bytes, both loaded.
    fn siblings(
        name: &str,
    ) -> (
        App,
        std::path::PathBuf,
        DocumentHandle<TextBuffer>,
        DocumentHandle<Raw>,
    ) {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .register_viewer::<TextBuffer, TextView>("Text")
            .formats([TEXT])
            .register_viewer::<Raw, RawView>("Raw")
            .formats([DATA]);
        layout(app.world_mut());
        let directory =
            std::env::temp_dir().join(format!("bedtime-siblings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("notes.txt");
        std::fs::write(&path, "one").unwrap();
        let world = app.world_mut();
        let text = world.workbench_mut().open_path(&path, default()).unwrap();
        let raw = world
            .workbench_mut()
            .open_path(
                &path,
                PathOptions {
                    viewer: Some(ViewerId::of::<RawView>()),
                    ..default()
                },
            )
            .unwrap();
        let (text, raw) = (document(world, text), document(world, raw));
        assert_ne!(text, raw, "a document per model");
        update_until(&mut app, |world| {
            [text, raw]
                .iter()
                .all(|&id| info(world, id).status == DocumentStatus::Ready)
        });
        let workbench = app.world().workbench();
        let text = workbench.cast_document::<TextBuffer>(text).unwrap();
        let raw = workbench.cast_document::<Raw>(raw).unwrap();
        (app, directory, text, raw)
    }

    fn raw(world: &World, handle: DocumentHandle<Raw>) -> Vec<u8> {
        world.workbench().read_document(handle).unwrap().0.clone()
    }

    fn text(world: &World, handle: DocumentHandle<TextBuffer>) -> String {
        world.workbench().read_document(handle).unwrap().to_string()
    }

    fn set_raw(world: &mut World, handle: DocumentHandle<Raw>, bytes: &[u8]) {
        let revision = info(world, handle.id()).revision;
        world
            .workbench_mut()
            .edit_document(handle, revision, bytes.to_vec(), EditOptions::new("Set"))
            .unwrap();
    }

    #[test]
    fn edits_pass_between_siblings_both_ways() {
        let (mut app, directory, text_doc, raw_doc) = siblings("both-ways");
        // Opening the same file as text again is the same document.
        let world = app.world_mut();
        let path = directory.join("notes.txt");
        let again = world.workbench_mut().open_path(&path, default()).unwrap();
        assert_eq!(document(world, again), text_doc.id());

        type_text(world, text_doc, 3, " two");
        update_until(&mut app, |world| raw(world, raw_doc) == b"one two");
        // The raw document has the unsaved change too, as one undo step.
        let world = app.world_mut();
        assert!(info(world, raw_doc.id()).dirty);
        assert_eq!(
            info(world, raw_doc.id()).undo_label.as_deref(),
            Some("Change in Another View")
        );

        set_raw(world, raw_doc, b"three");
        update_until(&mut app, |world| text(world, text_doc) == "three");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_sibling_opened_later_takes_the_unsaved_content() {
        let (mut app, directory, text_doc, raw_doc) = siblings("later");
        // Close the raw view: its document goes once nothing references it.
        let world = app.world_mut();
        let raw_panel = world
            .layouts()
            .find::<RawView>(world.layouts().active().unwrap())
            .unwrap();
        world.layouts_mut().close_panel(raw_panel).unwrap();
        update_until(&mut app, |world| world.get_entity(raw_doc.id().0).is_err());
        let world = app.world_mut();
        type_text(world, text_doc, 3, " more");
        let path = directory.join("notes.txt");
        let panel = world
            .workbench_mut()
            .open_path(
                &path,
                PathOptions {
                    viewer: Some(ViewerId::of::<RawView>()),
                    ..default()
                },
            )
            .unwrap();
        let reopened = world
            .workbench()
            .cast_document::<Raw>(document(world, panel))
            .unwrap();
        update_until(&mut app, |world| {
            world
                .workbench()
                .read_document(reopened)
                .is_ok_and(|raw| raw.0 == b"one more")
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_sibling_that_cant_take_the_content_keeps_its_own() {
        let (mut app, directory, text_doc, raw_doc) = siblings("bad");
        let world = app.world_mut();
        set_raw(world, raw_doc, &[0, 159]);
        update_until(&mut app, |world| info(world, text_doc.id()).error.is_some());
        let world = app.world();
        assert_eq!(text(world, text_doc), "one");
        let error = info(world, text_doc.id()).error.unwrap();
        assert!(
            error.starts_with("Another view's changes can't be shown here"),
            "{error}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn changes_in_both_are_reported_not_passed() {
        let (mut app, directory, text_doc, raw_doc) = siblings("both");
        let world = app.world_mut();
        type_text(world, text_doc, 0, "a ");
        set_raw(world, raw_doc, b"b");
        update_until(&mut app, |world| {
            world
                .resource::<Notifications>()
                .latest()
                .is_some_and(|n| n.text.contains("was changed in two views"))
        });
        let world = app.world();
        assert_eq!(text(world, text_doc), "a one");
        assert_eq!(raw(world, raw_doc), b"b");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_save_is_shared_by_siblings_holding_the_same_content() {
        let (mut app, directory, text_doc, raw_doc) = siblings("save");
        let world = app.world_mut();
        type_text(world, text_doc, 3, "!");
        update_until(&mut app, |world| raw(world, raw_doc) == b"one!");
        app.world_mut()
            .workbench_mut()
            .save_document(text_doc.id())
            .unwrap();
        update_until(&mut app, |world| {
            !info(world, text_doc.id()).dirty && !info(world, raw_doc.id()).dirty
        });
        assert_eq!(std::fs::read(directory.join("notes.txt")).unwrap(), b"one!");
        // The raw document saves on from there without a conflict.
        let world = app.world_mut();
        set_raw(world, raw_doc, b"done");
        world.workbench_mut().save_document(raw_doc.id()).unwrap();
        update_until(&mut app, |world| !info(world, raw_doc.id()).saving);
        let world = app.world();
        assert!(!info(world, raw_doc.id()).conflict);
        assert_eq!(std::fs::read(directory.join("notes.txt")).unwrap(), b"done");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
