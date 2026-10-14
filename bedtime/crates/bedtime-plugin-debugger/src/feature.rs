//! Debug sessions keep their registered source-provider allocation in the world.
use super::*;
use bedtime_api::documents::DocumentFrame;
use bedtime_api::storage::{FeatureStateContribution, FeatureStateRestore};
use bedtime_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction, register,
};
use bedtime_api::{
    documents::{DocumentEvent, DocumentReply, PanelBinding},
    features::{FeatureSystems, HostEffects},
    workbench::FileDialogService,
};
use bedtime_terminal::{
    bedtime_terminal::BedTerminal,
    terminal_pty::{PtyOptions, TerminalShell},
};
use bevy::{ecs::system::SystemState, prelude::*};
use std::path::PathBuf;

#[path = "status.rs"]
mod status;

#[derive(Default)]
pub struct DebugState(pub Debugger);

#[derive(Resource, Default)]
struct RetainedTerminal(Option<u64>);

struct PendingLaunch {
    invocation: CommandInvocation,
    action: DebuggerAction,
    token: bedtime_api::documents::SaveToken,
    documents: Vec<DocumentId>,
    session_id: bedtime_documents::session::SessionId,
    target: DebugTarget,
    result: Option<Result<Vec<bedtime_api::documents::SavedDocument>, String>>,
}

#[derive(Resource, Default)]
struct LaunchBarrier(Option<PendingLaunch>);

const ACTION_COMMAND: &str = "bedtime.debug.action";
const COMMANDS: &[(&str, &str)] = &[
    (SHOW_COMMAND, "Debug"),
    (START_COMMAND, "Start Debugging"),
    (STOP_COMMAND, "Stop Debugging"),
    (RESTART_COMMAND, "Restart Debugging"),
    (CONTINUE_COMMAND, "Continue Debugging"),
    (PAUSE_COMMAND, "Pause Debugging"),
    (OVER_COMMAND, "Step Over"),
    (INTO_COMMAND, "Step Into"),
    (OUT_COMMAND, "Step Out"),
    (BREAKPOINT_COMMAND, "Toggle Breakpoint"),
];

#[derive(Component)]
struct DebugPresentation;

/// Launch directory and transport policy selected by the host.
#[derive(Resource, Clone, Default, PartialEq, Eq)]
pub struct DebugTarget {
    pub directory: String,
    pub remote: bool,
}

impl DebugTarget {
    fn resolve(&self, context: Option<&bedtime_api::files::FileContext>) -> Self {
        if !self.directory.is_empty() {
            return self.clone();
        }
        context.map_or_else(
            || self.clone(),
            |context| Self {
                directory: context.directory_string(),
                remote: self.remote
                    || matches!(
                        context.provider,
                        bedtime_api::files::FileProviderId::Ssh { .. }
                    ),
            },
        )
    }
}
fn selected_target(world: &World) -> DebugTarget {
    world
        .resource::<DebugTarget>()
        .resolve(world.get_resource::<bedtime_api::files::FileContext>())
}

pub struct DebuggerPlugin;

impl Plugin for DebuggerPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .init_non_send::<bedtime_documents::DocumentSession>();
        app.configure_sets(
            PreUpdate,
            bedtime_terminal::TerminalCoreSystems::Poll.before(FeatureSystems::Prepare),
        );
        if !app.is_plugin_added::<bedtime_terminal::TerminalCorePlugin>() {
            app.add_plugins(bedtime_terminal::TerminalCorePlugin);
        }
        app.world_mut().init_non_send::<DebugState>();
        app.init_non_send::<bedtime_editor_widgets::extensions::EditorExtensions>()
            .init_non_send::<bedtime_editor_widgets::source_intents::SourceRequests>()
            .init_resource::<bedtime_api::workbench::CommandPresentations>()
            .init_resource::<DocumentFrame>()
            .init_resource::<DebugTarget>()
            .init_resource::<bedtime_api::files::FileContext>()
            .init_resource::<HostEffects>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .init_resource::<bedtime_terminal::TerminalRetentions>()
            .init_resource::<RetainedTerminal>()
            .init_resource::<LaunchBarrier>()
            .add_message::<DocumentReply>()
            .add_message::<DocumentEvent>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                for &(id, _) in COMMANDS {
                    registrar.command_ref(id);
                }
                registrar.menu(MenuSlot::Application, SHOW_COMMAND);
                registrar.toolbar(SHOW_COMMAND);
                registrar.panel_options(PANEL_ID, true, PanelPlacement::Bottom);
            })
            .expect("Debug metadata is registered once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Debug", create_panel);
            for &(id, label) in COMMANDS {
                registrar.command(id, label, command);
            }
            registrar.command(
                "bedtime.debug.start_or_continue",
                "Start or Continue Debugging",
                start_or_continue,
            );
            for (id, keys) in [
                (SHOW_COMMAND, &["Primary+Shift+D"][..]),
                ("bedtime.debug.start_or_continue", &["F5"][..]),
                (STOP_COMMAND, &["Shift+F5"][..]),
                (BREAKPOINT_COMMAND, &["F9"][..]),
                (OVER_COMMAND, &["F10"][..]),
                (INTO_COMMAND, &["F11"][..]),
                (OUT_COMMAND, &["Shift+F11"][..]),
            ] {
                registrar.global_shortcut(id, keys);
            }
            registrar.command(ACTION_COMMAND, "Debug action", command);
            registrar.command(
                "bedtime.debug.source-action",
                "Apply debugger source action",
                source_action,
            );
            registrar.panel_icon(PANEL_ID, "debug");
            registrar.command_icon(SHOW_COMMAND, "debug");
            registrar.toolbar(SHOW_COMMAND);
            registrar.menu(SHOW_COMMAND);
        })
        .expect("DebuggerPlugin is installed once");
        status::install(app);
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(FeatureStateContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        app.add_systems(
            PreUpdate,
            (
                launch_replies,
                launch_pending,
                document_events,
                prepare,
                publish_readiness,
            )
                .chain()
                .in_set(FeatureSystems::Prepare),
        )
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
    }
}

