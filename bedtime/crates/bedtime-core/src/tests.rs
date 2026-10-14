use crate::*;
use bedtime_api::{
    documents::{DocumentFrame, PanelBinding},
    features::HostAction,
};
use bedtime_documents::{ByteEdit, DocumentSession};
use bedtime_workbench::{WorkbenchLayout, WorkbenchRequests};
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
fn attached_layout_uses_saved_focus_unless_a_real_carried_panel_has_focus() {
    let mut original = app();
    let first = panel(original.world_mut());
    panel(original.world_mut());
    focus_panel(original.world_mut(), first, true).unwrap();
    let layout = original.world().resource::<WorkbenchLayout>().clone();
    let navigation = original.world().resource::<DocumentNavigation>().clone();
    let panels = panel_entities(original.world_mut());
    let saved =
        snapshot_document_layout(original.world_mut(), panels, &layout, &navigation).unwrap();
    for carry in [false, true] {
        let mut restored = app();
        let carried = carry.then(|| panel(restored.world_mut()));
        restore_document_layout(restored.world_mut(), &saved, true, None).unwrap();
        let focused = restored
            .world()
            .resource::<WorkbenchLayout>()
            .focused
            .unwrap();
        if let Some(carried) = carried {
            assert_eq!(focused, carried);
        } else {
            assert_eq!(
                restored
                    .world()
                    .get::<api::workbench::PanelPresentation>(focused)
                    .unwrap()
                    .id,
                saved["focused"].as_u64().unwrap()
            );
        }
    }
}

