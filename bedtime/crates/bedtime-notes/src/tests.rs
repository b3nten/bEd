use super::*;

fn world_with_panel() -> (World, Entity) {
    let mut world = World::new();
    world.init_resource::<Notes>();
    let panel = world.spawn(NotesPanel::default()).id();
    (world, panel)
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
    let a = notes.add("a", "");
    let b = notes.add("b", "");
    assert_ne!(a, b);
    assert_eq!(notes.notes[0].id, b);
    // A lost counter still can't hand out an id in use.
    notes.next_id = 0;
    let c = notes.add("c", "");
    assert!(c > a && c > b);
}

#[test]
fn duplicating_puts_the_copy_below() {
    let mut notes = Notes::default();
    let a = notes.add("a", "text");
    let copy = notes.duplicate(a).unwrap();
    assert_eq!(notes.notes[1].id, copy);
    assert_eq!(notes.get(copy).unwrap().title, "a copy");
    assert_eq!(notes.get(copy).unwrap().text, "text");
}

#[test]
fn removing_shows_the_next_note_then_the_one_before() {
    let mut notes = Notes::default();
    let c = notes.add("c", "");
    let b = notes.add("b", "");
    let a = notes.add("a", "");
    // a, b, c
    assert_eq!(notes.remove(b), Some(c));
    assert_eq!(notes.remove(c), Some(a));
    assert_eq!(notes.remove(a), None);
    assert_eq!(notes.remove(a), None);
}

#[test]
fn intents_create_select_edit_and_delete() {
    let (mut world, panel) = world_with_panel();
    apply(&mut world, panel, vec![Intent::New]);
    let first = world.get::<NotesPanel>(panel).unwrap().selected.unwrap();
    assert_eq!(
        world.get::<NotesView>(panel).unwrap().focus,
        Some(Field::Title)
    );
    apply(
        &mut world,
        panel,
        vec![
            Intent::Rename(first, "Todo".into()),
            Intent::SetText(first, "buy milk".into()),
            Intent::New,
        ],
    );
    let second = world.get::<NotesPanel>(panel).unwrap().selected.unwrap();
    assert_ne!(first, second);
    let notes = world.resource::<Notes>();
    assert_eq!(notes.get(first).unwrap().title, "Todo");
    assert_eq!(notes.get(first).unwrap().text, "buy milk");

    // Deleting the note shown shows another.
    apply(&mut world, panel, vec![Intent::Delete(second)]);
    assert_eq!(
        world.get::<NotesPanel>(panel).unwrap().selected,
        Some(first)
    );
    // Deleting one not shown leaves the selection alone.
    let other = world.resource_mut::<Notes>().add("other", "");
    apply(&mut world, panel, vec![Intent::Delete(other)]);
    assert_eq!(
        world.get::<NotesPanel>(panel).unwrap().selected,
        Some(first)
    );
}

#[test]
fn notes_survive_a_round_trip_through_the_store() {
    let mut notes = Notes::default();
    notes.add("Plans", "one\ntwo");
    let text = ron::to_string(&notes).unwrap();
    let back: Notes = ron::from_str(&text).unwrap();
    assert_eq!(back, notes);
    // Older or hand-edited files with missing fields still load.
    let sparse: Notes = ron::from_str("(notes: [(id: 3)])").unwrap();
    assert_eq!(sparse.notes[0].label(), "Untitled");
}
