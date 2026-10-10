//! File opening must preserve Files panels and reveal the requested document.
use super::tests::{frame, workspace};
use super::*;
use crate::test_support::TempDir;

fn initialize(workbench: &mut Workbench) -> Context {
    workbench.settings.settings["ui_animations"] = json!(false);
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Fullscreen)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for _ in 0..3 {
        frame(&mut context, workbench);
    }
    context
}

fn document_index(workbench: &Workbench, path: &Path) -> usize {
    let document = workbench.session.document_for_path(path).unwrap();
    workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.document() == Some(document))
        .unwrap()
}

fn assert_document_focused(context: &Context, workbench: &Workbench, path: &Path) {
    let tab = &workbench.tabs[document_index(workbench, path)];
    assert_eq!(workbench.focused, Some(tab.id));
    context.binding().with_bound_context(|| unsafe {
        let title = CString::new(workbench.title(tab)).unwrap();
        let window = sys::igFindWindowByName(title.as_ptr()).as_ref().unwrap();
        assert!(
            window.DockTabIsVisible(),
            "opened file must be the visible tab"
        );
        let focused = (*sys::igGetCurrentContext()).NavWindow.as_ref().unwrap();
        assert_eq!(focused.RootWindow, window.RootWindow);
    });
}

fn collapse_to_one_dock(_context: &Context, workbench: &mut Workbench) {
    workbench.tiling = crate::workspace::tiling_state::TilingState::new(
        crate::workspace::tiling::Layout::default(),
    );
    workbench.dock_built = true;
    let panels = workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
    for id in panels {
        workbench.place_panel_in_area(id, 1);
    }
    workbench.reset_tiling_docks();
}

#[test]
fn opening_files_joins_the_only_available_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"file contents");
    let second = dir.write("project/second.txt", b"second file");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let other_panels = workbench
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.panel.kind != bed_module_explorer::PANEL_ID)
        .map(|(index, _)| index)
        .collect();
    workbench.close_tabs(other_panels).unwrap();
    let mut context = initialize(&mut workbench);
    assert_eq!(workbench.tabs.len(), 1);
    collapse_to_one_dock(&context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.open_or_focus(&path).unwrap();
    workbench.open_or_focus(&second).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    let files = workbench
        .tabs
        .iter()
        .find(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    let document = &workbench.tabs[document_index(&workbench, &path)];
    assert_ne!(document.dock_id, 0);
    assert_eq!(
        document.dock_id, files.dock_id,
        "new documents join the only available area"
    );
    assert_eq!(
        document.dock_id,
        workbench.tabs[document_index(&workbench, &second)].dock_id,
        "opening several files before rendering keeps the same area"
    );
    assert_document_focused(&context, &workbench, &second);
    workbench.cleanup().unwrap();
}

#[test]
fn opening_a_file_uses_the_largest_area_despite_a_smaller_area_having_focus() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"new file");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    workbench.dispatch(WindowCommand::NewDocument).unwrap();
    let mut context = initialize(&mut workbench);
    collapse_to_one_dock(&context, &mut workbench);
    let files = 1;
    let large = workbench.split_area(files, 0, 6000).unwrap();
    let small = workbench.split_area(large, 1, 7500).unwrap();
    let assignments = workbench
        .tabs
        .iter()
        .map(|tab| {
            let area = if tab.panel.kind == bed_module_explorer::PANEL_ID {
                files
            } else if tab.panel.document().is_some() {
                small
            } else {
                large
            };
            (tab.id, area)
        })
        .collect::<Vec<_>>();
    for (id, area) in assignments {
        workbench.place_panel_in_area(id, area);
    }
    let focused = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.document().is_some())
        .unwrap();
    workbench.switch_to_tab(focused);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    // File viewers request opening through the host independently of the
    // application menu and toolbar. The largest area may contain any panel.
    workbench
        .modules
        .requests
        .push(bed_workbench_api::HostRequest::OpenFile {
            path: path.to_string_lossy().into_owned(),
            viewer: Some("bed.hex".into()),
        });
    workbench.process_plugin_requests().unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(
        workbench.area_for_panel(workbench.tabs[document_index(&workbench, &path)].id),
        Some(files)
    );
    assert_document_focused(&context, &workbench, &path);
    workbench.cleanup().unwrap();
}

