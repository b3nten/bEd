use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::FramePrepareOptions;

fn workspace(dir: &TempDir) -> Workbench {
    let mut settings = Settings::with_paths(
        dir.path("config"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .unwrap();
    for key in [
        "terminal_visible",
        "treesitter",
        "git_changed_lines",
        "minimap",
        "ui_animations",
    ] {
        settings.settings[key] = json!(false);
    }
    settings.terminal_visible = false;
    Workbench::with_settings(settings, crate::builtins::modules)
}

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

fn frame(context: &mut Context, workbench: &mut Workbench) -> Vec<HostAction> {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    let actions = workbench.render(context.frame()).unwrap();
    drop(context.render_legacy());
    context.binding().with_bound_context(|| unsafe {
        if (*sys::igGetIO_Nil()).ConfigFlags & sys::ImGuiConfigFlags_ViewportsEnable != 0 {
            sys::igUpdatePlatformWindows();
        }
    });
    actions
}

fn wait_terminal_snapshot(
    workbench: &mut Workbench,
    predicate: impl Fn(&bed_terminal::terminal::TerminalSnapshot) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        workbench.terminal.poll().unwrap();
        if workbench.terminal.active_terminal().is_some_and(&predicate) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal did not publish its expected state"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn compact_area_frames_preserve_host_rounding_and_leave_no_extra_header_space() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    workbench.root_top_inset = 32.0;
    let mut context = initialize(&mut workbench);
    context.style_mut().set_window_rounding(12.0);
    context.style_mut().set_child_rounding(9.0);
    let padding = context.style().window_padding();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    context.binding().with_bound_context(|| unsafe {
        for tab in &workbench.tabs {
            let title = std::ffi::CString::new(workbench.title(tab)).unwrap();
            let window = sys::igFindWindowByName(title.as_ptr()).as_ref().unwrap();
            assert!(window.DockIsActive());
            assert_eq!(
                window.TitleBarHeight, 0.0,
                "custom tabs replace native headers"
            );
            let area = workbench
                .tiling
                .layout
                .area(workbench.tiling.area_for(tab.id).unwrap());
            let native = sys::ImGuiDockNode_Rect(window.DockNode);
            for axis in 0..2 {
                let scale =
                    workbench.tiling_ui.size[axis] / crate::workspace::tiling::EXTENT as f32;
                let min = workbench.tiling_ui.origin[axis] + area.rect.min[axis] as f32 * scale;
                let max = workbench.tiling_ui.origin[axis] + area.rect.max[axis] as f32 * scale;
                let native_min = [native.Min.x, native.Min.y][axis];
                let native_max = [native.Max.x, native.Max.y][axis];
                let tab_height = if axis == 1 {
                    workbench
                        .tiling_ui
                        .bar_rects
                        .get(&area.id)
                        .map_or(0.0, |(min, max)| max[1] - min[1] + 1.0)
                } else {
                    0.0
                };
                // Native dock windows align fractional split edges to pixels.
                let single = workbench.tiling.layout.areas.len() == 1;
                let min_padding = if single {
                    0.0
                } else if area.rect.min[axis] == 0 {
                    7.0
                } else {
                    4.5
                };
                let max_padding = if single {
                    0.0
                } else if area.rect.max[axis] == crate::workspace::tiling::EXTENT {
                    7.0
                } else {
                    4.5
                };
                assert!(
                    (native_min - min - tab_height - min_padding).abs() <= 1.0,
                    "area starts within a small gap: min={min}, native_min={native_min}"
                );
                assert!(
                    (max - native_max - max_padding).abs() <= 1.0,
                    "area ends within a small gap: max={max}, native_max={native_max}"
                );
            }
        }
    });
    assert_eq!(context.style().window_rounding(), 12.0);
    assert_eq!(context.style().child_rounding(), 9.0);
    assert_eq!(context.style().window_padding(), padding);
}

#[test]
fn window_titles_follow_workspace_names_independently_of_panels() {
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    workbench.set_directory(dir.root()).unwrap();
    let title = format!("bEd • {}", dir.root().canonicalize().unwrap().display());
    assert_eq!(workbench.window_title(), title);
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    workbench.open_or_focus(&path).unwrap();
    assert_eq!(workbench.window_title(), title);
    workbench.set_project(&dir.path("project")).unwrap();
    assert_eq!(workbench.window_title(), "bEd • project");
    let spec = workbench.workspace_spec.clone().unwrap();
    workbench.workspace_spec = Some(
        workbench
            .store
            .as_mut()
            .unwrap()
            .rename_workspace(&spec, "Renamed workspace")
            .unwrap(),
    );
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    assert_eq!(workbench.window_title(), "bEd • Renamed workspace");
    workbench.workspace_spec = Some(WorkspaceSpec {
        name: "SSH café".into(),
        root: "/remote/project".into(),
        target: WorkspaceTarget::Ssh {
            host: "host".into(),
        },
    });
    assert_eq!(workbench.window_title(), "bEd • SSH café");
}

#[test]
fn oversized_file_errors_are_visible_dismissible_and_keep_the_active_buffer() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    let large = dir.write("project/huge.rs", b"");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&large)
        .unwrap()
        .set_len(bed_files::files::MAX_FILE_SIZE as u64 + 1)
        .unwrap();
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"// unsaved\n"))
        .unwrap();
    let original = workbench.active_snapshot().unwrap();
    let error = workbench
        .handle_tree_action(FileTreeAction::Open(large.to_string_lossy().into_owned()))
        .unwrap_err();
    assert!(error.to_string().contains("128 MiB"));
    assert!(error.to_string().contains("huge.rs"));
    workbench.error = Some(error.to_string());
    let mut context = initialize(&mut workbench);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    context.binding().with_bound_context(|| unsafe {
        let window = sys::igFindWindowByName(c"Error".as_ptr());
        assert!(!window.is_null());
        let window = &*window;
        assert!(window.Active && !window.Hidden);
        assert!(
            window.Size.x > 300.0,
            "long errors wrap at a readable width"
        );
        assert!(window.Pos.x >= 0.0 && window.Pos.y >= 0.0);
        assert!(window.Pos.x + window.Size.x <= 1200.0);
        assert!(window.Pos.y + window.Size.y <= 800.0);
    });
    assert_eq!(workbench.active_snapshot().unwrap().bytes, original.bytes);
    assert!(workbench.active_snapshot().unwrap().dirty);
    assert_eq!(workbench.panel_count("document"), 1);
    context.io_mut().add_key_event(Key::Escape, true);
    frame(&mut context, &mut workbench);
    context.io_mut().add_key_event(Key::Escape, false);
    frame(&mut context, &mut workbench);
    assert!(workbench.error.is_none());
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    assert_eq!(
        std::fs::metadata(&large).unwrap().len(),
        bed_files::files::MAX_FILE_SIZE as u64 + 1
    );
}

