//! Local terminal panels. Workbench owns the runtime's lifetime; the terminal
//! worker alone owns the PTY and emulator. Drawing never starts or reaps a child.
mod blocks;
mod color;
mod input;
#[cfg(unix)]
mod integration;
mod paste;
mod process_title;
mod shell;
pub mod shell_bridge;
pub mod terminal;
pub mod terminal_input;
pub mod terminal_links;
pub mod terminal_pty;
mod view;

use crate::{
    terminal::{Terminal, TerminalTheme},
    terminal_pty::{PtyEvent, TerminalCommand, TerminalPty},
};
use bedtime_ui::{Theme, UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Keybindings, ManagedTask, ManagedWork, Panel, PanelInstance, PanelPresentation, Root,
    WorkStatus,
};
use bevy::prelude::*;
use std::{collections::VecDeque, io, process::ExitStatus, sync::Arc};
use view::{TerminalInput, TerminalView};

pub use crate::{
    blocks::{CommandBlock, command_blocks},
    terminal::TerminalSnapshot,
    terminal_pty::{PtyOptions as TerminalOptions, TerminalShell, WindowSize},
};
pub use shell::{
    ShellCommandsAppExt, ShellInvocation, ShellReply, handle_shell_command, run_shell_command,
    shell_socket_path,
};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalSystems {
    Update,
}

/// A new terminal, focused: beside the other terminals, else in the largest
/// area.
pub struct NewTerminal;
impl bedtime_workbench::Action for NewTerminal {
    const KEY: &'static str = "terminal.new";
    const LABEL: &'static str = "New Terminal";
    const DEFAULT_KEYS: &'static [bedtime_workbench::KeyChord] =
        &[bedtime_workbench::KeyChord::new(
            bedtime_workbench::Modifiers::CONTROL,
            KeyCode::Backquote,
        )];
}

fn new_terminal(
    world: &mut World,
    _: bedtime_workbench::ActionInvocation,
) -> bedtime_workbench::ActionResult {
    use bedtime_workbench::PanelsWorldExt;
    let terminal = world.open_panel(TerminalPanel::default());
    world
        .get_entity(terminal)
        .map(|_| ())
        .map_err(|_| "The terminal couldn't be placed".into())
}

/// Another terminal, in the area of the one `New` ran for.
fn new_terminal_here(
    world: &mut World,
    invocation: bedtime_workbench::ActionInvocation,
) -> bedtime_workbench::ActionResult {
    let area = invocation
        .entity
        .and_then(|panel| world.get::<bedtime_workbench::InArea>(panel))
        .map(|area| area.0)
        .ok_or("The terminal isn't in an area")?;
    world.spawn((
        TerminalPanel::default(),
        bedtime_workbench::Place::in_area(area),
    ));
    world.flush();
    Ok(())
}

/// Installs the terminal's ordinary Update system. Requires the
/// application's Workbench and UI plugins; installs no windows or renderer.
pub struct TerminalPlugin;

impl Plugin for TerminalPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{ActionAppExt, MenusAppExt};
        app.register_type::<TerminalPanel>()
            .add_action_handler::<NewTerminal>(bedtime_workbench::Handler::new(new_terminal))
            .add_menu_item::<NewTerminal>(bedtime_workbench::StandardMenu::View, "tools")
            .init_resource::<shell::ShellCommands>()
            .add_systems(Startup, shell::initialize)
            .add_systems(
                Update,
                (
                    update_terminals,
                    report_finished_commands,
                    open_links,
                    shell::poll,
                )
                    .chain()
                    .in_set(TerminalSystems::Update),
            )
            .add_systems(Last, shell::shutdown.after(bevy::app::OnAppExitSystems));
    }
}

/// Child status, separate from the scoped runtime: an exited shell's emulator
/// remains available for scrolling, selection and copying until the panel closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    Pending,
    Starting,
    Running,
    Stopping,
    Exited(Option<ExitStatus>),
    Failed(String),
}

