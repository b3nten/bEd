//! bEd composition and native menu policy. Core owns shared desktop mechanics.
use crate::application::{
    AppAction, AppOptions, AppRuntimePlugin, WindowCommand, WorkspaceState, capture_app_request,
};
use crate::config::Settings;
use bed_api::workbench::PanelTarget;
use bed_desktop::{
    DesktopContext, DesktopInputEvents, DesktopPlugins, DesktopRenderStatus, DesktopSystems,
    DesktopUi,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use std::{error::Error, io, path::Path};
use winit::event::WindowEvent;

type HostResult<T> = Result<T, Box<dyn Error>>;

#[derive(Resource, Default)]
struct NativeFailure(std::sync::Arc<std::sync::Mutex<Option<String>>>);

/// Native toolbar facts become commands owned by the application reducer.
#[cfg(target_os = "macos")]
#[derive(Resource, Default)]
struct ToolbarDropCursor(bevy::ecs::message::MessageCursor<bed_workbench::api::PanelDrop>);

pub struct BedAppPlugin;
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedPlatformSystems {
    Prepare,
    Input,
}

impl Plugin for BedAppPlugin {
    fn build(&self, app: &mut App) {
        bed_workbench::register(app.world_mut(), "bed.desktop", |registrar| {
            registrar.command("bed.desktop.new_window", "New Window", new_window_command);
            registrar.global_shortcut("bed.desktop.new_window", &["Primary+Shift+N"]);
        })
        .expect("desktop shortcuts are registered once");
        app.init_resource::<NativeFailure>()
            .insert_resource(DesktopUi(draw))
            .add_systems(
                PreUpdate,
                configure
                    .in_set(DesktopSystems::Configure)
                    .after(crate::themes::ThemeSystems::Configure),
            )
            .add_systems(
                PreUpdate,
                prepare
                    .after(bed_desktop::DesktopNativeSystems::Prepare)
                    .in_set(DesktopSystems::Prepare)
                    .in_set(BedPlatformSystems::Prepare),
            )
            .add_systems(PreUpdate, after_ui.in_set(DesktopSystems::AfterUi));
        #[cfg(target_os = "macos")]
        app.init_resource::<ToolbarDropCursor>();
        #[cfg(not(feature = "terminal"))]
        app.add_systems(
            PreUpdate,
            native_input
                .in_set(DesktopSystems::InterceptInput)
                .in_set(BedPlatformSystems::Input),
        );
        #[cfg(feature = "terminal")]
        app.add_systems(
            PreUpdate,
            native_input
                .after(bed_plugin_terminal::TerminalInputSystems::Intercept)
                .in_set(BedPlatformSystems::Input)
                .in_set(DesktopSystems::InterceptInput),
        );
        #[cfg(feature = "idle")]
        app.configure_sets(
            PreUpdate,
            bed_plugin_idle::IdleSystems::Intercept.before(BedPlatformSystems::Input),
        );
        #[cfg(all(feature = "idle", feature = "terminal"))]
        app.configure_sets(
            PreUpdate,
            bed_plugin_idle::IdleSystems::Intercept
                .before(bed_plugin_terminal::TerminalInputSystems::Intercept),
        );
        #[cfg(feature = "automation")]
        app.init_resource::<NativeSmokeMode>()
            .init_resource::<NativeSpawnedInstances>();
        #[cfg(all(feature = "automation", target_os = "macos"))]
        app.init_resource::<NativeMenuObservations>();
        app.configure_sets(
            PreUpdate,
            BedPlatformSystems::Prepare.before(bed_api::features::FeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            crate::application::AppSystems::Services
                .after(DesktopSystems::Configure)
                .before(bed_api::features::FeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            bed_desktop::DesktopCanvasSystems::Prepare
                .after(bed_api::features::FeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            bed_api::features::FeatureSystems::Prepare
                .after(DesktopSystems::Input)
                .before(DesktopSystems::Ui),
        );
    }
}

fn fail(world: &mut World, error: impl std::fmt::Display) {
    world
        .resource::<NativeFailure>()
        .0
        .lock()
        .expect("native failure lock")
        .get_or_insert_with(|| error.to_string());
    world.write_message(AppExit::error());
}

fn configure(world: &mut World) {
    if let Err(error) = crate::config::sync_view(world) {
        fail(world, error);
        return;
    }
    let settings = world.non_send::<Settings>();
    #[cfg(feature = "file-explorer")]
    let palette = world
        .resource::<crate::themes::ActiveTheme>()
        .file
        .runtime_palette();
    let mut hidden_picker = world
        .resource::<bed_api::features::FeatureRegistry>()
        .panels()
        .iter()
        .filter(|panel| !panel.empty_input)
        .map(|panel| panel.id.to_owned())
        .collect::<std::collections::HashSet<_>>();
    hidden_picker.insert("bed.duck.panel".into());
    let workbench_appearance = bed_workbench::WorkbenchAppearance {
        add_button: settings.bool("bed.workbench", "tab_bar_add_button", false),
        hidden_picker,
    };

    #[cfg(feature = "file-explorer")]
    let explorer = bed_plugin_file_explorer::ExplorerPresentation {
        tree: bed_plugin_file_explorer::file_tree::FileTreeStyle {
            text_color: palette.text,
            rainbow: settings.rainbow(),
            rainbow_time: 0.0,
            animations: settings.bool("bed.ui", "ui_animations", true),
        },
        finder: bed_plugin_file_explorer::file_finder::FileFinderStyle {
            background_color: palette.background,
            embedded_pane: None,
        },
    };
    *world.resource_mut::<bed_workbench::WorkbenchAppearance>() = workbench_appearance;
    #[cfg(feature = "file-explorer")]
    {
        *world.resource_mut::<bed_plugin_file_explorer::ExplorerPresentation>() = explorer;
    }
    #[cfg(target_os = "macos")]
    update_native_presentation(world);
}

fn prepare(world: &mut World) {
    if !world.contains_non_send::<DesktopContext>() {
        return;
    }
    world
        .non_send_mut::<DesktopContext>()
        .platform
        .set_ime_auto_management(false);
    let top = world.resource::<bed_desktop::DesktopContentInsets>().top;
    crate::application::set_root_top_inset(world, top);
}

fn draw(world: &mut World, ui: &Ui) {
    bed_core::begin_native_toolbar_drag(ui);
    if let Err(error) = crate::application::draw_ui(world, ui) {
        fail(world, error);
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let drops = world.resource_scope(|world, mut cursor: Mut<ToolbarDropCursor>| {
            cursor
                .0
                .read(world.resource::<Messages<bed_workbench::api::PanelDrop>>())
                .filter(|drop| drop.payload_type == "BED_TOOLBAR_COMMAND")
                .cloned()
                .collect::<Vec<_>>()
        });
        for drop in drops {
            let Ok(command) = String::from_utf8(drop.data) else {
                continue;
            };
            let mut request = crate::application::capture_app_request_from(
                world,
                AppAction::PluginCommand(command),
                PanelTarget::LargestArea,
                drop.panel,
            );
            request.area = drop.area;
            request.tab_index = drop.tab_index;
            world
                .resource_mut::<crate::application::AppRequests>()
                .enqueue(request);
        }
    }
    bed_core::finish_native_toolbar_drag(ui);
    #[cfg(feature = "idle")]
    {
        let focused = world
            .query::<&bevy::window::Window>()
            .iter(world)
            .any(|window| window.focused);
        world
            .resource_mut::<bed_plugin_idle::IdleSettings>()
            .eligible = focused.then_some(ui.main_viewport().id().raw());
        bed_plugin_idle::draw_ui(world, ui);
    }
}

fn native_input(world: &mut World) {
    let mut actions = Vec::new();
    #[cfg(target_os = "macos")]
    let mut edits = Vec::new();
    #[cfg(target_os = "macos")]
    let mut new_windows = 0;
    #[cfg(all(feature = "automation", target_os = "macos"))]
    let mut observations = Vec::new();
    {
        let mut events = world.resource_mut::<DesktopInputEvents>();
        for event in &mut events.0 {
            if event.consumed {
                continue;
            }
            if matches!(event.event.event, WindowEvent::CloseRequested) {
                event.consumed = true;
                actions.push(AppAction::Command(WindowCommand::Quit));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let dispatches =
            std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeActions>().0);
        for dispatch in dispatches {
            use crate::native_commands::MenuAction;
            if dispatch.keyboard {
                bed_workbench::resolve_keymap(world);
                let bindings = world
                    .non_send::<bed_workbench::ResolvedKeymap>()
                    .bindings
                    .clone();
                let stroke = if let Some(action) = MenuAction::from_command(&dispatch.command) {
                    crate::native_commands::menu_shortcut(
                        action,
                        &bindings,
                        editor_shortcuts_focused(world),
                        world
                            .non_send::<DesktopContext>()
                            .context
                            .io()
                            .want_text_input(),
                    )
                } else {
                    bindings
                        .for_action(&dispatch.command)
                        .into_iter()
                        .find(|binding| {
                            (binding.scope == bed_keybindings::Scope::Global
                                || (editor_shortcuts_focused(world)
                                    && binding.scope
                                        == bed_keybindings::Scope::panel(
                                            bed_keybindings::EDITOR_SCOPE,
                                        )))
                                && bindings
                                    .winner(&binding.scope, binding.shortcut)
                                    .is_some_and(|winner| winner.action == binding.action)
                        })
                        .map(|binding| binding.shortcut)
                };
                if let Some(stroke) = stroke {
                    #[cfg(feature = "automation")]
                    if let Some(action) = MenuAction::from_command(&dispatch.command) {
                        observations.push(crate::native_commands::MenuDispatch {
                            action,
                            keyboard: true,
                            shift: dispatch.modifiers.shift,
                        });
                    }
                    bed_core::queue_native_shortcut(
                        &mut world.non_send_mut::<DesktopContext>().context,
                        stroke,
                    );
                    continue;
                }
            }
            let Some(action) = MenuAction::from_command(&dispatch.command) else {
                actions.push(AppAction::PluginCommand(dispatch.command));
                continue;
            };
            #[cfg(feature = "automation")]
            observations.push(crate::native_commands::MenuDispatch {
                action,
                keyboard: dispatch.keyboard,
                shift: dispatch.modifiers.shift,
            });
            if let Some(command) = native_menu_command(action, dispatch.keyboard) {
                actions.push(AppAction::Command(command));
            } else if action == MenuAction::NewWindow {
                new_windows += 1;
            } else if action == MenuAction::Quit {
                actions.push(AppAction::Command(WindowCommand::Quit));
            } else if let Some(edit) = bed_core::NativeEditAction::from_command(&dispatch.command) {
                edits.push(edit);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        #[cfg(feature = "automation")]
        world
            .resource_mut::<NativeMenuObservations>()
            .0
            .extend(observations);
        for action in edits {
            bed_core::route_native_edit(world, action);
        }
        for _ in 0..new_windows {
            if let Err(error) = spawn_instance(world, None) {
                world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
            }
        }
    }
    for action in actions {
        let request = capture_app_request(world, action, PanelTarget::LargestArea);
        world
            .resource_mut::<crate::application::AppRequests>()
            .enqueue(request);
    }
}

#[cfg(feature = "editor")]
fn editor_shortcuts_focused(world: &World) -> bool {
    world
        .resource::<bed_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| crate::application::panel_presentation(world, panel))
        .and_then(|panel| world.get::<bed_workbench::PanelRecord>(panel))
        .is_some_and(|record| {
            record.panel_type == bed_plugin_editor::TEXT_PANEL_TYPE
                || record.panel_type == bed_plugin_editor::HEX_PANEL_TYPE
        })
}

#[cfg(not(feature = "editor"))]
fn editor_shortcuts_focused(_world: &World) -> bool {
    false
}

fn after_ui(world: &mut World) {
    if let Err(error) = crate::application::process_app_requests(world) {
        world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
    }
    let errors = std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopCanvasErrors>().0);
    for error in errors {
        world.resource_mut::<WorkspaceState>().error = Some(error.message);
    }
    if let Some(error) = world.resource::<DesktopRenderStatus>().take_error() {
        fail(world, error);
    }
    if world.resource::<WorkspaceState>().close_requested {
        world.write_message(AppExit::Success);
    }
    let appearance_errors =
        std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopThemeErrors>().0);
    for error in appearance_errors {
        fail(world, error);
    }
    let native_errors =
        std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeErrors>().0);
    for error in native_errors {
        fail(world, error);
    }
    let windows = std::mem::take(&mut world.resource_mut::<WorkspaceState>().windows);
    for workspace in windows {
        if let Err(error) = spawn_instance(world, Some(&workspace)) {
            world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
        }
    }
}

#[cfg(target_os = "macos")]
fn update_native_presentation(world: &mut World) {
    let toolbar = crate::application::toolbar_commands(world);
    let application = crate::application::application_commands(world);
    let available = world.resource::<bed_api::files::FileContext>().availability
        == bed_api::files::FileAvailability::Available;
    let has_document = world
        .resource::<crate::application::DocumentNavigation>()
        .active_document
        .is_some();
    let clipboard_focused = world
        .resource::<crate::application::WorkbenchLayout>()
        .focused
        .and_then(|panel| crate::application::panel_presentation(world, panel))
        .is_some_and(|panel| {
            world
                .get::<bed_api::workbench::PanelClipboard>(panel)
                .is_some()
                && world
                    .get::<bed_api::workbench::PanelActions>(panel)
                    .is_none()
        });
    let text_input_focused = world
        .non_send::<DesktopContext>()
        .context
        .io()
        .want_text_input();
    bed_workbench::resolve_keymap(world);
    let bindings = world
        .non_send::<bed_workbench::ResolvedKeymap>()
        .bindings
        .clone();
    let menus = crate::native_commands::menus(
        &bindings,
        &application,
        crate::native_commands::MenuContext {
            directory_available: available,
            document: has_document,
            clipboard: clipboard_focused,
            text_input: text_input_focused,
            editor_focused: editor_shortcuts_focused(world),
            active_layout: world.resource::<crate::application::LayoutSlots>().active(),
        },
    );
    let settings = world.non_send::<Settings>();
    let palette = world
        .resource::<crate::themes::ActiveTheme>()
        .file
        .runtime_palette();
    let presentation = bed_desktop::DesktopNativePresentation {
        opacity: settings.background_opacity(),
        blur: settings.bool("bed.application", "mac_blur_enabled", true),
        foreground: palette.text,
        background: palette.background,
        menus,
        toolbar: toolbar
            .into_iter()
            .map(|item| bed_desktop::DesktopToolbarItem {
                id: item.id,
                label: item.label,
                icon: item.icon,
                enabled: item.enabled,
            })
            .collect(),
        dock: vec![bed_desktop::DesktopMenuCommand {
            id: crate::native_commands::MenuAction::NewWindow.command_id(),
            label: "New Window".into(),
            enabled: true,
            shortcut: None,
        }],
    };
    *world.resource_mut::<bed_desktop::DesktopNativePresentation>() = presentation;
}

#[cfg(feature = "terminal")]
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
    bed_terminal::shell_bridge::invoke(&format!("+{command}"), args.into_iter().skip(2))?;
    Ok(Some(0))
}

/// bEd owns its CLI policy; feature plugins contribute private process handlers.
pub(crate) fn launcher() -> io::Result<bed_launch::LaunchBuilder> {
    let application = bed_launch::builder("bed", "bEd")?;
    #[cfg(feature = "terminal")]
    let application = application
        .with_launchers(bed_plugin_terminal::launchers())
        .with_launchers(bed_launch::LaunchRegistration {
            owner: "bed.application.shell",
            entrypoints: ["open", "workspace"]
                .into_iter()
                .map(|flag| bed_launch::ProcessEntrypoint {
                    selector: bed_launch::ProcessSelector::FirstArgument(flag.into()),
                    run: |args| {
                        run_shell_entrypoint(args, std::env::var_os("BEDTERM_SOCKET").is_some())
                    },
                })
                .collect(),
        });
    #[cfg(feature = "debugger")]
    let application = application.with_launchers(bed_plugin_debugger::launchers());
    Ok(application)
}

pub fn build_app(options: AppOptions, settings: Settings) -> io::Result<App> {
    let bed_launch::LaunchOutcome::Desktop(launch) =
        launcher()?.with_args(["bed".into()]).dispatch()?
    else {
        unreachable!("Normal desktop arguments cannot select a helper")
    };
    build_app_with_launch(options, settings, *launch)
}

fn build_app_with_launch(
    mut options: AppOptions,
    settings: Settings,
    launch: bed_launch::DesktopLaunch,
) -> io::Result<App> {
    if options.working_directory.is_none() {
        options.working_directory = Some(std::env::current_dir()?);
    }
    let desktop_options = bed_launch::DesktopOptions {
        config_directory: Some(settings.config_dir.clone()),
        presentation_mode: bed_core::CorePresentationMode::Application,
        desktop: DesktopPlugins {
            close_when_requested: false,
            window: bevy::window::Window {
                title: "bEd".into(),
                resolution: (1200, 800).into(),
                transparent: true,
                ime_enabled: true,
                present_mode: bevy::window::PresentMode::AutoNoVsync,
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
        let paths = app.world().resource::<bed_launch::ApplicationPaths>();
        let store = if cfg!(feature = "workspaces") {
            crate::application::WorkspaceStore::load(paths)?
        } else {
            crate::application::WorkspaceStore::empty(paths)
        };
        crate::application::WorkspacePersistence::install(app, store);
        app.insert_non_send(settings)
            .insert_resource(crate::application::RemoteHelperDirectory(
                crate::remote_helpers::helpers_directory(),
            ));
        let resources_root = app.world().non_send::<Settings>().resources_root.clone();
        app.add_plugins(crate::themes::ThemePlugin::new(resources_root));
        #[cfg(feature = "effects")]
        app.add_plugins(bed_plugin_effects::EffectsPlugin);
        #[cfg(feature = "idle")]
        app.add_plugins(bed_plugin_idle::IdlePlugin);
        app.add_plugins(crate::builtins::BedBuiltinPlugins);
        crate::config::register_app_settings(app.world_mut())?;
        crate::config::sync_view(app.world_mut())?;
        app.add_plugins((AppRuntimePlugin { options }, BedAppPlugin));
        #[cfg(feature = "automation")]
        {
            app.add_plugins(bed_plugin_automation::AutomationPlugin);
            crate::automation::configure(app);
        }
        Ok(())
    })
}

pub fn run() -> HostResult<i32> {
    let launch = match launcher()?.dispatch()? {
        bed_launch::LaunchOutcome::Handled(exit) => return Ok(exit),
        bed_launch::LaunchOutcome::Desktop(launch) => launch,
    };
    let Some(mut options) = crate::startup::Launch::parse(launch.arguments().iter().cloned())?
    else {
        return Ok(0);
    };
    if options.smoke.requested() && !cfg!(feature = "automation") {
        return Err(io::Error::other(
            "smoke and frame capture commands require building with --features automation",
        )
        .into());
    }
    if options.smoke.requested() && options.config_dir.is_none() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        options.config_dir = Some(
            std::env::temp_dir().join(format!("bed-smoke-config-{}-{unique}", std::process::id())),
        );
    }
    let paths = bed_launch::ApplicationPaths::new("bed", options.config_dir.clone(), None)?;
    let settings =
        Settings::with_paths(paths.config_dir.clone(), Settings::get_app_resources_path())?;
    let startup = options
        .startup
        .resolve(options.paths, std::env::current_dir().ok().as_deref())?;
    let app_options = startup.into_options(&paths)?;
    let mut app = build_app_with_launch(app_options, settings, *launch)?;
    #[cfg(feature = "automation")]
    app.add_plugins(crate::smoke::SmokePlugin(options.smoke));
    let native_failure = app.world().resource::<NativeFailure>().0.clone();
    #[cfg(feature = "automation")]
    let smoke_failure = app
        .world()
        .get_resource::<crate::smoke::SmokeFailure>()
        .map(|failure| failure.0.clone());
    let exit = app.run();
    if let Some(error) = native_failure.lock().expect("native failure lock").clone() {
        return Err(io::Error::other(error).into());
    }
    #[cfg(feature = "automation")]
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

#[cfg(target_os = "macos")]
fn native_menu_command(
    action: crate::native_commands::MenuAction,
    keyboard: bool,
) -> Option<WindowCommand> {
    use crate::native_commands::MenuAction;
    Some(match action {
        MenuAction::NewDocument => WindowCommand::NewDocument,
        MenuAction::NewTerminal => WindowCommand::NewTerminal,
        MenuAction::NewExplorer => WindowCommand::NewExplorer,
        MenuAction::NewSettings => WindowCommand::NewSettings,
        MenuAction::NewWorkspaces | MenuAction::Workspaces => WindowCommand::NewWorkspaces,
        MenuAction::NewDiagnostics | MenuAction::Diagnostics => WindowCommand::NewDiagnostics,
        MenuAction::Debug => WindowCommand::Debug,
        MenuAction::Structure | MenuAction::NewStructure => WindowCommand::NewStructure,
        MenuAction::NewReferences => WindowCommand::NewReferences,
        MenuAction::NewLspDashboard | MenuAction::LspDashboard => WindowCommand::NewLspDashboard,
        MenuAction::NewContentSearch => WindowCommand::NewContentSearch,
        MenuAction::DuplicateView => WindowCommand::DuplicateView,
        MenuAction::SplitRight => WindowCommand::SplitRight,
        MenuAction::SplitDown => WindowCommand::SplitDown,
        MenuAction::UseLayout(slot) => WindowCommand::UseLayout(slot),
        MenuAction::SetLayoutDefault(slot) => WindowCommand::SetLayoutDefault(slot),
        MenuAction::RestoreBuiltinLayout(slot) => WindowCommand::RestoreBuiltinLayout(slot),
        MenuAction::SetCurrentLayout => WindowCommand::SetCurrentLayout,
        MenuAction::ResetLayout => WindowCommand::ResetLayout,
        MenuAction::SaveDefaultLayout => WindowCommand::SaveDefaultLayout,
        MenuAction::ResetDefaultLayout => WindowCommand::ResetDefaultLayout,
        MenuAction::OpenFolder => WindowCommand::OpenFolder,
        MenuAction::OpenFile => WindowCommand::OpenFile,
        MenuAction::Save => WindowCommand::Save,
        MenuAction::SaveAs => WindowCommand::SaveAs,
        MenuAction::Close => WindowCommand::Close,
        MenuAction::Find => WindowCommand::Find,
        MenuAction::GoToLine => WindowCommand::GoToLine,
        MenuAction::FindFile => WindowCommand::FindFile,
        MenuAction::FindWorkspace if keyboard => WindowCommand::FindWorkspace,
        MenuAction::FindWorkspace => WindowCommand::NewContentSearch,
        MenuAction::Explorer if keyboard => WindowCommand::Explorer,
        MenuAction::Explorer => WindowCommand::NewExplorer,
        MenuAction::Terminal if keyboard => WindowCommand::Terminal,
        MenuAction::Terminal => WindowCommand::NewTerminal,
        MenuAction::Settings if keyboard => WindowCommand::Settings,
        MenuAction::Settings => WindowCommand::NewSettings,
        _ => return None,
    })
}

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_menu(
    world: &World,
    action: crate::native_commands::MenuAction,
) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_for_smoke(&action.command_id())
}

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_toolbar(world: &World, command: &str) -> bool {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .is_some_and(|window| window.click_command(command))
}

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_chrome(world: &World) -> io::Result<()> {
    let window = world
        .non_send::<bed_desktop::DesktopNativePlatform>()
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
    let config = world.non_send::<Settings>().config_dir.clone();
    let mut command = new_instance_command(&std::env::current_exe()?, &config, false, workspace)?;
    #[cfg(feature = "automation")]
    if world.resource::<NativeSmokeMode>().0 {
        command.arg("--smoke-test");
    }
    let child = command.spawn()?;
    #[cfg(feature = "automation")]
    if world.resource::<NativeSmokeMode>().0 {
        world.resource_mut::<NativeSpawnedInstances>().0.push(child);
        return Ok(());
    }
    drop(child);
    Ok(())
}

#[cfg(feature = "automation")]
#[derive(Resource, Default)]
pub(crate) struct NativeSmokeMode(pub bool);

#[cfg(feature = "automation")]
#[derive(Resource, Default)]
pub(crate) struct NativeSpawnedInstances(pub Vec<std::process::Child>);

#[cfg(all(feature = "automation", target_os = "macos"))]
#[derive(Resource, Default)]
pub(crate) struct NativeMenuObservations(pub Vec<crate::native_commands::MenuDispatch>);

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_key_equivalent(
    world: &World,
    characters: &str,
    key_code: u16,
    shift: bool,
) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .key_equivalent_for_smoke(characters, key_code, shift)
}

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_dock_new_window(world: &World) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_dock_for_smoke(&crate::native_commands::MenuAction::NewWindow.command_id())
}

#[cfg(feature = "automation")]
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

#[cfg(all(feature = "automation", feature = "editor", target_os = "macos"))]
pub(crate) fn smoke_native_material(world: &World) -> (bool, Option<bool>, Option<f32>) {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
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
    _invocation: In<bed_workbench::CommandInvocation>,
    world: &mut World,
) -> bed_workbench::api::CommandResult {
    spawn_instance(world, None).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}
