//! The smallest bedtime app: a window of panels with one kind of panel (a
//! counter), a setting, a status bar item, menus, a command palette and a
//! settings panel, all saved between runs. No documents, no workspaces.
//!
//! ```sh
//! cargo run -p hello
//! ```
//!
//! This is the measure of how much an app has to write. `CounterPlugin` is
//! the app's own feature, and is as short as a plugin should be. Everything
//! under "What bedtime should do" is what an app has to do today and
//! shouldn't have to: each is a gap for the framework to close (see
//! docs/concerns.md, "The second app").
use bedtime::{
    automation::AutomationPlugin,
    persistence::{DirectoryStore, PersistenceAppExt, PersistenceDirectory, ReflectPersist},
    ui::{
        UiContext, UiPlugin, egui,
        layouts::{LayoutCamera, LayoutSurface, LayoutsWorldExt, Panel, PanelsWorldExt, SurfaceBody},
    },
    workbench::{
        Action, ActionAppExt, ActionInvocation, ActionResult, CloseRequested, CommandPalettePlugin,
        Handler, KeyChord, MenusAppExt, OnWorkspaceOpen, StandardMenu, StandardMenusPlugin,
        WorkbenchPlugins, open_new,
        persistence::WorkbenchStore,
        settings::{SettingsAppExt, SettingsPanelPlugin, SettingsWorldExt, settings},
        status::{self, GlobalStatus, StatusAppExt},
    },
};
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins((ShellPlugin { name: "Hello" }, CounterPlugin))
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

/// Everything here is the same in any app: a window of panels with a status
/// bar, saved in the app's config folder, with settings, a palette and Quit.
struct ShellPlugin {
    name: &'static str,
}

impl Plugin for ShellPlugin {
    fn build(&self, app: &mut App) {
        let name = self.name;
        app.add_plugins((
            // 1. The window: the workbench decides when it closes.
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: name.into(),
                    // A script's window stays behind what's being worked in.
                    focused: !AutomationPlugin::from_env().active(),
                    ..default()
                }),
                close_when_requested: false,
                ..default()
            }),
            // 2. The name again, for the menu bar and the config folder.
            UiPlugin,
            WorkbenchPlugins
                .set(StandardMenusPlugin { app_name: name })
                .set(bedtime::persistence::PersistencePlugin::for_app(
                    &name.to_lowercase(),
                )),
            SettingsPanelPlugin,
            CommandPalettePlugin,
            AutomationPlugin::from_env(),
        ));

        // 3. Somewhere to save: the workbench has a store, but only
        // workspaces gives it a place and makes it a settings layer.
        if let Some(directory) = app.world().resource::<PersistenceDirectory>().path() {
            let directory = directory.to_owned();
            app.add_store::<WorkbenchStore>(DirectoryStore::new(directory));
        }
        app.add_settings_layer::<WorkbenchStore>();

        // 4. Quit: the action, its key and its menu item.
        app.add_action_handler::<Quit>(Handler::new(quit))
            .add_menu_item::<Quit>(StandardMenu::App, "quit");

        // 5. The window's surface, body and camera; 6. a first layout.
        app.add_systems(Startup, surface)
            .add_systems(OnWorkspaceOpen, first_layout);
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

fn surface(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands
        .entity(*window)
        .insert((LayoutSurface::default(), SurfaceBody(status::body)));
    commands.spawn((Camera2d, LayoutCamera));
}

/// With nothing saved, start with a counter.
fn first_layout(world: &mut World) {
    if world.layouts().active().is_none() {
        world.open_panel(Counter::default());
    }
}
