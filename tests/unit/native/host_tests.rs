use super::*;

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
fn native_mouse_menu_copy_and_paste_use_terminal_clipboard_shortcuts() {
    use crate::platform::macos_menu::MenuAction;
    use bed_terminal::{
        terminal::{SelectionSnap, Terminal},
        terminal_font::TerminalFonts,
        terminal_view::{TerminalIo, TerminalView},
    };
    use dear_imgui_rs::{Condition, FramePrepareOptions};
    use std::{cell::RefCell, rc::Rc};
    #[derive(Default)]
    struct Pipe(Vec<Vec<u8>>);
    impl TerminalIo for Pipe {
        fn pump(&mut self, _: &mut Terminal) -> io::Result<bool> {
            Ok(false)
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.0.push(bytes.to_vec());
            Ok(())
        }
        fn resize(&mut self, _: usize, _: usize, _: f32, _: f32) -> io::Result<()> {
            Ok(())
        }
    }
    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, text: &str) {
            *self.0.borrow_mut() = text.into();
        }
    }
    fn render(
        context: &mut Context,
        view: &mut TerminalView,
        terminal: &mut Terminal,
        pipe: &mut Pipe,
    ) {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let font = ui.current_font();
        let fonts = TerminalFonts {
            regular: Some(font),
            bold: Some(font),
            italic: Some(font),
            bold_italic: Some(font),
            size: 13.0,
            ..TerminalFonts::default()
        };
        ui.window("Native menu terminal canvas")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .build(|| {
                ui.set_window_focus(None);
                ui.set_keyboard_focus_here();
                view.draw(ui, terminal, &fonts, pipe).unwrap();
            });
        drop(context.render_legacy());
    }
    let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let clipboard = Rc::new(RefCell::new(String::new()));
    context.set_clipboard_backend(Clipboard(Rc::clone(&clipboard)));
    let mut view = TerminalView::default();
    let mut terminal = Terminal::new(80, 24);
    let mut pipe = Pipe::default();
    for _ in 0..2 {
        render(&mut context, &mut view, &mut terminal, &mut pipe);
    }
    terminal.feed(b"abc");
    terminal.select_start(0, 0, SelectionSnap::None);
    terminal.select_extend(2, 0, false, false);
    terminal.select_extend(2, 0, false, true);
    let NativeEditRoute::Shortcut(key, shift) = native_edit_route(MenuAction::Copy, true, false)
    else {
        panic!("copy must route to terminal input")
    };
    queue_native_edit_shortcut(&mut context, key, shift);
    for _ in 0..4 {
        render(&mut context, &mut view, &mut terminal, &mut pipe);
    }
    assert_eq!(&*clipboard.borrow(), "abc");
    assert!(
        pipe.0.is_empty(),
        "mouse Copy must never send Ctrl-C to the shell"
    );
    terminal.feed(b"\x1b[?2004h");
    *clipboard.borrow_mut() = "raw\n\x1b[31m".into();
    let NativeEditRoute::Shortcut(key, shift) = native_edit_route(MenuAction::Paste, true, false)
    else {
        panic!("paste must route to terminal input")
    };
    queue_native_edit_shortcut(&mut context, key, shift);
    for _ in 0..4 {
        render(&mut context, &mut view, &mut terminal, &mut pipe);
    }
    assert_eq!(pipe.0.concat(), b"\x1b[200~raw\n\x1b[31m\x1b[201~");
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
fn native_edit_actions_respect_terminal_and_text_input_ownership() {
    use crate::platform::macos_menu::MenuAction;
    use dear_imgui_rs::Key;
    for action in [
        MenuAction::Undo,
        MenuAction::Redo,
        MenuAction::Cut,
        MenuAction::SelectAll,
    ] {
        assert_eq!(
            native_edit_route(action, true, false),
            NativeEditRoute::Ignore
        );
    }
    assert_eq!(
        native_edit_route(MenuAction::Copy, true, false),
        NativeEditRoute::Shortcut(Key::C, true)
    );
    assert_eq!(
        native_edit_route(MenuAction::Paste, true, false),
        NativeEditRoute::Shortcut(Key::V, true)
    );
    assert_eq!(
        native_edit_route(MenuAction::SelectAll, false, true),
        NativeEditRoute::Shortcut(Key::A, false)
    );
    assert_eq!(
        native_edit_route(MenuAction::SelectAll, false, false),
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
