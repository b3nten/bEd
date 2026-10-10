//! Saved-workspace attachment recovery and dock selection regressions.
use super::tests::{frame, workspace};
use super::*;
use crate::test_support::TempDir;

fn initialize(workbench: &mut Workbench) -> Context {
    let mut context = Context::create();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

fn document_dock(workbench: &Workbench, path: &Path) -> u32 {
    let document = workbench.session.document_for_path(path).unwrap();
    workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.document() == Some(document))
        .unwrap()
        .dock_id
}

fn document_area(workbench: &Workbench, path: &Path) -> u32 {
    let document = workbench.session.document_for_path(path).unwrap();
    let panel = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.document() == Some(document))
        .unwrap()
        .id;
    workbench.area_for_panel(panel).unwrap()
}

#[test]
fn closing_immediately_after_attachment_restores_the_complete_graft() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for frames_before_close in 0..=1 {
        close_and_reopen_attachment(frames_before_close);
    }
}

fn close_and_reopen_attachment(frames_before_close: usize) {
    let dir = TempDir::new();
    let current = dir.write("current.txt", b"carry this document");
    let saved = dir.write("project/saved.txt", b"saved document");
    let project = dir.path("project");
    {
        let mut workbench = workspace(&dir);
        workbench.open_or_focus(&current).unwrap();
        let mut context = initialize(&mut workbench);
        frame(&mut context, &mut workbench);
        let root = workbench.dock_root;
        let spec = WorkspaceSpec::local(project.canonicalize().unwrap().to_str().unwrap());
        let ini = format!(
            "[Window][bed_tab_10]\nPos=0,0\nSize=240,800\nDockId=0x71232001,0\n\n\
             [Window][bed_tab_11]\nPos=240,0\nSize=960,800\nDockId=0x71232002,0\n\n\
             [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
               DockNode ID=0x71232001 Parent=0x{root:08X} SizeRef=240,800\n\
               DockNode ID=0x71232002 Parent=0x{root:08X} SizeRef=960,800 CentralNode=1\n"
        );
        workbench
            .store
            .as_mut()
            .unwrap()
            .set_layout(
                &spec,
                json!({"version":1,"ini":ini,"panels":[
                    {"id":10,"kind":"explorer"},
                    {"id":11,"kind":"document","path":saved}
                ]}),
            )
            .unwrap();
        workbench.attach_workspace(&project).unwrap();
        let persisted = workbench.store.as_ref().unwrap().layout(&spec).unwrap();
        assert_eq!(persisted["version"], json!(2));
        assert!(persisted["tiling"].is_object());
        assert!(persisted["graft"].is_null());
        assert!(workbench.pending_graft.is_none());
        if frames_before_close != 0 {
            workbench.apply_settings(&mut context).unwrap();
            frame(&mut context, &mut workbench);
            assert!(workbench.pending_graft.is_none());
        }
        // Exercise both an unapplied layout and the first completed graft frame.
        workbench.cleanup().unwrap();
        drop(workbench);
        drop(context);
    }

    let mut reopened = workspace(&dir);
    reopened.set_project(&project).unwrap();
    let mut context = initialize(&mut reopened);
    for _ in 0..4 {
        frame(&mut context, &mut reopened);
    }
    assert_eq!(
        document_area(&reopened, &current),
        2,
        "carried document after closing with {frames_before_close} attached frames"
    );
    assert_eq!(document_area(&reopened, &saved), 2);
    assert_ne!(document_dock(&reopened, &current), 0);
    assert_eq!(
        document_dock(&reopened, &current),
        document_dock(&reopened, &saved)
    );
    assert!(reopened.pending_graft.is_none());
    reopened.cleanup().unwrap();
}

#[test]
fn graft_uses_the_largest_main_leaf_even_when_its_saved_file_is_missing() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let current = dir.write("current.txt", b"carried document");
    let central = dir.write("project/central.txt", b"small central dock");
    let detached = dir.write("project/detached.txt", b"larger detached dock");
    let missing = dir.path("project/missing.txt");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&current).unwrap();
    let mut context = initialize(&mut workbench);
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
        "[Window][bed_tab_10]\nPos=0,0\nSize=960,800\nDockId=0x71233001,0\n\n\
         [Window][bed_tab_11]\nPos=960,0\nSize=240,800\nDockId=0x71233002,0\n\n\
         [Window][bed_tab_12]\nPos=0,0\nSize=2000,2000\nDockId=0x71233003,0\n\n\
         [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
           DockNode ID=0x71233001 Parent=0x{root:08X} SizeRef=960,800\n\
           DockNode ID=0x71233002 Parent=0x{root:08X} SizeRef=240,800 CentralNode=1\n\
         DockNode ID=0x71233003 Pos=0,0 Size=2000,2000\n"
    );
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({"version":1,"ini":ini,"panels":[
                {"id":10,"kind":"document","path":missing},
                {"id":11,"kind":"document","path":central},
                {"id":12,"kind":"document","path":detached}
            ]}),
        )
        .unwrap();
    workbench.attach_workspace(&dir.path("project")).unwrap();
    workbench.apply_settings(&mut context).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert!(workbench.session.document_for_path(&missing).is_none());
    assert_eq!(document_area(&workbench, &current), 1);
    assert_eq!(document_area(&workbench, &central), 2);
    assert_eq!(document_area(&workbench, &detached), 1);
    assert_eq!(
        document_dock(&workbench, &current),
        document_dock(&workbench, &detached)
    );
    assert_ne!(
        document_dock(&workbench, &current),
        document_dock(&workbench, &central)
    );
    assert!(workbench.pending_graft.is_none());
    workbench.cleanup().unwrap();
}

