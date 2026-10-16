//! Example-only verification of the real renderer. No OS input is injected.
use crate::viewport_demo::DemoState;
use bed2_demo_game::GameCamera;
use bedtime_ui2::{UiDiagnostics, UiViewport};
use bedtime_workbench2::{PanelPresentation, Workbench};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::PrimaryWindow,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Resource, Clone, Default)]
struct PipelinesReady(Arc<AtomicBool>);

#[derive(Resource, Default)]
struct Smoke {
    directory: PathBuf,
    frame: u32,
    attempts: u32,
    captures: HashMap<&'static str, (u64, [u8; 3])>,
    initial_size: UVec2,
    camera: Option<Entity>,
    drag_origin: Vec2,
    drag_target: Vec2,
    tabs_verified: bool,
}
#[derive(Component)]
struct Region {
    name: &'static str,
    rect: Rect,
    window: Vec2,
}
pub(crate) fn install(app: &mut App) {
    let mut args = std::env::args();
    if args.any(|arg| arg == "--viewport-smoke-dir" || arg == "--workbench-smoke-dir") {
        let directory = PathBuf::from(args.next().expect("--viewport-smoke-dir needs a directory"));
        std::fs::create_dir_all(&directory).unwrap();
        let ready = PipelinesReady::default();
        app.insert_resource(ready.clone());
        app.sub_app_mut(bevy::render::RenderApp)
            .insert_resource(ready)
            .add_systems(
                bevy::render::Render,
                (|cache: Res<bevy::render::render_resource::PipelineCache>,
                  ready: Res<PipelinesReady>| {
                    ready.0.store(
                        cache.waiting_pipelines().next().is_none(),
                        Ordering::Release,
                    );
                })
                .in_set(bevy::render::RenderSystems::Cleanup),
            );
        app.insert_resource(Smoke {
            directory,
            ..default()
        })
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_systems(Update, step);
    }
}

