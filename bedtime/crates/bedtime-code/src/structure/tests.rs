use super::*;
use crate::CodePlugin;
use bedtime_document::{BufferEdit, MemorySource};
use bedtime_workbench::{EditOptions, OpenOptions, Target, WorkbenchCorePlugins};

const CODE: &str = "fn cat() {\n    let age = 42;\n}\nfn dog() {}\n";

struct Host {
    app: App,
    /// Two editors of `cats.rs`, and Structure.
    a: Entity,
    b: Entity,
    structure: Entity,
}

fn host() -> Host {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, CodePlugin));
    let world = app.world_mut();
    let layout = world.layouts_mut().create("Test").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let source = MemorySource::new("cats.rs", TextBuffer::from(CODE));
    let open = |world: &mut World, target| {
        world
            .workbench_mut()
            .open_document(
                source.clone(),
                OpenOptions {
                    target,
                    ..default()
                },
            )
            .unwrap()
    };
    let a = open(world, Target::Existing);
    let b = open(world, Target::New);
    let area = world.layouts().destination_area(layout).unwrap();
    let structure = world
        .spawn((StructurePanel::default(), bedtime_workbench::InArea(area)))
        .id();
    world.flush();
    let mut host = Host {
        app,
        a,
        b,
        structure,
    };
    focus(&mut host, a);
    until(&mut host, |world| current_outline(world, a).is_some());
    host
}

fn until(host: &mut Host, done: impl Fn(&World) -> bool) {
    for _ in 0..1000 {
        host.app.update();
        if done(host.app.world()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never got there");
}

fn focus(host: &mut Host, panel: Entity) {
    host.app
        .world_mut()
        .layouts_mut()
        .focus_panel(panel)
        .unwrap();
    host.app.update();
}

fn source(host: &Host) -> Option<Entity> {
    host.app.world().resource::<StructureSource>().0
}

#[test]
fn structure_follows_the_focused_editor_and_remembers_it() {
    let mut host = host();
    let (b, structure) = (host.b, host.structure);
    assert_eq!(source(&host), Some(host.a));
    focus(&mut host, b);
    assert_eq!(source(&host), Some(host.b));
    // Focusing Structure itself keeps showing the last editor.
    focus(&mut host, structure);
    assert_eq!(source(&host), Some(host.b));
    // A closed editor is forgotten.
    host.app.world_mut().layouts_mut().close_panel(b).unwrap();
    host.app.update();
    assert_eq!(source(&host), None);
}

#[test]
fn an_entry_selects_its_name_in_the_editor_and_focuses_it() {
    let mut host = host();
    let structure = host.structure;
    focus(&mut host, structure);
    let world = host.app.world_mut();
    let outline = current_outline(world, host.a).unwrap();
    let labels: Vec<_> = outline.outline.iter().map(|n| n.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.contains("cat")) && labels.iter().any(|l| l.contains("dog")));
    let dog = outline
        .outline
        .iter()
        .find(|n| n.label.contains("dog"))
        .unwrap();
    assert!(reveal_entry(world, host.a, outline.key, dog.id));
    let layout = world.layouts().active().unwrap();
    assert_eq!(world.layouts().focused_panel(layout), Some(host.a));

    // An entry from an older analysis does nothing.
    let document = world.get::<DocumentReference>(host.a).unwrap().document();
    let handle = world
        .workbench()
        .cast_document::<TextBuffer>(document)
        .unwrap();
    world
        .workbench_mut()
        .edit_document(
            handle,
            0,
            BufferEdit::new(0..0, "\n"),
            EditOptions::new("Typing"),
        )
        .unwrap();
    assert!(!reveal_entry(world, host.a, outline.key, dog.id));
}

#[test]
fn the_outline_stays_while_an_edit_is_reanalysed() {
    let mut host = host();
    let world = host.app.world_mut();
    let mut panel = StructurePanel::default();
    panel.refresh(world);
    let before = panel.shown.as_ref().unwrap().1.key;
    let document = world.get::<DocumentReference>(host.a).unwrap().document();
    let handle = world
        .workbench()
        .cast_document::<TextBuffer>(document)
        .unwrap();
    world
        .workbench_mut()
        .edit_document(
            handle,
            0,
            BufferEdit::new(0..0, "\n"),
            EditOptions::new("Typing"),
        )
        .unwrap();
    // The edit frame: no current analysis, but the outline stays.
    panel.refresh(host.app.world());
    assert_eq!(panel.shown.as_ref().unwrap().1.key, before);
    host.app.update();
    panel.refresh(host.app.world());
    assert!(panel.shown.is_some());
    let a = host.a;
    until(&mut host, |world| current_outline(world, a).is_some());
    panel.refresh(host.app.world());
    assert_eq!(panel.shown.as_ref().unwrap().1.key.revision, 1);
}
