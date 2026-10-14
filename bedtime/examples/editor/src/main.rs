//! Text editing and shared undo without bEd configuration or a workspace.
use bedtime::core;
use bedtime_plugin_editor::EditorPlugin;
use core::api::{documents::PanelBinding, features::HostAction};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use {
    core::bedtime_documents::ByteEdit, core::bedtime_documents::DocumentId,
    core::bedtime_documents::DocumentSession, core::bevy::prelude::*,
};

const CONTENT: &[u8] = b"A shared document.\nEdit either view; both use the same history.\n";
#[derive(Resource)]
struct SharedDocument(DocumentId);
#[derive(Resource)]
struct SmokeFile(PathBuf);
impl Drop for SmokeFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn smoke_file() -> SmokeFile {
    let path = std::env::temp_dir().join(format!(
        "bed-editor-example-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, CONTENT).unwrap();
    SmokeFile(path)
}

fn start(world: &mut World) {
    let path = world.get_resource::<SmokeFile>().map(|file| file.0.clone());
    let document = match path {
        Some(path) => world
            .non_send_mut::<DocumentSession>()
            .open_file(&path)
            .unwrap(),
        None => world
            .non_send_mut::<DocumentSession>()
            .create_document(CONTENT)
            .unwrap(),
    };
    world.insert_resource(SharedDocument(document));
    let first = core::open_document_panel(
        world,
        bedtime_plugin_editor::TEXT_PANEL_TYPE,
        document,
        Value::Null,
        None,
    )
    .unwrap();
    let split = world
        .resource_mut::<core::bedtime_workbench::WorkbenchRequests>()
        .submit(core::api::workbench::WorkbenchAction::SplitPanel {
            panel: first,
            axis: 0,
            high: true,
        });
    core::resolve_documents(world);
    let second = match world
        .resource_mut::<core::bedtime_workbench::WorkbenchRequests>()
        .take_result(split)
    {
        Some(Ok(core::bedtime_workbench::WorkbenchOutcome::Opened(panel))) => panel,
        outcome => panic!("Splitting must open a second document view: {outcome:?}"),
    };
    assert_ne!(first, second);
}

fn request(world: &mut World, source: Entity, request: HostAction) -> core::HostOutcome {
    let id = core::submit_host_request(world, Some(source), request);
    core::resolve_documents(world);
    core::take_host_result(world, id)
        .expect("synchronous local request completes")
        .unwrap()
}

fn exercise(world: &mut World) {
    let document = world.resource::<SharedDocument>().0;
    let panels = world
        .query_filtered::<(Entity, &PanelBinding), With<core::api::workbench::PanelPresentation>>()
        .iter(world)
        .filter(|(_, binding)| binding.document == Some(document))
        .map(|(entity, binding)| (entity, binding.view.unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(panels.len(), 2);
    assert_ne!(panels[0].1, panels[1].1);
    let first_revision = world
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    request(
        world,
        panels[0].0,
        HostAction::ApplyEdits {
            document,
            revision: first_revision,
            edits: vec![ByteEdit {
                range: 0..0,
                bytes: b"First ".to_vec(),
            }],
        },
    );
    let revision = world
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    request(
        world,
        panels[1].0,
        HostAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..0,
                bytes: b"Second ".to_vec(),
            }],
        },
    );
    let edited = world
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap()
        .bytes;
    assert!(edited.starts_with(b"Second First "));
    request(world, panels[0].0, HostAction::Undo { document });
    assert!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes
            .starts_with(b"First ")
    );
    request(world, panels[1].0, HostAction::Redo { document });
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        edited
    );
    let revision = world
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    assert_eq!(
        request(world, panels[1].0, HostAction::Save { document }),
        core::HostOutcome::Saved {
            documents: vec![document],
            revisions: vec![revision]
        }
    );
    assert!(
        !world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
    let path = world.resource::<SmokeFile>().0.clone();
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    // Give the presentations different carets; restoring the layout reopens one
    // canonical document and restores each presentation's independent view facts.
    world
        .non_send_mut::<DocumentSession>()
        .with_commands(panels[0].1, |commands| {
            commands.set_cursor(
                0,
                1,
                false,
                bedtime_editing::text_commands::CursorReveal::Ensure,
            )
        })
        .unwrap();
    world
        .non_send_mut::<DocumentSession>()
        .with_commands(panels[1].1, |commands| {
            commands.set_cursor(
                1,
                3,
                false,
                bedtime_editing::text_commands::CursorReveal::Ensure,
            )
        })
        .unwrap();
    let saved = core::bedtime_workbench::save(world);
    core::bedtime_workbench::restore(world, &saved).unwrap();
    // The ordinary frame publishes each restored frontend's binding to its
    // logical panel. This headless path explicitly completes that same stage.
    core::publish_documents(world);
    let bindings = world
        .query_filtered::<&PanelBinding, With<core::api::workbench::PanelPresentation>>()
        .iter(world)
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 2);
    assert!(
        bindings
            .iter()
            .all(|binding| binding.document == Some(document))
    );
    assert_eq!(world.non_send::<DocumentSession>().view_count(document), 2);
    let mut positions = bindings
        .iter()
        .map(|binding| {
            let view = world
                .non_send::<DocumentSession>()
                .view_snapshot(binding.view.unwrap())
                .unwrap();
            (view.row, view.column)
        })
        .collect::<Vec<_>>();
    positions.sort_unstable();
    assert_eq!(positions, vec![(0, 1), (1, 3)]);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        edited
    );
}

