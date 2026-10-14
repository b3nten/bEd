//! The session owns documents and selections; typed main-thread state owns widgets.
use super::*;
use crate::lsp::lsp_symbol_info::LspSymbolDocumentation;
use bedtime_api::documents::DocumentFrame;
use bedtime_api::features::FrontendSnapshot;
use bedtime_api::workbench::{Panel, PanelCreation, PanelResult, WorkbenchAction, register};
use bedtime_api::{
    documents::PanelBinding,
    features::{FeatureSystems, HostEffects},
    workbench::{PanelActionInvocation, PanelActionResult, PanelActions, PanelClose, PanelFocus},
};
use bevy::prelude::*;
use std::collections::HashMap;

include!("panels.rs");

fn new_frontend(entity: Entity) -> Box<dyn Panel> {
    Box::new(EditorFrontend { entity, tool: None })
}

/// Register editors against the application's canonical NonSend DocumentSession,
/// EditorConfig and EditorRuntime. This plugin owns widgets, not another session.
pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        register_settings(app.world_mut());

        app.world_mut()
            .init_non_send::<bedtime_documents::DocumentSession>();
        app.init_non_send::<EditorViews>()
            .init_non_send::<EditorConfig>()
            .init_non_send::<EditorRuntime>()
            .init_resource::<DocumentFrame>()
            .init_resource::<bedtime_api::workbench::CommandPresentations>()
            .init_non_send::<bedtime_editor_widgets::extensions::EditorExtensions>()
            .init_non_send::<bedtime_editor_widgets::source_intents::SourceRequests>();
        app.init_resource::<HostEffects>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .add_systems(
                PreUpdate,
                (configure_preferences, prepare)
                    .chain()
                    .in_set(FeatureSystems::Prepare),
            )
            .add_systems(
                Last,
                (retire_views, shutdown.after(bevy::app::OnAppExitSystems)),
            );
        crate::zoom::install(app);
        crate::status::install(app);
        let actions = app.world_mut().register_system(panel_action);
        let focus = app
            .world_mut()
            .register_system(|In(entity): In<Entity>, world: &mut World| {
                focus_panel(world, entity)
            });
        let close = app
            .world_mut()
            .register_system(|In(entity): In<Entity>, world: &mut World| {
                close_panel(world, entity)
            });
        let clipboard = app.world_mut().register_system(clipboard_operation);
        app.insert_resource(Systems {
            clipboard,
            actions,
            focus,
            close,
        });
        let save = app
            .world_mut()
            .register_system(|world: &mut World| save_workspace(world));
        let restore = app.world_mut().register_system(
            |In(payload): In<bedtime_api::storage::FeatureStateRestore>, world: &mut World| {
                restore_workspace(world, payload.state.as_ref());
                Ok(())
            },
        );
        app.world_mut()
            .spawn(bedtime_api::storage::FeatureStateContribution {
                owner: MODULE_ID,
                save,
                restore,
            });
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, register_metadata)
            .expect("editor contributions are unique");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.command(NEW_DIAGNOSTICS_COMMAND, "New Diagnostics", open_diagnostics);
            registrar.command(
                "bedtime.editor.new_references",
                "New References",
                open_auxiliary,
            );
            registrar.command(
                "bedtime.editor.new_language_servers",
                "New Language Servers",
                open_auxiliary,
            );
            registrar.command(
                SHOW_DIAGNOSTICS_COMMAND,
                "Show Diagnostics",
                crate::status::show_panel,
            );
            registrar.command(
                SHOW_LSP_COMMAND,
                "Show Language Servers",
                crate::status::show_panel,
            );
            for (id, label) in [
                (crate::zoom::ZOOM_IN, "Zoom In"),
                (crate::zoom::ZOOM_OUT, "Zoom Out"),
                (crate::zoom::RESET_ZOOM, "Reset Zoom"),
            ] {
                registrar.command(id, label, crate::zoom::command);
            }
            for (id, label) in crate::lsp::editor_actions::COMMANDS {
                registrar.command(id, label, language_command);
            }
            for action in bedtime_editor_widgets::shortcuts::actions()
                .into_iter()
                .chain(bedtime_editor_widgets::shortcuts::hex_actions())
                .filter(|action| action.owner == MODULE_ID)
            {
                registrar.shortcut_action(action);
            }
            registrar.global_shortcut(SHOW_DIAGNOSTICS_COMMAND, &["Primary+Shift+M"]);
            registrar.panel(TEXT_PANEL_TYPE, "Text editor", create_text);
            registrar.panel(HEX_PANEL_TYPE, "Hex editor", create_hex);
            registrar.panel(DIAGNOSTICS_PANEL_TYPE, "Diagnostics", create_diagnostics);
            registrar.panel(REFERENCES_PANEL_TYPE, "References", create_references);
            registrar.panel(
                LSP_DASHBOARD_PANEL_TYPE,
                "Language Servers",
                create_dashboard,
            );
            registrar.panel_icon(TEXT_PANEL_TYPE, "text");
            registrar.panel_icon(HEX_PANEL_TYPE, "hex");
            registrar.panel_icon(DIAGNOSTICS_PANEL_TYPE, "diagnostics");
            registrar.panel_icon(LSP_DASHBOARD_PANEL_TYPE, "code");
            registrar.panel_icon(REFERENCES_PANEL_TYPE, "search");
            registrar.command_icon(SHOW_DIAGNOSTICS_COMMAND, "diagnostics");
            registrar.command_icon(NEW_DIAGNOSTICS_COMMAND, "diagnostics");
            registrar.command_icon(SHOW_LSP_COMMAND, "code");
            registrar.toolbar(NEW_DIAGNOSTICS_COMMAND);
        })
        .expect("EditorPlugin must be installed once");
    }
}

fn create_text(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_editor(world, creation, false)
}

