use super::*;
use crate::test_support::TempDir;
#[cfg(unix)]
use std::process::Command;
use std::time::Instant;

#[cfg(unix)]
fn client() -> RemoteClient {
    let executable = std::env::current_exe().unwrap();
    let helper = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bed-headless");
    if helper.is_file() {
        return RemoteClient::launch_local(helper).unwrap();
    }
    // A fresh package-only test run may not have built the sibling binary yet.
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .args([
            "run",
            "--quiet",
            "--offline",
            "-p",
            "bed-headless",
            "--",
            "--stdio",
        ]);
    RemoteClient::launch_command(command).unwrap()
}
#[cfg(unix)]
fn workspace(temp: &TempDir, root: &str) -> Workbench {
    let mut settings = Settings::with_paths(
        temp.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.terminal_visible = false;
    settings.settings["terminal_visible"] = json!(false);
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
    workbench
        .activate_remote_workspace(
            WorkspaceSpec {
                name: "Remote fixture".into(),
                target: WorkspaceTarget::Ssh {
                    host: "test-host".into(),
                },
                root: root.into(),
            },
            SshTarget {
                host: "test-host".into(),
                agent: "/cache/bed/version-one/bed-headless".into(),
            },
            client(),
        )
        .unwrap();
    let mut options = workbench.session.options().clone();
    options.autosave = None;
    options.lsp_config = None;
    workbench.session.configure(options).unwrap();
    workbench
}
fn wait(workbench: &mut Workbench, predicate: impl Fn(&Workbench) -> bool) {
    wait_for(workbench, Duration::from_secs(10), predicate);
}
fn wait_for(workbench: &mut Workbench, timeout: Duration, predicate: impl Fn(&Workbench) -> bool) {
    let deadline = Instant::now() + timeout;
    loop {
        workbench.tick().unwrap();
        if predicate(workbench) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "remote UI completion timed out: {:?}",
            workbench.error
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn structure_outline(
    workbench: &Workbench,
) -> Option<std::cell::Ref<'_, bed_highlight::outline::OutlineService>> {
    workbench.modules.instances.iter().find_map(|plugin| {
        plugin
            .as_any()
            .downcast_ref::<bed_plugin_structure::StructurePlugin>()?
            .outline()
    })
}

#[cfg(unix)]
#[test]
fn remote_new_file_menu_keeps_its_previous_area_through_dialog_and_async_creation() {
    let temp = TempDir::new();
    temp.write("project/seed.txt", b"existing");
    let root = temp.path("project").canonicalize().unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let source = workbench.tabs[0].id;
    let settings = workbench
        .open_native_panel("settings", &Value::Null)
        .unwrap();
    let mut layout = crate::workspace::tiling::Layout::default();
    let destination = layout.split(1, 0, 7000, 1).unwrap();
    workbench.tiling = crate::workspace::tiling_state::TilingState::new(layout);
    for tab in &workbench.tabs {
        workbench.tiling.assign(tab.id, 1);
    }
    workbench.tiling.assign(settings, destination);
    workbench.dock_built = true;
    let settings_index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == settings)
        .unwrap();
    let source_index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == source)
        .unwrap();
    // The Files/menu origin is current; its small working area is previous.
    workbench.switch_to_tab(settings_index);
    workbench.switch_to_tab(source_index);
    workbench
        .handle_tree_action(FileTreeAction::NewFile(root.to_string_lossy().into_owned()))
        .unwrap();
    let dialog = workbench.file_dialog.take().unwrap();
    assert_eq!(dialog.target_area, Some(destination));
    workbench.switch_to_tab(settings_index);

    let path = root.join("menu.txt");
    let before = workbench.tabs.len();
    workbench
        .apply_file_action_at(&dialog.action, "menu.txt", dialog.target_area)
        .unwrap();
    assert!(workbench.remote_ui.mutation_pending());
    assert_eq!(workbench.tabs.len(), before);
    assert!(workbench.session.document_for_path(&path).is_none());
    workbench.switch_to_tab(source_index);
    workbench.switch_to_tab(settings_index);

    wait(&mut workbench, |w| {
        w.session.document_for_path(&path).is_some_and(|document| {
            w.tabs.iter().any(|tab| {
                tab.panel.document() == Some(document)
                    && w.area_for_panel(tab.id) == Some(destination)
            })
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"");
    assert_eq!(workbench.focused_area(), Some(destination));

    let ordinary = root.join("ordinary.txt");
    workbench
        .apply_file_action(&dialog.action, "ordinary.txt")
        .unwrap();
    wait(&mut workbench, |w| {
        w.session
            .document_for_path(&ordinary)
            .is_some_and(|document| {
                w.tabs.iter().any(|tab| {
                    tab.panel.document() == Some(document) && w.area_for_panel(tab.id) == Some(1)
                })
            })
    });
    assert_eq!(std::fs::read(&ordinary).unwrap(), b"");
    workbench.cleanup().unwrap();
}

#[cfg(unix)]
#[test]
fn remote_document_previews_share_unsaved_text_and_restore_viewers() {
    let temp = TempDir::new();
    let markdown = temp.write("project/guide.md", b"# Remote\n");
    let json = temp.write("project/data.jsonc", b"{ /* comment */ \"value\": 1, }");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    for (path, viewer, replacement) in [
        (
            &markdown,
            bed_plugin_markdown::VIEWER_ID,
            b"# Unsaved\n".as_slice(),
        ),
        (
            &json,
            bed_plugin_json::VIEWER_ID,
            b"{ /* comment */ \"value\": 2, }".as_slice(),
        ),
    ] {
        let original = std::fs::read(path).unwrap();
        workbench.open_or_focus(path).unwrap();
        let canonical = std::fs::canonicalize(path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        wait(&mut workbench, |w| {
            w.active_snapshot()
                .is_some_and(|snapshot| snapshot.path == canonical)
        });
        let document = workbench.active_document().unwrap();
        let tab = workbench.active_panel_id().unwrap();
        assert!(
            workbench
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .unwrap()
                .panel
                .editor()
                .is_some()
        );
        let revision = workbench.session.document_revision(document).unwrap();
        workbench
            .modules
            .requests
            .push(bed_workbench_api::HostRequest::ApplyEdits {
                document,
                revision,
                edits: vec![bed_document_session::editor_session::ByteEdit {
                    range: 0..original.len(),
                    bytes: replacement.to_vec(),
                }],
            });
        workbench.process_plugin_requests().unwrap();
        workbench
            .switch_document_viewer(tab, document, viewer)
            .unwrap();
        assert_eq!(workbench.active_snapshot().unwrap().bytes, replacement);
        assert_eq!(std::fs::read(path).unwrap(), original);
        workbench.session.undo_document(document).unwrap();
        assert_eq!(workbench.active_snapshot().unwrap().bytes, original);
        // Persist clean documents so restoration checks the remote snapshot and viewer choice.
    }
    workbench.persist_workspace().unwrap();
    let state = workbench.last_state.clone().unwrap();
    workbench.restore_workspace(&state).unwrap();
    wait(&mut workbench, |w| {
        w.remote_ui.restore.is_empty() && w.session.document_ids().len() == 2
    });
    for viewer in [bed_plugin_markdown::VIEWER_ID, bed_plugin_json::VIEWER_ID] {
        assert!(
            workbench
                .tabs
                .iter()
                .any(|tab| tab.panel.viewer.as_deref() == Some(viewer))
        );
    }
}
#[cfg(unix)]
#[test]
fn remote_structure_uses_unsaved_local_buffer_and_restored_document_target() {
    let temp = TempDir::new();
    let file = temp.write("project/file.rs", b"mod demo { fn saved() {} }\n");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let file = std::fs::canonicalize(file).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let state = json!({"panels":[
            {"kind":"document","id":11,"path":file.to_str().unwrap()},
            {"kind":"structure","id":12},
            {"kind":"structure","id":13}
        ],"focused":13,"active_document_panel":11});
    workbench.restore_workspace(&state).unwrap();
    wait(&mut workbench, |w| {
        w.remote_ui.restore.is_empty()
            && structure_outline(w).is_some_and(|outline| outline.result().is_some())
    });
    assert_eq!(workbench.active_panel_id(), Some(13));
    assert_eq!(workbench.panel_count("structure"), 2);
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"fn unsaved() {}\n"))
        .unwrap();
    wait(&mut workbench, |w| {
        structure_outline(w).is_some_and(|outline| {
            !outline.updating()
                && outline
                    .result()
                    .is_some_and(|result| result.nodes.iter().any(|node| node.label == "unsaved"))
        })
    });
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"mod demo { fn saved() {} }\n"
    );
    let jump = {
        let outline = structure_outline(&workbench).unwrap();
        let result = outline.result().unwrap();
        bed_plugin_structure::presentation::StructureJump {
            key: result.key.clone(),
            offset: result
                .nodes
                .iter()
                .find(|node| node.label == "saved")
                .unwrap()
                .name_range
                .start,
        }
    };
    let request = bed_plugin_structure::StructurePlugin::navigation_request(
        jump,
        &workbench.modules.frame.context(),
    )
    .unwrap();
    workbench.modules.requests.push(request);
    workbench.process_plugin_requests().unwrap();
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(workbench.active_panel_id(), Some(11));
}
#[cfg(unix)]
#[test]
fn bulk_tab_close_waits_for_remote_saves_and_keeps_new_panels() {
    let temp = TempDir::new();
    let first = temp.write("project/a.rs", b"a");
    let second = temp.write("project/b.rs", b"b");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    for path in [&first, &second] {
        workbench.open_or_focus(path).unwrap();
        wait(&mut workbench, |w| {
            w.session.document_ids().len() == if path == &first { 1 } else { 2 }
        });
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.type_text(b"edited "))
            .unwrap();
    }
    let indices = (0..workbench.tabs.len()).collect();
    assert!(!workbench.close_tabs(indices).unwrap());
    assert!(workbench.pending_tab_close.is_some());
    workbench
        .dispatch(crate::WindowCommand::NewSettings)
        .unwrap();
    let added = workbench.focused;
    wait(&mut workbench, |w| w.pending_tab_close.is_none());
    assert_eq!(std::fs::read(first).unwrap(), b"edited a");
    assert_eq!(std::fs::read(second).unwrap(), b"edited b");
    assert_eq!(workbench.tabs.len(), 1);
    assert_eq!(workbench.focused, added);
    assert_eq!(workbench.tabs[0].panel.kind, bed_module_settings::PANEL_ID);
}

