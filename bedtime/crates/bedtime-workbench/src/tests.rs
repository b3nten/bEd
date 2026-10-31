//! Documents and closing with layouts: the parts of workbench that need both.
use super::*;

#[derive(Component, PartialEq, Debug)]
struct Draft(String);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugins);
    layout(app.world_mut());
    app
}

#[test]
fn user_closure_without_unsaved_documents_closes_immediately() {
    let mut app = app();
    let w = app.world_mut().layouts_mut();
    let layout = w.active().unwrap();
    let panel = w.create_panel(layout, "Nothing to save").unwrap();
    w.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel]))
        .unwrap();
    assert!(w.world().get_entity(panel).is_err());

    // An edit made in a layout that is no longer active is refused.
    let panel = w.create_panel(layout, "Stale request").unwrap();
    let other = w.create("Other").unwrap();
    w.activate(other).unwrap();
    assert!(
        w.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel]))
            .is_err()
    );
    assert!(w.world().get_entity(panel).is_ok());
}

#[test]
fn moving_between_layouts_preserves_shared_document_and_local_state() {
    let mut app = app();
    let w = app.world_mut().layouts_mut();
    let first = w.active().unwrap();
    let panel = w.create_panel(first, "Viewer").unwrap();
    let document = w
        .world_mut()
        .workbench_mut()
        .load_document(MemorySource::new("File", TextBuffer::from("text")))
        .unwrap();
    w.world_mut()
        .entity_mut(panel)
        .insert(DocumentReference::new(document.id()));
    w.world_mut()
        .entity_mut(panel)
        .insert(Draft("view state".into()));
    let second = w.create("Other").unwrap();
    let area = w.destination_area(second).unwrap();
    w.relocate_panel(panel, area).unwrap();
    assert_eq!(
        w.world()
            .get::<DocumentReference>(panel)
            .unwrap()
            .document(),
        document.id()
    );
    assert_eq!(
        w.world().get::<Draft>(panel),
        Some(&Draft("view state".into()))
    );
}

/// Bare test panels; applications spawn their own panel components instead.
pub(crate) trait TestPanels {
    fn create_panel(&mut self, layout: Entity, title: &str) -> Result<Entity, String>;
    fn create_panel_in_area(&mut self, area: Entity, title: &str) -> Result<Entity, String>;
}

impl TestPanels for Layouts {
    fn create_panel(&mut self, layout: Entity, title: &str) -> Result<Entity, String> {
        let area = self.destination_area(layout)?;
        self.create_panel_in_area(area, title)
    }
    fn create_panel_in_area(&mut self, area: Entity, title: &str) -> Result<Entity, String> {
        self.world()
            .get::<Area>(area)
            .ok_or("Area is unavailable")?;
        let instance = PanelInstance {
            title: title.into(),
            ..default()
        };
        Ok(self.world_mut().spawn((instance, InArea(area))).id())
    }
}

/// A layout fixture; installing the plugin deliberately creates no layout.
pub(crate) fn layout(world: &mut World) -> Entity {
    let layout = world.layouts_mut().create("Test").unwrap();
    if world.layouts().active().is_none() {
        world.layouts_mut().activate(layout).unwrap();
    }
    layout
}
