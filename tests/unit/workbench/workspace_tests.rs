//! Workspace lifecycle and saved-layout regressions.
use super::tests::{frame, workspace};
use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;

#[test]
fn workspace_attach_restores_saved_geometry_and_other_roots_open_separately() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let file = dir.write("project/file.txt", b"workspace document");
    let mut context = Context::create();
    let mut workbench = workspace(&dir);
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let root = workbench.dock_root;
    let spec = WorkspaceSpec::local(
        dir.path("project")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap(),
    );
    let ini = format!(
        "[Window][bed_tab_1]\nPos=0,0\nSize=1200,800\nDockId=0x{root:08X},0\n\n\
         [Window][bed_tab_10]\nPos=0,0\nSize=240,800\nDockId=0x00000001,0\n\n\
         [Window][bed_tab_11]\nPos=240,0\nSize=960,800\nDockId=0x00000002,0\n\n\
         [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
           DockNode ID=0x00000001 Parent=0x{root:08X} SizeRef=240,800\n\
           DockNode ID=0x00000002 Parent=0x{root:08X} SizeRef=960,800 CentralNode=1\n"
    );
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({
                "version":1, "ini":ini, "panels":[
                    {"id":10,"kind":"explorer"},
                    {"id":11,"kind":"document","path":file}
                ]
            }),
        )
        .unwrap();
    workbench.set_workspace(spec).unwrap();
    workbench.apply_settings(&mut context).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"workspace document"
    );
    workbench.persist_workspace().unwrap();
    let ini = workbench
        .store
        .as_ref()
        .unwrap()
        .layout(workbench.workspace_spec.as_ref().unwrap())
        .unwrap()["ini"]
        .as_str()
        .unwrap();
    assert!(
        !ini.contains("[Window][bed_tab_1]"),
        "obsolete picker must not be saved in the project"
    );
    dir.write("other/other.txt", b"other workspace");
    for _ in 0..3 {
        workbench.set_project(&dir.path("other")).unwrap();
        workbench.apply_settings(&mut context).unwrap();
        frame(&mut context, &mut workbench);
        assert_eq!(
            workbench
                .take_workspace_windows()
                .into_iter()
                .map(|spec| PathBuf::from(spec.root))
                .collect::<Vec<_>>(),
            vec![dir.path("other").canonicalize().unwrap()]
        );
        assert_eq!(
            workbench.active_snapshot().unwrap().bytes,
            b"workspace document"
        );
        assert!(!workbench.set_project(&dir.path("project")).unwrap());
        assert!(workbench.take_workspace_windows().is_empty());
        workbench.apply_settings(&mut context).unwrap();
        frame(&mut context, &mut workbench);
        frame(&mut context, &mut workbench);
        assert_eq!(
            workbench.active_snapshot().unwrap().bytes,
            b"workspace document"
        );
    }
    workbench.cleanup().unwrap();
}
#[test]
fn another_workspace_requested_during_render_keeps_the_current_window() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let first = dir.write("first/a.txt", b"first");
    dir.write("second/b.txt", b"second");
    let mut context = Context::create();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("first")).unwrap();
    workbench.open_or_focus(&first).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    frame(&mut context, &mut workbench);
    let old_workspace = workbench.session.workspace_id();
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    assert!(workbench.set_project(&dir.path("second")).unwrap());
    assert_eq!(workbench.session.workspace_id(), old_workspace);
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"first");
    drop(context.render_legacy());
    workbench.tick().unwrap();
    assert_eq!(workbench.session.workspace_id(), old_workspace);
    assert!(workbench.project_root.ends_with("/first"));
    assert_eq!(workbench.active_snapshot().unwrap().bytes, b"first");
    assert_eq!(
        workbench
            .take_workspace_windows()
            .into_iter()
            .map(|spec| PathBuf::from(spec.root))
            .collect::<Vec<_>>(),
        vec![dir.path("second").canonicalize().unwrap()]
    );
    workbench.apply_settings(&mut context).unwrap();
    frame(&mut context, &mut workbench);
    workbench.cleanup().unwrap();
}
#[test]
fn opening_files_from_the_picker_during_render_keeps_the_window_rootless() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let first = dir.write("project/a.txt", b"first file");
    let second = dir.write("project/b.txt", b"second file");
    let mut context = Context::create();
    let mut workbench = workspace(&dir);
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    workbench.open_or_focus(&first).unwrap();
    workbench
        .open_file_with_viewer(&second, Some("bed.hex"), false)
        .unwrap();
    assert_eq!(workbench.session.document_ids().len(), 2);
    assert!(workbench.workspace_spec.is_none());
    assert!(workbench.pending_workspace.is_none());
    drop(context.render_legacy());
    workbench.tick().unwrap();
    assert_eq!(workbench.session.document_ids().len(), 2);
    assert_eq!(workbench.panel_count("document"), 2);
    assert_eq!(workbench.panel_count("hex"), 1);
    assert!(workbench.workspace_spec.is_none());
    assert!(workbench.pending_workspace_files.is_empty());
    workbench.apply_settings(&mut context).unwrap();
    frame(&mut context, &mut workbench);
    workbench.cleanup().unwrap();
}

