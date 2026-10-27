use bedtime_ui::{BuiltinIcon, UiContext, UiPlugin, egui};
use bedtime_workbench::{
    PanelFactory, Titlebar, WorkbenchCamera, WorkbenchPlugin, WorkbenchSurface, panel_drag_source,
    persistence::{DirectoryStore, StoreAppExt, WorkbenchStore},
};
use bevy::{prelude::*, window::PrimaryWindow};
use std::path::PathBuf;

mod config;
mod panels;
mod settings;

use settings::SettingsPanel;

fn main() -> AppExit {
    let config = config_directory();
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bEd".into(),
                resolution: (960, 640).into(),
                titlebar_show_title: false,
                titlebar_transparent: true,
                fullsize_content_view: true,
                transparent: true,
                composite_alpha_mode: if cfg!(target_os = "macos") {
                    bevy::window::CompositeAlphaMode::PostMultiplied
                } else {
                    bevy::window::CompositeAlphaMode::Auto
                },
                ..default()
            }),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        WorkbenchPlugin,
        settings::SettingsPlugin {
            path: config
                .as_ref()
                .map(|directory| directory.join("settings.ron")),
        },
    ))
    .add_systems(Startup, setup);
    // Until workspaces exist, the layout is saved alongside the global settings.
    if let Some(directory) = config {
        app.add_store::<WorkbenchStore>(DirectoryStore::new(directory));
    }
    app.run()
}

/// `$BED_CONFIG_DIR`, else `$XDG_CONFIG_HOME/bed`, else `~/.config/bed`.
fn config_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("BED_CONFIG_DIR") {
        return Some(directory.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("bed"))
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        WorkbenchSurface::default(),
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: draw_titlebar,
        },
    ));
    commands.spawn((Camera2d, WorkbenchCamera));
}

fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        ui.visuals_mut().button_frame = false;
        let response = context
            .frame(ui)
            .icon_button_sized(BuiltinIcon::Settings, 16.0)
            .interact(egui::Sense::click_and_drag());
        context.frame(ui).tooltip(&response, "Settings");
        let factory = PanelFactory::of::<SettingsPanel>();
        panel_drag_source(&response, factory.clone());
        if response.clicked()
            && let Err(error) = panels::open_panel::<SettingsPanel>(world, (), factory)
        {
            warn!("Cannot open settings: {error}");
        }
    });
}
