//! Host/viewer invariants require no application layout, theme, or workspace policy.
use crate::headless_host;
use crate::temp_dir;
use bedtime_api::{
    documents::PanelBinding,
    features::HostAction,
    workbench::{PanelCreation, PanelTarget, WorkbenchAction},
};
use bedtime_core::*;
use bedtime_documents::{ByteEdit, DocumentSession};
use bedtime_workbench::{PanelPresentation, PanelRecord, WorkbenchLayout};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};
use temp_dir::TempDir;

fn unstarted_app(dir: &TempDir) -> App {
    let mut app = App::new();
    headless_host::install(&mut app, &dir.path("config"));
    app.add_plugins((
        bedtime_plugin_editor::EditorPlugin,
        bedtime_plugin_json_tree::JsonTreePlugin,
    ));
    bedtime_core::initialize_file_context(
        app.world_mut(),
        bedtime_files::FileContext::local(dir.root()).unwrap(),
        bedtime_files::FileServices::local(),
    )
    .unwrap();
    app
}
fn app(dir: &TempDir) -> App {
    let mut app = unstarted_app(dir);
    bedtime_core::settings::finalize_settings(app.world_mut()).unwrap();
    app.update();
    app
}
fn layout_snapshot(world: &mut World) -> Result<Value, String> {
    let panels = panel_entities(world);
    let layout = world.resource::<WorkbenchLayout>().clone();
    let navigation = world.resource::<DocumentNavigation>().clone();
    snapshot_document_layout(world, panels, &layout, &navigation)
}
fn open_viewer(world: &mut World, panel: Entity, viewer: &str) -> Result<Entity, String> {
    let binding = world.get::<PanelBinding>(panel).unwrap();
    let action = HostAction::OpenDocumentViewer {
        tab: world.get::<PanelPresentation>(panel).unwrap().id,
        document: binding.document.unwrap(),
        viewer: viewer.into(),
    };
    match headless_host::host(world, Some(panel), action)? {
        HostOutcome::Opened(panel) => Ok(panel),
        _ => panic!("viewer opens a tab"),
    }
}
fn preview(app: &mut App, path: &Path, area: u32) -> Entity {
    let request = bedtime_core::capture_host_request_area(
        app.world(),
        None,
        HostAction::PreviewFileIn {
            path: path.to_string_lossy().into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        },
        Some(area),
    );
    let id = submit_captured_host_request(app.world_mut(), request);
    match headless_host::wait_host(app.world_mut(), id).unwrap() {
        HostOutcome::Opened(panel) => panel,
        outcome => panic!("{outcome:?}"),
    }
}

