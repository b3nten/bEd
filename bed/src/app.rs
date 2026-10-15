//! bEd composition and native menu policy. Core owns shared desktop mechanics.
use crate::application::{AppOptions, AppRuntimePlugin, ApplicationState};
use crate::config::AppConfig;
use bedtime_desktop::{DesktopPlugins, DesktopSystems};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use std::{error::Error, io, path::Path};

type HostResult<T> = Result<T, Box<dyn Error>>;

pub struct BedAppPlugin;
impl Plugin for BedAppPlugin {
    fn build(&self, app: &mut App) {
        bedtime_workbench::register(app.world_mut(), "bed.desktop", |registrar| {
            registrar.command("bed.desktop.new_window", "New Window", new_window_command);
            registrar.global_shortcut("bed.desktop.new_window", &["Primary+Shift+N"]);
        })
        .expect("bEd process commands are registered once");
        app.add_systems(
            PreUpdate,
            configure
                .in_set(DesktopSystems::Configure)
                .after(crate::themes::ThemeSystems::Configure),
        )
        .add_systems(
            PreUpdate,
            after_ui
                .in_set(DesktopSystems::AfterUi)
                .after(bedtime_core::CoreSystems::Requests),
        );
        app.world_mut().spawn((
            bedtime_api::workbench::WorkbenchOverlay(draw_app_error),
            bedtime_api::workbench::OverlayPhase::AfterChrome,
        ));
        app.init_resource::<NativeSmokeMode>()
            .init_resource::<NativeSpawnedInstances>();
        #[cfg(target_os = "macos")]
        app.init_resource::<NativeMenuObservations>().add_systems(
            PreUpdate,
            observe_native_commands.in_set(DesktopSystems::AfterUi),
        );
    }
}

fn configure(world: &mut World) {
    world.insert_resource(crate::native_commands::layout(
        world.resource::<crate::application::LayoutSlots>().active(),
    ));
    crate::application::publish_status_identity(world);
    // Choosing which optional panel appears in bEd's add picker is product policy.
    world
        .resource_mut::<bedtime_workbench::WorkbenchAppearance>()
        .hidden_picker
        .insert("bedtime.duck.panel".into());
    #[cfg(target_os = "macos")]
    {
        world
            .resource_mut::<bedtime_desktop::DesktopNativePresentation>()
            .blur = world
            .resource::<bedtime_api::settings::SettingsSnapshot>()
            .get("bed.application", "mac_blur_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        world
            .resource_mut::<bedtime_desktop::DesktopNativePresentation>()
            .dock = vec![bedtime_desktop::DesktopMenuCommand {
            id: "bed.desktop.new_window".into(),
            label: "New Window".into(),
            enabled: true,
            shortcut: None,
        }];
    }
}

fn draw_app_error(world: &mut World, ui: &Ui) {
    if let Some(error) = world.resource::<ApplicationState>().error.clone() {
        ui.window("Application error")
            .flags(dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                ui.text_wrapped(&error);
                if ui.button("Dismiss") {
                    world.resource_mut::<ApplicationState>().error = None;
                }
            });
    }
}

#[cfg(target_os = "macos")]
fn observe_native_commands(
    mut commands: MessageReader<bedtime_core::NativeCommandDispatched>,
    mut observations: ResMut<NativeMenuObservations>,
    smoke: Res<NativeSmokeMode>,
) {
    if !smoke.0 {
        commands.clear();
        return;
    }
    for command in commands.read() {
        if let Some(action) = crate::native_commands::MenuAction::from_command(&command.0.command) {
            observations.0.push(crate::native_commands::MenuDispatch {
                action,
                keyboard: command.0.keyboard,
                shift: command.0.modifiers.shift,
            });
        }
    }
}

fn after_ui(world: &mut World) {
    crate::application::publish_app_state(world);
    let windows = std::mem::take(&mut world.resource_mut::<ApplicationState>().windows);
    for workspace in windows {
        if let Err(error) = spawn_instance(world, Some(&workspace)) {
            world.resource_mut::<ApplicationState>().error = Some(error.to_string());
        }
    }
}

pub(crate) fn run_shell_entrypoint(
    args: Vec<std::ffi::OsString>,
    has_socket: bool,
) -> io::Result<Option<i32>> {
    if !has_socket {
        return Ok(None);
    }
    let command = args[1]
        .to_str()
        .expect("Launch selector is a known UTF-8 command");
    bedtime_terminal::shell_bridge::invoke(&format!("+{command}"), args.into_iter().skip(2))?;
    Ok(Some(0))
}

