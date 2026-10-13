//! Native resize, minimize/restore and focus proof without application policy.
use bed_desktop::{bevy::prelude::*, *};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Resource)]
struct Proof {
    phase: u8,
    started: Instant,
    entered: u64,
    size: [u32; 2],
    restore_at: Option<Instant>,
    lost_focus: bool,
    gained_focus: bool,
    capture: Option<DesktopCaptureId>,
    verified: Arc<AtomicBool>,
}
fn window<T>(world: &mut World, callback: impl FnOnce(&winit::window::Window) -> T) -> T {
    let entity = world
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(world)
        .expect("one primary window");
    bevy::winit::WINIT_WINDOWS
        .with(|windows| callback(windows.borrow().get_window(entity).expect("native window")))
}
fn main() -> std::io::Result<()> {
    let verified = Arc::new(AtomicBool::new(false));
    let launch = match bed_launch::builder("example.desktop.lifecycle", "Desktop lifecycle proof")?
        .dispatch()?
    {
        bed_launch::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bed_launch::LaunchOutcome::Desktop(launch) => *launch,
    };
    let mut app = launch.build(
        bed_launch::DesktopOptions {
            config_directory: Some(
                std::env::temp_dir().join(format!("bed-lifecycle-proof-{}", std::process::id())),
            ),
            presentation_mode: bed_launch::bed_core::CorePresentationMode::Application,
            ..Default::default()
        },
        |app| {
            app.insert_resource(DesktopUi(|_, ui| ui.text("Native lifecycle proof")))
                .insert_resource(Proof {
                    phase: 0,
                    started: Instant::now(),
                    entered: 0,
                    size: [0; 2],
                    restore_at: None,
                    lost_focus: false,
                    gained_focus: false,
                    capture: None,
                    verified: Arc::clone(&verified),
                })
                .add_systems(Last, advance);
            Ok(())
        },
    )?;
    assert_eq!(app.run(), AppExit::Success);
    assert!(
        verified.load(Ordering::SeqCst),
        "lifecycle proof exited before completion"
    );
    println!("desktop lifecycle smoke PASS");
    Ok(())
}
fn advance(world: &mut World) {
    let mut proof = world.remove_resource::<Proof>().unwrap();
    assert!(
        proof.started.elapsed() < Duration::from_secs(45),
        "lifecycle proof timed out in phase {}",
        proof.phase
    );
    for event in &world.resource::<DesktopInputEvents>().0 {
        if let winit::event::WindowEvent::Focused(focused) = event.event.event {
            proof.lost_focus |= !focused;
            proof.gained_focus |= focused;
        }
    }
    let presented = world
        .resource::<DesktopRenderStatus>()
        .read()
        .completed_frames;
    match proof.phase {
        0 if presented >= 2 => {
            proof.size = window(world, |window| {
                window.focus_window();
                let size = winit::dpi::LogicalSize::new(840., 600.)
                    .to_physical::<u32>(window.scale_factor());
                let _ = window.request_inner_size(size);
                [size.width, size.height]
            });
            proof.entered = presented;
            proof.phase = 1;
        }
        1 if presented >= proof.entered + 2
            && window(world, |window| {
                [window.inner_size().width, window.inner_size().height] == proof.size
                    && window.has_focus()
            }) =>
        {
            window(world, |window| window.set_minimized(true));
            proof.restore_at = Some(Instant::now() + Duration::from_millis(700));
            proof.gained_focus = false;
            proof.phase = 2;
        }
        2 if Instant::now() >= proof.restore_at.unwrap() => {
            assert!(
                window(world, |window| window.is_minimized() == Some(true)),
                "native minimize not observed"
            );
            assert!(proof.lost_focus, "minimize did not report focus loss");
            window(world, |window| {
                window.set_minimized(false);
                window.focus_window();
            });
            proof.entered = presented;
            proof.phase = 3;
        }
        3 => {
            if !window(world, |window| window.has_focus()) {
                window(world, |window| window.focus_window());
            }
            if window(world, |window| {
                window.is_minimized() == Some(false) && window.has_focus()
            }) && proof.gained_focus
                && presented >= proof.entered + 2
            {
                proof.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![DesktopCaptureTarget::Window],
                    Instant::now() + Duration::from_secs(10),
                    None,
                ));
                proof.phase = 4;
            }
        }
        4 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(proof.capture.unwrap())
            {
                let frame = result.expect("capture after restore");
                assert!(
                    frame.images[0]
                        .rgba
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|pixel| pixel[..3].iter().any(|value| *value > 20)),
                    "restored desktop capture blank"
                );
                std::fs::create_dir_all("target").unwrap();
                frame.images[0]
                    .rgba
                    .write_ppm("target/native-window.ppm")
                    .unwrap();
                proof.verified.store(true, Ordering::SeqCst);
                world.write_message(AppExit::Success);
                proof.phase = 5;
            }
        }
        _ => {}
    }
    world.insert_resource(proof);
}