#[test]
fn previews_replace_across_viewer_types_and_keep_existing_tab_order() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = unstarted_app(&dir);
    app.add_plugins(bedtime_plugin_image_viewer::ImageViewerPlugin);
    bedtime_core::settings::finalize_settings(app.world_mut()).unwrap();
    app.update();
    let kept = open_file_instance(
        app.world_mut(),
        &dir.write("kept.txt", b"kept"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    let path = dir.write("image.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>"#);
    let HostOutcome::Opened(image) = headless_host::host(
        app.world_mut(),
        None,
        HostAction::PreviewFileIn {
            path: path.to_string_lossy().into_owned(),
            viewer: Some(bedtime_plugin_image_viewer::VIEWER_ID.into()),
            target: PanelTarget::LargestArea,
        },
    )
    .unwrap() else {
        panic!("image preview opens a tab")
    };
    assert_eq!(
        app.world()
            .get::<PanelBinding>(image)
            .unwrap()
            .viewer
            .as_deref(),
        Some(bedtime_plugin_image_viewer::VIEWER_ID)
    );
    let image_surface = panel_presentation(app.world(), image).unwrap();
    let text = preview(&mut app, &dir.write("preview.txt", b"preview"), 1);
    assert!(!app.world().entities().contains(image));
    assert!(!app.world().entities().contains(image_surface));
    assert!(app.world().entities().contains(kept));
    let group = &app.world().resource::<WorkbenchLayout>().tiling.areas[0];
    assert_eq!(
        group.tabs,
        vec![
            app.world().get::<PanelPresentation>(kept).unwrap().id,
            app.world().get::<PanelPresentation>(text).unwrap().id
        ]
    );
    assert_eq!(
        group.preview,
        Some(app.world().get::<PanelPresentation>(text).unwrap().id)
    );
    assert_eq!(group.selected, group.preview);
}

#[test]
fn local_file_viewers_share_the_preview_slot_with_document_viewers() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    struct LocalViewer;

    impl bedtime_api::workbench::Panel for LocalViewer {
        fn title(&self, _: &World) -> String {
            "Local viewer".into()
        }

        fn draw(
            &mut self,
            _: &dear_imgui_rs::Ui,
            _: &mut World,
            _: &mut Vec<WorkbenchAction>,
            _: &bedtime_api::workbench::ShortcutInput,
        ) {
        }
    }
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.world_mut()
        .resource_mut::<bedtime_api::features::FeatureRegistry>()
        .register_with("fixture.local", |registrar| {
            registrar.panel_ref("fixture.local.panel");
            registrar.local_file_viewer(
                "fixture.local.viewer",
                "Local viewer",
                "fixture.local.panel",
                &["local"],
            );
        })
        .unwrap();
    bedtime_api::workbench::register(app.world_mut(), "fixture.local", |registrar| {
        registrar.panel(
            "fixture.local.panel",
            "Local viewer",
            |_: In<PanelCreation>| -> bedtime_api::workbench::PanelResult {
                Ok(Box::new(LocalViewer))
            },
        );
    })
    .unwrap();
    let local = preview(&mut app, &dir.write("first.local", b"\0local"), 1);
    let binding = app.world().get::<PanelBinding>(local).unwrap();
    assert!(binding.document.is_none());
    assert!(binding.input.local_file().is_some());
    let text = preview(&mut app, &dir.write("text.txt", b"text"), 1);
    assert!(!app.world().entities().contains(local));
    let second = preview(&mut app, &dir.write("second.local", b"\0local"), 1);
    assert!(!app.world().entities().contains(text));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(second).unwrap().id)
    );
    let path = dir.root().join("second.local");
    let duplicate = open_file_instance(app.world_mut(), &path, None, true, Some(1)).unwrap();
    assert_ne!(duplicate, second);
    assert_eq!(
        open_file_instance(app.world_mut(), &path, None, false, Some(1)).unwrap(),
        duplicate,
        "ordinary local-file opening still chooses its most recently focused presentation"
    );
}

#[test]
fn preview_requests_do_not_demote_kept_files_and_explicit_opens_retain_previews() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let kept_path = dir.write("kept.txt", b"kept");
    let kept = open_file_instance(app.world_mut(), &kept_path, None, false, Some(1)).unwrap();
    let path = dir.write("preview.txt", b"preview");
    let first = preview(&mut app, &path, 1);
    assert_eq!(preview(&mut app, &kept_path, 1), kept);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(first).unwrap().id)
    );
    assert_eq!(
        open_file_instance(app.world_mut(), &path, None, false, Some(1)).unwrap(),
        first
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    let second = preview(&mut app, &dir.write("second.txt", b"second"), 1);
    // Drop/menu-style ordinary openings create kept tabs beside the preview.
    let other = open_file_instance(
        app.world_mut(),
        &dir.write("other.txt", b"other"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    assert!(app.world().entities().contains(first));
    assert!(app.world().entities().contains(second));
    assert!(app.world().entities().contains(other));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(second).unwrap().id)
    );
}

#[test]
fn plugin_keep_requests_and_edits_retain_the_owning_preview_tab() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let request =
        bedtime_api::settings::SettingsRequest::new("bedtime.documents", "autosave", false);
    bedtime_core::settings::apply_settings_request(app.world_mut(), &request).unwrap();
    bedtime_core::settings::configure_document_settings(app.world_mut()).unwrap();
    let first = preview(&mut app, &dir.write("first.txt", b"first"), 1);
    let surface = panel_presentation(app.world(), first).unwrap();
    headless_host::workbench(app.world_mut(), WorkbenchAction::KeepPanel(surface)).unwrap();
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    let second = preview(&mut app, &dir.write("second.txt", b"second"), 1);
    let document = app
        .world()
        .get::<PanelBinding>(second)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    headless_host::host(
        app.world_mut(),
        Some(second),
        HostAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..6,
                bytes: b"edited".to_vec(),
            }],
        },
    )
    .unwrap();
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    // Saving cannot restore replaceability once editing promoted the tab.
    headless_host::host(
        app.world_mut(),
        Some(second),
        HostAction::SaveDocuments {
            documents: vec![document],
        },
    )
    .unwrap();
    preview(&mut app, &dir.write("third.txt", b"third"), 1);
    assert!(app.world().entities().contains(first));
    assert!(app.world().entities().contains(second));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"edited"
    );
}

