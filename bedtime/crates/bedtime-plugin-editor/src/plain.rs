//! Plain editors reuse the document algorithms and widgets without IDE services.
use super::*;
use bedtime_api::documents::DocumentFrame;
use bedtime_api::features::FrontendSnapshot;
use bedtime_api::workbench::{Panel, PanelCreation, PanelResult, WorkbenchAction, register};
use bedtime_api::{
    documents::PanelBinding,
    features::HostEffects,
    workbench::{PanelActionInvocation, PanelActionResult, PanelActions, PanelClose, PanelFocus},
};
use bevy::{ecs::system::SystemState, prelude::*};
use std::collections::HashMap;

include!("panels.rs");

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        register_settings(app.world_mut());
        if !app.is_plugin_added::<bedtime_workbench::WorkbenchPlugin>() {
            app.add_plugins(bedtime_workbench::WorkbenchPlugin);
        }

        app.init_non_send::<DocumentSession>()
            .init_non_send::<EditorViews>()
            .init_non_send::<EditorConfig>()
            .init_non_send::<bedtime_editor_widgets::extensions::EditorExtensions>()
            .init_non_send::<bedtime_editor_widgets::source_intents::SourceRequests>()
            .init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .add_message::<bedtime_api::features::HostRequest>()
            .add_systems(
                PreUpdate,
                configure_preferences.in_set(bedtime_api::features::FeatureSystems::Prepare),
            )
            .add_systems(Last, retire_views);
        let actions = app.world_mut().register_system(panel_action);
        let clipboard = app.world_mut().register_system(clipboard_operation);
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
        app.insert_resource(Systems {
            actions,
            clipboard,
            focus,
            close,
        });
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, register_metadata)
            .expect("editor contributions are unique");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(TEXT_PANEL_TYPE, "Text editor", create_text);
            registrar.panel(HEX_PANEL_TYPE, "Hex editor", create_hex);
            registrar.panel_icon(TEXT_PANEL_TYPE, "text");
            registrar.panel_icon(HEX_PANEL_TYPE, "hex");
            for action in bedtime_editor_widgets::shortcuts::actions()
                .into_iter()
                .chain(bedtime_editor_widgets::shortcuts::hex_actions())
                .filter(|action| action.owner == MODULE_ID)
                .filter(|action| {
                    !matches!(
                        action.id.as_str(),
                        "bedtime.editor.toggle_comment"
                            | "bedtime.editor.definition"
                            | "bedtime.editor.references"
                            | "bedtime.editor.symbol_info"
                            | "bedtime.editor.completion"
                            | "bedtime.editor.format_document"
                            | "bedtime.editor.rename"
                            | "bedtime.editor.quick_fix"
                    )
                })
            {
                registrar.shortcut_action(action);
            }
        })
        .expect("EditorPlugin is installed once");
    }
}

fn register_metadata(registrar: &mut FeatureRegistrar<'_>) {
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
}

fn create_text(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    create_editor(world, creation, false)
}

pub struct EditorFrontend {
    entity: Entity,
}
fn new_frontend(entity: Entity) -> Box<dyn Panel> {
    Box::new(EditorFrontend { entity })
}

