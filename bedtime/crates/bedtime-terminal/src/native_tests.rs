use dear_imgui_rs::Context;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
#[test]
fn native_copy_and_paste_use_terminal_service_commands() {
    use crate::{
        bedtime_terminal::BedTerminal,
        terminal_pty::{PtyOptions, TerminalShell},
    };
    use dear_imgui_rs::ClipboardBackend;
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
    use crate::{
        bedtime_terminal::BedTerminal,
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