#[test]
fn autosave_settings_debounce_and_apply_to_existing_dirty_documents() {
    let dir = TempDir::new();
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.settings.settings["autosave"] = json!(false);
    workbench.open_or_focus(&path).unwrap();
    let view = workbench.active_view().unwrap();
    assert_eq!(workbench.session.options().autosave, None);
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"// edited\n"))
        .unwrap();
    workbench.session.tick();
    assert_eq!(std::fs::read(&path).unwrap(), b"fn main() {}\n");
    workbench.settings.settings["autosave"] = json!(true);
    workbench.settings.settings["autosave_delay_ms"] = json!(100);
    workbench.sync_services().unwrap();
    assert_eq!(
        workbench.session.options().autosave,
        Some(Duration::from_millis(100))
    );
    workbench.session.tick();
    assert!(workbench.active_snapshot().unwrap().dirty);
    std::thread::sleep(Duration::from_millis(120));
    workbench.session.tick();
    assert_eq!(std::fs::read(&path).unwrap(), b"// edited\nfn main() {}\n");
    assert!(!workbench.active_snapshot().unwrap().dirty);
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"// more\n"))
        .unwrap();
    workbench.settings.settings["autosave"] = json!(false);
    workbench.sync_services().unwrap();
    std::thread::sleep(Duration::from_millis(120));
    workbench.session.tick();
    assert!(workbench.active_snapshot().unwrap().dirty);
    assert_eq!(std::fs::read(&path).unwrap(), b"// edited\nfn main() {}\n");
}

