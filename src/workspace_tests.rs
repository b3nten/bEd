//! Workspace lifecycle and saved-layout regressions.
use super::tests::{frame, workspace};
use super::*;
use crate::files::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;

#[test]
fn workspace_switch_does_not_restore_obsolete_windows_into_split_nodes() {
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
        assert!(workbench.session.document_ids().is_empty());
        workbench.set_project(&dir.path("project")).unwrap();
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
fn workspace_switch_requested_during_render_waits_until_the_frame_finishes() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let first = dir.write("first/a.txt", b"first");
    dir.write("second/b.txt", b"second");
    let mut context = Context::create();
    let mut workbench = workspace(&dir);
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
    assert_ne!(workbench.session.workspace_id(), old_workspace);
    assert!(workbench.project_root.ends_with("/second"));
    workbench.apply_settings(&mut context).unwrap();
    frame(&mut context, &mut workbench);
    workbench.cleanup().unwrap();
}
#[test]
fn opening_files_from_the_picker_during_render_keeps_them_after_the_workspace_switch() {
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
    assert!(workbench.session.document_ids().is_empty());
    drop(context.render_legacy());
    workbench.tick().unwrap();
    assert_eq!(workbench.session.document_ids().len(), 2);
    assert_eq!(workbench.panel_count("document"), 2);
    assert_eq!(workbench.panel_count("hex"), 1);
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
    assert_eq!(
        workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        [1, 2]
    );
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    assert_eq!(workbench.active_panel_id(), Some(3));
    workbench.cleanup().unwrap();
}
