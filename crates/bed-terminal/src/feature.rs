//! Shell sessions belong to BedTerminal; panel entities attach to those sessions.
use super::{MODULE_ID, NEW_COMMAND, PANEL_ID, shell_directory};
use bed_editor_api::{EditorPanelBinding, EditorPanelClose, EditorPanelFocus, PanelPlacement};
use bed_terminal_core::{
    bed_terminal_core::{BedTerminal, TerminalSessionState},
    terminal_font::TerminalFonts,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelPersistence, PanelResult,
    StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText, register,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Window observations used for fresh shells, never the focused process's cwd.
#[derive(Resource, Default, Clone)]
pub struct TerminalWorkspace {
    pub working_directory: String,
    pub project_root: String,
    pub remote: bool,
}

/// Runtime colors; the host decides how appearance preferences are stored.
#[derive(Resource, Clone, PartialEq)]
pub struct TerminalAppearance {
    pub background: [f32; 4],
    pub foreground: [f32; 4],
    pub ansi: [[f32; 4]; 16],
}
impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            background: [0.08, 0.08, 0.08, 1.0],
            foreground: [0.9, 0.9, 0.9, 1.0],
            ansi: [
                [0.0, 0.0, 0.0, 1.0],
                [0.8, 0.0, 0.0, 1.0],
                [0.0, 0.8, 0.0, 1.0],
                [0.8, 0.8, 0.0, 1.0],
                [0.0, 0.0, 0.8, 1.0],
                [0.8, 0.0, 0.8, 1.0],
                [0.0, 0.8, 0.8, 1.0],
                [0.75, 0.75, 0.75, 1.0],
                [0.5, 0.5, 0.5, 1.0],
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [1.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0],
                [1.0, 0.0, 1.0, 1.0],
                [0.0, 1.0, 1.0, 1.0],
                [1.0, 1.0, 1.0, 1.0],
            ],
        }
    }
}

pub use bed_terminal_core::{TerminalAttachment as TerminalPanelState, TerminalRetentions};

#[derive(Resource, Default)]
struct PanelSessions(HashSet<u64>);
#[derive(Resource, Clone, Copy)]
struct Lifecycle {
    focus: bed_editor_api::EditorPanelLifecycleSystem,
    close: bed_editor_api::EditorPanelLifecycleSystem,
    clipboard: bed_editor_api::EditorPanelClipboardSystem,
}