// Native process ownership stays in BedTerminal. A release preserves a transcript
// still attached to a panel; it closes an unobserved debugger-owned process.
#[derive(Clone, Debug)]
pub(crate) struct TerminalLaunch {
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) env: std::collections::HashMap<String, Option<String>>,
    pub(crate) title: String,
}

pub(crate) struct Terminals<'a> {
    terminal: &'a mut BedTerminal,
    visible: std::collections::HashSet<u64>,
}

impl Terminals<'_> {
    pub(crate) fn spawn(&mut self, launch: TerminalLaunch) -> io::Result<(u64, u32)> {
        let program = launch
            .args
            .first()
            .ok_or_else(|| io::Error::other("Terminal command is empty"))?;
        let mut options = PtyOptions {
            working_directory: launch.cwd,
            shell: Some(TerminalShell::new(program, launch.args[1..].to_vec())),
            ..Default::default()
        };
        for (name, value) in launch.env {
            if let Some(value) = value {
                options.env.insert(name, value);
            } else {
                options.env_remove.push(name);
            }
        }
        self.terminal.new_command_session(options, launch.title)
    }

    pub(crate) fn stop(&mut self, id: u64) {
        self.terminal.stop_session_id(id);
    }

    pub(crate) fn release(&mut self, id: u64) {
        if !self.visible.contains(&id) {
            self.terminal.close_session_id(id);
        }
    }
}

fn visible(world: &mut World) -> std::collections::HashSet<u64> {
    world
        .query::<&bedtime_terminal::TerminalAttachment>()
        .iter(world)
        .map(|panel| panel.session_id)
        .collect()
}

fn publish(
    world: &mut World,
    source: Option<Entity>,
    area: Option<u32>,
    requests: Vec<HostAction>,
) {
    for request in requests {
        let mut intents = world.resource_mut::<HostEffects>();
        // Source previews use the host's editor-area placement, even when the
        // user navigates from a smaller Debug panel.
        if let Some(area) = area.filter(|_| !matches!(&request, HostAction::PreviewSource { .. })) {
            intents.push_at(source, area, request);
        } else {
            intents.push(source, request);
        }
    }
    let terminal = world.non_send::<DebugState>().0.terminal;
    let previous = std::mem::replace(&mut world.resource_mut::<RetainedTerminal>().0, terminal);
    let mut retention = world.resource_mut::<bedtime_terminal::TerminalRetentions>();
    if let Some(previous) = previous {
        retention.0.remove(&previous);
    }
    if let Some(id) = terminal {
        retention.0.insert(id);
    }
}