/// One panel owns one local shell. Explicitly supplied launch options survive
/// panel initialization. Editing options affects the next start/restart only.
/// Saved with the layout as a panel only: a restored one starts a fresh shell.
/// Without a working directory, the shell starts in the workbench's `Root`.
#[bedtime_ui::panel]
#[derive(Reflect)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
// New, with a terminal focused, is another terminal beside it.
#[require(bedtime_workbench::ActionHandler<bedtime_workbench::New> = bedtime_workbench::ActionHandler::new(bedtime_workbench::Handler::new(new_terminal_here)))]
pub struct TerminalPanel {
    #[reflect(ignore)]
    pub options: TerminalOptions,
    #[reflect(ignore)]
    status: TerminalStatus,
    #[reflect(ignore)]
    runtime: Option<Runtime>,
    #[reflect(ignore)]
    snapshot: Arc<TerminalSnapshot>,
    #[reflect(ignore)]
    view: TerminalView,
    #[reflect(ignore)]
    size: Option<WindowSize>,
    #[reflect(ignore)]
    theme: Option<TerminalTheme>,
    #[reflect(ignore)]
    drawn: bool,
    #[reflect(ignore)]
    action: Option<Action>,
    #[reflect(ignore)]
    restart_after_stop: bool,
    #[reflect(ignore)]
    error: Option<String>,
    /// Commands finished since last reported (shell integration).
    #[reflect(ignore)]
    finished: Vec<(Option<i32>, Option<std::time::Duration>)>,
    /// Links clicked, opened in `Update`.
    #[reflect(ignore)]
    links: Vec<terminal_links::LinkTarget>,
}

// A saturated backend pauses new process input while this bounded, ordered
// handoff is retried. Releases/focus cleanup must not be dropped on WouldBlock.
const MAX_PENDING_INPUTS: usize = 512;
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

struct Runtime {
    shell_token: Option<String>,
    pty: TerminalPty,
    pending_inputs: VecDeque<TerminalInput>,
    task: ManagedTask<Result<(), String>>,
    size: WindowSize,
    theme: Option<TerminalTheme>,
}

#[derive(Clone, Copy)]
enum Action {
    Stop,
    Restart,
}

