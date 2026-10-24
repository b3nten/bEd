//! Optional panel composition; source loading and persistence remain host-owned.
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::{Panel, PanelInstance};
use bevy::prelude::*;
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>(), crate::ModelScene)]
pub struct ModelPanel;
impl Panel for ModelPanel {
    const KEY: &'static str = "bedtime.model";
    const LABEL: &'static str = "Model";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        crate::ModelView::show(ui, world, panel);
    }
}
