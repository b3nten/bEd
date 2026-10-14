//! Explicit application infrastructure for editor integration fixtures.
use bed::application::{RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore};
use bed::config::AppConfig;
use bevy::prelude::*;
use dear_imgui_rs::Context;
use std::{io, path::PathBuf};

#[allow(dead_code)] // Shared by integration targets with different setup paths.
pub fn install(app: &mut App, settings: AppConfig) -> io::Result<()> {
    let paths =
        bedtime_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)?;
    let store = WorkspaceStore::load(&paths)?;
    app.insert_resource(paths)
        .insert_non_send(settings)
        .insert_non_send(bedtime_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(store))
        .init_non_send::<bedtime_ui::font::UiFonts>()
        .insert_resource(RemoteHelperDirectory(PathBuf::new()));
    app.add_systems(Startup, |world: &mut World| {
        let config = world.non_send::<AppConfig>();
        let path = config.settings_path.clone();
        let resources = config.resources_root.clone();
        bedtime_core::settings::initialize_settings(world, Some(path)).unwrap();
        bed::themes::install_theme_services(world, resources);
        bed::config::register_app_settings(world).unwrap();
        bedtime_core::settings::finalize_settings(world).unwrap();
        bed::themes::configure_themes(world);
    });
    Ok(())
}

#[allow(dead_code)]
pub fn prepare_ui(world: &mut World, context: &mut Context) -> io::Result<()> {
    bed::themes::configure_themes(world);
    let palette = world
        .resource::<bed::themes::ActiveTheme>()
        .file
        .runtime_palette();
    let mut appearance = bedtime_ui::appearance::UiTheme {
        palette,
        ..Default::default()
    };
    bedtime_core::apply_ui_preferences(
        &mut appearance,
        world.resource::<bedtime_api::settings::SettingsSnapshot>(),
    );
    context
        .set_ini_filename(None::<PathBuf>)
        .map_err(io::Error::other)?;
    let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::DOCKING_ENABLE;
    context.io_mut().set_config_flags(flags);
    let mut runtime = world
        .remove_non_send::<bedtime_ui::UiRuntime>()
        .unwrap_or_else(|| bedtime_ui::UiRuntime::new(context));
    let mut fonts = world
        .remove_non_send::<bedtime_ui::font::UiFonts>()
        .unwrap_or_default();
    runtime.prepare(context, &appearance, &mut fonts)?;
    world.insert_non_send(runtime.frame_context());
    world.insert_non_send(runtime);
    world.insert_non_send(fonts);
    Ok(())
}

/// Raw/headless fixture boundary; real desktop drawing owns its own scope.
#[allow(dead_code)]
pub fn draw_ui(world: &mut World, ui: &dear_imgui_rs::Ui) -> io::Result<()> {
    let context = world.non_send::<bedtime_ui::UiRuntime>().frame_context();
    world.non_send_mut::<bedtime_ui::UiRuntime>().begin(ui);
    let result = context.scope(|| crate::application_requests::draw_ui(world, ui));
    world.non_send_mut::<bedtime_ui::UiRuntime>().end(ui);
    result
}
