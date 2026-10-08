use super::super::{WindowCommand, WorkbenchHostMode};
use super::*;
use crate::test_support::TempDir;
use dear_imgui_rs::{Context, FramePrepareOptions};
use std::{
    thread,
    time::{Duration, Instant},
};

fn workspace(dir: &TempDir) -> Workbench {
    let mut workbench = super::super::tests::workspace(dir);
    for key in ["minimap", "ui_animations"] {
        workbench.settings.settings[key] = json!(false);
    }
    workbench
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn idle_debug_session(root: &Path) -> DebugSession {
    // Supply debugger snapshots over a live, idle transport, without requiring
    // LLDB or a native debuggee for host presentation regressions.
    DebugSession::launch_with_args(
        Path::new("/bin/sleep"),
        &["30".into()],
        LaunchConfig {
            program: "/unused/program".into(),
            cwd: root.to_string_lossy().into(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn loader_entry_stop_is_nonfatal_and_continue_can_reveal_project_source() {
    let dir = TempDir::new();
    let initial = dir
        .write("project/README", b"project\n")
        .canonicalize()
        .unwrap();
    let source = dir
        .write(
            "project/src/main.rs",
            b"fn main() {\n    let value = 42;\n}\n",
        )
        .canonicalize()
        .unwrap();
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&initial).unwrap();
    let source_view = workbench.active_panel_id();
    workbench.debugger().launch_profile = Some(DebugProfile {
        stop_on_entry: true,
        ..Default::default()
    });
    let mut session = idle_debug_session(&dir.path("project"));
    session.state = SessionState::Stopped;
    // A stop generation need not start at one: running/continued events also
    // invalidate the backend's inspection state before its first stop.
    session.stop_generation = 7;
    session.selected_thread = Some(1);
    session.frames = vec![bed_debug::StackFrame {
        id: 10,
        name: "_dyld_start".into(),
        source: Some("/usr/lib/dyld`_dyld_start".into()),
        line: 1,
        column: 1,
    }];
    session.select_frame(10).unwrap();
    workbench.debugger().session = Some(session);
    workbench.tick_debugger().unwrap();
    assert!(workbench.debugger().error.is_none());
    assert!(workbench.debugger().active());
    assert!(
        workbench
            .debugger()
            .source_notice
            .as_deref()
            .unwrap()
            .contains("Continue (F5)")
    );
    assert_eq!(workbench.active_panel_id(), source_view);
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        initial.to_string_lossy()
    );

    workbench.debug_action(Action::Continue).unwrap();
    workbench.tick_debugger().unwrap();
    assert!(workbench.debugger().source_notice.is_none());
    {
        let mut debugger = workbench.debugger();
        let session = debugger.session.as_mut().unwrap();
        session.state = SessionState::Stopped;
        session.stop_generation += 1;
        session.frames = vec![bed_debug::StackFrame {
            id: 20,
            name: "ecurl::run".into(),
            source: Some("src/main.rs".into()),
            line: 2,
            column: 5,
        }];
        session.select_frame(20).unwrap();
    }
    workbench.tick_debugger().unwrap();
    assert!(workbench.debugger().error.is_none());
    assert!(workbench.debugger().source_notice.is_none());
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        source.to_string_lossy()
    );
    assert_eq!(workbench.debugger().source_paths.get(&20), Some(&source));
    assert_eq!(
        workbench.debugger().session.as_ref().unwrap().frames[0]
            .source
            .as_deref(),
        Some("src/main.rs")
    );
    assert_eq!(
        workbench
            .debug_source_presentation(workbench.active_document().unwrap())
            .unwrap()
            .unwrap()
            .execution_row,
        Some(1)
    );

    // A later missing frame is still inspectable, and gets source-mapping help
    // instead of the initial loader-stop explanation.
    {
        let mut debugger = workbench.debugger();
        let session = debugger.session.as_mut().unwrap();
        session.frames[0].source = Some("/build/elsewhere/library.rs".into());
        session.frames[0].id = 30;
        session.select_frame(30).unwrap();
    }
    workbench.tick_debugger().unwrap();
    assert!(workbench.debugger().error.is_none());
    let notice = workbench.debugger().source_notice.clone().unwrap();
    assert!(notice.contains("source mapping"));
    assert!(!notice.contains("program entry"));

    // This filename exists in bEd's working directory, but not in the debugged
    // project. It must not open bEd's own library source as a fallback.
    {
        let mut debugger = workbench.debugger();
        let session = debugger.session.as_mut().unwrap();
        session.frames[0].source = Some("src/lib.rs".into());
        session.frames[0].id = 40;
        session.select_frame(40).unwrap();
    }
    workbench.tick_debugger().unwrap();
    assert!(workbench.debugger().error.is_none());
    assert!(workbench.debugger().source_notice.is_some());
    assert_eq!(
        workbench.active_snapshot().unwrap().path,
        source.to_string_lossy()
    );
    workbench.stop_debugger();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn source_mappings_apply_once_when_switching_frames() {
    let dir = TempDir::new();
    let mapped = dir
        .write("project/local/main.rs", b"fn main() {}\n")
        .canonicalize()
        .unwrap();
    dir.write(
        "project/local/local/main.rs",
        b"// wrong mapping applied twice\n",
    );
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    let raw = Path::new(&workbench.project_root)
        .join("main.rs")
        .to_string_lossy()
        .into_owned();
    workbench.debugger().launch_profile = Some(DebugProfile {
        source_map: vec![[workbench.project_root.clone(), "local".into()]],
        ..Default::default()
    });
    let mut session = idle_debug_session(Path::new(&workbench.project_root));
    session.state = SessionState::Stopped;
    session.frames = vec![
        bed_debug::StackFrame {
            id: 1,
            name: "main".into(),
            source: Some(raw.clone()),
            line: 1,
            column: 1,
        },
        bed_debug::StackFrame {
            id: 2,
            name: "runtime".into(),
            source: None,
            line: 0,
            column: 0,
        },
    ];
    session.select_frame(1).unwrap();
    workbench.debugger().session = Some(session);
    for frame in [1, 2, 1] {
        workbench.debug_action(Action::Frame(frame)).unwrap();
        workbench.tick_debugger().unwrap();
        assert_eq!(workbench.debugger().source_paths.get(&1), Some(&mapped));
        assert_eq!(
            workbench.debugger().session.as_ref().unwrap().frames[0]
                .source
                .as_deref(),
            Some(raw.as_str())
        );
        assert_eq!(
            workbench.active_snapshot().unwrap().path,
            mapped.to_string_lossy()
        );
    }
    workbench.stop_debugger();
}

#[test]
fn program_terminal_titles_prefer_the_executable_over_the_default_profile_name() {
    let cargo = DebugProfile {
        cargo: Some(CargoLaunch {
            target: bed_debug::CargoTarget {
                name: "ecurl".into(),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(program_terminal_title(Some(&cargo)), "Program: ecurl");
    let manual = DebugProfile {
        program: "build directory/my program".into(),
        ..Default::default()
    };
    assert_eq!(program_terminal_title(Some(&manual)), "Program: my program");
    assert_eq!(
        program_terminal_title(Some(&DebugProfile::default())),
        "Program: Debug"
    );
    assert_eq!(program_terminal_title(None), "Program Terminal");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn program_terminal_can_be_hidden_reopened_and_stopped_without_duplication() {
    let dir = TempDir::new();
    let mut workbench = workspace(&dir);
    let (terminal_id, pid) = workbench
        .terminal
        .new_command_session(
            PtyOptions {
                shell: Some(TerminalShell::new("/bin/sleep", vec!["30".into()])),
                ..Default::default()
            },
            "Program: sleep",
        )
        .unwrap();
    workbench.debugger().terminal = Some(terminal_id);
    workbench
        .open_plugin_panel(
            bed_module_terminal::PANEL_ID,
            None,
            &json!({"session": terminal_id}),
            None,
        )
        .unwrap();
    let terminal_tab = |workbench: &Workbench| {
        workbench
            .tabs
            .iter()
            .position(|tab| tab.panel.terminal_id() == Some(terminal_id))
    };
    assert!(workbench.debugger().owns_terminal(terminal_id));
    assert!(
        workbench
            .close_tab(terminal_tab(&workbench).unwrap())
            .unwrap()
    );
    assert!(terminal_tab(&workbench).is_none());
    assert_eq!(workbench.terminal.session_ids(), vec![terminal_id]);
    assert!(workbench.terminal.is_started());
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0);

    workbench.debug_action(Action::ShowTerminal).unwrap();
    assert!(terminal_tab(&workbench).is_some());
    assert_eq!(workbench.terminal.active_session_id(), Some(terminal_id));
    workbench.debug_action(Action::ShowTerminal).unwrap();
    assert_eq!(
        workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.terminal_id() == Some(terminal_id))
            .count(),
        1
    );
    assert_eq!(workbench.terminal.session_ids(), vec![terminal_id]);
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, 0);

    assert!(
        workbench
            .close_tab(terminal_tab(&workbench).unwrap())
            .unwrap()
    );
    workbench.debug_action(Action::Stop).unwrap();
    assert!(workbench.terminal.session_ids().is_empty());
    assert!(workbench.debugger().terminal.is_none());
    assert!(!workbench.debugger().owns_terminal(terminal_id));
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[test]
fn profiles_and_watches_persist_after_panel_close_and_breakpoints_stay_ephemeral() {
    let dir = TempDir::new();
    let first_file = dir.write("first/main.cpp", b"int main() { return 0; }\n");
    dir.write("second/main.cpp", b"int main() { return 1; }\n");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("first")).unwrap();
    workbench.open_or_focus(&first_file).unwrap();
    let document = workbench.active_document().unwrap();
    let path = workbench.session.snapshot(document).unwrap().path;
    workbench.debugger().profiles = vec![
        DebugProfile {
            name: "Application".into(),
            program: "out/app".into(),
            ..Default::default()
        },
        DebugProfile {
            name: "Tests".into(),
            program: "out/tests".into(),
            args: vec!["--exact".into()],
            test_filter: "case_name".into(),
            stop_on_entry: false,
            ..Default::default()
        },
    ];
    workbench.debugger().selected = 1;
    workbench.debugger().watches = vec!["local".into(), "object.field".into()];
    workbench.debugger().adapter_path = "/custom/path/lldb-dap".into();
    workbench.toggle_debug_breakpoint(document, 0).unwrap();
    workbench.persist_debugger_settings().unwrap();
    let expected = workbench.debugger().settings();
    workbench.dispatch(WindowCommand::Debug).unwrap();
    workbench.dispatch(WindowCommand::Debug).unwrap();
    assert_eq!(workbench.panel_count("debug"), 1);
    let index = workbench
        .tabs
        .iter()
        .position(|t| t.panel.kind == bed_module_debug::PANEL_ID)
        .unwrap();
    assert!(workbench.close_tab(index).unwrap());
    assert!(!workbench.panel_visible("debug"));
    assert_eq!(workbench.debugger().settings(), expected);
    assert_eq!(workbench.debugger().source.breakpoints[&path].len(), 1);

    let mut restored = workspace(&dir);
    restored.set_project(&dir.path("first")).unwrap();
    assert_eq!(restored.debugger().settings(), expected);
    assert!(restored.debugger().source.breakpoints.is_empty());
    workbench.set_project(&dir.path("second")).unwrap();
    assert!(workbench.debugger().watches.is_empty());
    assert!(workbench.debugger().source.breakpoints.is_empty());
    workbench.set_project(&dir.path("first")).unwrap();
    assert_eq!(workbench.debugger().settings(), expected);
    assert!(workbench.debugger().source.breakpoints.is_empty());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn start_saves_dirty_source_and_failed_build_never_launches_an_old_executable() {
    let dir = TempDir::new();
    let path = dir.write("project/main.cpp", b"int main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.set_project(&dir.path("project")).unwrap();
    workbench.open_or_focus(&path).unwrap();
    let document = workbench.active_document().unwrap();
    workbench
        .session
        .with_commands(workbench.active_view().unwrap(), |commands| {
            commands.type_text(b"// UNSAVED_DEBUG_SOURCE\n");
        })
        .unwrap();
    assert!(workbench.session.snapshot(document).unwrap().dirty);
    let expected = workbench.session.snapshot(document).unwrap().bytes;
    workbench.debugger().profiles[0].program =
        std::env::current_exe().unwrap().to_string_lossy().into();
    // If an existing binary were launched after failure, adapter discovery
    // would replace the build error and create a session or terminal.
    workbench.debugger().adapter_path = dir.path("missing-adapter").to_string_lossy().into();
    workbench.debugger().profiles[0].build_command = "cat main.cpp; exit 17".into();
    workbench.debug_action(Action::Start).unwrap();
    assert!(!workbench.session.snapshot(document).unwrap().dirty);
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    let deadline = Instant::now() + Duration::from_secs(10);
    while workbench.debugger().build.is_some() {
        workbench.tick_debugger().unwrap();
        assert!(Instant::now() < deadline, "Prelaunch build timed out");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        workbench
            .debugger()
            .build_log
            .contains("UNSAVED_DEBUG_SOURCE")
    );
    assert!(
        workbench
            .debugger()
            .error
            .as_deref()
            .unwrap()
            .contains("Build failed")
    );
    assert!(workbench.debugger().session.is_none());
    assert!(workbench.debugger().terminal.is_none());
    assert_eq!(workbench.terminal.session_count(), 0);
    assert!(!workbench.debugger().active());
}

fn initialize(workbench: &mut Workbench) -> Context {
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
fn frame(context: &mut Context, workbench: &mut Workbench) {
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.render(context.frame()).unwrap();
    assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
}

#[test]
fn debug_command_is_singleton_and_draws_in_a_separate_dock() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.rs", b"fn main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    workbench.dispatch(WindowCommand::Debug).unwrap();
    let id = workbench
        .tabs
        .iter()
        .find(|t| t.panel.kind == bed_module_debug::PANEL_ID)
        .unwrap()
        .id;
    workbench.dispatch(WindowCommand::Debug).unwrap();
    assert_eq!(workbench.panel_count("debug"), 1);
    assert_eq!(workbench.active_panel_id(), Some(id));
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let debug_dock = workbench.tabs.iter().find(|t| t.id == id).unwrap().dock_id;
    let source_dock = workbench
        .tabs
        .iter()
        .find(|t| t.panel.editor().is_some())
        .unwrap()
        .dock_id;
    assert_ne!(debug_dock, 0);
    assert_ne!(source_dock, 0);
    assert_ne!(debug_dock, source_dock);
}

#[test]
fn stop_reveal_selects_debug_dock_and_source_keeps_keyboard_focus() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.cpp", b"int main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let source_id = workbench.active_panel_id().unwrap();
    let mut context = initialize(&mut workbench);
    workbench.show_debugger();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let debug = workbench
        .tabs
        .iter()
        .find(|t| t.panel.kind == bed_module_debug::PANEL_ID)
        .unwrap();
    let debug_id = debug.id;
    let debug_dock = debug.dock_id;
    workbench.dispatch(WindowCommand::Diagnostics).unwrap();
    workbench
        .tabs
        .iter_mut()
        .find(|t| t.panel.kind == bed_module_editor::DIAGNOSTICS_PANEL_TYPE)
        .unwrap()
        .dock = Some(debug_dock);
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    let debug_name = std::ffi::CString::new(format!("###bed_tab_{debug_id}")).unwrap();
    unsafe {
        let window = sys::igFindWindowByName(debug_name.as_ptr());
        assert!(!window.is_null());
        assert!(!(*window).DockTabIsVisible());
    }
    // A stopped event reveals Debug, then navigation returns focus to source.
    workbench.show_debugger();
    workbench
        .navigate_file(path.to_str().unwrap(), 0, 0, true)
        .unwrap();
    frame(&mut context, &mut workbench);
    frame(&mut context, &mut workbench);
    unsafe {
        let window = sys::igFindWindowByName(debug_name.as_ptr());
        assert!((*window).DockTabIsVisible());
    }
    assert_eq!(workbench.active_panel_id(), Some(source_id));
}

#[test]
fn runtime_hover_accepts_only_the_selected_frames_source_file() {
    let dir = TempDir::new();
    let selected = dir.write("main.cpp", b"int local;\n");
    let other = dir.write("other.cpp", b"int local;\n");
    assert!(same_debug_source(
        selected.to_str().unwrap(),
        selected.to_str().unwrap()
    ));
    assert!(same_debug_source(
        dir.path("./main.cpp").to_str().unwrap(),
        selected.to_str().unwrap()
    ));
    assert!(!same_debug_source(
        selected.to_str().unwrap(),
        other.to_str().unwrap()
    ));
    assert!(!same_debug_source("", ""));
    assert!(!same_debug_source("src/main.rs", "/unrelated/src/main.rs"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn breakpoint_shortcut_yields_to_find_and_terminal_focus() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let dir = TempDir::new();
    let path = dir.write("project/main.cpp", b"int main() {}\n");
    let mut workbench = workspace(&dir);
    workbench.open_or_focus(&path).unwrap();
    let mut context = initialize(&mut workbench);
    let document = workbench.active_document().unwrap();
    let path = workbench.session.snapshot(document).unwrap().path;
    workbench.dispatch(WindowCommand::Find).unwrap();
    assert_eq!(workbench.active_overlay(), Overlay::Find);
    context.io_mut().add_key_event(Key::F9, true);
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.debug_shortcuts(context.frame()).unwrap();
    drop(context.render_legacy());
    assert!(workbench.debugger().source.breakpoints.is_empty());
    context.io_mut().add_key_event(Key::F9, false);
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.debug_shortcuts(context.frame()).unwrap();
    drop(context.render_legacy());
    // A second source view has no overlay, so terminal focus is tested
    // independently of the first view's still-open Find bar.
    workbench.add_view(document).unwrap();
    assert_eq!(workbench.active_overlay(), Overlay::None);
    workbench.new_terminal();
    assert!(workbench.focused_terminal());
    assert_eq!(workbench.active_overlay(), Overlay::None);
    context.io_mut().add_key_event(Key::F9, true);
    context.prepare_frame(FramePrepareOptions::new([1200.0, 800.0], 1.0 / 60.0));
    workbench.debug_shortcuts(context.frame()).unwrap();
    drop(context.render_legacy());
    assert!(!workbench.debugger().source.breakpoints.contains_key(&path));
    assert!(!workbench.terminal.is_started());
}
