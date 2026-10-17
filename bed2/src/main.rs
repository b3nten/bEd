//! Host platform services and application-owned workspace policy.
mod status_demo;
mod theme_editor;
mod tool_panels;
mod viewport_demo;
mod viewport_smoke;
mod workbench_demo;
use bedtime_clipboard2::Clipboard;
use bedtime_ui2::{UiCamera, UiError, UiPlugin, UiSurface, UsesTheme, themes};
use bedtime_workbench2::*;
use bevy::{prelude::*, window::PrimaryWindow};

#[derive(Resource, Default)]
struct Exiting(bool);

fn main() -> AppExit {
    let mut app = App::new();
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a.ends_with("smoke-dir")) {
        app.insert_resource(Clipboard::memory());
    }
    app.add_plugins(DefaultPlugins.set(bevy::window::WindowPlugin {
        primary_window: Some(Window {
            title: "bEd 2 — Workspaces".into(),
            resolution: (1440, 960).into(),
            transparent: true,
            composite_alpha_mode: if cfg!(target_os = "macos") {
                bevy::window::CompositeAlphaMode::PostMultiplied
            } else {
                bevy::window::CompositeAlphaMode::Auto
            },
            titlebar_transparent: true,
            fullsize_content_view: true,
            ..default()
        }),
        close_when_requested: false,
        ..default()
    }));
    let ui_first = args.iter().any(|a| a == "--ui-first");

    if ui_first {
        app.add_plugins(UiPlugin);
    }

    if args.iter().any(|a| a == "--compose-workbench") {
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

    app.init_resource::<Exiting>()
        .add_systems(Startup, setup)
        .add_systems(
            PreUpdate,
            tool_panels::apply
                .after(bedtime_ui2::UiSystems::Draw)
                .before(WorkspaceSystems::Dispatch),
        )
        .add_systems(
            Update,
            (
                handle_close_requests.after(WorkspaceSystems::Update),
                report_errors,
            ),
        )
        .add_systems(
            PostUpdate,
            viewport_demo::extract
                .after(bedtime_ui2::UiSystems::PrepareTargets)
                .before(bevy::camera::CameraUpdateSystems),
        );
    viewport_smoke::install(&mut app);
    app.run()
}

pub(crate) fn open_workspace(host: &mut World, name: &str) -> WorkspaceId {
    let image = host
        .resource_mut::<Assets<Image>>()
        .add(bedtime_ui2::viewport_image(UVec2::new(640, 400)));
    let tools = tool_panels::PanelTools::from_host(host);
    let runtime = workbench_demo::runtime(name, image, tools);
    Workspaces::new(host).open(runtime)
}

fn setup(world: &mut World) {
    let light = world.spawn(themes::light()).id();
    let dark = world.spawn(themes::dark()).id();
    let surface = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world.entity_mut(surface).insert((
        UiSurface,
        UsesTheme(dark),
        WindowBlur(std::env::args().any(|a| a == "--blur")),
    ));
    world.spawn((
        Camera2d,
        Camera {
            order: 100,
            ..default()
        },
        UiCamera(surface),
    ));
    // Application chrome executes in the host before the workbench surface.
    bedtime_ui2::register_view(world, surface, workbench_demo::draw_controls);
    let host = attach_surface(world, surface);
    let mut gallery = bedtime_ui2::gallery::create_state(world, light, dark);
    gallery.embedded = true;
    world.insert_resource(workbench_demo::DemoWorkbench {
        host,
        surface,
        gallery,
    });
    viewport_demo::setup(world);
    let initial = Workspaces::new(world).active();
    let home = open_workspace(world, "Playground");
    open_workspace(world, "Scratch");
    Workspaces::new(world).activate(home).unwrap();
    Workspaces::new(world).close(initial, None).unwrap();
}

fn handle_close_requests(world: &mut World) {
    if !world
        .resource_mut::<Messages<CloseRequested>>()
        .drain()
        .collect::<Vec<_>>()
        .is_empty()
    {
        world.resource_mut::<Exiting>().0 = true;
        Workspaces::new(world).shutdown_work();
    }
    if world.resource::<Exiting>().0 && Workspaces::new(world).work_finished() {
        world.write_message(AppExit::Success);
    }
}

fn report_errors(
    mut ui: MessageReader<UiError>,
    mut native: MessageReader<NativeError>,
    mut workspaces: MessageReader<WorkspaceFinished>,
) {
    for error in ui.read() {
        warn!("UI: {}", error.0);
    }
    for error in native.read() {
        warn!("Native: {}", error.0);
    }
    for result in workspaces.read() {
        if let Err(error) = &result.result {
            warn!("Workspace: {error}");
        }
    }
}
