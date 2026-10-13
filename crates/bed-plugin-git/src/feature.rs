//! The same repository controller serves source widgets, sidebar and diff views.
use super::*;
use bed_api::documents::DocumentFrame;
use bed_api::storage::{FeatureStateContribution, FeatureStateRestore};
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction, register,
};
use bed_api::{
    documents::{DocumentEvent, DocumentReply},
    features::{FeatureSystems, HostEffects},
    workbench::{CommandPresentation, CommandPresentations, PanelClose},
};
use bed_files::{FileContext, FileProviderId, FileServices};
use bevy::{ecs::system::SystemState, prelude::*};
use std::collections::HashMap;

#[derive(Default)]
pub struct GitState(pub(crate) GitController);

#[derive(Default)]
pub struct GitPanels(pub(crate) HashMap<Entity, panels::GitPanel>);

#[derive(Resource)]
struct Close(bed_api::workbench::PanelLifecycleSystem);

/// Repository location and document lifetime supplied by the host.
#[derive(Resource, Clone, Default)]
pub struct GitTarget {
    pub root: String,
    pub working_directory: String,
    pub session_id: Option<bed_documents::SessionId>,
}

#[derive(Clone, PartialEq, Eq)]
struct RepositorySelection {
    directory: String,
    root: String,
    provider: FileProviderId,
    session: bed_documents::SessionId,
}
#[derive(Resource, Default)]
pub(crate) struct SelectedRepository(Option<RepositorySelection>);

fn working_directory(target: &GitTarget, context: &FileContext) -> String {
    if target.working_directory.is_empty() {
        context.directory_string()
    } else {
        target.working_directory.clone()
    }
}

pub struct GitPlugin;

