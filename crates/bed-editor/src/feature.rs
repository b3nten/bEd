//! The session owns documents and selections; typed main-thread state owns widgets.
use super::*;
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelActions, EditorPanelBinding,
    EditorPanelClose, EditorPanelFocus, PanelActionInvocation, PanelActionResult,
};
use bed_workbench_api::{Action, Panel, PanelCreation, PanelResult, register};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug)]
pub struct EditorPanelState {
    pub document: DocumentId,
    pub initial_revision: Option<bed_editor_api::Revision>,
    pub hex: bool,
}

pub(crate) enum Widget {
    Text(EditorView),
    Hex(RefCell<HexEditor>),
}
#[derive(Default)]
pub struct EditorViews {
    pub(crate) widgets: HashMap<Entity, Widget>,
}
impl EditorViews {
    pub fn text(&self, entity: Entity) -> Option<&EditorView> {
        match self.widgets.get(&entity)? {
            Widget::Text(view) => Some(view),
            Widget::Hex(_) => None,
        }
    }
    pub fn text_mut(&mut self, entity: Entity) -> Option<&mut EditorView> {
        match self.widgets.get_mut(&entity)? {
            Widget::Text(view) => Some(view),
            Widget::Hex(_) => None,
        }
    }
    pub fn hex(&self, entity: Entity) -> Option<std::cell::Ref<'_, HexEditor>> {
        match self.widgets.get(&entity)? {
            Widget::Hex(editor) => Some(editor.borrow()),
            Widget::Text(_) => None,
        }
    }
}

#[derive(Resource)]
struct Systems {
    actions: bed_editor_api::PanelActionSystem,
    clipboard: bed_editor_api::EditorPanelClipboardSystem,
    focus: bed_editor_api::EditorPanelLifecycleSystem,
    close: bed_editor_api::EditorPanelLifecycleSystem,
}

/// Register editors against the application's canonical NonSend EditorSession,
/// EditorConfig and EditorRuntime. This plugin owns widgets, not another session.
pub struct EditorPlugin;
impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .init_non_send::<bed_document_session::EditorSession>();
        app.init_non_send::<EditorViews>()
            .init_non_send::<EditorConfig>()
            .init_non_send::<EditorRuntime>()
            .init_resource::<EditorFrame>()
            .init_resource::<bed_editor_api::CommandPresentations>()
            .init_non_send::<bed_editor_ui::extensions::EditorExtensions>()
            .init_non_send::<bed_editor_ui::source_intents::SourceRequests>();
        app.init_resource::<EditorIntents>()
            .init_resource::<bed_editor_api::Registry>()
            .add_systems(PreUpdate, prepare.in_set(EditorFeatureSystems::Prepare))
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
            |In(payload): In<bed_editor_api::EditorWorkspaceRestore>, world: &mut World| {
                restore_workspace(world, payload.state.as_ref());
                Ok(())
            },
        );
        app.world_mut()
            .spawn(bed_editor_api::EditorWorkspaceContribution {
                owner: MODULE_ID,
                save,
                restore,
            });
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, register_metadata)
            .expect("editor contributions are unique");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.command(NEW_DIAGNOSTICS_COMMAND, "New Diagnostics", open_diagnostics);
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
            for action in bed_editor_ui::shortcuts::actions()
                .into_iter()
                .chain(bed_editor_ui::shortcuts::hex_actions())
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
        })
        .expect("EditorPlugin must be installed once");
    }
}

fn create_text(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_editor(world, creation, false)
}

fn open_diagnostics(
    In(invocation): In<bed_workbench_api::CommandInvocation>,
) -> bed_workbench_api::CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: DIAGNOSTICS_PANEL_TYPE.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

