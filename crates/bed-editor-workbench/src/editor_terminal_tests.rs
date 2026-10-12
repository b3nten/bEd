//! Calling-shell transitions keep frontend identity and use the document barriers.
use super::*;
use crate::test_support::TempDir;
use bed_workbench_api::{Panel, PanelResult};
use dear_imgui_rs::Ui;

#[derive(Resource, Default)]
struct Terminals {
    created: u64,
    closed: Vec<u64>,
}
struct TerminalFrontend(u64);
impl Panel for TerminalFrontend {
    fn title(&self, _: &World) -> String {
        format!("Shell {}", self.0)
    }
    fn draw(
        &mut self,
        _: &Ui,
        _: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
    }
    fn save(&self, _: &World) -> Value {
        json!({"transcript":self.0})
    }
}
pub(super) fn open_terminal(world: &mut World) -> Entity {
    if !world.contains_resource::<Terminals>() {
        world.init_resource::<Terminals>();
        let close = world.register_system(|In(entity): In<Entity>, world: &mut World| {
            let id = world
                .get::<EditorPanelBinding>(entity)
                .unwrap()
                .terminal
                .unwrap();
            world.resource_mut::<Terminals>().closed.push(id);
            Ok(())
        });
        bed_workbench_api::register(world, "bed.terminal", |registrar| {
            registrar.panel(
                "bed.terminal.panel",
                "Terminal",
                move |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                    world.resource_mut::<Terminals>().created += 1;
                    let id = world.resource::<Terminals>().created;
                    world
                        .get_mut::<EditorPanelBinding>(creation.entity)
                        .unwrap()
                        .terminal = Some(id);
                    world
                        .entity_mut(creation.entity)
                        .insert(EditorPanelClose(close));
                    Ok(Box::new(TerminalFrontend(id)))
                },
            );
        })
        .unwrap();
        world
            .resource_mut::<bed_editor_api::Registry>()
            .register_with("bed.terminal", |registrar| {
                registrar.panel_options(
                    "bed.terminal.panel",
                    "Terminal",
                    false,
                    bed_editor_api::PanelPlacement::Bottom,
                    Some("terminal"),
                );
            })
            .unwrap();
    }
    open_panel(
        world,
        "bed.terminal.panel",
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap()
}
fn app(dir: &TempDir) -> App {
    let mut app = App::new();
    install_test_infrastructure(&mut app, &dir.path("config"));
    app.add_plugins((
        bed_runtime::BedRuntimePlugin,
        EditorWorkbenchPlugin {
            options: EditorWorkbenchOptions {
                working_directory: Some(dir.root().to_owned()),
                ..Default::default()
            },
        },
        bed_editor::EditorPlugin,
    ));
    app.update();
    app
}
fn request(world: &mut World, source: Entity, host: HostRequest) -> EditorRequestOutcome {
    let request = capture_editor_request_from(
        world,
        EditorAction::HostRequest(host),
        PanelTarget::LastFocused,
        Some(source),
    );
    let id = submit_editor_request(world, request);
    process_editor_requests(world).unwrap();
    poll_editor_request_result(world, id).unwrap().unwrap()
}

#[test]
fn promotion_reuses_the_saved_terminal_slot_without_rebuilding_the_calling_frontend() {
    let dir = TempDir::new();
    let file = dir.write("project/dirty.txt", b"before");
    let mut app = app(&dir);
    let caller = open_terminal(app.world_mut());
    let child = panel_presentation(app.world(), caller).unwrap();
    let terminal = app
        .world()
        .get::<EditorPanelBinding>(caller)
        .unwrap()
        .terminal
        .unwrap();
    let other = open_terminal(app.world_mut());
    let document_panel = open_file(app.world_mut(), &file, None, false, None).unwrap();
    let document = app
        .world()
        .get::<EditorPanelBinding>(document_panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<EditorSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<EditorSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        )
        .unwrap();
    let session = app.world().non_send::<EditorSession>().workspace_id();
    let root = dir.path("project").canonicalize().unwrap();
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    let mut layout = bed_workbench::workspace::tiling::Layout::default();
    let slot = layout.split(1, 1, 7255, 100).unwrap();
    let mut tiling = bed_workbench::workspace::tiling_state::TilingState::new(layout.clone());
    tiling.assign(40, slot);
    app.world_mut().non_send_mut::<WorkspacePersistence>().store.set_layout(&spec, json!({
        "version":2,"panels":[{"id":40,"panel_type":"bed.terminal.panel","state":{"cwd":"unrelated"}}],
        "tiling":tiling.to_value([40]),"focused":40
    })).unwrap();
    assert_eq!(
        request(
            app.world_mut(),
            child,
            HostRequest::PromoteWorkspace { root }
        ),
        EditorRequestOutcome::Applied
    );
    assert_eq!(
        app.world().non_send::<EditorSession>().workspace_id(),
        session
    );
    assert_eq!(panel_presentation(app.world(), caller), Some(child));
    assert_eq!(
        app.world()
            .get::<EditorPanelBinding>(caller)
            .unwrap()
            .terminal,
        Some(terminal)
    );
    assert_eq!(app.world().resource::<Terminals>().created, 2);
    assert_eq!(app.world().resource::<Terminals>().closed, vec![2]);
    assert!(!app.world().entities().contains(other));
    assert!(!app.world().entities().contains(document_panel));
    assert_eq!(std::fs::read(file).unwrap(), b"after");
    assert!(
        app.world()
            .non_send::<EditorSession>()
            .document_ids()
            .is_empty()
    );
    let state = app.world().resource::<WorkbenchLayout>();
    assert_eq!(state.tiling.layout, layout);
    assert_eq!(
        state
            .tiling
            .area_for(app.world().get::<PanelPresentation>(caller).unwrap().id),
        Some(slot)
    );
    assert_eq!(state.focused, Some(caller));
    assert_eq!(
        bed_workbench::panel_state(app.world_mut(), child).unwrap(),
        json!({"transcript":1})
    );
}