fn prepare(world: &mut World) {
    if !world.contains_non_send::<DocumentSession>() || !world.contains_non_send::<BedTerminal>() {
        return;
    }
    let visible = visible(world);
    let target = selected_target(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<DebugState>,
        NonSendMut<DocumentSession>,
        NonSendMut<BedTerminal>,
        Res<DocumentFrame>,
        NonSendMut<bedtime_editor_widgets::extensions::EditorExtensions>,
        NonSendMut<bedtime_editor_widgets::source_intents::SourceRequests>,
        ResMut<bedtime_api::workbench::CommandPresentations>,
    )>::new(world);
    let (
        mut debugger,
        mut documents,
        mut terminal,
        frame,
        mut source,
        mut observations,
        mut presentations,
    ) = state.get_mut(world).expect("Debug services are installed");
    let debugger = &mut debugger.0;
    if let Err(error) = debugger.tick_native(
        &mut documents,
        &mut Terminals {
            terminal: &mut terminal,
            visible,
        },
        &target.directory,
        frame.active_view,
        &mut requests,
    ) {
        debugger.error = Some(error.to_string());
    }
    for observation in std::mem::take(&mut observations.hover) {
        if let Err(error) = debugger.observe_source_hover(&observation, &documents) {
            debugger.error = Some(error.to_string());
        }
    }
    source.clear_debug();
    for document in &frame.documents {
        if let Ok(Some(presentation)) = debugger.debug_source_presentation(&documents, document.id)
        {
            source.set_source_debug(document.id, document.revision, presentation);
        }
    }
    debugger.publish_source_hover(&documents, &mut source);
    let state = debugger.session.as_ref().map(|session| session.state);
    for &(command, _) in COMMANDS {
        let enabled = match command {
            START_COMMAND => !debugger.active(),
            STOP_COMMAND | RESTART_COMMAND => debugger.active(),
            CONTINUE_COMMAND | OVER_COMMAND | INTO_COMMAND | OUT_COMMAND => {
                state == Some(SessionState::Stopped)
            }
            PAUSE_COMMAND => state == Some(SessionState::Running),
            BREAKPOINT_COMMAND => frame.active_document.is_some(),
            _ => true,
        };
        presentations.0.insert(
            command.into(),
            bedtime_api::workbench::CommandPresentation {
                enabled,
                visible: !target.remote,
                label: None,
                ..Default::default()
            },
        );
    }
    // Session updates have no originating surface. Let the host choose the
    // destination from the current layout when it resolves these intentions.
    publish(world, None, None, requests);
}

fn command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    if invocation.command == BREAKPOINT_COMMAND {
        let view = invocation.source.and_then(|source| {
            world
                .get::<PanelBinding>(source)
                .and_then(|binding| binding.view)
        });
        if let Some(view) = view {
            let mut state =
                SystemState::<(NonSendMut<DebugState>, NonSend<DocumentSession>)>::new(world);
            let (mut debugger, documents) = state
                .get_mut(world)
                .expect("Debug document services are installed");
            let document = documents
                .document_for_view(view)
                .ok_or("Editor view is closed")?;
            let row = documents
                .view_snapshot(view)
                .map_err(|error| error.to_string())?
                .primary()
                .head_row;
            debugger
                .0
                .toggle_debug_breakpoint(&documents, document, row)
                .map_err(|error| error.to_string())?;
            world
                .resource_mut::<HostEffects>()
                .push(invocation.source, HostAction::Invalidate);
        }
        return Ok(Vec::new());
    }
    let action = match invocation.command.as_str() {
        SHOW_COMMAND => None,
        START_COMMAND => Some(DebuggerAction::Start),
        STOP_COMMAND => Some(DebuggerAction::Stop),
        RESTART_COMMAND => Some(DebuggerAction::Restart),
        CONTINUE_COMMAND => Some(DebuggerAction::Continue),
        PAUSE_COMMAND => Some(DebuggerAction::Pause),
        OVER_COMMAND => Some(DebuggerAction::Over),
        INTO_COMMAND => Some(DebuggerAction::Into),
        OUT_COMMAND => Some(DebuggerAction::Out),
        ACTION_COMMAND => Some(
            serde_json::from_value(invocation.arguments.clone())
                .map_err(|error| error.to_string())?,
        ),
        _ => return Err("Unknown debug command".into()),
    };
    if matches!(action, Some(DebuggerAction::Stop)) {
        world.resource_mut::<LaunchBarrier>().0 = None;
    }
    if matches!(
        action,
        Some(DebuggerAction::Start | DebuggerAction::Restart)
    ) {
        if world.resource::<LaunchBarrier>().0.is_some() {
            return Err("Debug launch is already waiting for documents to save".into());
        }
        let documents = world.non_send::<DocumentSession>();
        let target = selected_target(world);
        if target.remote
            || documents.is_remote()
            || !cfg!(any(target_os = "macos", target_os = "linux"))
        {
            return Err("Debugging is available for local macOS and Linux projects".into());
        }
        if target.directory.is_empty() {
            return Err("Open a local project before debugging".into());
        }
        let session_id = documents.session_id();
        let ids = documents
            .document_ids()
            .into_iter()
            .filter(|document| {
                !documents.is_snapshot_document(*document)
                    && documents
                        .snapshot(*document)
                        .is_ok_and(|snapshot| !snapshot.path.is_empty())
            })
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            let token = bedtime_api::documents::SaveToken::next();
            world.resource_mut::<HostEffects>().push_at(
                invocation.source,
                invocation.area,
                show_panel(),
            );
            world.resource_mut::<HostEffects>().push_at(
                invocation.source,
                invocation.area,
                HostAction::SaveDocumentsWithResult {
                    recipient: MODULE_ID.into(),
                    token,
                    documents: ids.clone(),
                },
            );
            world.resource_mut::<LaunchBarrier>().0 = Some(PendingLaunch {
                invocation,
                action: action.unwrap(),
                token,
                documents: ids,
                session_id,
                target,
                result: None,
            });
            return Ok(Vec::new());
        }
    }
    execute(world, invocation, action)
}

