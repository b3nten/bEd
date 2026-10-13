//! Private shell commands enter the same ordered, tracked editor reducer as UI.
use crate::TerminalPanelState;
use bed_api::workbench::{
    CommandInvocation, CommandResult, PanelOwner, PanelPresentation, WorkbenchAction,
};
use bed_api::{
    features::{HostAction, HostEffects, HostFrame},
    workbench::PanelTarget,
};
use bed_documents::DocumentSession;
use bed_terminal::{
    bed_terminal::BedTerminal,
    shell_bridge::{self, Bridge, Request},
};
use bed_workbench::{WorkbenchLayout, WorkbenchRequestId, WorkbenchRequests};
use bevy::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) const COMMAND: &str = "bed.terminal.editor.shell_request";

struct PendingRequest {
    source: Option<Entity>,
    request: HostAction,
}

type ShellContext<'w> = (
    Option<Res<'w, WorkbenchLayout>>,
    Option<Res<'w, bed_workbench::WorkbenchRequestMode>>,
    Option<Res<'w, HostFrame>>,
    Option<NonSend<'w, DocumentSession>>,
);

#[derive(Resource, Default)]
pub(crate) struct ShellBridge {
    bridge: Option<Bridge>,
    environment: HashMap<String, String>,
    next: u64,
    requests: HashMap<u64, PendingRequest>,
    operations: HashMap<u64, WorkbenchRequestId>,
    results: HashMap<u64, Result<(), String>>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ShellBridge>()
        .init_resource::<WorkbenchRequests>()
        .add_systems(Startup, initialize)
        .add_systems(
            PreUpdate,
            poll.after(bed_terminal::TerminalCoreSystems::Poll)
                .before(bed_workbench::WorkbenchSystems::Requests),
        )
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
}

fn initialize(
    mut shell: ResMut<ShellBridge>,
    mut terminal: NonSendMut<BedTerminal>,
    mut intents: ResMut<HostEffects>,
) {
    let result = (|| -> std::io::Result<_> {
        let bridge = Bridge::new()?;
        let executable = std::env::current_exe()?;
        let mut paths = vec![bridge.helper_directory().to_owned()];
        if let Some(parent) = executable.parent() {
            paths.push(parent.to_owned());
        }
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths).map_err(std::io::Error::other)?;
        let environment = HashMap::from([
            (
                shell_bridge::SOCKET_ENV.into(),
                bridge.socket_path().to_string_lossy().into_owned(),
            ),
            (
                "BEDTERM_EXE".into(),
                executable.to_string_lossy().into_owned(),
            ),
            ("PATH".into(), path.to_string_lossy().into_owned()),
        ]);
        Ok((bridge, environment))
    })();
    match result {
        Ok((bridge, environment)) => {
            terminal.configure_shell_environment(environment.clone());
            shell.environment = environment;
            shell.bridge = Some(bridge);
        }
        Err(error) => intents.push(
            None,
            HostAction::Notify {
                message: format!("Terminal shell commands are unavailable: {error}"),
            },
        ),
    }
}