#[cfg(unix)]
#[test]
fn corner_area_close_waits_for_remote_save_and_cancels_if_geometry_changes() {
    for change_layout in [false, true] {
        let temp = TempDir::new();
        let source_path = temp.write("project/source.rs", b"source");
        let target_path = temp.write("project/target.rs", b"target");
        let root = std::fs::canonicalize(temp.path("project")).unwrap();
        let mut workbench = workspace(&temp, root.to_str().unwrap());
        while !workbench.tabs.is_empty() {
            assert!(workbench.close_tab(0).unwrap());
        }
        for (count, path) in [(1, &source_path), (2, &target_path)] {
            workbench.open_or_focus(path).unwrap();
            wait(&mut workbench, |w| w.session.document_ids().len() == count);
        }
        let source = workbench.tabs[0].id;
        let target = workbench.tabs[1].id;
        let target_document = workbench.tabs[1].panel.document().unwrap();
        let target_view = workbench.tabs[1].panel.view_id().unwrap();
        workbench
            .session
            .with_commands(target_view, |commands| commands.type_text(b"edited "))
            .unwrap();
        assert!(workbench.session.snapshot(target_document).unwrap().dirty);

        let mut layout = crate::workspace::tiling::Layout::default();
        let target_area = layout.split(1, 0, 5000, 100).unwrap();
        workbench.tiling = crate::workspace::tiling_state::TilingState::new(layout.clone());
        workbench.pending_tiling = None;
        workbench.tiling.sync_area(1, vec![source], Some(source));
        workbench
            .tiling
            .sync_area(target_area, vec![target], Some(target));
        workbench.dock_built = true;
        workbench.switch_to_tab(0);
        let plan = layout.plan_join(1, target_area, [100, 100]).unwrap();

        assert!(
            !workbench
                .close_tiling_area(1, layout.clone(), plan, vec![target])
                .unwrap()
        );
        assert!(matches!(
            workbench.pending_tab_close,
            Some(PendingTabClose::Area { .. })
        ));
        assert_eq!(workbench.current_tiling().layout, layout);
        assert_eq!(
            workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            vec![source, target]
        );
        assert!(workbench.session.save_pending(target_document));
        let expected_layout = if change_layout {
            assert_eq!(
                workbench
                    .tiling
                    .layout
                    .move_border(0, 5000, [0, 10_000], 6000, 100),
                6000
            );
            workbench.tiling.layout.clone()
        } else {
            crate::workspace::tiling::Layout::default()
        };
        wait(&mut workbench, |w| {
            w.pending_tab_close.is_none() && !w.session.save_pending(target_document)
        });

        assert_eq!(std::fs::read(&target_path).unwrap(), b"edited target");
        assert_eq!(std::fs::read(&source_path).unwrap(), b"source");
        assert_eq!(workbench.focused, Some(source));
        if change_layout {
            assert_eq!(workbench.current_tiling().layout, expected_layout);
            assert_eq!(
                workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                vec![source, target]
            );
            assert_eq!(workbench.area_for_panel(target), Some(target_area));
            assert!(!workbench.session.snapshot(target_document).unwrap().dirty);
        } else {
            assert_eq!(workbench.current_tiling().layout.areas.len(), 1);
            assert_eq!(
                workbench.current_tiling().layout.area(1).rect,
                expected_layout.area(1).rect
            );
            assert_eq!(workbench.tabs.len(), 1);
            assert_eq!(workbench.tabs[0].id, source);
            assert_eq!(workbench.tiling.areas[0].tabs, vec![source]);
            assert_eq!(workbench.tiling.areas[0].selected, Some(source));
            assert_eq!(workbench.session.document_ids().len(), 1);
        }
        workbench.cleanup().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn enabling_remote_autosave_schedules_existing_unsaved_edits() {
    let temp = TempDir::new();
    let file = temp.write("project/code.rs", b"code");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    workbench.settings.settings["autosave"] = json!(false);
    workbench.sync_services().unwrap();
    workbench.open_or_focus(&file).unwrap();
    wait(&mut workbench, |w| w.active_document().is_some());
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"edited "))
        .unwrap();
    workbench.tick().unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"code");
    workbench.settings.settings["autosave"] = json!(true);
    workbench.settings.settings["autosave_delay_ms"] = json!(100);
    workbench.sync_services().unwrap();
    wait(&mut workbench, |w| !w.active_snapshot().unwrap().dirty);
    assert_eq!(std::fs::read(file).unwrap(), b"edited code");
}
#[cfg(unix)]
#[test]
fn remote_open_save_rename_keep_identity_and_remove_closes_clean_views() {
    let temp = TempDir::new();
    let project = temp.path("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = temp.write("project/file.txt", b"hello");
    let root = std::fs::canonicalize(&project).unwrap();
    let file = std::fs::canonicalize(file).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    assert!(workbench.open_or_focus(&file).unwrap());
    assert!(workbench.active_document().is_none());
    wait(&mut workbench, |w| w.active_document().is_some());
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"edited "))
        .unwrap();
    let index = workbench.active_tab_index().unwrap();
    assert!(!workbench.close_tab(index).unwrap());
    assert!(workbench.session.save_pending(document));
    assert!(
        workbench
            .tabs
            .iter()
            .any(|tab| tab.panel.view_id() == Some(view))
    );
    wait(&mut workbench, |w| !w.session.save_pending(document));
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(std::fs::read(&file).unwrap(), b"edited hello");
    workbench.rename_path(&file, "renamed.txt").unwrap();
    assert!(workbench.remote_ui.mutation_pending());
    wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
    let renamed = root.join("renamed.txt");
    assert_eq!(
        workbench.session.snapshot(document).unwrap().path,
        renamed.to_str().unwrap()
    );
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(workbench.session.document_for_view(view), Some(document));
    workbench.trash_path(&renamed).unwrap();
    wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
    assert!(workbench.session.snapshot(document).is_err());
    assert_eq!(workbench.session.document_for_view(view), None);
    assert!(!renamed.exists());
}
#[cfg(unix)]
#[test]
fn remote_binary_image_uses_its_viewer_and_shares_bytes_with_hex() {
    let temp = TempDir::new();
    let file = temp.write(
        "project/picture.PNG",
        b"\x89PNG\0\0\0\0binary image payload",
    );
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let file = std::fs::canonicalize(file).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    workbench.open_or_focus(&file).unwrap();
    wait(&mut workbench, |workbench| {
        workbench.session.document_for_path(&file).is_some()
    });
    let document = workbench.session.document_for_path(&file).unwrap();
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    assert_eq!(
        workbench.tabs.last().unwrap().panel.viewer.as_deref(),
        Some(bed_plugin_image::VIEWER_ID)
    );
    workbench
        .open_file_with_viewer(&file, Some("bed.hex"), true)
        .unwrap();
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.document() == Some(document))
            .count(),
        2
    );
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[bed_document_session::ByteEdit {
                range: 0..1,
                bytes: vec![0xff],
            }],
        )
        .unwrap();
    let bytes = workbench.session.snapshot(document).unwrap().bytes;
    let hex = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.hex().is_some())
        .unwrap();
    assert!(
        workbench.close_tab(hex).unwrap(),
        "closing a sibling view must retain the shared dirty document"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    let image = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.document() == Some(document))
        .unwrap();
    assert!(
        !workbench.close_tab(image).unwrap(),
        "last attached panel must wait for queued SSH save"
    );
    wait(&mut workbench, |workbench| {
        !workbench.session.save_pending(document)
    });
    assert!(workbench.close_tab(image).unwrap());
    assert_eq!(std::fs::read(file).unwrap(), bytes);
    assert!(workbench.session.snapshot(document).is_err());
}
#[cfg(unix)]
#[test]
fn remote_restore_preserves_hex_and_plugin_viewers_attached_to_one_byte_document() {
    let temp = TempDir::new();
    let file = temp.write("project/picture.png", b"\0\xffraw image bytes\r\n");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let file = std::fs::canonicalize(file).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let path = file.to_str().unwrap();
    let state = json!({"panels":[
            {"kind":"hex","id":51,"path":path,"viewer":"bed.hex","document_kind":"bytes","state":{"cursor":5,"anchor":3,"insert":true}},
            {"kind":"plugin","id":52,"path":path,"viewer":bed_plugin_image::VIEWER_ID,"panel_type":bed_plugin_image::PANEL_ID,"document_kind":"bytes","state":{"fit":false,"zoom":2.0,"pan":[3.0,4.0]}}
        ],"focused":51});
    workbench.restore_workspace(&state).unwrap();
    wait(&mut workbench, |workbench| {
        workbench.remote_ui.restore.is_empty() && workbench.session.document_ids().len() == 1
    });
    let document = workbench.session.document_for_path(&file).unwrap();
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Bytes
    );
    assert_eq!(workbench.active_panel_id(), Some(51));
    let hex = workbench.tabs.iter().find(|tab| tab.id == 51).unwrap();
    let hex = hex.panel.hex().expect("hex view restored as another panel");
    assert_eq!(hex.state(), json!({"cursor":5,"anchor":3,"insert":true}));
    let plugin = workbench.tabs.iter().find(|tab| tab.id == 52).unwrap();
    let plugin = &plugin.panel;
    assert_eq!(plugin.kind, bed_plugin_image::PANEL_ID);
    assert_eq!(plugin.viewer.as_deref(), Some(bed_plugin_image::VIEWER_ID));
    assert_eq!(plugin.instance.attached_document(), Some(document));
    assert_eq!(plugin.instance.save_state()["zoom"], json!(2.0));
    assert_eq!(plugin.instance.save_state()["pan"], json!([3.0, 4.0]));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"\0\xffraw image bytes\r\n"
    );
}
#[cfg(unix)]
#[test]
fn remote_layout_restores_shared_views_and_positions_after_open() {
    let temp = TempDir::new();
    let file = temp.write("project/file.txt", b"one\ntwo\nthree");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let file = std::fs::canonicalize(file).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let path = file.to_str().unwrap();
    let state = json!({"panels":[
            {"kind":"document","id":11,"path":path,"selections":[[1,2,1,0]],"primary":0,"scroll":[0,0]},
            {"kind":"document","id":12,"path":path,"selections":[[2,1,2,1]],"primary":0,"scroll":[0,0]}
        ],"focused":12,"active_document_panel":12});
    workbench.restore_workspace(&state).unwrap();
    assert_eq!(workbench.session.document_ids().len(), 0);
    wait(&mut workbench, |w| {
        w.remote_ui.restore.is_empty() && w.session.document_ids().len() == 1
    });
    let document = workbench.active_document().unwrap();
    assert_eq!(workbench.session.view_ids(document).len(), 2);
    let view = workbench
        .tabs
        .iter()
        .find(|tab| tab.id == 11)
        .and_then(|tab| tab.panel.view_id())
        .unwrap();
    let snapshot = workbench.session.view_snapshot(view).unwrap();
    assert_eq!(snapshot.selections[0].head_row, 1);
    assert_eq!(snapshot.selections[0].head_column, 2);
    assert_eq!(snapshot.selections[0].anchor_column, 0);
    assert_eq!(workbench.active_panel_id(), Some(12));
}
#[cfg(unix)]
#[test]
fn canonical_alias_and_failed_connection_preserve_active_buffer_and_views() {
    let temp = TempDir::new();
    let file = temp.write("project/file.txt", b"buffer");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let spec = workbench.workspace_spec.clone().unwrap();
    workbench.open_or_focus(&file).unwrap();
    wait(&mut workbench, |w| w.active_document().is_some());
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"unsaved "))
        .unwrap();
    // Connecting through ~/ or another canonical alias must retain the
    // current document, undo and remembered display name after validation.
    let mut unnamed_alias = spec.clone();
    unnamed_alias.name.clear();
    workbench.remote_ui.ready = Some(ConnectionResult {
        reconnect: false,
        result: Ok(ConnectedWorkspace {
            spec: unnamed_alias,
            target: workbench.session.ssh_target().unwrap().clone(),
            client: client(),
        }),
    });
    workbench.poll_remote_workspace().unwrap();
    assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
    assert_eq!(workbench.active_view(), Some(view));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"unsaved buffer"
    );
    workbench.remote_ui.ready = Some(ConnectionResult {
        reconnect: false,
        result: Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "fixture failed",
        )),
    });
    workbench.poll_remote_workspace().unwrap();
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"unsaved buffer"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert!(workbench.error.as_ref().unwrap().contains("fixture failed"));
}
#[cfg(unix)]
#[test]
fn disconnected_canonical_alias_reconnects_without_closing_dirty_views() {
    let temp = TempDir::new();
    let file = temp.write("project/file.txt", b"buffer");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let spec = workbench.workspace_spec.clone().unwrap();
    workbench.open_or_focus(&file).unwrap();
    wait(&mut workbench, |w| w.active_document().is_some());
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"unsaved "))
        .unwrap();
    workbench.session.remote_client().unwrap().disconnect();
    assert!(!workbench.session.remote_connected());
    let mut unnamed_alias = spec.clone();
    unnamed_alias.name.clear();
    workbench.remote_ui.ready = Some(ConnectionResult {
        reconnect: false,
        result: Ok(ConnectedWorkspace {
            spec: unnamed_alias,
            target: workbench.session.ssh_target().unwrap().clone(),
            client: client(),
        }),
    });
    workbench.poll_remote_workspace().unwrap();
    assert!(workbench.session.remote_connected());
    assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
    assert_eq!(workbench.active_view(), Some(view));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"unsaved buffer"
    );
    workbench
        .session
        .with_commands(view, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"buffer"
    );
}
#[cfg(unix)]
#[test]
fn resolved_helper_is_used_by_services_without_replacing_automatic_preference() {
    let temp = TempDir::new();
    let file = temp.write("project/file.txt", b"buffer");
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    let spec = workbench.workspace_spec.clone().unwrap();
    assert_eq!(
        spec.target,
        WorkspaceTarget::Ssh {
            host: "test-host".into()
        }
    );
    assert_eq!(
        workbench.session.ssh_target().unwrap().agent,
        "/cache/bed/version-one/bed-headless"
    );
    assert!(!workbench.set_workspace(spec.clone()).unwrap());
    assert!(!workbench.remote_ui.connecting());
    assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
    workbench.open_or_focus(&file).unwrap();
    wait(&mut workbench, |w| w.active_document().is_some());
    let document = workbench.active_document().unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"unsaved "))
        .unwrap();
    workbench.remote_ui.ready = Some(ConnectionResult {
        reconnect: true,
        result: Ok(ConnectedWorkspace {
            spec: spec.clone(),
            target: SshTarget {
                host: "test-host".into(),
                agent: "/cache/bed/version-two/bed-headless".into(),
            },
            client: client(),
        }),
    });
    workbench.poll_remote_workspace().unwrap();
    assert_eq!(
        workbench.session.ssh_target().unwrap().agent,
        "/cache/bed/version-two/bed-headless"
    );
    assert_eq!(workbench.workspace_spec.as_ref(), Some(&spec));
    assert_eq!(
        WorkspaceStore::load(&temp.path("config"))
            .unwrap()
            .recent_workspaces(),
        vec![spec]
    );
    assert_eq!(workbench.active_view(), Some(view));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"unsaved buffer"
    );
    assert!(workbench.session.snapshot(document).unwrap().dirty);
}
#[cfg(unix)]
#[test]
fn unnamed_remote_projects_derive_folder_names_and_keep_saved_labels() {
    let temp = TempDir::new();
    std::fs::create_dir_all(temp.path("first")).unwrap();
    let first = std::fs::canonicalize(temp.path("first")).unwrap();
    let mut workbench = workspace(&temp, first.to_str().unwrap());
    std::fs::create_dir_all(temp.path("folder with spaces")).unwrap();
    let root = std::fs::canonicalize(temp.path("folder with spaces")).unwrap();
    let spec = WorkspaceSpec {
        name: String::new(),
        target: WorkspaceTarget::Ssh {
            host: "test-host".into(),
        },
        root: root.to_str().unwrap().into(),
    };
    let target = workbench.session.ssh_target().unwrap().clone();
    workbench
        .activate_remote_workspace(spec.clone(), target.clone(), client())
        .unwrap();
    assert_eq!(
        workbench.workspace_spec.as_ref().unwrap().name,
        "folder with spaces"
    );
    workbench
        .store
        .as_mut()
        .unwrap()
        .rename_workspace(&spec, "Remembered label")
        .unwrap();
    workbench
        .activate_remote_workspace(spec, target, client())
        .unwrap();
    assert_eq!(
        workbench.workspace_spec.as_ref().unwrap().name,
        "Remembered label"
    );
}
#[test]
fn remote_paths_use_target_separators_and_component_boundaries() {
    assert_eq!(remote_join("/project/dir", "file"), "/project/dir/file");
    assert_eq!(remote_join("/", "file"), "/file");
    assert_eq!(
        remote_rename_target("/project/old", "new").unwrap(),
        "/project/new"
    );
    assert_eq!(
        remote_rebound_path("/project/old", "/project/new", "/project/old/sub/file").unwrap(),
        "/project/new/sub/file"
    );
    assert!(remote_contains("/project/dir", "/project/dir/file"));
    assert!(!remote_contains("/project/dir", "/project/directory/file"));
    assert!(
        remote_rebound_path("/project/dir", "/project/new", "/project/directory/file").is_err()
    );
}
/// Opt in with BED_TEST_SSH_HOST and BED_TEST_SSH_ROOT (absolute or ~/).
/// Creates and removes only its unique fixture child.
#[test]
fn live_ssh_workbench_connection_edit_conflict_and_reconnect() {
    let Ok(host) = std::env::var("BED_TEST_SSH_HOST") else {
        eprintln!(
            "Live SSH Workbench test skipped: set BED_TEST_SSH_HOST and BED_TEST_SSH_ROOT to opt in"
        );
        return;
    };
    let root = std::env::var("BED_TEST_SSH_ROOT")
        .expect("BED_TEST_SSH_ROOT must name an isolated remote test directory");
    assert!(root.starts_with('/') || root == "~" || root.starts_with("~/"));
    let temp = TempDir::new();
    let mut settings = Settings::with_paths(
        temp.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    settings.terminal_visible = false;
    settings.settings["terminal_visible"] = json!(false);
    settings.settings["treesitter"] = json!(false);
    settings.settings["git_changed_lines"] = json!(false);
    let mut workbench = Workbench::with_settings(settings, crate::builtins::modules);
    let requested = WorkspaceSpec {
        name: String::new(),
        target: WorkspaceTarget::Ssh { host },
        root,
    };
    assert!(workbench.set_workspace(requested).unwrap());
    assert!(!workbench.session.is_remote());
    wait_for(&mut workbench, Duration::from_secs(60), |w| {
        w.session.is_remote() && w.session.remote_connected() && !w.remote_ui.connecting()
    });
    let target = workbench.session.ssh_target().unwrap().clone();
    let preference = workbench.workspace_spec.clone().unwrap();
    assert!(target.agent.starts_with('/'));
    assert!(preference.root.starts_with('/'));
    assert_eq!(preference.name, WorkspaceSpec::local(&preference.root).name);
    assert_eq!(
        WorkspaceStore::load(&temp.path("config"))
            .unwrap()
            .recent_workspaces(),
        vec![preference.clone()]
    );
    let root = workbench.project_root.clone();
    let external = RemoteClient::launch_ssh(&target).unwrap();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixture_root = format!(
        "{}/bed-workbench-smoke-{}-{token}",
        root.trim_end_matches('/'),
        std::process::id()
    );
    external
        .call(Request::CreateDirectory {
            root: root.clone(),
            path: fixture_root.clone(),
        })
        .unwrap();
    struct Fixture {
        client: RemoteClient,
        root: String,
        path: String,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = self.client.call(Request::Remove {
                root: self.root.clone(),
                path: self.path.clone(),
                is_directory: true,
            }) {
                eprintln!("Unable to clean live SSH fixture {}: {error}", self.path);
            }
            self.client.disconnect();
        }
    }
    let fixture = Fixture {
        client: external,
        root,
        path: fixture_root.clone(),
    };
    let original = format!("{fixture_root}/file with spaces.txt");
    fixture
        .client
        .call(Request::WriteFile {
            root: fixture_root.clone(),
            path: original.clone(),
            bytes: b"hello".to_vec(),
            baseline: None,
        })
        .unwrap();
    let mut options = workbench.session.options().clone();
    options.autosave = None;
    options.lsp_config = None;
    workbench.session.configure(options).unwrap();
    workbench.open_or_focus(Path::new(&original)).unwrap();
    wait(&mut workbench, |w| w.active_document().is_some());
    let document = workbench.active_document().unwrap();
    let first = workbench.active_view().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workbench.active_view().unwrap();
    assert_ne!(first, second);
    workbench
        .session
        .with_commands(second, |commands| commands.paste(b"over SSH "))
        .unwrap();
    workbench.handle_action(HostAction::Save).unwrap();
    assert!(workbench.session.save_pending(document));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    wait(&mut workbench, |w| !w.session.save_pending(document));
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    let Response::File { bytes, .. } = fixture
        .client
        .call(Request::ReadFile {
            root: fixture_root.clone(),
            path: original.clone(),
        })
        .unwrap()
    else {
        panic!("Expected saved file");
    };
    assert_eq!(bytes, b"over SSH hello");
    workbench
        .rename_path(Path::new(&original), "renamed.txt")
        .unwrap();
    wait(&mut workbench, |w| !w.remote_ui.mutation_pending());
    let renamed = format!("{fixture_root}/renamed.txt");
    assert_eq!(workbench.session.snapshot(document).unwrap().path, renamed);
    assert_eq!(workbench.session.document_for_view(first), Some(document));
    assert_eq!(workbench.session.document_for_view(second), Some(document));
    workbench
        .session
        .with_commands(second, |commands| commands.paste(b"unsaved "))
        .unwrap();
    let retained = workbench.session.snapshot(document).unwrap().bytes;
    let Response::File { baseline, .. } = fixture
        .client
        .call(Request::ReadFile {
            root: fixture_root.clone(),
            path: renamed.clone(),
        })
        .unwrap()
    else {
        panic!("Expected rename baseline");
    };
    fixture
        .client
        .call(Request::WriteFile {
            root: fixture_root.clone(),
            path: renamed.clone(),
            bytes: b"external edit".to_vec(),
            baseline: Some(baseline),
        })
        .unwrap();
    wait(&mut workbench, |w| {
        w.session
            .snapshot(document)
            .unwrap()
            .disk_conflict
            .is_some()
    });
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        retained
    );
    assert!(workbench.handle_action(HostAction::Save).is_err());
    workbench.session.remote_client().unwrap().disconnect();
    workbench.tick().unwrap();
    assert!(!workbench.session.remote_connected());
    assert!(workbench.reconnect_workspace().unwrap());
    wait(&mut workbench, |w| {
        w.session.remote_connected() && !w.remote_ui.connecting()
    });
    assert_eq!(workbench.workspace_spec.as_ref(), Some(&preference));
    assert_eq!(workbench.session.ssh_target(), Some(&target));
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        retained
    );
    assert_eq!(workbench.session.document_for_view(first), Some(document));
    assert_eq!(workbench.session.document_for_view(second), Some(document));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    let Response::File { bytes, .. } = fixture
        .client
        .call(Request::ReadFile {
            root: fixture_root,
            path: renamed,
        })
        .unwrap()
    else {
        panic!("Expected external file after reconnect");
    };
    assert_eq!(bytes, b"external edit");
    workbench
        .session
        .with_commands(second, |commands| commands.undo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        b"over SSH hello"
    );
    workbench
        .session
        .with_commands(second, |commands| commands.redo())
        .unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        retained
    );
    workbench.session.shutdown(ClosePolicy::Discard).unwrap();
    workbench.terminal.shutdown();
    workbench.tabs.clear();
    workbench.remote_ui = RemoteUi::default();
    assert!(workbench.session.document_ids().is_empty());
    drop(fixture);
}