#[test]
fn invalid_saved_layouts_and_panel_ids_preserve_documents_and_allow_new_panels() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let file = dir.write("project/file.txt", b"keep this document");
    let spec = WorkspaceSpec::local(
        dir.path("project")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap(),
    );
    let mut context = Context::create();
    let mut workbench = workspace(&dir);
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({
                "version":1, "ini":"[Docking][Data]\nDockNode ID=0x1 Parent=0x1 SizeRef=800,600\n",
                "panels":[{"id":10,"kind":"document","path":file}]
            }),
        )
        .unwrap();
    workbench.set_workspace(spec.clone()).unwrap();
    assert!(workbench.pending_ini.is_none());
    assert!(workbench.error.as_ref().unwrap().contains("default layout"));
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    frame(&mut context, &mut workbench);
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"keep this document"
    );
    workbench.cleanup().unwrap();
    drop(workbench);
    drop(context);
    let mut workbench = workspace(&dir);
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({
                "version":1, "panels":[
                    {"id":u64::MAX,"kind":"document","path":file},
                    {"id":u64::MAX,"kind":"document","path":file},
                    null
                ]
            }),
        )
        .unwrap();
    workbench.set_workspace(spec).unwrap();
    assert_eq!(workbench.session.document_ids().len(), 1);
    assert_eq!(workbench.panel_count("document"), 2);
    let existing_ids = workbench
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect::<HashSet<_>>();
    assert_eq!(existing_ids.len(), workbench.tab_count());
    assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    let added = workbench.active_panel_id().unwrap();
    assert!(!existing_ids.contains(&added));
    assert_eq!(
        workbench.tabs.iter().filter(|tab| tab.id == added).count(),
        1
    );
    workbench.cleanup().unwrap();
}

#[test]
fn explicit_workspace_windows_preserve_saved_names_and_the_current_session() {
    let dir = TempDir::new();
    let file = dir.write("project/a.txt", b"keep this buffer");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&file).unwrap();
    let session = workbench.session.workspace_id();
    let active = workbench.active_view();
    let spec = WorkspaceSpec::local(dir.path("project").to_str().unwrap());
    let spec = workbench
        .store
        .as_mut()
        .unwrap()
        .record_workspace(spec)
        .unwrap();
    let named = workbench
        .store
        .as_mut()
        .unwrap()
        .rename_workspace(&spec, "My project")
        .unwrap();
    let remote = WorkspaceSpec {
        name: "Remote project".into(),
        root: "/srv/project with spaces".into(),
        target: WorkspaceTarget::Ssh {
            host: "user@project-host".into(),
        },
    };
    workbench.request_workspace_window(spec).unwrap();
    workbench.request_workspace_window(remote.clone()).unwrap();
    assert_eq!(
        workbench.take_workspace_windows(),
        vec![named.clone(), remote]
    );
    assert!(workbench.workspace_spec.is_none());
    assert_eq!(workbench.session.workspace_id(), session);
    assert_eq!(workbench.active_view(), active);
    assert_eq!(
        workbench.active_snapshot().unwrap().bytes,
        b"keep this buffer"
    );
    // The explicit menu action opens another window even for the active root.
    workbench.set_workspace(named.clone()).unwrap();
    let session = workbench.session.workspace_id();
    workbench.request_workspace_window(named.clone()).unwrap();
    assert_eq!(workbench.take_workspace_windows(), vec![named]);
    assert_eq!(workbench.session.workspace_id(), session);
    assert!(
        workbench
            .request_workspace_window(WorkspaceSpec::local("/missing/bed-workspace"))
            .is_err()
    );
    assert!(workbench.take_workspace_windows().is_empty());
    workbench.cleanup().unwrap();
}

#[test]
fn opening_a_workspace_dismisses_startup_and_does_not_save_it() {
    for saved in [false, true] {
        let dir = TempDir::new();
        dir.write("project/file.txt", b"project");
        let mut workbench = workspace(&dir);
        let spec = WorkspaceSpec::local(
            dir.path("project")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap(),
        );
        if saved {
            // Clean up launcher panels accidentally persisted before this fix.
            workbench
                .store
                .as_mut()
                .unwrap()
                .set_layout(
                    &spec,
                    json!({
                        "version":1, "panels":[{"id":20,"kind":"projects"}],
                    }),
                )
                .unwrap();
        }
        workbench.dispatch(WindowCommand::NewProjects).unwrap();
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 2);
        assert!(workbench.set_project(&dir.path("project")).unwrap());
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
        assert_eq!(workbench.panel_count("explorer"), 1);
        let layout = workbench
            .store
            .as_ref()
            .unwrap()
            .layout(workbench.workspace_spec.as_ref().unwrap())
            .unwrap();
        assert!(layout["panels"].as_array().unwrap().iter().all(|panel| {
            panel["kind"] != "projects" && panel["panel_type"] != bed_module_projects::PANEL_ID
        }));
        // Opening Startup manually is temporary even inside a workspace.
        workbench.dispatch(WindowCommand::NewProjects).unwrap();
        workbench.persist_workspace().unwrap();
        let layout = workbench
            .store
            .as_ref()
            .unwrap()
            .layout(workbench.workspace_spec.as_ref().unwrap())
            .unwrap();
        assert!(
            layout["panels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|panel| panel["panel_type"] != bed_module_projects::PANEL_ID)
        );
        workbench.cleanup().unwrap();
    }
}