#[test]
fn theme_settings_update_existing_and_future_terminal_sessions_and_preserve_osc_colors() {
    use bed_editing::util::color::contrast_ratio;

    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    let (session, _) = workbench
        .terminal
        .new_command_session(
            bed_terminal::terminal_pty::PtyOptions {
                shell: Some(bed_terminal::terminal_pty::TerminalShell::new(
                    "/bin/sh",
                    vec!["-c".into(), r"printf '\033]4;1;#123456\007'".into()],
                )),
                ..Default::default()
            },
            "Theme fixture",
        )
        .unwrap();
    assert!(workbench.terminal.focus_session(session));
    wait_terminal_snapshot(&mut workbench, |terminal| {
        terminal.palette()[1] == [0x12, 0x34, 0x56]
    });
    let rgba = |rgb: [u8; 3]| {
        [
            rgb[0] as f32 / 255.0,
            rgb[1] as f32 / 255.0,
            rgb[2] as f32 / 255.0,
            1.0,
        ]
    };
    let mut previous_background = None;
    for (name, background, foreground) in [
        ("light", [0.96, 0.94, 0.90, 1.0], [0.10, 0.12, 0.14, 1.0]),
        ("dark", [0.04, 0.05, 0.07, 1.0], [0.90, 0.91, 0.92, 1.0]),
    ] {
        let mut theme = bed_settings::read_json(
            &workbench
                .settings
                .resources_root
                .join("resources/themes/tokyo.json"),
        )
        .unwrap();
        let hex = |rgba: [f32; 4]| {
            format!(
                "#{:02x}{:02x}{:02x}",
                (rgba[0] * 255.0).round() as u8,
                (rgba[1] * 255.0).round() as u8,
                (rgba[2] * 255.0).round() as u8
            )
        };
        theme["name"] = json!(name);
        theme["ui"]["background"] = json!(hex(background));
        theme["ui"]["foreground"] = json!(hex(foreground));
        let selection = format!("themes/{name}.json");
        bed_settings::write_json(&workbench.settings.config_dir.join(&selection), &theme).unwrap();
        workbench.settings.select_theme(&selection).unwrap();
        workbench.sync_services().unwrap();
        let expected_background = [background[0], background[1], background[2]]
            .map(|value| (value * 255.0).round() as u8);
        wait_terminal_snapshot(&mut workbench, |terminal| {
            terminal.palette()[259] == expected_background
        });
        let terminal = workbench.terminal.active_terminal().unwrap();
        let palette = *terminal.palette();
        assert_eq!(palette[1], [0x12, 0x34, 0x56]);
        assert!(contrast_ratio(rgba(palette[258]), rgba(palette[259])) >= 4.5);
        if let Some(previous) = previous_background {
            assert_ne!(palette[259], previous);
        }
        previous_background = Some(palette[259]);
        let revision = terminal.revision();
        workbench.sync_services().unwrap();
        assert_eq!(
            workbench.terminal.active_terminal().unwrap().revision(),
            revision
        );
    }
    workbench.dispatch(WindowCommand::NewTerminal).unwrap();
    let palette = workbench.terminal.active_terminal().unwrap().palette();
    assert_eq!(Some(palette[259]), previous_background);
    assert_ne!(palette[1], [0x12, 0x34, 0x56]);
    for index in (0..16).chain([258]) {
        assert!(contrast_ratio(rgba(palette[index]), rgba(palette[259])) >= 4.5);
    }
}