/// Attach a text presentation owned by another feature, such as a Git diff.
/// It shares the session, LSP runtime, action systems, and close lifecycle.
pub fn create_embedded(
    world: &mut World,
    entity: Entity,
    document: DocumentId,
    state: &Value,
) -> Result<bed_document_session::ViewId, String> {
    let mut binding = world
        .get::<EditorPanelBinding>(entity)
        .cloned()
        .unwrap_or_default();
    binding.document = Some(document);
    binding.input = bed_editor_api::PanelInput::Document(document);
    world.entity_mut(entity).insert(binding);
    create_editor(
        world,
        PanelCreation {
            entity,
            state: state.clone(),
        },
        false,
    )?;
    Ok(world
        .get::<EditorPanelBinding>(entity)
        .expect("an embedded editor has a binding")
        .view
        .expect("a text presentation has a managed view"))
}

/// Draw with feature-specific presentation while retaining normal editor input,
/// LSP integration, and source-captured host intentions.
pub fn draw_embedded(
    world: &mut World,
    entity: Entity,
    ui: &Ui,
    options: &EditorViewOptions,
) -> io::Result<bed_editor_ui::ViewResponse> {
    let mut config = world.non_send::<EditorConfig>().clone();
    config.options = options.clone();
    if let Some(input) = world.get_non_send::<bed_workbench_api::ShortcutRuntime>() {
        config.options.shortcuts = input
            .input
            .with_scope(bed_keybindings::Scope::panel(bed_keybindings::EDITOR_SCOPE));
    }
    config.lsp.shortcuts = config.options.shortcuts.clone();
    let menu = bed_editor_api::panel_viewer_menu(world, entity, TEXT_VIEWER);
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
            .resource_mut::<EditorIntents>()
            .push(Some(entity), request);
    }
    emit_runtime_requests(world);
    result
}
fn create_hex(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_editor(world, creation, true)
}

fn create_editor(world: &mut World, creation: PanelCreation, hex: bool) -> PanelResult {
    let mut binding = world
        .get::<EditorPanelBinding>(creation.entity)
        .cloned()
        .unwrap_or_default();
    if binding.input.local_file().is_some() {
        return Err("Editor requires a document, not a local-file panel".into());
    }
    let mut session = world
        .get_non_send_mut::<EditorSession>()
        .ok_or("Editor document session is unavailable")?;
    let created = binding.input.document().or(binding.document).is_none();
    let document = match binding.input.document().or(binding.document) {
        Some(document) => document,
        None => session
            .create_document_with_kind(
                &[],
                if hex {
                    DocumentKind::Bytes
                } else {
                    DocumentKind::Text
                },
            )
            .map_err(|error| error.to_string())?,
    };
    let initial_revision = pristine_revision(&session, document);
    let result = (|| -> io::Result<Widget> {
        if hex {
            if session.document_kind(document)? != DocumentKind::Bytes {
                return Err(io::Error::other(
                    "Hex editor requires an exact-byte document",
                ));
            }
            let mut editor = HexEditor::new(document);
            editor.restore_state(&creation.state);
            editor.synchronize(&session)?;
            Ok(Widget::Hex(RefCell::new(editor)))
        } else {
            let view = if let Some(view) = binding.view {
                if session.document_for_view(view) != Some(document) {
                    return Err(io::Error::other(
                        "Editor view belongs to a different document",
                    ));
                }
                EditorView::from_existing(&mut session, view)?
            } else {
                let view = EditorView::new(&mut session, document)?;
                if let Err(error) = restore_text_state(&mut session, view.id(), &creation.state) {
                    session.detach_view(view.id());
                    return Err(error);
                }
                view
            };
            Ok(Widget::Text(view))
        }
    })();
    if created && result.is_err() {
        let _ = session.close_document(
            document,
            bed_document_session::editor_session::ClosePolicy::Discard,
        );
    }
    let widget = result.map_err(|error| error.to_string())?;
    binding.document = Some(document);
    binding.input = bed_editor_api::PanelInput::Document(document);
    binding.workspace = session.workspace_id().0;
    binding
        .viewer
        .get_or_insert_with(|| if hex { HEX_VIEWER } else { TEXT_VIEWER }.into());
    binding.view = match &widget {
        Widget::Text(view) => Some(view.id()),
        Widget::Hex(_) => None,
    };
    if let Widget::Text(view) = &widget {
        world.non_send_mut::<EditorRuntime>().record_overlay(view);
    }
    world
        .non_send_mut::<EditorViews>()
        .widgets
        .insert(creation.entity, widget);
    let systems = world.resource::<Systems>();
    let hooks = (
        EditorPanelActions(systems.actions),
        bed_editor_api::EditorPanelClipboard(systems.clipboard),
        EditorPanelFocus(systems.focus),
        EditorPanelClose(systems.close),
    );
    world.entity_mut(creation.entity).insert((
        binding,
        bed_workbench_api::PanelPadding([0.0; 2]),
        EditorPanelState {
            document,
            initial_revision,
            hex,
        },
        hooks,
    ));
    if let Some(revision) = initial_revision {
        world
            .entity_mut(creation.entity)
            .insert(bed_editor_api::EditorPanelDefaultInput { document, revision });
    }
    Ok(Box::new(EditorFrontend {
        entity: creation.entity,
        tool: None,
    }))
}