fn open_auxiliary(
    In(invocation): In<bedtime_api::workbench::CommandInvocation>,
) -> bedtime_api::workbench::CommandResult {
    let panel_type = if invocation.command == "bedtime.editor.new_references" {
        REFERENCES_PANEL_TYPE
    } else {
        LSP_DASHBOARD_PANEL_TYPE
    };
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: panel_type.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn open_diagnostics(
    In(invocation): In<bedtime_api::workbench::CommandInvocation>,
) -> bedtime_api::workbench::CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: DIAGNOSTICS_PANEL_TYPE.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn language_command(
    In(invocation): In<bedtime_api::workbench::CommandInvocation>,
    world: &mut World,
) -> bedtime_api::workbench::CommandResult {
    let entity = invocation
        .source
        .ok_or("Language actions require a captured text presentation")?;
    let entity = world
        .get::<bedtime_api::workbench::PanelFrontend>(entity)
        .map_or(entity, |front| front.0);
    with_editor(world, |views, runtime, session| {
        let view = views
            .text(entity)
            .ok_or_else(|| io::Error::other("Language actions require a text presentation"))?;
        runtime.request_lsp_command(session, view.id(), &invocation.command)
    })
    .map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

/// Draw with feature-specific presentation while retaining normal editor input,
/// LSP integration, and source-captured host intentions.
pub fn draw_embedded(
    world: &mut World,
    entity: Entity,
    ui: &Ui,
    options: &EditorViewOptions,
) -> io::Result<bedtime_editor_widgets::ViewResponse> {
    let mut config = world.non_send::<EditorConfig>().clone();
    config.options = options.clone();
    if let Some(input) = world.get_non_send::<bedtime_api::workbench::ShortcutRuntime>() {
        config.options.shortcuts = input.input.with_scope(bedtime_keybindings::Scope::panel(
            bedtime_keybindings::EDITOR_SCOPE,
        ));
    }
    config.lsp.shortcuts = config.options.shortcuts.clone();
    let menu = bedtime_api::documents::panel_viewer_menu(world, entity, TEXT_VIEWER);
    let mut requests = Vec::new();
    let result = with_editor(world, |views, runtime, session| {
        let view = views
            .text_mut(entity)
            .expect("embedded text presentation owns a view");
        runtime.set_viewer_menu(view.id(), menu);
        draw_text_view(ui, view, runtime, session, &config, &mut requests)
    });
    for request in requests {
        world
            .resource_mut::<HostEffects>()
            .push(Some(entity), request);
    }
    emit_runtime_requests(world);
    result
}

fn create_diagnostics(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_tool(creation, world, runtime::ToolKind::Diagnostics)
}

fn create_references(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_tool(creation, world, runtime::ToolKind::References)
}

fn create_dashboard(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_tool(creation, world, runtime::ToolKind::Dashboard)
}

fn create_tool(creation: PanelCreation, world: &mut World, kind: runtime::ToolKind) -> PanelResult {
    let body = match kind {
        runtime::ToolKind::Diagnostics => {
            "Review errors and warnings from language servers and project checks. Select a diagnostic to navigate to its source location."
        }
        runtime::ToolKind::References => {
            "Use Find References in the editor to populate this panel. Select a result to navigate to that use of the symbol."
        }
        runtime::ToolKind::Dashboard => {
            "Inspect language server status, capabilities, and messages. Use the available server controls to manage a connection."
        }
    };
    world
        .entity_mut(creation.entity)
        .insert(bedtime_api::workbench::PanelDocumentation::new(&[(
            "Use", body,
        )]));
    Ok(Box::new(EditorFrontend {
        entity: creation.entity,
        tool: Some(kind),
    }))
}

pub struct EditorFrontend {
    pub entity: Entity,
    tool: Option<runtime::ToolKind>,
}

impl Panel for EditorFrontend {
    fn title(&self, world: &World) -> String {
        if let Some(tool) = self.tool {
            return tool.title().into();
        }
        let state = world
            .get::<EditorPanelState>(self.entity)
            .expect("editor presentation owns document state");
        if let Some(session) = world.get_non_send::<DocumentSession>()
            && let Ok(path) =
                session.with_document(state.document, |document| document.path.clone())
        {
            let name = if path.is_empty() {
                "Untitled".into()
            } else {
                Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            };
            return if state.hex {
                format!("{name} [Hex]")
            } else {
                name
            };
        }
        world
            .get_resource::<DocumentFrame>()
            .map(|_frame| {
                document_title(
                    &FrontendSnapshot::capture(world).context(),
                    state.document,
                    state.hex,
                )
            })
            .unwrap_or_else(|| "Closed".into())
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        actions: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let mut config = world.non_send::<EditorConfig>().clone();
        if let Some(settings) = world.get_resource::<bedtime_api::settings::SettingsSnapshot>() {
            apply_settings(&mut config, settings);
        }
        config.options.shortcuts = _shortcuts.with_scope(bedtime_keybindings::Scope::panel(
            bedtime_keybindings::EDITOR_SCOPE,
        ));
        config.lsp.shortcuts = config.options.shortcuts.clone();
        config.options.extensions = world
            .non_send::<bedtime_editor_widgets::extensions::EditorExtensions>()
            .clone();
        let mut response = None;
        let before = world
            .get::<EditorPanelState>(self.entity)
            .and_then(|state| {
                world
                    .non_send::<DocumentSession>()
                    .document_revision(state.document)
                    .ok()
                    .map(|revision| (state.document, revision))
            });
        let window = crate::zoom::window(ui);
        let menu = bedtime_api::documents::panel_viewer_menu(world, self.entity, TEXT_VIEWER);
        let expired_focus = unfocused_view(world, self.entity);
        let mut requests = Vec::new();
        let mut symbol_request = None;
        let result = with_editor(world, |views, runtime, session| {
            if let Some(view) = expired_focus {
                // A popup may defer a widget's old request. Another logical
                // panel receiving focus expires it without blocking clicks.
                session.with_view(view, |editor| editor.view_mut().request_focus = false)?;
            }
            if let Some(tool) = self.tool {
                return runtime.draw_tool(tool, ui, session, &mut requests);
            }
            match views
                .widgets
                .get_mut(&self.entity)
                .expect("editor presentation owns a widget")
            {
                Widget::Text(view) => {
                    runtime.set_viewer_menu(view.id(), menu);
                    let drawn = draw_text_view(ui, view, runtime, session, &config, &mut requests)?;
                    if let Some((row, column)) = drawn.symbol_request {
                        symbol_request = Some((view.id(), row, column));
                    }
                    response = Some(drawn);
                    Ok(())
                }
                Widget::Hex(editor) => {
                    editor.get_mut().shortcuts = config.options.shortcuts.clone();
                    editor.get_mut().draw(ui, session)
                }
            }
        });
        if let Some((view, row, column)) = symbol_request {
            let mut state = bevy::ecs::system::SystemState::<(
                NonSendMut<DocumentSession>,
                Query<&mut LspSymbolDocumentation>,
            )>::new(world);
            let (mut session, mut docs) =
                state.get_mut(world).expect("editor services are installed");
            let mut symbol = docs.get_mut(self.entity).expect("text frontend owns docs");
            if let Err(error) = symbol.request(&mut session, view, row, column) {
                ui.text_disabled(error.to_string());
            }
        }
        world.entity_mut(self.entity).insert(window);
        if before.is_some_and(|(document, revision)| {
            world
                .non_send::<DocumentSession>()
                .document_revision(document)
                .is_ok_and(|current| current != revision)
        }) {
            actions.push(WorkbenchAction::KeepPanel(self.entity));
        }
        for request in requests {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.entity), request);
        }
        emit_runtime_requests(world);
        if let Some(response) = response {
            world
                .non_send_mut::<bedtime_editor_widgets::source_intents::SourceRequests>()
                .observe(&response);
            actions.extend(
                bedtime_editor_widgets::source_intents::source_commands(&response)
                    .into_iter()
                    .map(|(command, arguments)| {
                        WorkbenchAction::Invoke(bedtime_api::workbench::CommandInvocation {
                            command: command.into(),
                            source: Some(self.entity),
                            area: 0,
                            arguments,
                        })
                    }),
            );
        }
        if let Err(error) = result {
            ui.text_disabled(error.to_string());
        }
    }

    fn save(&self, world: &World) -> Value {
        if self.tool.is_some() {
            return Value::Null;
        }
        save_embedded(world, self.entity)
    }
}

