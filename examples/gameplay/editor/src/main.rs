//! The game shares this Bevy World with the editor. This adapter owns output
//! attachments and input admission; the game has no knowledge of its panels.
use bed_core::api::workbench::*;
use bed_desktop::{DesktopCapture, DesktopIntegrationPlugin, DesktopSystems};
use bed_example_game::{ExampleGamePlugin, GameCamera, GameExecution, GameProgress};
use bed_ui::{TextureHandle, gpu::*};
use bevy::{
    camera::{ManualTextureViewHandle, RenderTarget as CameraTarget},
    prelude::*,
    render::texture::{ManualTextureView, ManualTextureViews},
};
use serde_json::{Value, json};

const PANEL: &str = "example.game.viewport";
const VIEW: ManualTextureViewHandle = ManualTextureViewHandle(u32::MAX - 1);
#[derive(Resource, Default)]
struct ViewportInput {
    drawn: bool,
    keyboard: bool,
    pointer: bool,
    output: Option<Entity>,
    generation: u64,
    center: [f32; 2],
}
struct GamePanel {
    entity: Entity,
    texture: TextureHandle,
    keyboard_focus: bool,
}
impl Panel for GamePanel {
    fn title(&self, _: &World) -> String {
        "Game viewport".into()
    }
    fn draw(
        &mut self,
        ui: &bed_desktop::dear_imgui_rs::Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        let playing = world.resource::<GameExecution>().playing;
        if ui.button(if playing { "Pause" } else { "Play" }) {
            world.resource_mut::<GameExecution>().playing = !playing;
            self.keyboard_focus = false;
        }
        ui.same_line();
        ui.text("Click viewport · WASD moves · mouse motion rotates");
        let texture = world
            .resource::<CanvasTextures>()
            .0
            .get(&self.texture)
            .copied();
        let origin = ui.cursor_screen_pos();
        let canvas = Canvas::show(ui, texture, "game-canvas");
        let window_focused = ui.is_window_focused();
        if ui.is_mouse_clicked(bed_desktop::dear_imgui_rs::MouseButton::Left) {
            self.keyboard_focus = canvas.hovered;
        }
        self.keyboard_focus &= window_focused;
        let playing = world.resource::<GameExecution>().playing;
        *world.resource_mut::<ViewportInput>() = ViewportInput {
            drawn: true,
            keyboard: self.keyboard_focus && playing,
            pointer: self.keyboard_focus && canvas.hovered && playing,
            output: Some(self.entity),
            generation: world.resource::<ViewportInput>().generation,
            center: [
                origin[0] + canvas.size[0] * 0.5,
                origin[1] + canvas.size[1] * 0.5,
            ],
        };
        let revision = world.resource::<GameProgress>().ticks;
        world.entity_mut(self.entity).insert((
            CanvasOutput(RenderOutput {
                handle: self.texture,
                size: canvas.pixels,
                depth: false,
                revision,
            }),
            CanvasVisible(true),
        ));
    }
    fn save(&self, _: &World) -> Value {
        Value::Null
    }
}
fn create(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    let texture = TextureHandle::next();
    let render = world.register_system(attach_game_camera);
    world
        .entity_mut(input.entity)
        .insert((CanvasRenderer(render), CanvasVisible(false)));
    Ok(Box::new(GamePanel {
        entity: input.entity,
        texture,
        keyboard_focus: false,
    }))
}
fn attach_game_camera(
    In(input): In<CanvasRenderInvocation>,
    world: &mut World,
) -> CanvasRenderResult {
    let view = input
        .target
        .texture
        .create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            ..default()
        });
    world.resource_mut::<ManualTextureViews>().insert(
        VIEW,
        ManualTextureView {
            texture_view: view.into(),
            size: UVec2::from_array(input.target.size),
            view_format: wgpu::TextureFormat::Rgba8UnormSrgb,
        },
    );
    let camera = world
        .query_filtered::<Entity, With<GameCamera>>()
        .single(world)
        .map_err(|err| err.to_string())?;
    world
        .entity_mut(camera)
        .insert(CameraTarget::TextureView(VIEW));
    world.get_mut::<Camera>(camera).unwrap().is_active = true;
    world.resource_mut::<ViewportInput>().generation = input.generation;
    Ok(Vec::new())
}
fn begin_frame(world: &mut World) {
    let generation = world.resource::<ViewportInput>().generation;
    *world.resource_mut::<ViewportInput>() = ViewportInput {
        generation,
        ..default()
    };
    for mut camera in world
        .query_filtered::<&mut Camera, With<GameCamera>>()
        .iter_mut(world)
    {
        camera.is_active = false;
    }
    if generation != 0
        && world
            .get_resource::<bed_desktop::DesktopGpu>()
            .map(|gpu| gpu.generation)
            != Some(generation)
    {
        world.resource_mut::<ManualTextureViews>().remove(&VIEW);
        let cameras = world
            .query_filtered::<Entity, With<GameCamera>>()
            .iter(world)
            .collect::<Vec<_>>();
        for camera in cameras {
            world.entity_mut(camera).insert(CameraTarget::default());
        }
    }
}
fn admit_input(world: &mut World) {
    let input = world.resource::<ViewportInput>();
    let capture = DesktopCapture {
        keyboard: !input.keyboard,
        pointer: !input.pointer,
    };
    world.insert_resource(capture);
    if !world.resource::<ViewportInput>().drawn {
        world.resource_mut::<ManualTextureViews>().remove(&VIEW);
        let cameras = world
            .query_filtered::<Entity, With<GameCamera>>()
            .iter(world)
            .collect::<Vec<_>>();
        for camera in cameras {
            world.entity_mut(camera).insert(CameraTarget::default());
        }
    }
}
fn open(In(_): In<CommandInvocation>, _: &mut World) -> CommandResult {
    Ok(vec![WorkbenchAction::ShowPanel {
        panel_type: PANEL.into(),
        state: Value::Null,
        area: None,
    }])
}
struct GameEditorPlugin;
impl Plugin for GameEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewportInput>();
        bed_core::bed_workbench::register(app.world_mut(), "example.game", |register| {
            register.panel(PANEL, "Game viewport", create);
            register.command("example.game.open", "Open game viewport", open);
            register.menu("example.game.open");
        })
        .unwrap();
        app.add_systems(Startup, start)
            .add_systems(PreUpdate, begin_frame.in_set(DesktopSystems::Configure))
            .add_systems(
                PreUpdate,
                admit_input
                    .in_set(DesktopSystems::AfterUi)
                    .before(bed_core::CoreSystems::Requests),
            );
    }
}
fn start(world: &mut World) {
    bed_core::bed_workbench::open_panel(world, json!({"panel_type":PANEL}), None, None).unwrap();
}
fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Game editor — same Bevy World".into(),
                    resolution: (1000, 700).into(),
                    ..default()
                }),
                ..default()
            })
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    );
    // Recovery remains a host decision even when desktop UI is attached.
    app.insert_resource(bevy::render::error_handler::RenderErrorHandler(
        |error, world, _| {
            use bevy::render::error_handler::{ErrorType, RenderErrorPolicy};
            if error.ty == ErrorType::DeviceLost {
                RenderErrorPolicy::Recover(bevy::render::settings::WgpuSettings::default().into())
            } else {
                world.write_message(AppExit::error());
                RenderErrorPolicy::StopRendering
            }
        },
    ));
    app.add_plugins((
        DesktopIntegrationPlugin,
        bed_core::BedCoreIntegrationPlugin,
        ExampleGamePlugin,
        GameEditorPlugin,
    ));
    if std::env::args().any(|arg| arg == "--smoke-test") {
        // Pixel assertions concern viewport content, independently of the
        // workbench's time-based opening fade.
        app.world_mut()
            .resource_mut::<bed_core::bed_workbench::WorkbenchAppearance>()
            .animations = false;
        app.insert_resource(Smoke {
            phase: 0,
            ticks: 0,
            camera: None,
            started: std::time::Instant::now(),
            size: UVec2::ZERO,
            input: None,
            capture: None,
        })
        .add_systems(Update, smoke.after(bed_example_game::GameSystems));
    }
    app.run();
}
#[derive(Resource)]
struct Smoke {
    phase: u8,
    ticks: u64,
    camera: Option<Entity>,
    started: std::time::Instant,
    size: UVec2,
    input: Option<bed_desktop::DesktopInputId>,
    capture: Option<bed_desktop::DesktopCaptureId>,
}
fn player_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|color| {
            color[2] > 180
                && color[1] > 140
                && color[0] < 160
                && color[1] > color[0].saturating_add(50)
                && color[2] > color[1].saturating_add(10)
        })
        .count()
}
fn smoke(world: &mut World) {
    assert!(
        world.resource::<Smoke>().started.elapsed() < std::time::Duration::from_secs(30),
        "game editor smoke timed out"
    );
    let frames = world
        .resource::<bed_desktop::DesktopRenderStatus>()
        .read()
        .completed_frames;
    if frames < 8 {
        return;
    }
    let phase = world.resource::<Smoke>().phase;
    match phase {
        0 => {
            assert!(!world.contains_resource::<bed_desktop::BedMemoryAssets>());
            assert_eq!(
                world
                    .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
                    .iter(world)
                    .count(),
                1
            );
            let camera = world
                .query_filtered::<Entity, With<GameCamera>>()
                .single(world)
                .unwrap();
            assert!(matches!(
                world.get::<CameraTarget>(camera),
                Some(CameraTarget::TextureView(VIEW))
            ));
            assert!(world.resource::<ViewportInput>().generation > 0);
            world.resource_mut::<Smoke>().camera = Some(camera);
            world.resource_mut::<Smoke>().size = world.resource::<ManualTextureViews>()[&VIEW].size;
            use bed_desktop::{DesktopInjectedEvent as Event, DesktopInputStep};
            let center = world.resource::<ViewportInput>().center;
            let mut steps = vec![
                vec![
                    Event::PointerMoved(center),
                    Event::PointerButton {
                        button: bed_desktop::winit::event::MouseButton::Left,
                        pressed: true,
                    },
                ]
                .into(),
                vec![Event::PointerButton {
                    button: bed_desktop::winit::event::MouseButton::Left,
                    pressed: false,
                }]
                .into(),
                vec![Event::Key {
                    physical: bed_desktop::winit::keyboard::KeyCode::KeyW,
                    logical: bed_desktop::winit::keyboard::Key::Character("w".into()),
                    pressed: true,
                }]
                .into(),
            ];
            steps.extend((0..8).map(|_| DesktopInputStep::default()));
            let id = world
                .resource_mut::<bed_desktop::DesktopInputRequests>()
                .submit(steps);
            world.resource_mut::<Smoke>().input = Some(id);
            world.resource_mut::<Smoke>().phase = 1;
        }
        1 => {
            let id = world.resource::<Smoke>().input.unwrap();
            let Some(result) = world
                .resource_mut::<bed_desktop::DesktopInputResults>()
                .take(id)
            else {
                return;
            };
            result.unwrap();
            assert!(
                world.resource::<GameProgress>().position.y > 0.0,
                "focused viewport must admit ordinary Bevy keyboard input"
            );
            assert!(
                !world
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::KeyW)
            );
            world.write_message(bevy::window::WindowEvent::MouseMotion(
                bevy::input::mouse::MouseMotion {
                    delta: Vec2::new(9.0, 0.0),
                },
            ));
            world.resource_mut::<Smoke>().phase = 2;
        }
        2 => {
            assert!(
                world.resource::<GameProgress>().heading < 0.0,
                "viewport must admit ordinary relative mouse input"
            );
            world.resource_mut::<Smoke>().ticks = world.resource::<GameProgress>().ticks;
            let panel = world.resource::<ViewportInput>().output.unwrap();
            bed_core::bed_workbench::request_close(world, panel).unwrap();
            world.resource_mut::<Smoke>().phase = 3;
        }
        3 => {
            assert!(
                !world
                    .get::<Camera>(world.resource::<Smoke>().camera.unwrap())
                    .unwrap()
                    .is_active
            );
            assert!(!world.resource::<ManualTextureViews>().contains_key(&VIEW));
            assert!(world.resource::<DesktopCapture>().keyboard);
            assert!(world.resource::<DesktopCapture>().pointer);
            start(world);
            world.resource_mut::<Smoke>().phase = 4;
        }
        4 => {
            if world.resource::<ViewportInput>().output.is_none() {
                return;
            }
            let camera = world
                .query_filtered::<Entity, With<GameCamera>>()
                .single(world)
                .unwrap();
            assert_eq!(Some(camera), world.resource::<Smoke>().camera);
            assert!(world.resource::<GameProgress>().ticks > world.resource::<Smoke>().ticks);
            world
                .query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>()
                .single_mut(world)
                .unwrap()
                .resolution
                .set(720.0, 480.0);
            world.resource_mut::<GameExecution>().playing = false;
            world.resource_mut::<Smoke>().ticks = world.resource::<GameProgress>().ticks;
            world.resource_mut::<Smoke>().phase = 5;
        }
        5 => {
            assert_eq!(
                world.resource::<Smoke>().ticks,
                world.resource::<GameProgress>().ticks
            );
            if world.resource::<ManualTextureViews>()[&VIEW].size == world.resource::<Smoke>().size
            {
                return;
            }
            let panel = world.resource::<ViewportInput>().output.unwrap();
            let info = world.resource::<bed_desktop::DesktopFrameInfo>().clone();
            let deadline = world.resource::<Smoke>().started + std::time::Duration::from_secs(30);
            let id = world
                .resource_mut::<bed_desktop::DesktopCaptureRequests>()
                .submit(
                    vec![bed_desktop::DesktopCaptureTarget::Panel(panel)],
                    deadline,
                    Some(info.frame),
                );
            world.resource_mut::<Smoke>().capture = Some(id);
            world.resource_mut::<Smoke>().phase = 6;
        }
        6 => {
            let id = world.resource::<Smoke>().capture.unwrap();
            let Some(result) = world
                .resource_mut::<bed_desktop::DesktopCaptureResults>()
                .take(id)
            else {
                return;
            };
            let capture = result.unwrap();
            assert_eq!(
                world
                    .query_filtered::<Entity, With<GameCamera>>()
                    .iter(world)
                    .count(),
                1
            );
            let pixels = &capture.images[0].rgba.pixels;
            capture.images[0]
                .rgba
                .write_png(std::env::temp_dir().join("bed-game-editor-smoke.png"))
                .unwrap();
            assert!(
                player_pixels(pixels) > 400,
                "captured viewport must contain the game's blue player"
            );
            world.resource::<bed_desktop::DesktopGpu>().device.destroy();
            world.resource_mut::<Smoke>().phase = 7;
        }
        7 => {
            let status = world.resource::<bed_desktop::DesktopRenderStatus>().read();
            if status.recoveries != 1
                || status.recovering
                || world.resource::<ViewportInput>().generation < 2
            {
                return;
            }
            assert_eq!(
                world
                    .query_filtered::<Entity, With<GameCamera>>()
                    .single(world)
                    .unwrap(),
                world.resource::<Smoke>().camera.unwrap()
            );
            assert_eq!(
                world.resource::<Smoke>().ticks,
                world.resource::<GameProgress>().ticks
            );
            let panel = world.resource::<ViewportInput>().output.unwrap();
            let deadline = world.resource::<Smoke>().started + std::time::Duration::from_secs(30);
            let info = world.resource::<bed_desktop::DesktopFrameInfo>().clone();
            let id = world
                .resource_mut::<bed_desktop::DesktopCaptureRequests>()
                .submit(
                    vec![bed_desktop::DesktopCaptureTarget::Panel(panel)],
                    deadline,
                    Some(info.frame),
                );
            world.resource_mut::<Smoke>().capture = Some(id);
            world.resource_mut::<Smoke>().phase = 8;
        }
        8 => {
            let id = world.resource::<Smoke>().capture.unwrap();
            let Some(result) = world
                .resource_mut::<bed_desktop::DesktopCaptureResults>()
                .take(id)
            else {
                return;
            };
            let capture = result.unwrap();
            assert!(capture.info.generation >= 2);
            capture.images[0]
                .rgba
                .write_png(std::env::temp_dir().join("bed-game-editor-recovered-smoke.png"))
                .unwrap();
            assert!(player_pixels(&capture.images[0].rgba.pixels) > 400);
            println!(
                "Game editor smoke passed: same world, admitted keyboard/relative mouse, panel close/reopen/resize, pause and recovered viewport pixels"
            );
            world.write_message(AppExit::Success);
            world.resource_mut::<Smoke>().phase = 9;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn losing_viewport_focus_blocks_both_input_domains() {
        let mut world = World::new();
        world.insert_resource(ViewportInput {
            drawn: true,
            keyboard: true,
            pointer: true,
            ..default()
        });
        world.insert_resource(DesktopCapture::default());
        admit_input(&mut world);
        assert!(!world.resource::<DesktopCapture>().keyboard);
        world.resource_mut::<ViewportInput>().keyboard = false;
        world.resource_mut::<ViewportInput>().pointer = false;
        admit_input(&mut world);
        assert!(world.resource::<DesktopCapture>().keyboard);
        assert!(world.resource::<DesktopCapture>().pointer);
    }
}
