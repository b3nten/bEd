//! Application context-menu contribution for the library document viewer.
pub(crate) use bedtime_image::document::*;
pub(crate) fn draw(
    ui: &mut bedtime_ui::egui::Ui,
    world: &mut bevy::prelude::World,
    context: &bedtime_ui::UiContext<'_>,
    panel: bevy::prelude::Entity,
) {
    bedtime_image::document::draw(ui, world, context, panel, |ui, world| {
        crate::opening::viewer_menu(ui, world, panel)
    });
}