/// The application supplies its configured NonSend BedTerminal and TerminalFonts;
/// the plugin registers shell panel factories and retains no separate process owner.
pub struct TerminalPlugin;
impl Plugin for TerminalPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bed_terminal_core::TerminalCorePlugin>() {
            app.add_plugins(bed_terminal_core::TerminalCorePlugin);
        }
        app.configure_sets(
            PreUpdate,
            bed_terminal_core::TerminalCoreSystems::Poll
                .before(bed_editor_api::EditorFeatureSystems::Prepare),
        );
        app.add_systems(
            PreUpdate,
            (
                update_workspace,
                observations.after(bed_terminal_core::TerminalCoreSystems::Poll),
            )
                .in_set(bed_editor_api::EditorFeatureSystems::Prepare),
        );
        app.init_resource::<TerminalAppearance>()
            .init_resource::<TerminalWorkspace>()
            .init_resource::<TerminalRetentions>()
            .init_resource::<PanelSessions>()
            .init_resource::<bed_editor_api::EditorIntents>()
            .init_resource::<bed_editor_api::Registry>()
            .add_systems(Last, retire_sessions);
        let focus = app
            .world_mut()
            .register_system(|In(entity): In<Entity>, world: &mut World| {
                focus_panel(world, entity);
                Ok(())
            });
        let close = app
            .world_mut()
            .register_system(|In(entity): In<Entity>, world: &mut World| {
                close_panel(world, entity);
                Ok(())
            });
        let clipboard = app.world_mut().register_system(clipboard);
        app.insert_resource(Lifecycle {
            focus,
            close,
            clipboard,
        });
        #[cfg(feature = "desktop")]
        crate::input::install(app);
        #[cfg(unix)]
        crate::shell::install(app);
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    "Terminal",
                    false,
                    PanelPlacement::Bottom,
                    Some("terminal"),
                );
                registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
                registrar.command(NEW_COMMAND, "New Terminal", Some("terminal"));
                registrar.toolbar(NEW_COMMAND);
            })
            .expect("terminal editor contributions are unique");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Terminal", create_panel);
            registrar.command(NEW_COMMAND, "New Terminal", open_panel);
            registrar.command("bed.terminal.show", "Show Terminal", show_panel);
            registrar.global_shortcut("bed.terminal.show", &["Ctrl+Grave"]);
            registrar.global_shortcut(NEW_COMMAND, &["Ctrl+Shift+Grave"]);
            #[cfg(unix)]
            registrar.command(
                crate::shell::COMMAND,
                "Shell Request",
                crate::shell::apply_request,
            );
        })
        .expect("TerminalPlugin must be installed once");
    }
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn update_workspace(world: &mut World) {
    let Some(frame) = world.get_resource::<bed_editor_api::EditorFrame>() else {
        return;
    };
    let documents = world.get_non_send::<bed_document_session::EditorSession>();
    let project_root = documents.map_or_else(
        || frame.project_root.clone(),
        |documents| {
            documents
                .options()
                .project_root
                .as_ref()
                .map(|root| root.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
    );
    let working_directory = if project_root.is_empty() {
        frame.working_directory.clone()
    } else {
        project_root.clone()
    };
    let remote = documents.map_or(frame.remote, |documents| documents.is_remote());
    let ssh = documents
        .and_then(|documents| documents.ssh_target())
        .cloned();
    *world.resource_mut::<TerminalWorkspace>() = TerminalWorkspace {
        working_directory,
        project_root: project_root.clone(),
        remote,
    };
    let mut terminal = world.non_send_mut::<BedTerminal>();
    terminal.set_project_root(&project_root);
    terminal.set_ssh_target(ssh);
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Shell", "Type commands into the terminal. Drag to select text; copy and paste use Bed’s configured shortcuts."),
        ("Tabs", "Open additional terminals from the panel picker. Each terminal has its own shell session."),
    ]).shortcut_actions(&["bed.edit.copy", "bed.edit.paste", "bed.edit.select_all"]));
    if world
        .get::<EditorPanelBinding>(creation.entity)
        .is_some_and(|binding| {
            binding.input.document().or(binding.document).is_some()
                || binding.input.local_file().is_some()
        })
    {
        return Err("Terminal requires its own panel without a document".into());
    }
    // Factories may run during a workspace transition before its next frame.
    update_workspace(world);
    let workspace = world.resource::<TerminalWorkspace>().clone();
    let mut binding = world
        .get::<EditorPanelBinding>(creation.entity)
        .cloned()
        .unwrap_or_default();
    let mut terminal = world
        .get_non_send_mut::<BedTerminal>()
        .ok_or("Terminal sessions are unavailable")?;
    let session = if let Some(session) = binding
        .terminal
        .or_else(|| creation.state["session"].as_u64())
    {
        if terminal.session_title(session).is_none() {
            return Err("Terminal session is no longer available".into());
        }
        session
    } else {
        let cwd = shell_directory(
            creation.state["cwd"].as_str(),
            &workspace.working_directory,
            workspace.remote,
        );
        match cwd {
            Some(cwd) => terminal.new_session_at(cwd),
            None => terminal.new_session(),
        }
    };
    let command = terminal.is_command_session(session);
    binding.terminal = Some(session);
    let lifecycle = *world.resource::<Lifecycle>();
    let hooks = (
        EditorPanelFocus(lifecycle.focus),
        EditorPanelClose(lifecycle.close),
    );
    world.entity_mut(creation.entity).insert((
        TerminalPanelState { session, command },
        StatusItems(vec![StatusItem {
            id: "bed.terminal.session",
            label: "Terminal",
            priority: 50,
            icon: "terminal",
            prefer_icon: false,
            measure: status_measure,
            draw: status_draw,
            tooltip: status_tooltip,
        }]),
        bed_editor_api::EditorPanelClipboard(lifecycle.clipboard),
        binding,
        PanelPersistence(!command),
        hooks,
    ));
    world.resource_mut::<PanelSessions>().0.insert(session);
    Ok(Box::new(TerminalFrontend {
        entity: creation.entity,
    }))
}

