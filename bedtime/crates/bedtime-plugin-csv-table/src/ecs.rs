//! The CSV feature runs in the host world. Frontends retain only their entity.
use crate::{
    CsvPanel, OPEN_COMMAND, OPEN_FILE_COMMAND, PANEL_ID, PLUGIN_ID, VIEWER_ID, register_metadata,
    supported_path,
};
use bedtime_api::documents::DocumentFrame;
use bedtime_api::features::FrontendSnapshot;
use bedtime_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction,
};
use bedtime_api::{
    documents::{PanelBinding, PanelEditResult, PanelEditSystem, PanelEdits, panel_viewer_menu},
    features::{FeatureRegistry, FeatureSystems, HostAction, HostEffects},
    workbench::{PanelActionInvocation, PanelActionResult, PanelActionSystem, PanelActions},
};
use bedtime_editing::identity::DocumentId;
use bevy::{ecs::system::SystemState, prelude::*};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvDocument(pub DocumentId);

/// Main-thread workers and their table presentations belong to the app.
#[derive(Default)]
pub struct CsvStates(HashMap<Entity, CsvPanel>);

impl CsvStates {
    pub fn get(&self, entity: Entity) -> Option<&CsvPanel> {
        self.0.get(&entity)
    }

    /// Set up a real in-progress cell edit for cross-crate host regression tests.
    #[cfg(feature = "test-support")]
    pub fn set_cell_draft(
        &mut self,
        entity: Entity,
        row: usize,
        column: usize,
        text: String,
    ) -> Result<(), String> {
        self.0
            .get_mut(&entity)
            .ok_or("CSV presentation is missing")?
            .set_cell_draft(row, column, text)
    }

    #[cfg(feature = "test-support")]
    pub fn draft_text(&self, entity: Entity) -> Option<&str> {
        self.0.get(&entity)?.draft_text()
    }

    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, entity: Entity) -> Option<&mut CsvPanel> {
        self.0.get_mut(&entity)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CsvSystems {
    Poll,
}

pub struct CsvTablePlugin;

#[derive(Resource)]
struct CsvOperationHandlers {
    action: PanelActionSystem,
    clipboard: bedtime_api::workbench::PanelClipboardSystem,
    edit: PanelEditSystem,
}

impl Plugin for CsvTablePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<FeatureRegistry>()
            .init_non_send::<CsvStates>()
            .add_systems(
                PreUpdate,
                poll_csv
                    .in_set(CsvSystems::Poll)
                    .in_set(FeatureSystems::Prepare),
            )
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<FeatureRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("CSV declarations must register exactly once");
        let action = app.world_mut().register_system(csv_action);
        let edit = app.world_mut().register_system(csv_edit_result);
        let clipboard = app.world_mut().register_system(csv_clipboard);
        app.insert_resource(CsvOperationHandlers {
            action,
            edit,
            clipboard,
        });
        bedtime_api::workbench::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "CSV Table", create_csv_panel);
            for action in crate::shortcut_actions() {
                registrar.shortcut_action(action);
            }
            registrar.command(OPEN_COMMAND, "Open CSV/TSV…", csv_command);
            registrar.command(OPEN_FILE_COMMAND, "Open in CSV Table", csv_command);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("CSV systems must register exactly once");
    }
}

fn shutdown(mut exits: MessageReader<bevy::app::AppExit>, mut states: NonSendMut<CsvStates>) {
    if exits.read().next().is_some() {
        states.0.clear();
    }
}

fn create_csv_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bedtime_api::workbench::PanelDocumentation::new(&[
        ("Cells", "Select cells with the mouse or arrow keys. Double-click or use Edit Cell to edit; Tab moves between cells."),
        ("Table", "Use column sorting, filters, and search to find rows. Copy/paste supports tabular selections; edits share the text document’s undo history."),
    ]).shortcut_actions(&["bedtime.navigation.move_left", "bedtime.navigation.move_right", "bedtime.navigation.move_up", "bedtime.navigation.move_down", "bedtime.navigation.select_left", "bedtime.navigation.select_right", "bedtime.navigation.select_up", "bedtime.navigation.select_down", "bedtime.edit.find", "bedtime.edit.copy", "bedtime.edit.cut", "bedtime.edit.paste", "bedtime.edit.select_all", "bedtime.edit.undo", "bedtime.edit.redo"]));
    if world
        .get::<PanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("CSV Table requires an editor document".into());
    }
    let document = world
        .get::<PanelBinding>(creation.entity)
        .and_then(|binding| binding.input.document().or(binding.document))
        .or_else(|| creation.state["document"].as_u64().map(DocumentId));
    if let Some(document) = document {
        let operations = world.resource::<CsvOperationHandlers>();
        let action_system = operations.action;
        let edit_system = operations.edit;
        let clipboard_system = operations.clipboard;
        world.entity_mut(creation.entity).insert((
            CsvDocument(document),
            crate::status::items(),
            PanelActions(action_system),
            bedtime_api::workbench::PanelClipboard(clipboard_system),
            PanelEdits(edit_system),
        ));
        world
            .non_send_mut::<CsvStates>()
            .0
            .insert(creation.entity, CsvPanel::new(document, &creation.state));
    }
    Ok(Box::new(CsvFrontend {
        entity: creation.entity,
    }))
}

