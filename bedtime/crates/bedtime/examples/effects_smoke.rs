#[path = "../../../tests/support/native_smoke.rs"]
mod support;
use support::*;

fn main() -> io::Result<()> {
    let verified = Arc::<AtomicBool>::default();
    let launch = match bedtime::awaken("example.effects", "effects smoke")?.dispatch()? {
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
    };
    let options = bedtime::DesktopOptions {
        config_directory: Some(
            std::env::temp_dir().join(format!("bed-effects-config-{}", std::process::id())),
        ),
        desktop: bedtime_desktop::DesktopPlugins {
            window: Window {
                resolution: (1200, 800).into(),
                ..default()
            },
            ..default()
        },
        ..Default::default()
    };
    let exit = launch.run(options, |app| {
        app.add_plugins((
            bedtime_plugin_effects::EffectsPlugin,
            bedtime_plugin_automation::AutomationPlugin,
        ))
        .insert_resource(Smoke {
            verified: Arc::clone(&verified),
            ..Default::default()
        })
        .add_systems(Last, advance.before(bevy::app::OnAppExitSystems));
        Ok(())
    })?;
    ensure(
        verified.load(Ordering::SeqCst),
        "native smoke exited before completing its assertions",
    )?;
    if exit.is_error() {
        return Err(io::Error::other("effects smoke failed"));
    }
    Ok(())
}
fn advance(world: &mut World) {
    world.resource_scope(|world, mut smoke: Mut<Smoke>| {
        let result = (|| -> io::Result<bool> {
            ensure(
                smoke.started.elapsed() < Duration::from_secs(90),
                format!("effects smoke timed out in phase {}", smoke.phase),
            )?;
            if !world.contains_resource::<bedtime_desktop::DesktopGpu>() {
                return Ok(false);
            }
            smoke.poll(world)?;
            if smoke.snapshot.is_none() {
                return Ok(false);
            }
            if presented(world) < 2 {
                focus_window(world);
                return Ok(false);
            }
            effects(world, &mut smoke)
        })();
        match result {
            Ok(true) => {
                smoke.verified.store(true, Ordering::SeqCst);
                eprintln!("effects smoke PASS");
                world.write_message(AppExit::Success);
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("effects smoke failed: {error}");
                world.write_message(AppExit::error());
            }
        }
    });
}
fn effects(world: &mut World, smoke: &mut Smoke) -> io::Result<bool> {
    if smoke.phase == 0 {
        world.write_message(bedtime_api::settings::SettingsRequest::new(
            "bedtime.effects",
            "effect_preset",
            "sharp",
        ));
        smoke.next(world);
        return Ok(false);
    }
    if smoke.elapsed_frames(world) < 3 {
        return Ok(false);
    }
    ensure(
        world
            .resource::<bedtime_plugin_effects::desktop::EffectsState>()
            .settings
            .enabled,
        "preset was not applied",
    )?;
    if let Some(frame) = smoke.capture(world, vec![CaptureTarget::Window])? {
        ensure(
            visible_pixels(&frame.images[0].rgba.pixels) > 1000,
            "effects capture is blank",
        )?;
        std::fs::create_dir_all("target")?;
        frame.images[0].rgba.write_png("target/effects-smoke.png")?;
        return Ok(true);
    }
    Ok(false)
}