#[cfg(unix)]
#[test]
fn remote_activation_cancels_local_file_jobs_before_save_preflight() {
    let temp = TempDir::new();
    let source = temp
        .write("local/source.txt", b"source")
        .canonicalize()
        .unwrap();
    let destination = temp
        .write("local/destination/source.txt", b"destination")
        .canonicalize()
        .unwrap();
    std::fs::create_dir_all(temp.path("remote")).unwrap();
    let remote_root = temp.path("remote").canonicalize().unwrap();
    let mut workbench = super::super::tests::workspace(&temp);
    workbench.set_project(&temp.path("local")).unwrap();
    workbench.open_or_focus(&source).unwrap();
    workbench
        .handle_tree_action(FileTreeAction::Move {
            paths: vec![source.to_str().unwrap().into()],
            destination: destination.parent().unwrap().to_str().unwrap().into(),
        })
        .unwrap();
    let spec = WorkspaceSpec {
        name: "Connected remote workspace".into(),
        root: remote_root.to_str().unwrap().into(),
        target: WorkspaceTarget::Ssh {
            host: "test-host".into(),
        },
    };
    workbench.remote_ui.ready = Some(ConnectionResult {
        reconnect: false,
        result: Ok(ConnectedWorkspace {
            spec,
            target: SshTarget::new("test-host"),
            client: client(),
        }),
    });
    workbench.poll_remote_workspace().unwrap();
    assert!(
        workbench.session.is_remote(),
        "a paused file job must not cause the validated connection to be dropped"
    );
    assert_eq!(workbench.project_root, remote_root.to_str().unwrap());
    assert_eq!(std::fs::read(source).unwrap(), b"source");
    assert_eq!(std::fs::read(destination).unwrap(), b"destination");
    workbench.cleanup().unwrap();
}

