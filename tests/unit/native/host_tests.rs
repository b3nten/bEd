use super::*;

#[test]
fn workspace_window_launch_passes_only_the_shared_config_and_selected_root() {
    let config = std::env::temp_dir().canonicalize().unwrap();
    let root = config.join("workspace with spaces");
    let mut command = new_instance_command(Path::new("/tmp/bed"), &config, false, None).unwrap();
    command.arg(&root);
    assert_eq!(command.get_program(), "/tmp/bed");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            std::ffi::OsStr::new("--new-window"),
            std::ffi::OsStr::new("--config-dir"),
            config.as_os_str(),
            root.as_os_str(),
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn bundled_workspace_window_launch_uses_launch_services_for_a_new_instance() {
    let config = std::env::temp_dir().canonicalize().unwrap();
    let command = new_instance_command(
        Path::new("/Applications/bEd.app/Contents/MacOS/bed"),
        &config,
        true,
        None,
    )
    .unwrap();
    assert_eq!(command.get_program(), "/usr/bin/open");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            std::ffi::OsStr::new("-n"),
            std::ffi::OsStr::new("-W"),
            std::ffi::OsStr::new("/Applications/bEd.app"),
            std::ffi::OsStr::new("--args"),
            std::ffi::OsStr::new("--new-window"),
            std::ffi::OsStr::new("--config-dir"),
            config.as_os_str(),
        ]
    );
}

#[test]
fn transparent_surfaces_select_the_pinned_metal_mode_and_prefer_premultiplied() {
    use wgpu::{Backend, CompositeAlphaMode as Alpha};
    assert_eq!(
        transparent_alpha_mode(Backend::Metal, &[Alpha::Opaque, Alpha::PostMultiplied]),
        Alpha::PostMultiplied
    );
    assert_eq!(
        transparent_alpha_mode(Backend::Metal, &[Alpha::Opaque, Alpha::PreMultiplied]),
        Alpha::PreMultiplied
    );
    assert_eq!(
        transparent_alpha_mode(Backend::Vulkan, &[Alpha::Opaque, Alpha::Inherit]),
        Alpha::Inherit
    );
    assert_eq!(
        transparent_alpha_mode(Backend::Vulkan, &[Alpha::Opaque]),
        Alpha::Opaque
    );
}

#[test]
fn smaller_checkbox_keeps_its_full_click_target_and_label_layout() {
    use dear_imgui_rs::{Condition, FramePrepareOptions, MouseButton, StyleColor, sys};
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let background = [0.2, 0.4, 0.6, 1.0];
    context
        .style_mut()
        .set_color(StyleColor::FrameBg, background);
    context.style_mut().set_frame_rounding(0.0);
    context.style_mut().set_frame_border_size(0.0);
    let packed = unsafe { sys::igColorConvertFloat4ToU32(background.into()) };
    let mut checked = false;
    let draw = |context: &mut Context, checked: &mut bool| {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut row = ([0.0; 2], [0.0; 2]);
        let mut height = 0.0;
        ui.window("Checkbox size")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                height = ui.frame_height();
                ui.checkbox("Option", checked);
                row = ui.item_rect();
            });
        let square = ui.with_bound_context(|| unsafe {
            let window = &*sys::igFindWindowByName(c"Checkbox size".as_ptr());
            let buffer = &(*window.DrawList).VtxBuffer;
            let vertices = std::slice::from_raw_parts(buffer.Data, buffer.Size as usize);
            let mut min = [f32::INFINITY; 2];
            let mut max = [f32::NEG_INFINITY; 2];
            for vertex in vertices.iter().filter(|v| v.col == packed) {
                min[0] = min[0].min(vertex.pos.x);
                min[1] = min[1].min(vertex.pos.y);
                max[0] = max[0].max(vertex.pos.x);
                max[1] = max[1].max(vertex.pos.y);
            }
            (min, max)
        });
        drop(context.render_legacy());
        (row, square, height)
    };
    draw(&mut context, &mut checked);
    let ((min, max), (square_min, square_max), height) = draw(&mut context, &mut checked);
    for axis in 0..2 {
        assert!((square_max[axis] - square_min[axis] - height * 0.7).abs() < 0.01);
        assert!((square_min[axis] - min[axis] - height * 0.15).abs() < 0.01);
    }
    assert!((max[1] - min[1] - height).abs() < 0.01);
    // Click inside the original square, outside the smaller painted square.
    context
        .io_mut()
        .add_mouse_pos_event([min[0] + 1.0, min[1] + height * 0.5]);
    draw(&mut context, &mut checked);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    draw(&mut context, &mut checked);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    let (row, _, _) = draw(&mut context, &mut checked);
    assert!(checked);
    assert_eq!(row, (min, max));
}

