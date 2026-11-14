//! The Schedules panel: the viewer in a workbench panel, opened from View.
use crate::{AppSnapshot, ScheduleExtractPlugin, view};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{
    Action, ActionAppExt, Handler, MenusAppExt, Panel, open_new, persistence::ReflectPersist,
};
use bevy::prelude::*;

pub struct OpenSchedules;
impl Action for OpenSchedules {
    const KEY: &'static str = "schedules.show";
    const LABEL: &'static str = "Schedules";
}

/// Reads the app's schedules (`ScheduleExtractPlugin`) and offers them in a
/// panel, under View. It asks nothing of the app's other plugins: their
/// systems are read from the schedules they were added to.
#[derive(Default)]
pub struct SchedulesPlugin;

impl Plugin for SchedulesPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ScheduleExtractPlugin>() {
            app.add_plugins(ScheduleExtractPlugin::default());
        }
        app.register_type::<SchedulesPanel>()
            .add_action_handler::<OpenSchedules>(Handler::new(open_new::<SchedulesPanel>))
            .add_menu_item::<OpenSchedules>(bedtime_workbench::StandardMenu::View, "tools");
    }
}

/// A panel on the app's schedules. Each has its own view: what's shown,
/// selected and hidden. Only that it's open is saved.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
pub struct SchedulesPanel {
    #[reflect(ignore)]
    view: Box<view::ViewState>,
}

impl Panel for SchedulesPanel {
    const LABEL: &'static str = "Schedules";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut state) = world
            .get_mut::<SchedulesPanel>(panel)
            .map(|mut panel| std::mem::take(&mut panel.view))
        else {
            return;
        };
        match world.get_resource::<AppSnapshot>() {
            Some(snapshot) => view::frame(ui, context.theme, snapshot, &mut state),
            None => {
                ui.label("The schedules aren't being read.");
            }
        }
        if let Some(mut panel) = world.get_mut::<SchedulesPanel>(panel) {
            panel.view = state;
        }
    }
}
