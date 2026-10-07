//! Opt-in SSH terminal acceptance, including native PTY resize forwarding.
use bed_remote::SshTarget;
use bed_terminal::terminal_pty::{PtyEvent, PtyOptions, TerminalPty, WindowSize};
use std::{
    thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires BED_TEST_SSH_HOST and BED_TEST_SSH_ROOT"]
fn remote_login_shell_uses_target_cwd_and_receives_resize() {
    let target = SshTarget::new(std::env::var("BED_TEST_SSH_HOST").unwrap());
    let root = std::env::var("BED_TEST_SSH_ROOT").unwrap();
    let options = PtyOptions::default().for_ssh(&target, &root).unwrap();
    let mut pty = TerminalPty::spawn(&options, size(80, 24)).unwrap();
    pty.write(b"printf 'BED_REMOTE_CWD='; pwd; stty size\n")
        .unwrap();
    let output = collect_until(&mut pty, |output| {
        output.contains(&format!("BED_REMOTE_CWD={root}")) && output.contains("24 80")
    });
    assert!(output.contains(&format!("BED_REMOTE_CWD={root}")));
    pty.resize(size(101, 37)).unwrap();
    pty.write(b"stty size\n").unwrap();
    let output = collect_until(&mut pty, |output| output.contains("37 101"));
    assert!(output.contains("37 101"));
    let started = Instant::now();
    pty.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
}

fn size(cols: u16, rows: u16) -> WindowSize {
    WindowSize {
        num_cols: cols,
        num_lines: rows,
        cell_width: 8,
        cell_height: 16,
    }
}
fn collect_until(pty: &mut TerminalPty, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut bytes = Vec::new();
    loop {
        for event in pty.poll() {
            match event {
                PtyEvent::Output(output) => bytes.extend(output),
                other => panic!(
                    "SSH terminal ended: {other:?}\n{}",
                    String::from_utf8_lossy(&bytes)
                ),
            }
        }
        let output = String::from_utf8_lossy(&bytes);
        if done(&output) {
            return output.into_owned();
        }
        assert!(
            Instant::now() < deadline,
            "SSH terminal timed out: {output}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