impl Default for TerminalPanel {
    fn default() -> Self {
        Self {
            options: TerminalOptions::default(),
            status: TerminalStatus::Pending,
            runtime: None,
            snapshot: Terminal::new_text_only(80, 24).snapshot(),
            view: TerminalView::default(),
            size: None,
            theme: None,
            drawn: false,
            action: None,
            restart_after_stop: false,
            error: None,
            finished: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl TerminalPanel {
    pub fn new(options: TerminalOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    pub fn status(&self) -> &TerminalStatus {
        &self.status
    }

    /// None until the worker has started its child. A PID is observation only,
    /// never a restoration descriptor or an authorization to signal a process.
    pub fn process_id(&self) -> Option<u32> {
        let pid = self.runtime.as_ref()?.pty.process_id();
        (pid != 0).then_some(pid)
    }

    /// Latest immutable viewport; hidden output is parsed continuously, but a
    /// hidden view may retain an older viewport until it is drawn again.
    pub fn snapshot(&self) -> &Arc<TerminalSnapshot> {
        &self.snapshot
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or(match &self.status {
            TerminalStatus::Failed(error) => Some(error.as_str()),
            _ => None,
        })
    }

    /// Stop the child while retaining its transcript. Pending user input is
    /// discarded so backpressure cannot block termination. Applied next update.
    pub fn stop(&mut self) {
        self.action = Some(Action::Stop);
    }

    /// Cancel and finish the old runtime before starting a fresh terminal. Applied
    /// by the next update; never overlaps two child lifetimes for one panel.
    pub fn restart(&mut self) {
        self.action = Some(Action::Restart);
    }
}

impl Panel for TerminalPanel {
    const LABEL: &'static str = "Terminal";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|p| p.focused);
        let window_focused = world
            .get::<Window>(context.surface)
            .is_some_and(|w| w.focused);
        let prior_capture = world.get::<bedtime_ui::UiCapture>(context.surface);
        let allow_interaction = window_focused
            && ui.is_enabled()
            && !prior_capture.is_some_and(|capture| capture.modal || capture.text)
            && !ui.ctx().any_popup_open()
            && ui.ctx().memory(|m| m.top_modal_layer().is_none());
        let reserved = reserved_shortcuts(world, panel);
        let mut clipboard = world
            .get_resource::<bedtime_clipboard::Clipboard>()
            .cloned();
        let mut paste_error = None;
        let mut panel_state = world.get_mut::<Self>(panel).unwrap();
        let state = &mut *panel_state;
        state.drawn = true;
        state.theme = Some(terminal_theme(context.theme));
        state.view.motion = context.motion;
        // Relative paths in the output are the shell's (OSC 7), else where
        // it started.
        state.view.directory = state
            .snapshot
            .current_directory
            .clone()
            .or_else(|| state.options.working_directory.clone());

        let snapshot = state.snapshot.clone();
        let accepts_input = matches!(state.status, TerminalStatus::Running)
            && state
                .runtime
                .as_ref()
                .is_none_or(|runtime| runtime.pending_inputs.is_empty());
        // A healthy shell needs no second header beneath the workbench tab.
        // Keep lifecycle/recovery information visible only when it is useful.
        if !accepts_input || state.error().is_some() {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: view::PADDING.x as i8,
                    right: view::PADDING.x as i8,
                    top: view::PADDING.y as i8,
                    bottom: 0,
                })
                .show(ui, |ui| {
                    if !accepts_input {
                        ui.horizontal(|ui| {
                            match &state.status {
                                TerminalStatus::Pending | TerminalStatus::Starting => {
                                    ui.weak("Starting shell…");
                                }
                                TerminalStatus::Running => {
                                    ui.weak("Input paused: waiting for the terminal worker…");
                                }
                                TerminalStatus::Stopping => {
                                    ui.weak("Stopping…");
                                }
                                TerminalStatus::Exited(Some(exit)) => {
                                    ui.weak(format!("Shell exited: {exit}"));
                                }
                                TerminalStatus::Exited(None) => {
                                    ui.weak("Shell stopped");
                                }
                                TerminalStatus::Failed(_) => {
                                    ui.weak("Terminal unavailable");
                                }
                            }
                            if matches!(
                                state.status,
                                TerminalStatus::Exited(_) | TerminalStatus::Failed(_)
                            ) && ui.small_button("Restart").clicked()
                            {
                                state.restart();
                            }
                        });
                    }
                    if let Some(error) = state.error() {
                        ui.colored_label(
                            bedtime_ui::theme::color(context.theme.palette.error),
                            error,
                        );
                    }
                });
        }
        let output = ui
            .add_enabled_ui(allow_interaction, |ui| {
                state.view.show(
                    ui,
                    &snapshot,
                    context.theme,
                    context.metrics.font_size,
                    focused,
                    accepts_input,
                    &reserved,
                )
            })
            .inner;
        state.size = Some(output.size);
        if let Some(link) = output.link {
            state.links.push(link);
        }
        let inputs = output
            .inputs
            .into_iter()
            .filter_map(|input| match input {
                TerminalInput::PasteClipboard(bridged) => {
                    match paste::clipboard_paste(clipboard.as_mut(), bridged) {
                        Ok(text) => {
                            text.map(|text| TerminalInput::Command(TerminalCommand::Paste(text)))
                        }
                        Err(error) => {
                            paste_error = Some(format!("Couldn't paste: {error}"));
                            None
                        }
                    }
                }
                input => Some(input),
            })
            .collect();
        send_inputs(state, inputs);
        if let Some(error) = paste_error {
            world.write_message(bedtime_ui::UiError(error));
        }
    }
}

fn send_inputs(state: &mut TerminalPanel, inputs: Vec<TerminalInput>) {
    let Some(runtime) = &mut state.runtime else {
        return;
    };
    if !matches!(runtime.task.status(), WorkStatus::Running) {
        return;
    }
    let count = runtime.pending_inputs.len().saturating_add(inputs.len());
    let bytes = runtime
        .pending_inputs
        .iter()
        .chain(&inputs)
        .fold(0usize, |total, input| {
            total.saturating_add(match input {
                TerminalInput::Bytes(bytes) => bytes.len(),
                TerminalInput::Command(
                    TerminalCommand::Text(text) | TerminalCommand::Paste(text),
                ) => text.len(),
                TerminalInput::Command(TerminalCommand::Key(event)) => {
                    event.text.as_ref().map_or(0, String::len)
                }
                _ => 0,
            })
        });
    if count > MAX_PENDING_INPUTS || bytes > MAX_PENDING_BYTES {
        // Do not discard an arbitrary release and leave the child believing a
        // key/button is held. Overflow stops this runtime explicitly instead.
        runtime.pending_inputs.clear();
        runtime.task.cancel();
        state.view.suspend();
        state.status =
            TerminalStatus::Failed("Terminal input backlog limit exceeded; runtime stopped".into());
        return;
    }
    runtime.pending_inputs.extend(inputs);
    while let Some(input) = runtime.pending_inputs.pop_front() {
        let result = match input {
            TerminalInput::Command(command) => {
                if !runtime.pty.is_alive()
                    && matches!(
                        command,
                        TerminalCommand::Key(_)
                            | TerminalCommand::Text(_)
                            | TerminalCommand::Paste(_)
                            | TerminalCommand::Focus(_)
                    )
                {
                    continue;
                }
                runtime
                    .pty
                    .try_command(command)
                    .map_err(|(command, error)| (TerminalInput::Command(command), error))
            }
            TerminalInput::Bytes(bytes) if runtime.pty.is_alive() => runtime
                .pty
                .write(&bytes)
                .map_err(|error| (TerminalInput::Bytes(bytes), error)),
            TerminalInput::Bytes(_) => continue,
            // Resolved by `draw` before inputs are sent.
            TerminalInput::PasteClipboard(_) => continue,
        };
        if let Err((input, error)) = result {
            if error.kind() == io::ErrorKind::WouldBlock {
                runtime.pending_inputs.push_front(input);
            } else {
                // A permanently rejected input must not let a later Enter run
                // an unintended partial line or strand earlier held input.
                runtime.pending_inputs.clear();
                runtime.task.cancel();
                state.view.suspend();
                state.status = TerminalStatus::Failed(format!(
                    "Terminal input rejected; runtime stopped: {error}"
                ));
            }
            break;
        }
    }
}

/// Commands that ran a while (10 s) and finished while you weren't looking
/// at their terminal are reported as notifications.
pub const NOTIFY_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

fn report_finished_commands(world: &mut World) {
    let window_focused = world
        .query::<&Window>()
        .iter(world)
        .any(|window| window.focused);
    let mut reports = Vec::new();
    for (mut panel, instance, presentation) in world
        .query::<(
            &mut TerminalPanel,
            &PanelInstance,
            Option<&PanelPresentation>,
        )>()
        .iter_mut(world)
    {
        let watching = window_focused && presentation.is_some_and(|p| p.focused);
        for (status, duration) in panel.finished.drain(..) {
            let Some(duration) = duration.filter(|d| *d >= NOTIFY_AFTER && !watching) else {
                continue;
            };
            let took = format_duration(duration);
            reports.push(match status {
                Some(status) if status != 0 => bedtime_workbench::Notification::error(format!(
                    "{}: a command failed (exit {status}) after {took}",
                    instance.title
                )),
                _ => bedtime_workbench::Notification::info(format!(
                    "{}: a command finished after {took}",
                    instance.title
                )),
            });
        }
    }
    for report in reports {
        bedtime_workbench::WorkbenchWorldExt::workbench_mut(world).notify(report);
    }
}

/// Open clicked links: files in the app (`open_path`, relative to the
/// shell's directory), web and mail links in the system's browser.
fn open_links(world: &mut World) {
    use bedtime_workbench::{Notification, PathOptions, WorkbenchWorldExt};
    let mut clicked = Vec::new();
    for mut panel in world.query::<&mut TerminalPanel>().iter_mut(world) {
        let directory = panel
            .snapshot
            .current_directory
            .clone()
            .or_else(|| panel.options.working_directory.clone());
        clicked.extend(panel.links.drain(..).map(|link| (link, directory.clone())));
    }
    for (link, directory) in clicked {
        let result = match link {
            terminal_links::LinkTarget::External(url) => open_externally(&url),
            terminal_links::LinkTarget::File {
                host: Some(host), ..
            } => Err(format!("That file is on another computer ({host})")),
            terminal_links::LinkTarget::File {
                host: None,
                path,
                position,
            } => {
                let path = match directory {
                    Some(directory) if path.is_relative() => directory.join(path),
                    _ => path,
                };
                let options = PathOptions {
                    line: position.map(|(line, column)| (line.max(0) as u32, column.max(0) as u32)),
                    ..Default::default()
                };
                world
                    .workbench_mut()
                    .open_path(&path, options)
                    .map(|_| ())
                    .map_err(|error| format!("{}: {error}", path.display()))
            }
        };
        if let Err(error) = result {
            world.workbench_mut().notify(Notification::warning(error));
        }
    }
}

fn open_externally(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open {url}: {error}"))
}

fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

/// One visible execution point for startup, restart, status and snapshot handoff.
/// Polls world-owned PTY work regardless of selected layout.
fn update_terminals(
    mut panels: Query<(Entity, &mut TerminalPanel, &mut PanelInstance)>,
    mut scope: ResMut<ManagedWork>,
    shell: Res<shell::ShellCommands>,
    root: Option<Res<Root>>,
) {
    for (panel, mut state, mut instance) in &mut panels {
        // A shell without a folder of its own starts in the workbench's.
        if state.options.working_directory.is_none()
            && let Some(root) = &root
        {
            state.options.working_directory = Some(root.0.clone());
        }
        update_terminal(panel, &mut state, &mut scope, &shell);

        // The PTY worker owns OS process queries, including quiet/hidden jobs.
        // This reads its cached observation; OSC titles do not rename the tab.
        let name = state.runtime.as_ref().map(|runtime| {
            runtime
                .pty
                .foreground_process_name()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| runtime.pty.shell_name().to_owned())
        });
        let title: String = name
            .as_deref()
            .unwrap_or(TerminalPanel::LABEL)
            .chars()
            .filter(|ch| !ch.is_control())
            .take(64)
            .collect();
        let title = if title.is_empty() {
            TerminalPanel::LABEL.into()
        } else {
            title
        };
        if instance.title != title {
            instance.title = title;
        }
    }
}