// These owners are temporarily local only while one coherent operation borrows
// all of them. A quarantined UI panic cannot remove services from the world.
fn with_editor<R>(
    world: &mut World,
    callback: impl FnOnce(&mut EditorViews, &mut EditorRuntime, &mut DocumentSession) -> R,
) -> R {
    let mut state = bevy::ecs::system::SystemState::<(
        NonSendMut<EditorViews>,
        NonSendMut<EditorRuntime>,
        NonSendMut<DocumentSession>,
    )>::new(world);
    let (mut views, mut runtime, mut session) = state
        .get_mut(world)
        .expect("editor state is installed together");
    callback(&mut views, &mut runtime, &mut session)
}

fn clipboard_operation(
    In(invocation): In<bedtime_api::workbench::ClipboardInvocation>,
    world: &mut World,
) -> bedtime_api::workbench::ClipboardResult {
    use bedtime_api::workbench::ClipboardAction;
    with_editor(world, |views, _, session| -> io::Result<Option<String>> {
        let widget = views
            .widgets
            .get_mut(&invocation.entity)
            .ok_or_else(|| io::Error::other("Editor panel is closed"))?;
        match widget {
            Widget::Text(view) => {
                if view.read_only()
                    && matches!(
                        invocation.action,
                        ClipboardAction::Cut
                            | ClipboardAction::Paste(_)
                            | ClipboardAction::PasteFromClipboard(_)
                    )
                {
                    return Ok(None);
                }
                session.with_commands(view.id(), |commands| match invocation.action {
                    ClipboardAction::Copy => {
                        Some(String::from_utf8_lossy(&commands.copy()).into_owned())
                    }
                    ClipboardAction::Cut => {
                        Some(String::from_utf8_lossy(&commands.cut()).into_owned())
                    }
                    ClipboardAction::Paste(text)
                    | ClipboardAction::PasteFromClipboard(Some(text)) => {
                        commands.paste(text.as_bytes());
                        None
                    }
                    ClipboardAction::PasteFromClipboard(None) => None,
                    ClipboardAction::SelectAll => {
                        commands.select_all();
                        None
                    }
                })
            }
            Widget::Hex(editor) => {
                let mut editor = editor.borrow_mut();
                match invocation.action {
                    ClipboardAction::Copy => editor.copy_text(session).map(Some),
                    ClipboardAction::Cut => editor.cut_text(session).map(Some),
                    ClipboardAction::Paste(text)
                    | ClipboardAction::PasteFromClipboard(Some(text)) => {
                        editor.paste_text(session, &text).map(|_| None)
                    }
                    ClipboardAction::PasteFromClipboard(None) => Ok(None),
                    ClipboardAction::SelectAll => editor.select_all(session).map(|_| None),
                }
            }
        }
    })
    .map_err(|error| error.to_string())
}

fn panel_action(In(invocation): In<PanelActionInvocation>, world: &mut World) -> PanelActionResult {
    let mut requests = Vec::new();
    let result = with_editor(world, |views, runtime, session| -> io::Result<bool> {
        let Some(widget) = views.widgets.get_mut(&invocation.entity) else {
            return Ok(false);
        };
        match widget {
            Widget::Text(view) => {
                match invocation.action {
                    PanelAction::Find => view.open_find(session)?,
                    PanelAction::GoToLine => view.open_line_jump(),
                    PanelAction::SelectAll => {
                        session.with_commands(view.id(), |commands| commands.select_all())?
                    }
                    PanelAction::Undo if view.read_only() => {}
                    PanelAction::Redo if view.read_only() => {}
                    PanelAction::Undo => session.undo_document(view.document_id())?,
                    PanelAction::Redo => session.redo_document(view.document_id())?,
                    PanelAction::CommitEdit => {}
                }
                runtime.record_overlay(view);
            }
            Widget::Hex(editor) => {
                let editor = editor.get_mut();
                match invocation.action {
                    PanelAction::SelectAll => editor.select_all(session)?,
                    PanelAction::Undo => requests.push(HostAction::Undo {
                        document: editor.document(),
                    }),
                    PanelAction::Redo => requests.push(HostAction::Redo {
                        document: editor.document(),
                    }),
                    PanelAction::Find | PanelAction::GoToLine => return Ok(false),
                    PanelAction::CommitEdit => {}
                }
            }
        }
        Ok(true)
    })
    .map_err(|error| error.to_string());
    for request in requests {
        world
            .resource_mut::<HostEffects>()
            .push(Some(invocation.entity), request);
    }
    result
}

