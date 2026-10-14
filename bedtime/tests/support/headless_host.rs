//! Minimal document host for integration tests; applications choose their features and policies.
use bevy::prelude::*;
use dear_imgui_rs::{Context, Ui};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn install(app: &mut App, config: &Path) {
    app.add_plugins((MinimalPlugins, bevy::input::InputPlugin));
    app.insert_resource(
        bedtime_api::launch::ApplicationPaths::new("fixture", Some(config.to_owned()), None)
            .unwrap(),
    );
    app.add_plugins(bedtime_core::DocumentHostPlugin);
    bedtime_core::settings::initialize_settings(app.world_mut(), None).unwrap();
}

pub fn finish(app: &mut App, directory: &Path) {
    bedtime_core::initialize_file_context(
        app.world_mut(),
        bedtime_files::FileContext::local(directory.to_owned()).unwrap(),
        bedtime_files::FileServices::local(),
    )
    .unwrap();
    bedtime_core::settings::finalize_settings(app.world_mut()).unwrap();
    app.update();
}

pub fn host(
    world: &mut World,
    source: Option<Entity>,
    action: bedtime_api::features::HostAction,
) -> Result<bedtime_core::HostOutcome, String> {
    let id = bedtime_core::submit_host_request(world, source, action);
    wait_host(world, id)
}

pub fn wait_host(
    world: &mut World,
    id: bedtime_core::HostRequestId,
) -> Result<bedtime_core::HostOutcome, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        bedtime_core::poll_document_services(world);
        bedtime_core::resolve_documents(world);
        if let Some(result) = bedtime_core::take_host_result(world, id) {
            return result;
        }
        assert!(Instant::now() < deadline, "host request timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn workbench(
    world: &mut World,
    action: bedtime_api::workbench::WorkbenchAction,
) -> Result<bedtime_workbench::WorkbenchOutcome, String> {
    let id = world
        .resource_mut::<bedtime_workbench::WorkbenchRequests>()
        .submit(action);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        bedtime_core::poll_document_services(world);
        bedtime_core::resolve_documents(world);
        if let Some(result) = world
            .resource_mut::<bedtime_workbench::WorkbenchRequests>()
            .take_result(id)
        {
            return result;
        }
        assert!(Instant::now() < deadline, "workbench request timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn invoke(
    world: &mut World,
    command: &str,
    source: Option<Entity>,
) -> Result<bedtime_workbench::WorkbenchOutcome, String> {
    let mut invocation =
        bedtime_workbench::capture_command(world, command, serde_json::Value::Null);
    invocation.source = source;
    workbench(
        world,
        bedtime_api::workbench::WorkbenchAction::Invoke(invocation),
    )
}

pub fn prepare_ui(world: &mut World, context: &mut Context) -> Result<(), String> {
    let mut theme = bedtime_ui::appearance::UiTheme::default();
    bedtime_core::apply_ui_preferences(
        &mut theme,
        world.resource::<bedtime_api::settings::SettingsSnapshot>(),
    );
    context
        .set_ini_filename(None::<PathBuf>)
        .map_err(|error| error.to_string())?;
    let flags = context.io().config_flags() | dear_imgui_rs::ConfigFlags::DOCKING_ENABLE;
    context.io_mut().set_config_flags(flags);
    let mut runtime = world
        .remove_non_send::<bedtime_ui::UiRuntime>()
        .unwrap_or_else(|| bedtime_ui::UiRuntime::new(context));
    let mut fonts = world
        .remove_non_send::<bedtime_ui::font::UiFonts>()
        .unwrap_or_default();
    runtime
        .prepare(context, &theme, &mut fonts)
        .map_err(|error| error.to_string())?;
    world.insert_non_send(runtime.frame_context());
    world.insert_non_send(runtime);
    world.insert_non_send(fonts);
    Ok(())
}

pub fn draw_ui(world: &mut World, ui: &Ui) -> Result<(), String> {
    bedtime_core::publish_documents(world);
    let context = world.non_send::<bedtime_ui::UiRuntime>().frame_context();
    world.non_send_mut::<bedtime_ui::UiRuntime>().begin(ui);
    context.scope(|| bedtime_core::draw_workbench(world, ui));
    world.non_send_mut::<bedtime_ui::UiRuntime>().end(ui);
    bedtime_core::resolve_documents(world);
    Ok(())
}
