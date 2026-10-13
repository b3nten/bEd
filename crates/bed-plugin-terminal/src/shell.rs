//! Local shell aliases dispatch to their owning registered workbench command.
use crate::TerminalPanelState;
#[cfg(feature = "remote")]
use bed_api::files::FileProviderId;
use bed_api::workbench::{
    CommandInvocation, CommandPresentations, PanelOwner, PanelPresentation, ShellInvocation,
    WorkbenchAction, WorkbenchRegistry,
};
use bed_api::{
    features::{HostAction, HostEffects},
    files::FileContext,
};
use bed_terminal::{
    bed_terminal::BedTerminal,
    shell_bridge::{self, Bridge},
};
use bed_workbench::{WorkbenchLayout, WorkbenchRequestId, WorkbenchRequests};
use bevy::prelude::*;
#[cfg(test)]
use serde_json::Value;
use std::collections::{HashMap, HashSet};

type ShellContext<'w> = (
    Option<Res<'w, WorkbenchLayout>>,
    Res<'w, WorkbenchRegistry>,
    Option<Res<'w, FileContext>>,
    Option<Res<'w, CommandPresentations>>,
);

#[derive(Resource, Default)]
pub(crate) struct ShellBridge {
    bridge: Option<Bridge>,
    environment: HashMap<String, String>,
    next: u64,
    operations: HashMap<u64, WorkbenchRequestId>,
    results: HashMap<u64, Result<(), String>>,
}

/// The active private local command endpoint, if any aliases were registered.
pub fn socket_path(world: &World) -> Option<&std::path::Path> {
    world
        .get_resource::<ShellBridge>()?
        .bridge
        .as_ref()
        .map(Bridge::socket_path)
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
    mut intents: Option<ResMut<HostEffects>>,
    registry: Res<WorkbenchRegistry>,
    mut activity: ResMut<bed_terminal::TerminalActivity>,
) {
    if registry.shell_aliases().is_empty() {
        return;
    }
    let result = (|| -> std::io::Result<_> {
        let bridge =
            Bridge::with_aliases(registry.shell_aliases().iter().map(|entry| entry.alias))?;
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
        Err(error) => report_error(
            &mut intents,
            &mut activity,
            None,
            format!("Terminal shell commands are unavailable: {error}"),
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
    feedback: (
        Option<ResMut<HostEffects>>,
        ResMut<bed_terminal::TerminalActivity>,
    ),
) {
    let (mut intents, mut activity) = feedback;
    let (layout, registry, files, command_presentations) = context;
    #[cfg(not(feature = "remote"))]
    let _ = files;
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
                        if !files.as_ref().is_some_and(|files| matches!(&files.provider, FileProviderId::Ssh { host } if host == &ssh.host)) {
                            return Err(
                                "Open this terminal's SSH directory before opening its file links"
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
                    let intents = intents
                        .as_mut()
                        .ok_or("File links require shared document integration")?;
                    intents.push(
                        Some(source),
                        HostAction::OpenTerminalLink { path, position },
                    );
                }
            }
            Ok(())
        })();
        if let Err(message) = result {
            report_error(&mut intents, &mut activity, source, message);
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
            requests.take_result(operation).map(|result| {
                (
                    nonce,
                    result.and_then(|outcome| {
                        if matches!(outcome, bed_workbench::WorkbenchOutcome::Ignored) {
                            Err(
                                "The shell command is no longer available in its originating panel"
                                    .into(),
                            )
                        } else {
                            Ok(())
                        }
                    }),
                )
            })
        })
        .collect::<Vec<_>>();
    for (nonce, result) in completed {
        shell.operations.remove(&nonce);
        shell.results.insert(nonce, result);
    }
    let ShellBridge {
        bridge,
        next,
        operations,
        results,
        ..
    } = &mut *shell;
    let bridge = bridge.as_mut().expect("checked above");
    let result = bridge.poll_deferred(
        |request| {
            let request = request.into_invocation();
            let command = registry
                .shell_command(&request.alias)
                .ok_or_else(|| format!("Shell command unavailable: {}", request.alias))?;
            if command_presentations.as_ref().is_some_and(|presentations| {
                presentations
                    .0
                    .get(command.id)
                    .is_some_and(|presentation| !presentation.enabled || !presentation.visible)
            }) {
                return Err(format!("Shell command unavailable: {}", request.alias));
            }
            let pane = &request.pane;
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
            let arguments = ShellInvocation {
                arguments: request.arguments,
                directory: std::os::unix::ffi::OsStrExt::as_bytes(request.directory.as_os_str())
                    .to_vec(),
                pane: request.pane,
            }
            .to_arguments();
            let operation = requests.submit(WorkbenchAction::Invoke(CommandInvocation {
                command: command.id.into(),
                source,
                area,
                arguments,
            }));
            operations.insert(nonce, operation);
            Ok(nonce)
        },
        |nonce| results.remove(&nonce),
    );
    let waiting = bridge.pending_requests().collect::<HashSet<_>>();
    results.retain(|nonce, _| waiting.contains(nonce));
    if let Err(error) = result {
        report_error(
            &mut intents,
            &mut activity,
            None,
            format!("Terminal shell command socket failed: {error}"),
        );
    }
}