#[test]
#[cfg(feature = "language-services")]
fn same_file_navigation_returns_to_its_requesting_view_after_another_view_is_focused() {
    let directory = TempDir::new();
    let path = directory.write("source.rs", b"first\nsecond\n");
    let mut app = app();
    let world = app.world_mut();
    api::workbench::register(world, "navigation", |registrar| {
        registrar.panel(
            "navigation.panel",
            "Source",
            |In(input): In<api::workbench::PanelCreation>,
             world: &mut World|
             -> api::workbench::PanelResult {
                let document = world
                    .get::<PanelBinding>(input.entity)
                    .unwrap()
                    .document
                    .unwrap();
                let view = world
                    .non_send_mut::<DocumentSession>()
                    .create_view(document)
                    .map_err(|error| error.to_string())?;
                world.get_mut::<PanelBinding>(input.entity).unwrap().view = Some(view);
                Ok(Box::new(EmptyPanel))
            },
        );
    })
    .unwrap();
    world
        .resource_mut::<api::features::FeatureRegistry>()
        .register_with("navigation", |registrar| {
            registrar.panel_ref("navigation.panel");
            registrar.fallback_viewer(
                "navigation.text",
                "Source",
                "navigation.panel",
                bedtime_documents::DocumentKind::Text,
            );
        })
        .unwrap();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let requesting =
        open_document_panel(world, "navigation.panel", document, Value::Null, None).unwrap();
    let sibling = duplicate_panel(world, requesting, None).unwrap();
    let request_view = world.get::<PanelBinding>(requesting).unwrap().view.unwrap();
    let sibling_view = world.get::<PanelBinding>(sibling).unwrap().view.unwrap();
    let sibling_selection = world
        .non_send::<DocumentSession>()
        .view_snapshot(sibling_view)
        .unwrap()
        .selections;
    let receipt = submit_host_request(
        world,
        panel_presentation(world, requesting),
        HostAction::NavigateLocation {
            path: path.to_string_lossy().into_owned(),
            row: 1,
            utf16_column: 2,
            origin: None,
        },
    );
    focus_panel(world, sibling, true).unwrap();
    resolve_documents(world);
    assert_eq!(
        take_host_result(world, receipt),
        Some(Ok(HostOutcome::Opened(requesting)))
    );
    assert_eq!(
        world.resource::<DocumentFrame>().active_view,
        Some(request_view)
    );
    let snapshot = world
        .non_send::<DocumentSession>()
        .view_snapshot(request_view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 2));
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .view_snapshot(sibling_view)
            .unwrap()
            .selections,
        sibling_selection
    );
    assert_eq!(panel_entities(world).len(), 2);
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
        prepare_document_transition(world, barrier, bedtime_documents::ClosePolicy::Cancel)
            .unwrap(),
        OperationBarrierStatus::NeedsDecision(_)
    ));
    prepare_document_transition(world, barrier, bedtime_documents::ClosePolicy::Discard).unwrap();
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
    bedtime_workbench::open_panel(
        world,
        json!({"panel_type":"test.panel","state":null}),
        None,
        None,
    )
    .unwrap()
}
fn edit(world: &World, document: bedtime_documents::DocumentId, bytes: &[u8]) -> HostAction {
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
        .get_mut::<bedtime_workbench::PanelRecord>(source)
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
    bedtime_workbench::advance_layout_epoch(world);
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
    world.insert_resource(bedtime_workbench::WorkbenchClosePolicy(defer_close));
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
    assert!(bedtime_workbench::waiting_for_layout(world));
    world.resource_mut::<Messages<HostRequest>>().clear();
    bedtime_workbench::commit_close(world, source);
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
        .insert(bedtime_api::workbench::PanelClose(hook));
    world
        .entity_mut(second)
        .insert(bedtime_api::workbench::PanelClose(hook));
    let request = edit(world, dirty, b"changed ");
    submit_host_request(world, Some(second), request);
    resolve_documents(world);
    let snapshot = bedtime_workbench::save(world);
    assert!(bedtime_workbench::restore(world, &snapshot).is_err());
    assert!(world.resource::<Retired>().0.is_empty());
    assert!(world.entities().contains(first) && world.entities().contains(second));
    assert!(bedtime_workbench::request_close(world, first).unwrap());
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
    let snapshot = bedtime_workbench::save(world);
    bedtime_workbench::restore(world, &snapshot).unwrap();
    let restored = world
        .query::<(&PanelBinding, &bedtime_workbench::PanelRecord)>()
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
    bedtime_workbench::restore(fresh.world_mut(), &collided).unwrap();
    let record = fresh
        .world_mut()
        .query::<&bedtime_workbench::PanelRecord>()
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
        .resource_mut::<bedtime_api::features::FeatureRegistry>()
        .register_with("test", |registrar| {
            registrar.panel_ref("test.panel");
            registrar.fallback_viewer(
                "test.text",
                "Text",
                "test.panel",
                bedtime_documents::DocumentKind::Text,
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
        target: bedtime_api::workbench::PanelTarget::LastFocused,
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
fn document_commands_keep_the_captured_panel_and_tools_retain_shared_selection() {
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
    let captured = bedtime_workbench::capture_command(world, "bedtime.documents.undo", Value::Null);
    let tool = panel(world);
    resolve_documents(world);
    assert_eq!(
        world.resource::<DocumentFrame>().active_document,
        Some(document)
    );
    assert!(
        world.resource::<api::workbench::CommandPresentations>().0["bedtime.documents.undo"]
            .enabled
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
        .command("bedtime.documents.undo")
        .unwrap()
        .handler;
    let result = world
        .run_system_with(
            handler,
            api::workbench::CommandInvocation {
                command: "bedtime.documents.undo".into(),
                source: Some(tool),
                area: 1,
                arguments: json!({"document":null}),
            },
        )
        .unwrap();
    assert!(result.unwrap_err().contains("No document"));
}

#[test]
fn an_explicit_save_target_runs_while_the_current_tool_invocation_is_disabled() {
    let temp = TempDir::new();
    let path = temp.write("document.txt", b"initial");
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let editor = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    submit_host_request(world, Some(editor), edit(world, document, b"changed "));
    resolve_documents(world);
    let tool = panel(world);
    world.insert_resource(DocumentFrameSelection::default());
    resolve_documents(world);
    assert_eq!(world.resource::<DocumentFrame>().active_document, None);
    assert!(
        !world.resource::<api::workbench::CommandPresentations>().0["bedtime.documents.save"]
            .enabled
    );
    let invocation = bedtime_workbench::capture_command(
        world,
        "bedtime.documents.save",
        json!({"document":document.0}),
    );
    assert_eq!(invocation.source, panel_presentation(world, tool));
    let receipt = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::Invoke(invocation));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let outcome = loop {
        poll_document_services(world);
        resolve_documents(world);
        if let Some(outcome) = world
            .resource_mut::<WorkbenchRequests>()
            .take_result(receipt)
        {
            break outcome;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "captured save did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(outcome, Ok(bedtime_workbench::WorkbenchOutcome::Applied));
    assert_eq!(std::fs::read(path).unwrap(), b"changed initial");
}

#[test]
fn application_selection_is_published_by_core_and_can_be_explicitly_cleared() {
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"remembered")
        .unwrap();
    open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let tool = panel(world);
    world.insert_resource(DocumentFrameSelection {
        document: Some(document),
        view: None,
    });
    for _ in 0..3 {
        app.update();
        assert_eq!(
            app.world().resource::<DocumentFrame>().active_document,
            Some(document)
        );
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(tool)
        );
    }
    app.world_mut()
        .insert_resource(DocumentFrameSelection::default());
    app.update();
    assert!(
        app.world()
            .resource::<DocumentFrame>()
            .active_document
            .is_none()
    );
    app.world_mut().remove_resource::<DocumentFrameSelection>();
    app.update();
    assert!(
        app.world()
            .resource::<DocumentFrame>()
            .active_document
            .is_none()
    );
}

#[test]
fn shared_save_commands_use_the_explicit_document_selection() {
    let temp = TempDir::new();
    let path = temp.write("remembered.txt", b"before");
    let mut app = App::new();
    app.add_plugins(DocumentHostPlugin);
    api::workbench::register(app.world_mut(), "test", |registrar| {
        registrar.panel("test.panel", "Test", factory);
    })
    .unwrap();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let tool = panel(world);
    let revision = document_revision(world, document).unwrap();
    apply_edits(
        world,
        document,
        revision,
        &[ByteEdit {
            range: 0..6,
            bytes: b"after".to_vec(),
        }],
    )
    .unwrap();
    world.insert_resource(DocumentFrameSelection {
        document: Some(document),
        view: None,
    });
    publish_documents(world);
    let save = world
        .resource::<api::workbench::WorkbenchRegistry>()
        .command("bedtime.documents.save")
        .unwrap()
        .handler;
    world
        .run_system_with(
            save,
            api::workbench::CommandInvocation {
                command: "bedtime.documents.save".into(),
                source: Some(tool),
                area: 1,
                arguments: Value::Null,
            },
        )
        .unwrap()
        .unwrap();
    let effects = world.resource_mut::<api::features::HostEffects>().take();
    assert_eq!(effects.len(), 1);
    assert!(
        matches!(effects[0].action, HostAction::Save { document: selected } if selected == document)
    );
    let receipt = submit_host_request(
        world,
        Some(tool),
        effects.into_iter().next().unwrap().action,
    );
    wait_host_result(world, receipt).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"after");
    let registry = world.resource::<api::workbench::WorkbenchRegistry>();
    for command in [
        "bedtime.documents.open_file",
        "bedtime.documents.save",
        "bedtime.documents.save_as",
        "bedtime.documents.shell.open",
    ] {
        assert!(registry.command(command).is_some());
    }
}

#[test]
fn shared_save_arguments_keep_the_captured_target_and_reject_empty_or_invalid_ids() {
    let mut app = app();
    let world = app.world_mut();
    let first = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"first")
        .unwrap();
    let editor = open_document_panel(world, "test.panel", first, Value::Null, None).unwrap();
    let second = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"second")
        .unwrap();
    let tool = panel(world);
    world.insert_resource(DocumentFrameSelection {
        document: Some(second),
        view: None,
    });
    for command in ["bedtime.documents.save", "bedtime.documents.save_as"] {
        let handler = world
            .resource::<api::workbench::WorkbenchRegistry>()
            .command(command)
            .unwrap()
            .handler;
        world
            .run_system_with(
                handler,
                api::workbench::CommandInvocation {
                    command: command.into(),
                    source: Some(tool),
                    area: 1,
                    arguments: json!({"document":first.0}),
                },
            )
            .unwrap()
            .unwrap();
        let effects = world.resource_mut::<api::features::HostEffects>().take();
        assert!(
            matches!(effects.as_slice(), [effect] if matches!(effect.action, HostAction::SaveAsDialog { document } if document == first))
        );
        for document in [
            Value::Null,
            json!(0),
            json!(-1),
            json!("1"),
            json!(u64::MAX),
        ] {
            let result = world
                .run_system_with(
                    handler,
                    api::workbench::CommandInvocation {
                        command: command.into(),
                        source: Some(tool),
                        area: 1,
                        arguments: json!({"document":document}),
                    },
                )
                .unwrap();
            assert!(
                result.is_err(),
                "An empty or malformed captured selection cannot use the later document"
            );
            assert!(
                world
                    .resource_mut::<api::features::HostEffects>()
                    .take()
                    .is_empty()
            );
        }
        world
            .run_system_with(
                handler,
                api::workbench::CommandInvocation {
                    command: command.into(),
                    source: Some(editor),
                    area: 1,
                    arguments: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        let effects = world.resource_mut::<api::features::HostEffects>().take();
        assert!(
            matches!(effects.as_slice(), [effect] if matches!(effect.action, HostAction::SaveAsDialog { document } if document == first))
        );
    }
}

#[test]
fn open_file_is_available_with_a_direct_file_viewer_and_no_code_editor() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<api::features::FeatureRegistry>()
        .register_with("archive", |registrar| {
            registrar.panel_ref("archive.panel");
            registrar.local_file_viewer("archive.viewer", "Archive", "archive.panel", &["zip"]);
        })
        .unwrap();
    api::workbench::register(app.world_mut(), "archive", |registrar| {
        registrar.panel("archive.panel", "Archive", factory);
    })
    .unwrap();
    app.update();
    let commands = app
        .world()
        .resource::<api::workbench::CommandPresentations>();
    assert!(commands.0["bedtime.documents.open_file"].visible);
    assert!(commands.0["bedtime.documents.open_file"].enabled);
    assert!(!commands.0["bedtime.documents.new"].visible);
}

#[test]
fn model_history_commits_panel_drafts_before_undoing_the_latest_change() {
    #[derive(Component)]
    struct Draft(u32);
    let mut app = app();
    let world = app.world_mut();
    let document = create_model_document(world, Counter(1), "Counter", None);
    edit_model::<Counter>(world, document, 0, 2, "Initial change").unwrap();
    let panel = panel(world);
    let commit = world.register_system(
        |In(input): In<api::workbench::PanelActionInvocation>,
         world: &mut World|
         -> api::workbench::PanelActionResult {
            if input.action == api::workbench::PanelAction::CommitEdit
                && let Some(draft) = world.entity_mut(input.entity).take::<Draft>()
            {
                let document = world.get::<DocumentBinding>(input.entity).unwrap().0;
                let revision = model_document(world, document).unwrap().revision();
                edit_model::<Counter>(world, document, revision, draft.0, "Panel draft")?;
            }
            Ok(true)
        },
    );
    world.entity_mut(panel).insert((
        DocumentBinding(document),
        api::workbench::PanelActions(commit),
        Draft(3),
    ));
    let request = capture_host_request(world, Some(panel), HostAction::Undo { document });
    apply_document_request(world, request).unwrap();
    assert_eq!(model::<Counter>(world, document).unwrap().0, 2);
    assert!(world.get::<Draft>(panel).is_none());
    let request = capture_host_request(world, Some(panel), HostAction::Redo { document });
    apply_document_request(world, request).unwrap();
    assert_eq!(model::<Counter>(world, document).unwrap().0, 3);
}

#[test]
fn revision_checked_panel_edits_acknowledge_success_and_failure_once() {
    #[derive(Resource, Default)]
    struct Acknowledgements(Vec<bedtime_api::documents::PanelEditResult>);
    let mut app = app();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"old")
        .unwrap();
    let panel = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    world.init_resource::<Acknowledgements>();
    let handler = world.register_system(
        |In(result): In<bedtime_api::documents::PanelEditResult>,
         mut replies: ResMut<Acknowledgements>| {
            replies.0.push(result);
        },
    );
    world
        .entity_mut(panel)
        .insert(bedtime_api::documents::PanelEdits(handler));
    let revision = document_revision(world, document).unwrap();
    for (token, expected_revision) in [
        (bedtime_api::documents::EditToken(1), revision),
        (bedtime_api::documents::EditToken(2), revision),
    ] {
        let request = capture_host_request(
            world,
            Some(panel),
            HostAction::ApplyEditsWithResult {
                token,
                document,
                revision: expected_revision,
                edits: vec![ByteEdit {
                    range: 0..3,
                    bytes: b"new".to_vec(),
                }],
            },
        );
        let _ = apply_document_request(world, request);
    }
    let replies = &world.resource::<Acknowledgements>().0;
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0].token, bedtime_api::documents::EditToken(1));
    assert!(replies[0].result.is_ok());
    assert_eq!(replies[1].token, bedtime_api::documents::EditToken(2));
    assert!(replies[1].result.is_err());
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"new"
    );
}

