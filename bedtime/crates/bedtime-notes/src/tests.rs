use super::*;
use bedtime_workbench::persistence::{MemoryStore, PersistenceAppExt};
use bedtime_workbench::{BedtimeWorldExt, WorkbenchCorePlugins};

fn notes(changes: impl IntoIterator<Item = NoteChange>) -> Notes {
    let mut notes = Notes::default();
    for change in changes {
        notes.apply(&change).unwrap();
    }
    notes
}

#[test]
fn labels_fall_back_to_the_first_line_then_untitled() {
    let mut note = Note {
        id: 0,
        title: "  ".into(),
        text: "\n  first line \nsecond".into(),
    };
    assert_eq!(note.label(), "first line");
    note.title = "Plans".into();
    assert_eq!(note.label(), "Plans");
    assert_eq!(Note::default().label(), "Untitled");
}

#[test]
fn new_notes_go_on_top_and_ids_never_repeat() {
    let mut notes = Notes::default();
    let (a, change) = notes.new_note("a", "");
    notes.apply(&change).unwrap();
    let (b, change) = notes.new_note("b", "");
    notes.apply(&change).unwrap();
    assert_ne!(a, b);
    assert_eq!(notes.notes[0].id, b);
    // A lost counter still can't hand out an id in use.
    notes.next_id = 0;
    let (c, _) = notes.new_note("c", "");
    assert!(c > a && c > b);
}

#[test]
fn copies_go_below_and_removing_shows_the_next_then_the_one_before() {
    let mut notes = Notes::default();
    let (a, change) = notes.new_note("a", "text");
    notes.apply(&change).unwrap();
    let (copy, change) = notes.copy_note(a).unwrap();
    notes.apply(&change).unwrap();
    assert_eq!(notes.notes[1].id, copy);
    assert_eq!(notes.get(copy).unwrap().title, "a copy");
    assert_eq!(notes.get(copy).unwrap().text, "text");
    // a, copy
    assert_eq!(notes.shown_after_removing(a), Some(copy));
    assert_eq!(notes.shown_after_removing(copy), Some(a));
}

#[test]
fn every_change_is_undone_by_its_inverse() {
    let (id, insert) = Notes::default().new_note("a", "hello world");
    let start = notes([insert]);
    for change in [
        NoteChange::SetTitle {
            id,
            title: "b".into(),
        },
        NoteChange::EditText {
            id,
            range: 0..5,
            text: "goodbye".into(),
        },
        NoteChange::Remove { id },
        start.new_note("other", "").1,
    ] {
        let mut changed = start.clone();
        let inverse = changed.apply(&change).unwrap();
        assert_ne!(changed.notes, start.notes);
        changed.apply(&inverse).unwrap();
        assert_eq!(changed.notes, start.notes, "{change:?}");
    }
    // Nonsense is refused, and changes nothing.
    let mut changed = start.clone();
    assert!(changed.apply(&NoteChange::Remove { id: 99 }).is_err());
    assert!(
        changed
            .apply(&NoteChange::EditText {
                id,
                range: 3..99,
                text: String::new(),
            })
            .is_err()
    );
    assert_eq!(changed, start);
}

#[test]
fn a_text_change_is_only_what_differs() {
    let (id, insert) = Notes::default().new_note("", "héllo wörld");
    let notes = notes([insert]);
    assert_eq!(
        notes.text_change(id, "héllo, wörld"),
        Some(NoteChange::EditText {
            id,
            range: 6..6,
            text: ",".into(),
        })
    );
    // At a character boundary, though é and ö share their first byte.
    let change = notes.text_change(id, "hölo wörld").unwrap();
    let mut changed = notes.clone();
    changed.apply(&change).unwrap();
    assert_eq!(changed.get(id).unwrap().text, "hölo wörld");
    assert_eq!(notes.text_change(id, "héllo wörld"), None);
}

#[test]
fn notes_survive_a_round_trip_through_their_byte_form() {
    let notes = notes([Notes::default().new_note("Plans", "one\ntwo").1]);
    let mut bytes = Vec::new();
    notes.encode(&mut bytes).unwrap();
    assert_eq!(Notes::decode(&bytes).unwrap(), notes);
    // Older or hand-edited files with missing fields still load, and an
    // empty one is no notes.
    let sparse = Notes::decode(b"(notes: [(id: 3)])").unwrap();
    assert_eq!(sparse.notes[0].label(), "Untitled");
    assert_eq!(Notes::decode(b"").unwrap(), Notes::default());
}

