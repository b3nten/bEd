//! Calling-shell transitions keep frontend identity and use the document barriers.
use super::*;
use crate::application::test_support::TempDir;
use bedtime_api::workbench::{Panel, PanelResult};
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
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
    }

    fn save(&self, _: &World) -> Value {
        json!({"transcript":self.0})
    }
}

pub(in crate::application) fn open_terminal(world: &mut World) -> Entity {
    if !world.contains_resource::<Terminals>() {
        world.init_resource::<Terminals>();
        let close = world.register_system(|In(entity): In<Entity>, world: &mut World| {
            let id = world.get::<PanelBinding>(entity).unwrap().terminal.unwrap();
            world.resource_mut::<Terminals>().closed.push(id);
            Ok(())
        });
        bedtime_api::workbench::register(world, "bedtime.terminal", |registrar| {
            registrar.panel(
                "bedtime.terminal.panel",
                "Terminal",
                move |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                    world.resource_mut::<Terminals>().created += 1;
                    let id = world.resource::<Terminals>().created;
                    world
                        .get_mut::<PanelBinding>(creation.entity)
                        .unwrap()
                        .terminal = Some(id);
                    world.entity_mut(creation.entity).insert(PanelClose(close));
                    Ok(Box::new(TerminalFrontend(id)))
                },
            );
        })
        .unwrap();
        world
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("bedtime.terminal", |registrar| {
                registrar.panel_options(
                    "bedtime.terminal.panel",
                    false,
                    bedtime_api::workbench::PanelPlacement::Bottom,
                );
            })
            .unwrap();
    }
    open_panel(
        world,
        "bedtime.terminal.panel",
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
        (bevy::MinimalPlugins, bevy::input::InputPlugin),
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(dir.root().to_owned()),
                ..Default::default()
            },
        },
        bedtime_plugin_editor::EditorPlugin,
    ));
    app.update();
    app
}

fn request(world: &mut World, source: Entity, host: HostAction) -> AppRequestOutcome {
    let request = capture_app_request_from(
        world,
        AppAction::HostAction(host),
        PanelTarget::LastFocused,
        Some(source),
    );
    let id = submit_app_request(world, request);
    wait_app_request(world, id).unwrap()
}