#[test]
fn native_quit_waits_for_drawing_and_prior_document_operations() {
    let temp = TempDir::new();
    let path = temp.write("quit.txt", b"before");
    let mut app = app();
    app.init_resource::<bedtime_desktop::DesktopNativeActions>()
        .add_message::<AppExit>();
    let world = app.world_mut();
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let panel = open_document_panel(world, "test.panel", document, Value::Null, None).unwrap();
    let inactive = open_panel(
        world,
        "test.panel",
        bedtime_api::documents::PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    world
        .entity_mut(inactive)
        .insert(bedtime_api::workbench::PanelSuspended);
    let request = edit(world, document, b"after ");
    submit_host_request(world, None, request);
    let saved = submit_host_request(world, None, HostAction::Save { document });
    world
        .resource_mut::<bedtime_desktop::DesktopNativeActions>()
        .0
        .push(bedtime_desktop::DesktopNativeAction {
            command: "bedtime.core.quit".into(),
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
    assert!(!world.entities().contains(panel));
    assert!(!world.entities().contains(inactive));
    assert!(panel_entities(world).is_empty());
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
    let snapshot = bedtime_workbench::save(world);
    assert!(
        bedtime_workbench::restore(world, &snapshot)
            .unwrap_err()
            .contains("Save or discard")
    );
    assert!(world.entities().contains(first) && world.entities().contains(second));
    assert!(bedtime_workbench::closing_panels(world).is_none());
    assert!(document_dirty(world, document).unwrap());
    assert!(bedtime_workbench::request_close_many(world, &[first, second]).is_err());
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
    let generation = world.resource::<bedtime_files::FileContext>().generation;
    let id = submit_host_request(
        world,
        None,
        HostAction::OpenDirectory {
            location: bedtime_files::FileLocation {
                provider: bedtime_files::FileProviderId::Local,
                path: directory.clone(),
            },
        },
    );
    assert_eq!(wait_host_result(world, id), Ok(HostOutcome::Applied));
    assert_eq!(
        world.resource::<bedtime_files::FileContext>().directory,
        std::fs::canonicalize(directory).unwrap()
    );
    assert!(world.resource::<bedtime_files::FileContext>().generation > generation);
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
    let panel = bedtime_workbench::open_panel(
        world,
        json!({"panel_type":"test.panel","state":null}),
        None,
        None,
    )
    .unwrap();
    world.entity_mut(panel).insert(DocumentBinding(document));
    edit_model::<Counter>(world, document, 0, 2, "increment").unwrap();
    world.resource_mut::<DocumentClosePolicy>().0 = bedtime_documents::ClosePolicy::Save;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !bedtime_workbench::request_close_many(world, &[panel]).unwrap() {
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
    prepare_document_transition(world, barrier, bedtime_documents::ClosePolicy::Discard).unwrap();
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
    let original = bedtime_files::FileContext::local(temp.root()).unwrap();
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
            location: bedtime_files::FileLocation {
                provider: bedtime_files::FileProviderId::Local,
                path: next,
            },
        },
    );
    assert_eq!(
        wait_host_result(world, request).unwrap_err(),
        "Typed model draft was rejected"
    );
    assert_eq!(world.resource::<bedtime_files::FileContext>(), &original);
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
    let invocation =
        bedtime_workbench::capture_command(world, "test.continuations.run", Value::Null);
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

#[test]
fn another_viewer_opens_a_separate_tab_with_shared_content_and_history_without_bed() {
    fn view_factory(
        In(input): In<api::workbench::PanelCreation>,
        world: &mut World,
    ) -> api::workbench::PanelResult {
        let binding = world.get::<PanelBinding>(input.entity).unwrap();
        assert!(
            matches!(binding.viewer.as_deref(), Some("views.text" | "views.tree")),
            "Viewer identity is available to the factory"
        );
        let document = binding.document.unwrap();
        let view = world
            .non_send_mut::<DocumentSession>()
            .create_view(document)
            .map_err(|error| error.to_string())?;
        world.get_mut::<PanelBinding>(input.entity).unwrap().view = Some(view);
        Ok(Box::new(EmptyPanel))
    }
    let mut app = app();
    let world = app.world_mut();
    api::workbench::register(world, "views", |registrar| {
        registrar.panel("views.panel", "Document view", view_factory);
    })
    .unwrap();
    world
        .resource_mut::<api::features::FeatureRegistry>()
        .register_with("views", |registrar| {
            registrar.panel_ref("views.panel");
            registrar.viewer(
                "views.text",
                "Text",
                "views.panel",
                &["json"],
                bedtime_documents::DocumentKind::Text,
            );
            registrar.viewer(
                "views.tree",
                "Tree",
                "views.panel",
                &["json"],
                bedtime_documents::DocumentKind::Text,
            );
        })
        .unwrap();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(b"{} ")
        .unwrap();
    let first = open_document_viewer(world, document, "views.text", None).unwrap();
    let tab = world
        .get::<api::workbench::PanelPresentation>(first)
        .unwrap()
        .id;
    let request = capture_host_request(
        world,
        Some(first),
        HostAction::OpenDocumentViewer {
            tab,
            document,
            viewer: "views.tree".into(),
        },
    );
    let HostOutcome::Opened(second) = apply_document_request(world, request).unwrap() else {
        panic!("Another viewer opens a tab")
    };
    assert_ne!(first, second);
    assert_eq!(
        world.get::<PanelBinding>(first).unwrap().viewer.as_deref(),
        Some("views.text")
    );
    assert_eq!(
        world.get::<PanelBinding>(second).unwrap().document,
        Some(document)
    );
    let first_view = world.get::<PanelBinding>(first).unwrap().view.unwrap();
    let second_view = world.get::<PanelBinding>(second).unwrap().view.unwrap();
    assert_ne!(first_view, second_view);
    assert_eq!(
        world.resource::<WorkbenchLayout>().tiling.areas[0]
            .tabs
            .len(),
        2
    );
    assert_eq!(
        open_document_viewer(world, document, "views.tree", None).unwrap(),
        second
    );
    apply_document_request(
        world,
        capture_host_request(world, Some(first), edit(world, document, b"shared ")),
    )
    .unwrap();
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"shared {} "
    );
    undo_document(world, document).unwrap();
    assert_eq!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"{} "
    );
    bedtime_workbench::commit_close(world, first);
    assert!(!retire_document(world, document).unwrap());
    assert!(world.entities().contains(second));
    assert_eq!(world.non_send::<DocumentSession>().view_count(document), 1);
}

#[cfg(all(unix, feature = "language-services"))]
#[path = "format_save_tests.rs"]
mod format_save_tests;

#[test]
fn asking_before_close_cancels_without_retiring_or_writing_the_document() {
    let temp = TempDir::new();
    let path = temp.write("ask.txt", b"before");
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    world.insert_resource(DocumentCloseDecisionPolicy::Ask);
    let panel = open_file(world, &path, None, None).unwrap();
    let document = world.get::<PanelBinding>(panel).unwrap().document.unwrap();
    apply_edits(
        world,
        document,
        document_revision(world, document).unwrap(),
        &[ByteEdit {
            range: 0..0,
            bytes: b"dirty ".to_vec(),
        }],
    )
    .unwrap();
    let receipt = world
        .resource_mut::<bedtime_workbench::WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(panel));
    resolve_documents(world);
    let prompt = world.resource::<CloseDecisionPrompt>().token;
    assert!(world.entities().contains(panel));
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
    assert!(answer_close_prompt(world, prompt, CloseDecision::Cancel));
    resolve_documents(world);
    assert!(world.entities().contains(panel));
    assert!(document_dirty(world, document).unwrap());
    assert!(
        world
            .resource_mut::<bedtime_workbench::WorkbenchRequests>()
            .take_result(receipt)
            .unwrap()
            .is_err()
    );
}