fn execute(
    world: &mut World,
    invocation: CommandInvocation,
    action: Option<DebuggerAction>,
) -> CommandResult {
    let visible = visible(world);
    let target = selected_target(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<DebugState>,
        NonSendMut<DocumentSession>,
        NonSendMut<BedTerminal>,
        Option<NonSendMut<Box<dyn FileDialogService>>>,
    )>::new(world);
    let (mut debugger, mut documents, mut terminal, mut dialogs) = state
        .get_mut(world)
        .expect("Debug command services are installed");
    let debugger = &mut debugger.0;
    if invocation.command == SHOW_COMMAND || matches!(action, Some(DebuggerAction::Start)) {
        requests.push(show_panel());
        if debugger.profiles.is_empty() {
            debugger.load(None);
        }
        if debugger.cargo_workspace.is_none()
            && debugger.discovery.is_none()
            && debugger.profiles[debugger.selected].cargo.is_some()
            && let Err(error) = debugger.action_native(
                DebuggerAction::Discover,
                &mut documents,
                &mut Terminals {
                    terminal: &mut terminal,
                    visible: visible.clone(),
                },
                dialogs
                    .as_mut()
                    .map(|dialogs| &mut ***dialogs as &mut dyn FileDialogService),
                &target.directory,
                &mut requests,
            )
        {
            debugger.error = Some(error.to_string());
        }
    }
    if let Some(action) = action
        && let Err(error) = debugger.action_native(
            action,
            &mut documents,
            &mut Terminals {
                terminal: &mut terminal,
                visible,
            },
            dialogs
                .as_mut()
                .map(|dialogs| &mut ***dialogs as &mut dyn FileDialogService),
            &target.directory,
            &mut requests,
        )
    {
        debugger.error = Some(error.to_string());
    }
    publish(world, invocation.source, Some(invocation.area), requests);
    Ok(Vec::new())
}

fn launch_replies(mut replies: MessageReader<DocumentReply>, mut barrier: ResMut<LaunchBarrier>) {
    for reply in replies.read() {
        if let DocumentReply::Save {
            recipient,
            token,
            result,
        } = reply
            && recipient == MODULE_ID
            && let Some(pending) = &mut barrier.0
            && pending.token == *token
        {
            pending.result = Some(result.clone());
        }
    }
}

fn launch_pending(world: &mut World) {
    if world
        .resource::<LaunchBarrier>()
        .0
        .as_ref()
        .is_none_or(|pending| pending.result.is_none())
    {
        return;
    }
    let mut pending = world.resource_mut::<LaunchBarrier>().0.take().unwrap();
    let result = pending.result.take().unwrap().and_then(|saved| {
        let documents = world.non_send::<DocumentSession>();
        if documents.session_id() != pending.session_id {
            return Err("The workspace changed while preparing the debug launch".into());
        }
        if selected_target(world) != pending.target {
            return Err("The debug launch target changed while documents were saving".into());
        }
        if pending
            .documents
            .iter()
            .any(|document| !saved.iter().any(|saved| saved.document == *document))
        {
            return Err("The debug launch save barrier did not acknowledge every document".into());
        }
        if saved.iter().any(|saved| {
            documents.document_revision(saved.document).ok() != Some(saved.revision)
                || !documents
                    .snapshot(saved.document)
                    .is_ok_and(|snapshot| !snapshot.dirty)
        }) {
            return Err(
                "A document changed while preparing the debug launch; start debugging again".into(),
            );
        }
        if pending
            .invocation
            .source
            .is_some_and(|source| world.get_entity(source).is_err())
        {
            return Err("The debug command's originating panel is closed".into());
        }
        execute(world, pending.invocation, Some(pending.action)).map(|_| ())
    });
    if let Err(error) = result {
        world.non_send_mut::<DebugState>().0.error = Some(format!(
            "Debug launch stopped because saving failed: {error}"
        ));
        world
            .resource_mut::<HostEffects>()
            .push(None, HostAction::Invalidate);
    }
}

fn publish_readiness(world: &mut World) {
    let debugger = &world.non_send::<DebugState>().0;
    let readiness = match &debugger.error {
        Some(error) => bedtime_api::workbench::PanelReadiness::Failed(error.clone()),
        None if debugger.build.is_some()
            || world.resource::<LaunchBarrier>().0.is_some()
            || debugger.session.as_ref().is_some_and(|session| {
                !matches!(
                    session.state,
                    SessionState::Running | SessionState::Stopped | SessionState::Terminated
                )
            }) =>
        {
            bedtime_api::workbench::PanelReadiness::Pending
        }
        None => bedtime_api::workbench::PanelReadiness::Ready,
    };
    let entities = world
        .query_filtered::<Entity, With<DebugPresentation>>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in entities {
        world.entity_mut(entity).insert(readiness.clone());
    }
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bedtime_api::workbench::PanelDocumentation::new(&[
        ("Run", "Configure a launch profile, then start debugging. Use the session controls to continue, pause, step, or stop."),
        ("Inspect", "Select a stack frame to inspect variables. Add expressions to Watches and manage breakpoints in this panel or the editor gutter."),
    ]));
    if world.non_send::<DebugState>().0.profiles.is_empty() {
        world.non_send_mut::<DebugState>().0.load(None);
    }
    world.entity_mut(creation.entity).insert(DebugPresentation);
    Ok(Box::new(Frontend(creation.entity)))
}