#[test]
fn explicit_save_restores_cursor_focus_and_keeps_multiple_selections() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let view = workbench.active_view().unwrap();
    workbench
        .session
        .with_commands(view, |commands| {
            commands.type_text(b"// edit\n");
            commands.add_cursor_below();
        })
        .unwrap();
    let before = workbench.session.view_snapshot(view).unwrap().selections;
    // Native menu tracking can temporarily take focus away from the editor.
    workbench
        .session
        .with_view(view, |editor| editor.view_mut().block_input = true)
        .unwrap();
    workbench.handle_action(HostAction::Save).unwrap();
    frame(&mut context, &mut workbench);
    let after = workbench.session.view_snapshot(view).unwrap();
    assert_eq!(after.selections, before);
    assert!(!after.block_input);
    assert!(after.cursor_blink_time < 0.5);
    assert!(!workbench.active_snapshot().unwrap().dirty);
    // Portable Cmd/Ctrl+S routes through the same action and can save again.
    context.io_mut().add_key_event(Key::ModCtrl, true);
    context.io_mut().add_key_event(Key::S, true);
    let actions = frame(&mut context, &mut workbench);
    assert_eq!(actions, vec![HostAction::Save]);
    for action in actions {
        workbench.handle_action(action).unwrap();
    }
    context.io_mut().add_key_event(Key::S, false);
    context.io_mut().add_key_event(Key::ModCtrl, false);
    frame(&mut context, &mut workbench);
    assert!(!workbench.session.view_snapshot(view).unwrap().block_input);
    assert_eq!(
        workbench.session.view_snapshot(view).unwrap().selections,
        before
    );
}

#[test]
fn standalone_panels_are_transient_and_workspaces_support_explicit_resume() {
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    workbench.dispatch(WindowCommand::NewSettings).unwrap();
    workbench.dispatch(WindowCommand::NewTerminal).unwrap();
    workbench.request_close_all().unwrap();
    let mut restored = workspace(&dir);
    restored.restore_last_workspace().unwrap();
    assert!(restored.project_root.is_empty());
    assert_eq!(restored.panel_count("settings"), 0);
    assert!(restored.terminal.session_ids().is_empty());
    assert!(!dir.path("config/workspaces.json").exists());
    let path = dir.write("project/code.rs", b"fn main() {}\n");
    restored.set_project(&dir.path("project")).unwrap();
    restored.open_or_focus(&path).unwrap();
    restored.dispatch(WindowCommand::NewSettings).unwrap();
    restored.dispatch(WindowCommand::Diagnostics).unwrap();
    let project = restored.project_root.clone();
    let focused = restored.focused;
    restored.request_close_all().unwrap();
    let mut reopened = workspace(&dir);
    reopened.restore_last_workspace().unwrap();
    assert_eq!(reopened.project_root, project);
    assert_eq!(reopened.focused, focused);
    assert_eq!(reopened.panel_count("document"), 1);
    assert_eq!(reopened.panel_count("settings"), 1);
    assert_eq!(reopened.panel_count("diagnostics"), 1);
}

#[test]
fn tab_actions_follow_reordered_tabs_and_save_dirty_files_before_closing() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let paths = ["a.rs", "b.rs", "c.rs"].map(|name| dir.write(&format!("project/{name}"), b"code"));
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&paths[0]).unwrap();
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench
        .close_panels_of_type(bed_module_explorer::PANEL_ID)
        .unwrap();
    for kind in [
        bed_module_terminal::PANEL_ID,
        bed_module_editor::DIAGNOSTICS_PANEL_TYPE,
        bed_module_debug::PANEL_ID,
        "bed.mascot.panel",
    ] {
        workbench.close_panels_of_type(kind).unwrap();
    }
    for path in &paths[1..] {
        workbench.open_or_focus(path).unwrap();
    }
    let mut context = initialize(&mut workbench);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let ids = workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
    assert_eq!(workbench.tab_group_order(ids[0]), vec![0, 1, 2]);
    let area = workbench.tiling.area_for(ids[1]).unwrap();
    workbench.tiling.move_tab(ids[1], area, 0);
    frame(&mut context, &mut workbench);
    // Backend selection must preserve the model's visible order: b, a, c.
    assert_eq!(workbench.tab_group_order(ids[0]), vec![1, 0, 2]);
    assert_eq!(
        workbench.tab_close_indices(ids[0], TabCloseAction::Left),
        vec![1]
    );
    assert_eq!(
        workbench.tab_close_indices(ids[0], TabCloseAction::Right),
        vec![2]
    );
    assert_eq!(
        workbench.tab_close_indices(ids[0], TabCloseAction::Others),
        vec![1, 2]
    );
    let view = workbench.tabs[2].panel.view_id().unwrap();
    workbench
        .session
        .with_commands(view, |commands| commands.type_text(b"updated "))
        .unwrap();
    assert!(
        workbench
            .close_tabs(workbench.tab_close_indices(ids[0], TabCloseAction::Right))
            .unwrap()
    );
    assert_eq!(std::fs::read(&paths[2]).unwrap(), b"updated code");
    assert_eq!(
        workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        ids[..2]
    );
    assert!(
        workbench
            .close_tabs(workbench.tab_close_indices(ids[0], TabCloseAction::All))
            .unwrap()
    );
    assert!(workbench.tabs.is_empty());
}

