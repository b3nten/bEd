use crate::*;
use bed_api::{documents::PanelBinding, features::HostAction};
use bed_documents::{ByteEdit, DocumentSession};
use bed_workbench::{WorkbenchLayout, WorkbenchRequests};
use bevy::prelude::*;
use serde_json::{Value, json};

#[path = "../../../tests/support/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDir;

struct EmptyPanel;
impl api::workbench::Panel for EmptyPanel {
    fn title(&self, _: &World) -> String {
        "Test".into()
    }
    fn draw(
        &mut self,
        _: &dear_imgui_rs::Ui,
        _: &mut World,
        _: &mut Vec<api::workbench::WorkbenchAction>,
        _: &api::workbench::ShortcutInput,
    ) {
    }
}
fn factory(_: In<api::workbench::PanelCreation>) -> api::workbench::PanelResult {
    Ok(Box::new(EmptyPanel))
}
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(DocumentHostPlugin);
    api::workbench::register(app.world_mut(), "test", |registrar| {
        registrar.panel("test.panel", "Test", factory);
    })
    .unwrap();
    app
}
fn panel(world: &mut World) -> Entity {
    bed_workbench::open_panel(
        world,
        json!({"panel_type":"test.panel","state":null}),
        None,
        None,
    )
    .unwrap()
}
fn edit(world: &World, document: bed_documents::DocumentId, bytes: &[u8]) -> HostAction {
    HostAction::ApplyEdits {
        document,
        revision: document_revision(world, document).unwrap(),
        edits: vec![ByteEdit {
            range: 0..0,
            bytes: bytes.to_vec(),
        }],
    }
}

#[test]
fn a_save_receipt_identifies_the_committed_revision_and_shared_views_follow_history() {
    let temp = TempDir::new();
    let path = temp.write("document.txt", b"old");
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let first = world
        .non_send_mut::<DocumentSession>()
        .create_view(document)
        .unwrap();
    let second = world
        .non_send_mut::<DocumentSession>()
        .create_view(document)
        .unwrap();
    let request = edit(world, document, b"new ");
    let edit_id = submit_host_request(world, None, request);
    let save_id = submit_host_request(world, None, HostAction::Save { document });
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, edit_id),
        Some(Ok(HostOutcome::Applied))
    );
    let revision = document_revision(world, document).unwrap();
    assert_eq!(
        take_host_result(world, save_id),
        Some(Ok(HostOutcome::Saved {
            documents: vec![document],
            revisions: vec![revision]
        }))
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"new old");
    assert!(!document_dirty(world, document).unwrap());
    assert_eq!(
        world.non_send::<DocumentSession>().document_for_view(first),
        Some(document)
    );
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .document_for_view(second),
        Some(document)
    );
    undo_document(world, document).unwrap();
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"old"
    );
    assert!(document_dirty(world, document).unwrap());
    redo_document(world, document).unwrap();
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"new old"
    );
}

#[test]
fn session_replacement_and_failed_frontends_reject_captured_effects() {
    let mut app = app();
    let world = app.world_mut();
    let source = panel(world);
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"original")
        .unwrap();
    let request = capture_host_request(world, Some(source), edit(world, document, b"bad "));
    world
        .get_mut::<bed_workbench::PanelRecord>(source)
        .unwrap()
        .unavailable = Some("failed while drawing".into());
    world.write_message(request);
    resolve_documents(world);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"original"
    );
    let request = edit(world, document, b"stale ");
    let id = submit_host_request(world, None, request);
    world.insert_non_send(DocumentSession::new());
    resolve_documents(world);
    assert!(
        take_host_result(world, id)
            .unwrap()
            .unwrap_err()
            .contains("replaced")
    );
}

#[test]
fn stale_layout_rejects_opening_but_explicit_document_edits_remain_valid() {
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"old")
        .unwrap();
    let open = submit_host_request(
        world,
        None,
        HostAction::OpenPanel {
            panel_type: "test.panel".into(),
            document: None,
            state: Value::Null,
        },
    );
    let request = edit(world, document, b"new ");
    let edit = submit_host_request(world, None, request);
    bed_workbench::advance_layout_epoch(world);
    resolve_documents(world);
    assert!(
        take_host_result(world, open)
            .unwrap()
            .unwrap_err()
            .contains("layout")
    );
    assert_eq!(
        take_host_result(world, edit),
        Some(Ok(HostOutcome::Applied))
    );
}

