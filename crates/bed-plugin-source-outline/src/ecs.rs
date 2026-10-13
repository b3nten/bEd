//! Outline preparation belongs to the world; each surface owns its tree widgets.
use crate::{OPEN_COMMAND, PANEL_ID, PLUGIN_ID, presentation, register_metadata, sync_outline};
use bed_api::documents::DocumentFrame;
use bed_api::features::FrontendSnapshot;
use bed_api::features::{FeatureRegistry, FeatureSystems, HostAction, HostEffects};
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction,
};
use bed_highlight::outline::OutlineService;
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

#[derive(Component)]
pub struct StructureSurface;

#[derive(Default)]
pub struct StructureOutline(pub Option<OutlineService>);

pub struct SourceOutlinePlugin;

impl Plugin for SourceOutlinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<FeatureRegistry>()
            .init_non_send::<StructureOutline>()
            .add_systems(PreUpdate, prepare.in_set(FeatureSystems::Prepare))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<FeatureRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("structure declarations are unique");
        bed_api::workbench::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Structure", create);
            registrar.command(OPEN_COMMAND, "Structure", open);
            registrar.menu(OPEN_COMMAND);
            registrar.command_icon(OPEN_COMMAND, "bed.core.structure");
            registrar.panel_icon(PANEL_ID, "bed.core.structure");
            registrar.toolbar(OPEN_COMMAND);
        })
        .expect("SourceOutlinePlugin is installed once");
    }
}

fn shutdown(mut exit: MessageReader<AppExit>, mut outline: NonSendMut<StructureOutline>) {
    if exit.read().next().is_some() {
        outline.0 = None;
    }
}

fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Structure", "Focus a text document to view its outline. Expand entries to reveal nested symbols."),
        ("Navigate", "Select a symbol to jump to its location in the document."),
    ]));
    world.entity_mut(creation.entity).insert(StructureSurface);
    Ok(Box::new(StructureFrontend {
        entity: creation.entity,
        tree: presentation::StructurePanel::default(),
    }))
}

fn prepare(
    frame: Res<DocumentFrame>,
    live: Query<Entity, With<StructureSurface>>,
    mut outline: NonSendMut<StructureOutline>,
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

    sync_outline(&mut outline.0, &frame.context(), !live.is_empty());
    if outline.0.as_ref().is_some_and(OutlineService::updating) {
        intents.push(None, HostAction::Invalidate);
    }
    let readiness = match outline.0.as_ref() {
        Some(outline) if outline.updating() => bed_api::workbench::PanelReadiness::Pending,
        Some(outline) => match outline.result().map(|result| &result.status) {
            Some(bed_highlight::outline::OutlineStatus::Failed(error)) => {
                bed_api::workbench::PanelReadiness::Failed(error.clone())
            }
            _ => bed_api::workbench::PanelReadiness::Ready,
        },
        None => bed_api::workbench::PanelReadiness::Ready,
    };
    for entity in &live {
        commands.entity(entity).insert(readiness.clone());
    }
}

struct StructureFrontend {
    entity: Entity,
    tree: presentation::StructurePanel,
}

impl Panel for StructureFrontend {
    fn title(&self, _: &World) -> String {
        "Structure".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let frame = FrontendSnapshot::capture(world);
        let host = frame.context();
        let request = {
            let mut outline = world.non_send_mut::<StructureOutline>();
            sync_outline(&mut outline.0, &host, true);
            outline
                .0
                .as_ref()
                .and_then(|outline| self.tree.draw(ui, outline, host.animations))
                .and_then(|jump| crate::navigation_request(jump, &host))
        };
        if let Some(request) = request {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.entity), request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::documents::PluginDocument;
    use bed_editing::{
        document_state::{DocumentKind, DocumentState},
        identity::DocumentId,
    };
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests, capture_command};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn world_worker_uses_unsaved_source_and_releases_outline_after_retirement() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, SourceOutlinePlugin));
        let command = capture_command(app.world(), OPEN_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::Invoke(command));
        app.update();
        let mut state = DocumentState::default();
        state.set_from_bytes(b"fn unsaved_outline() {}\n");
        let document = PluginDocument {
            id: DocumentId(8),
            path: "/fixture.rs".into(),
            kind: DocumentKind::Text,
            language_id: "rust".into(),
            revision: (3, 9),
            dirty: true,
            bytes: Arc::from(&b"fn unsaved_outline() {}\n"[..]),
            text: Some(state.snapshot()),
        };
        app.world_mut()
            .resource_mut::<DocumentFrame>()
            .documents
            .push(document);
        app.world_mut()
            .resource_mut::<DocumentFrame>()
            .active_document = Some(DocumentId(8));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            let outline = app.world().non_send::<StructureOutline>();
            if outline
                .0
                .as_ref()
                .is_some_and(|outline| outline.result().is_some())
            {
                break;
            }
            assert!(Instant::now() < deadline, "outline worker did not complete");
            std::thread::sleep(Duration::from_millis(2));
        }
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<StructureSurface>>()
            .single(app.world())
            .unwrap();
        app.world_mut().despawn(entity);
        app.update();
        assert!(app.world().non_send::<StructureOutline>().0.is_none());
    }
}
