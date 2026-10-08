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
                    (window.InnerRect.Min.y + window.InnerRect.Max.y) * 0.5,
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

fn feed_numbered_lines(workbench: &mut Workbench, session: u64) {
    // Native focus can change while sibling windows render. Select the exact
    // model before accessing the active-session convenience API.
    focus_terminal(workbench, session);
    let terminal = workbench.terminal.active_terminal_mut().unwrap();
    let lines = (0..200)
        .map(|index| {
            // Distinct first characters make a viewport shift observable even
            // when both rows begin with the same decimal hundreds digit.
            let marker = char::from_u32(0x0100 + index).unwrap();
            format!("{marker} line {index:03}\r\n")
        })
        .collect::<String>();
    terminal.feed(lines.as_bytes());
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

    workbench.dispatch(WindowCommand::NewTerminal).unwrap();
    let first_tab = workbench.active_panel_id().unwrap();
    let first_session = workbench.tabs.last().unwrap().panel.terminal_id().unwrap();
    // Stop before rendering so the panel uses deterministic ClosedSessionIo;
    // native wheel handling still runs on its retained terminal screen.
    assert!(workbench.terminal.stop_session_id(first_session));
    for _ in 0..3 {
        tests::frame(&mut context, &mut workbench);
    }
    feed_numbered_lines(&mut workbench, first_session);

    workbench.dispatch(WindowCommand::NewTerminal).unwrap();
    let sibling_session = workbench.tabs.last().unwrap().panel.terminal_id().unwrap();
    assert_ne!(first_session, sibling_session);
    assert!(workbench.terminal.stop_session_id(sibling_session));
    for _ in 0..3 {
        tests::frame(&mut context, &mut workbench);
    }
    feed_numbered_lines(&mut workbench, sibling_session);
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