pub struct TerminalFrontend {
    pub entity: Entity,
}
impl Panel for TerminalFrontend {
    fn title(&self, world: &World) -> String {
        let session = world
            .get::<TerminalPanelState>(self.entity)
            .expect("a live terminal frontend has session state")
            .session;
        world
            .get_non_send::<BedTerminal>()
            .and_then(|terminal| terminal.session_title(session))
            .unwrap_or_else(|| format!("Terminal {session}"))
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let session = world
            .get::<TerminalPanelState>(self.entity)
            .expect("a live terminal frontend has session state")
            .session;
        let mut params = bevy::ecs::system::SystemState::<(
            NonSendMut<BedTerminal>,
            NonSend<TerminalFonts>,
        )>::new(world);
        let (mut terminal, fonts) = params
            .get_mut(world)
            .expect("TerminalPlugin installs process and font state");
        if let Err(error) = terminal.render_session(ui, &fonts, session) {
            ui.text_disabled(error.to_string());
        }
    }

    fn save(&self, world: &World) -> Value {
        let state = world
            .get::<TerminalPanelState>(self.entity)
            .expect("a live terminal frontend has session state");
        let cwd = world
            .get_non_send::<BedTerminal>()
            .and_then(|terminal| terminal.working_directory(state.session));
        // A live session id is an attachment request, never persisted process state.
        json!({ "cwd":cwd,"command":state.command })
    }
}

fn terminal_status(world: &World, context: StatusContext) -> Option<StatusText> {
    if !context.is_focused() {
        return None;
    }
    let session = world.get::<TerminalPanelState>(context.publisher)?.session;
    let terminal = world.get_non_send::<BedTerminal>()?;
    let title = terminal.session_title(session)?;
    let state = match terminal.session_state(session)? {
        TerminalSessionState::Pending => "Not started",
        TerminalSessionState::Running => "Running",
        TerminalSessionState::Ended => "Session ended",
    };
    let remote = terminal.ssh_target(session);
    let current = terminal.cached_working_directory(session);
    let directory = current.or_else(|| terminal.working_directory(session));
    let summary = format!("{title} · {state}");
    let mut details = format!("Terminal: {title}\nState: {state}");
    if let Some(remote) = remote {
        details.push_str(&format!("\nSSH: {}", remote.host));
    }
    if let Some(directory) = directory {
        let label = if current.is_some() {
            "Directory"
        } else {
            "Launch directory"
        };
        let path = directory.to_string_lossy();
        details.push_str(&format!("\n{label}: {path}"));
    }
    Some(StatusText::new(summary, details))
}
fn status_measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    terminal_status(world, context).map(|status| status.measure(ui))
}
fn status_draw(world: &mut World, ui: &Ui, context: StatusDrawContext, actions: &mut Vec<Action>) {
    if let Some(status) = terminal_status(world, context.context) {
        status.draw(ui, context, actions);
    }
}
fn status_tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(status) = terminal_status(world, context) {
        status.tooltip(ui);
    }
}

pub fn focus_panel(world: &mut World, entity: Entity) {
    let Some(state) = world.get::<TerminalPanelState>(entity).copied() else {
        return;
    };
    if let Some(mut terminal) = world.get_non_send_mut::<BedTerminal>() {
        terminal.focus_session(state.session);
    }
}

/// Called before a presentation is retired; other panels and debuggers keep
/// their shared session. The Last system also covers plain ECS despawns.
pub fn close_panel(world: &mut World, entity: Entity) {
    let Some(state) = world.get::<TerminalPanelState>(entity).copied() else {
        return;
    };
    if world
        .resource::<TerminalRetentions>()
        .0
        .contains(&state.session)
    {
        return;
    }
    let shared = world
        .query::<(Entity, &TerminalPanelState)>()
        .iter(world)
        .any(|(other, panel)| other != entity && panel.session == state.session);
    if !shared && let Some(mut terminal) = world.get_non_send_mut::<BedTerminal>() {
        terminal.close_session_id(state.session);
    }
}

fn retire_sessions(world: &mut World) {
    if !world.contains_non_send::<BedTerminal>() {
        return;
    }
    let live = world
        .query::<&TerminalPanelState>()
        .iter(world)
        .map(|panel| panel.session)
        .collect::<HashSet<_>>();
    let stale = world
        .resource::<PanelSessions>()
        .0
        .difference(&live)
        .copied()
        .filter(|session| !world.resource::<TerminalRetentions>().0.contains(session))
        .collect::<Vec<_>>();
    for session in stale {
        world
            .non_send_mut::<BedTerminal>()
            .close_session_id(session);
        world.resource_mut::<PanelSessions>().0.remove(&session);
    }
}