#[test]
fn promotion_uses_default_terminal_placement_and_opens_later_roots_in_another_window() {
    let dir = TempDir::new();
    std::fs::create_dir(dir.path("project")).unwrap();
    std::fs::create_dir(dir.path("second")).unwrap();
    let mut app = app(&dir);
    let caller = open_terminal(app.world_mut());
    let template = layouts::template(app.world(), LayoutSlot::Layout1);
    let panels = template["panels"].as_array().unwrap();
    let terminal = panels
        .iter()
        .find(|panel| panel["panel_type"] == "bed.terminal.panel")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let default_tiling = bed_workbench::workspace::tiling_state::TilingState::from_value(
        &template["tiling"],
        panels.iter().map(|panel| panel["id"].as_u64().unwrap()),
    )
    .unwrap();
    let terminal_rect = default_tiling
        .layout
        .area(default_tiling.area_for(terminal).unwrap())
        .rect;
    let session = app.world().non_send::<EditorSession>().workspace_id();
    request(
        app.world_mut(),
        caller,
        HostRequest::PromoteWorkspace {
            root: dir.path("project"),
        },
    );
    let state = app.world().resource::<WorkbenchLayout>();
    let area = state
        .tiling
        .area_for(app.world().get::<PanelPresentation>(caller).unwrap().id)
        .unwrap();
    assert_eq!(state.tiling.layout.area(area).rect, terminal_rect);
    let layout = state.tiling.clone();
    request(
        app.world_mut(),
        caller,
        HostRequest::PromoteWorkspace {
            root: dir.path("second"),
        },
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, layout);
    assert_eq!(
        app.world().non_send::<EditorSession>().workspace_id(),
        session
    );
    assert_eq!(app.world().resource::<Terminals>().created, 1);
    assert!(app.world().resource::<Terminals>().closed.is_empty());
    assert_eq!(
        app.world().resource::<EditorWorkspace>().windows[0].root,
        dir.path("second").canonicalize().unwrap().to_string_lossy()
    );
}

#[test]
fn shell_companions_share_documents_but_keep_per_caller_views_and_group_placement() {
    let dir = TempDir::new();
    let first = dir.write("first.txt", b"first");
    let next = dir.write("next.txt", b"next");
    let mut app = app(&dir);
    let caller = open_terminal(app.world_mut());
    let other = open_terminal(app.world_mut());
    let source = panel_presentation(app.world(), caller).unwrap();
    let queued = capture_editor_request_from(
        app.world(),
        EditorAction::HostRequest(HostRequest::OpenTerminalCompanions {
            paths: vec![first.clone()],
        }),
        PanelTarget::LastFocused,
        Some(source),
    );
    focus_panel(app.world_mut(), other, true).unwrap();
    let id = submit_editor_request(app.world_mut(), queued);
    process_editor_requests(app.world_mut()).unwrap();
    let a = match poll_editor_request_result(app.world_mut(), id)
        .unwrap()
        .unwrap()
    {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let area = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(app.world().get::<PanelPresentation>(a).unwrap().id)
        .unwrap();
    assert_ne!(
        area,
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(caller).unwrap().id)
            .unwrap()
    );
    let b = match request(
        app.world_mut(),
        other,
        HostRequest::OpenTerminalCompanions {
            paths: vec![first.clone()],
        },
    ) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    assert_ne!(a, b);
    let first_binding = app.world().get::<EditorPanelBinding>(a).unwrap();
    let other_binding = app.world().get::<EditorPanelBinding>(b).unwrap();
    assert_eq!(first_binding.document, other_binding.document);
    assert_ne!(first_binding.view, other_binding.view);
    let again = request(
        app.world_mut(),
        source,
        HostRequest::OpenTerminalCompanions { paths: vec![first] },
    );
    assert!(matches!(again, EditorRequestOutcome::Opened {panel,..} if panel == a));
    let c = match request(
        app.world_mut(),
        source,
        HostRequest::OpenTerminalCompanions { paths: vec![next] },
    ) {
        EditorRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(c).unwrap().id),
        Some(area)
    );
    assert_eq!(app.world().resource::<Terminals>().created, 2);
    assert!(app.world().resource::<Terminals>().closed.is_empty());
}
