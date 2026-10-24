use crate::{Mascot, MascotKind, MascotScene};
use bedtime_ui2::{Component as _, UiContext, egui};
use bedtime_workbench2::{Panel, PanelInstance};
use bevy::prelude::*;

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>(), MascotScene = MascotScene::new(MascotKind::Bed))]
pub struct BedPanel;
impl Panel for BedPanel {
    const KEY: &'static str = "bedtime.bed";
    const LABEL: &'static str = "Bed";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, entity: Entity) {
        Mascot::draw(ui, world, context, entity);
    }
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>(), MascotScene = MascotScene::new(MascotKind::Duck))]
pub struct DuckPanel;
impl Panel for DuckPanel {
    const KEY: &'static str = "bedtime.duck";
    const LABEL: &'static str = "Ducky";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, entity: Entity) {
        Mascot::draw(ui, world, context, entity);
    }
}
