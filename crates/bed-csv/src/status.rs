use crate::CsvStates;
use bed_workbench_api::{
    Action, StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) fn items() -> StatusItems {
    StatusItems(vec![StatusItem {
        id: "bed.csv.document",
        label: "CSV table",
        priority: 50,
        icon: "spreadsheet",
        prefer_icon: false,
        measure,
        draw,
        tooltip,
    }])
}
fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    if !context.is_focused() {
        return None;
    }
    Some(
        world
            .get_non_send::<CsvStates>()?
            .get(context.publisher)?
            .status(),
    )
}
fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    Some(presentation(world, context)?.measure(ui))
}
fn draw(world: &mut World, ui: &Ui, context: StatusDrawContext, actions: &mut Vec<Action>) {
    if let Some(text) = presentation(world, context.context) {
        text.draw(ui, context, actions);
    }
}
fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(text) = presentation(world, context) {
        text.tooltip(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::CsvDocument;
    use bed_editing::identity::DocumentId;
    use bed_editor_api::EditorPanelBinding;
    use serde_json::Value;

    #[test]
    fn publication_is_owned_by_the_focused_frontend_and_withdraws_on_close() {
        let mut app = App::new();
        app.add_plugins(crate::CsvPlugin);
        let entity = app
            .world_mut()
            .spawn(EditorPanelBinding {
                document: Some(DocumentId(1)),
                ..Default::default()
            })
            .id();
        let creation = bed_workbench_api::PanelCreation {
            entity,
            state: Value::Null,
        };
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel(crate::PANEL_ID)
            .unwrap()
            .factory;
        app.world_mut()
            .run_system_with(handler, creation)
            .unwrap()
            .unwrap();
        let context = StatusContext {
            publisher: entity,
            item_id: "bed.csv.document",
            focused_panel: None,
            focused_frontend: Some(entity),
            source: Some(entity),
            area: 0,
        };
        assert!(
            presentation(app.world(), context)
                .unwrap()
                .summary
                .contains("Parsing CSV")
        );
        assert!(
            presentation(
                app.world(),
                StatusContext {
                    focused_frontend: None,
                    ..context
                }
            )
            .is_none()
        );
        app.world_mut().entity_mut(entity).remove::<CsvDocument>();
        app.update();
        assert!(presentation(app.world(), context).is_none());
    }
}
