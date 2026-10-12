//! The same repository controller serves source widgets, sidebar and diff views.
use super::*;
use bed_editor_api::{
    CommandPresentation, CommandPresentations, EditorDocumentEvent, EditorFeatureReply,
    EditorFeatureSystems, EditorFrame, EditorIntents, EditorPanelClose,
    EditorWorkspaceContribution, EditorWorkspaceRestore,
};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, register,
};
use bevy::{ecs::system::SystemState, prelude::*};
use std::collections::HashMap;

#[derive(Default)]
pub struct GitState(pub(crate) GitController);
#[derive(Default)]
pub struct GitPanels(pub(crate) HashMap<Entity, panels::GitPanel>);
#[derive(Resource)]
struct Close(bed_editor_api::EditorPanelLifecycleSystem);
pub struct GitPlugin;
impl Plugin for GitPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .init_non_send::<bed_document_session::EditorSession>();
        app.world_mut().init_non_send::<GitState>();
        app.world_mut().init_non_send::<GitPanels>();
        app.init_non_send::<bed_editor_ui::extensions::EditorExtensions>()
            .init_non_send::<bed_editor_ui::source_intents::SourceRequests>()
            .init_resource::<EditorFrame>()
            .init_resource::<EditorIntents>()
            .init_resource::<bed_editor_api::Registry>()
            .init_resource::<CommandPresentations>()
            .add_message::<EditorFeatureReply>()
            .add_message::<EditorDocumentEvent>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.command(SHOW_COMMAND, "Git Changes", Some("git"));
                registrar.command(REFRESH_COMMAND, "Refresh Git Status", None);
                registrar.command(DIFF_COMMAND, "Show File Diff", None);
                registrar.menu(MenuSlot::Application, SHOW_COMMAND);
                registrar.menu(MenuSlot::File, DIFF_COMMAND);
                registrar.menu(MenuSlot::TextSelection, DIFF_COMMAND);
                registrar.toolbar(SHOW_COMMAND);
                registrar.panel_options(PANEL_ID, "Git", true, PanelPlacement::Sidebar, None);
                registrar.panel(DIFF_PANEL_ID, "Git diff");
            })
            .expect("Git metadata is registered once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Git", create_panel);
            registrar.panel(DIFF_PANEL_ID, "Git diff", crate::diff_feature::create_panel);
            registrar.command(SHOW_COMMAND, "Git Changes", command);
            registrar.command(REFRESH_COMMAND, "Refresh Git Status", command);
            registrar.command(DIFF_COMMAND, "Show File Diff", command);
            registrar.command(
                "bed.git.source-action",
                "Apply Git source action",
                source_action,
            );
        })
        .expect("GitPlugin is installed once");
        crate::status::install(app);
        let close = app.world_mut().register_system(close_panel);
        app.insert_resource(Close(close));
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_workspace);
        app.world_mut().spawn(EditorWorkspaceContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        app.add_systems(
            PreUpdate,
            (replies, document_events, prepare)
                .chain()
                .in_set(EditorFeatureSystems::Prepare),
        )
        .add_systems(Last, shutdown.after(bevy::app::OnAppExitSystems));
        crate::diff_feature::install(app);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Bevy system parameters make canonical readers and writers explicit"
)]
pub(crate) fn prepare(
    mut controller: NonSendMut<GitState>,
    mut panels: NonSendMut<GitPanels>,
    alive: Query<Entity>,
    documents: Option<NonSendMut<EditorSession>>,
    filesystem: Option<NonSend<bed_remote::WorkspaceFilesystem>>,
    frame: Res<EditorFrame>,
    mut intents: ResMut<EditorIntents>,
    mut source: NonSendMut<bed_editor_ui::extensions::EditorExtensions>,
    mut commands: Commands,
    mut presentations: ResMut<CommandPresentations>,
) {
    panels.0.retain(|entity, _| alive.contains(*entity));
    let Some(mut documents) = documents else {
        return;
    };
    let mut requests = Vec::new();
    let controller = &mut controller.0;
    if let Err(error) = controller.tick_native(
        &mut documents,
        &frame.project_root,
        filesystem.as_deref(),
        &mut requests,
    ) {
        controller.error = Some(error.to_string());
    }
    for entity in panels.0.keys() {
        commands.entity(*entity).insert(match &controller.error {
            Some(error) => bed_editor_api::EditorPanelReadiness::Failed(error.clone()),
            None if controller.status.is_some() && !controller.busy() => {
                bed_editor_api::EditorPanelReadiness::Ready
            }
            None => bed_editor_api::EditorPanelReadiness::Pending,
        });
    }
    presentations.0.insert(
        REFRESH_COMMAND.into(),
        CommandPresentation {
            label: controller.busy().then(|| "Refreshing Git…".into()),
            enabled: !controller.busy(),
            visible: true,
        },
    );
    presentations.0.insert(
        DIFF_COMMAND.into(),
        CommandPresentation {
            enabled: !controller.busy(),
            ..Default::default()
        },
    );
    source.clear_git();
    for document in &frame.documents {
        if let Ok(Some(presentation)) = controller.source_presentation(&documents, document.id) {
            source.set_source_git(document.id, document.revision, presentation);
        }
    }
    for request in requests {
        intents.push(None, request);
    }
}
fn replies(mut replies: MessageReader<EditorFeatureReply>, mut controller: NonSendMut<GitState>) {
    for reply in replies.read() {
        if let EditorFeatureReply::Save {
            recipient,
            token,
            result,
        } = reply
            && recipient == MODULE_ID
        {
            controller.0.save_result(*token, result.clone());
        }
    }
}
fn document_events(
    mut events: MessageReader<EditorDocumentEvent>,
    mut controller: NonSendMut<GitState>,
) {
    if events.read().any(|event| {
        matches!(
            event.0,
            SessionEvent::Saved { .. }
                | SessionEvent::Reloaded { .. }
                | SessionEvent::PathChanged { .. }
        )
    }) {
        controller.0.refresh = true;
    }
}
fn command(
    In(invocation): In<CommandInvocation>,
    frame: Res<EditorFrame>,
    mut controller: NonSendMut<GitState>,
    mut intents: ResMut<EditorIntents>,
) -> CommandResult {
    let controller = &mut controller.0;
    controller.capture_directory_native(&frame.working_directory, &frame.project_root);
    let mut requests = Vec::new();
    match invocation.command.as_str() {
        SHOW_COMMAND => requests.push(HostRequest::ShowPanel {
            panel_type: PANEL_ID.into(),
            document: None,
            state: Value::Null,
            action: None,
        }),
        REFRESH_COMMAND => controller.refresh = true,
        DIFF_COMMAND => {
            if let Some(path) = invocation.arguments["path"].as_str() {
                controller.open_absolute_diff_native(path, &mut requests)
            }
        }
        _ => return Err("Unknown Git command".into()),
    }
    for request in requests {
        intents.push_at(invocation.source, invocation.area, request);
    }
    Ok(Vec::new())
}
fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    let workspace = world.get_non_send::<EditorSession>().map_or_else(
        || world.resource::<EditorFrame>().workspace,
        |session| session.workspace_id().0,
    );
    let frame = world.resource::<EditorFrame>().clone();
    let mut controller = world.non_send_mut::<GitState>();
    controller.0.claim_workspace(workspace)?;
    controller
        .0
        .capture_directory_native(&frame.working_directory, &frame.project_root);
    controller.0.visible += 1;
    world
        .non_send_mut::<GitPanels>()
        .0
        .insert(creation.entity, panels::GitPanel::default());
    let close = world.resource::<Close>().0;
    world
        .entity_mut(creation.entity)
        .insert(EditorPanelClose(close));
    Ok(Box::new(Frontend(creation.entity)))
}
fn close_panel(
    In(entity): In<Entity>,
    mut panels: NonSendMut<GitPanels>,
    mut controller: NonSendMut<GitState>,
) -> Result<(), String> {
    if panels.0.remove(&entity).is_some() {
        controller.0.visible = controller.0.visible.saturating_sub(1);
    }
    Ok(())
}
struct Frontend(Entity);
impl Panel for Frontend {
    fn title(&self, _: &World) -> String {
        "Git".into()
    }
    fn draw(
        &mut self,
        ui: &dear_imgui_rs::Ui,
        world: &mut World,
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let mut state = SystemState::<(
            NonSendMut<GitPanels>,
            NonSendMut<EditorSession>,
            Res<EditorFrame>,
            NonSendMut<GitState>,
            Option<NonSend<bed_ui::icons::Icons>>,
        )>::new(world);
        let (mut panels, mut documents, frame, mut controller, icons) = state
            .get_mut(world)
            .expect("Git frontend resources are installed");
        if let Err(error) = panels
            .0
            .get_mut(&self.0)
            .expect("Git frontend has widget state")
            .draw_body(
                ui,
                &mut controller.0,
                &mut documents,
                &frame.project_root,
                icons.as_deref(),
                &mut requests,
            )
        {
            requests.push(HostRequest::Notify {
                message: error.to_string(),
            });
        }
        for request in requests {
            world
                .resource_mut::<EditorIntents>()
                .push(Some(self.0), request);
        }
    }
}
fn save_workspace(controller: NonSendMut<GitState>) -> Value {
    json!({"commit_message":controller.0.draft})
}
fn restore_workspace(
    In(restore): In<EditorWorkspaceRestore>,
    mut controller: NonSendMut<GitState>,
) -> Result<(), String> {
    let draft = restore
        .state
        .as_ref()
        .and_then(|state| state["commit_message"].as_str())
        .unwrap_or_default()
        .into();
    controller
        .0
        .restore_workspace(Some(restore.workspace), restore.root, draft);
    Ok(())
}
fn shutdown(mut exit: MessageReader<bevy::app::AppExit>, mut controller: NonSendMut<GitState>) {
    if exit.read().next().is_some() {
        controller.0.cancel();
    }
}

fn source_action(
    In(invocation): In<CommandInvocation>,
    mut controller: NonSendMut<GitState>,
    mut documents: NonSendMut<EditorSession>,
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
        return Err("Source changed before applying the Git action".into());
    }
    let action = serde_json::from_value(invocation.arguments["action"].clone())
        .map_err(|error| error.to_string())?;
    controller
        .0
        .source_action(&mut documents, document, action)
        .map_err(|error| error.to_string())?;
    intents.push(invocation.source, HostRequest::Invalidate);
    Ok(Vec::new())
}
