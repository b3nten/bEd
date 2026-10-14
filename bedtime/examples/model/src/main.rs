//! A typed model with two views and core document/history coordination.
use bedtime::core;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use {
    core::DocumentBinding, core::DocumentModel, core::api::workbench::*,
    core::bedtime_documents::DocumentId, core::bevy::prelude::*,
};

#[derive(Clone)]
struct Graph {
    nodes: Vec<[f32; 2]>,
}
#[derive(Clone)]
struct MoveNode {
    node: usize,
    position: [f32; 2],
}
impl DocumentModel for Graph {
    type Change = MoveNode;
    fn apply(&mut self, change: &MoveNode) -> Result<MoveNode, String> {
        if !change.position.iter().all(|value| value.is_finite()) {
            return Err("Node positions must be finite".into());
        }
        let position = self
            .nodes
            .get_mut(change.node)
            .ok_or("Node is unavailable")?;
        let inverse = MoveNode {
            node: change.node,
            position: *position,
        };
        *position = change.position;
        Ok(inverse)
    }
    fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.nodes).map_err(|error| error.to_string())
    }
}
struct GraphPanel {
    entity: Entity,
    document: DocumentId,
}
impl Panel for GraphPanel {
    fn title(&self, _: &World) -> String {
        "Graph document".into()
    }
    fn draw(
        &mut self,
        ui: &core::bedtime_desktop::dear_imgui_rs::Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        _: &ShortcutInput,
    ) {
        let graph = core::model::<Graph>(world, self.document).unwrap();
        let position = graph.nodes[0];
        let info = core::model_document(world, self.document).unwrap();
        let revision = info.revision();
        ui.text(format!("Node 0: {:.0}, {:.0}", position[0], position[1]));
        ui.text(format!("Revision: {revision}   Changed: {}", info.dirty()));
        let command = if ui.button("Move node right") {
            Some((
                "example.model.move",
                json!({"document":self.document.0,"revision":revision,"x":position[0]+10.0}),
            ))
        } else if ui.button("Undo") {
            Some(("example.model.undo", json!({"document":self.document.0})))
        } else if ui.button("Redo") {
            Some(("example.model.redo", json!({"document":self.document.0})))
        } else {
            None
        };
        if let Some((command, arguments)) = command {
            let mut invocation =
                core::bedtime_workbench::capture_command(world, command, arguments);
            invocation.source = Some(self.entity);
            requests.push(WorkbenchAction::Invoke(invocation));
        }
    }
}
fn create(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    let document = DocumentId(
        input.state["document"]
            .as_u64()
            .ok_or("Missing model document")?,
    );
    if core::model::<Graph>(world, document).is_none() {
        return Err("Model document is unavailable".into());
    }
    world
        .entity_mut(input.entity)
        .insert(DocumentBinding(document));
    Ok(Box::new(GraphPanel {
        entity: input.entity,
        document,
    }))
}
fn move_node(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = DocumentId(
        input.arguments["document"]
            .as_u64()
            .ok_or("Missing document")?,
    );
    let revision = input.arguments["revision"]
        .as_u64()
        .ok_or("Missing revision")?;
    let x = input.arguments["x"].as_f64().ok_or("Missing position")? as f32;
    core::edit_model::<Graph>(
        world,
        document,
        revision,
        MoveNode {
            node: 0,
            position: [x, 0.0],
        },
        "Move node",
    )?;
    Ok(Vec::new())
}
fn undo(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    core::undo_document(
        world,
        DocumentId(
            input.arguments["document"]
                .as_u64()
                .ok_or("Missing document")?,
        ),
    )?;
    Ok(Vec::new())
}
fn redo(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    core::redo_document(
        world,
        DocumentId(
            input.arguments["document"]
                .as_u64()
                .ok_or("Missing document")?,
        ),
    )?;
    Ok(Vec::new())
}
struct GraphPlugin;
impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        register(app.world_mut(), "example.model", |registrar| {
            registrar.panel("example.model.panel", "Graph", create);
            registrar.command("example.model.move", "Move node", move_node);
            registrar.command("example.model.undo", "Undo model change", undo);
            registrar.command("example.model.redo", "Redo model change", redo);
        })
        .unwrap();
        app.add_systems(Startup, start);
    }
}
#[derive(Resource)]
struct SharedGraph(DocumentId);
fn start(world: &mut World) {
    let document = core::create_model_document(
        world,
        Graph {
            nodes: vec![[0.0, 0.0], [100.0, 100.0]],
        },
        "Graph",
        None,
    );
    world.insert_resource(SharedGraph(document));
    for _ in 0..2 {
        core::bedtime_workbench::open_panel(
            world,
            json!({"panel_type":"example.model.panel","state":{"document":document.0}}),
            None,
            None,
        )
        .unwrap();
    }
}
#[derive(Resource)]
struct Smoke {
    phase: u8,
    started: Instant,
    save: Option<core::HostRequestId>,
    capture: Option<core::bedtime_desktop::DesktopCaptureId>,
    verified: Arc<AtomicBool>,
}
fn smoke(world: &mut World) {
    assert!(
        world.resource::<Smoke>().started.elapsed() < Duration::from_secs(30),
        "model smoke timed out"
    );
    let frames = world
        .resource::<core::bedtime_desktop::DesktopRenderStatus>()
        .read()
        .completed_frames;
    let document = world.resource::<SharedGraph>().0;
    let phase = world.resource::<Smoke>().phase;
    if frames < u64::from(phase + 1) * 3 {
        return;
    }
    match phase {
        0 => {
            let invocation = core::bedtime_workbench::capture_command(
                world,
                "example.model.move",
                json!({"document":document.0,"revision":0,"x":10}),
            );
            world
                .resource_mut::<core::bedtime_workbench::WorkbenchRequests>()
                .push(WorkbenchAction::Invoke(invocation));
        }
        1 => {
            assert_eq!(
                core::model::<Graph>(world, document).unwrap().nodes[0],
                [10.0, 0.0]
            );
            core::undo_document(world, document).unwrap();
        }
        2 => {
            assert_eq!(
                core::model::<Graph>(world, document).unwrap().nodes[0],
                [0.0, 0.0]
            );
            assert!(!core::model_document(world, document).unwrap().dirty());
            core::redo_document(world, document).unwrap();
        }
        3 => {
            let panel = world
                .resource::<core::bedtime_workbench::WorkbenchLayout>()
                .focused
                .unwrap();
            world
                .resource_mut::<core::bedtime_workbench::WorkbenchRequests>()
                .push(WorkbenchAction::ClosePanel(panel));
        }
        4 => {
            assert_eq!(world.query::<&DocumentBinding>().iter(world).count(), 1);
            assert_eq!(
                core::model::<Graph>(world, document).unwrap().nodes[0],
                [10.0, 0.0]
            );
            let id = core::submit_host_request(
                world,
                None,
                core::api::features::HostAction::Save { document },
            );
            world.resource_mut::<Smoke>().save = Some(id);
        }
        5 => {
            let id = world.resource::<Smoke>().save.unwrap();
            let Some(result) = core::take_host_result(world, id) else {
                return;
            };
            assert!(result.is_err());
            assert!(core::model_document(world, document).unwrap().dirty());
        }
        6 => {
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
            let frame = result.expect("model window/panel capture");
            assert!(
                frame.images.iter().all(|image| image
                    .rgba
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[..3].iter().any(|channel| *channel > 20))),
                "model capture blank"
            );
            std::fs::create_dir_all("target").unwrap();
            frame.images[0]
                .rgba
                .write_png("target/model-smoke.png")
                .unwrap();
            world
                .resource::<Smoke>()
                .verified
                .store(true, Ordering::SeqCst);
            println!(
                "Model smoke passed: two views, typed history, retention, failed save and native capture"
            );
            world.write_message(AppExit::Success);
        }
        _ => return,
    }
    world.resource_mut::<Smoke>().phase += 1;
}
fn main() -> std::io::Result<()> {
    let smoke_test = std::env::args().any(|argument| argument == "--smoke-test");
    let launch = match bedtime::awaken("example.model", "Typed model editor")?.dispatch()? {
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let options = bedtime::DesktopOptions {
        config_directory: smoke_test.then(|| {
            std::env::temp_dir().join(format!("bed-model-example-config-{}", std::process::id()))
        }),
        ..Default::default()
    };
    let verified = Arc::<AtomicBool>::default();
    let mut app = launch.build(options, |app| {
        app.add_plugins(GraphPlugin);
        if smoke_test {
            app.insert_resource(Smoke {
                phase: 0,
                started: Instant::now(),
                save: None,
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
            "model smoke stopped before completion"
        );
    }
    if exit.is_error() {
        return Err(std::io::Error::other("Model example exited with an error"));
    }
    Ok(())
}
