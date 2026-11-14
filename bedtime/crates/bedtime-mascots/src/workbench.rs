//! The mascots as panels: Bed and Ducky, saved with the layout.
use crate::{Mascot, MascotKind, MascotScene};
use bedtime_ui::{UiComponent as _, UiContext, egui};
use bedtime_workbench::Panel;
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bevy::prelude::*;

/// Open Bed: a new one, in the focused area.
pub struct ShowBed;
impl bedtime_workbench::Action for ShowBed {
    const KEY: &'static str = "mascots.show_bed";
    const LABEL: &'static str = "Bed";
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(MascotScene = MascotScene::new(MascotKind::Bed))]
pub struct BedPanel;

impl Panel for BedPanel {
    const LABEL: &'static str = "Bed";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, entity: Entity) {
        Mascot::draw(ui, world, context, entity);
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(MascotScene = MascotScene::new(MascotKind::Duck))]
pub struct DuckPanel;

impl Panel for DuckPanel {
    const LABEL: &'static str = "Ducky";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, entity: Entity) {
        Mascot::draw(ui, world, context, entity);
    }
}