#[test]
fn failed_precommit_persistence_retains_the_entire_old_context() {
    let temp = TempDir::new();
    let path = temp.write("document.txt", b"before");
    let destination = temp.path("other");
    std::fs::create_dir(&destination).unwrap();
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    let panel = open_file(world, &path, None, None).unwrap();
    let document = world.get::<PanelBinding>(panel).unwrap().document.unwrap();
    let session = world.non_send::<DocumentSession>().session_id();
    let original = world.resource::<bedtime_files::FileContext>().clone();
    let fail = world.register_system(|In(_): In<LifecycleReason>| -> Result<(), String> {
        Err("database commit failed".into())
    });
    world.resource_mut::<LifecyclePreCommit>().0.push(fail);
    let token = capture_operation_barrier(world, None);
    resolve_documents(world);
    let options = world.non_send::<DocumentSession>().options().clone();
    let status = prepare_context_change(
        world,
        token,
        ContextChange {
            context: bedtime_files::FileContext::local(&destination).unwrap(),
            services: bedtime_files::FileServices::local(),
            options,
            mode: ContextChangeMode::ReplaceDocuments,
        },
    )
    .unwrap();
    assert_eq!(status, OperationBarrierStatus::Prepared);
    assert_eq!(
        commit_prepared_context_change(world, token).unwrap_err(),
        "database commit failed"
    );
    assert_eq!(world.non_send::<DocumentSession>().session_id(), session);
    assert_eq!(
        world.resource::<bedtime_files::FileContext>().directory,
        original.directory
    );
    assert!(world.entities().contains(panel));
    assert_eq!(
        world.get::<PanelBinding>(panel).unwrap().document,
        Some(document)
    );
    finish_operation_barrier(world, token, Err("database commit failed".into())).unwrap();
}