impl Plugin for GitPlugin {
    fn build(&self, app: &mut App) {
        app.init_non_send::<bed_ui::icons::Icons>();
        let directory = app
            .world()
            .get_non_send::<bed_ui::icons::IconDirectories>()
            .and_then(|sources| sources.get(MODULE_ID))
            .map(std::path::Path::to_path_buf);
        let definitions = [
            ("branch", "branch.svg", "git-branch"),
            ("refresh", "refresh.svg", "refresh-cw"),
            ("fetch", "fetch.svg", "cloud-download"),
            ("pull", "pull.svg", "arrow-down-to-line"),
            ("push", "push.svg", "arrow-up-from-line"),
            ("stage", "stage.svg", "plus"),
            ("unstage", "unstage.svg", "minus"),
        ]
        .map(|(name, filename, bundled)| bed_ui::icons::IconDefinition {
            name,
            filename,
            embedded_svg: bed_ui::icons::Icons::bundled_svg(bundled),
        });
        app.world_mut()
            .non_send_mut::<bed_ui::icons::Icons>()
            .register(MODULE_ID, &definitions, directory.as_deref())
            .expect("plugin icon contract and configured sources are valid");
        app.world_mut()
            .init_non_send::<bed_documents::DocumentSession>();
        app.world_mut().init_non_send::<GitState>();
        app.world_mut().init_non_send::<GitPanels>();
        app.init_non_send::<bed_editor_widgets::extensions::EditorExtensions>()
            .init_non_send::<bed_editor_widgets::source_intents::SourceRequests>()
            .init_resource::<DocumentFrame>()
            .init_resource::<GitTarget>()
            .init_resource::<SelectedRepository>()
            .init_resource::<FileContext>()
            .init_resource::<FileServices>()
            .init_resource::<HostEffects>()
            .init_resource::<bed_api::features::FeatureRegistry>()
            .init_resource::<CommandPresentations>()
            .add_message::<DocumentReply>()
            .add_message::<DocumentEvent>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.command_ref(SHOW_COMMAND);
                registrar.command_ref(REFRESH_COMMAND);
                registrar.command_ref(DIFF_COMMAND);
                registrar.menu(MenuSlot::Application, SHOW_COMMAND);
                registrar.menu(MenuSlot::File, DIFF_COMMAND);
                registrar.menu(MenuSlot::TextSelection, DIFF_COMMAND);
                registrar.toolbar(SHOW_COMMAND);
                registrar.panel_options(PANEL_ID, true, PanelPlacement::Sidebar);
                registrar.panel_ref(DIFF_PANEL_ID);
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
            registrar.panel_icon(PANEL_ID, "bed.git.branch");
            registrar.panel_icon(DIFF_PANEL_ID, "bed.git.branch");
            registrar.command_icon(SHOW_COMMAND, "bed.git.branch");
            registrar.toolbar(SHOW_COMMAND);
            registrar.menu(SHOW_COMMAND);
        })
        .expect("GitPlugin is installed once");
        crate::status::install(app);
        let close = app.world_mut().register_system(close_panel);
        app.insert_resource(Close(close));
        let save = app.world_mut().register_system(save_workspace);
        let restore = app.world_mut().register_system(restore_session);
        app.world_mut().spawn(FeatureStateContribution {
            owner: MODULE_ID,
            save,
            restore,
        });
        app.add_systems(
            PreUpdate,
            (replies, document_events, prepare)
                .chain()
                .in_set(FeatureSystems::Prepare),
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
    documents: Option<NonSendMut<DocumentSession>>,
    filesystem: Option<NonSend<bed_remote::WorkspaceFilesystem>>,
    frame: Res<DocumentFrame>,
    target: Res<GitTarget>,
    mut selected: ResMut<SelectedRepository>,
    file_context: Res<FileContext>,
    file_services: Res<FileServices>,
    mut intents: ResMut<HostEffects>,
    mut source: NonSendMut<bed_editor_widgets::extensions::EditorExtensions>,
    mut commands: Commands,
    mut presentations: ResMut<CommandPresentations>,
) {
    panels.0.retain(|entity, _| alive.contains(*entity));
    let Some(mut documents) = documents else {
        return;
    };
    let mut requests = Vec::new();
    let file_services = file_services.for_context(&file_context);
    let controller = &mut controller.0;
    let selection = RepositorySelection {
        directory: working_directory(&target, &file_context),
        root: target.root.clone(),
        provider: file_context.provider.clone(),
        session: target.session_id.unwrap_or_else(|| documents.session_id()),
    };
    if selected.0.as_ref().is_some_and(|prior| prior != &selection) {
        let draft = std::mem::take(&mut controller.draft);
        controller.restore_session(Some(selection.session.0), selection.root.clone(), draft);
    }
    controller.capture_directory_native(&selection.directory, &selection.root);
    selected.0 = Some(selection);
    if let Err(error) = controller.tick_native(
        &mut documents,
        &target.root,
        filesystem.as_deref(),
        &file_services,
        &mut requests,
    ) {
        controller.error = Some(error.to_string());
    }
    for entity in panels.0.keys() {
        commands.entity(*entity).insert(match &controller.error {
            Some(error) => bed_api::workbench::PanelReadiness::Failed(error.clone()),
            None if controller.status.is_some() && !controller.busy() => {
                bed_api::workbench::PanelReadiness::Ready
            }
            None => bed_api::workbench::PanelReadiness::Pending,
        });
    }
    presentations.0.insert(
        REFRESH_COMMAND.into(),
        CommandPresentation {
            label: controller.busy().then(|| "Refreshing Git…".into()),
            enabled: !controller.busy(),
            visible: true,
            ..Default::default()
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

fn replies(mut replies: MessageReader<DocumentReply>, mut controller: NonSendMut<GitState>) {
    for reply in replies.read() {
        if let DocumentReply::Save {
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

fn document_events(mut events: MessageReader<DocumentEvent>, mut controller: NonSendMut<GitState>) {
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
    target: Res<GitTarget>,
    file_context: Res<FileContext>,
    mut controller: NonSendMut<GitState>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let controller = &mut controller.0;
    controller.capture_directory_native(&working_directory(&target, &file_context), &target.root);
    let mut requests = Vec::new();
    match invocation.command.as_str() {
        SHOW_COMMAND => requests.push(HostAction::ShowPanel {
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
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Changes", "Select a changed file to inspect its diff. Right-click for staged or unstaged comparisons and file operations."),
        ("Staging", "Stage or unstage changes, then enter a commit message. Refresh updates repository status."),
    ]));
    let target = world.resource::<GitTarget>().clone();
    let session = target
        .session_id
        .unwrap_or_else(|| world.non_send::<DocumentSession>().session_id());
    let cwd = working_directory(&target, world.resource::<FileContext>());
    let mut controller = world.non_send_mut::<GitState>();
    controller.0.claim_session(session.0)?;
    controller.0.capture_directory_native(&cwd, &target.root);
    controller.0.visible += 1;
    world
        .non_send_mut::<GitPanels>()
        .0
        .insert(creation.entity, panels::GitPanel::default());
    let close = world.resource::<Close>().0;
    world.entity_mut(creation.entity).insert(PanelClose(close));
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
        _: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let mut state = SystemState::<(
            NonSendMut<GitPanels>,
            NonSendMut<DocumentSession>,
            Res<GitTarget>,
            NonSendMut<GitState>,
            Option<NonSend<bed_ui::icons::Icons>>,
        )>::new(world);
        let (mut panels, mut documents, target, mut controller, icons) = state
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
                &target.root,
                icons.as_deref(),
                &mut requests,
            )
        {
            requests.push(HostAction::Notify {
                message: error.to_string(),
            });
        }
        for request in requests {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.0), request);
        }
    }
}

fn save_workspace(controller: NonSendMut<GitState>) -> Value {
    json!({"commit_message":controller.0.draft})
}

fn restore_session(
    In(restore): In<FeatureStateRestore>,
    mut controller: NonSendMut<GitState>,
    target: Res<GitTarget>,
    documents: NonSend<DocumentSession>,
) -> Result<(), String> {
    let draft = restore
        .state
        .as_ref()
        .and_then(|state| state["commit_message"].as_str())
        .unwrap_or_default()
        .into();
    controller.0.restore_session(
        Some(
            target
                .session_id
                .unwrap_or_else(|| documents.session_id())
                .0,
        ),
        target.root.clone(),
        draft,
    );
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
    mut documents: NonSendMut<DocumentSession>,
    mut intents: ResMut<HostEffects>,
) -> CommandResult {
    let document = DocumentId(
        invocation.arguments["document"]
            .as_u64()
            .ok_or("Source action requires a document")?,
    );
    let revision: bed_api::documents::Revision =
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
    intents.push(invocation.source, HostAction::Invalidate);
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_follows_live_directory_provider_and_session_without_workspace_restore() {
        let mut app = App::new();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.add_plugins(GitPlugin);
        let mut context = FileContext::local(std::env::temp_dir()).unwrap();
        context.availability = bed_files::FileAvailability::Disconnected;
        let first = context.directory_string();
        app.insert_resource(context);
        let system = app.world_mut().register_system(prepare);
        app.world_mut().run_system(system).unwrap();
        assert_eq!(app.world().non_send::<GitState>().0.root, first);
        let session = app.world().non_send::<DocumentSession>().session_id().0;
        {
            let mut controller = app.world_mut().non_send_mut::<GitState>();
            controller.0.session_id = Some(session);
            controller.0.visible = 2;
            controller.0.draft = "Commit draft".into();
        }
        let mut context = FileContext::remote("fixture-host", "/project").unwrap();
        context.availability = bed_files::FileAvailability::Disconnected;
        app.insert_resource(context);
        app.world_mut().run_system(system).unwrap();
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.root, "/project");
        assert!(controller.status.is_none() && !controller.busy());
        assert_eq!(controller.visible, 2);
        assert_eq!(controller.draft, "Commit draft");
        let old_session = app.world().non_send::<DocumentSession>().session_id();
        app.world_mut().insert_non_send(DocumentSession::new());
        assert_ne!(
            app.world().non_send::<DocumentSession>().session_id(),
            old_session
        );
        app.world_mut().run_system(system).unwrap();
        assert_eq!(app.world().non_send::<GitState>().0.visible, 0);
        app.world_mut().resource_mut::<GitTarget>().root = "/explicit-repository".into();
        app.world_mut().run_system(system).unwrap();
        assert_eq!(
            app.world().non_send::<GitState>().0.root,
            "/explicit-repository"
        );
    }
}