fn poll_csv(
    frame: Res<DocumentFrame>,
    mut states: NonSendMut<CsvStates>,
    live: Query<&CsvDocument>,
    mut intents: ResMut<HostEffects>,
    mut commands: Commands,
) {
    let frame = FrontendSnapshot {
        documents: frame.documents.clone(),
        active_document: frame.active_document,
        active_view: frame.active_view,
        session_id: frame.session_id,
        diagnostics: frame.diagnostics.clone(),
        ..Default::default()
    };

    // Removing a panel invalidates its bounded worker and releases its table.
    states.0.retain(|entity, _| live.get(*entity).is_ok());
    let host = frame.context();
    for (entity, panel) in &mut states.0 {
        panel.sync(&host);
        commands.entity(*entity).insert(match panel.error() {
            Some(error) => bedtime_api::workbench::PanelReadiness::Failed(error.into()),
            None if panel.is_ready() => bedtime_api::workbench::PanelReadiness::Ready,
            None => bedtime_api::workbench::PanelReadiness::Pending,
        });
    }
    if states.0.values().any(CsvPanel::preparing) {
        intents.push(None, HostAction::Invalidate);
    }
}

fn csv_command(
    In(invocation): In<CommandInvocation>,
    frame: Res<DocumentFrame>,
    bindings: Query<&PanelBinding>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let request = match invocation.command.as_str() {
        OPEN_COMMAND => Some(HostAction::OpenFileDialog {
            viewer: Some(VIEWER_ID.into()),
        }),
        OPEN_FILE_COMMAND => invocation.arguments["path"]
            .as_str()
            .or_else(|| {
                let entity = invocation.source?;
                let binding = bindings.get(entity).ok()?;
                let document = binding.input.document().or(binding.document)?;
                frame
                    .documents
                    .iter()
                    .find(|entry| entry.id == document)
                    .map(|document| document.path.as_str())
            })
            .filter(|path| supported_path(path))
            .map(|path| HostAction::OpenFile {
                path: path.to_owned(),
                viewer: Some(VIEWER_ID.into()),
            }),
        _ => return Err(format!("Unknown CSV command: {}", invocation.command)),
    };
    if let Some(request) = request {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}

fn csv_clipboard(
    In(invocation): In<bedtime_api::workbench::ClipboardInvocation>,
    mut states: NonSendMut<CsvStates>,
    mut intents: ResMut<HostEffects>,
) -> bedtime_api::workbench::ClipboardResult {
    let panel = states
        .0
        .get_mut(&invocation.entity)
        .ok_or("CSV panel is closed")?;
    let mut requests = Vec::new();
    let text = panel.clipboard_operation(invocation.action, &mut requests);
    for request in requests {
        intents.push(Some(invocation.entity), request);
    }
    Ok(text)
}

fn csv_action(
    In(invocation): In<PanelActionInvocation>,
    frame: Res<DocumentFrame>,
    mut states: NonSendMut<CsvStates>,
    live: Query<&CsvDocument>,
    mut intents: ResMut<HostEffects>,
) -> PanelActionResult {
    let frame = FrontendSnapshot {
        documents: frame.documents.clone(),
        active_document: frame.active_document,
        active_view: frame.active_view,
        session_id: frame.session_id,
        diagnostics: frame.diagnostics.clone(),
        ..Default::default()
    };

    if live.get(invocation.entity).is_err() {
        return Ok(false);
    }
    let Some(panel) = states.0.get_mut(&invocation.entity) else {
        return Ok(false);
    };
    let mut requests = Vec::new();
    let handled = panel.apply_action(invocation.action, &frame.context(), &mut requests)?;
    for request in requests {
        intents.push(Some(invocation.entity), request);
    }
    Ok(handled)
}

fn csv_edit_result(
    In(ack): In<PanelEditResult>,
    mut states: NonSendMut<CsvStates>,
    live: Query<&CsvDocument>,
) {
    if live.get(ack.entity).is_ok()
        && let Some(panel) = states.0.get_mut(&ack.entity)
    {
        panel.acknowledge_edit(ack.token, ack.result);
    }
}

struct CsvFrontend {
    entity: Entity,
}

impl Panel for CsvFrontend {
    fn title(&self, world: &World) -> String {
        world.non_send::<CsvStates>().get(self.entity).map_or_else(
            || "CSV Table".into(),
            |panel| panel.panel_title(&FrontendSnapshot::capture(world).context()),
        )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
    ) {
        let menu = panel_viewer_menu(world, self.entity, VIEWER_ID);
        let frame = FrontendSnapshot::capture(world);
        let mut state = SystemState::<(NonSendMut<CsvStates>, ResMut<HostEffects>)>::new(world);
        let (mut states, mut intents) = state
            .get_mut(world)
            .expect("CSV plugin initialized its presentation resources");
        let mut requests = Vec::new();
        if let Some(panel) = states.0.get_mut(&self.entity) {
            let mut host = frame.context();
            host.viewer_menu = menu.as_ref();
            panel.shortcuts = _shortcuts.clone();
            panel.draw_prepared(ui, &host, &mut requests);
        } else {
            ui.text_wrapped("Open a CSV or TSV file to view its table.");
            if ui.button("Open CSV/TSV…") {
                requests.push(HostAction::OpenFileDialog {
                    viewer: Some(VIEWER_ID.into()),
                });
            }
        }
        // Publish only after drawing succeeds; panel quarantine discards partial
        // intentions if its frontend panics.
        for request in requests {
            intents.push(Some(self.entity), request);
        }
    }

    fn save(&self, world: &World) -> Value {
        world
            .non_send::<CsvStates>()
            .get(self.entity)
            .map_or(Value::Null, CsvPanel::saved_state)
    }
}
