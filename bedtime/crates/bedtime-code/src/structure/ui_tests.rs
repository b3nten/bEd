//! Structure's caret following, keyboard, memory and hover; the breadcrumbs;
//! Go to Symbol. Panels don't draw without a window, so an editor's
//! navigation is settled as its next draw would, and keys go straight to
//! Structure's keyboard handling in a bare egui context.
use super::*;
use crate::CodePlugin;
use bedtime_document::MemorySource;
use bedtime_workbench::{ActionRequests, OpenOptions, Target, WorkbenchCorePlugins};

const CODE: &str = "struct Cat {\n    age: u32,\n}\nimpl Cat {\n    fn meow(&self) -> bool {\n        true\n    }\n}\nfn dog(name: &str) {}\n";

struct Host {
    app: App,
    editor: Entity,
    structure: Entity,
}

fn host() -> Host {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, CodePlugin));
    let world = app.world_mut();
    let layout = world.layouts_mut().create("Test").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let source = MemorySource::new("cats.rs", TextBuffer::from(CODE));
    let editor = world
        .workbench_mut()
        .open_document(
            source,
            OpenOptions {
                target: Target::Existing,
                ..default()
            },
        )
        .unwrap();
    let area = world.layouts().destination_area(layout).unwrap();
    let structure = world
        .spawn((StructurePanel::default(), bedtime_workbench::InArea(area)))
        .id();
    world.flush();
    world.layouts_mut().focus_panel(editor).unwrap();
    let mut host = Host {
        app,
        editor,
        structure,
    };
    for _ in 0..1000 {
        host.app.update();
        if current_outline(host.app.world(), editor).is_some() {
            return host;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never analysed");
}

fn entry(world: &World, editor: Entity, label: &str) -> (ObservationKey, u64) {
    let outline = current_outline(world, editor).unwrap();
    let node = outline.outline.iter().find(|n| n.label == label).unwrap();
    (outline.key, node.id)
}

fn head(world: &World, editor: Entity) -> usize {
    world
        .get::<EditorPanel>(editor)
        .unwrap()
        .view
        .selection()
        .head()
}

/// The editor's next draw, as far as its selection goes.
fn settle(world: &mut World, editor: Entity) {
    world
        .get_mut::<EditorPanel>(editor)
        .unwrap()
        .view
        .settle_navigation();
}

/// Show `label` in the editor, as browsing Structure does.
fn preview(world: &mut World, editor: Entity, label: &str) {
    let (key, item) = entry(world, editor, label);
    assert!(reveal_entry_as(world, editor, key, item, Reveal::Preview));
    settle(world, editor);
}

fn refreshed(world: &mut World, structure: Entity) -> StructurePanel {
    let mut panel = world
        .entity_mut(structure)
        .take::<StructurePanel>()
        .unwrap();
    panel.refresh(world);
    panel
}

#[test]
fn structure_selects_the_entry_the_caret_moves_into_and_opens_its_parents() {
    let mut host = host();
    let (editor, structure) = (host.editor, host.structure);
    let world = host.app.world_mut();
    let (_, meow) = entry(world, editor, "meow");
    let impl_id = current_outline(world, editor)
        .unwrap()
        .outline
        .iter()
        .find(|n| n.kind == "impl")
        .unwrap()
        .id;
    let mut panel = refreshed(world, structure);
    panel.collapsed.insert(impl_id);
    preview(world, editor, "meow");
    panel.refresh(world);
    assert_eq!(panel.selected, Some(meow));
    assert_eq!(panel.scroll_to, Some(meow));
    assert!(!panel.collapsed.contains(&impl_id));
    // Collapsing it again sticks while the caret stays in `meow`.
    panel.collapsed.insert(impl_id);
    panel.refresh(world);
    assert!(panel.collapsed.contains(&impl_id));
    // Not while it's told not to follow.
    panel.ignore_caret = true;
    preview(world, editor, "dog");
    panel.refresh(world);
    assert_eq!(panel.selected, Some(meow));
}

#[test]
fn a_pinned_structure_keeps_its_editor() {
    let mut host = host();
    let (editor, structure) = (host.editor, host.structure);
    let world = host.app.world_mut();
    let mut panel = refreshed(world, structure);
    panel.pinned = Some(editor);
    world.insert_resource(StructureSource(None));
    panel.refresh(world);
    assert_eq!(panel.shown.as_ref().map(|(e, _)| *e), Some(editor));
    // A closed editor unpins it.
    world.layouts_mut().close_panel(editor).unwrap();
    host.app.update();
    panel.refresh(host.app.world());
    assert!(panel.pinned.is_none() && panel.shown.is_none());
}

#[test]
fn breadcrumbs_name_the_entries_around_the_caret() {
    let mut host = host();
    let editor = host.editor;
    let world = host.app.world_mut();
    preview(world, editor, "meow");
    breadcrumbs::update(world, editor);
    let trail = world.get::<breadcrumbs::Trail>(editor).unwrap();
    assert_eq!(
        trail.labels(),
        vec!["impl Cat".to_owned(), "meow".to_owned()]
    );
    preview(world, editor, "dog");
    breadcrumbs::update(world, editor);
    let trail = world.get::<breadcrumbs::Trail>(editor).unwrap();
    assert_eq!(trail.labels(), vec!["dog".to_owned()]);
}

#[test]
fn go_to_symbol_lists_matches_best_first_and_can_go_back() {
    let mut host = host();
    let editor = host.editor;
    preview(host.app.world_mut(), editor, "age");
    let start = head(host.app.world(), editor);
    host.app
        .world_mut()
        .resource_mut::<ActionRequests>()
        .invoke::<GoToSymbol>();
    host.app.update();
    let world = host.app.world_mut();
    let before = picker::opened(world).expect("the picker opened");
    assert_eq!(before.1.head(), start);
    let all = picker::listed(world).unwrap();
    assert_eq!(all.len(), 5, "{all:?}");
    picker::type_query(world, "mw");
    assert_eq!(picker::listed(world).unwrap(), vec!["meow".to_owned()]);
    picker::type_query(world, "a");
    // A name starting with it is the best match.
    assert_eq!(picker::listed(world).unwrap()[0], "age");
    // Browsing moved the caret; going back puts it where it was.
    preview(world, editor, "dog");
    assert_ne!(head(world, editor), start);
    picker::go_back(world, editor, before);
    settle(world, editor);
    assert_eq!(head(world, editor), start);
}

/// Send `events` to Structure's keyboard handling; then do what it asked.
fn keys(
    world: &mut World,
    panel: &mut StructurePanel,
    list: bool,
    events: Vec<egui::Event>,
) -> Vec<view::Intent> {
    let ctx = egui::Context::default();
    let (editor, observation) = panel.shown.clone().unwrap();
    let mut asked = Vec::new();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            let arranged = view::arrange(
                &observation.outline,
                panel.hidden,
                &panel.filter,
                panel.sort_by_name,
            );
            let mut intents = view::Intents {
                editor: Some((editor, observation.key)),
                list: Vec::new(),
            };
            view::keyboard(
                ui,
                10,
                panel,
                &observation,
                &arranged,
                list,
                egui::Id::new("filter"),
                &mut intents,
            );
            asked = std::mem::take(&mut intents.list);
        },
    );
    output.textures_delta.clear();
    let summary = asked
        .iter()
        .map(|intent| match intent {
            view::Intent::Reveal(item, how) => view::Intent::Reveal(*item, *how),
            view::Intent::FocusEditor => view::Intent::FocusEditor,
            _ => unreachable!(),
        })
        .collect();
    view::apply(
        world,
        panel,
        view::Intents {
            editor: Some((editor, observation.key)),
            list: asked,
        },
    );
    settle(world, editor);
    summary
}

