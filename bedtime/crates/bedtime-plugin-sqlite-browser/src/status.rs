use crate::SqliteStates;
use bedtime_api::{
    status::{
        StatusCommand, StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText,
    },
    workbench::WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) const SHOW_COMMAND: &str = "bedtime.sqlite.status.show";
pub(crate) const CANCEL_QUERY_COMMAND: &str = "bedtime.sqlite.status.cancel-query";

pub(crate) fn items() -> StatusItems {
    StatusItems(vec![
        StatusItem {
            id: "bedtime.sqlite.document",
            label: "SQLite result",
            priority: 50,
            icon: "database",
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
        StatusItem {
            id: "bedtime.sqlite.query",
            label: "SQLite query",
            priority: 150,
            icon: "database",
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
    ])
}

pub(super) fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    let panel = world
        .get_non_send::<SqliteStates>()?
        .get(context.publisher)?;
    let running = panel.query_running();
    if context.item_id == "bedtime.sqlite.query" {
        if !running {
            return None;
        }
    } else if !context.is_focused() || running {
        return None;
    }
    let mut text = panel.status();
    if let Some(serial) = panel.query_serial() {
        text = text.command("Show query", SHOW_COMMAND);
        text.commands.push(StatusCommand {
            label: "Cancel query",
            command: CANCEL_QUERY_COMMAND,
            arguments: serde_json::json!({"serial": serial}),
            enabled: true,
        });
    }
    Some(text)
}

fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    Some(presentation(world, context)?.measure(ui))
}

fn draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    if let Some(text) = presentation(world, context.context) {
        text.draw(ui, context, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(text) = presentation(world, context) {
        text.tooltip(ui);
    }
}