/// bEd owns its CLI policy; feature plugins contribute private process handlers.
pub(crate) fn launcher() -> io::Result<bedtime::LaunchBuilder> {
    let application = bedtime::awaken("bed", "bEd")?;
    let application = application
        .with_launchers(bedtime_plugin_terminal::launchers())
        .with_launchers(bedtime::LaunchRegistration {
            owner: "bed.application.shell",
            entrypoints: ["open", "workspace"]
                .into_iter()
                .map(|flag| bedtime::ProcessEntrypoint {
                    selector: bedtime::ProcessSelector::FirstArgument(flag.into()),
                    run: |args| {
                        run_shell_entrypoint(args, std::env::var_os("BEDTERM_SOCKET").is_some())
                    },
                })
                .collect(),
        });
    let application = application.with_launchers(bedtime_plugin_debugger::launchers());
    Ok(application)
}

pub fn build_app(options: AppOptions, settings: AppConfig) -> io::Result<App> {
    let bedtime::LaunchOutcome::Desktop(launch) =
        launcher()?.with_args(["bed".into()]).dispatch()?
    else {
        unreachable!("Normal desktop arguments cannot select a helper")
    };
    build_app_with_launch(options, settings, *launch)
}

fn build_app_with_launch(
    mut options: AppOptions,
    settings: AppConfig,
    launch: bedtime::DesktopLaunch,
) -> io::Result<App> {
    if options.working_directory.is_none() {
        options.working_directory = Some(std::env::current_dir()?);
    }
    let desktop_options = bedtime::DesktopOptions {
        config_directory: Some(settings.config_dir.clone()),
        desktop: DesktopPlugins {
            close_when_requested: false,
            window: bevy::window::Window {
                title: "bEd".into(),
                resolution: (1200, 800).into(),
                transparent: true,
                ime_enabled: true,
                present_mode: bevy::window::PresentMode::Fifo,
                composite_alpha_mode: if cfg!(target_os = "macos") {
                    bevy::window::CompositeAlphaMode::PostMultiplied
                } else {
                    bevy::window::CompositeAlphaMode::Auto
                },
                titlebar_transparent: true,
                titlebar_show_title: false,
                fullsize_content_view: true,
                ..default()
            },
            ..default()
        },
        ..Default::default()
    };
    launch.build(desktop_options, move |app| {
        let paths = app.world().resource::<bedtime::ApplicationPaths>();
        let store = crate::application::WorkspaceStore::load(paths)?;
        crate::application::WorkspacePersistence::install(app, store);
        app.insert_non_send(settings)
            .insert_resource(crate::application::RemoteHelperDirectory(
                crate::remote_helpers::helpers_directory(),
            ));
        let resources_root = app.world().non_send::<AppConfig>().resources_root.clone();
        app.add_plugins(crate::themes::ThemePlugin::new(resources_root));
        app.add_plugins(bedtime_plugin_effects::EffectsPlugin);
        app.add_plugins(bedtime_plugin_idle::IdlePlugin);
        app.add_plugins(crate::builtins::BedBuiltinPlugins);
        crate::config::register_app_settings(app.world_mut())?;
        app.add_plugins((AppRuntimePlugin { options }, BedAppPlugin));
        crate::automation::configure(app);
        Ok(())
    })
}

pub fn run() -> HostResult<i32> {
    run_with_native(false)
}

#[cfg(feature = "test-support")]
pub fn run_native_fixture() -> HostResult<i32> {
    run_with_native(true)
}

fn run_with_native(native: bool) -> HostResult<i32> {
    let launch = match launcher()?.dispatch()? {
        bedtime::LaunchOutcome::Handled(exit) => return Ok(exit),
        bedtime::LaunchOutcome::Desktop(launch) => launch,
    };
    let Some(mut options) =
        crate::startup::Launch::parse(launch.arguments().iter().cloned(), native)?
    else {
        return Ok(0);
    };
    if options.smoke.requested() && options.config_dir.is_none() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        options.config_dir = Some(
            std::env::temp_dir().join(format!("bed-smoke-config-{}-{unique}", std::process::id())),
        );
    }
    let paths = bedtime::ApplicationPaths::new("bed", options.config_dir.clone(), None)?;
    let settings = AppConfig::with_paths(
        paths.config_dir.clone(),
        AppConfig::get_app_resources_path(),
    )?;
    let startup = options
        .startup
        .resolve(options.paths, std::env::current_dir().ok().as_deref())?;
    let app_options = startup.into_options(&paths)?;
    let mut app = build_app_with_launch(app_options, settings, *launch)?;
    #[cfg(feature = "test-support")]
    if native {
        app.add_plugins(crate::native_fixture::SmokePlugin(options.smoke));
    }
    #[cfg(feature = "test-support")]
    let smoke_failure = app
        .world()
        .get_resource::<crate::native_fixture::SmokeFailure>()
        .map(|failure| failure.0.clone());
    let exit = app.run();
    #[cfg(feature = "test-support")]
    if let Some(error) =
        smoke_failure.and_then(|failure| failure.lock().expect("smoke failure lock").clone())
    {
        return Err(io::Error::other(error).into());
    }
    if exit.is_error() {
        return Err(io::Error::other("Bevy desktop exited with an error").into());
    }
    Ok(0)
}

