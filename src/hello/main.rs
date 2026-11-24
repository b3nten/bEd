//! The smallest bedtime app: a window of panels with one kind of panel (a
//! counter), a setting, a status bar item, menus, a command palette and a
//! settings panel, all saved between runs. No documents, no workspaces.
//!
//! ```sh
//! cargo run -p hello
//! ```
//!
//! This is the measure of how much an app has to write: `AppPlugins` is the
//! app, from its name, and `CounterPlugin` is its own feature, as short as a
//! plugin should be (see docs/concerns.md, "The second app").
use bedtime::BedtimeWorldExt as _;
use bedtime::{
    AppPlugins,
    automation::AutomationPlugin,
    persistence::ReflectPersist,
    ui::{UiContext, layouts::Panel, prelude::*},
    workbench::{
        OnWorkspaceOpen, PanelsAppExt, ShowPanel,
        settings::{SettingsAppExt, settings},
        status::{GlobalStatus, StatusAppExt},
    },
};
use bevy::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins((
            AppPlugins::new("Hello"),
            AutomationPlugin::from_env(),
            CounterPlugin,
        ))
        .run()
}

// --- The app's own feature -------------------------------------------------

/// Counters: a panel that counts, by a step set in Settings, and their total
/// in the status bar. A new window starts with one.
struct CounterPlugin;

impl Plugin for CounterPlugin {
    fn build(&self, app: &mut App) {
        app.add_panel::<Counter>()
            .register_setting::<Step>()
            .add_global_status(total)
            .add_systems(OnWorkspaceOpen, |world: &mut World| {
                if world.bedtime().layouts().active().is_none() {
                    world.bedtime_mut().open_panel(Counter::default());
                }
            });
    }
}

settings! {
    "Counter" {
        pub Step: i32 {
            default: 1,
            key: "counter.step",
            label: "Step",
            description: "How much a click adds",
        }
    }
}

impl ShowPanel for Counter {
    const SHOW: &'static str = "counter.show";
}

/// A counter. Its count is saved with it.
#[bedtime::ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
struct Counter {
    count: i32,
}

impl Panel for Counter {
    const LABEL: &'static str = "Counter";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw::with_system(ui, world, context, panel, counter);
    }
}

/// The counter, drawn: a system that reads, and queues what a click changes.
fn counter(
    PanelUi {
        ui, entity, state, ..
    }: PanelUi<Counter>,
) {
    let step = draw::world(ui).bedtime().settings().get::<Step>();
    Column::new().padding(Space::Lg).show(ui, |ui| {
        ui.add(Text::heading(state.count.to_string()));
        if ui.add(Button::new(format!("Add {step}"))).clicked() {
            draw::update(ui, entity, move |counter: &mut Counter| {
                counter.count += step;
            });
        }
    });
}

/// The status bar's item: every counter's count, added up.
fn total(world: &World, status: &mut GlobalStatus) {
    let Some(mut counters) = world.try_query::<&Counter>() else {
        return;
    };
    let total: i32 = counters.iter(world).map(|counter| counter.count).sum();
    status.text(format!("Total {total}"));
}
