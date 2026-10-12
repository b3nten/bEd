//! Native proof that the desktop chrome preserves Bevy's Window configuration.
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
    entered: u64,
    deadline: Instant,
    verified: Arc<AtomicBool>,
}

fn main() {
    let verified = Arc::new(AtomicBool::new(false));
    let mut app = App::new();
    app.insert_resource(DesktopNativePresentation {
        toolbar: vec![DesktopToolbarItem {
            id: "proof.command".into(),
            label: "Proof".into(),
            icon: None,
            enabled: true,
        }],
        ..default()
    })
    .add_plugins(DesktopPlugins {
        window: Window {
            title: "Desktop window configuration proof".into(),
            resolution: (420, 260).into(),
            visible: false,
            focused: false,
            resizable: false,
            decorations: false,
            ..default()
        },
        ..default()
    })
    .insert_resource(Proof {
        phase: 0,
        entered: 0,
        deadline: Instant::now() + Duration::from_secs(30),
        verified: Arc::clone(&verified),
    })
    .insert_resource(DesktopUi(|_, ui| ui.text("Window configuration proof")))
    .add_systems(PreUpdate, advance.in_set(DesktopSystems::AfterUi));
    assert_eq!(app.run(), AppExit::Success);
    assert!(
        verified.load(Ordering::SeqCst),
        "window proof exited before completion"
    );
    println!("desktop window configuration smoke PASS");
}

fn advance(world: &mut World) {
    let mut proof = world.remove_resource::<Proof>().unwrap();
    assert!(
        Instant::now() < proof.deadline,
        "window proof timed out at phase {}",
        proof.phase
    );
    assert!(
        world.resource::<DesktopNativeErrors>().0.is_empty(),
        "native chrome errors"
    );
    let frame = world.resource::<DesktopFrameInfo>().frame.0;
    let primary = world
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(world)
        .expect("one primary window");
    let actual = bevy::winit::WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        windows.get_window(primary).map(|window| {
            (
                window.is_visible(),
                window.is_decorated(),
                window.is_resizable(),
                window.has_focus(),
            )
        })
    });
    let Some((visible, decorated, resizable, focused)) = actual else {
        world.insert_resource(proof);
        return;
    };
    #[cfg(target_os = "macos")]
    let controls = {
        let native = world.non_send::<DesktopNativePlatform>();
        let Some(chrome) = native.window.as_ref() else {
            world.insert_resource(proof);
            return;
        };
        assert!(chrome.preserves_winit_content_view());
        if !decorated {
            chrome.validate_control_layout().unwrap();
            assert!(!chrome.click_command("proof.command"));
        } else {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            bevy::winit::WINIT_WINDOWS.with(|windows| {
                let windows = windows.borrow();
                let RawWindowHandle::AppKit(handle) = windows
                    .get_window(primary)
                    .unwrap()
                    .window_handle()
                    .unwrap()
                    .as_raw()
                else {
                    unreachable!()
                };
                let view = unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
                assert_eq!(
                    view.window()
                        .unwrap()
                        .titlebarAccessoryViewControllers()
                        .len(),
                    1,
                    "native chrome retained an obsolete accessory"
                );
            });
        }
        chrome.control_frames().len()
    };
    match proof.phase {
        0 if frame >= 4 => {
            assert_eq!(visible, Some(false), "chrome revealed a hidden window");
            assert!(!decorated && !resizable && !focused);
            assert_eq!(world.resource::<DesktopContentInsets>().top, 0.0);
            #[cfg(target_os = "macos")]
            assert_eq!(controls, 0);
            world.get_mut::<Window>(primary).unwrap().decorations = true;
            proof.phase = 1;
        }
        1 if decorated => {
            assert_eq!(
                visible,
                Some(false),
                "adding chrome revealed a hidden window"
            );
            assert!(!resizable && !focused, "chrome changed window capabilities");
            #[cfg(target_os = "macos")]
            {
                assert_eq!(controls, 1);
                assert!(world.resource::<DesktopContentInsets>().top > 0.0);
            }
            world.get_mut::<Window>(primary).unwrap().visible = true;
            proof.phase = 2;
        }
        2 if visible == Some(true) => {
            assert!(decorated && !resizable);
            bevy::winit::WINIT_WINDOWS.with(|windows| {
                windows.borrow().get_window(primary).unwrap().focus_window();
            });
            proof.entered = world
                .resource::<DesktopRenderStatus>()
                .read()
                .completed_frames;
            proof.phase = 3;
        }
        3 if world
            .resource::<DesktopRenderStatus>()
            .read()
            .completed_frames
            >= proof.entered + 2 =>
        {
            assert!(decorated && !resizable);
            world.get_mut::<Window>(primary).unwrap().decorations = false;
            proof.phase = 4;
        }
        4 if !decorated => {
            assert!(!resizable);
            assert_eq!(world.resource::<DesktopContentInsets>().top, 0.0);
            #[cfg(target_os = "macos")]
            assert_eq!(controls, 0);
            world.get_mut::<Window>(primary).unwrap().visible = false;
            proof.entered = frame;
            proof.phase = 5;
        }
        5 if frame >= proof.entered + 4 => {
            assert_eq!(
                visible,
                Some(false),
                "later preparation revealed the window"
            );
            assert!(!decorated && !resizable);
            world.get_mut::<Window>(primary).unwrap().decorations = true;
            proof.phase = 6;
        }
        6 if decorated => {
            assert_eq!(visible, Some(false));
            assert!(!resizable && !focused);
            #[cfg(target_os = "macos")]
            assert_eq!(controls, 1);
            proof.verified.store(true, Ordering::SeqCst);
            world.write_message(AppExit::Success);
            proof.phase = 7;
        }
        _ => {}
    }
    world.insert_resource(proof);
}
