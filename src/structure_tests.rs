use super::*;
use crate::files::test_support::TempDir;
use bed_highlight::outline::{OutlineKey, OutlineService, OutlineStatus};
use bed_plugin_structure::{StructurePlugin, presentation::StructureJump};
use dear_imgui_rs::FramePrepareOptions;
use std::cell::Ref;

fn outline(workbench: &Workbench) -> Option<Ref<'_, OutlineService>> {
    workbench
        .plugins
        .instances
        .iter()
        .find_map(|plugin| plugin.as_any().downcast_ref::<StructurePlugin>())
        .and_then(StructurePlugin::outline)
}
fn presentation(
    workbench: &Workbench,
    id: u64,
) -> &bed_plugin_structure::presentation::StructurePanel {
    let tab = workbench.tabs.iter().find(|tab| tab.id == id).unwrap();
    let Panel::Plugin(panel) = &tab.panel else {
        panic!("expected plugin panel")
    };
    &panel
        .instance
        .as_any()
        .downcast_ref::<bed_plugin_structure::StructurePanel>()
        .unwrap()
        .presentation
}
fn outline_key(workbench: &Workbench) -> io::Result<Option<OutlineKey>> {
    let Some(document) = workbench.active_document() else {
        return Ok(None);
    };
    let (generation, revision) = workbench.session.document_revision(document)?;
    workbench.session.with_document(document, |state| {
        Some(OutlineKey {
            document,
            generation,
            revision,
            path: state.path.clone(),
            language_id: state.language_id.clone(),
        })
    })
}
fn jump_to_structure(workbench: &mut Workbench, jump: StructureJump) -> io::Result<bool> {
    workbench.refresh_plugins()?;
    let Some(request) =
        StructurePlugin::navigation_request(jump, &workbench.plugins.frame.context())
    else {
        return Ok(false);
    };
    workbench.plugins.requests.push(request);
    workbench.process_plugin_requests()?;
    Ok(true)
}

fn workspace(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap();
    for key in [
        "terminal_visible",
        "sidebar_visible",
        "treesitter",
        "git_changed_lines",
        "minimap",
        "ui_animations",
    ] {
        settings.settings[key] = json!(false);
    }
    settings.terminal_visible = false;
    Workbench::with_settings(settings)
}
fn wait(workbench: &mut Workbench) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        workbench.tick_plugins().unwrap();
        if outline(workbench).is_some_and(|outline| !outline.updating()) {
            return;
        }
        assert!(Instant::now() < deadline, "structure worker timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    drop(context.render_legacy());
}
fn context(workbench: &mut Workbench) -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    workbench
        .initialize(&mut context, WorkbenchHostMode::Floating)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

