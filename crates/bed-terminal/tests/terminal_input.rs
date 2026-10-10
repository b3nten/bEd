//! A real child negotiates keyboard modes and receives input through the same
//! ordered worker commands used by the native window. This catches mismatches
//! between negotiation, snapshots, and the otherwise pure input encoder.
#[cfg(unix)]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--input-child") {
        child();
    } else {
        worker_input();
        println!("test negotiated_worker_keyboard_paste_and_ime ... ok");
    }
}

#[cfg(not(unix))]
fn main() {}

#[cfg(unix)]
fn child() {
    use std::io::{Read, Write};
    // SAFETY: stdin is this child's live PTY; libc initializes the termios value
    // before it is read and cfmakeraw only mutates that initialized value.
    unsafe {
        let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(
            libc::tcgetattr(libc::STDIN_FILENO, attributes.as_mut_ptr()),
            0
        );
        let mut attributes = attributes.assume_init();
        libc::cfmakeraw(&mut attributes);
        assert_eq!(
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &attributes),
            0
        );
    }
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(b"\x1b[>1u\x1b[?2004hINPUT_READY\r\n")
        .unwrap();
    stdout.flush().unwrap();
    let expected = "\x1b[13;2u\x1b[99;5u\x1b[118;5u\x1b[200~one\rtwo\r界\x1b[201~日本語";
    let mut received = vec![0; expected.len()];
    std::io::stdin().read_exact(&mut received).unwrap();
    assert_eq!(received, expected.as_bytes());
    stdout.write_all(b"\x1b[<u\x1b[?2004lINPUT_OK\r\n").unwrap();
    stdout.flush().unwrap();
}

#[cfg(unix)]
fn worker_input() {
    use bed_terminal::{
        terminal::Terminal,
        terminal_input::{TerminalKey, TerminalKeyEvent, TerminalModifiers},
        terminal_pty::{
            PtyEvent, PtyOptions, TerminalCommand, TerminalPty, TerminalShell, WindowSize,
        },
    };
    use std::time::{Duration, Instant};

    let options = PtyOptions {
        shell: Some(TerminalShell::new(
            std::env::current_exe().unwrap().to_string_lossy(),
            vec!["--input-child".into()],
        )),
        ..Default::default()
    };
    let size = WindowSize {
        num_cols: 80,
        num_lines: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let mut pty = TerminalPty::spawn_terminal(&options, size, Terminal::new(80, 24)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while pty.snapshot().modes.kitty_keyboard != 1 || !pty.snapshot().modes.bracket_paste {
        assert!(
            Instant::now() < deadline,
            "child did not negotiate input modes"
        );
        for event in pty.poll() {
            if let PtyEvent::Error(error) = event {
                panic!("terminal worker failed: {error}");
            }
        }
        pty.request_snapshot();
        std::thread::sleep(Duration::from_millis(2));
    }
    pty.command(TerminalCommand::Key(TerminalKeyEvent {
        key: TerminalKey::Enter,
        modifiers: TerminalModifiers {
            shift: true,
            ..Default::default()
        },
        ..Default::default()
    }))
    .unwrap();
    for text in ["c", "v"] {
        pty.command(TerminalCommand::Key(TerminalKeyEvent {
            key: TerminalKey::Character(text.into()),
            unshifted_key: Some(text.into()),
            modifiers: TerminalModifiers {
                control: true,
                ..Default::default()
            },
            ..Default::default()
        }))
        .unwrap();
    }
    pty.command(TerminalCommand::Paste("one\r\ntwo\n界".into()))
        .unwrap();
    pty.command(TerminalCommand::Text("日本語".into())).unwrap();
    let mut status = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while status.is_none() {
        assert!(
            Instant::now() < deadline,
            "child did not receive its ordered input"
        );
        for event in pty.poll() {
            match event {
                PtyEvent::Exited(exit) => {
                    status = Some(exit.expect("child supplied native exit status"))
                }
                PtyEvent::Error(error) => panic!("terminal worker failed: {error}"),
                PtyEvent::Output(_) => {
                    panic!("production sessions must not queue raw output on the UI")
                }
            }
        }
        pty.request_snapshot();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(status.unwrap().success());
    let final_snapshot = pty.snapshot();
    let text = final_snapshot
        .lines
        .iter()
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell.text.as_ref())
        .collect::<String>();
    assert!(
        text.contains("INPUT_OK"),
        "the final snapshot must retain the child's final output"
    );
    assert_eq!(final_snapshot.modes.kitty_keyboard, 0);
    assert!(!final_snapshot.modes.bracket_paste);
    pty.shutdown();
}
