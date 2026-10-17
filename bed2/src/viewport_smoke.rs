//! Rendered checks of isolation, background execution and asynchronous cleanup.
use bedtime_workbench2::*;
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk},
    window::PrimaryWindow,
};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Resource, Clone, Default)]
struct PipelinesReady(Arc<AtomicBool>);

#[derive(Resource)]
struct Smoke {
    directory: PathBuf,
    frame: u32,
    a: Option<WorkspaceId>,
    b: Option<WorkspaceId>,
    paused_ticks: u64,
    process: Option<WorkspaceProcess>,
    output: String,
    captures: u32,
    attempts: u32,
    capture_pending: bool,
    capture_retry: Option<(&'static str, u32)>,
    capture_attempts: u32,
}
pub(crate) fn install(app: &mut App) {
    let args: Vec<_> = std::env::args().collect();
    if let Some(index) = args.iter().position(|a| {
        matches!(
            a.as_str(),
            "--workspace-smoke-dir" | "--workbench-smoke-dir" | "--viewport-smoke-dir"
        )
    }) {
        let directory = PathBuf::from(
            args.get(index + 1)
                .expect("Smoke needs an output directory"),
        );
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
            frame: 0,
            a: None,
            b: None,
            paused_ticks: 0,
            process: None,
            output: String::new(),
            captures: 0,
            attempts: 0,
            capture_pending: false,
            capture_retry: None,
            capture_attempts: 0,
        })
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_systems(Update, step.after(WorkspaceSystems::Update));
    }
}
fn capture(world: &mut World, name: &'static str) {
    {
        let mut smoke = world.resource_mut::<Smoke>();
        smoke.capture_pending = true;
        smoke.capture_attempts += 1;
        assert!(
            smoke.capture_attempts <= 30,
            "Renderer kept returning blank screenshots"
        );
    }
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    let path = world
        .resource::<Smoke>()
        .directory
        .join(format!("{name}.png"));
    let corner_check = if name == "playground" {
        let size = world.get::<Window>(window).unwrap();
        let size = Vec2::new(size.width(), size.height());
        let mut w = Workbench::new(world);
        let (area, _) = w
            .world_mut()
            .query::<(&InArea, &bedtime_ui2::UiViewport)>()
            .single(w.world())
            .unwrap();
        Some((
            w.world().get::<AreaPresentation>(area.0).unwrap().surface,
            size,
        ))
    } else {
        None
    };
    let mut save = save_to_disk(path);
    world.spawn(Screenshot::window(window)).observe(
        move |event: On<ScreenshotCaptured>, mut smoke: ResMut<Smoke>| {
            let pixels = event.image.clone().try_into_dynamic().unwrap().to_rgb8();
            let bright = pixels
                .pixels()
                .filter(|p| p.0.iter().any(|&v| v > 80))
                .count();
            if bright <= 100 {
                warn!("Blank {name} screenshot during renderer startup; retrying capture");
                smoke.capture_retry = Some((name, smoke.attempts + 10));
                return;
            }
            if let Some((panel, size)) = corner_check {
                let scale = Vec2::new(pixels.width() as f32, pixels.height() as f32) / size;
                for x in [panel.min.x + 1.0, panel.max.x - 1.0] {
                    let corner = (Vec2::new(x, panel.max.y - 1.0) * scale).round().as_uvec2();
                    let outside = (Vec2::new(x, panel.max.y + 2.0) * scale).round().as_uvec2();
                    let actual = pixels.get_pixel(corner.x, corner.y).0;
                    let expected = pixels.get_pixel(outside.x, outside.y).0;
                    assert!(actual.into_iter().zip(expected).all(|(a,b)| a.abs_diff(b) <= 4),
                        "Viewport leaks into its bottom rounded corner: {actual:?}, expected background {expected:?}");
                }
            }
            save(event);
            smoke.captures += 1;
            smoke.capture_pending = false;
            smoke.capture_attempts = 0;
        },
    );
}
fn step(world: &mut World) {
    world.resource_mut::<Smoke>().attempts += 1;
    assert!(
        world.resource::<Smoke>().attempts < 4000,
        "Renderer did not become ready"
    );
    if !world.resource::<PipelinesReady>().0.load(Ordering::Acquire) {
        return;
    }
    if world.resource::<Smoke>().capture_pending {
        let retry = world.resource::<Smoke>().capture_retry;
        if let Some((name, after)) = retry {
            if world.resource::<Smoke>().attempts >= after {
                world.resource_mut::<Smoke>().capture_retry = None;
                capture(world, name);
            }
        }
        return;
    }
    world.resource_mut::<Smoke>().frame += 1;
    let frame = world.resource::<Smoke>().frame;
    if frame == 15 {
        let workspaces = Workspaces::new(world).list();
        let a = workspaces
            .iter()
            .find(|w| w.name == "Playground")
            .unwrap()
            .id;
        let b = workspaces.iter().find(|w| w.name == "Scratch").unwrap().id;
        let mut w = Workbench::with_workspace(world, a).unwrap();
        for mut note in w
            .world_mut()
            .query::<&mut crate::workbench_demo::Notes>()
            .iter_mut(w.world_mut())
        {
            note.text = "Only in Playground".into();
        }
        #[cfg(unix)]
        {
            let mut spec = ProcessSpec::new("/bin/sh");
            spec.arguments = ["-c", "printf 'workspace process alive\\n'; cat"]
                .map(Into::into)
                .to_vec();
            let process = w.work().spawn_process(None, "Smoke process", spec).unwrap();
            world.resource_mut::<Smoke>().process = Some(process);
        }
        world.resource_mut::<Smoke>().a = Some(a);
        world.resource_mut::<Smoke>().b = Some(b);
    }
    if frame == 45 {
        let visible = Workbench::new(world)
            .world_mut()
            .query::<&PanelPresentation>()
            .iter(Workbench::new(world).world())
            .filter(|p| p.visible)
            .count();
        assert_eq!(
            visible, 3,
            "All three active workspace panels are presented"
        );
        let proxies = world.query::<&WorkspaceViewport>().iter(world).count();
        assert_eq!(proxies, 1, "Workspace viewport reaches the host renderer");
        capture(world, "playground");
    }
    if frame == 65 {
        let b = world.resource::<Smoke>().b.unwrap();
        Workspaces::new(world).activate(b).unwrap();
    }
    if frame == 95 {
        let mut w = Workbench::new(world);
        for note in w
            .world_mut()
            .query::<&crate::workbench_demo::Notes>()
            .iter(w.world())
        {
            assert_ne!(note.text, "Only in Playground");
        }
        assert!(
            w.world().resource::<crate::workbench_demo::Session>().ticks >= 90,
            "Inactive workspace updated"
        );
        capture(world, "scratch");
        let a = world.resource::<Smoke>().a.unwrap();
        Workspaces::new(world)
            .set_background_updates(a, false)
            .unwrap();
        let ticks = Workbench::with_workspace(world, a)
            .unwrap()
            .world()
            .resource::<crate::workbench_demo::Session>()
            .ticks;
        world.resource_mut::<Smoke>().paused_ticks = ticks;
    }
    if frame == 125 {
        let a = world.resource::<Smoke>().a.unwrap();
        let ticks = Workbench::with_workspace(world, a)
            .unwrap()
            .world()
            .resource::<crate::workbench_demo::Session>()
            .ticks;
        assert_eq!(
            ticks,
            world.resource::<Smoke>().paused_ticks,
            "Suspended workspace retains state"
        );
        let mut smoke = world.resource_mut::<Smoke>();
        if let Some(process) = &smoke.process {
            let output = process.drain_output();
            for chunk in output {
                smoke
                    .output
                    .push_str(&String::from_utf8_lossy(&chunk.bytes));
            }
        }
        #[cfg(unix)]
        assert!(smoke.output.contains("workspace process alive"));
        Workspaces::new(world).close(a, None).unwrap();
    }
    if frame == 155 {
        let a = world.resource::<Smoke>().a.unwrap();
        assert!(
            world.get_entity(a.entity()).is_err(),
            "Closing retires a suspended workspace and its process"
        );
        capture(world, "after-close");
    }
    if frame == 170 {
        let gallery = crate::tool_panels::open::<crate::tool_panels::Gallery>(world);
        assert_eq!(
            crate::tool_panels::open::<crate::tool_panels::Gallery>(world),
            gallery
        );
    }
    if frame == 185 {
        let mut w = Workbench::new(world);
        let (state, presentation) = w
            .world_mut()
            .query::<(&bedtime_ui2::gallery::GalleryState, &PanelPresentation)>()
            .single(w.world())
            .unwrap();
        assert!(state.embedded && presentation.visible);
        let primary = state.observations["primary"].center();
        assert!(
            presentation.rect.contains(Vec2::new(primary.x, primary.y)),
            "Gallery widgets render inside their docked panel"
        );
        capture(world, "docked-gallery");
    }
    if frame == 200 {
        let editor = crate::tool_panels::open::<crate::tool_panels::ThemeEditor>(world);
        assert_eq!(
            crate::tool_panels::open::<crate::tool_panels::ThemeEditor>(world),
            editor
        );
    }
    if frame == 215 {
        let mut w = Workbench::new(world);
        let editor = w
            .world_mut()
            .query_filtered::<&PanelPresentation, With<crate::tool_panels::ThemeEditor>>()
            .single(w.world())
            .unwrap();
        assert!(editor.visible);
        capture(world, "docked-theme-editor");
    }
    if frame >= 240
        && world.resource::<Smoke>().captures == 5
        && !world.resource::<crate::Exiting>().0
    {
        info!(
            "Workspace smoke passed: docked tools, isolation, rendering, background suspension and process cleanup"
        );
        world.write_message(CloseRequested { window: None });
    }
    assert!(frame < 2000, "Screenshot readback timed out");
}