#[test]
fn structure_follows_documents_retains_target_on_tool_focus_and_stops_when_closed() {
    let dir = TempDir::new();
    let first = dir.write("first.rs", b"fn first() {}\n");
    let second = dir.write("second.py", b"def second(): pass\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&first).unwrap();
    assert!(outline(&workbench).is_none());
    let document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    let first_panel = workbench.active_panel_id().unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    wait(&mut workbench);
    assert!(!workbench.session.options().highlighting);
    assert_eq!(workbench.active_document(), Some(document));
    assert_eq!(
        outline(&workbench).unwrap().result().unwrap().nodes[0].label,
        "first"
    );
    workbench.dispatch(WindowCommand::Settings).unwrap();
    wait(&mut workbench);
    assert_eq!(workbench.active_document(), Some(document));
    workbench.dispatch(WindowCommand::Structure).unwrap();
    assert_eq!(workbench.active_panel_id(), Some(first_panel));
    assert_eq!(workbench.panel_count("structure"), 2);
    let old_key = outline_key(&workbench).unwrap().unwrap();
    workbench.open_or_focus(&second).unwrap();
    assert!(
        !jump_to_structure(
            &mut workbench,
            StructureJump {
                key: old_key,
                offset: 3
            }
        )
        .unwrap()
    );
    wait(&mut workbench);
    assert_eq!(
        outline(&workbench).unwrap().result().unwrap().nodes[0].label,
        "second"
    );
    let document_index = workbench.active_tab_index().unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    workbench.close_tab(document_index).unwrap();
    assert!(outline(&workbench).unwrap().requested().is_none());
    while let Some(index) = workbench
        .tabs
        .iter()
        .position(|tab| matches!(&tab.panel, Panel::Plugin(panel) if panel.kind == bed_plugin_structure::PANEL_ID))
    {
        workbench.close_tab(index).unwrap();
    }
    assert!(outline(&workbench).is_none());
}

#[test]
fn structure_refreshes_unsaved_edits_replacements_and_renames_and_rejects_stale_jumps() {
    let dir = TempDir::new();
    let path = dir.write("file.rs", b"fn original() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    let document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    wait(&mut workbench);
    let key = outline_key(&workbench).unwrap().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"fn added() {}\n"))
        .unwrap();
    workbench.tick_plugins().unwrap();
    assert!(outline(&workbench).unwrap().updating());
    assert!(!jump_to_structure(&mut workbench, StructureJump { key, offset: 3 }).unwrap());
    wait(&mut workbench);
    assert_eq!(
        outline(&workbench).unwrap().result().unwrap().nodes[0].label,
        "added"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"fn original() {}\n");
    workbench
        .session
        .replace_content(document, b"fn replacement() {}\n")
        .unwrap();
    wait(&mut workbench);
    assert_eq!(
        outline(&workbench).unwrap().result().unwrap().nodes[0].label,
        "replacement"
    );
    workbench
        .session
        .rebind_path(document, &dir.path("renamed.txt"))
        .unwrap();
    wait(&mut workbench);
    assert_eq!(
        outline(&workbench).unwrap().result().unwrap().status,
        OutlineStatus::Unsupported
    );
}

#[test]
fn structure_jump_targets_last_shared_view_and_uses_buffer_byte_positions() {
    let dir = TempDir::new();
    let path = dir.write("file.rs", "// 🙂\r\nfn café() {}\r\n".as_bytes());
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let first = workbench.active_view().unwrap();
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    let second = workbench.active_view().unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    wait(&mut workbench);
    let result = outline(&workbench).unwrap().result().unwrap().clone();
    let jump = StructureJump {
        key: result.key.clone(),
        offset: result.nodes[0].name_range.start,
    };
    assert!(jump_to_structure(&mut workbench, jump).unwrap());
    assert_eq!(workbench.active_view(), Some(second));
    let caret = workbench.session.view_snapshot(second).unwrap();
    assert_eq!((caret.row, caret.column), (1, 3));
    let caret = workbench.session.view_snapshot(first).unwrap();
    assert_eq!((caret.row, caret.column), (0, 0));
    assert_eq!(
        workbench.tabs[workbench.active_index()].id,
        workbench.tabs[workbench.active_tab_index().unwrap()].id
    );
}

#[test]
fn structure_mouse_expanders_keep_target_and_labels_jump_to_source() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write(
        "file.rs",
        b"mod demo {\n    struct Item { value: i32 }\n    fn run() {}\n}\n",
    );
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    let panel = workbench.active_panel_id().unwrap();
    let mut context = context(&mut workbench);
    wait(&mut workbench);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let result = outline(&workbench).unwrap().result().unwrap().clone();
    let root_id = result.nodes[0].id;
    let run_id = result
        .nodes
        .iter()
        .find(|node| node.label == "run")
        .unwrap()
        .id;
    assert_eq!(presentation(&workbench, panel).rows.len(), 3);
    let rows = &presentation(&workbench, panel).rows;
    let root_x = rows.iter().find(|row| row.0 == root_id).unwrap().1[0];
    let child_x = rows.iter().find(|row| row.0 == run_id).unwrap().1[0];
    let font_size = context
        .binding()
        .with_bound_context(|| unsafe { dear_imgui_rs::sys::igGetFontSize() });
    assert!(
        (child_x - root_x - font_size * 0.5).abs() < 0.01,
        "child rows must indent by half a font height"
    );
    assert!(
        (presentation(&workbench, panel).caret_spacing - font_size * 0.14).abs() < 0.01,
        "caret spacing must halve the normal control padding"
    );
    let (_, min, max) = presentation(&workbench, panel)
        .rows
        .iter()
        .find(|row| row.0 == root_id)
        .copied()
        .unwrap();
    let arrow = [min[0] + 5.0, (min[1] + max[1]) * 0.5];
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(arrow);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    frame(&mut context, &mut workbench);
    assert_eq!(presentation(&workbench, panel).rows.len(), 1);
    assert_eq!(workbench.active_panel_id(), Some(panel));
    assert_eq!(workbench.active_view(), Some(view));
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(arrow);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    frame(&mut context, &mut workbench);
    let (_, min, max) = presentation(&workbench, panel)
        .rows
        .iter()
        .find(|row| row.0 == run_id)
        .copied()
        .unwrap();
    let label = [min[0] + 35.0, (min[1] + max[1]) * 0.5];
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(label);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    assert_eq!(workbench.active_view(), Some(view));
    let caret = workbench.session.view_snapshot(view).unwrap();
    assert_eq!((caret.row, caret.column), (2, 7));
    assert_eq!(
        workbench.active_index(),
        workbench.active_tab_index().unwrap()
    );
}