fn poll(
    mut shell: ResMut<ShellBridge>,
    mut terminal: NonSendMut<BedTerminal>,
    panels: Query<(Entity, &TerminalPanelState, Option<&PanelOwner>)>,
    presentations: Query<&PanelPresentation>,
    context: ShellContext,
    mut requests: ResMut<WorkbenchRequests>,
    mut intents: ResMut<HostEffects>,
) {
    let (layout, mode, frame, documents) = context;
    let source_for_session = |session| {
        panels.iter().find_map(|(entity, state, owner)| {
            let owner = owner.map_or(entity, |owner| owner.0);
            (state.session_id == session && presentations.get(owner).is_ok()).then_some(entity)
        })
    };
    for (session, target) in terminal.take_open_links() {
        let source = source_for_session(session);
        let result = (|| -> Result<(), String> {
            let source =
                source.ok_or("The terminal that opened this link is no longer available")?;
            use bed_terminal::terminal_links::{LinkTarget, classify_link};
            match classify_link(&target).ok_or("This link has no supported opening action")? {
                LinkTarget::External(url) => {
                    #[cfg(target_os = "macos")]
                    let program = "open";
                    #[cfg(not(target_os = "macos"))]
                    let program = "xdg-open";
                    let mut child = std::process::Command::new(program)
                        .arg(url)
                        .spawn()
                        .map_err(|error| error.to_string())?;
                    std::thread::Builder::new()
                        .name("bed-open-link".into())
                        .spawn(move || {
                            let _ = child.wait();
                        })
                        .map_err(|error| error.to_string())?;
                }
                LinkTarget::File {
                    host,
                    mut path,
                    position,
                } => {
                    let remote = terminal.is_remote_session(session);
                    #[cfg(not(feature = "remote"))]
                    let _ = host;
                    #[cfg(feature = "remote")]
                    if let Some(ssh) = terminal.ssh_target(session) {
                        let remote_host = ssh.host.split('@').next_back().unwrap_or(&ssh.host);
                        if host.as_deref().is_some_and(|host| host != remote_host) {
                            return Err("The file link belongs to a different SSH host".into());
                        }
                        if !documents.as_ref().is_some_and(|documents| {
                            documents
                                .ssh_target()
                                .is_some_and(|target| target.host == ssh.host)
                        }) {
                            return Err(
                                "Open this terminal's SSH workspace before opening its file links"
                                    .into(),
                            );
                        }
                    }
                    if let Ok(relative) = path.strip_prefix("~") {
                        if remote {
                            return Err("SSH file links must use an absolute path or a path relative to the shell's directory".into());
                        }
                        let home =
                            std::env::var_os("HOME").ok_or("The home directory is unavailable")?;
                        path = std::path::PathBuf::from(home).join(relative);
                    }
                    if !path.is_absolute() {
                        let directory = if remote {
                            terminal.reported_working_directory(session)
                                .ok_or("The SSH shell must report its working directory with OSC 7 to open relative file links")?
                                .to_owned()
                        } else {
                            terminal
                                .live_working_directory(session)
                                .map_err(|error| error.to_string())?
                        };
                        path = directory.join(path);
                    }
                    intents.push(
                        Some(source),
                        HostAction::OpenTerminalLink { path, position },
                    );
                }
            }
            Ok(())
        })();
        if let Err(message) = result {
            intents.push(source, HostAction::Notify { message });
        }
    }
    if shell.bridge.is_none() {
        return;
    }
    // This merges required values without discarding shell/profile overrides.
    terminal.configure_shell_environment(shell.environment.clone());
    let completed = shell
        .operations
        .iter()
        .filter_map(|(&nonce, &operation)| {
            requests
                .take_result(operation)
                .map(|result| (nonce, result.map(|_| ())))
        })
        .collect::<Vec<_>>();
    for (nonce, result) in completed {
        shell.operations.remove(&nonce);
        shell.requests.remove(&nonce);
        shell.results.insert(nonce, result);
    }
    let ShellBridge {
        bridge,
        next,
        requests: pending,
        operations,
        results,
        ..
    } = &mut *shell;
    let bridge = bridge.as_mut().expect("checked above");
    let result = bridge.poll_deferred(
        |request| {
            if !mode
                .as_ref()
                .is_some_and(|mode| **mode == bed_workbench::WorkbenchRequestMode::External)
            {
                return Err("Shell commands require an editor workbench".into());
            }
            let pane = match &request {
                Request::Open { pane, .. }
                | Request::Workspace { pane, .. }
                | Request::Ducky { pane } => pane,
            };
            let session = match pane {
                Some(pane) => Some(terminal.session_for_pane(pane).ok_or_else(|| {
                    "The shell that requested this command is no longer available".to_owned()
                })?),
                None => terminal.active_session_id(),
            };
            let source = session.and_then(source_for_session).filter(|source| {
                panels
                    .get(*source)
                    .is_ok_and(|(_, panel, _)| !panel.command)
            });
            if pane.is_some() && source.is_none() {
                return Err("The shell that requested this command is no longer available".into());
            }
            let host = match request {
                Request::Open { paths, .. } => HostAction::OpenTerminalFiles { paths },
                Request::Workspace { root, .. } => {
                    if frame.as_ref().is_some_and(|frame| frame.remote)
                        || documents
                            .as_ref()
                            .is_some_and(|documents| documents.is_remote())
                        || session.is_some_and(|session| terminal.is_remote_session(session))
                    {
                        return Err("Workspace upgrades require a local terminal".into());
                    }
                    HostAction::PromoteWorkspace { root }
                }
                Request::Ducky { .. } => HostAction::ShowPanelIn {
                    panel_type: "bed.duck.panel".into(),
                    document: None,
                    state: Value::Null,
                    action: None,
                    target: PanelTarget::LargestArea,
                },
            };
            let layout = layout
                .as_ref()
                .ok_or_else(|| "Shell commands require a workbench layout".to_owned())?;
            let owner = source.and_then(|source| {
                panels
                    .get(source)
                    .ok()
                    .map(|(_, _, owner)| owner.map_or(source, |owner| owner.0))
            });
            let area = owner
                .and_then(|owner| presentations.get(owner).ok())
                .and_then(|panel| layout.tiling.area_for(panel.id))
                .or(layout.focused_area)
                .unwrap_or(layout.tiling.layout.areas[0].id);
            *next = next.checked_add(1).expect("shell request ID overflow");
            let nonce = *next;
            pending.insert(
                nonce,
                PendingRequest {
                    source,
                    request: host,
                },
            );
            let operation = requests.submit(WorkbenchAction::Invoke(CommandInvocation {
                command: COMMAND.into(),
                source,
                area,
                arguments: Value::from(nonce),
            }));
            operations.insert(nonce, operation);
            Ok(nonce)
        },
        |nonce| results.remove(&nonce),
    );
    let waiting = bridge.pending_requests().collect::<HashSet<_>>();
    results.retain(|nonce, _| waiting.contains(nonce));
    if let Err(error) = result {
        intents.push(
            None,
            HostAction::Notify {
                message: format!("Terminal shell command socket failed: {error}"),
            },
        );
    }
}

