//! Shell sessions belong to BedTerminal; panel entities attach to those sessions.
use super::{MODULE_ID, NEW_COMMAND, PANEL_ID, shell_directory};
use bedtime_api::{
    documents::PanelBinding,
    workbench::{PanelClose, PanelFocus, PanelPlacement},
};
use bedtime_api::{
    status::{StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText},
    workbench::{
        CommandInvocation, CommandResult, Panel, PanelCreation, PanelPersistence, PanelResult,
        WorkbenchAction, register,
    },
};
use bedtime_terminal::{
    bedtime_terminal::{BedTerminal, TerminalSessionState},
    terminal_font::TerminalFonts,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Window observations used for fresh shells, never the focused process's cwd.
#[derive(Resource, Default, Clone)]
pub struct TerminalLaunchContext {
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

pub use bedtime_terminal::{TerminalAttachment as TerminalPanelState, TerminalRetentions};

#[derive(Resource, Default)]
struct PanelSessions(HashSet<u64>);

#[derive(Resource, Clone, Copy)]
struct Lifecycle {
    focus: bedtime_api::workbench::PanelLifecycleSystem,
    close: bedtime_api::workbench::PanelLifecycleSystem,
    clipboard: bedtime_api::workbench::PanelClipboardSystem,
}

/// Shell panels install their process, fonts, directory, lifecycle and clipboard integration.
pub struct TerminalPlugin;

impl Plugin for TerminalPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_terminal::TerminalCorePlugin>() {
            app.add_plugins(bedtime_terminal::TerminalCorePlugin);
        }
        #[cfg(unix)]
        crate::shell::install(app);
        app.init_resource::<TerminalAppearance>()
            .init_resource::<TerminalLaunchContext>()
            .init_resource::<TerminalRetentions>()
            .init_resource::<PanelSessions>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .add_systems(Last, retire_sessions);
        #[cfg(feature = "desktop")]
        crate::input::install(app);
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(PANEL_ID, false, PanelPlacement::Bottom);
                registrar.panel_open_placement(PANEL_ID, PanelPlacement::Center);
                registrar.command_ref(NEW_COMMAND);
                registrar.toolbar(NEW_COMMAND);
            })
            .expect("terminal editor contributions are unique");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Terminal", create_panel);
            registrar.command(NEW_COMMAND, "New Terminal", open_panel);
            registrar.command("bedtime.terminal.show", "Show Terminal", show_panel);
            registrar.global_shortcut("bedtime.terminal.show", &["Ctrl+Grave"]);
            registrar.global_shortcut(NEW_COMMAND, &["Primary+T", "Ctrl+Shift+Grave"]);
            registrar.panel_icon(PANEL_ID, "terminal");
            registrar.command_icon(NEW_COMMAND, "terminal");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("TerminalPlugin must be installed once");
        install_integration(app);
    }
}

#[derive(Resource)]
struct TerminalIntegrationInstalled;
fn install_integration(app: &mut App) {
    if app
        .world()
        .contains_resource::<TerminalIntegrationInstalled>()
    {
        return;
    }
    app.insert_resource(TerminalIntegrationInstalled);
    app.init_resource::<bedtime_api::features::HostEffects>()
        .configure_sets(
            PreUpdate,
            bedtime_terminal::TerminalCoreSystems::Poll
                .before(bedtime_api::features::FeatureSystems::Prepare),
        )
        .add_systems(
            PreUpdate,
            (
                update_launch_context,
                configure_appearance,
                observations.after(bedtime_terminal::TerminalCoreSystems::Poll),
            )
                .in_set(bedtime_api::features::FeatureSystems::Prepare),
        );
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
}

