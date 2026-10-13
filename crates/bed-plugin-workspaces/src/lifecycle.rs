//! Saved workspace lifecycle. Applications supply persistence and context switching.
use crate::{
    WorkspaceAction, WorkspaceAttachment, WorkspaceRepositoryResource, WorkspaceSnapshot,
    WorkspaceSpec, WorkspaceTarget, WorkspacesState,
};
use bed_api::workbench::{CommandInvocation, CommandResult};
use bevy::prelude::*;
use serde_json::Value;
use std::io;

pub const SAVE_COMMAND: &str = "bed.workspaces.save";
pub const ACTION_COMMAND: &str = "bed.workspaces.action";
pub const PROMOTE_COMMAND: &str = "bed.workspaces.promote";

/// Application integration consumes requests in the same emission order as editing.
#[derive(Message, Clone, Debug)]
pub struct WorkspaceRequest {
    pub action: WorkspaceAction,
    pub source: Option<Entity>,
    pub area: u32,
    pub session_id: u64,
    pub sequence: u64,
    pub layout_epoch: u64,
    pub barrier: Option<bed_api::features::OperationBarrierToken>,
}

#[derive(Resource, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceLifecycleMode {
    #[default]
    Managed,
    Application,
}

/// Optional host-owned layout serialization, independent of any numbered layout implementation.
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceLayoutSnapshot(pub bevy::ecs::system::SystemId<(), Value>);

/// Opening a workspace is a context transition; the host enforces its save/cancel barrier.
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceOpenHandler(
    pub bevy::ecs::system::SystemId<In<WorkspaceSpec>, Result<(), String>>,
);

pub struct WorkspacesIntegrationPlugin;
impl Plugin for WorkspacesIntegrationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bed_workbench::WorkbenchPlugin>() {
            app.add_plugins(bed_workbench::WorkbenchPlugin);
        }
        app.init_resource::<WorkspaceRepositoryResource>()
            .init_resource::<WorkspaceAttachment>()
            .init_resource::<WorkspaceLifecycleMode>()
            .init_resource::<WorkspacesState>()
            .add_message::<WorkspaceRequest>()
            .init_resource::<WorkspaceRequestCursor>()
            .add_systems(PostUpdate, process_workspace_requests)
            .add_systems(Startup, initialize_workspace_observations);
        bed_api::workbench::register(app.world_mut(), "bed.workspaces", |registrar| {
            registrar.command(SAVE_COMMAND, "Save Workspace", save_command);
            registrar.command(ACTION_COMMAND, "Workspace Action", action_command);
            registrar.command(PROMOTE_COMMAND, "Open Terminal Workspace", promote_command);
            registrar.shell_alias("+workspace", PROMOTE_COMMAND);
            registrar.shell_alias("+w", PROMOTE_COMMAND);
        })
        .expect("workspace integration is installed once");
        app.init_resource::<bed_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with("bed.workspaces", |registrar| {
                registrar.command_ref(SAVE_COMMAND);
                registrar.command_ref(ACTION_COMMAND);
                registrar.command_ref(PROMOTE_COMMAND);
                registrar.menu(bed_api::features::MenuSlot::Application, SAVE_COMMAND);
            })
            .expect("workspace feature metadata is installed once");
    }
}

pub fn rename_workspace(
    world: &mut World,
    spec: &WorkspaceSpec,
    name: &str,
) -> io::Result<WorkspaceSpec> {
    let renamed = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .rename(spec, name)?;
    if matches!(world.get_resource::<WorkspaceAttachment>(),Some(WorkspaceAttachment::Saved(current)) if current.identity()==spec.identity())
    {
        world.insert_resource(WorkspaceAttachment::Saved(renamed.clone()));
    }
    refresh_workspace_observations(world)?;
    Ok(renamed)
}

pub fn refresh_workspace_observations(world: &mut World) -> io::Result<()> {
    let recent = world.resource::<WorkspaceRepositoryResource>().0.recent()?;
    world.resource_mut::<WorkspacesState>().recent_workspaces = recent;
    Ok(())
}
fn initialize_workspace_observations(world: &mut World) {
    if let Err(error) = refresh_workspace_observations(world) {
        world.resource_mut::<WorkspacesState>().error = Some(error.to_string());
    }
}

pub fn request_workspace(
    world: &mut World,
    action: WorkspaceAction,
    source: Option<Entity>,
    area: u32,
) {
    let session_id = world
        .get_resource::<bed_api::documents::DocumentFrame>()
        .map_or(0, |frame| frame.session_id);
    let layout_epoch = world
        .get_resource::<bed_workbench::WorkbenchLayoutEpoch>()
        .map_or(0, |epoch| epoch.0);
    world.write_message(WorkspaceRequest {
        action,
        source,
        area,
        session_id,
        layout_epoch,
        sequence: bed_api::features::next_request_sequence(),
        barrier: None,
    });
}