#[test]
fn pathless_model_close_saves_through_its_writer_before_retiring_the_view() {
    #[derive(Clone)]
    struct Query(String);
    impl DocumentModel for Query {
        type Change = String;
        fn apply(&mut self, change: &String) -> Result<String, String> {
            Ok(std::mem::replace(&mut self.0, change.clone()))
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.as_bytes().to_vec())
        }
    }
    #[derive(Default)]
    struct Writer(std::sync::Mutex<Vec<(String, Vec<u8>)>>);
    impl DocumentWriter for Writer {
        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().push((key.into(), bytes.to_vec()));
            Ok(())
        }
    }
    let mut app = app();
    let world = app.world_mut();
    world.insert_resource(DocumentCloseDecisionPolicy::Ask);
    let writer = std::sync::Arc::new(Writer::default());
    let document = create_model_document(world, Query("select 1".into()), "Query", None);
    bind_model_document_writer(world, document, "query:1", writer.clone()).unwrap();
    let revision = model_document(world, document).unwrap().revision();
    edit_model::<Query>(world, document, revision, "select 2".into(), "Edit query").unwrap();
    let panel = bedtime_workbench::spawn_panel(
        world,
        json!({"panel_type":"query.panel","state":null}),
        None,
        None,
    )
    .unwrap();
    world.entity_mut(panel).insert(DocumentBinding(document));
    let receipt = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(panel));
    resolve_documents(world);
    let token = world.resource::<CloseDecisionPrompt>().token;
    assert!(world.entities().contains(panel));
    assert!(writer.0.lock().unwrap().is_empty());
    assert!(answer_close_prompt(world, token, CloseDecision::Save));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        poll_document_services(world);
        resolve_documents(world);
        if let Some(result) = world
            .resource_mut::<WorkbenchRequests>()
            .take_result(receipt)
        {
            result.unwrap();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "model close did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!world.entities().contains(panel));
    assert_eq!(
        *writer.0.lock().unwrap(),
        [(String::from("query:1"), b"select 2".to_vec())]
    );
    assert!(!document_dirty(world, document).unwrap());
}

