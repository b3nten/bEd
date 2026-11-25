//! The Schedules panel: the viewer in a workbench panel, opened from View.
use crate::{AppSnapshot, ScheduleExtractPlugin, view};
use bedtime_ui::{Text, UiContext, ViewUi, draw, egui};
use bedtime_workbench::PanelsAppExt as _;
use bedtime_workbench::{Panel, persistence::ReflectPersist};
use bevy::prelude::*;

impl bedtime_workbench::ShowPanel for SchedulesPanel {
    const SHOW: &'static str = "schedules.show";
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
            .add_panel::<SchedulesPanel>();
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
        draw::with_view(
            ui,
            world,
            context,
            panel,
            |panel: &mut SchedulesPanel| &mut panel.view,
            schedules,
        );
    }
}

/// The viewer, drawn: a system that reads the schedules as last extracted,
/// with the panel's view (what's shown, selected and laid out) lent to it.
fn schedules(
    ViewUi {
        ui, view, context, ..
    }: ViewUi<Box<view::ViewState>>,
    snapshot: Option<Res<AppSnapshot>>,
) {
    match snapshot {
        Some(snapshot) => view::frame(ui, context.theme, &snapshot, view),
        None => {
            ui.add(Text::new("The schedules aren't being read."));
        }
    }
}