#[test]
fn structure_instances_restore_with_focused_tool_and_last_document() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("file.rs", b"fn run() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    let focused = workbench.active_panel_id();
    let mut context = context(&mut workbench);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    workbench.persist_workspace().unwrap();
    let mut restored = workspace(&dir);
    restored.set_project(dir.root()).unwrap();
    assert_eq!(restored.panel_count("structure"), 2);
    assert_eq!(restored.active_panel_id(), focused);
    wait(&mut restored);
    assert_eq!(
        outline(&restored).unwrap().result().unwrap().nodes[0].label,
        "run"
    );
    workbench.cleanup().unwrap();
}

#[test]
fn structure_panels_keep_independent_expansion_across_offset_edits() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("file.rs", b"mod demo { struct Item { value: i32 } }\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    let first = workbench.active_panel_id().unwrap();
    workbench.dispatch(WindowCommand::NewStructure).unwrap();
    let second = workbench.active_panel_id().unwrap();
    let mut context = context(&mut workbench);
    wait(&mut workbench);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let root_id = outline(&workbench).unwrap().result().unwrap().nodes[0].id;
    let (_, min, max) = presentation(&workbench, second)
        .rows
        .iter()
        .find(|row| row.0 == root_id)
        .copied()
        .unwrap();
    let point = [min[0] + 5.0, (min[1] + max[1]) * 0.5];
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(point);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    frame(&mut context, &mut workbench);
    assert_eq!(presentation(&workbench, second).rows.len(), 1);
    workbench
        .session
        .with_commands(view, |commands| commands.paste(b"// shifted\n"))
        .unwrap();
    wait(&mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(presentation(&workbench, second).rows.len(), 1);
    let index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == first)
        .unwrap();
    workbench.switch_to_tab(index);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(presentation(&workbench, first).rows.len(), 2);
}

fn row_alphas(context: &Context, workbench: &Workbench, panel: u64) -> Vec<u8> {
    let paints = &presentation(workbench, panel).row_paints;
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        for index in 0..native.Windows.Size as usize {
            let window = &**native.Windows.Data.add(index);
            if !window.Active
                || window.Hidden
                || !CStr::from_ptr(window.Name)
                    .to_string_lossy()
                    .contains("structure_tree")
            {
                continue;
            }
            let vertices = &(*window.DrawList).VtxBuffer;
            return paints
                .iter()
                .filter_map(|(_, first, end)| {
                    (*first..(*end).min(vertices.Size as usize))
                        .map(|vertex| ((*vertices.Data.add(vertex)).col >> 24) as u8)
                        .max()
                })
                .collect();
        }
        Vec::new()
    })
}