impl Panel for EditorFrontend {
    fn title(&self, world: &World) -> String {
        let state = world
            .get::<EditorPanelState>(self.entity)
            .expect("editor owns panel state");
        let Ok(snapshot) = world.non_send::<DocumentSession>().snapshot(state.document) else {
            return "Closed".into();
        };
        let name = if snapshot.path.is_empty() {
            "Untitled".into()
        } else {
            Path::new(&snapshot.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        if state.hex {
            format!("{name} [Hex]")
        } else {
            name
        }
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        actions: &mut Vec<WorkbenchAction>,
        shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let mut config = world.non_send::<EditorConfig>().clone();
        if let Some(settings) = world.get_resource::<bedtime_api::settings::SettingsSnapshot>() {
            apply_settings(&mut config, settings);
        }
        config.options.shortcuts = shortcuts.with_scope(bedtime_keybindings::Scope::panel(
            bedtime_keybindings::EDITOR_SCOPE,
        ));
        config.options.extensions = world
            .non_send::<bedtime_editor_widgets::extensions::EditorExtensions>()
            .clone();
        let before = world
            .get::<EditorPanelState>(self.entity)
            .and_then(|state| {
                world
                    .non_send::<DocumentSession>()
                    .document_revision(state.document)
                    .ok()
                    .map(|revision| (state.document, revision))
            });
        let mut requests = Vec::new();
        let expired_focus = unfocused_view(world, self.entity);
        let result = with_editor(world, |views, session| {
            if let Some(view) = expired_focus {
                // Another logical panel receiving focus expires a request
                // deferred by a popup, while ordinary clicks remain enabled.
                session.with_view(view, |editor| editor.view_mut().request_focus = false)?;
            }
            match views
                .widgets
                .get_mut(&self.entity)
                .expect("editor owns a widget")
            {
                Widget::Text(view) => {
                    draw_text_view(ui, view, session, &config, &mut requests).map(|_| ())
                }
                Widget::Hex(editor) => {
                    editor.get_mut().shortcuts = config.options.shortcuts.clone();
                    editor.get_mut().draw(ui, session)
                }
            }
        });
        if before.is_some_and(|(document, revision)| {
            world
                .non_send::<DocumentSession>()
                .document_revision(document)
                .is_ok_and(|current| current != revision)
        }) {
            actions.push(WorkbenchAction::KeepPanel(self.entity));
        }
        emit(world, self.entity, requests);
        if let Err(error) = result {
            ui.text_disabled(error.to_string());
        }
    }

    fn save(&self, world: &World) -> Value {
        save_embedded(world, self.entity)
    }
}

fn with_editor<R>(
    world: &mut World,
    callback: impl FnOnce(&mut EditorViews, &mut DocumentSession) -> R,
) -> R {
    let mut state =
        SystemState::<(NonSendMut<EditorViews>, NonSendMut<DocumentSession>)>::new(world);
    let (mut views, mut session) = state
        .get_mut(world)
        .expect("editor owners are installed together");
    callback(&mut views, &mut session)
}

fn emit(world: &mut World, entity: Entity, requests: Vec<HostAction>) {
    if requests.is_empty() {
        return;
    }
    let session = world.non_send::<DocumentSession>().session_id();
    let epoch = world
        .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
        .0;
    let owner = bedtime_api::workbench::panel_owner(world, entity);
    let area = owner
        .and_then(|owner| world.get::<bedtime_api::workbench::PanelPresentation>(owner))
        .and_then(|panel| {
            world
                .resource::<bedtime_workbench::WorkbenchLayout>()
                .tiling
                .area_for(panel.id)
        })
        .unwrap_or_else(|| {
            world
                .resource::<bedtime_workbench::WorkbenchLayout>()
                .tiling
                .layout
                .largest_area()
        });
    for request in requests {
        world.write_message(bedtime_api::features::HostRequest::capture(
            session,
            Some(entity),
            request,
            area,
            epoch,
        ));
    }
}

fn draw_text_view(
    ui: &Ui,
    view: &mut EditorView,
    session: &mut DocumentSession,
    config: &EditorConfig,
    requests: &mut Vec<HostAction>,
) -> io::Result<bedtime_editor_widgets::ViewResponse> {
    view.set_navigation_animations(config.navigation_animations);
    if ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS)
        && !config.options.block_input
        && !ui.io().want_text_input()
    {
        for (id, operation) in [
            (bedtime_keybindings::ZOOM_IN, 0),
            (bedtime_keybindings::ZOOM_OUT, 1),
            (bedtime_keybindings::RESET_ZOOM, 2),
        ] {
            if config.options.shortcuts.take(ui, id) {
                match operation {
                    0 => view.zoom_in(),
                    1 => view.zoom_out(),
                    _ => view.reset_zoom(),
                }
            }
        }
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
    let response = view.draw(ui, session, &config.options)?;
    let unnamed = session.snapshot(response.document)?.path.is_empty();
    requests.extend(response.actions.iter().map(|action| match action {
        bedtime_editor_widgets::EditorHostAction::Open => {
            HostAction::OpenFileDialog { viewer: None }
        }
        bedtime_editor_widgets::EditorHostAction::Save if unnamed => HostAction::SaveAsDialog {
            document: response.document,
        },
        bedtime_editor_widgets::EditorHostAction::Save => HostAction::Save {
            document: response.document,
        },
        bedtime_editor_widgets::EditorHostAction::SaveAs => HostAction::SaveAsDialog {
            document: response.document,
        },
        bedtime_editor_widgets::EditorHostAction::Undo => HostAction::Undo {
            document: response.document,
        },
        bedtime_editor_widgets::EditorHostAction::Redo => HostAction::Redo {
            document: response.document,
        },
    }));
    Ok(response)
}

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
    let mut requests = Vec::new();
    let result = with_editor(world, |views, session| {
        draw_text_view(
            ui,
            views
                .text_mut(entity)
                .expect("embedded editor owns a text view"),
            session,
            &config,
            &mut requests,
        )
    });
    emit(world, entity, requests);
    result
}

fn panel_action(In(invocation): In<PanelActionInvocation>, world: &mut World) -> PanelActionResult {
    let mut requests = Vec::new();
    let result = with_editor(world, |views, session| -> io::Result<bool> {
        let Some(widget) = views.widgets.get_mut(&invocation.entity) else {
            return Ok(false);
        };
        let (document, read_only) = match widget {
            Widget::Text(view) => (view.document_id(), view.read_only()),
            Widget::Hex(editor) => (editor.get_mut().document(), false),
        };
        match invocation.action {
            PanelAction::Undo if !read_only => requests.push(HostAction::Undo { document }),
            PanelAction::Redo if !read_only => requests.push(HostAction::Redo { document }),
            PanelAction::Undo | PanelAction::Redo | PanelAction::CommitEdit => {}
            PanelAction::SelectAll => match widget {
                Widget::Text(view) => {
                    session.with_commands(view.id(), |commands| commands.select_all())?
                }
                Widget::Hex(editor) => editor.get_mut().select_all(session)?,
            },
            PanelAction::Find | PanelAction::GoToLine => {
                let Widget::Text(view) = widget else {
                    return Ok(false);
                };
                if invocation.action == PanelAction::Find {
                    view.open_find(session)?;
                } else {
                    view.open_line_jump();
                }
            }
        }
        Ok(true)
    })
    .map_err(|error| error.to_string());
    // Registered actions are synchronous effects of the current command.
    for request in requests {
        world
            .resource_mut::<HostEffects>()
            .push(Some(invocation.entity), request);
    }
    result
}

fn clipboard_operation(
    In(invocation): In<bedtime_api::workbench::ClipboardInvocation>,
    world: &mut World,
) -> bedtime_api::workbench::ClipboardResult {
    use bedtime_api::workbench::ClipboardAction;
    with_editor(world, |views, session| -> io::Result<Option<String>> {
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
            Widget::Hex(editor) => match invocation.action {
                ClipboardAction::Copy => editor.get_mut().copy_text(session).map(Some),
                ClipboardAction::Cut => editor.get_mut().cut_text(session).map(Some),
                ClipboardAction::Paste(text) | ClipboardAction::PasteFromClipboard(Some(text)) => {
                    editor.get_mut().paste_text(session, &text).map(|_| None)
                }
                ClipboardAction::PasteFromClipboard(None) => Ok(None),
                ClipboardAction::SelectAll => editor.get_mut().select_all(session).map(|_| None),
            },
        }
    })
    .map_err(|error| error.to_string())
}