#[test]
fn each_area_has_its_own_preview_and_failed_opens_preserve_it() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let area = app
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .split_area(1, 0, true, [100, 100])
        .unwrap();
    let left = preview(&mut app, &dir.write("left.txt", b"left"), 1);
    let right = preview(&mut app, &dir.write("right.txt", b"right"), area);
    let before = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let request = bedtime_core::HostRequest::capture(
        app.world().non_send::<DocumentSession>().session_id(),
        None,
        HostAction::PreviewFileIn {
            path: dir
                .root()
                .join("missing.txt")
                .to_string_lossy()
                .into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        },
        1,
        app.world()
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
            .0,
    );
    assert!(bedtime_core::apply_document_request(app.world_mut(), request).is_err());
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, before);
    preview(&mut app, &dir.write("replacement.txt", b"replacement"), 1);
    assert!(!app.world().entities().contains(left));
    assert!(app.world().entities().contains(right));
}

#[test]
fn different_viewers_have_independent_tabs_for_one_shared_document() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("values.json", b"{\"value\":1}");
    let text_tab = open_file_instance(
        app.world_mut(),
        &path,
        Some(&bedtime_api::ViewerId::new("bedtime.editor.text")),
        false,
        None,
    )
    .unwrap();
    let text = panel_presentation(app.world(), text_tab).unwrap();
    let json_tab = open_viewer(app.world_mut(), text_tab, "bedtime.json.viewer").unwrap();
    let json = panel_presentation(app.world(), json_tab).unwrap();
    assert_ne!(text_tab, json_tab);
    assert_eq!(panel_presentation(app.world(), text_tab), Some(text));
    let document = app
        .world()
        .get::<PanelBinding>(text_tab)
        .unwrap()
        .document
        .unwrap();
    assert_eq!(
        app.world().get::<PanelBinding>(json_tab).unwrap().document,
        Some(document)
    );
    assert_eq!(
        open_viewer(app.world_mut(), json_tab, "bedtime.editor.text").unwrap(),
        text_tab
    );
    assert_eq!(
        open_file_instance(
            app.world_mut(),
            &path,
            Some(&bedtime_api::ViewerId::new("bedtime.json.viewer")),
            false,
            None
        )
        .unwrap(),
        json_tab
    );
    let area = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|area| {
            area.tabs
                .contains(&app.world().get::<PanelPresentation>(text_tab).unwrap().id)
        })
        .unwrap();
    assert!(
        area.tabs
            .contains(&app.world().get::<PanelPresentation>(json_tab).unwrap().id)
    );
    headless_host::workbench(app.world_mut(), WorkbenchAction::ClosePanel(text_tab)).unwrap();
    assert!(!app.world().entities().contains(text));
    assert!(app.world().entities().contains(json));
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .is_ok()
    );
    headless_host::workbench(app.world_mut(), WorkbenchAction::ClosePanel(json_tab)).unwrap();
    assert!(!app.world().entities().contains(json));
}

