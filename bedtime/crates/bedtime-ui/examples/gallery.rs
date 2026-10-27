use bedtime_ui::{gallery, *};
use bevy::{prelude::*, window::PrimaryWindow};

fn main() -> AppExit {
    let mut app = App::new();
    let smoke_options = Smoke::from_args();
    if smoke_options.directory.is_some() {
        app.insert_resource(bedtime_clipboard::Clipboard::memory())
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(1.0 / 60.0),
            ))
            .add_systems(
                PreUpdate,
                isolate_pointer
                    .after(bevy_egui::EguiPreUpdateSet::ProcessInput)
                    .before(UiSystems::Draw),
            );
    }
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Bedtime UI 2 — Widget gallery".into(),
            resolution: (1160, 900).into(),
            ..default()
        }),
        close_when_requested: false,
        ..default()
    }))
    .add_plugins((bedtime_clipboard::ClipboardPlugin, UiPlugin))
    .insert_resource(smoke_options)
    .add_systems(Startup, setup)
    .add_systems(Update, close)
    .add_systems(
        PreUpdate,
        smoke
            .after(bevy::input::InputSystems)
            .before(bevy_egui::EguiPreUpdateSet::ProcessInput),
    )
    .run()
}

#[derive(Resource, Default)]
struct Smoke {
    directory: Option<std::path::PathBuf>,
    frame: u32,
    captured: u32,
    stable_rasters: u64,
    last_pointer: Option<Vec2>,
    cursor_emitted: bool,
}

impl Smoke {
    fn from_args() -> Self {
        let mut args = std::env::args();
        let directory = args
            .find(|a| a == "--smoke-dir")
            .map(|_| std::path::PathBuf::from(args.next().expect("--smoke-dir needs a directory")));
        if let Some(p) = &directory {
            std::fs::create_dir_all(p).unwrap();
        }
        Self {
            directory,
            ..default()
        }
    }
}

fn pointer(world: &mut World, window: Entity, position: Vec2, button: Option<(MouseButton, bool)>) {
    let moved = world.resource::<Smoke>().last_pointer != Some(position);
    world.resource_mut::<Smoke>().last_pointer = Some(position);
    if moved || button.is_some() {
        world.resource_mut::<Smoke>().cursor_emitted = true;
        let moved = bevy::window::CursorMoved {
            window,
            position,
            delta: None,
        };
        world.write_message(moved.clone());
        world.write_message(bevy::window::WindowEvent::CursorMoved(moved));
    }
    if let Some((button, pressed)) = button {
        let event = bevy::input::mouse::MouseButtonInput {
            window,
            button,
            state: if pressed {
                bevy::input::ButtonState::Pressed
            } else {
                bevy::input::ButtonState::Released
            },
        };
        world.write_message(event);
        world.write_message(bevy::window::WindowEvent::MouseButtonInput(event));
    }
}

// Keep real OS movement from redirecting the scripted pointer or resetting the
// tooltip delay. Stationary frames must not emit another PointerMoved event.
fn isolate_pointer(smoke: Res<Smoke>, mut inputs: Query<&mut bevy_egui::EguiInput>) {
    if smoke.last_pointer.is_none() {
        return;
    }
    for mut input in &mut inputs {
        let keep = smoke
            .cursor_emitted
            .then(|| {
                input
                    .events
                    .iter()
                    .rposition(|event| matches!(event, egui::Event::PointerMoved(_)))
            })
            .flatten();
        let mut index = 0;
        input.events.retain(|event| {
            let current = index;
            index += 1;
            match event {
                egui::Event::PointerMoved(_) => keep == Some(current),
                egui::Event::PointerGone | egui::Event::MouseMoved(_) => false,
                _ => true,
            }
        });
    }
}