struct Frontend(Entity);

impl Panel for Frontend {
    fn title(&self, _: &World) -> String {
        "Debug".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let target = selected_target(world);
        if target.remote || !cfg!(any(target_os = "macos", target_os = "linux")) {
            ui.text_wrapped("Debugging is available for local macOS and Linux projects.");
            return;
        }
        if target.directory.is_empty() {
            ui.text_disabled("Open a local project to configure debugging");
            return;
        }
        let actions = world
            .non_send_mut::<DebugState>()
            .0
            .draw(ui, Path::new(&target.directory));
        let owner = bedtime_api::workbench::panel_owner(world, self.0)
            .expect("Debug frontend belongs to a live panel");
        let panel = world
            .get::<bedtime_api::workbench::PanelPresentation>(owner)
            .unwrap();
        let area = world
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .tiling
            .area_for(panel.id)
            .expect("Debug panel belongs to a live area");
        requests.extend(actions.into_iter().map(|action| {
            WorkbenchAction::Invoke(CommandInvocation {
                command: ACTION_COMMAND.into(),
                source: Some(self.0),
                area,
                arguments: serde_json::to_value(action).expect("Debug actions serialize"),
            })
        }));
    }
}

fn document_events(
    mut events: MessageReader<DocumentEvent>,
    mut debugger: NonSendMut<DebugState>,
    documents: Option<NonSend<DocumentSession>>,
) {
    let events: Vec<_> = events.read().map(|event| event.0.clone()).collect();
    if let Some(documents) = documents
        && !events.is_empty()
    {
        let debugger = &mut debugger.0;
        if let Err(error) = debugger.debug_document_events(&events, &documents) {
            debugger.error = Some(error.to_string());
        }
    }
}

fn save_workspace(debugger: NonSendMut<DebugState>) -> Value {
    debugger.0.settings()
}

fn restore_workspace(
    In(restore): In<FeatureStateRestore>,
    world: &mut World,
) -> Result<(), String> {
    stop(world);
    let directory = selected_target(world).directory;
    world.resource_mut::<LaunchBarrier>().0 = None;
    let mut state = world.non_send_mut::<DebugState>();
    let debugger = &mut state.0;
    *debugger = Debugger::default();
    debugger.load(restore.state.as_ref());
    if restore.state.is_none() && Path::new(&directory).join("Cargo.toml").is_file() {
        debugger.profiles[0].cargo = Some(CargoLaunch::default());
    }
    Ok(())
}

fn stop(world: &mut World) {
    world.resource_mut::<LaunchBarrier>().0 = None;
    if !world.contains_non_send::<BedTerminal>() {
        return;
    }
    let visible = visible(world);
    let mut state = SystemState::<(NonSendMut<DebugState>, NonSendMut<BedTerminal>)>::new(world);
    let (mut debugger, mut terminal) = state
        .get_mut(world)
        .expect("Debug process services are installed");
    debugger.0.stop_native(
        &mut Terminals {
            terminal: &mut terminal,
            visible,
        },
        &mut Vec::new(),
    );
    if let Some(previous) = world.resource_mut::<RetainedTerminal>().0.take() {
        world
            .resource_mut::<bedtime_terminal::TerminalRetentions>()
            .0
            .remove(&previous);
    }
}

fn shutdown(world: &mut World) {
    if world
        .get_resource::<Messages<bevy::app::AppExit>>()
        .is_some_and(|events| !events.is_empty())
    {
        stop(world);
    }
}

/// Global shortcuts emit captured commands and run no debugger I/O in the UI.
pub fn shortcuts(world: &mut World, ui: &Ui) -> Vec<WorkbenchAction> {
    let actions = world
        .resource::<bedtime_api::workbench::WorkbenchRegistry>()
        .shortcuts()
        .to_vec();
    let input = world
        .get_non_send::<bedtime_api::workbench::ShortcutRuntime>()
        .map(|r| r.input.clone())
        .unwrap_or_else(|| {
            bedtime_keybindings::ShortcutInput::new(bedtime_keybindings::Bindings::new(
                actions,
                &Default::default(),
                bedtime_keybindings::Platform::current(),
            ))
        });
    let source = world
        .resource::<DocumentFrame>()
        .active_view
        .and_then(|view| {
            world
                .query::<(Entity, &PanelBinding)>()
                .iter(world)
                .find(|(_, b)| b.view == Some(view))
                .map(|(entity, _)| entity)
        });
    let layout = world.resource::<bedtime_workbench::WorkbenchLayout>();
    let area = source
        .and_then(|source| bedtime_api::workbench::panel_owner(world, source))
        .and_then(|owner| world.get::<bedtime_api::workbench::PanelPresentation>(owner))
        .and_then(|panel| layout.tiling.area_for(panel.id))
        .or(layout.focused_area)
        .unwrap_or_else(|| layout.tiling.layout.largest_area());
    input
        .bindings
        .actions
        .iter()
        .filter(|action| action.owner == MODULE_ID && input.take(ui, &action.id))
        .map(|action| {
            WorkbenchAction::Invoke(CommandInvocation {
                command: action.id.clone(),
                source,
                area,
                arguments: Value::Null,
            })
        })
        .collect()
}