#[test]
fn file_tree_open_with_menu_uses_the_previous_panel_area() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"menu file");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    let mut context = initialize(&mut workbench);
    let destination = workbench
        .split_area(workbench.largest_area(), 1, 7500)
        .unwrap();
    workbench.focus_empty_area(destination);
    let files = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    workbench.switch_to_tab(files);
    assert_eq!(workbench.previous_focused_area(), Some(destination));
    assert_ne!(destination, workbench.largest_area());
    let layout = workbench.current_tiling().layout.clone();
    workbench
        .handle_tree_action(FileTreeAction::Command {
            command: "bed.open_with:bed.hex".into(),
            context: bed_workbench_api::CommandContext {
                path: Some(path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        })
        .unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(
        workbench.area_for_panel(workbench.tabs[document_index(&workbench, &path)].id),
        Some(destination)
    );
    assert_eq!(workbench.current_tiling().layout, layout);
    assert_document_focused(&context, &workbench, &path);
    workbench.cleanup().unwrap();
}

#[test]
fn reopening_a_file_selects_its_tab_after_files_had_focus() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"existing file");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    collapse_to_one_dock(&context, &mut workbench);
    frame(&mut context, &mut workbench);
    let files = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    workbench.switch_to_tab(files);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert!(!workbench.open_or_focus(&path).unwrap());
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(workbench.panel_count("document"), 1);
    assert_document_focused(&context, &workbench, &path);
    workbench.cleanup().unwrap();
}

#[test]
fn saving_a_file_sharing_a_dock_with_files_restores_the_document_tab() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"original");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    collapse_to_one_dock(&context, &mut workbench);
    frame(&mut context, &mut workbench);
    let document = document_index(&workbench, &path);
    workbench.switch_to_tab(document);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert_document_focused(&context, &workbench, &path);
    let view = workbench.tabs[document].panel.view_id().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"edited "))
        .unwrap();
    workbench.handle_action(HostAction::Save).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"edited original");
    assert_document_focused(&context, &workbench, &path);
    // Saving also restores the document after the tree or a native menu takes focus.
    let files = workbench
        .tabs
        .iter()
        .position(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
        .unwrap();
    workbench.switch_to_tab(files);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    workbench.handle_action(HostAction::Save).unwrap();
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_document_focused(&context, &workbench, &path);
    workbench.cleanup().unwrap();
}

#[test]
fn double_clicking_a_file_keeps_the_document_focused() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/file.txt", b"double click");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench.show_tool(Tool::Explorer);
    // Seed directory contents so input testing does not race the file watcher.
    workbench.file_explorer().file_tree.root_node.children =
        vec![bed_module_explorer::file_tree::FileNode {
            name: "file.txt".into(),
            full_path: path.to_string_lossy().into_owned(),
            ..Default::default()
        }];
    let mut context = initialize(&mut workbench);
    // The tree contains the project row followed by its only file.
    let point = context.binding().with_bound_context(|| unsafe {
        let tab = workbench
            .tabs
            .iter()
            .find(|tab| tab.panel.kind == bed_module_explorer::PANEL_ID)
            .unwrap();
        let title = CString::new(workbench.title(tab)).unwrap();
        let window = &*sys::igFindWindowByName(title.as_ptr());
        let font_size = window.FontRefSize;
        [
            window.DC.CursorStartPos.x + 40.0,
            window.DC.CursorStartPos.y + font_size * 1.47 + 7.0,
        ]
    });
    context.io_mut().add_mouse_pos_event(point);
    frame(&mut context, &mut workbench);
    for _ in 0..2 {
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut workbench);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut workbench);
        // Let the first open consume its focus request before the second press.
        for _ in 0..2 {
            frame(&mut context, &mut workbench);
        }
    }
    assert!(
        workbench.session.document_for_path(&path).is_some(),
        "click must hit the file row"
    );
    for _ in 0..4 {
        frame(&mut context, &mut workbench);
    }
    assert_eq!(workbench.panel_count("document"), 1);
    assert_document_focused(&context, &workbench, &path);
    workbench.cleanup().unwrap();
}