fn defer_close(_: &mut World, _: Entity) -> Result<bool, String> {
    Ok(false)
}
#[test]
fn admitted_messages_survive_a_deferred_layout_and_message_expiry() {
    let mut app = app();
    let world = app.world_mut();
    let source = panel(world);
    world.insert_resource(bed_workbench::WorkbenchClosePolicy(defer_close));
    let tiling = world.resource::<WorkbenchLayout>().tiling.to_value([]);
    world
        .resource_mut::<WorkbenchRequests>()
        .push(api::workbench::WorkbenchAction::SetLayout {
            tiling,
            close: vec![source],
        });
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"old")
        .unwrap();
    world.write_message(capture_host_request(
        world,
        None,
        edit(world, document, b"retained "),
    ));
    resolve_documents(world);
    assert!(bed_workbench::waiting_for_layout(world));
    world.resource_mut::<Messages<HostRequest>>().clear();
    bed_workbench::commit_close(world, source);
    resolve_documents(world);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"retained old"
    );
}

#[derive(Clone)]
struct Counter(u32);
impl DocumentModel for Counter {
    type Change = u32;
    fn apply(&mut self, change: &u32) -> Result<u32, String> {
        let inverse = self.0;
        self.0 = *change;
        Ok(inverse)
    }
    fn encode(&self) -> Result<Vec<u8>, String> {
        Ok(self.0.to_string().into_bytes())
    }
}
#[test]
fn typed_model_close_and_save_use_the_same_document_policy_and_receipts() {
    let temp = TempDir::new();
    let path = temp.path("counter.txt");
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", Some(path.clone()));
    let first = panel(world);
    let second = panel(world);
    world.entity_mut(first).insert(DocumentBinding(document));
    world.entity_mut(second).insert(DocumentBinding(document));
    edit_model::<Counter>(world, document, 0, 2, "Increment").unwrap();
    let close_first = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(first));
    let close_last = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(second));
    resolve_documents(world);
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(close_first)
            .unwrap()
            .is_ok()
    );
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(close_last)
            .unwrap()
            .is_err()
    );
    assert!(world.entities().contains(second));
    let save = submit_host_request(world, Some(second), HostAction::Save { document });
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, save),
        Some(Ok(HostOutcome::Saved {
            documents: vec![document],
            revisions: vec![(0, 1)]
        }))
    );
    assert_eq!(std::fs::read(path).unwrap(), b"2");
    assert!(!document_dirty(world, document).unwrap());
    world
        .resource_mut::<WorkbenchRequests>()
        .push(api::workbench::WorkbenchAction::ClosePanel(second));
    resolve_documents(world);
    assert!(!world.entities().contains(second));
}

#[test]
fn failed_save_keeps_the_model_dirty_and_quit_keeps_documents_alive() {
    let temp = TempDir::new();
    let mut app = app();
    let world = app.world_mut();
    let document =
        create_model_document(world, Counter(1), "Counter", Some(temp.root().to_owned()));
    edit_model::<Counter>(world, document, 0, 2, "Increment").unwrap();
    let save = submit_host_request(world, None, HostAction::Save { document });
    resolve_documents(world);
    assert!(take_host_result(world, save).unwrap().is_err());
    assert!(document_dirty(world, document).unwrap());
    assert!(quit(world).is_err());
    assert!(model::<Counter>(world, document).is_some());
    assert_eq!(
        std::fs::read_dir(temp.root().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(
                |entry| entry.file_name().to_string_lossy().contains(&format!(
                    "bed-{}-{}",
                    std::process::id(),
                    document.0
                ))
            )
            .count(),
        0
    );
}

#[derive(Resource, Default)]
struct Retired(Vec<Entity>);
#[test]
fn rejected_restore_does_not_run_destructive_retirement_hooks() {
    let mut app = app();
    app.init_resource::<Retired>();
    let world = app.world_mut();
    let clean = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"clean")
        .unwrap();
    let dirty = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"dirty")
        .unwrap();
    let first = open_document_panel(world, "test.panel", clean, Value::Null, None).unwrap();
    let second = open_document_panel(world, "test.panel", dirty, Value::Null, None).unwrap();
    let hook = world.register_system(
        |In(entity): In<Entity>, mut retired: ResMut<Retired>| -> Result<(), String> {
            retired.0.push(entity);
            Ok(())
        },
    );
    world
        .entity_mut(first)
        .insert(bed_api::workbench::PanelClose(hook));
    world
        .entity_mut(second)
        .insert(bed_api::workbench::PanelClose(hook));
    let request = edit(world, dirty, b"changed ");
    submit_host_request(world, Some(second), request);
    resolve_documents(world);
    let snapshot = bed_workbench::save(world);
    assert!(bed_workbench::restore(world, &snapshot).is_err());
    assert!(world.resource::<Retired>().0.is_empty());
    assert!(world.entities().contains(first) && world.entities().contains(second));
    assert!(bed_workbench::request_close(world, first).unwrap());
    assert_eq!(world.resource::<Retired>().0, [first]);
}