#[derive(Resource)]
struct Smoke {
    started: Instant,
    exercised: bool,
    completed_at: u64,
    capture: Option<core::bedtime_desktop::DesktopCaptureId>,
    verified: Arc<AtomicBool>,
}
fn smoke(world: &mut World) {
    if world.resource::<Smoke>().started.elapsed() >= Duration::from_secs(30) {
        let status = world
            .resource::<core::bedtime_desktop::DesktopRenderStatus>()
            .read();
        let frame = world.resource::<core::bedtime_desktop::DesktopFrameInfo>();
        panic!(
            "editor smoke timed out: presented {}, UI {}, GPU {}, render error {:?}",
            status.completed_frames, frame.frame.0, frame.generation, status.error
        );
    }
    // A smoke window must present even when launched behind the active app.
    if !world.resource::<Smoke>().exercised {
        let primary = world
            .query_filtered::<Entity, With<core::bevy::window::PrimaryWindow>>()
            .iter(world)
            .next();
        core::bevy::winit::WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            if let Some(window) = primary.and_then(|entity| windows.get_window(entity)) {
                window.focus_window();
            }
        });
    }
    let frames = world
        .resource::<core::bedtime_desktop::DesktopRenderStatus>()
        .read()
        .completed_frames;
    if !world.resource::<Smoke>().exercised && frames >= 3 {
        exercise(world);
        let mut state = world.resource_mut::<Smoke>();
        state.exercised = true;
        state.completed_at = frames;
    } else if world.resource::<Smoke>().exercised
        && frames >= world.resource::<Smoke>().completed_at + 6
    {
        let Some(capture) = world.resource::<Smoke>().capture else {
            let panel = world
                .resource::<core::bedtime_workbench::WorkbenchLayout>()
                .focused
                .unwrap();
            let capture = world
                .resource_mut::<core::bedtime_desktop::DesktopCaptureRequests>()
                .submit(
                    vec![
                        core::bedtime_desktop::DesktopCaptureTarget::Window,
                        core::bedtime_desktop::DesktopCaptureTarget::Panel(panel),
                    ],
                    Instant::now() + Duration::from_secs(10),
                    None,
                );
            world.resource_mut::<Smoke>().capture = Some(capture);
            return;
        };
        let Some(result) = world
            .resource_mut::<core::bedtime_desktop::DesktopCaptureResults>()
            .take(capture)
        else {
            return;
        };
        let frame = result.expect("editor window and panel GPU capture");
        assert!(
            frame.images.iter().all(|image| image
                .rgba
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3].iter().any(|channel| *channel > 20))),
            "editor capture was blank"
        );
        std::fs::create_dir_all("target").unwrap();
        frame.images[0]
            .rgba
            .write_png("target/editor-smoke.png")
            .unwrap();
        world
            .resource::<Smoke>()
            .verified
            .store(true, Ordering::SeqCst);
        println!(
            "Editor smoke passed: shared history, save receipt, layout restore and native window/panel capture"
        );
        world.write_message(AppExit::Success);
    }
}

