use crate::ImageStates;
use bed_api::features::HostFrame;
use bed_api::{
    status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText},
    workbench::WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) fn items() -> StatusItems {
    StatusItems(vec![StatusItem {
        id: "bed.image.document",
        label: "Image",
        priority: 50,
        icon: "image",
        prefer_icon: false,
        measure,
        draw,
        tooltip,
    }])
}

pub(super) fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    if !context.is_focused() {
        return None;
    }
    let panel = world
        .get_non_send::<ImageStates>()?
        .get(context.publisher)?;
    let frame = world.get_resource::<HostFrame>()?;
    let document = frame
        .documents
        .iter()
        .find(|document| document.id == panel.document)?;
    let mut details = format!("{}\n{} bytes", document.path, document.bytes.len());
    let summary = if let Some(decoded) = &panel.decoded {
        let fit = panel.fit.unwrap_or_else(|| {
            frame.context().settings_for(crate::PLUGIN_ID)["fit_by_default"]
                .as_bool()
                .unwrap_or(true)
        });
        let scale = if fit {
            panel.canvas.map(|canvas| canvas.scale)
        } else {
            Some(panel.zoom)
        };
        let zoom = if fit {
            "Fit".to_owned()
        } else {
            format!("{:.0}%", panel.zoom * 100.0)
        };
        details.push_str(&format!(
            "\n{} × {} pixels",
            decoded.size[0], decoded.size[1]
        ));
        if let Some(scale) = scale {
            details.push_str(&format!("\nZoom {:.1}%", scale * 100.0));
        }
        format!("{} × {} · {zoom}", decoded.size[0], decoded.size[1])
    } else if panel.error.is_some() {
        "Image unavailable".into()
    } else {
        "Loading image…".into()
    };
    if let Some(error) = &panel.error {
        details.push_str(&format!("\n{error}"));
    }
    let mut text = StatusText::new(summary, details);
    text.error = panel.error.is_some();
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