pub fn close_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    with_editor(world, |views, runtime, session| {
        if let Some(Widget::Text(view)) = views.widgets.remove(&entity) {
            // Retirement may follow replacement of the canonical workspace.
            // The widget always drops, but it cannot mutate the new session.
            if view.session_id() == session.session_id() {
                session.detach_view(view.id());
                runtime.forget_view(view.id());
            }
        }
    });
    Ok(())
}

fn retire_views(world: &mut World) {
    if !world.contains_non_send::<DocumentSession>() || !world.contains_non_send::<EditorRuntime>()
    {
        return;
    }
    let dead = world
        .non_send::<EditorViews>()
        .widgets
        .keys()
        .filter(|entity| !world.entities().contains(**entity))
        .copied()
        .collect::<Vec<_>>();
    for entity in dead {
        close_panel(world, entity).expect("retiring a view only releases its owned presentation");
    }
}

fn prepare(world: &mut World) {
    if !world.contains_non_send::<DocumentSession>() || !world.contains_non_send::<EditorRuntime>()
    {
        return;
    }
    let commands = world
        .resource::<bedtime_api::features::FeatureRegistry>()
        .menus()
        .iter()
        .filter(|item| item.slot == bedtime_api::features::MenuSlot::TextSelection)
        .filter_map(|item| {
            let descriptor = world
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command(item.command)?;
            let presentation = descriptor
                .presentation(world.get_resource::<bedtime_api::workbench::CommandPresentations>());
            if !presentation.visible {
                return None;
            }
            Some(crate::runtime::EditorMenuCommand {
                id: descriptor.id.into(),
                label: presentation.label.into(),
                enabled: presentation.enabled,
            })
        })
        .collect::<Vec<_>>();
    let mut requests = Vec::new();
    let result = with_editor(world, |views, runtime, session| {
        for widget in views.widgets.values() {
            if let Widget::Text(view) = widget {
                runtime.set_menu_commands(view.id(), commands.clone());
            }
        }
        runtime.tick(session, &mut requests)
    });
    // Publish replies even when Docs has replaced the editor's visible tab.
    {
        let mut state = bevy::ecs::system::SystemState::<(
            NonSend<DocumentSession>,
            Query<(
                &LspSymbolDocumentation,
                &mut bedtime_api::workbench::PanelDocumentation,
            )>,
        )>::new(world);
        let (session, mut docs) = state.get_mut(world).expect("editor services are installed");
        for (symbol, mut notes) in &mut docs {
            if let Some(documentation) = symbol.snapshot(&session) {
                notes.symbol = Some(documentation);
            }
        }
    }
    for request in requests {
        world.resource_mut::<HostEffects>().push(None, request);
    }
    emit_runtime_requests(world);
    if let Err(error) = result {
        eprintln!("Editor language service: {error}");
    }
}

fn emit_runtime_requests(world: &mut World) {
    use bevy::ecs::system::SystemState;
    let mut resources = SystemState::<(
        NonSendMut<EditorRuntime>,
        NonSend<DocumentSession>,
        NonSend<EditorViews>,
        ResMut<HostEffects>,
    )>::new(world);
    let (mut runtime, session, views, mut intents) = resources
        .get_mut(world)
        .expect("EditorPlugin initializes its runtime owners");
    for (view, request) in runtime.drain_requests(&session) {
        let source = view.and_then(|view| {
            views
                .widgets
                .iter()
                .find_map(|(entity, widget)| match widget {
                    Widget::Text(text) if text.id() == view => Some(*entity),
                    _ => None,
                })
        });
        if view.is_none() || source.is_some() {
            intents.push(source, request);
        }
    }
}

fn shutdown(world: &mut World) {
    if world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|exits| !exits.is_empty())
        && let Some(mut runtime) = world.get_non_send_mut::<EditorRuntime>()
    {
        runtime.cancel_requests();
    }
}

pub fn save_workspace(world: &World) -> Value {
    json!({"project_check":world.non_send::<EditorRuntime>().project_check.as_ref().map(|config|config.to_json())})
}

pub fn restore_workspace(world: &mut World, state: Option<&Value>) {
    let mut runtime = world.non_send_mut::<EditorRuntime>();
    runtime.project_check = state.and_then(|value| {
        bedtime_documents::ProjectCheckConfig::from_json(&value["project_check"])
    });
    runtime.restore_project_check = true;
}

pub(crate) fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.settings_ui(
        "bedtime.editor.settings",
        "Code Editor",
        crate::settings_view::draw,
    );
    registrar.command_ref(SHOW_DIAGNOSTICS_COMMAND);
    registrar.command_ref(SHOW_LSP_COMMAND);
    registrar.command_ref(NEW_DIAGNOSTICS_COMMAND);
    registrar.toolbar(NEW_DIAGNOSTICS_COMMAND);
    for id in [
        crate::zoom::ZOOM_IN,
        crate::zoom::ZOOM_OUT,
        crate::zoom::RESET_ZOOM,
    ] {
        registrar.command_ref(id);
    }
    for (id, _) in crate::lsp::editor_actions::COMMANDS {
        registrar.command_ref(id);
    }
    registrar.panel_options(TEXT_PANEL_TYPE, false, PanelPlacement::Center);
    registrar.panel_options(HEX_PANEL_TYPE, false, PanelPlacement::Center);
    registrar.fallback_viewer(
        TEXT_VIEWER,
        "Text editor",
        TEXT_PANEL_TYPE,
        DocumentKind::Text,
    );
    registrar.fallback_viewer(
        HEX_VIEWER,
        "Hex editor",
        HEX_PANEL_TYPE,
        DocumentKind::Bytes,
    );
    for kind in [
        DIAGNOSTICS_PANEL_TYPE,
        REFERENCES_PANEL_TYPE,
        LSP_DASHBOARD_PANEL_TYPE,
    ] {
        registrar.panel_options(kind, false, PanelPlacement::Center);
    }
}