fn update_terminal(
    panel: Entity,
    state: &mut TerminalPanel,
    scope: &mut ManagedWork,
    shell: &shell::ShellCommands,
) {
    if !state.drawn {
        let cleanup = state.view.suspend();
        send_inputs(state, cleanup);
    }
    send_inputs(state, Vec::new());
    if let Some(action) = state.action.take() {
        state.error = None;
        match action {
            Action::Restart => {
                state.restart_after_stop = true;
                if let Some(runtime) = &mut state.runtime {
                    runtime.pending_inputs.clear();
                    runtime.task.cancel();
                    state.status = TerminalStatus::Stopping;
                }
            }
            Action::Stop => {
                // The retained exited emulator ignores Stop and emits no second
                // exit event. Repeated/application Stop is deliberately a no-op.
                if matches!(
                    state.status,
                    TerminalStatus::Running | TerminalStatus::Starting
                ) && state
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.pty.is_alive())
                {
                    // Termination bypasses a pending write whose byte budget is
                    // exhausted. Backend Stop needs no input reservation and
                    // clears its accepted write backlog on the owner thread.
                    state.runtime.as_mut().unwrap().pending_inputs.clear();
                    state.view.suspend();
                    send_inputs(state, vec![TerminalInput::Command(TerminalCommand::Stop)]);
                    if !matches!(state.status, TerminalStatus::Failed(_)) {
                        state.status = TerminalStatus::Stopping;
                    }
                }
            }
        }
    }

    if let Some(runtime) = &mut state.runtime {
        let pty = &mut runtime.pty;
        for event in pty.poll() {
            match event {
                PtyEvent::Exited(exit) => state.status = TerminalStatus::Exited(exit),
                PtyEvent::Error(error) => state.status = TerminalStatus::Failed(error),
                PtyEvent::CommandFinished { status, duration } => {
                    state.finished.push((status, duration));
                }
            }
        }
        if matches!(state.status, TerminalStatus::Starting)
            && pty.process_id() != 0
            && pty.is_alive()
        {
            state.status = TerminalStatus::Running;
        }
        state.snapshot = pty.snapshot();
        if state.drawn {
            if let Some(size) = state.size.filter(|size| *size != runtime.size) {
                match pty.resize(size) {
                    Ok(()) => runtime.size = size,
                    Err(error) => state.error = Some(error.to_string()),
                }
            }
            if state.theme != runtime.theme
                && let Some(theme) = &state.theme
            {
                match pty.command(TerminalCommand::Theme(theme.clone())) {
                    Ok(()) => runtime.theme = state.theme.clone(),
                    Err(error) => state.error = Some(error.to_string()),
                }
            }
            pty.request_snapshot();
        }
        match runtime.task.status() {
            WorkStatus::Failed(error) => state.status = TerminalStatus::Failed(error),
            WorkStatus::Finished => {
                if let Ok(Err(error)) = runtime.task.try_recv() {
                    state.status = TerminalStatus::Failed(error);
                }
            }
            WorkStatus::Cancelling | WorkStatus::Cancelled
                if !state.restart_after_stop
                    && !matches!(state.status, TerminalStatus::Failed(_)) =>
            {
                state.status = TerminalStatus::Stopping;
            }
            _ => {}
        }
    }

    if state.restart_after_stop
        && state.runtime.as_ref().is_none_or(|runtime| {
            !matches!(
                runtime.task.status(),
                WorkStatus::Running | WorkStatus::Cancelling
            )
        })
    {
        // Cancellation discards ManagedTask's return value. Re-read the
        // terminal mailbox after completion and require positive cleanup proof.
        // WorkStatus::Cancelled alone must never admit an overlapping child.
        let cleanup_confirmed = state.runtime.as_mut().is_none_or(|runtime| {
            for event in runtime.pty.poll() {
                if let PtyEvent::Error(error) = event {
                    state.error = Some(error);
                }
            }
            runtime.pty.cleanup_confirmed()
        });
        state.restart_after_stop = false;
        if cleanup_confirmed {
            state.runtime = None;
            state.status = TerminalStatus::Pending;
            state.error = None;
            state.snapshot = Terminal::new_text_only(80, 24).snapshot();
            state.view = TerminalView::default();
        } else {
            state.status = TerminalStatus::Failed(
                "Terminal cleanup could not be confirmed; restart refused".into(),
            );
        }
    }

    if matches!(state.status, TerminalStatus::Pending)
        && let Some(size) = state.size
    {
        let result = start_terminal(panel, state, size, scope, shell);
        if let Err(error) = result {
            state.status = TerminalStatus::Failed(error);
        }
    }
    state.drawn = false;
}