fn configure_appearance(
    frame: Option<NonSend<bedtime_ui::UiFrameContext>>,
    palette: Option<Res<bedtime_api::presentation::TerminalPalette>>,
    mut appearance: ResMut<TerminalAppearance>,
) {
    if let Some(frame) = frame {
        appearance.background = frame.theme().palette.background;
        appearance.foreground = frame.theme().palette.text;
    }
    if let Some(palette) = palette {
        appearance.ansi = palette.0;
    }
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn update_launch_context(world: &mut World) {
    let Some(files) = world.get_resource::<bedtime_api::files::FileContext>() else {
        return;
    };
    let project_root = files.directory_string();
    let remote = matches!(
        files.provider,
        bedtime_api::files::FileProviderId::Ssh { .. }
    );
    #[cfg(feature = "remote")]
    let ssh = world
        .get_non_send::<bedtime_documents::DocumentSession>()
        .and_then(|documents| documents.ssh_target())
        .cloned();
    *world.resource_mut::<TerminalLaunchContext>() = TerminalLaunchContext {
        working_directory: project_root.clone(),
        project_root: project_root.clone(),
        remote,
    };
    let mut terminal = world.non_send_mut::<BedTerminal>();
    terminal.set_project_root(&project_root);
    #[cfg(feature = "remote")]
    terminal.set_ssh_target(ssh);
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bedtime_api::workbench::PanelDocumentation::new(&[
        ("Shell", "Type commands into the terminal. Drag to select text; copy and paste use the configured shortcuts."),
        ("Tabs", "Open additional terminals from the panel picker. Each terminal has its own shell session."),
    ]).shortcut_actions(&["bedtime.edit.copy", "bedtime.edit.paste", "bedtime.edit.select_all"]));
    let lifecycle = world.get_resource::<Lifecycle>().copied();
    let mut binding = if lifecycle.is_some() {
        let binding = world
            .get::<PanelBinding>(creation.entity)
            .cloned()
            .unwrap_or_default();
        if binding.input.document().or(binding.document).is_some()
            || binding.input.local_file().is_some()
        {
            return Err("Terminal requires its own panel without a document".into());
        }
        // Editor factories may run before the next published frame.
        update_launch_context(world);
        binding
    } else {
        PanelBinding::default()
    };
    let launch = world.resource::<TerminalLaunchContext>().clone();
    if launch.remote && !cfg!(feature = "remote") {
        return Err(
            "Remote terminal support requires the bedtime-plugin-terminal remote feature".into(),
        );
    }
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
            &launch.working_directory,
            launch.remote,
        );
        match cwd {
            Some(cwd) => terminal.new_session_at(cwd),
            None => terminal.new_session(),
        }
    };
    let command = terminal.is_command_session(session);
    binding.terminal = Some(session);
    world.entity_mut(creation.entity).insert((
        TerminalPanelState {
            session_id: session,
            command,
        },
        StatusItems(vec![StatusItem {
            id: "bedtime.terminal.session",
            label: "Terminal",
            priority: 50,
            icon: "terminal",
            prefer_icon: false,
            measure: status_measure,
            draw: status_draw,
            tooltip: status_tooltip,
        }]),
        PanelPersistence(!command),
    ));
    if let Some(lifecycle) = lifecycle {
        world.entity_mut(creation.entity).insert((
            binding,
            bedtime_api::workbench::PanelClipboard(lifecycle.clipboard),
            PanelFocus(lifecycle.focus),
            PanelClose(lifecycle.close),
        ));
    }
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
            .session_id;
        world
            .get_non_send::<BedTerminal>()
            .and_then(|terminal| terminal.session_title(session))
            .unwrap_or_else(|| format!("Terminal {session}"))
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let session = world
            .get::<TerminalPanelState>(self.entity)
            .expect("a live terminal frontend has session state")
            .session_id;
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
            .and_then(|terminal| terminal.working_directory(state.session_id));
        // A live session id is an attachment request, never persisted process state.
        json!({ "cwd":cwd,"command":state.command })
    }
}