pub(crate) fn draw_text_view(
    ui: &Ui,
    view: &mut EditorView,
    runtime: &mut EditorRuntime,
    session: &mut DocumentSession,
    config: &EditorConfig,
    requests: &mut Vec<HostAction>,
) -> io::Result<bedtime_editor_widgets::ViewResponse> {
    runtime.before_draw(ui, view, session, config)?;
    view.set_navigation_animations(config.navigation_animations);
    if let Some(command) = crate::zoom::keyboard(
        ui,
        view,
        config.options.block_input,
        &config.options.shortcuts,
    ) {
        match command {
            bedtime_keybindings::ZOOM_IN | crate::zoom::ZOOM_IN => view.zoom_in(),
            bedtime_keybindings::ZOOM_OUT | crate::zoom::ZOOM_OUT => view.zoom_out(),
            bedtime_keybindings::RESET_ZOOM | crate::zoom::RESET_ZOOM => view.reset_zoom(),
            _ => unreachable!(),
        }
    }
    if ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS)
        && !config.options.block_input
        && !ui.io().want_text_input()
    {
        if config
            .options
            .shortcuts
            .take(ui, "bedtime.editor.toggle_wrap")
        {
            view.set_soft_wrap(!view.soft_wrap(config.options.soft_wrap));
        }
        if config
            .options
            .shortcuts
            .take(ui, "bedtime.editor.toggle_minimap")
        {
            view.set_minimap_enabled(!view.minimap_enabled(config.options.minimap_enabled));
        }
    }
    let mut options = config.options.clone();
    options.block_input |= runtime.language_action_blocks_view(view.id());
    if runtime.completion_visible_for(view.id()) {
        // Reserve the shared tooltip before the view can paint diagnostics or
        // extension hovers beneath the completion documentation window.
        view.presentation().tooltip_arbiter.claim(ui);
    }
    let response = view.draw(ui, session, &options)?;
    runtime.after_draw(ui, view, response.clone(), session, config, requests)?;
    runtime.record_overlay(view);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::documents::CommandContext;
    use bedtime_documents::ByteEdit;
    use bedtime_workbench::{WorkbenchPlugin, WorkbenchRequests};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn app() -> App {
        let mut app = App::new();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.world_mut().insert_non_send(EditorConfig::default());
        app.world_mut().insert_non_send(EditorRuntime::default());
        app.add_plugins((WorkbenchPlugin, EditorPlugin));
        app
    }

    #[test]
    fn registered_preferences_configure_editor_and_document_services_without_bed() {
        let mut app = app();
        app.insert_resource(bedtime_api::settings::SettingsSnapshot {
            settings: json!({
                "bedtime.editor": {
                    "word_wrap": false, "minimap": false, "rainbow": false,
                    "cursor_smear": false, "lsp_autocomplete": false,
                    "lsp_completion_delay_ms": 420.0,
                    "git_changed_lines": false, "treesitter": false,
                },
                "bedtime.ui": { "ui_animations": false },
            }),
            ..Default::default()
        });
        app.update();
        let editor = app.world().non_send::<EditorConfig>();
        assert!(!editor.options.soft_wrap);
        assert!(!editor.options.minimap_enabled);
        assert!(!editor.options.rainbow_mode);
        assert!(!editor.options.cursor_smear);
        assert!(!editor.navigation_animations);
        assert!(!editor.lsp.autocomplete);
        assert_eq!(editor.lsp.completion_delay_ms, 420);
        let session = app.world().non_send::<DocumentSession>();
        assert!(!session.options().git);
        assert!(!session.options().highlighting);
        app.world_mut()
            .resource_mut::<bedtime_api::settings::SettingsSnapshot>()
            .settings[MODULE_ID]["git_changed_lines"] = json!(true);
        app.world_mut()
            .resource_mut::<bedtime_api::settings::SettingsSnapshot>()
            .settings[MODULE_ID]["treesitter"] = json!(true);
        app.update();
        let session = app.world().non_send::<DocumentSession>();
        assert!(session.options().git);
        assert!(session.options().highlighting);
    }

    #[test]
    fn language_services_use_application_paths_and_preserve_explicit_configuration() {
        let directory = crate::test_support::TempDir::new();
        let mut app = app();
        let paths = bedtime_api::launch::ApplicationPaths::new(
            "example",
            Some(directory.path("config")),
            None,
        )
        .unwrap();
        app.insert_resource(paths.clone());
        app.world_mut().insert_non_send(
            DocumentSession::with_options(bedtime_documents::SessionOptions {
                project_root: Some(directory.0.clone()),
                ..Default::default()
            })
            .unwrap(),
        );
        crate::configure_preferences(app.world_mut());
        let options = app.world().non_send::<DocumentSession>().options();
        assert_eq!(options.lsp_config, Some(paths.config_dir.join("lsp.json")));
        assert_eq!(
            options.lsp_project_config,
            Some(std::path::PathBuf::from(".example/lsp.json"))
        );
        assert_eq!(
            options.lsp_config_mode,
            bedtime_documents::LspConfigMode::Layered
        );
        assert!(options.lsp_file_observations);
        assert!(!paths.config_dir.exists());

        let explicit = directory.path("custom-lsp.json");
        let mut options = options.clone();
        options.lsp_config = Some(explicit.clone());
        let explicit_project = std::path::PathBuf::from("project-config/language-servers.json");
        options.lsp_project_config = Some(explicit_project.clone());
        options.lsp_config_mode = bedtime_documents::LspConfigMode::Snapshot;
        options.lsp_file_observations = false;
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .configure(options)
            .unwrap();
        crate::configure_preferences(app.world_mut());
        let options = app.world().non_send::<DocumentSession>().options();
        assert_eq!(options.lsp_config, Some(explicit));
        assert_eq!(options.lsp_project_config, Some(explicit_project));
        assert_eq!(
            options.lsp_config_mode,
            bedtime_documents::LspConfigMode::Snapshot
        );
        assert!(!options.lsp_file_observations);
    }

    #[test]
    fn project_language_configuration_follows_application_identity_and_custom_folder() {
        let directory = crate::test_support::TempDir::new();
        for (name, configured_folder, expected_folder) in [
            ("first", None, ".first"),
            ("second", Some(".custom-project".into()), ".custom-project"),
        ] {
            let mut app = app();
            app.insert_resource(
                bedtime_api::launch::ApplicationPaths::new(
                    name,
                    Some(directory.path(name)),
                    configured_folder,
                )
                .unwrap(),
            );
            app.world_mut().insert_non_send(
                DocumentSession::with_options(bedtime_documents::SessionOptions {
                    project_root: Some(directory.0.clone()),
                    ..Default::default()
                })
                .unwrap(),
            );
            crate::configure_preferences(app.world_mut());
            let session = app.world().non_send::<DocumentSession>();
            assert_eq!(
                session.options().lsp_project_config,
                Some(std::path::PathBuf::from(expected_folder).join("lsp.json")),
            );
            assert!(
                session.lsp().unwrap().configuration_sources().contains(
                    &session
                        .options()
                        .project_root
                        .as_ref()
                        .unwrap()
                        .join(expected_folder)
                        .join("lsp.json")
                )
            );
        }

        let mut app = app();
        app.world_mut().insert_non_send(
            DocumentSession::with_options(bedtime_documents::SessionOptions {
                project_root: Some(directory.0.clone()),
                ..Default::default()
            })
            .unwrap(),
        );
        crate::configure_preferences(app.world_mut());
        let options = app.world().non_send::<DocumentSession>().options();
        assert!(options.lsp_config.is_none());
        assert!(options.lsp_project_config.is_none());
    }

    fn open(app: &mut App, panel_type: &str) -> Entity {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
                panel_type: panel_type.into(),
                state: Value::Null,
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

    fn attach(
        app: &mut App,
        panel_type: &str,
        document: DocumentId,
        view: Option<bedtime_documents::ViewId>,
        state: Value,
    ) -> (Entity, Box<dyn Panel>) {
        let entity = app
            .world_mut()
            .spawn(PanelBinding {
                input: bedtime_api::documents::PanelInput::Document(document),
                document: Some(document),
                view,
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .panel(panel_type)
            .unwrap()
            .factory;
        let frontend = app
            .world_mut()
            .run_system_with(factory, PanelCreation { entity, state })
            .unwrap()
            .unwrap();
        (entity, frontend)
    }

    #[test]
    fn adopting_inactive_editor_view_preserves_focus_request() {
        let mut app = app();
        let (document, focused, adopted) = {
            let mut session = app.world_mut().non_send_mut::<DocumentSession>();
            let document = session.create_document(b"shared text").unwrap();
            let focused = EditorView::new(&mut session, document).unwrap();
            let adopted = EditorView::new(&mut session, document).unwrap();
            session.request_focus(focused.id()).unwrap();
            (document, focused, adopted)
        };
        let (entity, _frontend) = attach(
            &mut app,
            TEXT_PANEL_TYPE,
            document,
            Some(adopted.id()),
            Value::Null,
        );
        let session = app.world().non_send::<DocumentSession>();
        assert!(session.view_snapshot(focused.id()).unwrap().request_focus);
        assert!(
            !session.view_snapshot(adopted.id()).unwrap().request_focus,
            "mounting an inactive presentation must not request keyboard focus"
        );
        let focus = app.world().get::<PanelFocus>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(focus, entity)
            .unwrap()
            .unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .view_snapshot(adopted.id())
                .unwrap()
                .request_focus
        );
    }

    #[test]
    fn toggle_comment_is_registered_for_keybinding_settings() {
        let app = app();
        let registry = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>();
        let action = registry
            .shortcuts()
            .iter()
            .find(|action| action.id == "bedtime.editor.toggle_comment")
            .unwrap();
        assert_eq!(action.label, "Toggle Line Comment");
        assert_eq!(
            action.scope,
            bedtime_keybindings::Scope::panel(bedtime_keybindings::EDITOR_SCOPE)
        );
        assert_eq!(action.defaults, vec!["Primary+Slash".parse().unwrap()]);
    }

    #[test]
    fn plugin_creates_independent_buffers_and_routes_actions_to_the_originating_view() {
        let mut app = app();
        let first = open(&mut app, TEXT_PANEL_TYPE);
        let second = open(&mut app, TEXT_PANEL_TYPE);
        let first_binding = app.world().get::<PanelBinding>(first).unwrap().clone();
        let second_binding = app.world().get::<PanelBinding>(second).unwrap().clone();
        assert_ne!(first_binding.document, second_binding.document);
        assert_ne!(first_binding.view, second_binding.view);
        let action = app.world().get::<PanelActions>(first).unwrap().0;
        for action_kind in [PanelAction::Find, PanelAction::GoToLine] {
            assert!(
                app.world_mut()
                    .run_system_with(
                        action,
                        PanelActionInvocation {
                            entity: first,
                            action: action_kind
                        }
                    )
                    .unwrap()
                    .unwrap()
            );
            let view = app.world().non_send::<EditorViews>().text(first).unwrap();
            assert!(match action_kind {
                PanelAction::Find => view.find_visible(),
                PanelAction::GoToLine => view.line_jump_visible(),
                _ => unreachable!(),
            });
        }
        let views = app.world().non_send::<EditorViews>();
        assert!(
            !views.text(second).unwrap().find_visible()
                && !views.text(second).unwrap().line_jump_visible()
        );
        let document = first_binding.document.unwrap();
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"one\ntwo".to_vec(),
                }],
            )
            .unwrap();
        app.world_mut()
            .run_system_with(
                action,
                PanelActionInvocation {
                    entity: first,
                    action: PanelAction::SelectAll,
                },
            )
            .unwrap()
            .unwrap();
        let session = app.world().non_send::<DocumentSession>();
        let selection = session
            .view_snapshot(first_binding.view.unwrap())
            .unwrap()
            .selections;
        assert_eq!(selection[0].head_row, 1);
        assert_eq!(selection[0].head_column, 3);
        assert!(
            session
                .snapshot(second_binding.document.unwrap())
                .unwrap()
                .bytes
                .is_empty()
        );
    }

    #[test]
    fn adopts_managed_view_identity_and_closes_only_its_presentation() {
        let mut app = app();
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        let document = session.create_document(b"abc\ndef").unwrap();
        let legacy = EditorView::new(&mut session, document).unwrap();
        let view = legacy.id();
        restore_text_state(
            &mut session,
            view,
            &json!({"selections":[[1,2,0,1]],"primary":0,"scroll":[4.0,8.0]}),
        )
        .unwrap();
        let (entity, frontend) =
            attach(&mut app, TEXT_PANEL_TYPE, document, Some(view), Value::Null);
        assert_eq!(
            app.world()
                .non_send::<EditorViews>()
                .text(entity)
                .unwrap()
                .id(),
            view
        );
        assert_eq!(
            app.world().non_send::<DocumentSession>().view_ids(document),
            vec![view]
        );
        drop(legacy);
        assert_eq!(
            frontend.save(app.world())["selections"],
            json!([[1, 2, 0, 1]])
        );
        let close = app.world().get::<PanelClose>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(close, entity)
            .unwrap()
            .unwrap();
        app.world_mut()
            .run_system_with(close, entity)
            .unwrap()
            .unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .view_ids(document)
                .is_empty()
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            b"abc\ndef"
        );
    }

    #[test]
    fn retiring_a_previous_workspace_widget_does_not_mutate_replacement_services() {
        let mut app = app();
        let old_entity = open(&mut app, TEXT_PANEL_TYPE);
        let old_view = app
            .world()
            .non_send::<EditorViews>()
            .text(old_entity)
            .unwrap()
            .id();
        let old_workspace = app.world().non_send::<DocumentSession>().session_id();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.world_mut().insert_non_send(EditorRuntime::default());
        let current_entity = open(&mut app, TEXT_PANEL_TYPE);
        let current_view = app
            .world()
            .non_send::<EditorViews>()
            .text(current_entity)
            .unwrap()
            .id();
        let current_document = app
            .world()
            .get::<EditorPanelState>(current_entity)
            .unwrap()
            .document;
        assert_ne!(
            app.world().non_send::<DocumentSession>().session_id(),
            old_workspace
        );
        // The current runtime's state belongs to the replacement workspace,
        // even while its restore pass still contains references to old views.
        app.world_mut()
            .non_send_mut::<EditorRuntime>()
            .set_context(old_view, CommandContext::default());
        close_panel(app.world_mut(), old_entity).unwrap();
        assert!(
            app.world()
                .non_send::<EditorViews>()
                .text(old_entity)
                .is_none()
        );
        assert!(
            app.world()
                .non_send::<EditorRuntime>()
                .context(old_view)
                .is_some()
        );
        let session = app.world().non_send::<DocumentSession>();
        assert_eq!(session.view_ids(current_document), vec![current_view]);
        assert!(session.snapshot(current_document).unwrap().bytes.is_empty());
        close_panel(app.world_mut(), current_entity).unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .view_ids(current_document)
                .is_empty()
        );
    }

    #[test]
    fn hidden_text_and_hex_persistence_observes_edits_in_sibling_views() {
        let mut app = app();
        let document = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .create_document(b"abc\ndef")
            .unwrap();
        let (text, frontend) = attach(
            &mut app,
            TEXT_PANEL_TYPE,
            document,
            None,
            json!({"selections":[[0,2,0,1]],"primary":0,"scroll":[4.0,8.0]}),
        );
        let bytes = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .create_document_with_kind(&[0, 1, 2, 3, 4, 5], DocumentKind::Bytes)
            .unwrap();
        let (_, hex) = attach(
            &mut app,
            HEX_PANEL_TYPE,
            bytes,
            None,
            json!({"cursor":2,"anchor":4,"insert":true}),
        );
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        for document in [document, bytes] {
            let revision = session.document_revision(document).unwrap();
            session
                .apply_edits(
                    document,
                    revision,
                    &[ByteEdit {
                        range: 0..0,
                        bytes: b"XY".to_vec(),
                    }],
                )
                .unwrap();
        }
        assert_eq!(
            frontend.save(app.world())["selections"],
            json!([[0, 4, 0, 3]])
        );
        assert_eq!(frontend.save(app.world())["scroll"], json!([4.0, 8.0]));
        assert_eq!(
            hex.save(app.world()),
            json!({"cursor":4,"anchor":6,"insert":true})
        );
        app.world_mut().despawn(text);
        app.update();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .view_ids(document)
                .is_empty()
        );
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .is_ok()
        );
    }

    #[test]
    fn frontend_unwind_restores_all_typed_owners() {
        let mut app = app();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            with_editor(app.world_mut(), |_, _, _| panic!("frontend failure"))
        }));
        assert!(panic.is_err());
        assert!(app.world().contains_non_send::<EditorViews>());
        assert!(app.world().contains_non_send::<EditorRuntime>());
        assert!(app.world().contains_non_send::<DocumentSession>());
        open(&mut app, TEXT_PANEL_TYPE);
    }

    #[test]
    fn embedded_snapshot_actions_are_read_only_before_the_first_draw() {
        let mut app = app();
        let document = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .create_snapshot_document(b"immutable\n", "rust")
            .unwrap();
        let entity = app.world_mut().spawn_empty().id();
        create_embedded(
            app.world_mut(),
            entity,
            document,
            &json!({"selections":[[0,4,0,1]],"primary":0,"scroll":[2.0,8.0]}),
        )
        .unwrap();
        assert!(
            app.world()
                .non_send::<EditorViews>()
                .text(entity)
                .unwrap()
                .read_only()
        );
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        let action = app.world().get::<PanelActions>(entity).unwrap().0;
        for action_kind in [PanelAction::Undo, PanelAction::Redo] {
            assert!(
                app.world_mut()
                    .run_system_with(
                        action,
                        PanelActionInvocation {
                            entity,
                            action: action_kind
                        }
                    )
                    .unwrap()
                    .unwrap()
            );
        }
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .document_revision(document)
                .unwrap(),
            revision
        );
        assert_eq!(
            save_embedded(app.world(), entity)["selections"],
            json!([[0, 4, 0, 1]])
        );
    }

    #[test]
    fn real_frontend_edits_the_canonical_document_and_undo_does_not_reclaim_pristine_input() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = app();
        let entity = open(&mut app, TEXT_PANEL_TYPE);
        let focus = app.world().get::<PanelFocus>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(focus, entity)
            .unwrap()
            .unwrap();
        let document = app
            .world()
            .get::<EditorPanelState>(entity)
            .unwrap()
            .document;
        assert!(input_is_empty(app.world(), entity));
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut draw = |context: &mut dear_imgui_rs::Context| {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [600.0, 400.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            ui.window("Plugin Editor")
                .position([0.0; 2], dear_imgui_rs::Condition::Always)
                .size([600.0, 400.0], dear_imgui_rs::Condition::Always)
                .build(|| bedtime_workbench::render_panel(app.world_mut(), entity, ui).unwrap());
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        };
        draw(&mut context);
        draw(&mut context);
        context.io_mut().add_input_characters_utf8("x");
        draw(&mut context);
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            b"x"
        );
        assert!(!input_is_empty(app.world(), entity));
        let system = app.world().get::<PanelActions>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(
                system,
                PanelActionInvocation {
                    entity,
                    action: PanelAction::Undo,
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes
                .is_empty()
        );
        assert!(!input_is_empty(app.world(), entity));
    }

    #[test]
    fn clicked_frontend_publishes_docs_after_it_stops_drawing() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};
        use std::time::{Duration, Instant};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let (_directory, session, document) =
            crate::lsp::test_fixture::session_mode("hover-delayed");
        let mut app = app();
        app.world_mut().insert_non_send(session);
        let (entity, mut frontend) = attach(&mut app, TEXT_PANEL_TYPE, document, None, Value::Null);
        let input = bedtime_api::workbench::ShortcutInput::default();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut draw = |context: &mut Context, app: &mut App| {
            context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut target = [0.0; 2];
            ui.window("Symbol docs editor")
                .position([0.0; 2], Condition::Always)
                .size([600.0, 400.0], Condition::Always)
                .build(|| {
                    frontend.draw(ui, app.world_mut(), &mut Vec::new(), &input);
                    let views = app.world().non_send::<EditorViews>();
                    let layout = views.text(entity).unwrap().presentation().layout;
                    target = [
                        bedtime_editor_widgets::views::view_layout::line_column_x(
                            ui,
                            b"let value = St",
                            12,
                            layout.text_pos[0],
                        ) + 1.0,
                        layout.text_pos[1] + layout.line_height * 0.5,
                    ];
                });
            drop(context.render_legacy());
            target
        };
        draw(&mut context, &mut app);
        let target = draw(&mut context, &mut app);
        context.io_mut().add_mouse_pos_event(target);
        draw(&mut context, &mut app);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut app);
        assert!(
            app.world()
                .get::<bedtime_api::workbench::PanelDocumentation>(entity)
                .unwrap()
                .symbol
                .is_none()
        );
        // No more editor frames: replies must publish through the update schedule.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.world_mut().non_send_mut::<DocumentSession>().tick();
            app.update();
            if let Some(text) = &app
                .world()
                .get::<bedtime_api::workbench::PanelDocumentation>(entity)
                .unwrap()
                .symbol
            {
                assert_eq!(
                    text.text,
                    "```rust\npub fn St()\n```\n\nA **Unicode** `string`."
                );
                assert!(text.markdown);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "hidden editor did not publish clicked symbol docs"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Empty targets and pending replies leave the last published symbol readable.
        let published = app
            .world()
            .get::<bedtime_api::workbench::PanelDocumentation>(entity)
            .unwrap()
            .symbol
            .clone()
            .unwrap();
        {
            let view = app
                .world()
                .non_send::<EditorViews>()
                .text(entity)
                .unwrap()
                .id();
            let mut state = bevy::ecs::system::SystemState::<(
                NonSendMut<DocumentSession>,
                Query<&mut LspSymbolDocumentation>,
            )>::new(app.world_mut());
            let (mut session, mut symbols) = state.get_mut(app.world_mut()).unwrap();
            symbols
                .get_mut(entity)
                .unwrap()
                .request(&mut session, view, 0, 0)
                .unwrap();
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<bedtime_api::workbench::PanelDocumentation>(entity)
                .unwrap()
                .symbol
                .as_ref(),
            Some(&published)
        );
        let until = Instant::now() + Duration::from_secs(5);
        while app
            .world()
            .get::<LspSymbolDocumentation>(entity)
            .unwrap()
            .pending()
        {
            app.world_mut().non_send_mut::<DocumentSession>().tick();
            app.update();
            assert!(Instant::now() < until, "empty hover did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            app.world()
                .get::<bedtime_api::workbench::PanelDocumentation>(entity)
                .unwrap()
                .symbol
                .as_ref(),
            Some(&published)
        );
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"x".to_vec(),
                }],
            )
            .unwrap();
        app.update();
        assert!(
            app.world()
                .get::<bedtime_api::workbench::PanelDocumentation>(entity)
                .unwrap()
                .symbol
                .is_some()
        );
    }

    #[test]
    fn workspace_contribution_restores_project_check_through_the_prepare_system() {
        let mut app = app();
        let config = bedtime_documents::ProjectCheckConfig {
            program: "custom-check".into(),
            automatic: false,
            ..Default::default()
        };
        app.world_mut()
            .non_send_mut::<EditorRuntime>()
            .project_check = Some(config.clone());
        let contribution = *app
            .world_mut()
            .query::<&bedtime_api::storage::FeatureStateContribution>()
            .single(app.world())
            .unwrap();
        let saved = app.world_mut().run_system(contribution.save).unwrap();
        app.world_mut()
            .non_send_mut::<EditorRuntime>()
            .project_check = None;
        app.world_mut()
            .run_system_with(
                contribution.restore,
                bedtime_api::storage::FeatureStateRestore { state: Some(saved) },
            )
            .unwrap()
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .project_check_config(),
            &config
        );
        assert!(
            !app.world()
                .non_send::<EditorRuntime>()
                .restore_project_check
        );
    }
}
