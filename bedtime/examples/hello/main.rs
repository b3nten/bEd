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
use bedtime::{
    AppPlugins,
    automation::AutomationPlugin,
    persistence::ReflectPersist,
    ui::{
        UiContext, egui,
        layouts::{LayoutsWorldExt, Panel, PanelsWorldExt},
    },
    workbench::{
        Action, ActionAppExt, Handler, MenusAppExt, OnWorkspaceOpen, StandardMenu, open_new,
        settings::{SettingsAppExt, SettingsWorldExt, settings},
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
        app.register_type::<Counter>()
            .register_setting::<Step>()
            .add_action_handler::<ShowCounter>(Handler::new(open_new::<Counter>))
            .add_menu_item::<ShowCounter>(StandardMenu::View, "panels")
            .add_global_status(total)
            .add_systems(OnWorkspaceOpen, first_layout);
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

struct ShowCounter;
impl Action for ShowCounter {
    const KEY: &'static str = "counter.show";
    const LABEL: &'static str = "Counter";
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

    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        let step = world.settings().get::<Step>();
        let Some(mut counter) = world.get_mut::<Counter>(panel) else {
            return;
        };
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            ui.heading(counter.count.to_string());
            if ui.button(format!("Add {step}")).clicked() {
                counter.count += step;
            }
        });
    }
}

/// The status bar's item: every counter's count, added up.
fn total(world: &World, status: &mut GlobalStatus) {
    let Some(mut counters) = world.try_query::<&Counter>() else {
        return;
    };
    let total: i32 = counters.iter(world).map(|counter| counter.count).sum();
    status.text(format!("Total {total}"));
}

/// With nothing saved, start with a counter.
fn first_layout(world: &mut World) {
    if world.layouts().active().is_none() {
        world.open_panel(Counter::default());
    }
}
