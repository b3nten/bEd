//! Attach to an existing Bevy app, keeping its window policy and recovery owner.
use bed_desktop::*;
use bevy::{
    prelude::*,
    render::{
        error_handler::{ErrorType, RenderErrorHandler, RenderErrorPolicy},
        pipelined_rendering::PipelinedRenderingPlugin,
        settings::WgpuSettings,
    },
};
use std::time::{Duration, Instant};
#[derive(Resource)]
struct Proof {
    phase: u8,
    host_recoveries: u32,
    deadline: Instant,
    capture: Option<DesktopCaptureId>,
}
fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Existing-app attachment proof".into(),
                    resolution: (640, 420).into(),
                    ..default()
                }),
                close_when_requested: false,
                ..default()
            })
            .disable::<PipelinedRenderingPlugin>(),
    );
    app.insert_resource(Proof {
        phase: 0,
        host_recoveries: 0,
        deadline: Instant::now() + Duration::from_secs(30),
        capture: None,
    });
    app.insert_resource(RenderErrorHandler(|error, world, _| {
        if error.ty == ErrorType::DeviceLost {
            world.resource_mut::<Proof>().host_recoveries += 1;
            RenderErrorPolicy::Recover(
                WgpuSettings {
                    device_label: Some("Attachment host replacement".into()),
                    ..default()
                }
                .into(),
            )
        } else {
            world.write_message(AppExit::error());
            RenderErrorPolicy::StopRendering
        }
    }));
    app.add_plugins(DesktopIntegrationPlugin)
        .insert_resource(DesktopUi(draw))
        .add_systems(Update, advance);
    assert_eq!(app.get_added_plugins::<WindowPlugin>().len(), 1);
    assert!(!app.get_added_plugins::<WindowPlugin>()[0].close_when_requested);
    assert!(!app.world().contains_resource::<BedMemoryAssets>());
    app.run();
}
fn draw(_: &mut World, ui: &dear_imgui_rs::Ui) {
    ui.get_background_draw_list()
        .add_rect([40.0, 40.0], [350.0, 230.0], [0.1, 0.8, 0.3, 1.0])
        .filled(true)
        .build();
}
fn advance(world: &mut World) {
    assert!(
        Instant::now() < world.resource::<Proof>().deadline,
        "attachment smoke timed out"
    );
    let status = world.resource::<DesktopRenderStatus>().read();
    assert!(status.error.is_none(), "{:?}", status.error);
    match world.resource::<Proof>().phase {
        0 if status.completed_frames >= 12 => {
            assert_eq!(
                world
                    .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
                    .iter(world)
                    .count(),
                1
            );
            world.resource::<DesktopGpu>().device.destroy();
            world.resource_mut::<Proof>().phase = 1;
        }
        1 if status.recoveries == 1 && !status.recovering && status.completed_frames >= 16 => {
            assert_eq!(world.resource::<Proof>().host_recoveries, 1);
            let info = world.resource::<DesktopFrameInfo>().clone();
            assert!(info.generation >= 2);
            let deadline = world.resource::<Proof>().deadline;
            let id = world.resource_mut::<DesktopCaptureRequests>().submit(
                vec![DesktopCaptureTarget::Window],
                deadline,
                Some(info.frame),
            );
            world.resource_mut::<Proof>().capture = Some(id);
            world.resource_mut::<Proof>().phase = 2;
        }
        2 => {
            let id = world.resource::<Proof>().capture.unwrap();
            let Some(result) = world.resource_mut::<DesktopCaptureResults>().take(id) else {
                return;
            };
            let captured = result.unwrap();
            assert!(
                captured.images[0]
                    .rgba
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[1] > 180 && pixel[0] < 60 && pixel[2] < 120)
            );
            println!(
                "Desktop attachment smoke passed: one window, preserved close policy and host recovery callback, fresh captured pixels"
            );
            world.write_message(AppExit::Success);
            world.resource_mut::<Proof>().phase = 3;
        }
        _ => {}
    }
}