#[test]
fn a_registered_viewer_factory_failure_retains_the_active_presentation_and_document() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    #[derive(Component)]
    struct FailedAllocation;
    let dir = TempDir::new();
    let mut app = app(&dir);
    bedtime_workbench::register(app.world_mut(), "fixture", |registrar| {
        registrar.panel(
            "fixture.panel",
            "Rejected viewer",
            |In(creation): In<PanelCreation>,
             world: &mut World|
             -> bedtime_api::workbench::PanelResult {
                world.entity_mut(creation.entity).insert(FailedAllocation);
                Err("Factory rejected the input".into())
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<bedtime_api::features::FeatureRegistry>()
        .register_with("fixture", |registrar| {
            registrar.panel_ref("fixture.panel");
            registrar.viewer(
                "fixture.viewer",
                "Rejected viewer",
                "fixture.panel",
                &[],
                bedtime_documents::DocumentKind::Text,
            );
        })
        .unwrap();
    let path = dir.write("document.txt", b"original");
    let tab = open_file_instance(app.world_mut(), &path, None, false, None).unwrap();
    let surface = panel_presentation(app.world(), tab).unwrap();
    let binding = app.world().get::<PanelBinding>(tab).unwrap().clone();
    let document = binding.document.unwrap();
    let view = binding.view.unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.type_text(b"unsaved "))
        .unwrap();
    let before = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap();
    let selections = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap()
        .selections;
    let document = binding.document.unwrap();
    let tab_id = app.world().get::<PanelPresentation>(tab).unwrap().id;
    let id = submit_host_request(
        app.world_mut(),
        Some(tab),
        HostAction::OpenDocumentViewer {
            tab: tab_id,
            document,
            viewer: "fixture.viewer".into(),
        },
    );
    app.update();
    assert!(
        take_host_result(app.world_mut(), id)
            .unwrap()
            .unwrap_err()
            .contains("Factory rejected")
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().focused, Some(tab));
    assert_eq!(panel_presentation(app.world(), tab), Some(surface));
    assert_eq!(app.world().get::<PanelBinding>(tab).unwrap(), &binding);
    assert!(
        app.world()
            .get::<PanelRecord>(tab)
            .unwrap()
            .unavailable
            .is_none()
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        before.bytes
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .selections,
        selections
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
    assert_eq!(
        app.world_mut()
            .query::<&FailedAllocation>()
            .iter(app.world())
            .count(),
        0
    );
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}

#[test]
fn missing_feature_records_roundtrip_without_discarding_opaque_data() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let record = json!({"id":37,"panel_type":"missing.feature.panel","viewer":"missing.feature.viewer","state":{"draft":"keep"},"extension":{"future":[1,2,3]}});
    let value = json!({"version":2,"panels":[record.clone()],"focused":37});
    restore_document_layout(app.world_mut(), &value, false, None).unwrap();
    let panel = panel_entities(app.world_mut())[0];
    assert_eq!(app.world().get::<PanelPresentation>(panel).unwrap().id, 37);
    let saved = layout_snapshot(app.world_mut()).unwrap();
    assert_eq!(saved["panels"][0]["extension"], record["extension"]);
    assert_eq!(saved["panels"][0]["state"], record["state"]);
}

#[test]
fn unavailable_viewers_and_missing_files_keep_their_original_records() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for missing_file in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let path = if missing_file {
            dir.root().join("missing.txt")
        } else {
            dir.write("exists.txt", b"retained")
        };
        let viewer = if missing_file {
            bedtime_plugin_editor::TEXT_VIEWER
        } else {
            "missing.viewer"
        };
        let record = json!({"id":41,"panel_type":bedtime_plugin_editor::TEXT_PANEL_TYPE,"viewer":viewer,"path":path,"state":{"position":17},"future":{"keep":[1,2,3]}});
        restore_document_layout(
            app.world_mut(),
            &json!({"version":2,"panels":[record.clone()],"focused":41}),
            false,
            None,
        )
        .unwrap();
        let panel = panel_entities(app.world_mut())[0];
        let surface = panel_presentation(app.world(), panel).unwrap();
        assert!(
            app.world()
                .get::<PanelRecord>(surface)
                .unwrap()
                .unavailable
                .is_some()
        );
        assert!(
            app.world()
                .get::<bedtime_plugin_editor::EditorPanelState>(surface)
                .is_none(),
            "unavailable content cannot silently create an unrelated scratch editor"
        );
        let saved = layout_snapshot(app.world_mut()).unwrap();
        assert_eq!(saved["panels"][0], record);
        assert_eq!(
            bedtime_workbench::panel_state(app.world_mut(), panel).unwrap(),
            record["state"]
        );
    }
}

