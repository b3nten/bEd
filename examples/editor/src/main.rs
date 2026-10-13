//! Text editing and shared undo without bEd configuration or a workspace.
use bed_core::api::{documents::PanelBinding, features::HostAction};
use bed_plugin_editor::EditorPlugin;
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use {
    bed_core::BedCorePlugin, bed_core::bed_documents::ByteEdit,
    bed_core::bed_documents::DocumentId, bed_core::bed_documents::DocumentSession,
    bed_core::bevy::prelude::*,
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
    bed_core::open_document_panel(
        world,
        bed_plugin_editor::TEXT_PANEL_TYPE,
        document,
        Value::Null,
        None,
    )
    .unwrap();
    let second = bed_core::open_document_panel(
        world,
        bed_plugin_editor::TEXT_PANEL_TYPE,
        document,
        Value::Null,
        None,
    )
    .unwrap();
    world
        .resource_mut::<bed_core::bed_workbench::WorkbenchRequests>()
        .push(bed_core::api::workbench::WorkbenchAction::SplitPanel {
            panel: second,
            axis: 0,
            high: true,
        });
    bed_core::resolve_documents(world);
}

fn request(world: &mut World, source: Entity, request: HostAction) -> bed_core::HostOutcome {
    let id = bed_core::submit_host_request(world, Some(source), request);
    bed_core::resolve_documents(world);
    bed_core::take_host_result(world, id)
        .expect("synchronous local request completes")
        .unwrap()
}

fn exercise(world: &mut World) {
    let document = world.resource::<SharedDocument>().0;
    let panels = world
        .query::<(Entity, &PanelBinding)>()
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
        bed_core::HostOutcome::Saved {
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
                bed_editing::text_commands::CursorReveal::Ensure,
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
                bed_editing::text_commands::CursorReveal::Ensure,
            )
        })
        .unwrap();
    let saved = bed_core::bed_workbench::save(world);
    bed_core::bed_workbench::restore(world, &saved).unwrap();
    let bindings = world
        .query::<&PanelBinding>()
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
}
fn smoke(world: &mut World) {
    assert!(
        world.resource::<Smoke>().started.elapsed() < Duration::from_secs(30),
        "editor smoke timed out"
    );
    let frames = world
        .resource::<bed_core::bed_desktop::DesktopRenderStatus>()
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
        println!(
            "Editor smoke passed: two text views, shared edits, undo/redo, revision save receipt and layout restore"
        );
        world.write_message(AppExit::Success);
    }
}

fn headless_smoke() {
    let mut app = App::new();
    app.add_plugins((bed_core::DocumentHostPlugin, EditorPlugin));
    app.insert_resource(smoke_file());
    start(app.world_mut());
    exercise(app.world_mut());
    println!("Headless editor smoke passed");
}

fn main() {
    if std::env::args().any(|argument| argument == "--headless-smoke") {
        headless_smoke();
        return;
    }
    let smoke_test = std::env::args().any(|argument| argument == "--smoke-test");
    let mut app = App::new();
    app.add_plugins((BedCorePlugin::default(), EditorPlugin))
        .add_systems(Startup, start);
    if smoke_test {
        app.insert_resource(smoke_file())
            .insert_resource(Smoke {
                started: Instant::now(),
                exercised: false,
                completed_at: 0,
            })
            .add_systems(Update, smoke);
    }
    app.run();
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
        app.add_plugins((bed_core::DocumentHostPlugin, EditorPlugin))
            .insert_resource(smoke_file());
        start(app.world_mut());
        let world = app.world_mut();
        let document = world.resource::<SharedDocument>().0;
        let source = world
            .resource::<bed_core::bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        let revision = bed_core::document_revision(world, document).unwrap();
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
        let undo = bed_core::submit_host_request(
            world,
            Some(source),
            HostAction::ShowPanel {
                panel_type: bed_plugin_editor::TEXT_PANEL_TYPE.into(),
                document: Some(document),
                state: Value::Null,
                action: Some(bed_core::api::workbench::PanelAction::Undo),
            },
        );
        let save =
            bed_core::submit_host_request(world, Some(source), HostAction::Save { document });
        bed_core::resolve_documents(world);
        assert!(bed_core::take_host_result(world, undo).unwrap().is_ok());
        assert!(bed_core::take_host_result(world, save).unwrap().is_ok());
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
