//! bEd shell commands retain the originating process through application transitions.
use crate::application::test_support::TempDir;
use crate::application::testing::{
    AppAction, capture_app_request, open_panel, panel_presentation, process_app_requests,
    submit_app_request,
};
use crate::application::{
    AppOptions, AppRuntimePlugin, PanelPresentation, RemoteHelperDirectory, WorkspacePersistence,
    WorkspaceStore,
};
use bedtime_api::workbench::PanelTarget;
use bedtime_documents::DocumentSession;
use bedtime_plugin_terminal::{TerminalPanelState, shell_socket_path};
use bedtime_terminal::{
    bedtime_terminal::BedTerminal, terminal_font::TerminalFonts, terminal_pty::TerminalShell,
};
use bedtime_workbench::WorkbenchLayout;
use bevy::prelude::*;
use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};

fn app(directory: &TempDir, startup_panel: Option<&str>) -> App {
    let mut app = App::new();
    let settings = crate::config::AppConfig::with_paths(
        directory.path("config"),
        env!("CARGO_MANIFEST_DIR").into(),
    )
    .unwrap();
    let paths =
        bedtime_api::launch::ApplicationPaths::new("bed", Some(settings.config_dir.clone()), None)
            .unwrap();
    app.insert_resource(paths.clone())
        .insert_non_send(bedtime_ui::icons::Icons::default())
        .insert_non_send(WorkspacePersistence::new(
            WorkspaceStore::load(&paths).unwrap(),
        ))
        .insert_non_send(settings)
        .insert_resource(RemoteHelperDirectory(std::path::PathBuf::new()));
    app.add_plugins((
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(directory.root().to_owned()),
                startup_panel: startup_panel.map(ToOwned::to_owned),
                ..Default::default()
            },
        },
        bedtime_plugin_editor::EditorPlugin,
        bedtime_plugin_terminal::TerminalPlugin,
    ));
    app.world_mut()
        .non_send_mut::<BedTerminal>()
        .configure_shell(TerminalShell::new("/bin/sh", vec!["-i".into()]));
    app.world_mut()
        .non_send_mut::<BedTerminal>()
        .configure_shell_environment(HashMap::from([
            ("BED_BRIDGE_TEST".into(), "profile retained".into()),
            ("ENV".into(), "/dev/null".into()),
            ("PS1".into(), String::new()),
        ]));
    app.update();
    app
}

fn terminal(app: &mut App) -> (Entity, Entity, u64) {
    let panel = open_panel(
        app.world_mut(),
        bedtime_plugin_terminal::PANEL_ID,
        bedtime_api::documents::PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let frontend = panel_presentation(app.world(), panel).unwrap();
    let session = app
        .world()
        .get::<TerminalPanelState>(frontend)
        .unwrap()
        .session_id;
    (panel, frontend, session)
}

fn start_shell(app: &mut App, session: u64) -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<String>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let fonts = TerminalFonts::default();
    let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
    terminal.prepare_frame(&mut context, &fonts, 1.0).unwrap();
    context.prepare_frame(FramePrepareOptions::new([640.0, 360.0], 1.0 / 60.0));
    let ui = context.frame();
    ui.window("Shell bridge fixture")
        .position([0.0; 2], Condition::Always)
        .size([640.0, 360.0], Condition::Always)
        .build(|| {
            terminal.render_session(ui, &fonts, session).unwrap();
        });
    drop(context.render_legacy());
    context
}

fn send(app: &App, value: Value) -> UnixStream {
    let mut stream = UnixStream::connect(shell_socket_path(app.world()).unwrap()).unwrap();
    serde_json::to_writer(&mut stream, &value).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.set_nonblocking(true).unwrap();
    stream
}