fn key(key: egui::Key) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]
}

#[test]
fn the_keyboard_moves_through_the_outline_showing_each_entry() {
    let mut host = host();
    let (editor, structure) = (host.editor, host.structure);
    let world = host.app.world_mut();
    world.layouts_mut().focus_panel(structure).unwrap();
    let mut panel = refreshed(world, structure);
    panel.ignore_caret = true;
    let (_, cat) = entry(world, editor, "Cat");
    let (_, age) = entry(world, editor, "age");
    let (_, dog) = entry(world, editor, "dog");
    keys(world, &mut panel, true, key(egui::Key::Home));
    assert_eq!(panel.selected, Some(cat));
    let asked = keys(world, &mut panel, true, key(egui::Key::ArrowDown));
    assert_eq!(asked, vec![view::Intent::Reveal(age, Reveal::Preview)]);
    assert_eq!(panel.selected, Some(age));
    // Shown in the editor, its name selected, while Structure keeps focus.
    let at = head(world, editor);
    assert_eq!(&CODE[at - 3..at], "age");
    let layout = world.layouts().active().unwrap();
    assert_eq!(world.layouts().focused_panel(layout), Some(structure));
    // Left goes to the parent, then closes it; Right opens it again.
    keys(world, &mut panel, true, key(egui::Key::ArrowLeft));
    assert_eq!(panel.selected, Some(cat));
    keys(world, &mut panel, true, key(egui::Key::ArrowLeft));
    assert!(panel.collapsed.contains(&cat));
    keys(world, &mut panel, true, key(egui::Key::ArrowDown));
    assert_ne!(
        panel.selected,
        Some(age),
        "a closed entry's children are skipped"
    );
    keys(world, &mut panel, true, key(egui::Key::Home));
    keys(world, &mut panel, true, key(egui::Key::ArrowRight));
    assert!(!panel.collapsed.contains(&cat));
    // Typing starts a filter, in the filter field.
    keys(
        world,
        &mut panel,
        true,
        vec![egui::Event::Text("dg".into())],
    );
    assert_eq!(panel.filter, "dg");
    assert!(panel.focus_filter);
    // From the field: Down picks the match, Enter goes to the editor.
    keys(world, &mut panel, false, key(egui::Key::ArrowDown));
    assert_eq!(panel.selected, Some(dog));
    keys(world, &mut panel, false, key(egui::Key::Enter));
    assert_eq!(world.layouts().focused_panel(layout), Some(editor));
    let at = head(world, editor);
    assert_eq!(&CODE[at - 3..at], "dog");
    // Escape in the field clears the filter.
    keys(world, &mut panel, false, key(egui::Key::Escape));
    assert!(panel.filter.is_empty());
}