pub(crate) fn apply_request(
    In(invocation): In<CommandInvocation>,
    mut shell: ResMut<ShellBridge>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let nonce = invocation
        .arguments
        .as_u64()
        .ok_or("Invalid shell request correlation")?;
    let request = shell
        .requests
        .remove(&nonce)
        .ok_or("The shell request is no longer available")?;
    if request.source != invocation.source {
        return Err("The shell request's originating panel changed".into());
    }
    if matches!(
        &request.request,
        HostAction::ShowPanelIn { .. }
            | HostAction::OpenFileIn { .. }
            | HostAction::OpenTerminalFiles { .. }
    ) {
        intents.push(request.source, request.request);
    } else {
        intents.push_at(request.source, invocation.area, request.request);
    }
    Ok(Vec::new())
}

fn shutdown(mut exit: MessageReader<bevy::app::AppExit>, mut shell: ResMut<ShellBridge>) {
    if exit.read().next().is_some() {
        shell.bridge = None;
        shell.environment.clear();
        shell.requests.clear();
        shell.operations.clear();
        shell.results.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bed_app::{
        AppAction, AppOptions, AppRuntimePlugin, PanelPresentation, WorkspaceState,
        capture_app_request, open_panel, panel_presentation, process_app_requests,
        submit_app_request,
    };
    use bed_terminal::{terminal_font::TerminalFonts, terminal_pty::TerminalShell};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    use serde_json::json;
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{ffi::OsStrExt, net::UnixStream},
        path::Path,
        time::{Duration, Instant},
    };

    fn app(directory: &TempDir, startup_panel: Option<&str>) -> App {
        let mut app = App::new();
        let config = directory.path("config");
        let settings = bed_app_config::Settings::with_paths(
            config.clone(),
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        )
        .unwrap();
        app.insert_non_send(bed_ui::icons::Icons::default())
            .insert_non_send(bed_app::WorkspacePersistence::new(
                bed_app::WorkspaceStore::load(&config).unwrap(),
            ))
            .insert_non_send(settings)
            .insert_resource(bed_app::RemoteHelperDirectory(std::path::PathBuf::new()));

        app.add_plugins((
            AppRuntimePlugin {
                options: AppOptions {
                    working_directory: Some(directory.root().to_owned()),
                    startup_panel: startup_panel.map(ToOwned::to_owned),
                    ..Default::default()
                },
            },
            bed_plugin_editor::EditorPlugin,
            crate::TerminalPlugin,
            crate::TerminalAppIntegrationPlugin,
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
            crate::PANEL_ID,
            bed_api::documents::PanelInput::None,
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
        let bridge = app
            .world()
            .resource::<ShellBridge>()
            .bridge
            .as_ref()
            .unwrap();
        let mut stream = UnixStream::connect(bridge.socket_path()).unwrap();
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

    fn has_no_reply(stream: &mut UnixStream) {
        let mut byte = [0];
        assert_eq!(
            stream.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn startup_terminal_launches_in_the_explicit_host_directory() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let mut app = app(&directory, Some(crate::PANEL_ID));
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
                .working_directory(session),
            Some(directory.root())
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
            ui.window("Shell bridge fixture")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 360.0], Condition::Always)
                .build(|| {
                    let origin = ui.cursor_screen_pos();
                    target = [origin[0] + 12.0, origin[1] + 12.0];
                    terminal.render_session(ui, &fonts, session).unwrap();
                });
            drop(context.render_legacy());
            target
        };
        let target = frame(&mut app, &mut context);
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
        frame(&mut app, &mut context);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        context.io_mut().add_key_event(modifier, false);
        frame(&mut app, &mut context);
        app.update();
        let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
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
            .get::<bed_api::documents::PanelBinding>(panel)
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
                .working_directory(session),
            Some(directory.root())
        );
        let _context = start_shell(&mut app, session);
        let pid = app
            .world()
            .non_send::<BedTerminal>()
            .process_id(session)
            .unwrap();
        let socket = app
            .world()
            .resource::<ShellBridge>()
            .bridge
            .as_ref()
            .unwrap()
            .socket_path()
            .to_owned();
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

        let mut failure = send(
            &app,
            json!({"command":"workspace", "root":directory.path("missing").as_os_str().as_bytes(), "pane":pane}),
        );
        app.update();
        has_no_reply(&mut failure);
        assert!(
            reply(&mut app, failure)["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
        assert!(app.world().resource::<WorkspaceState>().spec.is_none());
        assert!(app.world().get::<PanelPresentation>(other).is_some());

        let mut open = send(
            &app,
            json!({"command":"open", "paths":[file.as_os_str().as_bytes()], "pane":pane}),
        );
        app.update();
        has_no_reply(&mut open);
        assert!(reply(&mut app, open)["error"].is_null());
        let editor = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        let editor_frontend = panel_presentation(app.world(), editor).unwrap();
        let binding = app
            .world()
            .get::<bed_api::documents::PanelBinding>(editor_frontend)
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

        let mut promote = send(
            &app,
            json!({"command":"workspace", "root":root.as_os_str().as_bytes(), "pane":pane}),
        );
        app.update();
        has_no_reply(&mut promote);
        assert!(reply(&mut app, promote)["error"].is_null());
        assert_eq!(
            app.world()
                .resource::<WorkspaceState>()
                .spec
                .as_ref()
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
}