fn step(world: &mut World) {
    use bevy::winit::{RawWinitWindowEvent, WINIT_WINDOWS};
    let ready = world.resource::<PipelinesReady>().0.load(Ordering::Acquire);
    let frame = {
        let mut smoke = world.resource_mut::<Smoke>();
        smoke.attempts += 1;
        assert!(
            smoke.attempts < 4000,
            "Viewport pipelines/captures timed out"
        );
        if !ready {
            return;
        }
        smoke.frame += 1;
        smoke.frame
    };
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    // Scripted input is independent of OS focus (the user may be in another
    // application). Supply focus observations only inside this smoke fixture.
    if !world.get::<Window>(window).unwrap().focused {
        world.get_mut::<Window>(window).unwrap().focused = true;
        WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            let native = windows.get_window(window).unwrap();
            world.write_message(RawWinitWindowEvent {
                window_id: native.id(),
                event: winit::event::WindowEvent::Focused(true),
            });
        });
    }
    let view = world
        .query_filtered::<Entity, With<DemoState>>()
        .iter(world)
        .next();
    if frame == 380 {
        let camera = world
            .query_filtered::<Entity, With<GameCamera>>()
            .single(world)
            .unwrap();
        world.resource_mut::<Smoke>().camera = Some(camera);
        world.resource_mut::<Smoke>().initial_size = world
            .get::<UiViewport>(view.unwrap())
            .unwrap()
            .physical_size;
    }
    // Resize the actual ImGui window using native-shaped events through our adapter.
    // This exercises image resizing, camera projection refresh and GPU view replacement.
    if frame == 475 {
        let size = world.get::<Window>(window).unwrap().resolution.size();
        world.resource_mut::<Smoke>().drag_origin = Vec2::new(size.x * 0.59, size.y * 0.5);
    }
    if frame == 850 {
        let gallery = main_gallery(world);
        Workbench::new(world)
            .move_panel(
                view.unwrap(),
                gallery,
                Some(bedtime_workbench2::SplitAxis::Horizontal),
            )
            .unwrap();
    }
    if frame == 865 {
        let gallery = main_gallery(world);
        let origin = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect
            .center();
        let target = world
            .get::<PanelPresentation>(gallery)
            .unwrap()
            .tab_rect
            .center();
        let mut smoke = world.resource_mut::<Smoke>();
        smoke.drag_origin = origin;
        smoke.drag_target = target;
    }
    if frame == 885 {
        let gallery = main_gallery(world);
        world.resource_mut::<Smoke>().drag_origin = world
            .get::<PanelPresentation>(gallery)
            .unwrap()
            .tab_rect
            .center();
    }
    if frame == 910 {
        world.resource_mut::<Smoke>().drag_origin = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect
            .center();
    }
    let origin = world.resource::<Smoke>().drag_origin;
    let target = world.resource::<Smoke>().drag_target;
    let pointer = match frame {
        480..=483 => Some(origin.to_array()),
        484..=487 => Some((origin + Vec2::new(-100.0, 100.0)).to_array()),
        870..=873 | 890..=895 | 920..=925 => Some(origin.to_array()),
        874..=877 => Some(target.to_array()),
        _ => None,
    };
    if let Some([x, y]) = pointer {
        WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            let window = windows.get_window(window).unwrap();
            world.write_message(RawWinitWindowEvent {
                window_id: window.id(),
                event: winit::event::WindowEvent::CursorMoved {
                    device_id: winit::event::DeviceId::dummy(),
                    position: winit::dpi::PhysicalPosition::new(
                        f64::from(x) * window.scale_factor(),
                        f64::from(y) * window.scale_factor(),
                    ),
                },
            });
            if [482, 486, 872, 876, 892, 894, 922, 924].contains(&frame) {
                world.write_message(RawWinitWindowEvent {
                    window_id: window.id(),
                    event: winit::event::WindowEvent::MouseInput {
                        device_id: winit::event::DeviceId::dummy(),
                        button: winit::event::MouseButton::Left,
                        state: if [482, 872, 892, 922].contains(&frame) {
                            winit::event::ElementState::Pressed
                        } else {
                            winit::event::ElementState::Released
                        },
                    },
                });
            }
        });
    }
    if frame == 510 {
        let viewport = world.get::<UiViewport>(view.unwrap()).unwrap();
        assert_ne!(
            viewport.physical_size,
            world.resource::<Smoke>().initial_size,
            "The native drag must resize the scene target"
        );
        assert_eq!(
            world
                .resource::<Assets<Image>>()
                .get(&viewport.image)
                .unwrap()
                .size(),
            viewport.physical_size
        );
    }
    if frame == 560 {
        world.get_mut::<DemoState>(view.unwrap()).unwrap().overlay = true;
    }
    if frame == 680 {
        Workbench::new(world).close_panel(view.unwrap()).unwrap();
    }
    if frame == 700 {
        assert_eq!(
            world
                .get::<UiDiagnostics>(window)
                .unwrap()
                .viewport_textures,
            0
        );
        let camera = world.resource::<Smoke>().camera.unwrap();
        assert!(!world.get::<Camera>(camera).unwrap().is_active);
    }
    if frame == 710 {
        crate::viewport_demo::show(world);
    }
    let name = match frame {
        400 => Some("panel"),
        460 => Some("spinning"),
        530 => Some("resized"),
        620 => Some("overlay"),
        770 => Some("reopened"),
        _ => None,
    };
    if let Some(name) = name {
        let size = world.get::<Window>(window).unwrap().resolution.size();
        let rect = if name == "overlay" {
            Rect::from_corners(size * 0.25, size * 0.75)
        } else {
            world.get::<UiViewport>(view.unwrap()).unwrap().rect
        };
        world
            .spawn((
                Screenshot::primary_window(),
                Region {
                    name,
                    rect,
                    window: size,
                },
            ))
            .observe(inspect);
    }
    // Exercise retention through the public lifecycle API, after visual checks.
    if frame == 800 {
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let (reading, scratch) = (context.reading, context.scratch);
        let mut w = Workbench::new(world);
        w.activate_layout(reading).unwrap();
        w.activate_workspace(scratch).unwrap();
    }
    if frame == 805 {
        let cube = world
            .query_filtered::<Entity, With<DemoState>>()
            .single(world)
            .unwrap();
        assert!(!world.get::<PanelPresentation>(cube).unwrap().visible);
        let camera = world.resource::<Smoke>().camera.unwrap();
        assert!(!world.get::<Camera>(camera).unwrap().is_active);
        let note = world
            .query::<&crate::workbench_demo::Notes>()
            .single(world)
            .unwrap();
        assert!(note.text.contains("panel-local state"));
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let (home, split) = (context.home, context.split);
        let mut w = Workbench::new(world);
        w.activate_workspace(home).unwrap();
        w.activate_layout(split).unwrap();
    }
    if frame == 880 {
        let gallery = main_gallery(world);
        let a = world.get::<PanelPresentation>(gallery).unwrap().tab_rect;
        let b = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect;
        assert!(
            (a.center().x - b.center().x).abs() < 400.0,
            "Native tab drag must merge the split groups"
        );
    }
    if frame == 900 {
        let gallery = main_gallery(world);
        assert!(
            world.get::<PanelPresentation>(gallery).unwrap().visible,
            "Native click must select the gallery tab"
        );
        assert!(
            !world
                .get::<PanelPresentation>(view.unwrap())
                .unwrap()
                .visible
        );
        assert!(
            !world
                .get::<Camera>(world.resource::<Smoke>().camera.unwrap())
                .unwrap()
                .is_active
        );
        let rect = world.get::<PanelPresentation>(gallery).unwrap().rect;
        let size = world.get::<Window>(window).unwrap().resolution.size();
        world
            .spawn((
                Screenshot::primary_window(),
                Region {
                    name: "tabs",
                    rect,
                    window: size,
                },
            ))
            .observe(inspect_tabs);
    }
    crate::panel_smoke::step(world, frame);
    if frame > 2240
        && world.resource::<Smoke>().captures.len() == 5
        && world.resource::<Smoke>().tabs_verified
    {
        let smoke = world.resource::<Smoke>();
        let camera = smoke.camera.unwrap();
        assert_ne!(
            smoke.captures["panel"].0, smoke.captures["spinning"].0,
            "The cube must visibly rotate"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<GameCamera>>()
                .single(world)
                .unwrap(),
            camera
        );
        assert!(world.get::<UiDiagnostics>(window).unwrap().error.is_none());
        let cube = world
            .query_filtered::<Entity, With<DemoState>>()
            .single(world)
            .unwrap();
        assert!(world.get::<PanelPresentation>(cube).unwrap().visible);
        info!(
            "Workbench smoke passed: spinning panel, splitter input, full-window overlay/alpha, actual close/recreate, retained workspace/layout switches, native tab drag/selection and five pixel-checked captures"
        );
        world.write_message(AppExit::Success);
    }
    assert!(frame < 2500, "Viewport smoke timed out");
}

