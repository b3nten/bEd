//! Exercise terminal wheel input through registered panels and the native host.
use super::*;
use crate::test_support::TempDir;

fn terminal_interior(context: &Context, tab: u64) -> [f32; 2] {
    context.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        let suffix = format!("###bed_tab_{tab}");
        for index in 0..native.Windows.Size as usize {
            let window = &**native.Windows.Data.add(index);
            if window.Active
                && !window.Hidden
                && CStr::from_ptr(window.Name)
                    .to_string_lossy()
                    .ends_with(&suffix)
            {
                return [
                    (window.InnerRect.Min.x + window.InnerRect.Max.x) * 0.5,
                    // A compact area can put its midpoint in the ended-session
                    // toolbar. Aim inside the terminal canvas below it.
                    window.InnerRect.Max.y - 12.0,
                ];
            }
        }
        panic!("terminal tab {tab} must have a visible native window");
    })
}

fn focus_terminal(workbench: &mut Workbench, session: u64) {
    assert!(workbench.terminal.focus_session(session));
    assert_eq!(workbench.terminal.active_session_id(), Some(session));
}

fn numbered_terminal(workbench: &mut Workbench) -> (u64, u64) {
    let lines = (0..200)
        .map(|index| {
            // Distinct first characters make a viewport shift observable even
            // when both rows begin with the same decimal hundreds digit.
            let marker = char::from_u32(0x0100 + index).unwrap();
            format!("{marker} line {index:03}\r\n")
        })
        .collect::<String>();
    let (session, _) = workbench
        .terminal
        .new_command_session(
            bed_terminal::terminal_pty::PtyOptions {
                shell: Some(bed_terminal::terminal_pty::TerminalShell::new(
                    "/bin/sh",
                    vec![
                        "-c".into(),
                        "printf '%s' \"$1\"".into(),
                        "transcript".into(),
                        lines,
                    ],
                )),
                ..Default::default()
            },
            "Transcript fixture",
        )
        .unwrap();
    let tab = workbench
        .open_native_panel("terminal", &json!({"session": session}))
        .unwrap();
    focus_terminal(workbench, session);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        workbench.terminal.poll().unwrap();
        if !workbench.terminal.is_started()
            && workbench.terminal.active_terminal().unwrap().history_size() >= 176
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the transcript child must finish and publish its final screen"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    (tab, session)
}

fn poll_scrolled(workbench: &mut Workbench, session: u64) {
    focus_terminal(workbench, session);
    let deadline = Instant::now() + Duration::from_secs(3);
    while workbench
        .terminal
        .active_terminal()
        .unwrap()
        .display_offset()
        == 0
    {
        workbench.terminal.poll().unwrap();
        assert!(
            Instant::now() < deadline,
            "the worker must publish the scroll command"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn poll_resized(workbench: &mut Workbench, session: u64) {
    focus_terminal(workbench, session);
    let deadline = Instant::now() + Duration::from_secs(3);
    while {
        let terminal = workbench.terminal.active_terminal().unwrap();
        terminal.cols() == 80 && terminal.rows() == 24
    } {
        workbench.terminal.poll().unwrap();
        assert!(
            Instant::now() < deadline,
            "the native panel must publish its resized viewport"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn terminal_wheel_scrolls_the_hovered_module_without_moving_a_sibling_session() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let mut workbench = tests::workspace(&dir);
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

    let (first_tab, first_session) = numbered_terminal(&mut workbench);
    for _ in 0..3 {
        tests::frame(&mut context, &mut workbench);
    }
    poll_resized(&mut workbench, first_session);
    let (_, sibling_session) = numbered_terminal(&mut workbench);
    assert_ne!(first_session, sibling_session);
    for _ in 0..3 {
        tests::frame(&mut context, &mut workbench);
    }
    poll_resized(&mut workbench, sibling_session);
    let sibling_top = workbench
        .terminal
        .active_terminal()
        .unwrap()
        .display_cell(0, 0);
    assert_ne!(sibling_top.character, ' ');
    assert_eq!(
        workbench
            .terminal
            .active_terminal()
            .unwrap()
            .display_offset(),
        0
    );

    let first_index = workbench
        .tabs
        .iter()
        .position(|tab| tab.id == first_tab)
        .unwrap();
    assert!(workbench.switch_to_tab(first_index));
    for _ in 0..3 {
        tests::frame(&mut context, &mut workbench);
    }
    focus_terminal(&mut workbench, first_session);
    let before = workbench
        .terminal
        .active_terminal()
        .unwrap()
        .display_cell(0, 0);
    assert_eq!(
        workbench
            .terminal
            .active_terminal()
            .unwrap()
            .display_offset(),
        0
    );
    let mouse = terminal_interior(&context, first_tab);
    context.io_mut().add_mouse_pos_event(mouse);
    tests::frame(&mut context, &mut workbench);
    context.io_mut().add_mouse_wheel_event([0.0, 1.0]);
    tests::frame(&mut context, &mut workbench);
    poll_scrolled(&mut workbench, first_session);

    focus_terminal(&mut workbench, first_session);
    let terminal = workbench.terminal.active_terminal().unwrap();
    assert!(
        terminal.display_offset() > 0,
        "wheel input must reach the terminal module"
    );
    assert_ne!(
        terminal.display_cell(0, 0),
        before,
        "scrolling must reveal retained output"
    );

    focus_terminal(&mut workbench, sibling_session);
    let sibling = workbench.terminal.active_terminal().unwrap();
    assert_eq!(sibling.display_offset(), 0);
    assert_eq!(sibling.display_cell(0, 0), sibling_top);
    workbench.cleanup().unwrap();
}