fn request_workspace_command(
    world: &mut World,
    action: WorkspaceAction,
    source: Option<Entity>,
    area: u32,
) {
    let barrier = world
        .contains_resource::<bed_core::HostRequests>()
        .then(|| bed_core::create_operation_barrier(world));
    if let Some(token) = barrier {
        world.resource_mut::<bed_api::features::HostEffects>().push(
            source,
            bed_api::features::HostAction::OperationBarrier { token },
        );
    }
    let session_id = world
        .get_resource::<bed_api::documents::DocumentFrame>()
        .map_or(0, |frame| frame.session_id);
    let layout_epoch = world
        .get_resource::<bed_workbench::WorkbenchLayoutEpoch>()
        .map_or(0, |epoch| epoch.0);
    world.write_message(WorkspaceRequest {
        action,
        source,
        area,
        session_id,
        layout_epoch,
        barrier,
        sequence: bed_api::features::next_request_sequence(),
    });
}

fn save_command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    request_workspace_command(
        world,
        WorkspaceAction::SaveCurrent {
            name: invocation.arguments["name"].as_str().map(str::to_owned),
        },
        invocation.source,
        invocation.area,
    );
    Ok(Vec::new())
}
fn action_command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let action =
        WorkspaceAction::from_value(&invocation.arguments).ok_or("Invalid workspace action")?;
    request_workspace_command(world, action, invocation.source, invocation.area);
    Ok(Vec::new())
}
fn promote_command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let root = if invocation.arguments.get("shell").is_some() {
        let shell = bed_api::workbench::ShellInvocation::from_arguments(&invocation.arguments)?;
        if !shell.arguments.is_empty() {
            return Err("Usage: +workspace".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::path::PathBuf::from(std::ffi::OsString::from_vec(shell.directory))
        }
        #[cfg(not(unix))]
        {
            std::path::PathBuf::from(
                String::from_utf8(shell.directory).map_err(|_| "Calling directory is not UTF-8")?,
            )
        }
    } else {
        invocation.arguments["directory"]
            .as_str()
            .or_else(|| invocation.arguments["cwd"].as_str())
            .ok_or("Workspace promotion requires the calling directory")?
            .into()
    };
    request_workspace_command(
        world,
        WorkspaceAction::Promote(root),
        invocation.source,
        invocation.area,
    );
    Ok(Vec::new())
}

/// Commit current live state before marking attachment saved. Failed commits leave it untouched.
pub fn save_current_workspace(
    world: &mut World,
    name: Option<String>,
    layouts: Value,
) -> io::Result<WorkspaceSpec> {
    let context = world
        .get_resource::<bed_files::FileContext>()
        .ok_or_else(|| io::Error::other("No file context is installed"))?;
    let target = match &context.provider {
        bed_files::FileProviderId::Local => WorkspaceTarget::Local,
        bed_files::FileProviderId::Ssh { host } => WorkspaceTarget::Ssh { host: host.clone() },
    };
    let mut spec = WorkspaceSpec {
        name: name.clone().unwrap_or_default(),
        target,
        root: context.directory_string(),
    };
    if name.is_none()
        && let Some(stored) = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&spec)?
    {
        spec.name = stored.name;
    }
    if spec.name.is_empty() {
        spec.name = crate::default_name(&spec.root);
    }
    let previous = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?;
    let plugins =
        bed_storage::collect_snapshot(world, previous.as_ref().map(|state| &state.plugins))?;
    let spec = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec, WorkspaceSnapshot { layouts, plugins })?;
    world.insert_resource(WorkspaceAttachment::Saved(spec.clone()));
    match world.resource::<WorkspaceRepositoryResource>().0.recent() {
        Ok(recent) => world.resource_mut::<WorkspacesState>().recent_workspaces = recent,
        Err(error) => world.resource_mut::<WorkspacesState>().error = Some(error.to_string()),
    }
    Ok(spec)
}

