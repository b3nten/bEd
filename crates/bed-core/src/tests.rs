use crate::*;
use bed_api::{documents::PanelBinding, features::HostAction};
use bed_documents::{ByteEdit, DocumentSession};
use bed_workbench::{WorkbenchLayout, WorkbenchRequests};
use bevy::prelude::*;
use serde_json::{Value, json};

#[path = "../../../tests/support/temp_dir.rs"]
pub(super) mod temp_dir;
use temp_dir::TempDir;
fn wait_host_result(world: &mut World, id: HostRequestId) -> Result<HostOutcome, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        poll_document_services(world);
        resolve_documents(world);
        if let Some(result) = take_host_result(world, id) {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation did not complete"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn save_admission_validates_every_destination_before_writing_any_document() {
    let temp = TempDir::new();
    let path = temp.write("first.txt", b"old");
    let mut app = app();
    let world = app.world_mut();
    let first = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let untitled = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"untitled")
        .unwrap();
    let request = edit(world, first, b"new ");
    apply_document_request(world, capture_host_request(world, None, request)).unwrap();
    let result = begin_document_save(
        world,
        &[
            DocumentSaveTarget {
                document: first,
                destination: None,
            },
            DocumentSaveTarget {
                document: untitled,
                destination: None,
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"old");
}

#[test]
fn operation_barrier_orders_requests_and_rechecks_changes_before_replacement() {
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"start")
        .unwrap();
    let before = submit_host_request(world, None, edit(world, document, b"before "));
    let barrier = capture_operation_barrier(world, None);
    let later = submit_host_request(
        world,
        None,
        HostAction::Notify {
            message: "after".into(),
        },
    );
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, before),
        Some(Ok(HostOutcome::Applied))
    );
    assert!(take_host_result(world, later).is_none());
    assert_eq!(
        barrier_status(world, barrier),
        Some(OperationBarrierStatus::Ready)
    );
    assert!(matches!(
        prepare_document_transition(world, barrier, bed_documents::ClosePolicy::Cancel).unwrap(),
        OperationBarrierStatus::NeedsDecision(_)
    ));
    prepare_document_transition(world, barrier, bed_documents::ClosePolicy::Discard).unwrap();
    let revision = document_revision(world, document).unwrap();
    world
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..0,
                bytes: b"during prompt ".to_vec(),
            }],
        )
        .unwrap();
    assert!(validate_document_transition(world, barrier).is_err());
    assert!(commit_context_change(world, barrier, DocumentSession::new()).is_err());
    finish_operation_barrier(world, barrier, Err("cancelled".into())).unwrap();
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, later),
        Some(Ok(HostOutcome::Applied))
    );
    assert!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes
            .starts_with(b"during prompt")
    );
}

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
        Some(wait_host_result(world, save)),
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
    assert!(wait_host_result(world, save).is_err());
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