fn terminal_status(world: &World, context: StatusContext) -> Option<StatusText> {
    if !context.is_focused() {
        return None;
    }
    let session = world
        .get::<TerminalPanelState>(context.publisher)?
        .session_id;
    let terminal = world.get_non_send::<BedTerminal>()?;
    let title = terminal.session_title(session)?;
    let state = match terminal.session_state(session)? {
        TerminalSessionState::Pending => "Not started",
        TerminalSessionState::Running => "Running",
        TerminalSessionState::Ended => "Session ended",
    };
    let current = terminal.cached_working_directory(session);
    let directory = current.or_else(|| terminal.working_directory(session));
    let summary = format!("{title} · {state}");
    let mut details = format!("Terminal: {title}\nState: {state}");
    #[cfg(feature = "remote")]
    if let Some(remote) = terminal.ssh_target(session) {
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

fn status_draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
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
        terminal.focus_session(state.session_id);
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
        .contains(&state.session_id)
    {
        return;
    }
    let shared = world
        .query::<(Entity, &TerminalPanelState)>()
        .iter(world)
        .any(|(other, panel)| other != entity && panel.session_id == state.session_id);
    if !shared && let Some(mut terminal) = world.get_non_send_mut::<BedTerminal>() {
        terminal.close_session_id(state.session_id);
    }
}

fn retire_sessions(world: &mut World) {
    if !world.contains_non_send::<BedTerminal>() {
        return;
    }
    let live = world
        .query::<&TerminalPanelState>()
        .iter(world)
        .map(|panel| panel.session_id)
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
    In(invocation): In<bedtime_api::workbench::ClipboardInvocation>,
    states: Query<&TerminalPanelState>,
    mut terminal: NonSendMut<BedTerminal>,
    mut files: Option<NonSendMut<Box<dyn bedtime_api::workbench::FileClipboardService>>>,
) -> bedtime_api::workbench::ClipboardResult {
    let state = states
        .get(invocation.entity)
        .map_err(|_| "Terminal panel is closed")?;
    if !terminal.focus_session(state.session_id) {
        return Err("Terminal session is closed".into());
    }
    match invocation.action {
        bedtime_api::workbench::ClipboardAction::Copy => Ok(terminal.native_copy()),
        bedtime_api::workbench::ClipboardAction::Cut => Ok(None),
        bedtime_api::workbench::ClipboardAction::Paste(text) => {
            terminal
                .native_paste(text)
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
        bedtime_api::workbench::ClipboardAction::PasteFromClipboard(text) => {
            let paths = files
                .as_deref_mut()
                .map(|clipboard| clipboard.read_paste_files())
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_default();
            let text = if paths.is_empty() {
                text
            } else {
                Some(file_paste_text(&paths).map_err(|error| error.to_string())?)
            };
            if let Some(text) = text {
                terminal
                    .native_paste(text)
                    .map_err(|error| error.to_string())?;
            }
            Ok(None)
        }
        bedtime_api::workbench::ClipboardAction::SelectAll => {
            terminal
                .native_select_all()
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
    }
}

fn observations(
    mut activity: ResMut<bedtime_terminal::TerminalActivity>,
    mut intents: ResMut<bedtime_api::features::HostEffects>,
) {
    if activity.changed {
        intents.push(None, bedtime_api::features::HostAction::Invalidate);
    }
    if let Some(message) = activity.error.take() {
        intents.push(None, bedtime_api::features::HostAction::Notify { message });
    }
}

fn show_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::ShowPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

/// Preserve each path as one shell argument. Reject control characters because
/// terminal paste normalizes newlines and strips escape bytes.
pub(crate) fn file_paste_text(paths: &[std::path::PathBuf]) -> std::io::Result<String> {
    let mut text = String::new();
    for path in paths {
        let path = path.to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Clipboard file path is not UTF-8",
            )
        })?;
        if path.is_empty() || path.chars().any(char::is_control) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Clipboard file path is empty or contains control characters",
            ));
        }
        if !text.is_empty() {
            text.push(' ');
        }
        for character in path.chars() {
            if !character.is_alphanumeric() && !matches!(character, '/' | '.' | '_' | '-') {
                text.push('\\');
            }
            text.push(character);
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::{WorkbenchPlugin, WorkbenchRequests};

    #[cfg(feature = "remote")]
    #[test]
    fn status_tracks_its_focused_session_and_labels_remote_launch_directory() {
        let mut app = make_app();
        let session = {
            let mut terminal = app.world_mut().non_send_mut::<BedTerminal>();
            terminal.set_ssh_target(Some(bedtime_remote::SshTarget::new("user@status-fixture")));
            terminal.new_session_at("/remote/launch")
        };
        let entity = app
            .world_mut()
            .spawn(TerminalPanelState {
                session_id: session,
                command: false,
            })
            .id();
        let mut context = StatusContext {
            publisher: entity,
            item_id: "bedtime.terminal.session",
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

    #[test]
    fn terminal_plugin_projects_shared_palette_without_an_application_bridge() {
        let mut app = make_app();
        let mut palette = TerminalAppearance::default().ansi;
        palette[1] = [0.1, 0.2, 0.3, 1.0];
        app.insert_resource(bedtime_api::presentation::TerminalPalette(palette));
        app.update();
        assert_eq!(app.world().resource::<TerminalAppearance>().ansi, palette);
        palette[1] = [0.4, 0.5, 0.6, 1.0];
        app.world_mut()
            .resource_mut::<bedtime_api::presentation::TerminalPalette>()
            .0 = palette;
        app.update();
        assert_eq!(app.world().resource::<TerminalAppearance>().ansi, palette);
    }

    #[cfg(unix)]
    #[test]
    fn registered_clipboard_paste_uses_files_only_for_native_requests() {
        use bedtime_terminal::terminal_pty::{PtyOptions, TerminalShell};
        use std::{
            cell::Cell,
            rc::Rc,
            time::{Duration, Instant},
        };
        struct Files {
            paths: Vec<std::path::PathBuf>,
            reads: Rc<Cell<usize>>,
        }
        impl bedtime_api::workbench::FileClipboardService for Files {
            fn read_files(&mut self) -> std::io::Result<Vec<std::path::PathBuf>> {
                self.reads.set(self.reads.get() + 1);
                Ok(self.paths.clone())
            }
            fn write_files(&mut self, _: &[std::path::PathBuf]) -> std::io::Result<()> {
                Ok(())
            }
        }
        fn output(terminal: &BedTerminal) -> String {
            terminal
                .active_terminal()
                .unwrap()
                .lines
                .iter()
                .flat_map(|row| row.cells.iter())
                .map(|cell| cell.text.as_ref())
                .collect()
        }
        let paths = vec![std::path::PathBuf::from("/tmp/pasted file")];
        let expected = format!("EXPLICIT{}", file_paste_text(&paths).unwrap());
        let expected_hex = expected
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let script = format!(
            "stty raw -echo; printf READY; data=$(dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'); test \"$data\" = {expected_hex} && printf PASTE_OK",
            expected.len()
        );
        let mut terminal = BedTerminal::new_empty();
        let (session_id, _) = terminal
            .new_command_session(
                PtyOptions {
                    shell: Some(TerminalShell::new("/bin/sh", vec!["-c".into(), script])),
                    ..Default::default()
                },
                "Registered clipboard fixture",
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !output(&terminal).contains("READY") {
            terminal.poll().unwrap();
            assert!(Instant::now() < deadline, "Child must be ready for paste");
            std::thread::sleep(Duration::from_millis(2));
        }
        let reads = Rc::new(Cell::new(0));
        let mut world = World::new();
        world.insert_non_send(terminal);
        world.insert_non_send(Box::new(Files {
            paths,
            reads: Rc::clone(&reads),
        })
            as Box<dyn bedtime_api::workbench::FileClipboardService>);
        let panel = world
            .spawn(TerminalPanelState {
                session_id,
                command: true,
            })
            .id();
        let handler = world.register_system(clipboard);
        world
            .run_system_with(
                handler,
                bedtime_api::workbench::ClipboardInvocation {
                    entity: panel,
                    action: bedtime_api::workbench::ClipboardAction::Paste("EXPLICIT".into()),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            reads.get(),
            0,
            "Explicit text must not consult the native clipboard"
        );
        world
            .run_system_with(
                handler,
                bedtime_api::workbench::ClipboardInvocation {
                    entity: panel,
                    action: bedtime_api::workbench::ClipboardAction::PasteFromClipboard(None),
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(reads.get(), 1);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            world.non_send_mut::<BedTerminal>().poll().unwrap();
            if output(world.non_send::<BedTerminal>()).contains("PASTE_OK") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Native files and explicit text must reach the child once"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        world.non_send_mut::<BedTerminal>().shutdown();
    }

    #[test]
    fn ordinary_terminal_uses_host_directory_without_document_services() {
        let directory = crate::test_support::TempDir::new();
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, TerminalPlugin))
            .insert_resource(TerminalLaunchContext {
                working_directory: directory.root().to_string_lossy().into_owned(),
                ..Default::default()
            });
        let panel = open(&mut app, Value::Null);
        assert_eq!(
            app.world().get::<PanelBinding>(panel).unwrap().document,
            None
        );
        assert!(app.world().get::<PanelFocus>(panel).is_some());
        assert!(
            app.world()
                .contains_resource::<bedtime_api::features::HostEffects>()
        );
        #[cfg(unix)]
        assert!(app.world().contains_resource::<crate::shell::ShellBridge>());
        let session = app
            .world()
            .get::<TerminalPanelState>(panel)
            .unwrap()
            .session_id;
        assert_eq!(
            app.world()
                .non_send::<BedTerminal>()
                .working_directory(session),
            Some(directory.root())
        );
        app.world_mut()
            .non_send_mut::<BedTerminal>()
            .close_session_id(session);
    }

    #[cfg(not(feature = "remote"))]
    #[test]
    fn a_remote_workspace_without_transport_cannot_start_a_local_shell() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, TerminalPlugin))
            .insert_resource(TerminalLaunchContext {
                remote: true,
                working_directory: "/remote/project".into(),
                ..Default::default()
            });
        let panel = open(&mut app, Value::Null);
        assert!(app.world().get::<TerminalPanelState>(panel).is_none());
        assert!(
            app.world()
                .non_send::<BedTerminal>()
                .session_ids()
                .is_empty()
        );
        assert!(
            app.world()
                .get::<bedtime_workbench::PanelRecord>(panel)
                .unwrap()
                .unavailable
                .as_deref()
                .unwrap()
                .contains("remote feature")
        );
    }

    fn open(app: &mut App, state: Value) -> Entity {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
                panel_type: PANEL_ID.into(),
                state,
                area: None,
            });
        bedtime_workbench::process_requests(app.world_mut());
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
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
            .resource_mut::<TerminalLaunchContext>()
            .working_directory = second_dir.to_string_lossy().into_owned();
        let first = open(&mut app, json!({"cwd":first_dir}));
        let second = open(&mut app, Value::Null);
        let first_session = app
            .world()
            .get::<TerminalPanelState>(first)
            .unwrap()
            .session_id;
        let second_session = app
            .world()
            .get::<TerminalPanelState>(second)
            .unwrap()
            .session_id;
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
        let saved = bedtime_workbench::save(app.world_mut());
        let panels = saved["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 2);
        assert!(
            panels
                .iter()
                .all(|panel| panel["state"].get("session").is_none())
        );
        let mut restored = make_app();
        bedtime_workbench::restore(restored.world_mut(), &saved).unwrap();
        let restored_cwds = restored
            .world_mut()
            .query::<&TerminalPanelState>()
            .iter(restored.world())
            .map(|panel| {
                restored
                    .world()
                    .non_send::<BedTerminal>()
                    .working_directory(panel.session_id)
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
            .session_id;
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
        let close = app.world().get::<PanelClose>(second).unwrap().0;
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
            .push(WorkbenchAction::OpenPanel {
                panel_type: PANEL_ID.into(),
                state: json!({"session":999999}),
                area: None,
            });
        bedtime_workbench::process_requests(app.world_mut());
        let record = app
            .world_mut()
            .query::<&bedtime_workbench::PanelRecord>()
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
            .spawn(PanelBinding {
                terminal: Some(adopted),
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
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
        let focus = app.world().get::<PanelFocus>(entity).unwrap().0;
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
            .spawn(PanelBinding {
                terminal: Some(session),
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
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
                .session_id,
            session
        );
        assert_eq!(
            app.world().get::<PanelBinding>(entity).unwrap().terminal,
            Some(session)
        );
        assert_eq!(
            app.world().non_send::<BedTerminal>().session_ids(),
            vec![session]
        );
        assert_eq!(frontend.save(app.world())["cwd"], json!("/legacy-shell"));
    }
}