#[test]
fn malformed_document_records_are_rejected_before_any_panel_is_restored() {
    let mut app = app();
    let world = app.world_mut();
    let value = json!({"version":2,"panels":[
        {"id":1,"panel_type":"missing.feature","state":{"opaque":true}},
        {"id":2,"panel_type":false}
    ]});
    assert!(restore_document_layout(world, &value, false, None).is_err());
    assert!(panel_entities(world).is_empty());
}

#[test]
fn panel_effects_and_nested_command_receipts_wait_for_the_actual_writer() {
    #[derive(Clone)]
    struct Draft(String);
    impl DocumentModel for Draft {
        type Change = String;
        fn apply(&mut self, change: &String) -> Result<String, String> {
            Ok(std::mem::replace(&mut self.0, change.clone()))
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.as_bytes().to_vec())
        }
    }
    struct Writer {
        started: std::sync::mpsc::Sender<()>,
        released: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        bytes: std::sync::Mutex<Vec<u8>>,
    }
    impl DocumentWriter for Writer {
        fn write(&self, _: &str, bytes: &[u8]) -> Result<(), String> {
            self.started.send(()).map_err(|error| error.to_string())?;
            self.released
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| error.to_string())?;
            *self.bytes.lock().unwrap() = bytes.to_vec();
            Ok(())
        }
    }
    let mut app = app();
    let world = app.world_mut();
    let (started, observed_start) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = std::sync::Arc::new(Writer {
        started,
        released: std::sync::Mutex::new(released),
        bytes: Default::default(),
    });
    let document = create_model_document(world, Draft("original".into()), "Draft", None);
    bind_model_document_writer(world, document, "draft", writer.clone()).unwrap();
    let revision = model_document(world, document).unwrap().revision();
    edit_model::<Draft>(world, document, revision, "captured".into(), "Edit draft").unwrap();
    let captured_revision = document_revision(world, document).unwrap();
    api::workbench::register(world, "test.nested", |registrar| {
        registrar.command(
            "test.nested.save",
            "Save draft",
            move |In(_): In<api::workbench::CommandInvocation>, world: &mut World| {
                world
                    .resource_mut::<api::features::HostEffects>()
                    .push(None, HostAction::Save { document });
                world.resource_mut::<api::features::HostEffects>().push(
                    None,
                    HostAction::Notify {
                        message: "nested save finished".into(),
                    },
                );
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    let source = bedtime_workbench::spawn_panel(
        world,
        json!({"panel_type":"tool.panel","state":null}),
        None,
        None,
    )
    .unwrap();
    let handler = world.register_system(
        |In(invocation): In<api::workbench::PanelActionInvocation>,
         world: &mut World|
         -> Result<bool, String> {
            world.resource_mut::<api::features::HostEffects>().push(
                Some(invocation.entity),
                HostAction::Command {
                    command: "test.nested.save".into(),
                    context: Default::default(),
                },
            );
            Ok(true)
        },
    );
    world
        .entity_mut(source)
        .insert(api::workbench::PanelActions(handler));
    let receipt = submit_host_request(
        world,
        Some(source),
        HostAction::PanelAction {
            panel: source,
            action: api::workbench::PanelAction::Find,
        },
    );
    let independent = submit_host_request(
        world,
        None,
        HostAction::Notify {
            message: "independent".into(),
        },
    );
    let mut notices = bevy::ecs::message::MessageCursor::<CoreNotice>::default();
    resolve_documents(world);
    observed_start
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(take_host_result(world, receipt).is_none());
    assert!(take_host_result(world, independent).is_none());
    assert!(document_dirty(world, document).unwrap());
    assert_eq!(
        notices
            .read(world.resource::<Messages<CoreNotice>>())
            .count(),
        0
    );
    release.send(()).unwrap();
    assert_eq!(
        wait_host_result(world, receipt).unwrap(),
        HostOutcome::Saved {
            documents: vec![document],
            revisions: vec![captured_revision],
        }
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
        ["nested save finished", "independent"]
    );
    assert_eq!(*writer.bytes.lock().unwrap(), b"captured");
    assert!(!document_dirty(world, document).unwrap());
}

#[test]
fn final_view_discard_retires_unsaved_data_and_reopen_reads_the_saved_file() {
    let temp = TempDir::new();
    let path = temp.write("discard.txt", b"saved");
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    world.insert_resource(DocumentCloseDecisionPolicy::Ask);
    let first = open_file(world, &path, None, None).unwrap();
    let document = world.get::<PanelBinding>(first).unwrap().document.unwrap();
    let second = open_file_instance(world, &path, None, true, None).unwrap();
    apply_edits(
        world,
        document,
        document_revision(world, document).unwrap(),
        &[ByteEdit {
            range: 0..5,
            bytes: b"unsaved".to_vec(),
        }],
    )
    .unwrap();
    let first_close = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(first));
    resolve_documents(world);
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(first_close)
            .unwrap()
            .is_ok()
    );
    assert!(world.entities().contains(second));
    assert!(document_dirty(world, document).unwrap());
    assert!(!world.contains_resource::<CloseDecisionPrompt>());
    let last_close = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::ClosePanel(second));
    resolve_documents(world);
    let token = world.resource::<CloseDecisionPrompt>().token;
    answer_close_prompt(world, token, CloseDecision::Discard);
    resolve_documents(world);
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(last_close)
            .unwrap()
            .is_ok()
    );
    assert!(
        world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .is_err()
    );
    let reopened = open_file(world, &path, None, None).unwrap();
    let reopened_document = world
        .get::<PanelBinding>(reopened)
        .unwrap()
        .document
        .unwrap();
    assert_ne!(reopened_document, document);
    let snapshot = world
        .non_send::<DocumentSession>()
        .snapshot(reopened_document)
        .unwrap();
    assert_eq!(snapshot.bytes, b"saved");
    assert!(!snapshot.dirty);
}