#[cfg(target_os = "macos")]
#[test]
fn native_edit_shortcuts_undo_and_redo_once_without_inserting_characters() {
    use bed_editor_ui::views::view_layout::ViewLayout;
    use dear_imgui_rs::{Condition, FramePrepareOptions, Key};
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut editor = Editor::new();
    // A named in-memory document records upstream undo without touching any
    // source file or scheduling a disk save in this input-only fixture.
    editor
        .api()
        .open_document("test://native-menu", b"baseline");
    editor.commands().type_text(b"!");
    let mut input = EditorInput::default();
    let mut render = |context: &mut Context, editor: &mut Editor| {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Native menu editing")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                ui.set_window_focus(None);
                assert!(
                    input
                        .process(ui, &mut editor.view_context(), &ViewLayout::default())
                        .is_empty()
                );
            });
        drop(context.render_legacy());
    };
    render(&mut context, &mut editor);
    assert_eq!(editor.state.join(), b"!baseline");
    queue_native_edit_shortcut(&mut context, Key::Z, false);
    for _ in 0..4 {
        render(&mut context, &mut editor);
    }
    assert_eq!(editor.state.join(), b"baseline");
    queue_native_edit_shortcut(&mut context, Key::Z, true);
    for _ in 0..4 {
        render(&mut context, &mut editor);
    }
    assert_eq!(editor.state.join(), b"!baseline");
    assert!(!context.io().key_ctrl());
    assert!(!context.io().key_super());
    assert!(!context.io().key_shift());
}

