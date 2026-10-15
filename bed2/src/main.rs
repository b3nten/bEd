//! Application-owned Bevy setup, ECS menus and typed workbench commands.
mod viewport_demo;
mod viewport_smoke;
use bedtime_desktop2::{
    Accelerator, ActionEnabled, Activates, Code, DesktopAction, DesktopClipboard, DesktopError,
    DesktopPlugin, InMenu, Menu, MenuBar, MenuItem, MenuSeparator, Modifiers,
};
use bedtime_ui2::{UiError, UiGraph, UiPlugin, UiSurface, UiSystems, UsesTheme, gallery, themes};
use bedtime_workbench2::{
    CommandDefinition, CommandFinished, CommandInvocation, CommandRequests, CommandResult,
    CommandSystems, Workbench, WorkbenchPlugin,
};
use bevy::prelude::*;
use bevy::{render::camera::CameraRenderGraph, window::PrimaryWindow};

struct CopyGreeting;
struct OpenViewport;
impl CommandDefinition for OpenViewport {
    const KEY: &'static str = "bed2.open_viewport";
    const LABEL: &'static str = "Open 3D viewport";
}
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
    app.add_plugins(DefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: Some(Window {
            title: "bEd 2 — Widget gallery".into(),
            resolution: (1160, 900).into(),
            transparent: true,
            titlebar_transparent: true,
            ..default()
        }),
        close_when_requested: false,
        ..default()
    }))
    .add_plugins((
        DesktopPlugin,
        UiPlugin,
        WorkbenchPlugin,
        bed2_demo_game::CubeGamePlugin,
    ))
    .configure_sets(PostUpdate, UiSystems::Draw.before(CommandSystems::Ingress))
    .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
    .add_systems(Startup, setup.after(bed2_demo_game::GameSetup))
    .add_systems(
        Update,
        (handle_close_requests, keyboard_quit, report_errors),
    );
    viewport_demo::install(&mut app);
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
    let gallery = gallery::spawn(world, surface, light, dark);
    viewport_demo::spawn(world, surface);
    let open = Workbench::new(world)
        .register_command::<OpenViewport, _>(open_viewport)
        .unwrap();
    world.spawn((
        gallery::GalleryAction {
            label: "Open 3D viewport".into(),
        },
        bedtime_ui2::InView(gallery),
        Activates(open),
    ));

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

    world.spawn((
        gallery::GalleryAction {
            label: "Copy greeting".into(),
        },
        bedtime_ui2::InView(gallery),
        Activates(copy),
    ));
    world.spawn((
        gallery::GalleryAction {
            label: "Toggle copy availability".into(),
        },
        bedtime_ui2::InView(gallery),
        Activates(toggle),
    ));

    let primary = if cfg!(target_os = "macos") {
        Modifiers::META
    } else {
        Modifiers::CONTROL
    };

    let bar = world.spawn(MenuBar).id();

    let application = world
        .spawn((
            Menu {
                label: "bEd 2".into(),
            },
            InMenu(bar),
        ))
        .id();

    world.spawn((
        MenuItem {
            shortcut: Some(Accelerator::new(primary | Modifiers::SHIFT, Code::KeyC)),
            ..MenuItem::new(CopyGreeting::LABEL)
        },
        InMenu(application),
        Activates(copy),
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
        Activates(toggle),
    ));
    world.spawn((MenuSeparator, InMenu(application)));
    world.spawn((
        MenuItem {
            shortcut: Some(Accelerator::new(primary, Code::KeyQ)),
            ..MenuItem::new(QuitApplication::LABEL)
        },
        InMenu(application),
        Activates(quit),
    ));
    info!("bEd 2: native ECS menus on macOS; Escape requests quit on either platform.");
}

fn open_viewport(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    viewport_demo::show(world);
    Ok(())
}

fn copy_greeting(
    In(_invocation): In<CommandInvocation>,
    mut clipboard: Option<NonSendMut<DesktopClipboard>>,
) -> CommandResult {
    clipboard
        .as_mut()
        .ok_or("The native clipboard is not ready")?
        .write_text("Hello from bEd 2")
        .map_err(|error| error.to_string())?;
    info!("Copied greeting");
    Ok(())
}

fn toggle_greeting(In(_invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let copy = Workbench::new(world)
        .command::<CopyGreeting>()
        .ok_or("Copy Greeting is unavailable")?;
    let mut enabled = world
        .get_mut::<ActionEnabled>(copy)
        .expect("registered commands have availability");
    enabled.0 = !enabled.0;
    Ok(())
}

fn request_quit(
    In(_invocation): In<CommandInvocation>,
    mut actions: MessageWriter<DesktopAction>,
) -> CommandResult {
    actions.write(DesktopAction::CloseRequested { window: None });
    Ok(())
}

fn handle_close_requests(
    mut actions: MessageReader<DesktopAction>,
    mut exit: MessageWriter<AppExit>,
) {
    for action in actions.read() {
        if let DesktopAction::CloseRequested { window } = action {
            info!("Application accepted close request: {window:?}");
            // Workbench document validation will replace this demonstration policy.
            exit.write(AppExit::Success);
        }
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
        world.resource_mut::<CommandRequests>().invoke(quit);
    }
}

fn report_errors(
    mut ui: MessageReader<UiError>,
    mut desktop: MessageReader<DesktopError>,
    mut commands: MessageReader<CommandFinished>,
) {
    for error in ui.read() {
        warn!("UI: {}", error.0);
    }
    for error in desktop.read() {
        warn!("Desktop: {}", error.0);
    }
    for command in commands.read() {
        if let Err(error) = &command.result {
            warn!("Command {:?}: {error}", command.invocation.command);
        }
    }
}