#[test]
fn a_model_write_acknowledges_its_snapshot_without_marking_newer_edits_clean() {
    let temp = TempDir::new();
    let path = temp.root().join("model.txt");
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(0), "Counter", Some(path.clone()));
    edit_model::<Counter>(world, document, 0, 1, "First").unwrap();
    let save = begin_document_save(
        world,
        &[DocumentSaveTarget {
            document,
            destination: None,
        }],
    )
    .unwrap();
    edit_model::<Counter>(world, document, 1, 2, "Second").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let saved = loop {
        if let Some(result) = poll_document_save(world, save) {
            break result.unwrap();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(saved[0].revision, (0, 1));
    assert_eq!(std::fs::read(&path).unwrap(), b"1");
    assert!(document_dirty(world, document).unwrap());
    undo_document(world, document).unwrap();
    assert_eq!(model::<Counter>(world, document).unwrap().0, 1);
    assert!(!document_dirty(world, document).unwrap());
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
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"initial")
        .unwrap();
    let editor = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let request = edit(world, document, b"changed ");
    submit_host_request(world, Some(editor), request);
    resolve_documents(world);
    let captured = bed_workbench::capture_command(world, "bed.documents.undo", Value::Null);
    let tool = panel(world);
    resolve_documents(world);
    assert!(
        world
            .resource::<bed_api::documents::DocumentFrame>()
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
        .command("bed.documents.undo")
        .unwrap()
        .handler;
    let result = world
        .run_system_with(
            handler,
            api::workbench::CommandInvocation {
                command: "bed.documents.undo".into(),
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
    app.init_resource::<bed_desktop::DesktopNativeActions>()
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

#[test]
fn opening_a_directory_without_workspaces_retains_live_documents_and_layout() {
    let temp = TempDir::new();
    let directory = temp.path("project");
    std::fs::create_dir(&directory).unwrap();
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"draft")
        .unwrap();
    let session = world.non_send::<DocumentSession>().session_id();
    let panel = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let request = edit(world, document, b"unsaved ");
    apply_document_request(world, capture_host_request(world, Some(panel), request)).unwrap();
    let generation = world.resource::<bed_files::FileContext>().generation;
    let id = submit_host_request(
        world,
        None,
        HostAction::OpenDirectory {
            location: bed_files::FileLocation {
                provider: bed_files::FileProviderId::Local,
                path: directory.clone(),
            },
        },
    );
    assert_eq!(wait_host_result(world, id), Ok(HostOutcome::Applied));
    assert_eq!(
        world.resource::<bed_files::FileContext>().directory,
        std::fs::canonicalize(directory).unwrap()
    );
    assert!(world.resource::<bed_files::FileContext>().generation > generation);
    assert_eq!(world.non_send::<DocumentSession>().session_id(), session);
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"unsaved draft"
    );
    assert!(world.entities().contains(panel));
    assert!(document_dirty(world, document).unwrap());
}

#[test]
fn saving_the_last_model_view_waits_for_storage_before_closing_it() {
    let temp = TempDir::new();
    let path = temp.path("counter.txt");
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", Some(path.clone()));
    let panel = bed_workbench::open_panel(
        world,
        json!({"panel_type":"test.panel","state":null}),
        None,
        None,
    )
    .unwrap();
    world.entity_mut(panel).insert(DocumentBinding(document));
    edit_model::<Counter>(world, document, 0, 2, "increment").unwrap();
    world.resource_mut::<DocumentClosePolicy>().0 = bed_documents::ClosePolicy::Save;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !bed_workbench::request_close_many(world, &[panel]).unwrap() {
        assert!(world.entities().contains(panel));
        assert!(std::time::Instant::now() < deadline);
        poll_document_services(world);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"2");
    assert!(!world.entities().contains(panel));
    assert!(!model_document(world, document).unwrap().dirty());
}

#[test]
fn context_replacement_retires_unbound_typed_documents() {
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", None);
    let barrier = capture_operation_barrier(world, None);
    resolve_documents(world);
    prepare_document_transition(world, barrier, bed_documents::ClosePolicy::Discard).unwrap();
    validate_document_transition(world, barrier).unwrap();
    commit_context_change(world, barrier, DocumentSession::new()).unwrap();
    finish_operation_barrier(world, barrier, Ok(())).unwrap();
    assert!(model_document(world, document).is_none());
}

#[test]
fn settings_requests_use_core_without_application_or_workspace_services() {
    let mut app = app();
    let world = app.world_mut();
    api::settings::register(
        world,
        "example.palette",
        vec![api::settings::SettingDefinition::new(
            "swatches",
            "Swatches",
            json!([]),
        )],
    )
    .unwrap();
    world.write_message(api::settings::SettingsRequest::new(
        "example.palette",
        "swatches",
        json!(["red"]),
    ));
    let barrier = capture_operation_barrier(world, None);
    world.write_message(api::settings::SettingsRequest::new(
        "example.palette",
        "swatches",
        json!(["blue"]),
    ));
    resolve_documents(world);
    assert_eq!(
        world.resource::<api::settings::SettingsSnapshot>().settings["example.palette"]["swatches"],
        json!(["red"])
    );
    finish_operation_barrier(world, barrier, Ok(())).unwrap();
    resolve_documents(world);
    assert_eq!(
        world.resource::<api::settings::SettingsSnapshot>().settings["example.palette"]["swatches"],
        json!(["blue"])
    );
}

#[test]
fn an_aggregate_failure_still_acknowledges_an_earlier_model_write() {
    let temp = TempDir::new();
    let model_path = temp.root().join("model.txt");
    let text_path = temp.write("removed.txt", b"original");
    let mut app = app();
    let world = app.world_mut();
    let model_id = create_model_document(world, Counter(0), "Counter", Some(model_path.clone()));
    edit_model::<Counter>(world, model_id, 0, 1, "Increment").unwrap();
    let text_id = world
        .non_send_mut::<DocumentSession>()
        .open_file(&text_path)
        .unwrap();
    let request = edit(world, text_id, b"changed ");
    apply_document_request(world, capture_host_request(world, None, request)).unwrap();
    std::fs::remove_file(&text_path).unwrap();

    // Destination admission succeeds. The text write fails after the lower-ID
    // model's worker has started, so its checkpoint must still be observed.
    let save = begin_document_save(
        world,
        &[
            DocumentSaveTarget {
                document: model_id,
                destination: None,
            },
            DocumentSaveTarget {
                document: text_id,
                destination: None,
            },
        ],
    )
    .unwrap();
    let mut result = poll_document_save(world, save);
    if result.is_none() {
        assert!(
            save_document(world, model_id)
                .unwrap_err()
                .contains("pending save")
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while result.is_none() {
        assert!(std::time::Instant::now() < deadline);
        poll_document_services(world);
        result = poll_document_save(world, save);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(result.unwrap().is_err());
    assert_eq!(std::fs::read(model_path).unwrap(), b"1");
    assert!(!document_dirty(world, model_id).unwrap());
    assert!(document_dirty(world, text_id).unwrap());
    assert!(!text_path.exists());
}

#[test]
fn a_rejected_typed_model_draft_prevents_local_directory_adoption() {
    fn reject(In(invocation): In<api::workbench::PanelActionInvocation>) -> Result<bool, String> {
        assert!(matches!(
            invocation.action,
            api::workbench::PanelAction::CommitEdit
        ));
        Err("Typed model draft was rejected".into())
    }
    let temp = TempDir::new();
    let next = temp.root().join("next");
    std::fs::create_dir(&next).unwrap();
    let original = bed_files::FileContext::local(temp.root()).unwrap();
    let mut app = app();
    let world = app.world_mut();
    world.insert_resource(original.clone());
    let document = create_model_document(world, Counter(1), "Counter", None);
    let panel = panel(world);
    let handler = world.register_system(reject);
    world.entity_mut(panel).insert((
        DocumentBinding(document),
        api::workbench::PanelActions(handler),
    ));
    let request = submit_host_request(
        world,
        None,
        HostAction::OpenDirectory {
            location: bed_files::FileLocation {
                provider: bed_files::FileProviderId::Local,
                path: next,
            },
        },
    );
    assert_eq!(
        wait_host_result(world, request).unwrap_err(),
        "Typed model draft was rejected"
    );
    assert_eq!(world.resource::<bed_files::FileContext>(), &original);
    assert!(world.entities().contains(panel));
    assert_eq!(model::<Counter>(world, document).unwrap().0, 1);
}

#[test]
fn command_effects_resume_in_order_between_two_operation_barriers() {
    #[derive(Resource, Default)]
    struct CommandBarriers(Option<[OperationBarrierToken; 2]>);
    fn invoke(
        _: In<api::workbench::CommandInvocation>,
        world: &mut World,
    ) -> api::workbench::CommandResult {
        let first = create_operation_barrier(world);
        let second = create_operation_barrier(world);
        world.resource_mut::<CommandBarriers>().0 = Some([first, second]);
        for action in [
            HostAction::OperationBarrier { token: first },
            HostAction::Notify {
                message: "first".into(),
            },
            HostAction::OperationBarrier { token: second },
            HostAction::Notify {
                message: "second".into(),
            },
        ] {
            world
                .resource_mut::<api::features::HostEffects>()
                .push(None, action);
        }
        Ok(Vec::new())
    }
    let mut app = app();
    let world = app.world_mut();
    world.init_resource::<CommandBarriers>();
    api::workbench::register(world, "test.continuations", |registrar| {
        registrar.command("test.continuations.run", "Continue", invoke);
    })
    .unwrap();
    let invocation = bed_workbench::capture_command(world, "test.continuations.run", Value::Null);
    let command = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::Invoke(invocation));
    let independent = submit_host_request(
        world,
        None,
        HostAction::Notify {
            message: "independent".into(),
        },
    );
    let mut notices = bevy::ecs::message::MessageCursor::<CoreNotice>::default();
    resolve_documents(world);
    let [first, second] = world.resource::<CommandBarriers>().0.unwrap();
    assert_eq!(active_operation_barrier(world), Some(first));
    assert_eq!(
        notices
            .read(world.resource::<Messages<CoreNotice>>())
            .count(),
        0
    );
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(command)
            .is_none()
    );
    assert!(take_host_result(world, independent).is_none());

    finish_operation_barrier(world, first, Ok(())).unwrap();
    resolve_documents(world);
    assert_eq!(active_operation_barrier(world), Some(second));
    assert_eq!(
        notices
            .read(world.resource::<Messages<CoreNotice>>())
            .map(|notice| notice.message.as_str())
            .collect::<Vec<_>>(),
        ["first"]
    );
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(command)
            .is_none()
    );
    assert!(take_host_result(world, independent).is_none());

    finish_operation_barrier(world, second, Ok(())).unwrap();
    resolve_documents(world);
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(command)
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        take_host_result(world, independent),
        Some(Ok(HostOutcome::Applied))
    );
    assert_eq!(
        notices
            .read(world.resource::<Messages<CoreNotice>>())
            .map(|notice| notice.message.as_str())
            .collect::<Vec<_>>(),
        ["second", "independent"]
    );
}

#[test]
fn an_asynchronous_model_failure_does_not_abandon_other_model_checkpoints() {
    let temp = TempDir::new();
    let mut app = app();
    let world = app.world_mut();
    let failed = create_model_document(
        world,
        Counter(0),
        "Missing destination",
        Some(temp.root().join("missing/model.txt")),
    );
    let saved_path = temp.root().join("saved.txt");
    let saved = create_model_document(world, Counter(0), "Saved", Some(saved_path.clone()));
    edit_model::<Counter>(world, failed, 0, 1, "Increment").unwrap();
    edit_model::<Counter>(world, saved, 0, 2, "Increment").unwrap();
    let save = begin_document_save(
        world,
        &[
            DocumentSaveTarget {
                document: failed,
                destination: None,
            },
            DocumentSaveTarget {
                document: saved,
                destination: None,
            },
        ],
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Some(result) = poll_document_save(world, save) {
            break result;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert!(result.is_err());
    assert!(document_dirty(world, failed).unwrap());
    assert!(!document_dirty(world, saved).unwrap());
    assert_eq!(std::fs::read(saved_path).unwrap(), b"2");
}

#[test]
fn a_rejected_typed_model_draft_prevents_quitting_a_clean_model() {
    fn reject(_: In<api::workbench::PanelActionInvocation>) -> Result<bool, String> {
        Err("Typed model draft was rejected".into())
    }
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", None);
    assert!(!document_dirty(world, document).unwrap());
    let panel = panel(world);
    let handler = world.register_system(reject);
    world.entity_mut(panel).insert((
        DocumentBinding(document),
        api::workbench::PanelActions(handler),
    ));
    assert_eq!(quit(world).unwrap_err(), "Typed model draft was rejected");
    assert!(world.non_send::<DocumentSession>().is_running());
    assert!(world.entities().contains(panel));
}
