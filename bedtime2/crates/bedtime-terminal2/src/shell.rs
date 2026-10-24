//! Local helpers call application-owned handlers with a captured terminal origin.
use crate::{TerminalOptions, TerminalPanel};
use bedtime_terminal::shell_bridge::{self, Bridge};
use bedtime_workbench2::ManagedWork;
use bevy::prelude::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Handle a `+alias` executable before constructing the Bevy application.
/// Ordinary application invocations return None. A helper failure must exit,
/// rather than falling through to GUI startup.
pub fn run_shell_command(args: impl IntoIterator<Item = OsString>) -> Option<io::Result<()>> {
    let mut args = args.into_iter();
    let executable = args.next()?;
    let alias = Path::new(&executable).file_name()?.to_str()?;
    alias
        .starts_with('+')
        .then(|| shell_bridge::invoke(alias, args))
}

/// Ergonomic main-entrypoint variant: report helper errors and return its exit
/// status. This never constructs an App or terminates the process itself.
pub fn handle_shell_command() -> Option<AppExit> {
    run_shell_command(std::env::args_os()).map(|result| match result {
        Ok(()) => AppExit::Success,
        Err(error) => {
            eprintln!("{error}");
            AppExit::error()
        }
    })
}

/// Arguments and cwd belong to the helper process, after any shell `cd`.
/// Panel/workspace have been validated against the current terminal runtime.
pub struct ShellInvocation {
    pub alias: String,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
    pub panel: Entity,
    pub reply: ShellReply,
}

type Completion = Arc<Mutex<Option<Result<(), String>>>>;

/// Complete after the application operation succeeds or fails. Keep this with
/// asynchronous work; dropping it without completion reports an abandoned command.
pub struct ShellReply(Completion);

impl ShellReply {
    pub fn complete(self, result: Result<(), String>) {
        *self.0.lock().expect("Shell reply lock") = Some(result);
    }
}

impl Drop for ShellReply {
    fn drop(&mut self) {
        let mut result = self.0.lock().expect("Shell reply lock");
        if result.is_none() {
            *result = Some(Err("Shell command was abandoned before completion".into()));
        }
    }
}

type Handler = fn(&mut World, ShellInvocation);

pub trait ShellCommandsAppExt {
    /// Register static local aliases during composition, before the first update.
    /// Handlers run on the application thread and must not block on I/O.
    fn add_shell_command(
        &mut self,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        handler: fn(&mut World, ShellInvocation),
    ) -> Result<&mut Self, String>;
}

impl ShellCommandsAppExt for App {
    fn add_shell_command(
        &mut self,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        handler: fn(&mut World, ShellInvocation),
    ) -> Result<&mut Self, String> {
        self.init_resource::<ShellCommands>();
        let mut commands = self.world_mut().resource_mut::<ShellCommands>();
        if commands.started {
            return Err("Register shell commands before application startup".into());
        }
        let aliases: Vec<String> = aliases.into_iter().map(Into::into).collect();
        let mut seen = std::collections::BTreeSet::new();
        for alias in &aliases {
            shell_bridge::validate_alias(alias)?;
            if commands.handlers.contains_key(alias) || !seen.insert(alias) {
                return Err(format!("Duplicate shell alias: {alias}"));
            }
        }
        for alias in aliases {
            commands.handlers.insert(alias, handler);
        }
        Ok(self)
    }
}

#[derive(Resource, Default)]
pub(crate) struct ShellCommands {
    handlers: BTreeMap<String, Handler>,
    started: bool,
    bridge: Option<Bridge>,
    error: Option<String>,
    next: u64,
    pending: BTreeMap<u64, Completion>,
}

pub fn shell_socket_path(world: &World) -> Option<&Path> {
    world
        .get_resource::<ShellCommands>()?
        .bridge
        .as_ref()
        .map(Bridge::socket_path)
}

pub(crate) fn initialize(mut commands: ResMut<ShellCommands>) {
    commands.started = true;
    if commands.handlers.is_empty() {
        return;
    }
    match Bridge::with_aliases(commands.handlers.keys()) {
        Ok(bridge) => commands.bridge = Some(bridge),
        Err(error) => {
            let error = format!("Terminal shell commands are unavailable: {error}");
            bevy::log::error!("{error}");
            commands.error = Some(error);
        }
    }
}

