//! Explicit application infrastructure for editor integration fixtures.
use bed::application::{RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore};
use bed::config::Settings;
use bevy::prelude::*;
use dear_imgui_rs::Context;
use std::{io, path::PathBuf};

#[allow(dead_code)] // Shared by integration targets with different setup paths.
pub fn install(app: &mut App, settings: Settings) -> io::Result<()> {
    let store = WorkspaceStore::load(
        &bed_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
            .unwrap(),
    )?;
    app.insert_resource(
        bed_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
            .unwrap(),
    )
    .insert_non_send(settings)
    .insert_non_send(bed_ui::icons::Icons::default())
    .insert_non_send(WorkspacePersistence::new(store))
    .init_non_send::<bed_ui::font::UiFonts>()
    .insert_resource(RemoteHelperDirectory(PathBuf::new()));
    Ok(())
}

#[allow(dead_code)]
pub fn prepare_ui(world: &mut World, context: &mut Context) -> io::Result<()> {
    bed::config::sync_view(world)?;
    let appearance = {
        let mut settings = world.non_send_mut::<Settings>();
        settings.refresh();
        settings.appearance()
    };
    context
        .set_ini_filename(None::<PathBuf>)
        .map_err(io::Error::other)?;
    let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::DOCKING_ENABLE;
    context.io_mut().set_config_flags(flags);
    let mut runtime = world
        .remove_non_send::<bed_ui::UiRuntime>()
        .unwrap_or_else(|| bed_ui::UiRuntime::new(context));
    let mut fonts = world
        .remove_non_send::<bed_ui::font::UiFonts>()
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
    let context = world.non_send::<bed_ui::UiRuntime>().frame_context();
    world.non_send_mut::<bed_ui::UiRuntime>().begin(ui);
    let result = context.scope(|| bed::application::draw_ui(world, ui));
    world.non_send_mut::<bed_ui::UiRuntime>().end(ui);
    result
}