fn smoke(world: &mut World) {
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
    let (frame, directory) = {
        let mut s = world.resource_mut::<Smoke>();
        let Some(dir) = s.directory.clone() else {
            return;
        };
        s.frame += 1;
        s.cursor_emitted = false;
        (s.frame, dir)
    };
    let window = world
        .query_filtered::<Entity, With<UiSurface>>()
        .single(world)
        .unwrap();
    if !world.get::<Window>(window).unwrap().focused {
        world.get_mut::<Window>(window).unwrap().focused = true;
        world.write_message(bevy::window::WindowEvent::WindowFocused(
            bevy::window::WindowFocused {
                window,
                focused: true,
            },
        ));
    }
    let target = match frame {
        10..=110 => Some("tooltip"),
        120..=125 => Some("light"),
        170..=175 => Some("selector"),
        187..=191 => Some("compact"),
        220..=225 => Some("context"),
        _ => None,
    };
    if let Some(name) = target {
        if let Some(rect) = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap()
            .observations
            .get(name)
            .copied()
        {
            let button = match frame {
                122 | 172 | 188 => Some((MouseButton::Left, true)),
                124 | 174 | 190 => Some((MouseButton::Left, false)),
                222 => Some((MouseButton::Right, true)),
                224 => Some((MouseButton::Right, false)),
                _ => None,
            };
            pointer(
                world,
                window,
                Vec2::new(rect.center().x, rect.center().y),
                button,
            );
        }
    }
    if frame == 240 || frame == 242 {
        pointer(
            world,
            window,
            Vec2::new(1100.0, 850.0),
            Some((MouseButton::Left, frame == 240)),
        );
    }
    if frame == 150 {
        let mut state = world
            .query::<&mut gallery::GalleryState>()
            .single_mut(world)
            .unwrap();
        state.toggled = true;
        state.open = true;
    }
    if frame == 195 {
        assert_eq!(
            world
                .query::<&gallery::GalleryState>()
                .single(world)
                .unwrap()
                .choice,
            1,
            "Native pointer input must select Compact"
        );
    }
    if frame == 205 || frame == 216 {
        world
            .query::<&mut gallery::GalleryState>()
            .single_mut(world)
            .unwrap()
            .dialog = frame == 205;
    }
    if frame == 207 || frame == 209 {
        let key = bevy::input::keyboard::KeyboardInput {
            window,
            key_code: KeyCode::KeyS,
            logical_key: bevy::input::keyboard::Key::Character("s".into()),
            state: if frame == 207 {
                bevy::input::ButtonState::Pressed
            } else {
                bevy::input::ButtonState::Released
            },
            text: None,
            repeat: false,
        };
        world.write_message(key.clone());
        world.write_message(bevy::window::WindowEvent::KeyboardInput(key));
    }
    if frame == 208 {
        assert!(world.get::<UiCapture>(window).unwrap().modal);
    }
    if frame == 250 {
        world
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1000.0, 820.0);
    }
    if frame == 300 {
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
        world.entity_mut(window).insert(UiIcons(pack));
    }
    if frame == 350 {
        let icon = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap()
            .custom_icon
            .clone();
        let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="m12 1 3 7 8 1-6 5 2 9-7-4-7 4 2-9-6-5 8-1Z"/></svg>"#;
        world
            .resource_mut::<Assets<Icon>>()
            .insert(icon.id(), Icon::from_svg(svg).unwrap())
            .unwrap();
    }
    if frame == 370 {
        world.entity_mut(window).remove::<UiIcons>();
    }
    if frame == 395 {
        world
            .query::<&mut gallery::GalleryState>()
            .single_mut(world)
            .unwrap()
            .icon_browser = false;
        pointer(world, window, Vec2::new(1100.0, 850.0), None);
    }
    if frame == 405 {
        world.resource_mut::<Smoke>().stable_rasters = world
            .get::<UiDiagnostics>(window)
            .unwrap()
            .icon_rasterizations;
    }
    if matches!(frame, 43 | 49 | 94 | 106) {
        assert!(
            world
                .query::<&gallery::GalleryState>()
                .single(world)
                .unwrap()
                .tooltip_visible,
            "Tooltip captures must contain an open tooltip"
        );
    }
    let name = match frame {
        35 => Some("dark.png"),
        43 => Some("tooltip-opening.png"),
        49 => Some("tooltip-late.png"),
        94 => Some("tooltip.png"),
        106 => Some("tooltip-settled.png"),
        140 => Some("light.png"),
        155 => Some("motion.png"),
        182 => Some("dropdown.png"),
        212 => Some("modal.png"),
        226 => Some("menu-opening.png"),
        232 => Some("menu-late.png"),
        235 => Some("menu.png"),
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
            .observe(|_: On<ScreenshotCaptured>, mut s: ResMut<Smoke>| {
                s.captured += 1;
            });
    }
    if frame > 430 && world.resource::<Smoke>().captured == 17 {
        let state = world
            .query::<&gallery::GalleryState>()
            .single(world)
            .unwrap();
        assert_eq!(
            world.get::<UsesTheme>(window).unwrap().0,
            state.light,
            "Native light theme click"
        );
        let diag = world.get::<UiDiagnostics>(window).unwrap();
        assert!(diag.vertices > 0);
        assert_eq!(diag.active_animations, 0);
        assert_eq!(
            diag.icon_rasterizations,
            world.resource::<Smoke>().stable_rasters
        );
        assert!(world.get::<UiIconStatus>(window).unwrap().active.is_none());
        info!(
            "egui gallery smoke passed: native pointer, themes, dropdown, overlay growth, resize, icon packs, hot replacement and seventeen captures"
        );
        world.write_message(AppExit::Success);
    }
    assert!(frame < 600, "Gallery smoke timed out");
}

fn setup(world: &mut World) {
    let light = world.spawn(themes::light()).id();
    let dark = world.spawn(themes::dark()).id();
    let surface = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world
        .entity_mut(surface)
        .insert((UiSurface, UsesTheme(dark)));
    world.spawn((Camera2d, UiCamera(surface)));
    gallery::spawn(world, surface, light, dark);
}

fn close(
    mut actions: MessageReader<bevy::window::WindowCloseRequested>,
    mut errors: MessageReader<UiError>,
    mut exit: MessageWriter<AppExit>,
) {
    for _ in actions.read() {
        exit.write(AppExit::Success);
    }
    for error in errors.read() {
        error!("{}", error.0);
    }
}