fn clipboard(
    In(invocation): In<bed_editor_api::ClipboardInvocation>,
    states: Query<&TerminalPanelState>,
    mut terminal: NonSendMut<BedTerminal>,
) -> bed_editor_api::ClipboardResult {
    let state = states
        .get(invocation.entity)
        .map_err(|_| "Terminal panel is closed")?;
    if !terminal.focus_session(state.session) {
        return Err("Terminal session is closed".into());
    }
    match invocation.action {
        bed_editor_api::ClipboardAction::Copy => Ok(terminal.native_copy()),
        bed_editor_api::ClipboardAction::Cut => Ok(None),
        bed_editor_api::ClipboardAction::Paste(text) => {
            terminal
                .native_paste(text)
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
        bed_editor_api::ClipboardAction::SelectAll => {
            terminal
                .native_select_all()
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
    }
}

fn observations(
    mut activity: ResMut<bed_terminal_core::TerminalActivity>,
    mut intents: ResMut<bed_editor_api::EditorIntents>,
) {
    if activity.changed {
        intents.push(None, bed_editor_api::HostRequest::Invalidate);
    }
    if let Some(message) = activity.error.take() {
        intents.push(None, bed_editor_api::HostRequest::Notify { message });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests};

    #[test]
    fn status_tracks_its_focused_session_and_labels_remote_launch_directory() {
        let mut app = make_app();
        let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
        terminal.set_ssh_target(Some(bed_remote::SshTarget::new("user@status-fixture")));
        let session = terminal.new_session_at("/remote/launch");
        drop(terminal);
        let entity = app
            .world_mut()
            .spawn(TerminalPanelState {
                session,
                command: false,
            })
            .id();
        let mut context = StatusContext {
            publisher: entity,
            item_id: "bed.terminal.session",
            focused_panel: None,
            focused_frontend: None,
            source: Some(entity),
            area: 1,
        };
        assert!(terminal_status(app.world(), context).is_none());
        context.focused_frontend = Some(entity);
        let status = terminal_status(app.world(), context).unwrap();
        assert!(status.summary.contains("Not started"));
        assert!(!status.summary.contains("/remote/launch"));
        assert!(status.details.contains("Launch directory: /remote/launch"));
        assert!(status.details.contains("SSH: user@status-fixture"));
        app.world_mut()
            .non_send_mut::<BedTerminal>()
            .stop_session_id(session);
        assert!(
            terminal_status(app.world(), context)
                .unwrap()
                .summary
                .contains("Session ended")
        );
        app.world_mut()
            .non_send_mut::<BedTerminal>()
            .close_session_id(session);
        assert!(terminal_status(app.world(), context).is_none());
    }

    fn make_app() -> App {
        let mut app = App::new();
        app.world_mut().insert_non_send(BedTerminal::new_empty());
        app.add_plugins((WorkbenchPlugin, TerminalPlugin));
        app
    }
    fn open(app: &mut App, state: Value) -> Entity {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: PANEL_ID.into(),
                state,
                area: None,
            });
        bed_workbench::process_requests(app.world_mut());
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap()
    }

    #[test]
    fn plugin_shells_restore_cwd_independently_and_persist_no_process_identity() {
        let root = crate::test_support::TempDir::new();
        let first_dir = root.path("first");
        let second_dir = root.path("second");
        std::fs::create_dir(&first_dir).unwrap();
        std::fs::create_dir(&second_dir).unwrap();
        let mut app = make_app();
        app.world_mut()
            .resource_mut::<TerminalWorkspace>()
            .working_directory = second_dir.to_string_lossy().into_owned();
        let first = open(&mut app, json!({"cwd":first_dir}));
        let second = open(&mut app, Value::Null);
        let first_session = app
            .world()
            .get::<TerminalPanelState>(first)
            .unwrap()
            .session;
        let second_session = app
            .world()
            .get::<TerminalPanelState>(second)
            .unwrap()
            .session;
        assert_ne!(first_session, second_session);
        let terminal = app.world().non_send::<BedTerminal>();
        assert_eq!(
            terminal.working_directory(first_session),
            Some(first_dir.as_path())
        );
        assert_eq!(
            terminal.working_directory(second_session),
            Some(second_dir.as_path())
        );
        let saved = bed_workbench::save(app.world_mut());
        let panels = saved["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 2);
        assert!(
            panels
                .iter()
                .all(|panel| panel["state"].get("session").is_none())
        );
        let mut restored = make_app();
        bed_workbench::restore(restored.world_mut(), &saved).unwrap();
        let restored_cwds = restored
            .world_mut()
            .query::<&TerminalPanelState>()
            .iter(restored.world())
            .map(|panel| {
                restored
                    .world()
                    .non_send::<BedTerminal>()
                    .working_directory(panel.session)
                    .unwrap()
                    .to_owned()
            })
            .collect::<HashSet<_>>();
        assert_eq!(restored_cwds, HashSet::from([first_dir, second_dir]));
    }

    #[test]
    fn plugin_lifecycle_protects_shared_and_debugger_retained_sessions() {
        let mut app = make_app();
        let first = open(&mut app, Value::Null);
        let session = app
            .world()
            .get::<TerminalPanelState>(first)
            .unwrap()
            .session;
        let second = open(&mut app, json!({"session":session}));
        close_panel(app.world_mut(), first);
        assert!(
            app.world()
                .non_send::<BedTerminal>()
                .session_title(session)
                .is_some()
        );
        app.world_mut().despawn(first);
        app.world_mut()
            .resource_mut::<TerminalRetentions>()
            .0
            .insert(session);
        let close = app.world().get::<EditorPanelClose>(second).unwrap().0;
        app.world_mut()
            .run_system_with(close, second)
            .unwrap()
            .unwrap();
        app.world_mut().despawn(second);
        app.update();
        assert!(
            app.world()
                .non_send::<BedTerminal>()
                .session_title(session)
                .is_some()
        );
        app.world_mut()
            .resource_mut::<TerminalRetentions>()
            .0
            .remove(&session);
        app.update();
        assert!(
            app.world()
                .non_send::<BedTerminal>()
                .session_title(session)
                .is_none()
        );
    }

    #[test]
    fn invalid_live_attachment_fails_without_creating_a_shell() {
        let mut app = make_app();
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: PANEL_ID.into(),
                state: json!({"session":999999}),
                area: None,
            });
        bed_workbench::process_requests(app.world_mut());
        let record = app
            .world_mut()
            .query::<&bed_workbench::PanelRecord>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            record.unavailable.as_deref(),
            Some("Terminal session is no longer available")
        );
        assert!(
            app.world()
                .non_send::<BedTerminal>()
                .session_ids()
                .is_empty()
        );
    }

    #[test]
    fn adopting_inactive_shell_preserves_requested_session_focus() {
        let mut app = make_app();
        let (focused, adopted) = {
            let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
            let focused = terminal.new_session_at("/focused-shell");
            let adopted = terminal.new_session_at("/inactive-shell");
            assert!(terminal.focus_session(focused));
            (focused, adopted)
        };
        let entity = app
            .world_mut()
            .spawn(EditorPanelBinding {
                terminal: Some(adopted),
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let _frontend = app
            .world_mut()
            .run_system_with(
                factory,
                PanelCreation {
                    entity,
                    state: Value::Null,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            app.world().non_send::<BedTerminal>().active_session_id(),
            Some(focused),
            "mounting an inactive presentation must preserve the selected shell"
        );
        let focus = app.world().get::<EditorPanelFocus>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(focus, entity)
            .unwrap()
            .unwrap();
        assert_eq!(
            app.world().non_send::<BedTerminal>().active_session_id(),
            Some(adopted)
        );
    }

    #[test]
    fn legacy_handoff_reuses_the_canonical_session_before_saved_state() {
        let mut app = make_app();
        let session = app
            .world_mut()
            .non_send_mut::<BedTerminal>()
            .new_session_at("/legacy-shell");
        let entity = app
            .world_mut()
            .spawn(EditorPanelBinding {
                terminal: Some(session),
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .panel(PANEL_ID)
            .unwrap()
            .factory;
        let frontend = app
            .world_mut()
            .run_system_with(
                factory,
                PanelCreation {
                    entity,
                    state: json!({"session":999999,"cwd":"/new-shell"}),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            app.world()
                .get::<TerminalPanelState>(entity)
                .unwrap()
                .session,
            session
        );
        assert_eq!(
            app.world()
                .get::<EditorPanelBinding>(entity)
                .unwrap()
                .terminal,
            Some(session)
        );
        assert_eq!(
            app.world().non_send::<BedTerminal>().session_ids(),
            vec![session]
        );
        assert_eq!(frontend.save(app.world())["cwd"], json!("/legacy-shell"));
    }
}

fn show_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::ShowPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