#[test]
fn untitled_restore_reuses_only_an_identity_from_the_same_live_session() {
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"retained scratch contents")
        .unwrap();
    open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let snapshot = bed_workbench::save(world);
    bed_workbench::restore(world, &snapshot).unwrap();
    let restored = world
        .query::<(&PanelBinding, &bed_workbench::PanelRecord)>()
        .iter(world)
        .next()
        .unwrap();
    assert_eq!(restored.0.document, Some(document));
    assert!(restored.1.unavailable.is_none());
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"retained scratch contents"
    );
    let mut fresh = self::app();
    let unrelated = fresh
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .create_document(b"unrelated new contents")
        .unwrap();
    // Simulate counter reuse in a later process: only the live origin differs.
    let mut collided = snapshot.clone();
    collided["panels"][0]["document"] = json!(unrelated.0);
    collided["panels"][0]["document_session"] =
        json!(fresh.world().non_send::<DocumentSession>().session_id().0);
    bed_workbench::restore(fresh.world_mut(), &collided).unwrap();
    let record = fresh
        .world_mut()
        .query::<&bed_workbench::PanelRecord>()
        .iter(fresh.world())
        .next()
        .unwrap();
    assert!(
        record
            .unavailable
            .as_deref()
            .unwrap()
            .contains("application restoration")
    );
    assert_eq!(
        fresh.world().non_send::<DocumentSession>().document_ids(),
        [unrelated]
    );
    assert_eq!(
        fresh
            .world()
            .non_send::<DocumentSession>()
            .snapshot(unrelated)
            .unwrap()
            .bytes,
        b"unrelated new contents"
    );
}

fn viewers(world: &mut World) {
    world
        .resource_mut::<bed_api::features::FeatureRegistry>()
        .register_with("test", |registrar| {
            registrar.panel_ref("test.panel");
            registrar.fallback_viewer(
                "test.text",
                "Text",
                "test.panel",
                bed_documents::DocumentKind::Text,
                &[],
            );
        })
        .unwrap();
}
#[test]
fn previews_replace_clean_tabs_retain_dirty_tabs_and_drops_move_existing_tabs() {
    let temp = TempDir::new();
    let paths = [
        temp.write("a.txt", b"a"),
        temp.write("b.txt", b"b"),
        temp.write("c.txt", b"c"),
    ];
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    let preview = |path: &std::path::Path| HostAction::PreviewFileIn {
        path: path.to_string_lossy().into_owned(),
        viewer: None,
        target: bed_api::workbench::PanelTarget::LastFocused,
    };
    let first = submit_host_request(world, None, preview(&paths[0]));
    resolve_documents(world);
    let HostOutcome::Opened(first) = take_host_result(world, first).unwrap().unwrap() else {
        panic!()
    };
    let second = submit_host_request(world, None, preview(&paths[1]));
    resolve_documents(world);
    let HostOutcome::Opened(second) = take_host_result(world, second).unwrap().unwrap() else {
        panic!()
    };
    assert!(!world.entities().contains(first));
    let document = world.get::<PanelBinding>(second).unwrap().document.unwrap();
    let request = edit(world, document, b"changed ");
    submit_host_request(world, Some(second), request);
    resolve_documents(world);
    let third = submit_host_request(world, Some(second), preview(&paths[2]));
    resolve_documents(world);
    let HostOutcome::Opened(third) = take_host_result(world, third).unwrap().unwrap() else {
        panic!()
    };
    assert!(world.entities().contains(second));
    assert_eq!(
        world.resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(
            world
                .get::<api::workbench::PanelPresentation>(third)
                .unwrap()
                .id
        )
    );
    world
        .resource_mut::<WorkbenchRequests>()
        .push(api::workbench::WorkbenchAction::SplitPanel {
            panel: second,
            axis: 0,
            high: true,
        });
    resolve_documents(world);
    let target = world
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(
            world
                .get::<api::workbench::PanelPresentation>(third)
                .unwrap()
                .id,
        )
        .unwrap();
    let drop = submit_host_request(
        world,
        Some(third),
        HostAction::DropFile {
            path: paths[1].to_string_lossy().into_owned(),
        },
    );
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, drop),
        Some(Ok(HostOutcome::Opened(second)))
    );
    assert_eq!(
        world.resource::<WorkbenchLayout>().tiling.area_for(
            world
                .get::<api::workbench::PanelPresentation>(second)
                .unwrap()
                .id
        ),
        Some(target)
    );
}

