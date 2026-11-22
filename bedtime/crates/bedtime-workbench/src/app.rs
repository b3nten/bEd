//! An app, from its name: `AppPlugins::new("Hello")` is Bevy's plugins with
//! the app's window, bedtime's UI, the workbench saved in the app's config
//! folder, the settings panel, the command palette, and the window's panels
//! with a status bar under them. An app adds it, then `DocumentsPlugins` if
//! its panels show documents, then its own plugins.
use crate::{
    Action, ActionAppExt, ActionInvocation, ActionResult, CloseRequested, Handler, KeyChord,
    MenusAppExt, StandardMenu,
    AppWindowPlugins, CommandPalettePlugin, LayoutCamera, LayoutSurface, StandardMenusPlugin,
    SurfaceBody, persistence::PersistencePlugin, settings::SettingsPanelPlugin,
};
use bevy::{app::PluginGroupBuilder, prelude::*, window::PrimaryWindow};

/// Who the app is. One place for its name: the window's title, the App menu,
/// and where it keeps what it saves.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// As people read it: "bEd".
    pub name: &'static str,
    /// As files and environment variables name it: "bed". Its config folder
    /// is `~/.config/<id>`, or `$<ID>_CONFIG_DIR`
    /// ([`PersistencePlugin::for_app`]).
    pub id: String,
}

/// An app called `name`: see the module. Its parts can be replaced with
/// `.set()` or dropped with `.disable()`, as with Bevy's `DefaultPlugins`
/// (which are among them, unless [`Self::without_bevy_plugins`]).
pub struct AppPlugins {
    info: AppInfo,
    bevy_plugins: bool,
}

impl AppPlugins {
    /// The app's id is its name in lower case, with `-` for what isn't a
    /// letter or digit ("My App" is `my-app`).
    pub fn new(name: &'static str) -> Self {
        let id = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        Self {
            info: AppInfo { name, id },
            bevy_plugins: true,
        }
    }

    /// Another id than the name gives.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.info.id = id.into();
        self
    }

    /// For an app that adds Bevy's plugins itself (with its own window, or
    /// some of them changed). Its `WindowPlugin` needs
    /// `close_when_requested: false`: the session decides when a window
    /// closes, after unsaved work is settled.
    pub fn without_bevy_plugins(mut self) -> Self {
        self.bevy_plugins = false;
        self
    }
}

impl PluginGroup for AppPlugins {
    fn build(self) -> PluginGroupBuilder {
        let AppInfo { name, id } = &self.info;
        let mut group = PluginGroupBuilder::start::<Self>();
        if self.bevy_plugins {
            group = group.add_group(DefaultPlugins).set(WindowPlugin {
                primary_window: Some(Window {
                    title: (*name).into(),
                    ..default()
                }),
                close_when_requested: false,
                ..default()
            });
        }
        group
            .add(bedtime_ui::UiPlugin)
            .add_group(AppWindowPlugins)
            .set(StandardMenusPlugin { app_name: name })
            .set(PersistencePlugin::for_app(id))
            .add(SettingsPanelPlugin)
            .add(CommandPalettePlugin)
            .add(ShellPlugin(self.info.clone()))
    }
}

/// The app's window as a window of panels: its layouts, with the status bar
/// under them, drawn by a camera. An app that wants its window otherwise
/// changes it in `Startup`, which runs after this.
pub struct ShellPlugin(pub AppInfo);

impl Plugin for ShellPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.0.clone())
            .add_action_handler::<Quit>(Handler::new(quit))
            .add_menu_item_labelled::<Quit>(
                StandardMenu::App,
                "quit",
                format!("Quit {}", self.0.name),
            )
            .add_systems(PreStartup, shell);
    }
}

/// Quit the app: what would be lost is settled first (`exit`).
pub struct Quit;
impl Action for Quit {
    const KEY: &'static str = "app.quit";
    const LABEL: &'static str = "Quit";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyQ)];
}

fn quit(world: &mut World, _: ActionInvocation) -> ActionResult {
    world.write_message(CloseRequested { window: None });
    Ok(())
}

fn shell(
    mut commands: Commands,
    info: Res<AppInfo>,
    mut windows: Query<(Entity, &mut Window, Has<LayoutSurface>), With<PrimaryWindow>>,
    cameras: Query<(), With<LayoutCamera>>,
) {
    let Ok((window, mut settings, surface)) = windows.single_mut() else {
        return;
    };
    // A window from an app's own `WindowPlugin` keeps the title it gave.
    if settings.title == Window::default().title {
        settings.title = info.name.into();
    }
    if !surface {
        commands
            .entity(window)
            .insert((LayoutSurface::default(), SurfaceBody(crate::status::body)));
    }
    if cameras.is_empty() {
        commands.spawn((Camera2d, LayoutCamera));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_id_comes_from_its_name() {
        assert_eq!(AppPlugins::new("bEd").info.id, "bed");
        assert_eq!(AppPlugins::new("My App").info.id, "my-app");
        assert_eq!(AppPlugins::new("My App").id("mine").info.id, "mine");
    }
}
