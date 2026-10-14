//! Content-independent identity and Docs behavior need only the workbench.
use bedtime_workbench::{api::*, *};
use bevy::prelude::*;
use serde_json::{Value, json};

struct Tool;
impl Panel for Tool {
    fn title(&self, _: &World) -> String {
        "Tool".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
    }
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchPlugin);
    register(app.world_mut(), "fixture", |registrar| {
        registrar.panel(
            "fixture.tool",
            "Tool",
            |_: In<PanelCreation>| -> PanelResult { Ok(Box::new(Tool)) },
        );
    })
    .unwrap();
    app
}
fn request(world: &mut World, action: WorkbenchAction) -> WorkbenchOutcome {
    let id = world.resource_mut::<WorkbenchRequests>().submit(action);
    process_requests(world);
    world
        .resource_mut::<WorkbenchRequests>()
        .take_result(id)
        .unwrap()
        .unwrap()
}
#[test]
fn panel_identity_is_live_entity_and_stable_persisted_number() {
    let mut app = app();
    let first = open_panel(
        app.world_mut(),
        json!({"panel_type":"fixture.tool"}),
        None,
        None,
    )
    .unwrap();
    let number = app.world().get::<PanelPresentation>(first).unwrap().id;
    let other = open_panel(
        app.world_mut(),
        json!({"panel_type":"fixture.tool"}),
        None,
        None,
    )
    .unwrap();
    assert_ne!(first, other);
    assert_ne!(
        number,
        app.world().get::<PanelPresentation>(other).unwrap().id
    );
    request(app.world_mut(), WorkbenchAction::FocusPanel(first));
    assert_eq!(
        app.world().get::<PanelPresentation>(first).unwrap().id,
        number
    );
    request(app.world_mut(), WorkbenchAction::ClosePanel(other));
    assert!(!app.world().entities().contains(other));
    assert_eq!(
        app.world().get::<PanelPresentation>(first).unwrap().id,
        number
    );
}
#[test]
fn show_docs_is_available_and_reuses_the_panel() {
    let mut app = app();
    app.add_plugins(DocsPlugin);
    assert!(
        app.world()
            .resource::<WorkbenchRegistry>()
            .command(SHOW_DOCS_COMMAND)
            .is_some()
    );
    let docs = open_panel(
        app.world_mut(),
        json!({"panel_type":DOCS_PANEL_ID}),
        None,
        None,
    )
    .unwrap();
    let tool = open_panel(
        app.world_mut(),
        json!({"panel_type":"fixture.tool"}),
        None,
        None,
    )
    .unwrap();
    for _ in 0..2 {
        request(app.world_mut(), WorkbenchAction::FocusPanel(tool));
        let invocation = capture_command(app.world(), SHOW_DOCS_COMMAND, Value::Null);
        request(app.world_mut(), WorkbenchAction::Invoke(invocation));
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(docs)
        );
    }
    assert_eq!(
        app.world_mut()
            .query::<&PanelRecord>()
            .iter(app.world())
            .filter(|record| record.panel_type == DOCS_PANEL_ID)
            .count(),
        1
    );
}