#[test]
fn document_commands_use_the_captured_panel_and_tools_do_not_inherit_selection() {
    let mut app = app();
    app.add_plugins(crate::commands::DocumentCommandsPlugin);
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"initial")
        .unwrap();
    let editor = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let request = edit(world, document, b"changed ");
    submit_host_request(world, Some(editor), request);
    resolve_documents(world);
    let captured = bed_workbench::capture_command(world, "bed.core.undo", Value::Null);
    let tool = panel(world);
    resolve_documents(world);
    assert!(
        world
            .resource::<bed_api::features::HostFrame>()
            .active_document
            .is_none()
    );
    // The earlier editor command still targets its originating document.
    world
        .resource_mut::<WorkbenchRequests>()
        .push(api::workbench::WorkbenchAction::Invoke(captured));
    resolve_documents(world);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"initial"
    );
    let handler = world
        .resource::<api::workbench::WorkbenchRegistry>()
        .command("bed.core.undo")
        .unwrap()
        .handler;
    let result = world
        .run_system_with(
            handler,
            api::workbench::CommandInvocation {
                command: "bed.core.undo".into(),
                source: Some(tool),
                area: 1,
                arguments: Value::Null,
            },
        )
        .unwrap();
    assert!(result.unwrap_err().contains("No document"));
}

#[test]
fn native_quit_waits_for_drawing_and_prior_document_operations() {
    let temp = TempDir::new();
    let path = temp.write("quit.txt", b"before");
    let mut app = app();
    app.add_plugins(crate::commands::DocumentCommandsPlugin)
        .init_resource::<bed_desktop::DesktopNativeActions>()
        .add_message::<AppExit>();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let request = edit(world, document, b"after ");
    submit_host_request(world, None, request);
    let saved = submit_host_request(world, None, HostAction::Save { document });
    world
        .resource_mut::<bed_desktop::DesktopNativeActions>()
        .0
        .push(bed_desktop::DesktopNativeAction {
            command: "bed.core.quit".into(),
            keyboard: false,
            modifiers: Default::default(),
            quit: true,
        });
    crate::route_native_commands(world);
    assert!(!world.resource::<crate::documents::CoreExiting>().0);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
    resolve_documents(world);
    assert!(take_host_result(world, saved).unwrap().is_ok());
    assert_eq!(std::fs::read(path).unwrap(), b"after before");
    assert!(world.resource::<crate::documents::CoreExiting>().0);
}

#[test]
fn aggregate_restore_cannot_treat_another_closing_view_as_a_survivor() {
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"shared")
        .unwrap();
    let first = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let second = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let request = edit(world, document, b"changed ");
    submit_host_request(world, Some(first), request);
    resolve_documents(world);
    let snapshot = bed_workbench::save(world);
    assert!(
        bed_workbench::restore(world, &snapshot)
            .unwrap_err()
            .contains("Save or discard")
    );
    assert!(world.entities().contains(first) && world.entities().contains(second));
    assert!(bed_workbench::closing_panels(world).is_none());
    assert!(document_dirty(world, document).unwrap());
    assert!(bed_workbench::request_close_many(world, &[first, second]).is_err());
    assert!(world.entities().contains(first) && world.entities().contains(second));
}

#[test]
fn model_mutation_and_persistence_stop_with_the_shared_session() {
    let temp = TempDir::new();
    let path = temp.path("closed-model.txt");
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", Some(path.clone()));
    quit(world).unwrap();
    assert!(edit_model::<Counter>(world, document, 0, 2, "After quit").is_err());
    assert!(undo_document(world, document).is_err());
    assert!(save_document(world, document).is_err());
    assert!(mark_model_saved(world, document, 0).is_err());
    assert!(!path.exists());
}