#[test]
fn graft_does_not_mistake_stale_nested_sidebar_sizes_for_the_largest_panel() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let current = dir.write("current.txt", b"carried document");
    let saved = dir.write("project/saved.txt", b"main workspace panel");
    let missing = dir.path("project/missing.txt");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&current).unwrap();
    workbench
        .open_native_panel("terminal", &Value::Null)
        .unwrap();
    let terminal = workbench.terminal.active_session_id().unwrap();
    let terminal_panel = workbench.terminal_panel_id(terminal).unwrap();
    let mut context = initialize(&mut workbench);
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
        "[Window][bed_tab_10]\nPos=0,0\nSize=304,572\nDockId=0x71234002,0\n\n\
         [Window][bed_tab_11]\nPos=0,572\nSize=304,196\nDockId=0x71234003,0\n\n\
         [Window][bed_tab_12]\nPos=306,0\nSize=894,768\nDockId=0x71234004,0\n\n\
         [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,768 Split=X\n\
           DockNode ID=0x71234001 Parent=0x{root:08X} SizeRef=304,768 Split=Y\n\
             DockNode ID=0x71234002 Parent=0x71234001 SizeRef=1396,804 Selected=0x00000001\n\
             DockNode ID=0x71234003 Parent=0x71234001 SizeRef=1396,276\n\
           DockNode ID=0x71234004 Parent=0x{root:08X} SizeRef=894,768 CentralNode=1\n"
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
                    {"id":11,"kind":"document","path":missing},
                    {"id":12,"kind":"document","path":saved}
                ]
            }),
        )
        .unwrap();
    workbench.attach_workspace(&dir.path("project")).unwrap();
    workbench.apply_settings(&mut context).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(document_area(&workbench, &current), 3);
    assert_eq!(document_area(&workbench, &saved), 3);
    assert_eq!(
        workbench
            .tabs
            .iter()
            .find(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
            .unwrap()
            .dock_id,
        workbench.tiling_ui.docks[&1],
    );
    assert_eq!(
        workbench
            .tabs
            .iter()
            .find(|tab| tab.id == terminal_panel)
            .unwrap()
            .dock_id,
        workbench.tiling_ui.docks[&3]
    );
    assert_eq!(workbench.terminal.session_count(), 1);
    assert!(workbench.pending_graft.is_none());
    workbench.cleanup().unwrap();
}

#[test]
fn graft_tabs_live_terminals_into_saved_panels_without_reviving_obsolete_splits() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let current = dir.write("current.txt", b"live viewer");
    let saved = dir.write("project/saved.txt", b"saved editor");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&current).unwrap();
    workbench
        .open_native_panel("terminal", &Value::Null)
        .unwrap();
    let first = workbench.terminal.active_session_id().unwrap();
    let mut context = initialize(&mut workbench);
    frame(&mut context, &mut workbench);
    let first_pid = workbench.terminal.process_id(first).unwrap();
    workbench.split_last_terminal(false).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let terminals = workbench.terminal.session_ids();
    assert_eq!(terminals.len(), 2);
    let root = workbench.dock_root;
    let spec = WorkspaceSpec::local(
        dir.path("project")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap(),
    );
    // The outer split no longer owns any saved panel. ImGui leaves it hidden
    // and gives its space to the explorer/editor branch until it is reused.
    let ini = format!(
        "[Window][bed_tab_10]\nPos=0,0\nSize=240,800\nDockId=0x71235002,0\n\n\
         [Window][bed_tab_11]\nPos=240,0\nSize=960,800\nDockId=0x71235003,0\n\n\
         [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
           DockNode ID=0x71235001 Parent=0x{root:08X} SizeRef=240,800 Split=X\n\
             DockNode ID=0x71235002 Parent=0x71235001 SizeRef=240,800\n\
             DockNode ID=0x71235003 Parent=0x71235001 SizeRef=960,800 CentralNode=1\n\
           DockNode ID=0x71235004 Parent=0x{root:08X} SizeRef=960,800\n"
    );
    workbench
        .store
        .as_mut()
        .unwrap()
        .set_layout(
            &spec,
            json!({"version":1,"ini":ini,"panels":[
                {"id":10,"kind":"explorer"},
                {"id":11,"kind":"document","path":saved}
            ]}),
        )
        .unwrap();
    workbench.attach_workspace(&dir.path("project")).unwrap();
    workbench.apply_settings(&mut context).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(document_area(&workbench, &saved), 2);
    assert_eq!(document_area(&workbench, &current), 2);
    for terminal in terminals {
        let panel = workbench.terminal_panel_id(terminal).unwrap();
        assert_eq!(
            workbench
                .tabs
                .iter()
                .find(|tab| tab.id == panel)
                .unwrap()
                .dock_id,
            workbench.tiling_ui.docks[&2],
        );
    }
    assert_eq!(workbench.terminal.process_id(first), Some(first_pid));
    // The saved editor keeps its width instead of sharing it with an old split.
    let rect = unsafe {
        sys::ImGuiDockNode_Rect(sys::igDockBuilderGetNode(workbench.tiling_ui.docks[&2]))
    };
    assert!(rect.Max.x - rect.Min.x > 900.0);
    workbench.cleanup().unwrap();
}