fn inspect(capture: On<ScreenshotCaptured>, regions: Query<&Region>, mut smoke: ResMut<Smoke>) {
    let region = regions.get(capture.entity).unwrap();
    let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
    image
        .save(smoke.directory.join(format!("{}.png", region.name)))
        .unwrap();
    let scale = image.width() as f32 / region.window.x;
    let sample = |position: Vec2| -> [u8; 3] {
        let x = (position.x * scale)
            .round()
            .clamp(0.0, image.width() as f32 - 1.0) as u32;
        let y = (position.y * scale)
            .round()
            .clamp(0.0, image.height() as f32 - 1.0) as u32;
        let pixel = image.get_pixel(x, y).0;
        [pixel[0], pixel[1], pixel[2]]
    };
    let mut bright = 0;
    let mut hash = 0_u64;
    let min = region.rect.min + Vec2::splat(4.0);
    let max = region.rect.max - Vec2::splat(4.0);
    for y in (min.y as u32..max.y as u32).step_by(3) {
        for x in (min.x as u32..max.x as u32).step_by(3) {
            let rgb = sample(Vec2::new(x as f32, y as f32));
            bright += usize::from(rgb.iter().map(|&v| v as u32).sum::<u32>() > 220);
            for value in rgb {
                hash = hash.wrapping_mul(16777619) ^ value as u64;
            }
        }
    }
    assert!(
        bright > 80,
        "{} must contain rendered cube pixels, got {bright}",
        region.name
    );
    let clear = if region.name == "overlay" {
        // The in-application menu now covers the top edge. Sample unobstructed
        // scene output at the right edge, away from chrome and the alpha patch.
        sample(Vec2::new(region.window.x - 20.0, region.window.y * 0.5))
    } else {
        sample(region.rect.min + Vec2::splat(8.0))
    };
    if region.name == "overlay" {
        let reference = smoke.captures["panel"].1;
        for channel in 0..3 {
            assert!(
                clear[channel].abs_diff(reference[channel]) <= 3,
                "Panel and full-window output must preserve scene colors: {clear:?} vs {reference:?}"
            );
        }
        let patch = sample(region.window - Vec2::new(90.0, 70.0));
        let decode = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let encode = |v: f32| {
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        for (channel, ink) in [0.8, 0.2, 0.3].into_iter().enumerate() {
            let expected = (encode(decode(ink) * 0.5 + decode(clear[channel] as f32 / 255.0) * 0.5)
                * 255.0)
                .round() as u8;
            assert!(
                patch[channel].abs_diff(expected) <= 5,
                "Transparent UI must blend in linear light: {patch:?}, channel {channel}, expected {expected}"
            );
        }
    }
    info!(
        "Viewport capture {}: {bright} cube samples, background {clear:?}",
        region.name
    );
    smoke.captures.insert(region.name, (hash, clear));
}

fn main_gallery(world: &mut World) -> Entity {
    let split = world
        .resource::<crate::workbench_demo::DemoWorkbench>()
        .split;
    world.query_filtered::<(Entity, &bedtime_workbench2::InArea), With<bedtime_ui2::gallery::GalleryState>>()
        .iter(world).find_map(|(e,l)| (world.get::<bedtime_workbench2::InLayout>(l.0).unwrap().0 == split).then_some(e)).unwrap()
}

fn inspect_tabs(
    capture: On<ScreenshotCaptured>,
    regions: Query<&Region>,
    mut smoke: ResMut<Smoke>,
) {
    let region = regions.get(capture.entity).unwrap();
    let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
    image.save(smoke.directory.join("tabs.png")).unwrap();
    let scale = image.width() as f32 / region.window.x;
    let mut ink = 0;
    // The first column must remain visible after the content window is reparented.
    // Its headings/buttons/text are all brighter than the uniform background.
    for y in (160..450).step_by(2) {
        for x in (5..240).step_by(2) {
            let pixel = image
                .get_pixel(
                    ((region.rect.min.x + x as f32) * scale) as u32,
                    ((region.rect.min.y + y as f32) * scale) as u32,
                )
                .0;
            ink += usize::from(pixel[..3].iter().map(|&v| v as u32).sum::<u32>() > 300);
        }
    }
    assert!(
        ink > 100,
        "Regrouped gallery must retain visible first-column widgets, got {ink} ink samples"
    );
    smoke.tabs_verified = true;
}
