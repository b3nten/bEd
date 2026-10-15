use bedtime_desktop2::{DesktopAction, DesktopPlugin};
use bedtime_ui2::{gallery, *};
use bevy::{prelude::*, render::camera::CameraRenderGraph, window::PrimaryWindow};

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bedtime UI 2 — Widget gallery".into(),
                resolution: (1160, 900).into(),
                transparent: true,
                titlebar_transparent: true,
                ..default()
            }),
            close_when_requested: false,
            ..default()
        }))
        .add_plugins((DesktopPlugin, UiPlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, (close, smoke))
        .insert_resource(Smoke::from_args())
        .run()
}

// Example-only native verification. Captures use Bevy's actual window output.
#[derive(Resource, Default)]
struct Smoke {
    directory: Option<std::path::PathBuf>,
    frame: u32,
    captured: u32,
    stable_rasters: u64,
}

impl Smoke {
    fn from_args() -> Self {
        let mut args = std::env::args();
        let directory = args
            .find(|a| a == "--smoke-dir")
            .map(|_| std::path::PathBuf::from(args.next().expect("--smoke-dir needs a directory")));
        if let Some(path) = &directory {
            std::fs::create_dir_all(path).unwrap();
        }
        Self {
            directory,
            ..default()
        }
    }
}

fn smoke(world: &mut World) {
    use bevy::{
        render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
        winit::{RawWinitWindowEvent, WINIT_WINDOWS},
    };
    use winit::{
        dpi::PhysicalPosition,
        event::{DeviceId, ElementState, MouseButton, WindowEvent},
    };
    let (frame, directory, captured) = {
        let mut smoke = world.resource_mut::<Smoke>();
        let Some(directory) = smoke.directory.clone() else {
            return;
        };
        smoke.frame += 1;
        (smoke.frame, directory, smoke.captured)
    };
    let surface = world
        .query_filtered::<Entity, With<UiSurface>>()
        .single(world)
        .unwrap();
    let pointer = match frame {
        70..=74 => Some([33.0, 115.0]),
        145..=148 => Some([400.0, 462.0]),
        170..=173 => Some([400.0, 514.0]),
        185..=188 => Some([80.0, 698.0]),
        220..=223 => Some([1100.0, 50.0]),
        _ => None,
    };
    let button = match frame {
        72 | 146 | 171 | 221 => Some((MouseButton::Left, ElementState::Pressed)),
        74 | 148 | 173 | 223 => Some((MouseButton::Left, ElementState::Released)),
        186 => Some((MouseButton::Right, ElementState::Pressed)),
        188 => Some((MouseButton::Right, ElementState::Released)),
        _ => None,
    };
    if pointer.is_some() || button.is_some() {
        // Feed native-shaped events through the real platform adapter. No OS
        // pointer movement or interaction with other applications is involved.
        WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            let Some(window) = windows.get_window(surface) else {
                return;
            };
            if let Some([x, y]) = pointer {
                world.write_message(RawWinitWindowEvent {
                    window_id: window.id(),
                    event: WindowEvent::CursorMoved {
                        device_id: DeviceId::dummy(),
                        position: PhysicalPosition::new(
                            x * window.scale_factor(),
                            y * window.scale_factor(),
                        ),
                    },
                });
            }
            if let Some((button, state)) = button {
                world.write_message(RawWinitWindowEvent {
                    window_id: window.id(),
                    event: WindowEvent::MouseInput {
                        device_id: DeviceId::dummy(),
                        state,
                        button,
                    },
                });
            }
        });
    }
    if frame == 130 {
        for mut state in world.query::<&mut gallery::GalleryState>().iter_mut(world) {
            state.toggled = true;
            state.open = true;
        }
    }
    if frame == 230 {
        world
            .get_mut::<Window>(surface)
            .unwrap()
            .resolution
            .set(1000.0, 820.0);
    }
    if frame == 132 {
        assert!(
            world
                .get::<UiDiagnostics>(surface)
                .unwrap()
                .active_animations
                >= 2,
            "Toggle and disclosure must be mid-animation"
        );
    }
    if frame == 73 {
        assert!(
            world
                .get::<UiDiagnostics>(surface)
                .unwrap()
                .active_animations
                > 0,
            "Tooltip must be mid-animation while hovered"
        );
    }
    if frame == 180 {
        assert_eq!(
            world
                .query::<&gallery::GalleryState>()
                .single(world)
                .unwrap()
                .choice,
            1,
            "Native pointer input must select Compact from the spaced dropdown"
        );
    }
    if frame == 310 {
        let mut state = world
            .query::<&mut gallery::GalleryState>()
            .single_mut(world)
            .unwrap();
        state.icon_browser = true;
        state.icon_size = 40.0;
    }
    if frame == 330 {
        let pack = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap()
            .custom_icons
            .clone();
        world.entity_mut(surface).insert(UiIcons(pack));
    }
    if frame == 336 {
        let state = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap();
        let status = world.get::<UiIconStatus>(surface).unwrap();
        assert_eq!(status.active.as_ref(), Some(&state.custom_icons));
        assert!(!status.pending && status.error.is_none());
    }
    if frame == 350 {
        let icon = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap()
            .custom_icon
            .clone();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m12 1 3 7 8 1-6 5 2 9-7-4-7 4 2-9-6-5 8-1Z"/></svg>"#;
        world
            .resource_mut::<Assets<Icon>>()
            .insert(icon.id(), Icon::from_svg(svg).unwrap())
            .unwrap();
    }
    if frame == 370 {
        world.entity_mut(surface).remove::<UiIcons>();
    }
    if frame == 395 {
        world
            .query::<&mut gallery::GalleryState>()
            .single_mut(world)
            .unwrap()
            .icon_browser = false;
    }
    if frame == 400 {
        world.resource_mut::<Smoke>().stable_rasters = world
            .get::<UiDiagnostics>(surface)
            .unwrap()
            .icon_rasterizations;
    }
    let name = match frame {
        35 => Some("dark.png"),
        71 => Some("tooltip.png"),
        105 => Some("light.png"),
        132 => Some("motion.png"),
        165 => Some("dropdown.png"),
        215 => Some("menu.png"),
        275 => Some("resized.png"),
        325 => Some("icons-bundled.png"),
        345 => Some("icons-custom.png"),
        365 => Some("icons-updated.png"),
        385 => Some("icons-restored.png"),
        _ => None,
    };
    if let Some(name) = name {
        world
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(directory.join(name)))
            .observe(|_: On<ScreenshotCaptured>, mut smoke: ResMut<Smoke>| {
                smoke.captured += 1;
            });
    }
    if frame > 430 && captured == 11 {
        let selected = world.get::<UsesTheme>(surface).unwrap().0;
        let state = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap();
        assert_eq!(
            selected, state.light,
            "Synthetic native click must select light theme"
        );
        assert!(
            world.get::<UiDiagnostics>(surface).unwrap().vertices > 0,
            "Gallery must publish draw geometry"
        );
        assert_eq!(
            world
                .get::<UiDiagnostics>(surface)
                .unwrap()
                .active_animations,
            0,
            "Animations must settle without ECS animation entities"
        );
        assert!(world.get::<UiIconStatus>(surface).unwrap().active.is_none());
        assert_eq!(
            world
                .get::<UiDiagnostics>(surface)
                .unwrap()
                .icon_rasterizations,
            world.resource::<Smoke>().stable_rasters,
            "Settled frames must reuse icon rasters"
        );
        info!(
            "Gallery smoke: native input, themes, motion, popups, resize, icon packs, artwork updates, stable raster cache and eleven captures passed"
        );
        world.write_message(AppExit::Success);
    }
    assert!(frame < 600, "Gallery capture timed out");
}

fn setup(world: &mut World) {
    if world.resource::<Smoke>().directory.is_some() {
        world.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ));
    }
    let light = world.spawn(themes::light()).id();
    let dark = world.spawn(themes::dark()).id();
    let surface = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world
        .entity_mut(surface)
        .insert((UiSurface, UsesTheme(dark)));
    world.spawn((Camera2d, CameraRenderGraph::new(UiGraph)));
    gallery::spawn(world, surface, light, dark);
}

fn close(
    mut actions: MessageReader<DesktopAction>,
    mut errors: MessageReader<UiError>,
    mut exit: MessageWriter<AppExit>,
) {
    for action in actions.read() {
        if matches!(action, DesktopAction::CloseRequested { .. }) {
            exit.write(AppExit::Success);
        }
    }
    for error in errors.read() {
        error!("{}", error.0);
    }
}
