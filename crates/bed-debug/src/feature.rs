//! Debug sessions keep their registered source-provider allocation in the world.
use super::*;
use bed_editor_api::{
    EditorDocumentEvent, EditorFeatureReply, EditorFeatureSystems, EditorFrame, EditorIntents,
    EditorPanelBinding, EditorWorkspaceContribution, EditorWorkspaceRestore, FileDialogService,
};
use bed_terminal_core::{
    bed_terminal_core::BedTerminal,
    terminal_pty::{PtyOptions, TerminalShell},
};
use bed_workbench_api::{
    Action as WorkbenchAction, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
    register,
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
    action: Action,
    token: bed_editor_api::SaveToken,
    documents: Vec<DocumentId>,
    workspace: bed_document_session::editor_session::WorkspaceId,
    result: Option<Result<Vec<bed_editor_api::SavedDocument>, String>>,
}
#[derive(Resource, Default)]
struct LaunchBarrier(Option<PendingLaunch>);

const ACTION_COMMAND: &str = "bed.debug.action";
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
pub struct DebugPlugin;
impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .init_non_send::<bed_document_session::EditorSession>();
        app.configure_sets(
            PreUpdate,
            bed_terminal_core::TerminalCoreSystems::Poll.before(EditorFeatureSystems::Prepare),
        );
        if !app.is_plugin_added::<bed_terminal_core::TerminalCorePlugin>() {
            app.add_plugins(bed_terminal_core::TerminalCorePlugin);
        }
        app.world_mut().init_non_send::<DebugState>();
        app.init_non_send::<bed_editor_ui::extensions::EditorExtensions>()
            .init_non_send::<bed_editor_ui::source_intents::SourceRequests>()
            .init_resource::<bed_editor_api::CommandPresentations>()
            .init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<bed_editor_api::Registry>()
            .init_resource::<bed_terminal_core::TerminalRetentions>()
            .init_resource::<RetainedTerminal>()
            .init_resource::<LaunchBarrier>()
            .add_message::<EditorFeatureReply>()
            .add_message::<EditorDocumentEvent>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                for &(id, label) in COMMANDS {
                    registrar.command(id, label, (id == SHOW_COMMAND).then_some("debug"));
                }
                registrar.menu(MenuSlot::Application, SHOW_COMMAND);
                registrar.toolbar(SHOW_COMMAND);
                registrar.panel_options(
                    PANEL_ID,
                    "Debug",
                    true,
                    PanelPlacement::Bottom,
                    Some("debug"),
                );
            })
            .expect("Debug metadata is registered once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Debug", create_panel);
            for &(id, label) in COMMANDS {
                registrar.command(id, label, command);
            }
            registrar.command(
                "bed.debug.start_or_continue",
                "Start or Continue Debugging",
                start_or_continue,
            );
            for (id, keys) in [
                (SHOW_COMMAND, &["Primary+Shift+D"][..]),
                ("bed.debug.start_or_continue", &["F5"][..]),
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
                "bed.debug.source-action",
                "Apply debugger source action",
                source_action,
            );
        })
        .expect("DebugPlugin is installed once");
        status::install(app);
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(EditorWorkspaceContribution {
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
                .in_set(EditorFeatureSystems::Prepare),
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
        .query::<&bed_terminal_core::TerminalAttachment>()
        .iter(world)
        .map(|panel| panel.session)
        .collect()
}
fn publish(
    world: &mut World,
    source: Option<Entity>,
    area: Option<u32>,
    requests: Vec<HostRequest>,
) {
    for request in requests {
        let mut intents = world.resource_mut::<EditorIntents>();
        // Source previews use the host's editor-area placement, even when the
        // user navigates from a smaller Debug panel.
        if let Some(area) = area.filter(|_| !matches!(&request, HostRequest::PreviewSource { .. }))
        {
            intents.push_at(source, area, request);
        } else {
            intents.push(source, request);
        }
    }
    let terminal = world.non_send::<DebugState>().0.terminal;
    let previous = std::mem::replace(&mut world.resource_mut::<RetainedTerminal>().0, terminal);
    let mut retention = world.resource_mut::<bed_terminal_core::TerminalRetentions>();
    if let Some(previous) = previous {
        retention.0.remove(&previous);
    }
    if let Some(id) = terminal {
        retention.0.insert(id);
    }
}

fn prepare(world: &mut World) {
    if !world.contains_non_send::<EditorSession>() || !world.contains_non_send::<BedTerminal>() {
        return;
    }
    let visible = visible(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<DebugState>,
        NonSendMut<EditorSession>,
        NonSendMut<BedTerminal>,
        Res<EditorFrame>,
        NonSendMut<bed_editor_ui::extensions::EditorExtensions>,
        NonSendMut<bed_editor_ui::source_intents::SourceRequests>,
        ResMut<bed_editor_api::CommandPresentations>,
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
        &frame.project_root,
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
            bed_editor_api::CommandPresentation {
                enabled,
                visible: !frame.remote,
                label: None,
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
                .get::<EditorPanelBinding>(source)
                .and_then(|binding| binding.view)
        });
        if let Some(view) = view {
            let mut state =
                SystemState::<(NonSendMut<DebugState>, NonSend<EditorSession>)>::new(world);
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
                .resource_mut::<EditorIntents>()
                .push(invocation.source, HostRequest::Invalidate);
        }
        return Ok(Vec::new());
    }
    let action = match invocation.command.as_str() {
        SHOW_COMMAND => None,
        START_COMMAND => Some(Action::Start),
        STOP_COMMAND => Some(Action::Stop),
        RESTART_COMMAND => Some(Action::Restart),
        CONTINUE_COMMAND => Some(Action::Continue),
        PAUSE_COMMAND => Some(Action::Pause),
        OVER_COMMAND => Some(Action::Over),
        INTO_COMMAND => Some(Action::Into),
        OUT_COMMAND => Some(Action::Out),
        ACTION_COMMAND => Some(
            serde_json::from_value(invocation.arguments.clone())
                .map_err(|error| error.to_string())?,
        ),
        _ => return Err("Unknown debug command".into()),
    };
    if matches!(action, Some(Action::Stop)) {
        world.resource_mut::<LaunchBarrier>().0 = None;
    }
    if matches!(action, Some(Action::Start | Action::Restart)) {
        if world.resource::<LaunchBarrier>().0.is_some() {
            return Err("Debug launch is already waiting for documents to save".into());
        }
        let documents = world.non_send::<EditorSession>();
        let frame = world.resource::<EditorFrame>().clone();
        if frame.remote
            || documents.is_remote()
            || !cfg!(any(target_os = "macos", target_os = "linux"))
        {
            return Err("Debugging is available for local macOS and Linux projects".into());
        }
        if frame.project_root.is_empty() {
            return Err("Open a local project before debugging".into());
        }
        let workspace = documents.workspace_id();
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
            let token = bed_editor_api::SaveToken::next();
            world.resource_mut::<EditorIntents>().push_at(
                invocation.source,
                invocation.area,
                show_panel(),
            );
            world.resource_mut::<EditorIntents>().push_at(
                invocation.source,
                invocation.area,
                HostRequest::SaveDocumentsWithResult {
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
                workspace,
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
    action: Option<Action>,
) -> CommandResult {
    let visible = visible(world);
    let mut requests = Vec::new();
    let mut state = SystemState::<(
        NonSendMut<DebugState>,
        NonSendMut<EditorSession>,
        NonSendMut<BedTerminal>,
        Option<NonSendMut<Box<dyn FileDialogService>>>,
        Res<EditorFrame>,
    )>::new(world);
    let (mut debugger, mut documents, mut terminal, mut dialogs, frame) = state
        .get_mut(world)
        .expect("Debug command services are installed");
    let debugger = &mut debugger.0;
    if invocation.command == SHOW_COMMAND || matches!(action, Some(Action::Start)) {
        requests.push(show_panel());
        if debugger.profiles.is_empty() {
            debugger.load(None);
        }
        if debugger.cargo_workspace.is_none()
            && debugger.discovery.is_none()
            && debugger.profiles[debugger.selected].cargo.is_some()
            && let Err(error) = debugger.action_native(
                Action::Discover,
                &mut documents,
                &mut Terminals {
                    terminal: &mut terminal,
                    visible: visible.clone(),
                },
                dialogs
                    .as_mut()
                    .map(|dialogs| &mut ***dialogs as &mut dyn FileDialogService),
                &frame.project_root,
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
            &frame.project_root,
            &mut requests,
        )
    {
        debugger.error = Some(error.to_string());
    }
    publish(world, invocation.source, Some(invocation.area), requests);
    Ok(Vec::new())
}
fn launch_replies(
    mut replies: MessageReader<EditorFeatureReply>,
    mut barrier: ResMut<LaunchBarrier>,
) {
    for reply in replies.read() {
        if let EditorFeatureReply::Save {
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
        let documents = world.non_send::<EditorSession>();
        if documents.workspace_id() != pending.workspace {
            return Err("The workspace changed while preparing the debug launch".into());
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
            .resource_mut::<EditorIntents>()
            .push(None, HostRequest::Invalidate);
    }
}
fn publish_readiness(world: &mut World) {
    let debugger = &world.non_send::<DebugState>().0;
    let readiness = match &debugger.error {
        Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.clone()),
        None if debugger.build.is_some()
            || world.resource::<LaunchBarrier>().0.is_some()
            || debugger.session.as_ref().is_some_and(|session| {
                !matches!(
                    session.state,
                    SessionState::Running | SessionState::Stopped | SessionState::Terminated
                )
            }) =>
        {
            bed_editor_api::EditorPanelReadiness::Pending
        }
        None => bed_editor_api::EditorPanelReadiness::Ready,
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
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
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
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let frame = world.resource::<EditorFrame>().clone();
        if frame.remote || !cfg!(any(target_os = "macos", target_os = "linux")) {
            ui.text_wrapped("Debugging is available for local macOS and Linux projects.");
            return;
        }
        if frame.project_root.is_empty() {
            ui.text_disabled("Open a local project to configure debugging");
            return;
        }
        let actions = world
            .non_send_mut::<DebugState>()
            .0
            .draw(ui, Path::new(&frame.project_root));
        let owner = bed_workbench_api::panel_owner(world, self.0)
            .expect("Debug frontend belongs to a live panel");
        let panel = world
            .get::<bed_workbench_api::PanelPresentation>(owner)
            .unwrap();
        let area = world
            .resource::<bed_workbench::WorkbenchLayout>()
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
    mut events: MessageReader<EditorDocumentEvent>,
    mut debugger: NonSendMut<DebugState>,
    documents: Option<NonSend<EditorSession>>,
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
    In(restore): In<EditorWorkspaceRestore>,
    world: &mut World,
) -> Result<(), String> {
    stop(world);
    world.resource_mut::<LaunchBarrier>().0 = None;
    let mut state = world.non_send_mut::<DebugState>();
    let debugger = &mut state.0;
    *debugger = Debugger::default();
    debugger.load(restore.state.as_ref());
    if restore.state.is_none() && Path::new(&restore.root).join("Cargo.toml").is_file() {
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
            .resource_mut::<bed_terminal_core::TerminalRetentions>()
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
        .resource::<bed_workbench_api::Registry>()
        .shortcuts()
        .to_vec();
    let input = world
        .get_non_send::<bed_workbench_api::ShortcutRuntime>()
        .map(|r| r.input.clone())
        .unwrap_or_else(|| {
            bed_keybindings::ShortcutInput::new(bed_keybindings::Bindings::new(
                actions,
                &Default::default(),
                bed_keybindings::Platform::current(),
            ))
        });
    let source = world
        .resource::<EditorFrame>()
        .active_view
        .and_then(|view| {
            world
                .query::<(Entity, &EditorPanelBinding)>()
                .iter(world)
                .find(|(_, b)| b.view == Some(view))
                .map(|(entity, _)| entity)
        });
    let layout = world.resource::<bed_workbench::WorkbenchLayout>();
    let area = source
        .and_then(|source| bed_workbench_api::panel_owner(world, source))
        .and_then(|owner| world.get::<bed_workbench_api::PanelPresentation>(owner))
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
    documents: NonSend<EditorSession>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    let document = DocumentId(
        invocation.arguments["document"]
            .as_u64()
            .ok_or("Source action requires a document")?,
    );
    let revision: bed_editor_api::Revision =
        serde_json::from_value(invocation.arguments["revision"].clone())
            .map_err(|error| error.to_string())?;
    if documents
        .document_revision(document)
        .map_err(|error| error.to_string())?
        != revision
    {
        return Err("Source changed before applying the debugger action".into());
    }
    let action: bed_editor_ui::SourceDebugAction =
        serde_json::from_value(invocation.arguments["action"].clone())
            .map_err(|error| error.to_string())?;
    match action {
        bed_editor_ui::SourceDebugAction::ToggleBreakpoint { row } => debugger
            .0
            .toggle_debug_breakpoint(&documents, document, row)
            .map_err(|error| error.to_string())?,
    }
    intents.push(invocation.source, HostRequest::Invalidate);
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
        let mut documents = EditorSession::new();
        let first = documents.open_file(&first_path).unwrap();
        let second = documents.open_file(&second_path).unwrap();
        let first_view = documents.create_view(first).unwrap();
        let second_view = documents.create_view(second).unwrap();
        let first_key = documents.snapshot(first).unwrap().path;
        let second_key = documents.snapshot(second).unwrap().path;
        let workspace = documents.workspace_id().0;
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.add_plugins(DebugPlugin);
        let mut layout = bed_workbench::WorkbenchLayout::default();
        let area = layout.tiling.split_area(1, 0, true, [100, 100]).unwrap();
        layout
            .tiling
            .layout
            .move_border(0, 5000, [0, 10000], 7500, 100);
        layout.tiling.assign(1, area);
        app.world_mut().insert_resource(layout);
        let tab = app
            .world_mut()
            .spawn(bed_workbench_api::PanelPresentation::new(1))
            .id();
        let source = app
            .world_mut()
            .spawn((
                bed_workbench_api::PanelOwner(tab),
                EditorPanelBinding {
                    document: Some(first),
                    view: Some(first_view),
                    workspace,
                    ..Default::default()
                },
            ))
            .id();
        {
            let mut frame = app.world_mut().resource_mut::<EditorFrame>();
            frame.active_view = Some(first_view);
            frame.workspace = workspace;
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
        app.world_mut().resource_mut::<EditorFrame>().active_view = Some(second_view);
        let handler = app
            .world()
            .resource::<bed_workbench_api::Registry>()
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
        let mut documents = EditorSession::new();
        let first = documents.open_file(&first_path).unwrap();
        let second = documents.open_file(&second_path).unwrap();
        let first_key = documents.snapshot(first).unwrap().path;
        let second_key = documents.snapshot(second).unwrap().path;
        let first_view = documents.create_view(first).unwrap();
        let second_view = documents.create_view(second).unwrap();
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.add_plugins(DebugPlugin);
        app.world_mut().resource_mut::<EditorFrame>().active_view = Some(second_view);
        let source = app
            .world_mut()
            .spawn(EditorPanelBinding {
                document: Some(first),
                view: Some(first_view),
                ..Default::default()
            })
            .id();
        let command = app
            .world()
            .resource::<bed_workbench_api::Registry>()
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
        for outcome in ["failure", "missing", "changed", "workspace"] {
            let directory = crate::test_support::TempDir::new();
            let path = directory.write("main.rs", b"fn main() {}\n");
            let mut documents = EditorSession::new();
            let document = documents.open_file(&path).unwrap();
            let revision = documents.document_revision(document).unwrap();
            let mut app = App::new();
            app.world_mut().insert_non_send(documents);
            app.add_plugins(DebugPlugin);
            app.world_mut().resource_mut::<EditorFrame>().project_root =
                path.parent().unwrap().to_string_lossy().into();
            let source = app.world_mut().spawn_empty().id();
            let handler = app
                .world()
                .resource::<bed_workbench_api::Registry>()
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
            let intents = app.world_mut().resource_mut::<EditorIntents>().take();
            let token = intents
                .iter()
                .find_map(|intent| {
                    if let HostRequest::SaveDocumentsWithResult {
                        recipient,
                        token,
                        documents,
                    } = &intent.request
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
            app.world_mut().write_message(EditorFeatureReply::Save {
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
                        .non_send_mut::<EditorSession>()
                        .replace_content(document, b"fn main() { changed(); }\n")
                        .unwrap();
                    Ok(vec![bed_editor_api::SavedDocument { document, revision }])
                }
                "workspace" => {
                    let mut replacement = EditorSession::new();
                    replacement.open_file(&path).unwrap();
                    app.world_mut().insert_non_send(replacement);
                    Ok(vec![bed_editor_api::SavedDocument { document, revision }])
                }
                _ => unreachable!(),
            };
            app.world_mut().write_message(EditorFeatureReply::Save {
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
    fn invalid_launch_does_not_request_saves_or_create_pending_work() {
        for remote in [false, true] {
            let directory = crate::test_support::TempDir::new();
            let path = directory.write("main.rs", b"saved\n");
            let mut documents = EditorSession::new();
            let document = documents.open_file(&path).unwrap();
            documents.replace_content(document, b"unsaved\n").unwrap();
            let mut app = App::new();
            app.world_mut().insert_non_send(documents);
            app.add_plugins(DebugPlugin);
            if remote {
                let mut frame = app.world_mut().resource_mut::<EditorFrame>();
                frame.remote = true;
                frame.project_root = path.parent().unwrap().to_string_lossy().into();
            }
            let handler = app
                .world()
                .resource::<bed_workbench_api::Registry>()
                .command(START_COMMAND)
                .unwrap()
                .handler;
            assert!(
                app.world_mut()
                    .run_system_with(
                        handler,
                        CommandInvocation {
                            command: START_COMMAND.into(),
                            source: None,
                            area: 0,
                            arguments: Value::Null,
                        }
                    )
                    .unwrap()
                    .is_err()
            );
            assert!(app.world().resource::<LaunchBarrier>().0.is_none());
            assert!(
                app.world_mut()
                    .resource_mut::<EditorIntents>()
                    .take()
                    .is_empty()
            );
            assert!(
                app.world()
                    .non_send::<EditorSession>()
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
        let mut documents = EditorSession::new();
        documents.open_file(&path).unwrap();
        let mut app = App::new();
        app.world_mut().insert_non_send(documents);
        app.world_mut().insert_non_send(BedTerminal::new_empty());
        app.world_mut()
            .insert_non_send(Box::new(Dialogs) as Box<dyn FileDialogService>);
        app.add_plugins(DebugPlugin);
        app.world_mut().resource_mut::<EditorFrame>().project_root =
            path.parent().unwrap().to_string_lossy().into();
        for command in [START_COMMAND, STOP_COMMAND] {
            let handler = app
                .world()
                .resource::<bed_workbench_api::Registry>()
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
            .resource_mut::<EditorIntents>()
            .take()
            .iter()
            .find_map(|intent| {
                if let HostRequest::SaveDocumentsWithResult { token, .. } = intent.request {
                    Some(token)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(app.world().resource::<LaunchBarrier>().0.is_none());
        app.world_mut().write_message(EditorFeatureReply::Save {
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
        app.world_mut().insert_non_send(EditorSession::new());
        app.world_mut().insert_non_send(BedTerminal::new_empty());
        app.world_mut()
            .insert_non_send(Box::new(Dialogs) as Box<dyn FileDialogService>);
        app.add_plugins(DebugPlugin);
        let command = app
            .world()
            .resource::<bed_workbench_api::Registry>()
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
                    arguments: serde_json::to_value(Action::Watch("value.field".into())).unwrap(),
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