#[test]
fn a_hovered_entry_is_shaded_in_the_editor_for_a_pass() {
    let mut host = host();
    let editor = host.editor;
    let world = host.app.world_mut();
    let (key, _) = entry(world, editor, "dog");
    peek(world, editor, key, 3..9, 7);
    assert_eq!(
        world.get::<EditorPanel>(editor).unwrap().view.peek,
        Some((key.revision, 3..9, 7))
    );
    // Not for an older analysis.
    let stale = ObservationKey {
        revision: key.revision + 1,
        ..key
    };
    peek(world, editor, stale, 0..1, 8);
    assert_eq!(
        world.get::<EditorPanel>(editor).unwrap().view.peek,
        Some((key.revision, 3..9, 7))
    );
}

#[test]
fn an_outline_shown_again_is_shown_as_it_was_left() {
    let mut panel = StructurePanel::default();
    panel.collapsed.insert(7);
    panel.selected = Some(3);
    panel.switch(Some((1, 0)), Some((2, 0)));
    assert!(panel.collapsed.is_empty() && panel.selected.is_none());
    panel.collapsed.insert(9);
    panel.switch(Some((2, 0)), Some((1, 0)));
    assert!(panel.collapsed.contains(&7) && panel.selected == Some(3));
    panel.switch(Some((1, 0)), Some((2, 0)));
    assert!(panel.collapsed.contains(&9));
}