fn reply(app: &mut App, mut stream: UnixStream) -> Value {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    loop {
        app.update();
        let mut chunk = [0; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => panic!("shell bridge closed before acknowledging the operation"),
            Ok(count) => {
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.last() == Some(&b'\n') {
                    return serde_json::from_slice(&bytes).unwrap();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("shell bridge reply failed: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "shell bridge did not acknowledge the operation"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn startup_terminal_launches_in_the_explicit_host_directory() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let mut app = app(&directory, Some(bedtime_plugin_terminal::PANEL_ID));
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let frontend = panel_presentation(app.world(), panel).unwrap();
    let session = app
        .world()
        .get::<TerminalPanelState>(frontend)
        .unwrap()
        .session_id;
    assert_eq!(
        app.world()
            .non_send::<BedTerminal>()
            .working_directory(session)
            .map(Path::to_path_buf),
        Some(directory.root().canonicalize().unwrap())
    );
    let _context = start_shell(&mut app, session);
    let expected = directory.root().canonicalize().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app
        .world()
        .non_send::<BedTerminal>()
        .live_working_directory(session)
        .ok()
        .as_ref()
        != Some(&expected)
    {
        app.update();
        assert!(
            Instant::now() < deadline,
            "startup shell ignored the host directory"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    app.world_mut().write_message(bevy::app::AppExit::Success);
    app.update();
}

#[test]
fn clicking_a_bare_file_location_uses_the_shells_live_directory() {
    use dear_imgui_rs::{Key, MouseButton};
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let file = directory.write("changed/src.rs", b"first\nabcdefghijk\nlast");
    let mut app = app(&directory, None);
    let (caller, _, session) = terminal(&mut app);
    let mut context = start_shell(&mut app, session);
    app.world()
        .non_send::<BedTerminal>()
        .write_active(b"cd changed; printf '\\033[2J\\033[Hsrc.rs:2:4\\r\\n'\n")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.update();
        let terminal = app.world().non_send::<BedTerminal>();
        let snapshot = terminal.active_terminal().unwrap();
        let line: String = snapshot.lines[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_ref())
            .collect();
        if line.starts_with("src.rs:2:4") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shell did not print the diagnostic"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let frame = |app: &mut App, context: &mut Context| {
        let fonts = TerminalFonts::default();
        let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
        terminal.prepare_frame(context, &fonts, 1.0).unwrap();
        context.prepare_frame(FramePrepareOptions::new([640.0, 360.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut target = [0.0; 2];
        let mut press = (false, false);
        ui.window("Shell bridge fixture")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 360.0], Condition::Always)
            .build(|| {
                let origin = ui.cursor_screen_pos();
                target = [origin[0] + 12.0, origin[1] + 12.0];
                // The default ImGui context normalizes macOS Command to Ctrl.
                press = (ui.io().key_ctrl(), ui.is_mouse_clicked(MouseButton::Left));
                terminal.render_session(ui, &fonts, session).unwrap();
            });
        drop(context.render_legacy());
        (target, press)
    };
    let (target, _) = frame(&mut app, &mut context);
    context.io_mut().add_mouse_pos_event(target);
    frame(&mut app, &mut context);
    let modifier = if cfg!(target_os = "macos") {
        Key::ModSuper
    } else {
        Key::ModCtrl
    };
    context.io_mut().add_key_event(modifier, true);
    frame(&mut app, &mut context);
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, true);
    let (press_target, (primary, pressed)) = frame(&mut app, &mut context);
    assert!(primary && pressed, "fixture must deliver a Primary+click");
    assert_eq!(
        press_target, target,
        "fixture click target must stay stable"
    );
    context
        .io_mut()
        .add_mouse_button_event(MouseButton::Left, false);
    context.io_mut().add_key_event(modifier, false);
    frame(&mut app, &mut context);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut notices = Vec::new();
    let panel = loop {
        app.update();
        notices.extend(
            app.world()
                .resource::<Messages<bedtime_core::CoreNotice>>()
                .iter_current_update_messages()
                .map(|notice| notice.message.clone()),
        );
        let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        if panel != caller {
            break panel;
        }
        assert!(
            Instant::now() < deadline,
            "click must open an editor beside the sole terminal; notices: {notices:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_ne!(
        panel, caller,
        "click must open an editor beside the sole terminal"
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas.len(),
        2
    );
    let binding = app
        .world()
        .get::<bedtime_api::documents::PanelBinding>(panel)
        .unwrap();
    let documents = app.world().non_send::<DocumentSession>();
    assert_eq!(
        binding.document,
        documents.document_for_path(&file.canonicalize().unwrap())
    );
    let snapshot = documents.view_snapshot(binding.view.unwrap()).unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 3));
    app.world_mut().write_message(bevy::app::AppExit::Success);
    app.update();
}

#[test]
fn shell_socket_tracks_actual_open_and_save_promotion_while_preserving_its_calling_process() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let directory = TempDir::new();
    let file = directory.write("project/file.txt", b"original");
    let root = file.parent().unwrap().canonicalize().unwrap();
    let environment = directory.path("environment");
    let mut app = app(&directory, None);
    let (caller, frontend, session) = terminal(&mut app);
    assert_eq!(
        app.world()
            .non_send::<BedTerminal>()
            .working_directory(session)
            .map(Path::to_path_buf),
        Some(directory.root().canonicalize().unwrap())
    );
    let _context = start_shell(&mut app, session);
    let pid = app
        .world()
        .non_send::<BedTerminal>()
        .process_id(session)
        .unwrap();
    let socket = shell_socket_path(app.world()).unwrap().to_owned();
    let command = format!(
        "printf '%s\\n%s\\n%s' \"$BEDTERM_SOCKET\" \"$BEDTERM_PANE\" \"$BED_BRIDGE_TEST\" > '{}'\n",
        environment.to_string_lossy().replace('\'', "'\\''")
    );
    app.world()
        .non_send::<BedTerminal>()
        .write_active(command.as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fs::read(&environment).unwrap_or_default().is_empty() {
        app.update();
        assert!(
            Instant::now() < deadline,
            "shell did not publish its bridge environment"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let environment = fs::read_to_string(environment).unwrap();
    let lines = environment.lines().collect::<Vec<_>>();
    assert_eq!(Path::new(lines[0]), socket);
    assert_eq!(lines[2], "profile retained");
    let pane = lines[1];
    assert_eq!(
        app.world().non_send::<BedTerminal>().session_for_pane(pane),
        Some(session)
    );
    let (other, _, other_session) = terminal(&mut app);
    let mut tiling = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let largest = tiling.layout.split(1, 0, 2500, 100).unwrap();
    tiling.add_area(largest);
    tiling.assign(
        app.world().get::<PanelPresentation>(other).unwrap().id,
        largest,
    );
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
    assert_eq!(
        app.world().non_send::<BedTerminal>().active_session_id(),
        Some(other_session)
    );

    let failure = send(
        &app,
        json!({"command":"workspace", "root":directory.path("missing").as_os_str().as_bytes(), "pane":pane}),
    );
    app.update();
    assert!(
        reply(&mut app, failure)["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
    assert!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert!(app.world().get::<PanelPresentation>(other).is_some());

    let open = send(
        &app,
        json!({"command":"open", "paths":[file.as_os_str().as_bytes()], "pane":pane}),
    );
    app.update();
    assert!(reply(&mut app, open)["error"].is_null());
    let editor = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let editor_frontend = panel_presentation(app.world(), editor).unwrap();
    let binding = app
        .world()
        .get::<bedtime_api::documents::PanelBinding>(editor_frontend)
        .unwrap();
    let view = binding.view.unwrap();
    let layout = app.world().resource::<WorkbenchLayout>();
    assert_eq!(layout.tiling.areas.len(), 2);
    assert_eq!(
        layout
            .tiling
            .area_for(app.world().get::<PanelPresentation>(editor).unwrap().id),
        Some(largest),
    );
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.paste(b"dirty "))
        .unwrap();
    let request = capture_app_request(
        app.world(),
        AppAction::FocusPanel(other),
        PanelTarget::LastFocused,
    );
    submit_app_request(app.world_mut(), request);
    process_app_requests(app.world_mut()).unwrap();

    let promote = send(
        &app,
        json!({"command":"workspace", "root":root.as_os_str().as_bytes(), "pane":pane}),
    );
    app.update();
    let promotion_reply = reply(&mut app, promote);
    assert!(promotion_reply["error"].is_null(), "{promotion_reply}");
    assert_eq!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .unwrap()
            .root,
        root.to_str().unwrap()
    );
    assert_eq!(panel_presentation(app.world(), caller), Some(frontend));
    assert_eq!(
        app.world().non_send::<BedTerminal>().process_id(session),
        Some(pid)
    );
    assert_eq!(
        app.world().non_send::<BedTerminal>().session_ids(),
        vec![session]
    );
    assert!(app.world().get::<PanelPresentation>(other).is_none());
    assert!(app.world().get::<PanelPresentation>(editor).is_none());
    assert_eq!(fs::read(file).unwrap(), b"dirty original");

    app.world_mut().write_message(bevy::app::AppExit::Success);
    app.update();
    assert!(!socket.exists());
    assert_eq!(app.world().non_send::<BedTerminal>().session_count(), 0);
}

#[test]
fn bed_shell_entrypoints_preserve_gui_arguments_and_forward_registered_aliases() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for command in ["open", "workspace"] {
        let args = [
            OsString::from("bed"),
            OsString::from(command),
            OsString::from_vec(b"invalid\0argument".to_vec()),
        ]
        .to_vec();
        let error = crate::app::run_shell_entrypoint(args.clone(), true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "Invalid shell argument bytes");
        assert!(
            crate::app::run_shell_entrypoint(args, false)
                .unwrap()
                .is_none()
        );
    }
    for command in ["ducky", "--help"] {
        let outcome = crate::app::launcher()
            .unwrap()
            .with_args([OsString::from("bed"), OsString::from(command)])
            .dispatch()
            .unwrap();
        assert!(matches!(outcome, bedtime::LaunchOutcome::Desktop(_)));
    }
}
