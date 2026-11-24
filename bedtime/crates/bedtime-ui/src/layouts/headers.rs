//! Panel headers: bars a plugin draws at the top of panels, above their
//! content, such as a document's "Changed on disk". Each header decides for
//! itself whether a panel gets one; most panels get none.
use bevy::prelude::*;
use egui;

/// Draw a header for `panel` at the top of `ui`, or nothing. What it draws
/// takes room from the panel's content below.
pub type PanelHeaderFn = fn(&mut egui::Ui, &mut World, &crate::UiContext<'_>, Entity);

#[derive(Resource, Default)]
pub(crate) struct PanelHeaders(Vec<PanelHeaderFn>);

pub trait PanelHeaderAppExt {
    /// Draw `header` at the top of every panel (it chooses which get one).
    fn add_panel_header(&mut self, header: PanelHeaderFn) -> &mut Self;
}

impl PanelHeaderAppExt for App {
    fn add_panel_header(&mut self, header: PanelHeaderFn) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<PanelHeaders>()
            .0
            .push(header);
        self
    }
}

/// Draw the registered headers for `panel`.
pub(crate) fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &crate::UiContext<'_>,
    panel: Entity,
) {
    let headers = world
        .get_resource::<PanelHeaders>()
        .map(|headers| headers.0.clone())
        .unwrap_or_default();
    for header in headers {
        header(ui, world, context, panel);
    }
}