fn new_instance_command(
    executable: &Path,
    config_dir: &Path,
    wait: bool,
    workspace: Option<&crate::application::WorkspaceSpec>,
) -> io::Result<std::process::Command> {
    // Preferences create their directory on the first write. A fresh window
    // must still hand its selected configuration location to another process.
    let config_dir = match std::fs::canonicalize(config_dir) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::path::absolute(config_dir)?,
        Err(error) => return Err(error),
    };
    // Launch Services needs -n to start another process for a running bundle.
    // Source builds run the executable directly. Neither path inherits the
    // parent's project arguments or smoke flags.
    #[cfg(target_os = "macos")]
    let bundle = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|extension| extension == "app"));
    #[cfg(target_os = "macos")]
    let mut command = if let Some(bundle) = bundle {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-n");
        if wait {
            command.arg("-W");
        }
        command.arg(bundle).arg("--args");
        command
    } else {
        std::process::Command::new(executable)
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new(executable);
    command
        .arg("--new-window")
        .arg("--config-dir")
        .arg(config_dir)
        .stdin(std::process::Stdio::null());
    if let Some(workspace) = workspace {
        command
            .arg("--workspace")
            .arg(workspace.to_value().to_string());
    }
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    if !wait {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    Ok(command)
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_menu(
    world: &World,
    action: crate::native_commands::MenuAction,
) -> io::Result<()> {
    world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_for_smoke(&action.command_id())
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_toolbar(world: &World, command: &str) -> bool {
    world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .is_some_and(|window| window.click_command(command))
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_chrome(world: &World) -> io::Result<()> {
    let window = world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .ok_or_else(|| io::Error::other("native window is not initialized"))?;
    window.validate_control_layout()?;
    if !window.preserves_winit_content_view() {
        return Err(io::Error::other(
            "native chrome replaced Bevy's content view",
        ));
    }
    Ok(())
}

fn spawn_instance(
    world: &mut World,
    workspace: Option<&crate::application::WorkspaceSpec>,
) -> io::Result<()> {
    let config = world.non_send::<AppConfig>().config_dir.clone();
    let mut command = new_instance_command(&std::env::current_exe()?, &config, false, workspace)?;
    if world.resource::<NativeSmokeMode>().0 {
        command.arg("--smoke-test");
    }
    let child = command.spawn()?;
    if world.resource::<NativeSmokeMode>().0 {
        world.resource_mut::<NativeSpawnedInstances>().0.push(child);
        return Ok(());
    }
    drop(child);
    Ok(())
}

#[derive(Resource, Default)]
pub(crate) struct NativeSmokeMode(pub bool);

#[derive(Resource, Default)]
pub(crate) struct NativeSpawnedInstances(pub Vec<std::process::Child>);

#[cfg(target_os = "macos")]
#[derive(Resource, Default)]
pub(crate) struct NativeMenuObservations(pub Vec<crate::native_commands::MenuDispatch>);

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_key_equivalent(
    world: &World,
    characters: &str,
    key_code: u16,
    shift: bool,
) -> io::Result<()> {
    world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .key_equivalent_for_smoke(characters, key_code, shift)
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_dock_new_window(world: &World) -> io::Result<()> {
    world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_dock_for_smoke(&crate::native_commands::MenuAction::NewWindow.command_id())
}

#[cfg(feature = "test-support")]
pub(crate) fn smoke_focus_window(world: &mut World) {
    let entity = world
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .iter(world)
        .next();
    bevy::winit::WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        if let Some(window) = entity.and_then(|entity| windows.get_window(entity)) {
            window.focus_window();
        }
    });
}

#[cfg(test)]
#[path = "../tests/unit/native/host_tests.rs"]
mod tests;

#[cfg(all(target_os = "macos", feature = "test-support"))]
pub(crate) fn smoke_native_material(world: &World) -> (bool, Option<bool>, Option<f32>) {
    world
        .non_send::<bedtime_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .map_or((false, None, None), |window| {
            (
                window.material_is_visible(),
                window.content_is_opaque(),
                window.content_opacity(),
            )
        })
}

fn new_window_command(
    _invocation: In<bedtime_workbench::CommandInvocation>,
    world: &mut World,
) -> bedtime_workbench::api::CommandResult {
    spawn_instance(world, None).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}