#[test]
fn invalid_saved_identities_and_membership_recover_all_file_contents() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for identities in [[1, 1], [0, 2], [1, 2]] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let files = [
            dir.write("first.txt", b"first"),
            dir.write("second.txt", b"second"),
        ];
        let records=files.iter().zip(identities).map(|(path,id)|json!({"id":id,"panel_type":bedtime_plugin_editor::TEXT_PANEL_TYPE,"viewer":bedtime_plugin_editor::TEXT_VIEWER,"path":path,"state":{}})).collect::<Vec<_>>();
        restore_document_layout(
            app.world_mut(),
            &json!({"version":2,"panels":records,"tiling":{"version":1,"layout":{},"areas":[]}}),
            false,
            None,
        )
        .unwrap();
        let panels = panel_entities(app.world_mut());
        assert_eq!(panels.len(), 2);
        let ids = panels
            .iter()
            .map(|panel| app.world().get::<PanelPresentation>(*panel).unwrap().id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&0));
        for (panel, expected) in panels
            .iter()
            .zip([b"first".as_slice(), b"second".as_slice()])
        {
            let document = app
                .world()
                .get::<PanelBinding>(*panel)
                .unwrap()
                .document
                .unwrap();
            assert_eq!(
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .bytes,
                expected
            );
        }
        assert!(!app.world().resource::<Messages<CoreNotice>>().is_empty());
        let saved = layout_snapshot(app.world_mut()).unwrap();
        let fresh = TempDir::new();
        let mut restored = self::app(&fresh);
        restore_document_layout(restored.world_mut(), &saved, false, None).unwrap();
        assert_eq!(panel_entities(restored.world_mut()).len(), 2);
    }
}

#[test]
fn restored_empty_areas_and_failed_splits_never_create_or_discard_content() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let mut layout = bedtime_workbench::workspace::tiling::Layout::default();
    let empty = layout.split(1, 0, 100, 1).unwrap();
    let tiling = bedtime_workbench::workspace::tiling_state::TilingState::new(layout);
    restore_document_layout(
        app.world_mut(),
        &json!({"version":2,"panels":[],"tiling":tiling.to_value([]),"focused_area":empty}),
        false,
        None,
    )
    .unwrap();
    assert!(panel_entities(app.world_mut()).is_empty());
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, tiling);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused_area,
        Some(empty)
    );
    let panel = open_file_instance(
        app.world_mut(),
        &dir.write("narrow.txt", b"narrow"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let before = app.world().resource::<WorkbenchLayout>().tiling.clone();
    assert!(split_panel(app.world_mut(), panel, 0, true).is_err());
    assert_eq!(panel_entities(app.world_mut()), vec![panel]);
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, before);
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
}