fn create_diagnostics(In(creation): In<PanelCreation>) -> PanelResult {
    create_tool(creation, runtime::ToolKind::Diagnostics)
}
fn create_references(In(creation): In<PanelCreation>) -> PanelResult {
    create_tool(creation, runtime::ToolKind::References)
}
fn create_dashboard(In(creation): In<PanelCreation>) -> PanelResult {
    create_tool(creation, runtime::ToolKind::Dashboard)
}
fn create_tool(creation: PanelCreation, kind: runtime::ToolKind) -> PanelResult {
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
        if let Some(session) = world.get_non_send::<EditorSession>()
            && let Ok(snapshot) = session.snapshot(state.document)
        {
            let name = if snapshot.path.is_empty() {
                "Untitled".into()
            } else {
                Path::new(&snapshot.path)
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
            .get_resource::<EditorFrame>()
            .map(|frame| document_title(&frame.context(), state.document, state.hex))
            .unwrap_or_else(|| "Closed".into())
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        actions: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let mut config = world.non_send::<EditorConfig>().clone();
        config.options.shortcuts =
            _shortcuts.with_scope(bed_keybindings::Scope::panel(bed_keybindings::EDITOR_SCOPE));
        config.lsp.shortcuts = config.options.shortcuts.clone();
        config.options.extensions = world
            .non_send::<bed_editor_ui::extensions::EditorExtensions>()
            .clone();
        let mut response = None;
        let window = crate::zoom::window(ui);
        let menu = bed_editor_api::panel_viewer_menu(world, self.entity, TEXT_VIEWER);
        let mut requests = Vec::new();
        let result = with_editor(world, |views, runtime, session| {
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
                    draw_text_view(ui, view, runtime, session, &config, &mut requests).map(
                        |drawn| {
                            response = Some(drawn);
                        },
                    )
                }
                Widget::Hex(editor) => {
                    editor.get_mut().shortcuts = config.options.shortcuts.clone();
                    editor.get_mut().draw(ui, session)
                }
            }
        });
        world.entity_mut(self.entity).insert(window);
        for request in requests {
            world
                .resource_mut::<EditorIntents>()
                .push(Some(self.entity), request);
        }
        emit_runtime_requests(world);
        if let Some(response) = response {
            world
                .non_send_mut::<bed_editor_ui::source_intents::SourceRequests>()
                .observe(&response);
            actions.extend(
                bed_editor_ui::source_intents::source_commands(&response)
                    .into_iter()
                    .map(|(command, arguments)| {
                        Action::Invoke(bed_workbench_api::CommandInvocation {
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

/// Persist the live editor view, including selection transforms from edits in siblings.
pub fn save_embedded(world: &World, entity: Entity) -> Value {
    let session = world.non_send::<EditorSession>();
    match world
        .non_send::<EditorViews>()
        .widgets
        .get(&entity)
        .expect("editor presentation owns a widget")
    {
        Widget::Text(view) => {
            let state = session
                .view_snapshot(view.id())
                .expect("live editor has a session view");
            json!({"selections":state.selections.iter().map(|selection| [selection.head_row,selection.head_column,selection.anchor_row,selection.anchor_column]).collect::<Vec<_>>(),"primary":state.primary_index,"scroll":state.requested_scroll.unwrap_or(state.scroll_position)})
        }
        Widget::Hex(editor) => {
            let mut editor = editor.borrow_mut();
            editor
                .synchronize(session)
                .expect("live hex editor has a session document");
            editor.state()
        }
    }
}

// These owners are temporarily local only while one coherent operation borrows
// all of them. A quarantined UI panic cannot remove services from the world.
fn with_editor<R>(
    world: &mut World,
    callback: impl FnOnce(&mut EditorViews, &mut EditorRuntime, &mut EditorSession) -> R,
) -> R {
    let mut state = bevy::ecs::system::SystemState::<(
        NonSendMut<EditorViews>,
        NonSendMut<EditorRuntime>,
        NonSendMut<EditorSession>,
    )>::new(world);
    let (mut views, mut runtime, mut session) = state
        .get_mut(world)
        .expect("editor state is installed together");
    callback(&mut views, &mut runtime, &mut session)
}

fn clipboard_operation(
    In(invocation): In<bed_editor_api::ClipboardInvocation>,
    world: &mut World,
) -> bed_editor_api::ClipboardResult {
    use bed_editor_api::ClipboardAction;
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
                        ClipboardAction::Cut | ClipboardAction::Paste(_)
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
                    ClipboardAction::Paste(text) => {
                        commands.paste(text.as_bytes());
                        None
                    }
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
                    ClipboardAction::Paste(text) => editor.paste_text(session, &text).map(|_| None),
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
                    PanelAction::Undo => {
                        session.with_commands(view.id(), |commands| commands.undo())?
                    }
                    PanelAction::Redo => {
                        session.with_commands(view.id(), |commands| commands.redo())?
                    }
                    PanelAction::CommitEdit => {}
                }
                runtime.record_overlay(view);
            }
            Widget::Hex(editor) => {
                let editor = editor.get_mut();
                match invocation.action {
                    PanelAction::SelectAll => editor.select_all(session)?,
                    PanelAction::Undo => requests.push(HostRequest::Undo {
                        document: editor.document(),
                    }),
                    PanelAction::Redo => requests.push(HostRequest::Redo {
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
            .resource_mut::<EditorIntents>()
            .push(Some(invocation.entity), request);
    }
    result
}

pub fn focus_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    let view = world
        .get::<EditorPanelBinding>(entity)
        .and_then(|binding| binding.view);
    if let Some(view) = view {
        world
            .non_send_mut::<EditorSession>()
            .request_focus(view)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
/// A pristine default buffer may be replaced until its first edit. Undoing
/// back to empty contents does not restore that permission.
pub fn input_is_empty(world: &World, entity: Entity) -> bool {
    let Some(state) = world.get::<EditorPanelState>(entity) else {
        return false;
    };
    if let Some(session) = world.get_non_send::<EditorSession>() {
        return session.document_revision(state.document).ok() == state.initial_revision
            && session.snapshot(state.document).is_ok_and(|document| {
                document.path.is_empty() && document.bytes.is_empty() && !document.dirty
            });
    }
    world.get_resource::<EditorFrame>().is_some_and(|frame| {
        input_is_pristine(&frame.context(), state.document, state.initial_revision)
    })
}
pub fn close_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    with_editor(world, |views, runtime, session| {
        if let Some(Widget::Text(view)) = views.widgets.remove(&entity) {
            // Retirement may follow replacement of the canonical workspace.
            // The widget always drops, but it cannot mutate the new session.
            if view.workspace_id() == session.workspace_id() {
                session.detach_view(view.id());
                runtime.forget_view(view.id());
            }
        }
    });
    Ok(())
}
fn retire_views(world: &mut World) {
    if !world.contains_non_send::<EditorSession>() || !world.contains_non_send::<EditorRuntime>() {
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
    if !world.contains_non_send::<EditorSession>() || !world.contains_non_send::<EditorRuntime>() {
        return;
    }
    let commands = world
        .resource::<bed_editor_api::Registry>()
        .menus()
        .iter()
        .filter(|item| item.slot == bed_editor_api::MenuSlot::TextSelection)
        .filter_map(|item| {
            let descriptor = world
                .resource::<bed_editor_api::Registry>()
                .command(item.command)?;
            let presentation = world
                .resource::<bed_editor_api::CommandPresentations>()
                .0
                .get(descriptor.id);
            if presentation.is_some_and(|state| !state.visible) {
                return None;
            }
            Some(crate::runtime::EditorMenuCommand {
                id: descriptor.id.into(),
                label: presentation
                    .and_then(|state| state.label.clone())
                    .unwrap_or_else(|| descriptor.label.into()),
                enabled: presentation.is_none_or(|state| state.enabled),
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
    for request in requests {
        world.resource_mut::<EditorIntents>().push(None, request);
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
        NonSend<EditorSession>,
        NonSend<EditorViews>,
        ResMut<EditorIntents>,
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
        bed_document_session::ProjectCheckConfig::from_json(&value["project_check"])
    });
    runtime.restore_project_check = true;
}

pub(crate) fn register_metadata(registrar: &mut Registrar<'_>) {
    registrar.command(
        SHOW_DIAGNOSTICS_COMMAND,
        "Show Diagnostics",
        Some("diagnostics"),
    );
    registrar.command(SHOW_LSP_COMMAND, "Show Language Servers", Some("code"));
    registrar.command(
        NEW_DIAGNOSTICS_COMMAND,
        "New Diagnostics",
        Some("diagnostics"),
    );
    registrar.toolbar(NEW_DIAGNOSTICS_COMMAND);
    for (id, label) in [
        (crate::zoom::ZOOM_IN, "Zoom In"),
        (crate::zoom::ZOOM_OUT, "Zoom Out"),
        (crate::zoom::RESET_ZOOM, "Reset Zoom"),
    ] {
        registrar.command(id, label, None);
    }
    registrar.panel_options(
        TEXT_PANEL_TYPE,
        "Text editor",
        false,
        PanelPlacement::Center,
        Some("document"),
    );
    registrar.panel_options(
        HEX_PANEL_TYPE,
        "Hex editor",
        false,
        PanelPlacement::Center,
        Some("hex"),
    );
    registrar.fallback_viewer(
        TEXT_VIEWER,
        "Text editor",
        TEXT_PANEL_TYPE,
        DocumentKind::Text,
        &["bed.text"],
    );
    registrar.fallback_viewer(
        HEX_VIEWER,
        "Hex editor",
        HEX_PANEL_TYPE,
        DocumentKind::Bytes,
        &["bed.hex"],
    );
    for (kind, title, legacy) in [
        (DIAGNOSTICS_PANEL_TYPE, "Diagnostics", "diagnostics"),
        (REFERENCES_PANEL_TYPE, "References", "references"),
        (LSP_DASHBOARD_PANEL_TYPE, "Language Servers", "lsp"),
    ] {
        registrar.panel_options(kind, title, false, PanelPlacement::Center, Some(legacy));
    }
}

pub(crate) fn draw_text_view(
    ui: &Ui,
    view: &mut EditorView,
    runtime: &mut EditorRuntime,
    session: &mut EditorSession,
    config: &EditorConfig,
    requests: &mut Vec<HostRequest>,
) -> io::Result<bed_editor_ui::ViewResponse> {
    view.set_navigation_animations(config.navigation_animations);
    if let Some(command) = crate::zoom::keyboard(
        ui,
        view,
        config.options.block_input,
        &config.options.shortcuts,
    ) {
        match command {
            crate::zoom::ZOOM_IN => view.zoom_in(),
            crate::zoom::ZOOM_OUT => view.zoom_out(),
            crate::zoom::RESET_ZOOM => view.reset_zoom(),
            _ => unreachable!(),
        }
    }
    if ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS)
        && !config.options.block_input
        && !ui.io().want_text_input()
    {
        if config.options.shortcuts.take(ui, "bed.editor.toggle_wrap") {
            view.set_soft_wrap(!view.soft_wrap(config.options.soft_wrap));
        }
        if config
            .options
            .shortcuts
            .take(ui, "bed.editor.toggle_minimap")
        {
            view.set_minimap_enabled(!view.minimap_enabled(config.options.minimap_enabled));
        }
    }
    let response = view.draw(ui, session, &config.options)?;
    runtime.after_draw(ui, view, response.clone(), session, config, requests)?;
    runtime.record_overlay(view);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::ByteEdit;
    use bed_editor_api::CommandContext;
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn app() -> App {
        let mut app = App::new();
        app.world_mut().insert_non_send(EditorSession::new());
        app.world_mut().insert_non_send(EditorConfig::default());
        app.world_mut().insert_non_send(EditorRuntime::default());
        app.add_plugins((WorkbenchPlugin, EditorPlugin));
        app
    }
    fn open(app: &mut App, panel_type: &str) -> Entity {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::OpenPanel {
                panel_type: panel_type.into(),
                state: Value::Null,
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
    fn attach(
        app: &mut App,
        panel_type: &str,
        document: DocumentId,
        view: Option<bed_document_session::ViewId>,
        state: Value,
    ) -> (Entity, Box<dyn Panel>) {
        let entity = app
            .world_mut()
            .spawn(EditorPanelBinding {
                input: bed_editor_api::PanelInput::Document(document),
                document: Some(document),
                view,
                ..Default::default()
            })
            .id();
        let factory = app
            .world()
            .resource::<bed_workbench_api::Registry>()
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
            let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
        let session = app.world().non_send::<EditorSession>();
        assert!(session.view_snapshot(focused.id()).unwrap().request_focus);
        assert!(
            !session.view_snapshot(adopted.id()).unwrap().request_focus,
            "mounting an inactive presentation must not request keyboard focus"
        );
        let focus = app.world().get::<EditorPanelFocus>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(focus, entity)
            .unwrap()
            .unwrap();
        assert!(
            app.world()
                .non_send::<EditorSession>()
                .view_snapshot(adopted.id())
                .unwrap()
                .request_focus
        );
    }

    #[test]
    fn plugin_creates_independent_buffers_and_routes_actions_to_the_originating_view() {
        let mut app = app();
        let first = open(&mut app, TEXT_PANEL_TYPE);
        let second = open(&mut app, TEXT_PANEL_TYPE);
        let first_binding = app
            .world()
            .get::<EditorPanelBinding>(first)
            .unwrap()
            .clone();
        let second_binding = app
            .world()
            .get::<EditorPanelBinding>(second)
            .unwrap()
            .clone();
        assert_ne!(first_binding.document, second_binding.document);
        assert_ne!(first_binding.view, second_binding.view);
        let action = app.world().get::<EditorPanelActions>(first).unwrap().0;
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
        let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
        let session = app.world().non_send::<EditorSession>();
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
        let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
            app.world().non_send::<EditorSession>().view_ids(document),
            vec![view]
        );
        drop(legacy);
        assert_eq!(
            frontend.save(app.world())["selections"],
            json!([[1, 2, 0, 1]])
        );
        let close = app.world().get::<EditorPanelClose>(entity).unwrap().0;
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
                .non_send::<EditorSession>()
                .view_ids(document)
                .is_empty()
        );
        assert_eq!(
            app.world()
                .non_send::<EditorSession>()
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
        let old_workspace = app.world().non_send::<EditorSession>().workspace_id();
        app.world_mut().insert_non_send(EditorSession::new());
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
            app.world().non_send::<EditorSession>().workspace_id(),
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
        let session = app.world().non_send::<EditorSession>();
        assert_eq!(session.view_ids(current_document), vec![current_view]);
        assert!(session.snapshot(current_document).unwrap().bytes.is_empty());
        close_panel(app.world_mut(), current_entity).unwrap();
        assert!(
            app.world()
                .non_send::<EditorSession>()
                .view_ids(current_document)
                .is_empty()
        );
    }

    #[test]
    fn hidden_text_and_hex_persistence_observes_edits_in_sibling_views() {
        let mut app = app();
        let document = app
            .world_mut()
            .non_send_mut::<EditorSession>()
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
            .non_send_mut::<EditorSession>()
            .create_document_with_kind(&[0, 1, 2, 3, 4, 5], DocumentKind::Bytes)
            .unwrap();
        let (_, hex) = attach(
            &mut app,
            HEX_PANEL_TYPE,
            bytes,
            None,
            json!({"cursor":2,"anchor":4,"insert":true}),
        );
        let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
                .non_send::<EditorSession>()
                .view_ids(document)
                .is_empty()
        );
        assert!(
            app.world()
                .non_send::<EditorSession>()
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
        assert!(app.world().contains_non_send::<EditorSession>());
        open(&mut app, TEXT_PANEL_TYPE);
    }

    #[test]
    fn embedded_snapshot_actions_are_read_only_before_the_first_draw() {
        let mut app = app();
        let document = app
            .world_mut()
            .non_send_mut::<EditorSession>()
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
            .non_send::<EditorSession>()
            .document_revision(document)
            .unwrap();
        let action = app.world().get::<EditorPanelActions>(entity).unwrap().0;
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
                .non_send::<EditorSession>()
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
        let focus = app.world().get::<EditorPanelFocus>(entity).unwrap().0;
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
                .build(|| bed_workbench::render_panel(app.world_mut(), entity, ui).unwrap());
            assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        };
        draw(&mut context);
        draw(&mut context);
        context.io_mut().add_input_characters_utf8("x");
        draw(&mut context);
        assert_eq!(
            app.world()
                .non_send::<EditorSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            b"x"
        );
        assert!(!input_is_empty(app.world(), entity));
        let system = app.world().get::<EditorPanelActions>(entity).unwrap().0;
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
                .non_send::<EditorSession>()
                .snapshot(document)
                .unwrap()
                .bytes
                .is_empty()
        );
        assert!(!input_is_empty(app.world(), entity));
    }

    #[test]
    fn workspace_contribution_restores_project_check_through_the_prepare_system() {
        let mut app = app();
        let config = bed_document_session::ProjectCheckConfig {
            program: "custom-check".into(),
            automatic: false,
            ..Default::default()
        };
        app.world_mut()
            .non_send_mut::<EditorRuntime>()
            .project_check = Some(config.clone());
        let contribution = *app
            .world_mut()
            .query::<&bed_editor_api::EditorWorkspaceContribution>()
            .single(app.world())
            .unwrap();
        let saved = app.world_mut().run_system(contribution.save).unwrap();
        app.world_mut()
            .non_send_mut::<EditorRuntime>()
            .project_check = None;
        app.world_mut()
            .run_system_with(
                contribution.restore,
                bed_editor_api::EditorWorkspaceRestore {
                    workspace: 0,
                    root: String::new(),
                    state: Some(saved),
                },
            )
            .unwrap()
            .unwrap();
        app.update();
        assert_eq!(
            app.world()
                .non_send::<EditorSession>()
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