/// A session with a Notes panel and its document, kept in `store`.
fn session(store: &MemoryStore) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, NotesPlugin))
        .add_store::<WorkbenchStore>(store.clone());
    app.update();
    let world = app.world_mut();
    let layout = world.bedtime_mut().layouts_mut().create("Test").unwrap();
    let area = world.bedtime().layouts().destination_area(layout).unwrap();
    let panel = world
        .spawn((NotesPanel::default(), bedtime_workbench::InArea(area)))
        .id();
    until(&mut app, |world| {
        document(world, panel).is_some_and(|notes| {
            world
                .bedtime()
                .inspect_document(notes.id())
                .is_ok_and(|info| info.status == DocumentStatus::Ready)
        })
    });
    (app, panel)
}

fn until(app: &mut App, done: impl Fn(&World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app.world()) {
        assert!(Instant::now() < deadline, "Timed out");
        app.update();
    }
}

fn read(world: &World, panel: Entity) -> Notes {
    let notes = document(world, panel).unwrap();
    world.bedtime().read_document(notes).unwrap().clone()
}

fn selected(world: &World, panel: Entity) -> Option<u64> {
    world.get::<NotesPanel>(panel).unwrap().selected
}

#[test]
fn intents_create_select_edit_and_delete_and_are_saved() {
    let store = MemoryStore::default();
    let (mut app, panel) = session(&store);
    let world = app.world_mut();
    apply(world, panel, vec![Intent::New]);
    let first = selected(world, panel).unwrap();
    assert_eq!(
        world.get::<NotesView>(panel).unwrap().focus,
        Some(Field::Title)
    );
    apply(
        world,
        panel,
        vec![
            Intent::Rename(first, "Todo".into()),
            Intent::SetText(first, "buy milk".into()),
            Intent::New,
        ],
    );
    let second = selected(world, panel).unwrap();
    assert_ne!(first, second);
    let notes = read(world, panel);
    assert_eq!(notes.get(first).unwrap().title, "Todo");
    assert_eq!(notes.get(first).unwrap().text, "buy milk");

    // Deleting the note shown shows another.
    apply(world, panel, vec![Intent::Delete(second)]);
    assert_eq!(selected(world, panel), Some(first));
    // Saved as it's made: never unsaved.
    let id = document(world, panel).unwrap().id();
    assert!(!world.bedtime().inspect_document(id).unwrap().dirty);
    until(&mut app, |_| {
        store.text(ENTRY).is_some_and(|text| {
            text.contains("buy milk") && !text.contains(&format!("id: {second}"))
        })
    });
}

#[test]
fn typing_is_one_undo_step_and_a_delete_can_be_undone() {
    let store = MemoryStore::default();
    let (mut app, panel) = session(&store);
    let world = app.world_mut();
    apply(world, panel, vec![Intent::New]);
    let note = selected(world, panel).unwrap();
    for text in ["h", "he", "hel", "hello"] {
        apply(world, panel, vec![Intent::SetText(note, text.into())]);
    }
    assert_eq!(read(world, panel).get(note).unwrap().text, "hello");
    let id = document(world, panel).unwrap().id();
    world.bedtime_mut().undo_document(id).unwrap();
    assert_eq!(read(world, panel).get(note).unwrap().text, "");

    apply(world, panel, vec![Intent::Delete(note)]);
    assert!(read(world, panel).get(note).is_none());
    world.bedtime_mut().undo_document(id).unwrap();
    assert!(read(world, panel).get(note).is_some());
}

#[test]
fn notes_kept_in_the_store_come_back_and_follow_changes_to_it() {
    let store = MemoryStore::default();
    let kept = notes([Notes::default().new_note("Kept", "").1]);
    let mut bytes = Vec::new();
    kept.encode(&mut bytes).unwrap();
    bedtime_workbench::persistence::StoreBackend::write(&store, ENTRY, &bytes).unwrap();
    let (mut app, panel) = session(&store);
    assert_eq!(read(app.world(), panel), kept);

    // Changed elsewhere (by hand, another window).
    let changed = notes([Notes::default().new_note("Changed", "").1]);
    let mut bytes = Vec::new();
    changed.encode(&mut bytes).unwrap();
    bedtime_workbench::persistence::StoreBackend::write(&store, ENTRY, &bytes).unwrap();
    let id = document(app.world(), panel).unwrap().id();
    app.world_mut().bedtime_mut().refresh_document(id).unwrap();
    until(&mut app, |world| read(world, panel) == changed);
}

#[test]
fn the_list_hides_and_shows_again() {
    let (mut app, panel) = session(&MemoryStore::default());
    let world = app.world_mut();
    assert!(!world.get::<NotesPanel>(panel).unwrap().list_hidden);
    apply(world, panel, vec![Intent::HideList(true)]);
    assert!(world.get::<NotesPanel>(panel).unwrap().list_hidden);
    apply(world, panel, vec![Intent::HideList(false)]);
    assert!(!world.get::<NotesPanel>(panel).unwrap().list_hidden);
}