#[test]
fn right_clicking_a_tab_opens_close_actions_and_close_others_keeps_it() {
    check_tab_context_menu(false, false);
}

#[test]
fn right_clicking_native_main_window_tabs_keeps_menu_in_the_main_viewport() {
    check_tab_context_menu(true, false);
}

#[test]
fn outside_workspace_tabs_return_to_main_window_before_opening_their_menu() {
    check_tab_context_menu(true, true);
}

fn check_tab_context_menu(native_viewports: bool, attempt_detachment: bool) {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let first = dir.write("project/a.rs", b"a");
    let second = dir.write("project/b.rs", b"b");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&first).unwrap();
    workbench.set_project(&dir.path("project")).unwrap();
    workbench
        .close_panels_of_type(bed_module_projects::PANEL_ID)
        .unwrap();
    workbench
        .close_panels_of_type(bed_module_explorer::PANEL_ID)
        .unwrap();
    for kind in [
        bed_module_terminal::PANEL_ID,
        bed_module_editor::DIAGNOSTICS_PANEL_TYPE,
        bed_module_debug::PANEL_ID,
        "bed.mascot.panel",
    ] {
        workbench.close_panels_of_type(kind).unwrap();
    }
    let keep = workbench.tabs[0].id;
    workbench.open_or_focus(&second).unwrap();
    let mut context = initialize(&mut workbench);
    let _backend = native_viewports
        .then(|| crate::presentation::popup_style_native_tests::enable_viewports(&mut context));
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    if attempt_detachment {
        context.binding().with_bound_context(|| unsafe {
            let node = sys::igDockBuilderAddNode(0, 0);
            sys::igDockBuilderSetNodePos(node, [1250.0, 100.0].into());
            sys::igDockBuilderSetNodeSize(node, [650.0, 520.0].into());
            for tab in &workbench.tabs {
                let name = CString::new(workbench.title(tab)).unwrap();
                sys::igDockBuilderDockWindow(name.as_ptr(), node);
            }
            sys::igDockBuilderFinish(node);
        });
        for _ in 0..3 {
            frame(&mut context, &mut workbench);
        }
    }
    let (min, max) = workbench.tiling_ui.tab_rects[&keep];
    assert!(max[0] > min[0] && max[1] > min[1]);
    let point = [0, 1].map(|axis| (min[axis] + max[axis]) * 0.5);
    let viewport = context.binding().with_bound_context(|| unsafe {
        let name = CString::new(workbench.title(&workbench.tabs[0])).unwrap();
        let window = sys::igFindWindowByName(name.as_ptr());
        let main = (*sys::igGetMainViewport()).ID;
        assert_eq!((*window).ViewportId, main);
        assert_eq!((*window).TitleBarHeight, 0.0);
        assert!(
            sys::ImGuiDockNode_IsDockSpace((*window).DockNode),
            "workspace panels remain in a tiled area, including outside drops"
        );
        (*window).ViewportId
    });
    context.io_mut().add_mouse_pos_event(point);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.tab_context, Some(keep));
    let menu_point = context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1);
        let popup = &*(*native.OpenPopupStack.Data).Window;
        assert_eq!(
            popup.ViewportId, viewport,
            "menu must use the clicked tab's OS window"
        );
        assert!(!popup.Hidden);
        assert!(popup.DrawList.as_ref().unwrap().VtxBuffer.Size > 0);
        let height = popup.DC.PrevLineSize.y;
        [
            popup.DC.CursorStartPos.x + height,
            popup.DC.CursorStartPos.y + 2.0 * (height + native.Style.ItemSpacing.y) + height * 0.5,
        ]
    });
    context.io_mut().add_mouse_pos_event(menu_point);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    assert_eq!(workbench.tabs.len(), 1);
    assert_eq!(workbench.tabs[0].id, keep);
}