fn headless_smoke() {
    let mut app = App::new();
    app.add_plugins((core::DocumentHostPlugin, EditorPlugin));
    app.insert_resource(smoke_file());
    start(app.world_mut());
    exercise(app.world_mut());
    println!("Headless editor smoke passed");
}

fn main() -> std::io::Result<()> {
    if std::env::args().any(|argument| argument == "--headless-smoke") {
        headless_smoke();
        return Ok(());
    }
    let smoke_test = std::env::args().any(|argument| argument == "--smoke-test");
    let launch = match bedtime::awaken("example.editor", "Shared document editor")?.dispatch()? {
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let options = bedtime::DesktopOptions {
        config_directory: smoke_test.then(|| {
            std::env::temp_dir().join(format!("bed-editor-example-config-{}", std::process::id()))
        }),
        ..Default::default()
    };
    let verified = Arc::<AtomicBool>::default();
    let mut app = launch.build(options, |app| {
        app.add_plugins(EditorPlugin).add_systems(Startup, start);
        if smoke_test {
            app.insert_resource(smoke_file())
                .insert_resource(Smoke {
                    started: Instant::now(),
                    exercised: false,
                    completed_at: 0,
                    capture: None,
                    verified: Arc::clone(&verified),
                })
                .add_systems(Update, smoke);
        }
        Ok(())
    })?;
    let exit = app.run();
    if smoke_test {
        assert!(
            verified.load(Ordering::SeqCst),
            "editor smoke stopped before completion"
        );
    }
    if exit.is_error() {
        return Err(std::io::Error::other("Editor example exited with an error"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn plain_editor_panels_share_history_save_and_restore_without_ide_services() {
        super::headless_smoke();
    }

    #[test]
    fn panel_action_undo_finishes_before_the_next_save() {
        use super::*;
        let mut app = App::new();
        app.add_plugins((core::DocumentHostPlugin, EditorPlugin))
            .insert_resource(smoke_file());
        start(app.world_mut());
        let world = app.world_mut();
        let document = world.resource::<SharedDocument>().0;
        let source = world
            .resource::<core::bedtime_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        let revision = core::document_revision(world, document).unwrap();
        request(
            world,
            source,
            HostAction::ApplyEdits {
                document,
                revision,
                edits: vec![ByteEdit {
                    range: 0..0,
                    bytes: b"changed ".to_vec(),
                }],
            },
        );
        let undo = core::submit_host_request(
            world,
            Some(source),
            HostAction::ShowPanel {
                panel_type: bedtime_plugin_editor::TEXT_PANEL_TYPE.into(),
                document: Some(document),
                state: Value::Null,
                action: Some(core::api::workbench::PanelAction::Undo),
            },
        );
        let save = core::submit_host_request(world, Some(source), HostAction::Save { document });
        core::resolve_documents(world);
        assert!(core::take_host_result(world, undo).unwrap().is_ok());
        assert!(core::take_host_result(world, save).unwrap().is_ok());
        assert_eq!(
            std::fs::read(&world.resource::<SmokeFile>().0).unwrap(),
            CONTENT
        );
        assert_eq!(
            world
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            CONTENT
        );
    }
}