#[test]
fn retaining_one_tool_saves_and_retires_its_peers_without_rebuilding_the_caller() {
    let temp = TempDir::new();
    let path = temp.write("peer.txt", b"before");
    let destination = temp.path("destination");
    std::fs::create_dir(&destination).unwrap();
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    world.insert_resource(DocumentClosePolicy(bedtime_documents::ClosePolicy::Save));
    let caller = panel(world);
    let caller_frontend = panel_presentation(world, caller).unwrap();
    let peer = open_file(world, &path, None, None).unwrap();
    let document = world.get::<PanelBinding>(peer).unwrap().document.unwrap();
    apply_edits(
        world,
        document,
        document_revision(world, document).unwrap(),
        &[ByteEdit {
            range: 0..6,
            bytes: b"after".to_vec(),
        }],
    )
    .unwrap();
    let session = world.non_send::<DocumentSession>().session_id();
    let old_directory = world
        .resource::<bedtime_files::FileContext>()
        .directory
        .clone();
    let before = world.register_system(
        move |In(_): In<LifecycleReason>, world: &mut World| -> Result<(), String> {
            assert!(world.entities().contains(caller));
            assert!(world.entities().contains(peer));
            assert_eq!(
                world.resource::<bedtime_files::FileContext>().directory,
                old_directory
            );
            Ok(())
        },
    );
    world.resource_mut::<LifecyclePreCommit>().0.push(before);
    let barrier = capture_operation_barrier(world, Some(caller));
    resolve_documents(world);
    let options = world.non_send::<DocumentSession>().options().clone();
    prepare_context_change(
        world,
        barrier,
        ContextChange {
            context: bedtime_files::FileContext::local(&destination).unwrap(),
            services: bedtime_files::FileServices::local(),
            options,
            mode: ContextChangeMode::RetainPanel(caller),
        },
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        poll_document_services(world);
        if poll_context_change(world, barrier).unwrap() == OperationBarrierStatus::Prepared {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "peer saves did not finish"
        );
    }
    commit_prepared_context_change(world, barrier).unwrap();
    finish_operation_barrier(world, barrier, Ok(())).unwrap();
    assert_eq!(world.non_send::<DocumentSession>().session_id(), session);
    assert_eq!(panel_presentation(world, caller), Some(caller_frontend));
    assert!(world.entities().contains(caller));
    assert!(!world.entities().contains(peer));
    assert!(
        world
            .non_send::<DocumentSession>()
            .document_ids()
            .is_empty()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"after");
    assert_eq!(active_panel_entities(world), vec![caller]);
    assert_eq!(world.resource::<WorkbenchLayout>().focused, Some(caller));
    assert_eq!(
        world.resource::<bedtime_files::FileContext>().directory,
        destination.canonicalize().unwrap()
    );
}