#[derive(Resource, Default)]
struct WorkspaceRequestCursor {
    cursor: bevy::ecs::message::MessageCursor<WorkspaceRequest>,
    pending: Vec<WorkspaceRequest>,
}
fn process_workspace_requests(world: &mut World) {
    if *world.resource::<WorkspaceLifecycleMode>() == WorkspaceLifecycleMode::Application {
        return;
    }
    let requests = world.resource_scope(|world, mut cursor: Mut<WorkspaceRequestCursor>| {
        cursor
            .cursor
            .read(world.resource::<Messages<WorkspaceRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world
        .resource_mut::<WorkspaceRequestCursor>()
        .pending
        .extend(requests);
    let mut requests =
        std::mem::take(&mut world.resource_mut::<WorkspaceRequestCursor>().pending).into_iter();
    while let Some(request) = requests.next() {
        let waiting = request.barrier.is_some_and(|token| {
            matches!(
                bed_core::barrier_status(world, token),
                Some(bed_core::OperationBarrierStatus::Waiting)
            )
        }) || (request.barrier.is_none()
            && world.contains_resource::<bed_core::HostRequests>()
            && bed_core::operation_barrier_active(world));
        if waiting {
            let mut cursor = world.resource_mut::<WorkspaceRequestCursor>();
            cursor.pending.push(request);
            cursor.pending.extend(requests);
            break;
        }
        let result = (|| -> io::Result<()> {
            if world
                .get_resource::<bed_api::documents::DocumentFrame>()
                .is_some_and(|frame| frame.session_id != request.session_id)
            {
                return Err(io::Error::other(
                    "Workspace request belongs to an old document session",
                ));
            }
            if request.barrier.is_some_and(|token| {
                bed_core::barrier_status(world, token)
                    != Some(bed_core::OperationBarrierStatus::Ready)
            }) {
                return Err(io::Error::other("Workspace operation is no longer active"));
            }
            match request.action {
                WorkspaceAction::SaveCurrent { name } => {
                    let layouts = match world.get_resource::<WorkspaceLayoutSnapshot>().copied() {
                        Some(hook) => world.run_system(hook.0).map_err(io::Error::other)?,
                        None => Value::Null,
                    };
                    save_current_workspace(world, name, layouts)?;
                }
                WorkspaceAction::OpenWorkspace(spec)
                | WorkspaceAction::OpenWorkspaceWindow(spec) => {
                    let hook = world
                        .get_resource::<WorkspaceOpenHandler>()
                        .copied()
                        .ok_or_else(|| {
                            io::Error::other(
                                "The application has not supplied a workspace context transition",
                            )
                        })?;
                    world
                        .run_system_with(hook.0, spec)
                        .map_err(io::Error::other)?
                        .map_err(io::Error::other)?;
                }
                WorkspaceAction::RemoveWorkspace(spec) => world
                    .resource_mut::<WorkspaceRepositoryResource>()
                    .0
                    .forget(&spec)?,
                WorkspaceAction::RenameWorkspace(spec, name) => {
                    rename_workspace(world, &spec, &name)?;
                }
                WorkspaceAction::Error(message) => return Err(io::Error::other(message)),
                _ => {
                    return Err(io::Error::other(
                        "This workspace action requires application integration",
                    ));
                }
            }
            refresh_workspace_observations(world)?;
            Ok(())
        })();
        if let Some(token) = request.barrier {
            let _ = bed_core::finish_operation_barrier(
                world,
                token,
                result.as_ref().map(|_| ()).map_err(ToString::to_string),
            );
        }
        if let Err(error) = result {
            world.resource_mut::<WorkspacesState>().error = Some(error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkspaceAttachment, WorkspaceRepository, WorkspaceSnapshot};
    use serde_json::json;
    struct FailedRepository;
    impl WorkspaceRepository for FailedRepository {
        fn recent(&self) -> io::Result<Vec<WorkspaceSpec>> {
            Ok(Vec::new())
        }
        fn find(&self, _: &WorkspaceSpec) -> io::Result<Option<WorkspaceSpec>> {
            Ok(None)
        }
        fn load(&self, _: &WorkspaceSpec) -> io::Result<Option<WorkspaceSnapshot>> {
            Ok(None)
        }
        fn commit(&mut self, _: WorkspaceSpec, _: WorkspaceSnapshot) -> io::Result<WorkspaceSpec> {
            Err(io::Error::other("disk full"))
        }
        fn forget(&mut self, _: &WorkspaceSpec) -> io::Result<()> {
            Ok(())
        }
        fn rename(&mut self, _: &WorkspaceSpec, _: &str) -> io::Result<WorkspaceSpec> {
            unreachable!()
        }
    }
    fn fixture() -> App {
        let mut app = App::new();
        app.insert_resource(
            bed_files::FileContext::local(std::env::current_dir().unwrap()).unwrap(),
        )
        .add_plugins(WorkspacesIntegrationPlugin);
        app
    }
    #[test]
    fn managed_requests_wait_for_admission_without_overtaking_each_other() {
        let mut app = fixture();
        app.add_plugins(bed_core::DocumentHostPlugin);
        app.update();
        let world = app.world_mut();
        let token = bed_core::create_operation_barrier(world);
        let session_id = world
            .resource::<bed_api::documents::DocumentFrame>()
            .session_id;
        world.write_message(WorkspaceRequest {
            action: WorkspaceAction::SaveCurrent {
                name: Some("First".into()),
            },
            source: None,
            area: 0,
            session_id,
            sequence: bed_api::features::next_request_sequence(),
            layout_epoch: 0,
            barrier: Some(token),
        });
        request_workspace(
            world,
            WorkspaceAction::SaveCurrent {
                name: Some("Second".into()),
            },
            None,
            0,
        );
        process_workspace_requests(world);
        assert!(
            world
                .resource::<WorkspacesState>()
                .recent_workspaces
                .is_empty()
        );
        assert_eq!(
            *world.resource::<WorkspaceAttachment>(),
            WorkspaceAttachment::Temporary
        );
        let request = bed_core::capture_host_request(
            world,
            None,
            bed_api::features::HostAction::OperationBarrier { token },
        );
        bed_core::apply_document_request(world, request).unwrap();
        process_workspace_requests(world);
        assert!(matches!(
            world.resource::<WorkspaceAttachment>(),
            WorkspaceAttachment::Saved(spec) if spec.name == "Second"
        ));
        assert_eq!(
            bed_core::barrier_status(world, token),
            Some(bed_core::OperationBarrierStatus::Complete)
        );
    }
    #[test]
    fn managed_save_retains_documents_and_pending_drafts() {
        let mut app = fixture();
        app.add_plugins(bed_core::DocumentHostPlugin);
        app.update();
        let world = app.world_mut();
        let document = world
            .non_send_mut::<bed_documents::DocumentSession>()
            .create_document(b"live")
            .unwrap();
        let session_id = world
            .non_send::<bed_documents::DocumentSession>()
            .session_id();
        let commit =
            world.register_system(|In(_): In<bed_api::workbench::PanelActionInvocation>| {
                Err::<bool, String>("stale draft".into())
            });
        let panel = world
            .spawn((
                bed_api::documents::PanelBinding {
                    session_id: session_id.0,
                    document: Some(document),
                    ..Default::default()
                },
                bed_api::workbench::PanelActions(commit),
            ))
            .id();
        save_command(
            In(CommandInvocation {
                command: SAVE_COMMAND.into(),
                source: None,
                area: 0,
                arguments: json!({"name":"Example"}),
            }),
            world,
        )
        .unwrap();
        bed_core::resolve_documents(world);
        process_workspace_requests(world);
        assert!(matches!(
            world.resource::<WorkspaceAttachment>(),
            WorkspaceAttachment::Saved(spec) if spec.name == "Example"
        ));
        assert!(world.resource::<WorkspacesState>().error.is_none());
        assert!(world.entities().contains(panel));
        assert_eq!(
            world
                .non_send::<bed_documents::DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            b"live"
        );
        assert!(!bed_core::operation_barrier_active(world));
    }
    #[test]
    fn failed_commit_keeps_attachment_temporary_and_live_state() {
        let mut app = fixture();
        app.insert_resource(WorkspaceRepositoryResource(Box::new(FailedRepository)));
        let panel = app.world_mut().spawn_empty().id();
        let context = app.world().resource::<bed_files::FileContext>().clone();
        assert!(
            save_current_workspace(
                app.world_mut(),
                Some("Example".into()),
                json!({"panels":[]})
            )
            .is_err()
        );
        assert_eq!(
            *app.world().resource::<WorkspaceAttachment>(),
            WorkspaceAttachment::Temporary
        );
        assert_eq!(*app.world().resource::<bed_files::FileContext>(), context);
        assert!(app.world().entities().contains(panel));
    }
    #[test]
    fn save_current_attaches_state_without_restoring_contributors() {
        let mut app = fixture();
        #[derive(Resource)]
        struct Draft(String);
        app.insert_resource(Draft("live".into()));
        let save = app
            .world_mut()
            .register_system(|draft: Res<Draft>| json!({"draft":draft.0}));
        let restore = app.world_mut().register_system(
            |In(_): In<bed_storage::FeatureStateRestore>, mut draft: ResMut<Draft>| {
                draft.0 = "restored".into();
                Ok(())
            },
        );
        let entity = app
            .world_mut()
            .spawn(bed_storage::FeatureStateContribution {
                owner: "plugin",
                save,
                restore,
            })
            .id();
        let generation = app.world().resource::<bed_files::FileContext>().generation;
        let spec = save_current_workspace(
            app.world_mut(),
            Some("Example".into()),
            json!({"panels":[]}),
        )
        .unwrap();
        assert_eq!(
            *app.world().resource::<WorkspaceAttachment>(),
            WorkspaceAttachment::Saved(spec.clone())
        );
        assert_eq!(app.world().resource::<Draft>().0, "live");
        assert_eq!(
            app.world().resource::<bed_files::FileContext>().generation,
            generation
        );
        assert!(app.world().entities().contains(entity));
        assert_eq!(
            app.world()
                .resource::<WorkspaceRepositoryResource>()
                .0
                .load(&spec)
                .unwrap()
                .unwrap()
                .plugins["plugin"]["draft"],
            "live"
        );
    }
}
