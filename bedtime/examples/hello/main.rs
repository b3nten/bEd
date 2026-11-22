//! The smallest bedtime app: a window of panels with one kind of panel (a
//! counter), a setting, a status bar item, menus, a command palette and a
//! settings panel, all saved between runs. No documents, no workspaces.
//!
//! ```sh
//! cargo run -p hello
//! ```
//!
//! This is the measure of how much an app has to write. `AppPlugins` is the
//! app, from its name; `CounterPlugin` is the app's own feature, and is as
//! short as a plugin should be. `RestPlugin` is what an app still has to do
//! and shouldn't have to: each part is a gap for the framework to close (see
//! docs/concerns.md, "The second app").
use bedtime::{
    AppPlugins,
    automation::AutomationPlugin,
    persistence::{DirectoryStore, PersistenceAppExt, PersistenceDirectory, ReflectPersist},
    ui::{
        UiContext, egui,
        layouts::{LayoutsWorldExt, Panel, PanelsWorldExt},
    },
    workbench::{
        Action, ActionAppExt, ActionInvocation, ActionResult, CloseRequested, Handler, KeyChord,
        MenusAppExt, OnWorkspaceOpen, StandardMenu, open_new,
        persistence::WorkbenchStore,
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
            RestPlugin,
            CounterPlugin,
        ))
        .run()
}

// --- The app's own feature -------------------------------------------------

/// Counters: a panel that counts, by a step set in Settings, and their total
/// in the status bar.
struct CounterPlugin;

impl Plugin for CounterPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Counter>()
            .register_setting::<Step>()
            .add_action_handler::<ShowCounter>(Handler::new(open_new::<Counter>))
            .add_menu_item::<ShowCounter>(StandardMenu::View, "panels")
            .add_global_status(total);
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

// --- What bedtime should do ------------------------------------------------

/// What's left for an app to do by hand, the same in any app.
struct RestPlugin;

impl Plugin for RestPlugin {
    fn build(&self, app: &mut App) {
        // 1. Somewhere to save: the workbench has a store, but only
        // workspaces gives it a place and makes it a settings layer.
        if let Some(directory) = app.world().resource::<PersistenceDirectory>().path() {
            let directory = directory.to_owned();
            app.add_store::<WorkbenchStore>(DirectoryStore::new(directory));
        }
        app.add_settings_layer::<WorkbenchStore>();

        // 2. Quit: the action, its key and its menu item.
        app.add_action_handler::<Quit>(Handler::new(quit))
            .add_menu_item::<Quit>(StandardMenu::App, "quit");

        // 3. A first layout.
        app.add_systems(OnWorkspaceOpen, first_layout);
    }
}

struct Quit;
impl Action for Quit {
    const KEY: &'static str = "app.quit";
    const LABEL: &'static str = "Quit";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyQ)];
}

fn quit(world: &mut World, _: ActionInvocation) -> ActionResult {
    world.write_message(CloseRequested { window: None });
    Ok(())
}

/// With nothing saved, start with a counter.
fn first_layout(world: &mut World) {
    if world.layouts().active().is_none() {
        world.open_panel(Counter::default());
    }
}