fn report_error(
    intents: &mut Option<ResMut<HostEffects>>,
    activity: &mut bed_terminal::TerminalActivity,
    source: Option<Entity>,
    message: String,
) {
    if let Some(intents) = intents {
        intents.push(source, HostAction::Notify { message });
    } else {
        activity.error = Some(message);
        activity.changed = true;
    }
}

fn shutdown(mut exit: MessageReader<bevy::app::AppExit>, mut shell: ResMut<ShellBridge>) {
    if exit.read().next().is_some() {
        shell.bridge = None;
        shell.environment.clear();
        shell.operations.clear();
        shell.results.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        os::unix::{ffi::OsStrExt, net::UnixStream},
        time::{Duration, Instant},
    };

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

    #[test]
    fn registered_third_party_shell_commands_need_no_workspace_or_documents() {
        #[derive(Resource, Default)]
        struct Observed(Vec<(Option<Entity>, ShellInvocation)>);
        fn command(
            In(input): In<CommandInvocation>,
            mut observed: ResMut<Observed>,
        ) -> bed_api::workbench::CommandResult {
            observed.0.push((
                input.source,
                ShellInvocation::from_arguments(&input.arguments)?,
            ));
            Ok(Vec::new())
        }
        let directory = TempDir::new();
        let mut app = App::new();
        app.add_plugins((bed_workbench::WorkbenchPlugin, crate::TerminalPlugin));
        app.init_resource::<Observed>();
        bed_workbench::register(app.world_mut(), "example", |register| {
            register.command("example.shell", "Custom shell command", command);
            register.shell_alias("+custom", "example.shell");
        })
        .unwrap();
        app.update();
        assert!(
            app.world()
                .get_non_send::<bed_documents::DocumentSession>()
                .is_none()
        );
        let bytes = b"argument\xff".to_vec();
        let stream = send(
            &app,
            json!({"command":"invoke", "alias":"+custom", "arguments":[bytes], "directory":directory.root().as_os_str().as_bytes()}),
        );
        assert!(reply(&mut app, stream)["error"].is_null());
        let observed = &app.world().resource::<Observed>().0;
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].1.arguments, vec![bytes]);
        assert_eq!(
            observed[0].1.directory,
            directory.root().as_os_str().as_bytes()
        );
        let stream = send(
            &app,
            json!({"command":"invoke", "alias":"+unavailable", "arguments":[], "directory":directory.root().as_os_str().as_bytes()}),
        );
        assert!(
            reply(&mut app, stream)["error"]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .insert(
                "example.shell".into(),
                bed_api::workbench::CommandPresentation {
                    label: None,
                    enabled: false,
                    visible: true,
                },
            );
        let stream = send(
            &app,
            json!({"command":"invoke", "alias":"+custom", "arguments":[], "directory":directory.root().as_os_str().as_bytes()}),
        );
        assert!(
            reply(&mut app, stream)["error"]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );
        assert_eq!(app.world().resource::<Observed>().0.len(), 1);
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
}