#[test]
fn structure_rows_stagger_when_async_results_mount_and_toggle_off_settles() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("mount.rs", b"");
    let mut workbench = workspace(&dir);
    workbench.settings.settings["ui_animations"] = json!(true);
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    let panel = workbench.active_panel_id().unwrap();
    let mut context = context(&mut workbench);
    wait(&mut workbench);
    // The panel itself has finished opening before its first outline arrives.
    for _ in 0..15 {
        frame(&mut context, &mut workbench);
    }
    workbench
        .session
        .replace_content(
            document,
            b"fn first() {}\nfn second() {}\nfn third() {}\nfn fourth() {}\n",
        )
        .unwrap();
    wait(&mut workbench);
    let mut staggered = false;
    for _ in 0..6 {
        frame(&mut context, &mut workbench);
        let alphas = row_alphas(&context, &workbench, panel);
        staggered |= alphas.len() >= 2
            && alphas[0] > *alphas.last().unwrap()
            && *alphas.last().unwrap() < 255;
    }
    assert!(
        staggered,
        "individual rows must enter in sequence after results arrive"
    );
    workbench.settings.settings["ui_animations"] = json!(false);
    frame(&mut context, &mut workbench);
    let alphas = row_alphas(&context, &workbench, panel);
    assert_eq!(alphas.len(), 4);
    assert!(alphas.iter().all(|&alpha| alpha == 255), "{alphas:?}");
    workbench.settings.settings["ui_animations"] = json!(true);
    frame(&mut context, &mut workbench);
    assert!(
        row_alphas(&context, &workbench, panel)
            .iter()
            .all(|&alpha| alpha == 255)
    );
}

#[test]
fn structure_branch_motion_moves_siblings_and_closing_rows_ignore_clicks() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write(
        "branch.rs",
        b"mod demo {\nfn first() {}\nfn second() {}\n}\nfn tail() {}\n",
    );
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    workbench.dispatch(WindowCommand::Structure).unwrap();
    let panel = workbench.active_panel_id().unwrap();
    let mut context = context(&mut workbench);
    wait(&mut workbench);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let result = outline(&workbench).unwrap().result().unwrap().clone();
    let root = result
        .nodes
        .iter()
        .find(|node| node.label == "demo")
        .unwrap()
        .id;
    let child = result
        .nodes
        .iter()
        .find(|node| node.label == "first")
        .unwrap()
        .id;
    let tail = result
        .nodes
        .iter()
        .find(|node| node.label == "tail")
        .unwrap()
        .id;
    let row = |workbench: &Workbench, id| {
        presentation(workbench, panel)
            .rows
            .iter()
            .find(|row| row.0 == id)
            .copied()
            .unwrap()
    };
    let (_, root_min, root_max) = row(&workbench, root);
    let (_, child_min, child_max) = row(&workbench, child);
    let before = row(&workbench, tail).1[1];
    let arrow = [root_min[0] + 5.0, (root_min[1] + root_max[1]) * 0.5];
    workbench.settings.settings["ui_animations"] = json!(true);
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(arrow);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    let mut positions = Vec::new();
    positions.push(row(&workbench, tail).1[1]);
    // A fading descendant keeps its visible paint but cannot jump to source.
    let closing = [child_min[0] + 35.0, (child_min[1] + child_max[1]) * 0.5];
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(closing);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
        positions.push(row(&workbench, tail).1[1]);
    }
    assert_eq!(workbench.active_panel_id(), Some(panel));
    for _ in 0..15 {
        frame(&mut context, &mut workbench);
        positions.push(row(&workbench, tail).1[1]);
    }
    let after = *positions.last().unwrap();
    assert!(before - after > 20.0, "{positions:?}");
    assert!(
        positions
            .iter()
            .any(|&y| y < before - 1.0 && y > after + 1.0),
        "{positions:?}"
    );
    assert!(
        positions.windows(2).all(|pair| pair[1] <= pair[0] + 0.1),
        "{positions:?}"
    );
    for down in [true, false] {
        context.io_mut().add_mouse_pos_event(arrow);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, down);
        frame(&mut context, &mut workbench);
    }
    let opening: Vec<_> = (0..15)
        .map(|_| {
            frame(&mut context, &mut workbench);
            row(&workbench, tail).1[1]
        })
        .collect();
    assert!(opening.iter().any(|&y| y > after + 1.0 && y < before - 1.0));
    assert!((opening.last().unwrap() - before).abs() < 1.0);
}