#[test]
fn one_discard_decision_closes_the_complete_dirty_panel_group() {
    let temp = TempDir::new();
    let first_path = temp.write("first.txt", b"first");
    let second_path = temp.write("second.txt", b"second");
    let mut app = app();
    let world = app.world_mut();
    viewers(world);
    world.insert_resource(DocumentCloseDecisionPolicy::Ask);
    let first = open_file(world, &first_path, None, None).unwrap();
    let second = open_file(world, &second_path, None, None).unwrap();
    for panel in [first, second] {
        let document = world.get::<PanelBinding>(panel).unwrap().document.unwrap();
        apply_edits(
            world,
            document,
            document_revision(world, document).unwrap(),
            &[ByteEdit {
                range: 0..0,
                bytes: b"dirty ".to_vec(),
            }],
        )
        .unwrap();
    }
    let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
    tiling.retain_panels([]);
    let receipt = world.resource_mut::<WorkbenchRequests>().submit(
        api::workbench::WorkbenchAction::SetLayout {
            tiling: tiling.to_value([]),
            close: vec![first, second],
        },
    );
    resolve_documents(world);
    let prompt = world.resource::<CloseDecisionPrompt>();
    assert_eq!(prompt.documents.len(), 2);
    let token = prompt.token;
    assert!(answer_close_prompt(world, token, CloseDecision::Discard));
    resolve_documents(world);
    assert!(
        world
            .resource_mut::<WorkbenchRequests>()
            .take_result(receipt)
            .unwrap()
            .is_ok()
    );
    assert!(!world.contains_resource::<CloseDecisionPrompt>());
    assert!(!world.entities().contains(first));
    assert!(!world.entities().contains(second));
    assert_eq!(std::fs::read(first_path).unwrap(), b"first");
    assert_eq!(std::fs::read(second_path).unwrap(), b"second");
}

#[test]
fn a_focused_owner_model_binding_drives_save_and_undo_without_child_bindings() {
    #[derive(Clone)]
    struct Counter(u64);
    impl DocumentModel for Counter {
        type Change = u64;
        fn apply(&mut self, change: &u64) -> Result<u64, String> {
            Ok(std::mem::replace(&mut self.0, *change))
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.to_string().into_bytes())
        }
    }
    let temp = TempDir::new();
    let path = temp.path("counter.txt");
    let mut app = app();
    let world = app.world_mut();
    let owner = panel(world);
    let child = panel_presentation(world, owner).unwrap();
    assert!(world.get::<DocumentBinding>(child).is_none());
    let document = create_model_document(world, Counter(1), "Counter", Some(path.clone()));
    world.entity_mut(owner).insert(DocumentBinding(document));
    let revision = model_document(world, document).unwrap().revision();
    edit_model::<Counter>(world, document, revision, 2, "Set counter").unwrap();
    bedtime_workbench::focus_panel(world, owner);
    publish_documents(world);
    assert_eq!(
        world.resource::<DocumentFrame>().active_document,
        Some(document)
    );
    assert!(
        world.resource::<api::workbench::CommandPresentations>().0["bedtime.documents.save"]
            .enabled
    );
    let save = bedtime_workbench::capture_command(world, "bedtime.documents.save", Value::Null);
    let receipt = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::Invoke(save));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        poll_document_services(world);
        resolve_documents(world);
        if let Some(result) = world
            .resource_mut::<WorkbenchRequests>()
            .take_result(receipt)
        {
            result.unwrap();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "owner-bound save did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(std::fs::read(path).unwrap(), b"2");
    let undo = bedtime_workbench::capture_command(world, "bedtime.documents.undo", Value::Null);
    let receipt = world
        .resource_mut::<WorkbenchRequests>()
        .submit(api::workbench::WorkbenchAction::Invoke(undo));
    resolve_documents(world);
    world
        .resource_mut::<WorkbenchRequests>()
        .take_result(receipt)
        .unwrap()
        .unwrap();
    assert_eq!(model::<Counter>(world, document).unwrap().0, 1);
    assert!(document_dirty(world, document).unwrap());
}
