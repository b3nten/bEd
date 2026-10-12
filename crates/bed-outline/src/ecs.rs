//! Outline preparation belongs to the world; each surface owns its tree widgets.
use crate::{OPEN_COMMAND, PANEL_ID, PLUGIN_ID, presentation, register_metadata, sync_outline};
use bed_editor_api::{
    EditorFeatureSystems, EditorFrame, EditorIntents, HostRequest, Registry as EditorRegistry,
};
use bed_highlight::outline::OutlineService;
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

#[derive(Component)]
pub struct StructureSurface;

#[derive(Default)]
pub struct StructureOutline(pub Option<OutlineService>);

pub struct OutlinePlugin;
impl Plugin for OutlinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<EditorRegistry>()
            .init_non_send::<StructureOutline>()
            .add_systems(PreUpdate, prepare.in_set(EditorFeatureSystems::Prepare))
            .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        app.world_mut()
            .resource_mut::<EditorRegistry>()
            .register_with(PLUGIN_ID, register_metadata)
            .expect("structure declarations are unique");
        bed_workbench_api::register(app.world_mut(), PLUGIN_ID, |registrar| {
            registrar.panel(PANEL_ID, "Structure", create);
            registrar.command(OPEN_COMMAND, "Structure", open);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("OutlinePlugin is installed once");
    }
}

fn shutdown(mut exit: MessageReader<AppExit>, mut outline: NonSendMut<StructureOutline>) {
    if exit.read().next().is_some() {
        outline.0 = None;
    }
}

fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
fn create(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(StructureSurface);
    Ok(Box::new(StructureFrontend {
        entity: creation.entity,
        tree: presentation::StructurePanel::default(),
    }))
}
fn prepare(
    frame: Res<EditorFrame>,
    live: Query<Entity, With<StructureSurface>>,
    mut outline: NonSendMut<StructureOutline>,
    mut intents: ResMut<EditorIntents>,
    mut commands: Commands,
) {
    sync_outline(&mut outline.0, &frame.context(), !live.is_empty());
    if outline.0.as_ref().is_some_and(OutlineService::updating) {
        intents.push(None, HostRequest::Invalidate);
    }
    let readiness = match outline.0.as_ref() {
        Some(outline) if outline.updating() => bed_editor_api::EditorPanelReadiness::Pending,
        Some(outline) => match outline.result().map(|result| &result.status) {
            Some(bed_highlight::outline::OutlineStatus::Failed(error)) => {
                bed_editor_api::EditorPanelReadiness::Failed(error.clone())
            }
            _ => bed_editor_api::EditorPanelReadiness::Ready,
        },
        None => bed_editor_api::EditorPanelReadiness::Ready,
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
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let frame = world.resource::<EditorFrame>().clone();
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
                .resource_mut::<EditorIntents>()
                .push(Some(self.entity), request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_editing::{
        editor_state::{DocumentKind, EditorState},
        identity::DocumentId,
    };
    use bed_editor_api::PluginDocument;
    use bed_workbench::{WorkbenchPlugin, WorkbenchRequests, capture_command};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn world_worker_uses_unsaved_source_and_releases_outline_after_retirement() {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, OutlinePlugin));
        let command = capture_command(app.world(), OPEN_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::Invoke(command));
        app.update();
        let mut state = EditorState::default();
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
            .resource_mut::<EditorFrame>()
            .documents
            .push(document);
        app.world_mut()
            .resource_mut::<EditorFrame>()
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