fn start_terminal(
    panel: Entity,
    state: &mut TerminalPanel,
    size: WindowSize,
    scope: &mut ManagedWork,
    shell: &shell::ShellCommands,
) -> Result<(), String> {
    let mut terminal = Terminal::new_text_only(size.num_cols as usize, size.num_lines as usize);
    if let Some(theme) = &state.theme {
        terminal.set_theme(theme);
    }
    let mut options = state.options.clone();
    #[cfg(unix)]
    integration::apply(&mut options);
    let shell_token = shell.configure(&mut options, panel)?;
    if !options.env_remove.iter().any(|name| name == "TERM_PROGRAM") {
        options
            .env
            .entry("TERM_PROGRAM".into())
            .or_insert_with(|| "bed".into());
    }
    let (pty, worker) =
        TerminalPty::prepare_terminal(options, size, terminal).map_err(|e| e.to_string())?;
    let task = scope.spawn_task(Some(panel), "Terminal", move |cancel| {
        worker
            .run(|| cancel.is_cancelled())
            .map_err(|e| e.to_string())
    })?;
    state.snapshot = pty.snapshot();
    state.runtime = Some(Runtime {
        shell_token,
        pty,
        pending_inputs: VecDeque::new(),
        task,
        size,
        theme: state.theme.clone(),
    });
    state.status = TerminalStatus::Starting;
    Ok(())
}