#[cfg(target_os = "macos")]
#[test]
fn native_copy_and_paste_use_terminal_service_commands() {
    use bed_terminal::{
        bed_terminal::BedTerminal,
        terminal_pty::{PtyOptions, TerminalShell},
    };
    use std::{cell::RefCell, rc::Rc};
    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, text: &str) {
            *self.0.borrow_mut() = text.into();
        }
    }
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    let clipboard = Rc::new(RefCell::new(String::new()));
    context.set_clipboard_backend(Clipboard(Rc::clone(&clipboard)));
    let mut terminal = BedTerminal::new_empty();
    // The child verifies the exact bytes delivered by a native Paste action.
    // A Copy action must leave this child waiting for that paste, without SIGINT.
    let script = r#"stty raw -echo; printf '\033[?2004habc'; data=$(dd bs=1 count=20 2>/dev/null | od -An -tx1 | tr -d ' \n'); test "$data" = 1b5b3230307e7261770d5b33316d1b5b3230317e && printf '\r\nPASTE_OK'"#;
    terminal
        .new_command_session(
            PtyOptions {
                shell: Some(TerminalShell::new(
                    "/bin/sh",
                    vec!["-c".into(), script.into()],
                )),
                ..Default::default()
            },
            "Native clipboard fixture",
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !terminal.active_terminal().unwrap().modes.bracket_paste {
        terminal.poll().unwrap();
        assert!(
            Instant::now() < deadline,
            "child must negotiate bracketed paste"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    terminal.native_select_all().unwrap();
    while terminal.native_copy().is_none() {
        terminal.poll().unwrap();
        assert!(
            Instant::now() < deadline,
            "Select All must publish its selection"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let copied = terminal.native_copy().unwrap();
    assert!(copied.starts_with("abc"));
    context.set_clipboard_text(&copied);
    assert_eq!(&*clipboard.borrow(), &copied);
    assert!(terminal.is_started(), "Copy must leave the child alive");
    *clipboard.borrow_mut() = "raw\n\x1b[31m".into();
    terminal
        .native_paste(context.clipboard_text().unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        terminal.poll().unwrap();
        let text = terminal
            .active_terminal()
            .unwrap()
            .lines
            .iter()
            .flat_map(|row| row.cells.iter())
            .map(|cell| cell.text.as_ref())
            .collect::<String>();
        if text.contains("PASTE_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native paste must reach the child exactly once"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    terminal.shutdown();
}

#[cfg(unix)]
#[test]
fn native_window_focus_reports_transitions_once_and_keeps_hidden_terminal_blurred() {
    use bed_terminal::{
        bed_terminal::BedTerminal,
        terminal_font::TerminalFonts,
        terminal_pty::{PtyOptions, TerminalShell},
    };
    use dear_imgui_rs::{Condition, FramePrepareOptions};

    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut terminal = BedTerminal::new_empty();
    // Four focus reports plus a sentinel must arrive in this precise order.
    // Repeated OS notifications and regaining focus after hiding send no bytes.
    let script = r#"stty raw -echo; printf '\033[?1004hFOCUS_READY'; data=$(dd bs=1 count=13 2>/dev/null | od -An -tx1 | tr -d ' \n'); test "$data" = 1b5b491b5b4f1b5b491b5b4f21 && printf '\r\nFOCUS_OK'"#;
    let (session, _) = terminal
        .new_command_session(
            PtyOptions {
                shell: Some(TerminalShell::new(
                    "/bin/sh",
                    vec!["-c".into(), script.into()],
                )),
                ..Default::default()
            },
            "Native focus fixture",
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !terminal.active_terminal().unwrap().modes.focus_reporting {
        terminal.poll().unwrap();
        assert!(
            Instant::now() < deadline,
            "child must enable focus reporting"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..4 {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Native terminal focus")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                ui.set_window_focus(None);
                terminal
                    .render_session(ui, &TerminalFonts::default(), session)
                    .unwrap();
            });
        drop(context.render_legacy());
        if terminal.is_focused() {
            break;
        }
    }
    assert!(terminal.is_focused(), "canvas must acquire keyboard focus");
    terminal.set_window_focused(false).unwrap();
    terminal.set_window_focused(false).unwrap();
    assert!(!terminal.is_focused());
    terminal.set_window_focused(true).unwrap();
    terminal.set_window_focused(true).unwrap();
    assert!(terminal.is_focused());
    terminal.hide().unwrap();
    terminal.set_window_focused(true).unwrap();
    assert!(!terminal.is_focused());
    terminal.write_active(b"!").unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        terminal.poll().unwrap();
        let text = terminal
            .active_terminal()
            .unwrap()
            .lines
            .iter()
            .flat_map(|row| row.cells.iter())
            .map(|cell| cell.text.as_ref())
            .collect::<String>();
        if text.contains("FOCUS_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "focus reports must arrive exactly once"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    terminal.shutdown();
}

#[cfg(target_os = "macos")]
#[test]
fn native_panel_clicks_create_and_configured_shortcuts_reveal() {
    use crate::platform::macos_menu::MenuAction;
    for (action, create, reveal) in [
        (
            MenuAction::Explorer,
            WindowCommand::NewExplorer,
            WindowCommand::Explorer,
        ),
        (
            MenuAction::Terminal,
            WindowCommand::NewTerminal,
            WindowCommand::Terminal,
        ),
        (
            MenuAction::Settings,
            WindowCommand::NewSettings,
            WindowCommand::Settings,
        ),
        (
            MenuAction::FindProject,
            WindowCommand::NewContentSearch,
            WindowCommand::FindProject,
        ),
    ] {
        assert_eq!(native_menu_command(action, false), Some(create));
        assert_eq!(native_menu_command(action, true), Some(reveal));
    }
    for (action, command) in [
        (MenuAction::NewExplorer, WindowCommand::NewExplorer),
        (MenuAction::NewTerminal, WindowCommand::NewTerminal),
        (MenuAction::NewSettings, WindowCommand::NewSettings),
        (MenuAction::Projects, WindowCommand::NewProjects),
        (MenuAction::Diagnostics, WindowCommand::NewDiagnostics),
        (MenuAction::Structure, WindowCommand::NewStructure),
        (MenuAction::NewStructure, WindowCommand::NewStructure),
        (MenuAction::NewReferences, WindowCommand::NewReferences),
        (MenuAction::LspDashboard, WindowCommand::NewLspDashboard),
    ] {
        assert_eq!(native_menu_command(action, false), Some(command));
        assert_eq!(native_menu_command(action, true), Some(command));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn native_edit_actions_respect_text_input_ownership() {
    use crate::platform::macos_menu::MenuAction;
    use dear_imgui_rs::Key;
    assert_eq!(
        native_edit_route(MenuAction::Copy, false),
        NativeEditRoute::Shortcut(Key::C, false)
    );
    assert_eq!(
        native_edit_route(MenuAction::Paste, false),
        NativeEditRoute::Shortcut(Key::V, false)
    );
    assert_eq!(
        native_edit_route(MenuAction::SelectAll, true),
        NativeEditRoute::Shortcut(Key::A, false)
    );
    assert_eq!(
        native_edit_route(MenuAction::SelectAll, false),
        NativeEditRoute::SelectAllDocument
    );
}

#[test]
fn screenshot_exports_bgra_padded_rows_in_display_order() {
    let path = std::env::temp_dir().join(format!("bed-screenshot-test-{}.ppm", std::process::id()));
    let pixels = [
        30, 20, 10, 255, 60, 50, 40, 255, 0, 0, 0, 0, 90, 80, 70, 255, 120, 110, 100, 255, 0, 0, 0,
        0,
    ];
    write_ppm(
        &path,
        &pixels,
        2,
        2,
        12,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    )
    .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(&bytes[..11], b"P6\n2 2\n255\n");
    assert_eq!(
        &bytes[11..],
        &[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
    );
}

#[test]
fn screenshot_rejects_incompatible_extent_before_creating_file() {
    let path =
        std::env::temp_dir().join(format!("bed-screenshot-invalid-{}.ppm", std::process::id()));
    assert!(write_ppm(&path, &[0; 4], 2, 2, 8, wgpu::TextureFormat::Rgba8Unorm).is_err());
    assert!(write_ppm(&path, &[0; 16], 2, 2, 4, wgpu::TextureFormat::Bgra8Unorm).is_err());
    assert!(!path.exists());
}

#[test]
fn workspace_window_launch_preserves_local_and_ssh_identity_as_one_argument() {
    use bed_workbench_api::workspace::{WorkspaceSpec, WorkspaceTarget};
    let config = std::env::temp_dir().canonicalize().unwrap();
    for workspace in [
        WorkspaceSpec::local("/tmp/project with spaces"),
        WorkspaceSpec {
            name: "Named remote".into(),
            root: "/srv/project with spaces".into(),
            target: WorkspaceTarget::Ssh {
                host: "user@alias".into(),
            },
        },
    ] {
        let command =
            new_instance_command(Path::new("/tmp/bed"), &config, false, Some(&workspace)).unwrap();
        let args = command.get_args().collect::<Vec<_>>();
        assert_eq!(args[3], "--workspace");
        assert_eq!(args.len(), 5);
        let serialized: serde_json::Value =
            serde_json::from_str(args[4].to_str().unwrap()).unwrap();
        assert_eq!(WorkspaceSpec::from_value(&serialized), Some(workspace));
    }
}
