//! Application-owned Bevy setup, ECS menus and typed workbench commands.
mod panel_smoke;
mod viewport_demo;
mod viewport_smoke;
mod workbench_demo;
use bedtime_clipboard2::Clipboard;
use bedtime_ui2::{UiError, UiGraph, UiPlugin, UiSurface, UsesTheme, themes};
use bedtime_workbench2::*;
use bevy::prelude::*;
use bevy::{render::camera::CameraRenderGraph, window::PrimaryWindow};

struct OpenViewport;
impl CommandDefinition for OpenViewport {
    const KEY: &'static str = "bed2.open_viewport";
    const LABEL: &'static str = "Open 3D viewport";
}

struct CopyGreeting;
impl CommandDefinition for CopyGreeting {
    const KEY: &'static str = "bed2.copy_greeting";
    const LABEL: &'static str = "Copy Greeting";
}

struct ToggleGreeting;
impl CommandDefinition for ToggleGreeting {
    const KEY: &'static str = "bed2.toggle_greeting";
    const LABEL: &'static str = "Toggle Copy Greeting";
}

struct QuitApplication;
impl CommandDefinition for QuitApplication {
    const KEY: &'static str = "bed2.quit";
    const LABEL: &'static str = "Quit bEd 2";
}

fn main() -> AppExit {
    let mut app = App::new();

    let args: Vec<_> = std::env::args().collect();
    let ui_first = args.iter().any(|a| a == "--ui-first");
    let compose = args.iter().any(|a| a == "--compose-workbench");
    if args
        .iter()
        .any(|a| a == "--workbench-smoke-dir" || a == "--viewport-smoke-dir")
    {
        // Native smoke must not replace the user's system clipboard.
        app.insert_resource(Clipboard::memory());
    }
    app.add_plugins(DefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: Some(Window {
            title: "bEd 2 — Workbench".into(),
            resolution: (1440, 960).into(),
            transparent: true,
            titlebar_transparent: true,
            ..default()
        }),
        close_when_requested: false,
        ..default()
    }));
    if ui_first {
        app.add_plugins(UiPlugin);
    }
    if compose {
        app.add_plugins((
            WorkbenchCorePlugin,
            ClipboardPlugin,
            WorkbenchNativeWindowPlugin,
            WorkbenchUiPlugin,
        ));
    } else {
        app.add_plugins(WorkbenchPlugin);
    }
    if !ui_first {
        app.add_plugins(UiPlugin);
    }
    app.add_plugins(bed2_demo_game::CubeGamePlugin)
        .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
        .add_systems(Startup, setup.after(bed2_demo_game::GameSetup))
        .add_systems(
            Update,
            (handle_close_requests, keyboard_quit, report_errors),
        );

    viewport_demo::install(&mut app);
    workbench_demo::install(&mut app);
    viewport_smoke::install(&mut app);

    app.run()
}

fn setup(world: &mut World) {
    world.spawn((
        Camera2d,
        Camera {
            order: 100,
            ..default()
        },
        CameraRenderGraph::new(UiGraph),
        viewport_demo::EditorUiCamera,
    ));

    let light = world.spawn(themes::light()).id();
    let dark = world.spawn(themes::dark()).id();

    let surface = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();

    world
        .entity_mut(surface)
        .insert((UiSurface, UsesTheme(dark)));

    workbench_demo::setup(world, surface, light, dark);

    Workbench::new(world)
        .register_command::<OpenViewport, _>(open_viewport)
        .unwrap();
    let (copy, toggle, quit) = {
        let mut workbench = Workbench::new(world);
        (
            workbench
                .register_command::<CopyGreeting, _>(copy_greeting)
                .expect("unique application command"),
            workbench
                .register_command::<ToggleGreeting, _>(toggle_greeting)
                .expect("unique application command"),
            workbench
                .register_command::<QuitApplication, _>(request_quit)
                .expect("unique application command"),
        )
    };

    world.spawn(KeyBinding::new::<CopyGreeting>(KeyChord::new(
        Modifiers::PRIMARY | Modifiers::SHIFT,
        KeyCode::KeyC,
    )));
    world.spawn(KeyBinding::new::<QuitApplication>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyQ,
    )));
    let bar = world.spawn((MenuBar, NativeMenuBar)).id();

    let application = world
        .spawn((
            Menu {
                label: "bEd 2".into(),
            },
            InMenu(bar),
        ))
        .id();

    world.spawn((
        MenuItem::new(CopyGreeting::LABEL),
        InMenu(application),
        Invokes(copy),
    ));

    let tools = world
        .spawn((
            Menu {
                label: "Tools".into(),
            },
            InMenu(application),
        ))
        .id();

    world.spawn((
        MenuItem::new(ToggleGreeting::LABEL),
        InMenu(tools),
        Invokes(toggle),
    ));

    world.spawn((
        MenuItem::new(OpenViewport::LABEL),
        InMenu(tools),
        Invokes::of::<OpenViewport>(),
    ));
    world.spawn((MenuSeparator, InMenu(application)));

    world.spawn((
        MenuItem::new(QuitApplication::LABEL),
        InMenu(application),
        Invokes(quit),
    ));

    info!("bEd 2: workbench native menus on macOS; Escape requests quit on either platform.");
}

fn open_viewport(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    viewport_demo::show(world);
    Ok(())
}

fn copy_greeting(
    In(_invocation): In<CommandInvocation>,
    mut clipboard: ResMut<Clipboard>,
) -> CommandResult {
    clipboard
        .write_text("Hello from bEd 2")
        .map_err(|error| error.to_string())?;
    info!("Copied greeting");
    Ok(())
}

fn toggle_greeting(In(_invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let copy = Workbench::new(world)
        .command::<CopyGreeting>()
        .ok_or("Copy Greeting is unavailable")?;
    let handler = Workbench::new(world)
        .handler(copy, None)
        .expect("Registered global handler");
    let mut enabled = world
        .get_mut::<ActionEnabled>(handler)
        .expect("registered commands have availability");
    enabled.0 = !enabled.0;
    Ok(())
}

fn request_quit(
    In(_invocation): In<CommandInvocation>,
    mut actions: MessageWriter<CloseRequested>,
) -> CommandResult {
    actions.write(CloseRequested { window: None });
    Ok(())
}

fn handle_close_requests(
    mut actions: MessageReader<CloseRequested>,
    mut exit: MessageWriter<AppExit>,
) {
    for action in actions.read() {
        info!("Application accepted close request: {:?}", action.window);
        // Workbench document validation will replace this demonstration policy.
        exit.write(AppExit::Success);
    }
}

fn keyboard_quit(world: &mut World) {
    if world
        .resource::<ButtonInput<KeyCode>>()
        .just_pressed(KeyCode::Escape)
        && !world
            .query::<&bedtime_ui2::UiCapture>()
            .iter(world)
            .any(|capture| capture.keyboard || capture.text)
    {
        let quit = Workbench::new(world)
            .command::<QuitApplication>()
            .expect("application registered quit");
        world.resource_mut::<CommandRequests>().invoke_type(quit);
    }
}

fn report_errors(
    mut ui: MessageReader<UiError>,
    mut native: MessageReader<NativeError>,
    mut commands: MessageReader<CommandFinished>,
) {
    for error in ui.read() {
        warn!("UI: {}", error.0);
    }
    for error in native.read() {
        warn!("Native: {}", error.0);
    }
    for command in commands.read() {
        if let Err(error) = &command.result {
            warn!("Command {:?}: {error}", command.invocation.command);
        }
    }
}