/// Every bound chord is left for the app's actions (resolved after drawing);
/// the terminal owns its other raw control and navigation keys.
fn reserved_shortcuts(world: &mut World, _panel: Entity) -> Vec<egui::KeyboardShortcut> {
    use bedtime_workbench::Modifiers;
    let Some(bindings) = world.get_resource::<Keybindings>() else {
        return Vec::new();
    };
    bindings
        .chords()
        .filter_map(|chord| {
            let key = bevy_egui::helpers::bevy_to_egui_physical_key(&chord.key)?;
            let flags = chord.modifiers.resolved();
            let ctrl = flags.contains(Modifiers::CONTROL);
            let meta = flags.contains(Modifiers::META);
            Some(egui::KeyboardShortcut::new(
                egui::Modifiers {
                    alt: flags.contains(Modifiers::ALT),
                    ctrl,
                    shift: flags.contains(Modifiers::SHIFT),
                    mac_cmd: meta,
                    command: if cfg!(target_os = "macos") {
                        meta
                    } else {
                        ctrl
                    },
                },
                key,
            ))
        })
        .collect()
}

fn terminal_theme(theme: &Theme) -> TerminalTheme {
    // The shared core ensures theme contrast while retaining shell OSC overrides
    // independently of host updates. Explicit xterm/truecolor output stays intact.
    TerminalTheme::new(
        theme.palette.panel_background,
        theme.palette.text,
        theme.palette.terminal,
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn test_layout(world: &mut bevy::prelude::World) -> bevy::prelude::Entity {
    use bedtime_workbench::LayoutsWorldExt;
    let layout = world.layouts_mut().create("Test").unwrap();
    if world.layouts().active().is_none() {
        world.layouts_mut().activate(layout).unwrap();
    }
    layout
}