#[test]
fn editor_context_menu_toggles_only_its_tabs_minimap_without_persisting() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/note.txt", b"first\nsecond\nthird\n");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.settings.settings["minimap"] = json!(true);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let first = workbench.active_view().unwrap();
    let layout = workbench
        .tabs
        .iter_mut()
        .find_map(|tab| {
            tab.panel
                .editor()
                .filter(|view| view.id() == first)
                .map(|view| view.presentation().layout)
        })
        .unwrap();
    assert!(layout.minimap_width > 0.0);
    context.io_mut().add_mouse_pos_event([
        layout.text_pos[0] + 60.0,
        layout.text_pos[1] + layout.line_height * 0.5,
    ]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let point = context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1, "editor context menu is open");
        let popup = &*(*native.OpenPopupStack.Data).Window;
        let height = popup.DC.PrevLineSize.y;
        // Eight row intervals and two separators separate Show Minimap from Select All.
        // Context menus use four pixels of vertical item spacing.
        let separators = 2.0 * (native.Style.SeparatorSize.max(1.0) + 4.0);
        [
            popup.DC.CursorStartPos.x + height,
            popup.DC.CursorPosPrevLine.y - 8.0 * (height + 4.0) - separators + height * 0.5,
        ]
    });
    context.io_mut().add_mouse_pos_event(point);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let first = workbench
        .tabs
        .iter_mut()
        .find_map(|tab| tab.panel.editor_mut().filter(|view| view.id() == first))
        .unwrap();
    assert!(!first.minimap_enabled(true));
    assert_eq!(first.presentation().layout.minimap_width, 0.0);
    assert_eq!(workbench.settings.settings["minimap"], json!(true));
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let second = workbench.active_view().unwrap();
    let second = workbench
        .tabs
        .iter_mut()
        .find_map(|tab| tab.panel.editor_mut().filter(|view| view.id() == second))
        .unwrap();
    assert!(second.minimap_enabled(true));
    assert!(second.presentation().layout.minimap_width > 0.0);
    assert_eq!(std::fs::read(&path).unwrap(), b"first\nsecond\nthird\n");
    workbench.request_close_all().unwrap();
    drop(context);
    drop(workbench);
    let mut restored = workspace(&dir);
    restored.restore_last_workspace().unwrap();
    assert_eq!(restored.panel_count("document"), 2);
    for tab in &restored.tabs {
        if let Some(view) = tab.panel.editor() {
            assert!(
                view.minimap_enabled(true),
                "per-tab overrides expire with the tab"
            );
        }
    }
}

#[test]
fn word_wrap_defaults_on_and_respects_missing_and_explicit_settings() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let bytes = format!("{}\nsecond\n", "long words ".repeat(100));
    let path = dir.write("project/note.txt", bytes.as_bytes());
    let mut workbench = workspace(&dir);
    assert_eq!(workbench.settings.settings["word_wrap"], json!(true));
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    let wraps = |workbench: &Workbench| {
        let active = workbench.active_view().unwrap();
        workbench
            .tabs
            .iter()
            .find_map(|tab| tab.panel.editor().filter(|view| view.id() == active))
            .unwrap()
            .visual_row(1)
            > 1
    };
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    assert!(wraps(&workbench), "fresh settings wrap long lines");

    workbench
        .settings
        .settings
        .as_object_mut()
        .unwrap()
        .remove("word_wrap");
    frame(&mut context, &mut workbench);
    assert!(
        wraps(&workbench),
        "older settings without the key wrap by default"
    );

    workbench.settings.settings["word_wrap"] = json!(false);
    frame(&mut context, &mut workbench);
    assert!(
        !wraps(&workbench),
        "an explicit global false disables wrapping"
    );

    workbench.settings.settings["word_wrap"] = json!(true);
    frame(&mut context, &mut workbench);
    assert!(
        wraps(&workbench),
        "existing views follow changes to the global setting"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
}