pub fn close_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    with_editor(world, |views, session| {
        if let Some(Widget::Text(view)) = views.widgets.remove(&entity)
            && view.session_id() == session.session_id()
        {
            session.detach_view(view.id());
        }
    });
    Ok(())
}

fn retire_views(world: &mut World) {
    let dead = world
        .non_send::<EditorViews>()
        .widgets
        .keys()
        .filter(|entity| !world.entities().contains(**entity))
        .copied()
        .collect::<Vec<_>>();
    for entity in dead {
        close_panel(world, entity).expect("retirement releases only the owned presentation");
    }
}

/// Plain editors have no application feature state beyond their panel snapshots.
pub fn save_workspace(_world: &World) -> Value {
    Value::Null
}
pub fn restore_workspace(_world: &mut World, _state: Option<&Value>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_messages_freeze_source_session_layout_and_order_among_panel_requests() {
        let mut app = App::new();
        app.add_plugins(EditorPlugin);
        let world = app.world_mut();
        let source = bedtime_workbench::open_panel(
            world,
            json!({"panel_type":TEXT_PANEL_TYPE,"state":null}),
            None,
            None,
        )
        .unwrap();
        let session = world.non_send::<DocumentSession>().session_id();
        let document = world.get::<PanelBinding>(source).unwrap().document.unwrap();
        let area = world
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .tiling
            .area_for(
                world
                    .get::<bedtime_api::workbench::PanelPresentation>(source)
                    .unwrap()
                    .id,
            )
            .unwrap();
        let epoch = world
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
            .0;
        emit(world, source, vec![HostAction::Undo { document }]);
        world
            .resource_mut::<bedtime_workbench::WorkbenchRequests>()
            .push(WorkbenchAction::KeepPanel(source));
        let panel_sequence = world
            .resource::<bedtime_workbench::WorkbenchRequests>()
            .next_sequence()
            .unwrap();
        bedtime_workbench::advance_layout_epoch(world);
        world.insert_non_send(DocumentSession::new());
        let replacement = world.non_send::<DocumentSession>().session_id();
        emit(world, source, vec![HostAction::Redo { document }]);
        let mut cursor =
            bevy::ecs::message::MessageCursor::<bedtime_api::features::HostRequest>::default();
        let requests = cursor
            .read(world.resource::<Messages<bedtime_api::features::HostRequest>>())
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].source(), Some(source));
        assert_eq!(requests[0].session_id(), session);
        assert_eq!(requests[0].area(), area);
        assert_eq!(requests[0].layout_epoch(), epoch);
        assert!(requests[0].sequence() < panel_sequence);
        assert!(panel_sequence < requests[1].sequence());
        assert_eq!(requests[1].session_id(), replacement);
        assert_ne!(replacement, session);
        assert_eq!(requests[1].layout_epoch(), epoch + 1);
        assert!(world.resource_mut::<HostEffects>().take().is_empty());
    }
}
