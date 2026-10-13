//! Connect optional preview ownership to desktop canvas lifecycle.
use crate::{DesktopCanvasSystems, DesktopSystems};
use bevy::prelude::*;

pub struct DesktopPreviewIntegrationPlugin;
impl Plugin for DesktopPreviewIntegrationPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<crate::DesktopIntegrationPlugin>(),
            "DesktopPreviewIntegrationPlugin requires DesktopIntegrationPlugin"
        );
        if !app.is_plugin_added::<crate::preview::PreviewPlugin>() {
            app.add_plugins(crate::preview::PreviewPlugin);
        }
        app.add_systems(PreUpdate, begin.before(DesktopSystems::Ui))
            .add_systems(
                PreUpdate,
                retire
                    .after(DesktopCanvasSystems::Render)
                    .before(DesktopSystems::RouteInput),
            );
    }
}
fn begin(world: &mut World) {
    crate::preview::begin_preview_frame(world);
}
fn retire(world: &mut World) {
    let live = world
        .query::<&bed_ui::gpu::CanvasOutput>()
        .iter(world)
        .map(|output| output.0.handle.0)
        .collect();
    crate::preview::retire_previews(world, &live);
}