#[test]
fn editor_context_menu_toggles_only_its_tabs_word_wrap_without_persisting() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let bytes = format!("{}\nsecond\n", "long words ".repeat(100));
    let path = dir.write("project/note.txt", bytes.as_bytes());
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.settings.settings["word_wrap"] = json!(true);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let first = workbench.active_view().unwrap();
    let view = workbench
        .tabs
        .iter()
        .find_map(|tab| tab.panel.editor().filter(|view| view.id() == first))
        .unwrap();
    assert!(view.soft_wrap(true));
    assert!(
        view.visual_row(1) > 1,
        "the persisted setting wraps long lines"
    );
    let layout = view.presentation().layout;
    context.io_mut().add_mouse_pos_event([
        layout.text_pos[0] + 60.0,
        layout.text_pos[1] + layout.line_height * 0.5,
    ]);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Right, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let point = context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        assert_eq!(native.OpenPopupStack.Size, 1, "editor context menu is open");
        let popup = &*(*native.OpenPopupStack.Data).Window;
        let height = popup.DC.PrevLineSize.y;
        // Seven row intervals and two separators separate Word Wrap from Select All.
        let separators = 2.0 * (native.Style.SeparatorSize.max(1.0) + 4.0);
        [
            popup.DC.CursorStartPos.x + height,
            popup.DC.CursorPosPrevLine.y - 7.0 * (height + 4.0) - separators + height * 0.5,
        ]
    });
    context.io_mut().add_mouse_pos_event(point);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    frame(&mut context, &mut workbench);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let view = workbench
        .tabs
        .iter()
        .find_map(|tab| tab.panel.editor().filter(|view| view.id() == first))
        .unwrap();
    assert!(!view.soft_wrap(true));
    assert_eq!(view.visual_row(1), 1);
    assert_eq!(workbench.settings.settings["word_wrap"], json!(true));
    workbench.dispatch(WindowCommand::DuplicateView).unwrap();
    for _ in 0..3 {
        frame(&mut context, &mut workbench);
    }
    let second = workbench.active_view().unwrap();
    let view = workbench
        .tabs
        .iter()
        .find_map(|tab| tab.panel.editor().filter(|view| view.id() == second))
        .unwrap();
    assert!(view.soft_wrap(true));
    assert!(
        view.visual_row(1) > 1,
        "new views use the persisted setting"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
}

#[test]
fn custom_theme_hot_reload_updates_live_terminal_with_unchanged_preferences() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    let mut value = bed_settings::read_json(
        &workbench
            .settings
            .resources_root
            .join("resources/themes/tokyo.json"),
    )
    .unwrap();
    let path = workbench.settings.config_dir.join("themes/live.json");
    bed_settings::write_json(&path, &value).unwrap();
    workbench.settings.select_theme("themes/live.json").unwrap();
    let mut context = initialize(&mut workbench);
    workbench.dispatch(WindowCommand::NewTerminal).unwrap();
    let preferences = workbench.settings.settings.clone();
    let previous = workbench.terminal.active_terminal().unwrap().palette()[259];
    value["ui"]["background"] = json!("#070809");
    bed_settings::write_json(&path, &value).unwrap();
    assert!(workbench.settings.check_settings_file());
    assert_eq!(workbench.settings.settings, preferences);
    assert!(workbench.apply_settings(&mut context).unwrap());
    wait_terminal_snapshot(&mut workbench, |terminal| {
        terminal.palette()[259] == [7, 8, 9]
    });
    let current = workbench.terminal.active_terminal().unwrap().palette()[259];
    assert_eq!(current, [7, 8, 9]);
    assert_ne!(current, previous);
}