fn start_or_continue(
    In(mut invocation): In<CommandInvocation>,
    world: &mut World,
) -> CommandResult {
    let debugger = &world.non_send::<DebugState>().0;
    if debugger.active()
        && !debugger
            .session
            .as_ref()
            .is_some_and(|session| session.state == SessionState::Stopped)
    {
        return Ok(Vec::new());
    }
    invocation.command = if debugger.active() {
        CONTINUE_COMMAND
    } else {
        START_COMMAND
    }
    .into();
    command(In(invocation), world)
}

fn source_action(
    In(invocation): In<CommandInvocation>,
    mut debugger: NonSendMut<DebugState>,
    documents: NonSend<DocumentSession>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let document = DocumentId(
        invocation.arguments["document"]
            .as_u64()
            .ok_or("Source action requires a document")?,
    );
    let revision: bedtime_api::documents::Revision =
        serde_json::from_value(invocation.arguments["revision"].clone())
            .map_err(|error| error.to_string())?;
    if documents
        .document_revision(document)
        .map_err(|error| error.to_string())?
        != revision
    {
        return Err("Source changed before applying the debugger action".into());
    }
    let action: bedtime_editor_widgets::SourceDebugAction =
        serde_json::from_value(invocation.arguments["action"].clone())
            .map_err(|error| error.to_string())?;
    match action {
        bedtime_editor_widgets::SourceDebugAction::ToggleBreakpoint { row } => debugger
            .0
            .toggle_debug_breakpoint(&documents, document, row)
            .map_err(|error| error.to_string())?,
    }
    intents.push(invocation.source, HostAction::Invalidate);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f9_shortcut_captures_the_active_presentation_before_focus_changes() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = crate::test_support::TempDir::new();
        let first_path = directory.write("first.rs", b"fn first() {}\n");
        let second_path = directory.write("second.rs", b"fn second() {}\n");
        let mut documents = DocumentSession::new();
        let first = documents.open_file(&first_path).unwrap();
        let second = documents.open_file(&second_path).unwrap();
        let first_view = documents.create_view(first).unwrap();
        let second_view = documents.create_view(second).unwrap();
        let first_key = documents.snapshot(first).unwrap().path;
        let second_key = documents.snapshot(second).unwrap().path;
        let session_id = documents.session_id().0;
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.add_plugins(DebuggerPlugin);
        let mut layout = bedtime_workbench::WorkbenchLayout::default();
        let area = layout.tiling.split_area(1, 0, true, [100, 100]).unwrap();
        layout
            .tiling
            .layout
            .move_border(0, 5000, [0, 10000], 7500, 100);
        layout.tiling.assign(1, area);
        app.world_mut().insert_resource(layout);
        let tab = app
            .world_mut()
            .spawn(bedtime_api::workbench::PanelPresentation::new(1))
            .id();
        let source = app
            .world_mut()
            .spawn((
                bedtime_api::workbench::PanelOwner(tab),
                PanelBinding {
                    document: Some(first),
                    view: Some(first_view),
                    session_id,
                    ..Default::default()
                },
            ))
            .id();
        {
            let mut frame = app.world_mut().resource_mut::<DocumentFrame>();
            frame.active_view = Some(first_view);
            frame.session_id = session_id;
        }
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.io_mut().add_key_event(Key::F9, true);
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [600.0, 400.0],
            1.0 / 60.0,
        ));
        let actions = shortcuts(app.world_mut(), context.frame());
        let _ = context.render_legacy();
        let [WorkbenchAction::Invoke(invocation)] = actions.as_slice() else {
            panic!("F9 must produce exactly its captured command")
        };
        assert_eq!(invocation.source, Some(source));
        assert_eq!(invocation.area, area);
        app.world_mut().resource_mut::<DocumentFrame>().active_view = Some(second_view);
        let handler = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(&invocation.command)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(handler, invocation.clone())
            .unwrap()
            .unwrap();
        let debugger = &app.world().non_send::<DebugState>().0;
        assert!(debugger.source.breakpoints.contains_key(&first_key));
        assert!(!debugger.source.breakpoints.contains_key(&second_key));
    }

    #[test]
    fn registered_breakpoint_command_uses_the_captured_view_and_existing_provider_state() {
        let directory = crate::test_support::TempDir::new();
        let first_path = directory.write("first.rs", b"fn first() {}\n");
        let second_path = directory.write("second.rs", b"fn second() {}\n");
        let mut documents = DocumentSession::new();
        let first = documents.open_file(&first_path).unwrap();
        let second = documents.open_file(&second_path).unwrap();
        let first_key = documents.snapshot(first).unwrap().path;
        let second_key = documents.snapshot(second).unwrap().path;
        let first_view = documents.create_view(first).unwrap();
        let second_view = documents.create_view(second).unwrap();
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.add_plugins(DebuggerPlugin);
        app.world_mut().resource_mut::<DocumentFrame>().active_view = Some(second_view);
        let source = app
            .world_mut()
            .spawn(PanelBinding {
                document: Some(first),
                view: Some(first_view),
                ..Default::default()
            })
            .id();
        let command = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(BREAKPOINT_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                command,
                CommandInvocation {
                    command: BREAKPOINT_COMMAND.into(),
                    source: Some(source),
                    area: 0,
                    arguments: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        let state = &app.world().non_send::<DebugState>().0;
        assert!(state.source.breakpoints.contains_key(&first_key));
        assert!(!state.source.breakpoints.contains_key(&second_key));
    }

    #[test]
    fn debug_launch_waits_for_every_file_and_rejects_failed_or_stale_save_replies() {
        for outcome in ["failure", "missing", "changed", "workspace", "target"] {
            let directory = crate::test_support::TempDir::new();
            let path = directory.write("main.rs", b"fn main() {}\n");
            let mut documents = DocumentSession::new();
            let document = documents.open_file(&path).unwrap();
            let revision = documents.document_revision(document).unwrap();
            let mut app = App::new();
            app.world_mut().insert_non_send(documents);
            app.add_plugins(DebuggerPlugin);
            app.world_mut().resource_mut::<DebugTarget>().directory =
                path.parent().unwrap().to_string_lossy().into();
            let source = app.world_mut().spawn_empty().id();
            let handler = app
                .world()
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(START_COMMAND)
                .unwrap()
                .handler;
            app.world_mut()
                .run_system_with(
                    handler,
                    CommandInvocation {
                        command: START_COMMAND.into(),
                        source: Some(source),
                        area: 7,
                        arguments: Value::Null,
                    },
                )
                .unwrap()
                .unwrap();
            let intents = app.world_mut().resource_mut::<HostEffects>().take();
            let token = intents
                .iter()
                .find_map(|intent| {
                    if let HostAction::SaveDocumentsWithResult {
                        recipient,
                        token,
                        documents,
                    } = &intent.action
                    {
                        assert_eq!(recipient, MODULE_ID);
                        assert_eq!(documents, &[document]);
                        assert_eq!(intent.source, Some(source));
                        assert_eq!(intent.area, Some(7));
                        Some(*token)
                    } else {
                        None
                    }
                })
                .expect("even a clean file must pass the widget edit/save barrier");
            assert!(app.world().non_send::<DebugState>().0.build.is_none());
            app.world_mut().write_message(DocumentReply::Save {
                recipient: "other.feature".into(),
                token,
                result: Err("ignored".into()),
            });
            app.update();
            assert!(app.world().resource::<LaunchBarrier>().0.is_some());
            assert!(app.world().non_send::<DebugState>().0.error.is_none());
            let result = match outcome {
                "failure" => Err("disk is read only".into()),
                "missing" => Ok(Vec::new()),
                "changed" => {
                    app.world_mut()
                        .non_send_mut::<DocumentSession>()
                        .replace_content(document, b"fn main() { changed(); }\n")
                        .unwrap();
                    Ok(vec![bedtime_api::documents::SavedDocument {
                        document,
                        revision,
                    }])
                }
                "workspace" => {
                    let mut replacement = DocumentSession::new();
                    replacement.open_file(&path).unwrap();
                    app.world_mut().insert_non_send(replacement);
                    Ok(vec![bedtime_api::documents::SavedDocument {
                        document,
                        revision,
                    }])
                }
                "target" => {
                    app.world_mut().resource_mut::<DebugTarget>().directory = "/different".into();
                    Ok(vec![bedtime_api::documents::SavedDocument {
                        document,
                        revision,
                    }])
                }
                _ => unreachable!(),
            };
            app.world_mut().write_message(DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result,
            });
            app.update();
            assert!(app.world().resource::<LaunchBarrier>().0.is_none());
            let debugger = &app.world().non_send::<DebugState>().0;
            assert!(
                debugger.build.is_none(),
                "{outcome} must not launch the build"
            );
            assert!(
                debugger
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Debug launch stopped")
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"fn main() {}\n");
        }
    }

    #[test]
    fn local_launch_uses_the_directory_without_a_workspace_and_remote_launch_stays_inert() {
        for remote in [false, true] {
            let directory = crate::test_support::TempDir::new();
            let path = directory.write("main.rs", b"saved\n");
            let mut documents = DocumentSession::new();
            let document = documents.open_file(&path).unwrap();
            documents.replace_content(document, b"unsaved\n").unwrap();
            let mut app = App::new();
            app.world_mut().insert_non_send(documents);
            app.add_plugins(DebuggerPlugin);
            app.insert_resource(bedtime_api::files::FileContext::local(directory.root()).unwrap());
            if remote {
                let mut target = app.world_mut().resource_mut::<DebugTarget>();
                target.remote = true;
                target.directory = path.parent().unwrap().to_string_lossy().into();
            }
            let handler = app
                .world()
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(START_COMMAND)
                .unwrap()
                .handler;
            let result = app
                .world_mut()
                .run_system_with(
                    handler,
                    CommandInvocation {
                        command: START_COMMAND.into(),
                        source: None,
                        area: 0,
                        arguments: Value::Null,
                    },
                )
                .unwrap();
            let effects = app.world_mut().resource_mut::<HostEffects>().take();
            if remote {
                assert!(result.is_err());
                assert!(app.world().resource::<LaunchBarrier>().0.is_none());
                assert!(effects.is_empty());
            } else {
                assert!(result.is_ok());
                let pending = app.world().resource::<LaunchBarrier>().0.as_ref().unwrap();
                assert_eq!(
                    pending.target.directory,
                    app.world()
                        .resource::<bedtime_api::files::FileContext>()
                        .directory_string()
                );
                assert!(effects.iter().any(|effect| matches!(&effect.action, HostAction::SaveDocumentsWithResult { documents, .. } if documents == &[document])));
            }
            assert!(
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .dirty
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"saved\n");
        }
    }

    #[test]
    fn stop_cancels_a_waiting_launch_before_its_save_reply_arrives() {
        struct Dialogs;

        impl FileDialogService for Dialogs {
            fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<std::path::PathBuf> {
                None
            }
        }
        let directory = crate::test_support::TempDir::new();
        let path = directory.write("main.rs", b"fn main() {}\n");
        let mut documents = DocumentSession::new();
        documents.open_file(&path).unwrap();
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.world_mut().insert_non_send(BedTerminal::new_empty());
        app.world_mut()
            .insert_non_send(Box::new(Dialogs) as Box<dyn FileDialogService>);
        app.add_plugins(DebuggerPlugin);
        app.world_mut().resource_mut::<DebugTarget>().directory =
            path.parent().unwrap().to_string_lossy().into();
        for command in [START_COMMAND, STOP_COMMAND] {
            let handler = app
                .world()
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(command)
                .unwrap()
                .handler;
            app.world_mut()
                .run_system_with(
                    handler,
                    CommandInvocation {
                        command: command.into(),
                        source: None,
                        area: 0,
                        arguments: Value::Null,
                    },
                )
                .unwrap()
                .unwrap();
        }
        let token = app
            .world_mut()
            .resource_mut::<HostEffects>()
            .take()
            .iter()
            .find_map(|intent| {
                if let HostAction::SaveDocumentsWithResult { token, .. } = intent.action {
                    Some(token)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(app.world().resource::<LaunchBarrier>().0.is_none());
        app.world_mut().write_message(DocumentReply::Save {
            recipient: MODULE_ID.into(),
            token,
            result: Err("late failure".into()),
        });
        app.update();
        let debugger = &app.world().non_send::<DebugState>().0;
        assert!(debugger.build.is_none());
        assert!(debugger.error.is_none());
    }

    #[test]
    fn registered_ui_actions_update_world_state_and_reject_unknown_payloads() {
        struct Dialogs;

        impl FileDialogService for Dialogs {
            fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<std::path::PathBuf> {
                None
            }
        }
        let mut app = App::new();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.world_mut().insert_non_send(BedTerminal::new_empty());
        app.world_mut()
            .insert_non_send(Box::new(Dialogs) as Box<dyn FileDialogService>);
        app.add_plugins(DebuggerPlugin);
        let command = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .command(ACTION_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                command,
                CommandInvocation {
                    command: ACTION_COMMAND.into(),
                    source: None,
                    area: 0,
                    arguments: serde_json::to_value(DebuggerAction::Watch("value.field".into()))
                        .unwrap(),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            app.world().non_send::<DebugState>().0.watches,
            ["value.field"]
        );
        assert!(
            app.world_mut()
                .run_system_with(
                    command,
                    CommandInvocation {
                        command: ACTION_COMMAND.into(),
                        source: None,
                        area: 0,
                        arguments: serde_json::json!({"Evaluate":["value","execute"]})
                    }
                )
                .unwrap()
                .is_err()
        );
        assert_eq!(
            app.world().non_send::<DebugState>().0.watches,
            ["value.field"]
        );
    }
}