#[cfg(unix)]
#[test]
fn remote_svg_uses_text_storage_and_switches_unsaved_presentations_in_place() {
    let temp = TempDir::new();
    let original = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
    let path = temp.write("project/drawing.svg", original);
    let root = std::fs::canonicalize(temp.path("project")).unwrap();
    let path = std::fs::canonicalize(path).unwrap();
    let mut workbench = workspace(&temp, root.to_str().unwrap());
    workbench.open_or_focus(&path).unwrap();
    wait(&mut workbench, |w| {
        w.tabs
            .iter()
            .any(|tab| tab.panel.viewer.as_deref() == Some(bed_plugin_image::VIEWER_ID))
    });
    let document = workbench.session.document_for_path(&path).unwrap();
    let tab = workbench.tabs.last().unwrap().id;
    let count = workbench.tabs.len();
    assert_eq!(
        workbench.session.document_kind(document).unwrap(),
        DocumentKind::Text
    );
    workbench
        .switch_document_viewer(tab, document, "bed.text")
        .unwrap();
    let revision = workbench.session.document_revision(document).unwrap();
    workbench
        .session
        .apply_edits(
            document,
            revision,
            &[bed_document_session::ByteEdit {
                range: original.len()..original.len(),
                bytes: b"\n<!-- unsaved -->".to_vec(),
            }],
        )
        .unwrap();
    workbench
        .switch_document_viewer(tab, document, bed_plugin_image::VIEWER_ID)
        .unwrap();
    assert_eq!(workbench.tabs.len(), count);
    assert_eq!(workbench.active_document(), Some(document));
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    workbench.session.undo_document(document).unwrap();
    assert_eq!(
        workbench.session.snapshot(document).unwrap().bytes,
        original
    );
}
