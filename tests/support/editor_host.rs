//! Explicit application infrastructure for editor integration fixtures.
use bed_config::Settings;
use bed_editor_workbench::{RemoteHelperDirectory, WorkspacePersistence, WorkspaceStore};
use bevy::prelude::*;
use dear_imgui_rs::Context;
use std::{io, path::PathBuf};

pub fn install(app: &mut App, settings: Settings) -> io::Result<()> {
    let store = WorkspaceStore::load(&settings.config_dir)?;
    app.insert_non_send(settings)
        .insert_non_send(bed_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(store))
        .init_non_send::<bed_ui::font::UiFonts>()
        .insert_resource(RemoteHelperDirectory(PathBuf::new()));
    Ok(())
}

#[allow(dead_code)]
pub fn prepare_ui(world: &mut World, context: &mut Context) -> io::Result<()> {
    let (appearance, embedded) = {
        let mut settings = world.non_send_mut::<Settings>();
        settings.check_settings_file();
        settings.refresh();
        (settings.appearance(), settings.is_embedded)
    };
    context
        .set_ini_filename(None::<PathBuf>)
        .map_err(io::Error::other)?;
    let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::DOCKING_ENABLE;
    context.io_mut().set_config_flags(flags);
    world
        .non_send_mut::<bed_ui::font::UiFonts>()
        .apply(context, &appearance.fonts, embedded)?;
    bed_ui::appearance::apply_style(context, &appearance, embedded);
    Ok(())
}