impl ShellCommands {
    pub(crate) fn configure(
        &self,
        options: &mut TerminalOptions,
        panel: Entity,
    ) -> Result<Option<String>, String> {
        // A nested application must never inherit its parent's endpoint/origin.
        for name in [
            shell_bridge::SOCKET_ENV,
            shell_bridge::PANE_ENV,
            "BEDTERM_EXE",
        ] {
            options.env.remove(name);
            if !options.env_remove.iter().any(|entry| entry == name) {
                options.env_remove.push(name.into());
            }
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let Some(bridge) = &self.bridge else {
            return Ok(None);
        };
        let token = format!(
            "{}-{}",
            panel.to_bits(),
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        let inherited = options
            .env
            .get("PATH")
            .map(OsString::from)
            .unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default());
        let paths = std::iter::once(bridge.helper_directory().to_owned())
            .chain(std::env::split_paths(&inherited));
        let path = std::env::join_paths(paths)
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|_| "Terminal helper PATH is not UTF-8".to_owned())?;
        let socket = bridge
            .socket_path()
            .to_str()
            .ok_or("Terminal helper socket path is not UTF-8")?;
        options.env.insert("PATH".into(), path);
        options
            .env
            .insert(shell_bridge::SOCKET_ENV.into(), socket.into());
        options
            .env
            .insert(shell_bridge::PANE_ENV.into(), token.clone());
        Ok(Some(token))
    }
}

pub(crate) fn poll(world: &mut World) {
    world.resource_scope(|world, mut commands: Mut<ShellCommands>| {
        let ShellCommands {
            bridge,
            handlers,
            next,
            pending,
            ..
        } = &mut *commands;
        let Some(bridge) = bridge else { return };
        let mut admitted = Vec::new();
        let result = bridge.poll_deferred(
            |request| {
                let request = request.into_invocation();
                let handler = *handlers
                    .get(&request.alias)
                    .ok_or_else(|| format!("Unknown shell command: {}", request.alias))?;
                let token = request
                    .pane
                    .as_deref()
                    .ok_or("Shell command has no originating terminal")?;
                let panel = world
                    .query::<(Entity, &TerminalPanel)>()
                    .iter(world)
                    .find_map(|(panel, terminal)| {
                        terminal
                            .runtime
                            .as_ref()
                            .filter(|runtime| {
                                runtime.shell_token.as_deref() == Some(token)
                                    && runtime.pty.is_alive()
                                    && matches!(
                                        terminal.status(),
                                        crate::TerminalStatus::Starting
                                            | crate::TerminalStatus::Running
                                    )
                            })
                            .map(|_| panel)
                    })
                    .ok_or("The originating terminal has closed or restarted")?;
                if world
                    .get_resource::<ManagedWork>()
                    .is_none_or(ManagedWork::is_closing)
                {
                    return Err("The originating workspace is closing".into());
                }
                *next = next.checked_add(1).expect("Shell request ID exhausted");
                let id = *next;
                let completion = Arc::new(Mutex::new(None));
                admitted.push((id, completion.clone()));
                handler(
                    world,
                    ShellInvocation {
                        alias: request.alias,
                        arguments: request
                            .arguments
                            .into_iter()
                            .map(OsString::from_vec)
                            .collect(),
                        directory: request.directory,
                        panel,
                        reply: ShellReply(completion),
                    },
                );
                Ok(id)
            },
            |id| pending.get(&id)?.lock().expect("Shell reply lock").take(),
        );
        pending.extend(admitted);
        let waiting: std::collections::BTreeSet<_> = bridge.pending_requests().collect();
        pending.retain(|id, _| waiting.contains(id));
        if let Err(error) = result {
            bevy::log::error!("Terminal command socket failed: {error}");
        }
    });
}

pub(crate) fn shutdown(mut exit: MessageReader<AppExit>, mut commands: ResMut<ShellCommands>) {
    if exit.read().next().is_some() {
        commands.bridge = None;
        commands.pending.clear();
    }
}