#[test]
fn opening_a_multi_kind_viewer_reuses_the_live_document_kind_and_unsaved_contents() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    struct BothKinds;
    impl bedtime_api::workbench::Panel for BothKinds {
        fn title(&self, _: &World) -> String {
            "Both kinds".into()
        }
        fn draw(
            &mut self,
            _: &dear_imgui_rs::Ui,
            _: &mut World,
            _: &mut Vec<WorkbenchAction>,
            _: &bedtime_api::workbench::ShortcutInput,
        ) {
        }
    }
    bedtime_workbench::register(app.world_mut(), "dual", |registrar| {
        registrar.panel(
            "dual.panel",
            "Both kinds",
            |_: In<PanelCreation>| -> bedtime_api::workbench::PanelResult {
                Ok(Box::new(BothKinds))
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<bedtime_api::features::FeatureRegistry>()
        .register_with("dual", |registrar| {
            registrar.panel_ref("dual.panel");
            registrar.viewer(
                "dual.viewer",
                "Both kinds",
                "dual.panel",
                &["json"],
                bedtime_documents::DocumentKind::Bytes,
            );
            registrar.viewer_document_kinds(
                "dual.viewer",
                &[
                    bedtime_documents::DocumentKind::Text,
                    bedtime_documents::DocumentKind::Bytes,
                ],
            );
        })
        .unwrap();
    let path = dir.write("values.json", b"{\"value\":1}");
    let text = open_file_instance(
        app.world_mut(),
        &path,
        Some(&bedtime_api::ViewerId::from(
            bedtime_plugin_editor::TEXT_VIEWER,
        )),
        false,
        None,
    )
    .unwrap();
    let binding = app.world().get::<PanelBinding>(text).unwrap().clone();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(binding.view.unwrap(), |commands| {
            commands.type_text(b"unsaved ")
        })
        .unwrap();
    let other = open_file_instance(
        app.world_mut(),
        &path,
        Some(&bedtime_api::ViewerId::new("dual.viewer")),
        false,
        None,
    )
    .unwrap();
    assert_ne!(text, other);
    assert_eq!(
        app.world().get::<PanelBinding>(other).unwrap().document,
        binding.document
    );
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(binding.document.unwrap())
        .unwrap();
    assert_eq!(snapshot.kind, bedtime_documents::DocumentKind::Text);
    assert_eq!(snapshot.bytes, b"unsaved {\"value\":1}");
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"value\":1}");
}

#[test]
fn opening_a_file_keeps_a_pristine_core_viewer_tab_and_its_widget_intact() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut app = app(&dir);
    let document = app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .create_document(b"")
        .unwrap();
    let core_tab = bedtime_core::open_document_viewer(
        app.world_mut(),
        document,
        &bedtime_api::ViewerId::from(bedtime_plugin_editor::TEXT_VIEWER),
        None,
    )
    .unwrap();
    focus_panel(app.world_mut(), core_tab, true).unwrap();
    let original = app.world().get::<PanelBinding>(core_tab).unwrap().clone();
    let file = dir.write("next.txt", b"next");
    let opened = open_file_instance(app.world_mut(), &file, None, false, None).unwrap();
    assert_ne!(opened, core_tab);
    assert_eq!(
        app.world().get::<PanelBinding>(core_tab).unwrap(),
        &original
    );
    assert!(
        app.world()
            .non_send::<bedtime_plugin_editor::EditorViews>()
            .text(panel_presentation(app.world(), core_tab).unwrap())
            .is_some()
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .document_for_view(original.view.unwrap()),
        Some(document)
    );
}

#[test]
fn different_viewer_tabs_restore_separately_with_shared_document_and_per_tab_state() {
    let _context_lock = crate::IMGUI_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = TempDir::new();
    let mut original = app(&dir);
    let path = dir.write("values.json", b"{\"value\":1}");
    let text = open_file_instance(
        original.world_mut(),
        &path,
        Some(&bedtime_api::ViewerId::from(
            bedtime_plugin_editor::TEXT_VIEWER,
        )),
        false,
        None,
    )
    .unwrap();
    let view = original
        .world()
        .get::<PanelBinding>(text)
        .unwrap()
        .view
        .unwrap();
    original
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(
                0,
                5,
                false,
                bedtime_editing::text_commands::CursorReveal::Ensure,
            );
        })
        .unwrap();
    let json = open_viewer(original.world_mut(), text, "bedtime.json.viewer").unwrap();
    assert_ne!(text, json);
    let saved = layout_snapshot(original.world_mut()).unwrap();
    let viewer_records = saved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| {
            record["viewer"] == bedtime_plugin_editor::TEXT_VIEWER
                || record["viewer"] == "bedtime.json.viewer"
        })
        .collect::<Vec<_>>();
    assert_eq!(viewer_records.len(), 2);
    assert!(
        viewer_records
            .iter()
            .all(|record| record.get("presentations").is_none())
    );
    let text_record = viewer_records
        .iter()
        .find(|record| record["viewer"] == bedtime_plugin_editor::TEXT_VIEWER)
        .unwrap();
    assert_eq!(text_record["state"]["selections"][0][1], 5);

    let fresh = TempDir::new();
    let mut restored = app(&fresh);
    restore_document_layout(restored.world_mut(), &saved, false, None).unwrap();
    let tabs = panel_entities(restored.world_mut())
        .into_iter()
        .filter(|panel| {
            restored
                .world()
                .get::<PanelBinding>(*panel)
                .is_some_and(|binding| {
                    binding.viewer.as_deref() == Some(bedtime_plugin_editor::TEXT_VIEWER)
                        || binding.viewer.as_deref() == Some("bedtime.json.viewer")
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(tabs.len(), 2);
    let bindings = tabs
        .iter()
        .map(|tab| restored.world().get::<PanelBinding>(*tab).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bindings[0].document, bindings[1].document);
    let frame = restored
        .world()
        .resource::<bedtime_api::documents::DocumentFrame>();
    assert_eq!(
        frame.active_document, bindings[0].document,
        "The selected JSON viewer retains the shared document selection"
    );
    assert!(frame.active_view.is_none(), "JSON has no text editing view");
    let text = bindings
        .iter()
        .find(|binding| binding.viewer.as_deref() == Some(bedtime_plugin_editor::TEXT_VIEWER))
        .unwrap();
    let snapshot = restored
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(text.view.unwrap())
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (0, 5));
    let resaved = layout_snapshot(restored.world_mut()).unwrap();
    assert_eq!(
        resaved["panels"]
            .as_array()
            .unwrap()
            .iter()
            .filter(
                |record| record["viewer"] == bedtime_plugin_editor::TEXT_VIEWER
                    || record["viewer"] == "bedtime.json.viewer"
            )
            .count(),
        2
    );
}