fn workspace_request(
    world: &mut World,
    source: Entity,
    action: WorkspaceAction,
) -> AppRequestOutcome {
    let request = capture_app_request_from(
        world,
        AppAction::Workspace(action),
        PanelTarget::LastFocused,
        Some(source),
    );
    let id = submit_app_request(world, request);
    wait_app_request(world, id).unwrap()
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
        .get::<PanelBinding>(caller)
        .unwrap()
        .terminal
        .unwrap();
    let other = open_terminal(app.world_mut());
    let document_panel = open_file(app.world_mut(), &file, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(document_panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        )
        .unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    let root = dir.path("project").canonicalize().unwrap();
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    let mut layout = bedtime_workbench::workspace::tiling::Layout::default();
    let slot = layout.split(1, 1, 7255, 100).unwrap();
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::new(layout.clone());
    tiling.assign(40, slot);
    app.world_mut()
        .non_send_mut::<WorkspacePersistence>()
        .store
        .record_workspace(spec.clone())
        .unwrap();
    app.world_mut().non_send_mut::<WorkspacePersistence>().store.save_session_layout(&spec, json!({
        "layouts":{LayoutSlot::Layout1.key():{"version":2,"panels":[{"id":40,"panel_type":"bedtime.terminal.panel","state":{"cwd":"unrelated"}}],
        "tiling":tiling.to_value([40]),"focused":40}},"active_layout_slot":LayoutSlot::Layout1.to_value()
    })).unwrap();
    assert_eq!(
        workspace_request(app.world_mut(), child, WorkspaceAction::Promote(root)),
        AppRequestOutcome::Applied
    );
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert_eq!(panel_presentation(app.world(), caller), Some(child));
    assert_eq!(
        app.world().get::<PanelBinding>(caller).unwrap().terminal,
        Some(terminal)
    );
    assert_eq!(app.world().resource::<Terminals>().created, 2);
    assert_eq!(app.world().resource::<Terminals>().closed, vec![2]);
    assert!(!app.world().entities().contains(other));
    assert!(!app.world().entities().contains(document_panel));
    assert_eq!(std::fs::read(file).unwrap(), b"after");
    assert!(
        app.world()
            .non_send::<DocumentSession>()
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
        bedtime_workbench::panel_state(app.world_mut(), child).unwrap(),
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
        .find(|panel| panel["panel_type"] == "bedtime.terminal.panel")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let default_tiling = bedtime_workbench::workspace::tiling_state::TilingState::from_value(
        &template["tiling"],
        panels.iter().map(|panel| panel["id"].as_u64().unwrap()),
    )
    .unwrap();
    let terminal_rect = default_tiling
        .layout
        .area(default_tiling.area_for(terminal).unwrap())
        .rect;
    let session = app.world().non_send::<DocumentSession>().session_id();
    workspace_request(
        app.world_mut(),
        caller,
        WorkspaceAction::Promote(dir.path("project")),
    );
    let state = app.world().resource::<WorkbenchLayout>();
    let area = state
        .tiling
        .area_for(app.world().get::<PanelPresentation>(caller).unwrap().id)
        .unwrap();
    assert_eq!(state.tiling.layout.area(area).rect, terminal_rect);
    let layout = state.tiling.clone();
    workspace_request(
        app.world_mut(),
        caller,
        WorkspaceAction::Promote(dir.path("second")),
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, layout);
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert_eq!(app.world().resource::<Terminals>().created, 1);
    assert!(app.world().resource::<Terminals>().closed.is_empty());
    assert_eq!(
        app.world().resource::<ApplicationState>().windows[0].root,
        dir.path("second").canonicalize().unwrap().to_string_lossy()
    );
}

#[test]
fn terminal_file_requests_use_the_largest_area_and_reuse_existing_tabs() {
    for linked in [false, true] {
        let dir = TempDir::new();
        let existing = dir.write("existing.rs", b"existing");
        let path = dir.write("source.rs", "first\n// 🦀 target\nlast".as_bytes());
        let mut app = app(&dir);
        let editor = open_file(app.world_mut(), &existing, None, false, None).unwrap();
        let caller = open_terminal(app.world_mut());
        let other = open_terminal(app.world_mut());
        let source = panel_presentation(app.world(), caller).unwrap();
        let mut tiling = app.world().resource::<WorkbenchLayout>().tiling.clone();
        let largest = tiling.layout.split(1, 0, 2500, 100).unwrap();
        tiling.add_area(largest);
        tiling.assign(
            app.world().get::<PanelPresentation>(editor).unwrap().id,
            largest,
        );
        app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
        focus_panel(app.world_mut(), caller, true).unwrap();
        let host = if linked {
            HostAction::OpenTerminalLink {
                path: path.clone(),
                position: Some((1, 8)),
            }
        } else {
            HostAction::OpenTerminalFiles {
                paths: vec![path.clone()],
            }
        };
        let queued = capture_app_request_from(
            app.world(),
            AppAction::HostAction(host),
            PanelTarget::LastFocused,
            Some(source),
        );
        assert_eq!(queued.area, largest);
        focus_panel(app.world_mut(), other, true).unwrap();
        let id = submit_app_request(app.world_mut(), queued);
        process_app_requests(app.world_mut()).unwrap();
        let opened = match poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .unwrap()
        {
            AppRequestOutcome::Opened { panel, .. } => panel,
            other => panic!("{other:?}"),
        };
        let layout = app.world().resource::<WorkbenchLayout>();
        assert_eq!(layout.tiling.areas.len(), 2);
        assert_eq!(
            layout
                .tiling
                .area_for(app.world().get::<PanelPresentation>(opened).unwrap().id),
            Some(largest)
        );
        let view = app
            .world()
            .get::<PanelBinding>(opened)
            .unwrap()
            .view
            .unwrap();
        if linked {
            let snapshot = app
                .world()
                .non_send::<DocumentSession>()
                .view_snapshot(view)
                .unwrap();
            assert_eq!((snapshot.row, snapshot.column), (1, 8));
        }
        // The other terminal and both entry points share the existing editor tab.
        let reused = request(
            app.world_mut(),
            other,
            HostAction::OpenTerminalFiles {
                paths: vec![path.clone()],
            },
        );
        assert!(matches!(reused, AppRequestOutcome::Opened { panel, .. } if panel == opened));
        let reused = request(
            app.world_mut(),
            source,
            HostAction::OpenTerminalLink {
                path,
                position: Some((2, 1)),
            },
        );
        assert!(matches!(reused, AppRequestOutcome::Opened { panel, .. } if panel == opened));
        assert_eq!(
            app.world().get::<PanelBinding>(opened).unwrap().view,
            Some(view)
        );
        let snapshot = app
            .world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap();
        assert_eq!((snapshot.row, snapshot.column), (2, 1));
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(opened)
        );
        assert_eq!(app.world().resource::<Terminals>().created, 2);
        assert!(app.world().resource::<Terminals>().closed.is_empty());
    }
}

#[test]
fn a_sole_terminal_gets_one_editor_area_for_file_links_and_multi_file_open() {
    for linked in [false, true] {
        let dir = TempDir::new();
        let first = dir.write("first.rs", b"first\nsecond");
        let next = dir.write("next.rs", b"next");
        let mut app = app(&dir);
        let caller = open_terminal(app.world_mut());
        let host = if linked {
            HostAction::OpenTerminalLink {
                path: first.clone(),
                position: Some((1, 2)),
            }
        } else {
            HostAction::OpenTerminalFiles {
                paths: vec![first.clone(), next.clone()],
            }
        };
        let opened = match request(app.world_mut(), caller, host) {
            AppRequestOutcome::Opened { panel, .. } => panel,
            other => panic!("{other:?}"),
        };
        let layout = app.world().resource::<WorkbenchLayout>();
        assert_eq!(layout.tiling.areas.len(), 2);
        let destination = layout
            .tiling
            .area_for(app.world().get::<PanelPresentation>(opened).unwrap().id)
            .unwrap();
        assert_ne!(
            layout
                .tiling
                .area_for(app.world().get::<PanelPresentation>(caller).unwrap().id),
            Some(destination)
        );
        let reused = request(
            app.world_mut(),
            caller,
            HostAction::OpenTerminalFiles {
                paths: vec![first.clone()],
            },
        );
        let first_panel = match reused {
            AppRequestOutcome::Opened { panel, .. } => panel,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().tiling.area_for(
                app.world()
                    .get::<PanelPresentation>(first_panel)
                    .unwrap()
                    .id
            ),
            Some(destination)
        );
        let last = request(
            app.world_mut(),
            caller,
            HostAction::OpenTerminalFiles { paths: vec![next] },
        );
        let last = match last {
            AppRequestOutcome::Opened { panel, .. } => panel,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().tiling.areas.len(),
            2
        );
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(app.world().get::<PanelPresentation>(last).unwrap().id),
            Some(destination)
        );
        assert_eq!(active_panel_entities(app.world_mut()).len(), 3);
        assert!(app.world().resource::<Terminals>().closed.is_empty());
    }
}

#[test]
fn failed_terminal_file_opens_leave_the_sole_terminal_layout_unchanged() {
    for linked in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let caller = open_terminal(app.world_mut());
        let original = app.world().resource::<WorkbenchLayout>().tiling.clone();
        let path = dir.path("missing.rs");
        let host = if linked {
            HostAction::OpenTerminalLink {
                path,
                position: Some((4, 2)),
            }
        } else {
            HostAction::OpenTerminalFiles { paths: vec![path] }
        };
        let queued = capture_app_request_from(
            app.world(),
            AppAction::HostAction(host),
            PanelTarget::LargestArea,
            Some(caller),
        );
        let id = submit_app_request(app.world_mut(), queued);
        process_app_requests(app.world_mut()).unwrap();
        assert!(
            poll_app_request_result(app.world_mut(), id)
                .unwrap()
                .is_err()
        );
        assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, original);
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(caller)
        );
    }
}
