//! Saved workspace lifecycle over an application-selected repository.
use crate::{
    WorkspaceAction, WorkspaceAttachment, WorkspaceRepository, WorkspaceRepositoryResource,
    WorkspaceSnapshot, WorkspaceSpec, WorkspaceTarget, WorkspacesState,
};
use bedtime_api::workbench::{CommandInvocation, CommandResult};
use bevy::prelude::*;
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const SAVE_COMMAND: &str = "bedtime.workspaces.lifecycle.save";
pub const ACTION_COMMAND: &str = "bedtime.workspaces.lifecycle.action";
pub const PROMOTE_COMMAND: &str = "bedtime.workspaces.lifecycle.promote";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceRequestId(pub u64);

#[derive(Message, Clone, Debug)]
pub struct WorkspaceRequest {
    pub action: WorkspaceAction,
    pub source: Option<Entity>,
    pub area: u32,
    pub session_id: u64,
    pub sequence: u64,
    pub layout_epoch: u64,
    pub barrier: Option<bedtime_api::features::OperationBarrierToken>,
    pub receipt: Option<WorkspaceRequestId>,
}

/// Supplying a repository explicitly always wins over the plugin's memory default.
pub struct WorkspacePersistencePlugin(Mutex<Option<Box<dyn WorkspaceRepository>>>);
impl WorkspacePersistencePlugin {
    pub fn new(repository: impl WorkspaceRepository) -> Self {
        Self(Mutex::new(Some(Box::new(repository))))
    }
}
impl Plugin for WorkspacePersistencePlugin {
    fn build(&self, app: &mut App) {
        let repository = self
            .0
            .lock()
            .expect("workspace repository configuration")
            .take()
            .expect("persistence plugin is installed once");
        app.insert_resource(WorkspaceRepositoryResource(repository));
    }
}

#[derive(Resource, Clone, Copy)]
pub struct WorkspaceLayoutSnapshot(pub bevy::ecs::system::SystemId<(), Value>);
#[derive(Clone, Debug)]
pub struct WorkspaceRestore {
    pub layouts: Option<Value>,
    pub retained_documents: bool,
    pub preserved_panel: Option<Entity>,
}
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceLayoutRestore(
    pub bevy::ecs::system::SystemId<In<WorkspaceRestore>, Result<(), String>>,
);
/// Validate the application's opaque layout payload before retiring live content.
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceLayoutPreflight(pub bevy::ecs::system::SystemId<In<Value>, Result<(), String>>);
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceDirectoryEntered(pub bevy::ecs::system::SystemId<(), Result<(), String>>);
/// Opening another OS window remains an application process policy.
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceWindowHandler(
    pub bevy::ecs::system::SystemId<In<WorkspaceSpec>, Result<(), String>>,
);
/// Applications without shared documents may supply their own context operation.
#[derive(Resource, Clone, Copy)]
pub struct WorkspaceOpenHandler(
    pub bevy::ecs::system::SystemId<In<WorkspaceSpec>, Result<(), String>>,
);

#[derive(Resource, Default)]
struct WorkspaceRequests {
    cursor: bevy::ecs::message::MessageCursor<WorkspaceRequest>,
    pending: VecDeque<WorkspaceRequest>,
    results: HashMap<WorkspaceRequestId, Result<(), String>>,
}
struct Opening {
    request: WorkspaceRequest,
    spec: WorkspaceSpec,
    retained: bool,
    preserved_panel: Option<Entity>,
    reconnect: bool,
    create: bool,
    snapshot: Option<WorkspaceSnapshot>,
    #[cfg(feature = "remote")]
    remote: Option<
        std::sync::mpsc::Receiver<
            io::Result<(bedtime_files::FileContext, bedtime_files::FileServices)>,
        >,
    >,
}
#[derive(Default)]
struct WorkspaceOpening(Option<Opening>);
#[derive(Resource)]
struct WorkspaceAutosave {
    last_saved: Instant,
    context: Option<(bedtime_files::FileProviderId, PathBuf)>,
    // A partial restore must never replace the authoritative saved snapshot.
    restore_error: Option<String>,
}
impl Default for WorkspaceAutosave {
    fn default() -> Self {
        Self {
            last_saved: Instant::now(),
            context: None,
            restore_error: None,
        }
    }
}

pub struct WorkspacesIntegrationPlugin;
impl Plugin for WorkspacesIntegrationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_workbench::WorkbenchPlugin>() {
            app.add_plugins(bedtime_workbench::WorkbenchPlugin);
        }
        app.init_resource::<WorkspaceRepositoryResource>()
            .init_resource::<WorkspaceAttachment>()
            .init_resource::<WorkspacesState>()
            .init_resource::<WorkspaceRequests>()
            .init_resource::<WorkspaceAutosave>()
            .init_non_send::<WorkspaceOpening>()
            .add_message::<WorkspaceRequest>()
            .add_message::<bedtime_storage::FeatureStateChanged>()
            .add_systems(PostUpdate, resolve_workspace_requests)
            .add_systems(Startup, initialize_workspace_observations);
        let persist = app.world_mut().register_system(
            |In(reason): In<bedtime_core::LifecycleReason>, world: &mut World| {
                if matches!(
                    reason,
                    bedtime_core::LifecycleReason::ContextChange {
                        mode: bedtime_core::ContextChangeMode::Reconnect,
                        ..
                    }
                ) {
                    return Ok(());
                }
                if world
                    .resource::<WorkspaceAutosave>()
                    .restore_error
                    .is_some()
                {
                    // The retained snapshot remains the source of truth. Leaving
                    // this incomplete presentation does not require rewriting it.
                    return Ok(());
                }
                persist_workspace(world).map_err(|error| error.to_string())
            },
        );
        app.init_resource::<bedtime_core::LifecyclePreCommit>();
        app.world_mut()
            .resource_mut::<bedtime_core::LifecyclePreCommit>()
            .0
            .push(persist);
        bedtime_api::workbench::register(app.world_mut(), "bedtime.workspaces.lifecycle", |r| {
            r.command(SAVE_COMMAND, "Save Workspace", save_command);
            r.command(ACTION_COMMAND, "Workspace Action", action_command);
            r.command(PROMOTE_COMMAND, "Open Terminal Workspace", promote_command);
            r.shell_alias("+workspace", PROMOTE_COMMAND);
            r.shell_alias("+w", PROMOTE_COMMAND);
        })
        .expect("workspace lifecycle installs once");
        app.init_resource::<bedtime_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("bedtime.workspaces.lifecycle", |r| {
                r.command_ref(SAVE_COMMAND);
                r.command_ref(ACTION_COMMAND);
                r.command_ref(PROMOTE_COMMAND);
                r.menu(bedtime_api::features::MenuSlot::Application, SAVE_COMMAND);
            })
            .expect("workspace metadata installs once");
    }
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
fn refresh_after_commit(world: &mut World) {
    if let Some(error) = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .take_index_error()
    {
        world.resource_mut::<WorkspacesState>().error = Some(error);
    }
    if let Err(error) = refresh_workspace_observations(world) {
        world.resource_mut::<WorkspacesState>().error = Some(format!(
            "Workspace saved, but recent locations could not be refreshed: {error}"
        ));
    }
}
fn capture_request(
    world: &mut World,
    action: WorkspaceAction,
    source: Option<Entity>,
    area: u32,
    tracked: bool,
    command: bool,
) -> WorkspaceRequestId {
    // Global workspace operations outlive their originating panel. Promotion
    // alone needs the caller to remain live because it retains that terminal.
    let barrier_source = if matches!(&action, WorkspaceAction::Promote(_)) {
        source
    } else {
        None
    };
    let barrier = if world.contains_resource::<bedtime_core::HostRequests>() {
        let token = bedtime_core::create_operation_barrier(world);
        if command {
            world
                .resource_mut::<bedtime_api::features::HostEffects>()
                .push(
                    barrier_source,
                    bedtime_api::features::HostAction::OperationBarrier { token },
                );
        } else {
            bedtime_core::submit_host_request(
                world,
                barrier_source,
                bedtime_api::features::HostAction::OperationBarrier { token },
            );
        }
        Some(token)
    } else {
        None
    };
    let sequence = bedtime_api::features::next_request_sequence();
    let id = WorkspaceRequestId(sequence);
    let session_id = world
        .get_non_send::<bedtime_documents::DocumentSession>()
        .map(|session| session.session_id().0)
        .unwrap_or_else(|| {
            world
                .get_resource::<bedtime_api::documents::DocumentFrame>()
                .map_or(0, |frame| frame.session_id)
        });
    let layout_epoch = world
        .get_resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
        .map_or(0, |epoch| epoch.0);
    world.write_message(WorkspaceRequest {
        action,
        source,
        area,
        session_id,
        layout_epoch,
        sequence,
        barrier,
        receipt: tracked.then_some(id),
    });
    id
}
pub fn request_workspace(
    world: &mut World,
    action: WorkspaceAction,
    source: Option<Entity>,
    area: u32,
) {
    capture_request(world, action, source, area, false, false);
}
pub fn submit_workspace_request(
    world: &mut World,
    action: WorkspaceAction,
    source: Option<Entity>,
    area: u32,
) -> WorkspaceRequestId {
    capture_request(world, action, source, area, true, false)
}
pub fn take_workspace_result(
    world: &mut World,
    id: WorkspaceRequestId,
) -> Option<Result<(), String>> {
    world
        .resource_mut::<WorkspaceRequests>()
        .results
        .remove(&id)
}
fn save_command(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    capture_request(
        world,
        WorkspaceAction::SaveCurrent {
            name: input.arguments["name"].as_str().map(str::to_owned),
        },
        input.source,
        input.area,
        false,
        true,
    );
    Ok(Vec::new())
}
fn action_command(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let action = WorkspaceAction::from_value(&input.arguments).ok_or("Invalid workspace action")?;
    capture_request(world, action, input.source, input.area, false, true);
    Ok(Vec::new())
}
fn promote_command(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let path = if input.arguments.get("shell").is_some() {
        let shell = bedtime_api::workbench::ShellInvocation::from_arguments(&input.arguments)?;
        if !shell.arguments.is_empty() {
            return Err("Usage: +workspace".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(shell.directory))
        }
        #[cfg(not(unix))]
        {
            PathBuf::from(
                String::from_utf8(shell.directory).map_err(|_| "Calling directory is not UTF-8")?,
            )
        }
    } else {
        input.arguments["directory"]
            .as_str()
            .or_else(|| input.arguments["cwd"].as_str())
            .ok_or("Workspace promotion requires a directory")?
            .into()
    };
    capture_request(
        world,
        WorkspaceAction::Promote(path),
        input.source,
        input.area,
        false,
        true,
    );
    Ok(Vec::new())
}

fn capture_snapshot(
    world: &mut World,
    spec: &WorkspaceSpec,
    layouts: Option<Value>,
) -> io::Result<WorkspaceSnapshot> {
    let previous = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(spec)?;
    let layouts = match layouts {
        Some(value) => value,
        None => match world.get_resource::<WorkspaceLayoutSnapshot>().copied() {
            Some(hook) => world.run_system(hook.0).map_err(io::Error::other)?,
            None => bedtime_workbench::save(world),
        },
    };
    let plugins = bedtime_storage::collect_snapshot(
        world,
        previous.as_ref().map(|snapshot| &snapshot.plugins),
    )?;
    let history = world
        .get_non_send_mut::<bedtime_documents::DocumentSession>()
        .map(|mut session| session.history_snapshot())
        .transpose()?;
    Ok(WorkspaceSnapshot {
        layouts,
        plugins,
        history,
    })
}
pub fn save_current_workspace(
    world: &mut World,
    name: Option<String>,
    layouts: Value,
) -> io::Result<WorkspaceSpec> {
    let context = world
        .get_resource::<bedtime_files::FileContext>()
        .ok_or_else(|| io::Error::other("No file context is installed"))?;
    let target = match &context.provider {
        bedtime_files::FileProviderId::Local => WorkspaceTarget::Local,
        bedtime_files::FileProviderId::Ssh { host } => WorkspaceTarget::Ssh { host: host.clone() },
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
    let snapshot = capture_snapshot(world, &spec, (!layouts.is_null()).then_some(layouts))?;
    let spec = world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec, snapshot.clone())?;
    world.insert_resource(WorkspaceAttachment::Saved(spec.clone()));
    if let Some(mut session) = world.get_non_send_mut::<bedtime_documents::DocumentSession>() {
        session.mark_history_persisted();
    }
    let mut autosave = world.resource_mut::<WorkspaceAutosave>();
    autosave.last_saved = Instant::now();
    autosave.restore_error = None;
    refresh_after_commit(world);
    Ok(spec)
}
pub fn persist_workspace(world: &mut World) -> io::Result<()> {
    let Some(WorkspaceAttachment::Saved(spec)) =
        world.get_resource::<WorkspaceAttachment>().cloned()
    else {
        return Ok(());
    };
    if let Some(error) = &world.resource::<WorkspaceAutosave>().restore_error {
        return Err(io::Error::other(format!(
            "Workspace state was not saved because restoring it failed: {error}. Explicitly save the workspace to replace its retained snapshot."
        )));
    }
    let snapshot = capture_snapshot(world, &spec, None)?;
    world
        .resource_mut::<WorkspaceRepositoryResource>()
        .0
        .commit(spec, snapshot.clone())?;
    if let Some(mut session) = world.get_non_send_mut::<bedtime_documents::DocumentSession>() {
        session.mark_history_persisted();
    }
    let mut autosave = world.resource_mut::<WorkspaceAutosave>();
    autosave.last_saved = Instant::now();
    refresh_after_commit(world);
    Ok(())
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
    refresh_after_commit(world);
    Ok(renamed)
}
pub fn restore_current_workspace(world: &mut World) -> io::Result<()> {
    let Some(WorkspaceAttachment::Saved(spec)) =
        world.get_resource::<WorkspaceAttachment>().cloned()
    else {
        return Ok(());
    };
    let snapshot = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?;
    restore_snapshot(world, snapshot.as_ref(), false, None)
}
fn restore_snapshot(
    world: &mut World,
    snapshot: Option<&WorkspaceSnapshot>,
    retained: bool,
    preserved_panel: Option<Entity>,
) -> io::Result<()> {
    let result = (|| {
        bedtime_storage::restore_snapshot(world, snapshot.map(|snapshot| &snapshot.plugins))?;
        if let Some(history) = snapshot.and_then(|snapshot| snapshot.history.as_ref()) {
            let mut session = world.non_send_mut::<bedtime_documents::DocumentSession>();
            if retained {
                session.merge_history_snapshot(history)?;
            } else {
                session.restore_history_snapshot(history)?;
            }
        }
        let restore = WorkspaceRestore {
            layouts: snapshot
                .map(|snapshot| snapshot.layouts.clone())
                .filter(|value| !value.is_null()),
            retained_documents: retained,
            preserved_panel,
        };
        if let Some(hook) = world.get_resource::<WorkspaceLayoutRestore>().copied() {
            world
                .run_system_with(hook.0, restore)
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
        } else if !retained && let Some(value) = restore.layouts {
            bedtime_workbench::restore(world, &value).map_err(io::Error::other)?;
        }
        Ok(())
    })();
    world.resource_mut::<WorkspaceAutosave>().restore_error = result
        .as_ref()
        .err()
        .map(|error: &io::Error| error.to_string());
    result
}
fn complete_request(world: &mut World, request: WorkspaceRequest, result: Result<(), String>) {
    if let Some(token) = request.barrier {
        let _ = bedtime_core::finish_operation_barrier(world, token, result.clone());
    }
    if let Some(id) = request.receipt {
        world
            .resource_mut::<WorkspaceRequests>()
            .results
            .insert(id, result.clone());
    }
    if let Err(error) = result {
        world.resource_mut::<WorkspacesState>().error = Some(error);
    }
}
fn request_is_current(world: &World, request: &WorkspaceRequest) -> io::Result<()> {
    if request
        .source
        .is_some_and(|source| bedtime_api::workbench::panel_owner(world, source).is_none())
    {
        return Err(io::Error::other("Originating panel is unavailable"));
    }
    if world
        .get_non_send::<bedtime_documents::DocumentSession>()
        .is_some_and(|session| session.session_id().0 != request.session_id)
    {
        return Err(io::Error::other(
            "Workspace request belongs to an old document session",
        ));
    }
    if let Some(token) = request.barrier {
        let status = bedtime_core::barrier_status(world, token);
        match status {
            Some(bedtime_core::OperationBarrierStatus::Ready) => {}
            Some(bedtime_core::OperationBarrierStatus::Failed(error)) => {
                return Err(io::Error::other(error));
            }
            _ => return Err(io::Error::other("Workspace operation is no longer active")),
        }
    }
    Ok(())
}
fn promotion_panel(world: &World, request: &WorkspaceRequest) -> io::Result<Entity> {
    let panel = request
        .source
        .and_then(|source| bedtime_api::workbench::panel_owner(world, source))
        .ok_or_else(|| io::Error::other("Workspace promotion requires a live calling terminal"))?;
    let surface = bedtime_core::panel_presentation(world, panel).unwrap_or(panel);
    if world
        .get::<bedtime_api::documents::PanelBinding>(surface)
        .or_else(|| world.get::<bedtime_api::documents::PanelBinding>(panel))
        .and_then(|binding| binding.terminal)
        .is_none()
    {
        return Err(io::Error::other(
            "Workspace promotion requires a live calling terminal",
        ));
    }
    Ok(panel)
}
fn start_open(
    world: &mut World,
    request: WorkspaceRequest,
    spec: WorkspaceSpec,
    reconnect: bool,
    create: bool,
) -> io::Result<()> {
    if !world.contains_resource::<bedtime_core::HostRequests>() {
        let hook = world
            .get_resource::<WorkspaceOpenHandler>()
            .copied()
            .ok_or_else(|| io::Error::other("No document context service is installed"))?;
        world
            .run_system_with(hook.0, spec)
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
        complete_request(world, request, Ok(()));
        return Ok(());
    }
    let preserved_panel = if matches!(request.action, WorkspaceAction::Promote(_)) {
        Some(promotion_panel(world, &request)?)
    } else {
        None
    };
    let retained = preserved_panel.is_some()
        || !reconnect
            && matches!(
                world.resource::<WorkspaceAttachment>(),
                WorkspaceAttachment::Temporary
            )
            && !world
                .non_send::<bedtime_documents::DocumentSession>()
                .is_remote()
            && matches!(spec.target, WorkspaceTarget::Local);
    let mut opening = Opening {
        request,
        spec,
        retained,
        preserved_panel,
        reconnect,
        create,
        snapshot: None,
        #[cfg(feature = "remote")]
        remote: None,
    };
    match &opening.spec.target {
        WorkspaceTarget::Local => {
            let context = bedtime_files::FileContext::local(&opening.spec.root)?;
            opening.spec.root = context.directory_string();
            prepare_open(
                world,
                &mut opening,
                context,
                bedtime_files::FileServices::local(),
            )?;
        }
        WorkspaceTarget::Ssh { host } => {
            #[cfg(feature = "remote")]
            {
                opening.remote = Some(bedtime_core::begin_remote_context(
                    world,
                    host.clone(),
                    opening.spec.root.clone(),
                )?);
            }
            #[cfg(not(feature = "remote"))]
            {
                let _ = host;
                return Err(io::Error::other("SSH workspace support is disabled"));
            }
        }
    }
    world.non_send_mut::<WorkspaceOpening>().0 = Some(opening);
    world.resource_mut::<WorkspacesState>().connecting = true;
    Ok(())
}
fn prepare_open(
    world: &mut World,
    opening: &mut Opening,
    context: bedtime_files::FileContext,
    services: bedtime_files::FileServices,
) -> io::Result<()> {
    opening.spec.root = context.directory_string();
    if !opening.reconnect {
        if let Some(stored) = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .find(&opening.spec)?
        {
            opening.spec = stored;
        }
        opening.snapshot = world
            .resource::<WorkspaceRepositoryResource>()
            .0
            .load(&opening.spec)?;
        if let Some(snapshot) = &opening.snapshot {
            bedtime_storage::validate_snapshot(&snapshot.plugins)?;
            if let Some(history) = &snapshot.history {
                bedtime_documents::DocumentSession::validate_history_snapshot(history)?;
            }
            if !snapshot.layouts.is_null() {
                if let Some(hook) = world.get_resource::<WorkspaceLayoutPreflight>().copied() {
                    world
                        .run_system_with(hook.0, snapshot.layouts.clone())
                        .map_err(io::Error::other)?
                        .map_err(io::Error::other)?;
                } else {
                    bedtime_workbench::validate_stored_layout(&snapshot.layouts)
                        .map_err(io::Error::other)?;
                }
            }
        }
    }
    if !opening.create && !opening.reconnect && opening.snapshot.is_none() {
        return Err(io::Error::other("Workspace has not been saved"));
    }
    let mut options = world
        .non_send::<bedtime_documents::DocumentSession>()
        .options()
        .clone();
    options.project_root = Some(context.directory.clone());
    options.history_path = None;
    let mode = if opening.reconnect {
        bedtime_core::ContextChangeMode::Reconnect
    } else if let Some(panel) = opening.preserved_panel {
        bedtime_core::ContextChangeMode::RetainPanel(panel)
    } else if opening.retained {
        bedtime_core::ContextChangeMode::RetainDocuments
    } else {
        bedtime_core::ContextChangeMode::ReplaceDocuments
    };
    let token = opening
        .request
        .barrier
        .ok_or_else(|| io::Error::other("Workspace context request has no admitted barrier"))?;
    bedtime_core::prepare_context_change(
        world,
        token,
        bedtime_core::ContextChange {
            context,
            services,
            options,
            mode,
        },
    )
    .map_err(io::Error::other)?;
    Ok(())
}
fn poll_open(world: &mut World) {
    let Some(mut opening) = world.non_send_mut::<WorkspaceOpening>().0.take() else {
        return;
    };
    let mut committed = false;
    let result = (|| -> io::Result<bool> {
        #[cfg(feature = "remote")]
        if let Some(receiver) = opening.remote.take() {
            match receiver.try_recv() {
                Ok(Ok((context, services))) => {
                    prepare_open(world, &mut opening, context, services)?
                }
                Ok(Err(error)) => return Err(error),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    opening.remote = Some(receiver);
                    return Ok(false);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::other("Workspace connection worker stopped"));
                }
            }
        }
        let token = opening
            .request
            .barrier
            .expect("shared context has a barrier");
        let status = bedtime_core::poll_context_change(world, token).map_err(io::Error::other)?;
        match status {
            bedtime_core::OperationBarrierStatus::Waiting
            | bedtime_core::OperationBarrierStatus::Ready
            | bedtime_core::OperationBarrierStatus::Pending
            | bedtime_core::OperationBarrierStatus::NeedsDecision(_) => return Ok(false),
            bedtime_core::OperationBarrierStatus::Failed(error) => {
                return Err(io::Error::other(error));
            }
            bedtime_core::OperationBarrierStatus::Prepared => {}
            bedtime_core::OperationBarrierStatus::Complete => {
                return Err(io::Error::other("Workspace context completed unexpectedly"));
            }
        }
        if opening.create && opening.snapshot.is_none() {
            opening.spec = world
                .resource_mut::<WorkspaceRepositoryResource>()
                .0
                .commit(opening.spec.clone(), WorkspaceSnapshot::default())?;
        }
        bedtime_core::commit_prepared_context_change(world, token).map_err(io::Error::other)?;
        committed = true;
        if !opening.reconnect {
            world.insert_resource(WorkspaceAttachment::Saved(opening.spec.clone()));
            let context = world.resource::<bedtime_files::FileContext>();
            let observed = (context.provider.clone(), context.directory.clone());
            world.resource_mut::<WorkspaceAutosave>().context = Some(observed);
            restore_snapshot(
                world,
                opening.snapshot.as_ref(),
                opening.retained,
                opening.preserved_panel,
            )?;
            if let Some(hook) = world.get_resource::<WorkspaceDirectoryEntered>().copied() {
                let entered = world
                    .run_system(hook.0)
                    .map_err(io::Error::other)
                    .and_then(|result| result.map_err(io::Error::other));
                if let Err(error) = entered {
                    world.resource_mut::<WorkspaceAutosave>().restore_error =
                        Some(error.to_string());
                    return Err(error);
                }
            }
            persist_workspace(world)?;
        }
        if !opening.reconnect {
            refresh_workspace_observations(world)?;
        }
        Ok(true)
    })();
    match result {
        Ok(false) => world.non_send_mut::<WorkspaceOpening>().0 = Some(opening),
        other => {
            world.resource_mut::<WorkspacesState>().connecting = false;
            complete_request(
                world,
                opening.request,
                other.map(|_| ()).map_err(|error| {
                    if committed {
                        format!("Workspace opened, but restoring its state failed: {error}")
                    } else {
                        error.to_string()
                    }
                }),
            );
        }
    }
}
fn process_workspace_requests(world: &mut World) {
    let messages = world.resource_scope(|world, mut requests: Mut<WorkspaceRequests>| {
        requests
            .cursor
            .read(world.resource::<Messages<WorkspaceRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world
        .resource_mut::<WorkspaceRequests>()
        .pending
        .extend(messages);
    poll_open(world);
    if world.non_send::<WorkspaceOpening>().0.is_some() {
        return;
    }
    while let Some(request) = world
        .resource_mut::<WorkspaceRequests>()
        .pending
        .pop_front()
    {
        if request.barrier.is_some_and(|token| {
            bedtime_core::barrier_status(world, token)
                == Some(bedtime_core::OperationBarrierStatus::Waiting)
        }) || request.barrier.is_none()
            && world.contains_resource::<bedtime_core::HostRequests>()
            && bedtime_core::operation_barrier_active(world)
        {
            world
                .resource_mut::<WorkspaceRequests>()
                .pending
                .push_front(request);
            break;
        }
        let result = (|| -> io::Result<bool> {
            request_is_current(world, &request)?;
            match request.action.clone() {
                WorkspaceAction::SaveCurrent { name } => {
                    save_current_workspace(world, name, Value::Null)?;
                }
                WorkspaceAction::Persist => persist_workspace(world)?,
                WorkspaceAction::RenameWorkspace(spec, name) => {
                    rename_workspace(world, &spec, &name)?;
                }
                WorkspaceAction::RemoveWorkspace(spec) => world
                    .resource_mut::<WorkspaceRepositoryResource>()
                    .0
                    .forget(&spec)?,
                WorkspaceAction::RemoveRecent(path) => world
                    .resource_mut::<WorkspaceRepositoryResource>()
                    .0
                    .forget(&WorkspaceSpec::local(path.to_string_lossy()))?,
                WorkspaceAction::OpenFolder => {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        start_open(
                            world,
                            request.clone(),
                            WorkspaceSpec::local(path.to_string_lossy()),
                            false,
                            true,
                        )?;
                        return Ok(false);
                    }
                }
                WorkspaceAction::OpenLocalWorkspace(path) => {
                    start_open(
                        world,
                        request.clone(),
                        WorkspaceSpec::local(path.to_string_lossy()),
                        false,
                        true,
                    )?;
                    return Ok(false);
                }
                WorkspaceAction::Promote(path) => {
                    promotion_panel(world, &request)?;
                    let context = world.resource::<bedtime_files::FileContext>();
                    let spec = match &context.provider {
                        bedtime_files::FileProviderId::Local => WorkspaceSpec::local(
                            bedtime_files::FileContext::local(&path)?.directory_string(),
                        ),
                        bedtime_files::FileProviderId::Ssh { host } => WorkspaceSpec {
                            name: crate::default_name(&path.to_string_lossy()),
                            target: WorkspaceTarget::Ssh { host: host.clone() },
                            root: path
                                .to_str()
                                .ok_or_else(|| {
                                    io::Error::other("Workspace directory is not UTF-8")
                                })?
                                .into(),
                        },
                    };
                    let remote_directory_change =
                        matches!(context.provider, bedtime_files::FileProviderId::Ssh { .. })
                            && path != context.directory;
                    if remote_directory_change
                        || matches!(
                            world.resource::<WorkspaceAttachment>(),
                            WorkspaceAttachment::Saved(_)
                        )
                    {
                        let hook = world
                            .get_resource::<WorkspaceWindowHandler>()
                            .copied()
                            .ok_or_else(|| {
                                io::Error::other(
                                    "Opening another window requires application process policy",
                                )
                            })?;
                        world
                            .run_system_with(hook.0, spec)
                            .map_err(io::Error::other)?
                            .map_err(io::Error::other)?;
                    } else {
                        start_open(world, request.clone(), spec, false, true)?;
                        return Ok(false);
                    }
                }
                WorkspaceAction::OpenWorkspace(spec) => {
                    start_open(world, request.clone(), spec, false, true)?;
                    return Ok(false);
                }
                WorkspaceAction::OpenWorkspaceWindow(spec) => {
                    let hook = world
                        .get_resource::<WorkspaceWindowHandler>()
                        .copied()
                        .ok_or_else(|| {
                            io::Error::other(
                                "Opening another window requires application process policy",
                            )
                        })?;
                    world
                        .run_system_with(hook.0, spec)
                        .map_err(io::Error::other)?
                        .map_err(io::Error::other)?;
                }
                WorkspaceAction::Reconnect => {
                    let context = world.resource::<bedtime_files::FileContext>();
                    let bedtime_files::FileProviderId::Ssh { host } = &context.provider else {
                        return Err(io::Error::other("No SSH context is open"));
                    };
                    let spec = WorkspaceSpec {
                        name: crate::default_name(&context.directory_string()),
                        target: WorkspaceTarget::Ssh { host: host.clone() },
                        root: context.directory_string(),
                    };
                    start_open(world, request.clone(), spec, true, false)?;
                    return Ok(false);
                }
                WorkspaceAction::Error(error) => return Err(io::Error::other(error)),
            }
            refresh_workspace_observations(world)?;
            Ok(true)
        })();
        match result {
            Ok(false) => break,
            other => complete_request(
                world,
                request,
                other.map(|_| ()).map_err(|error| error.to_string()),
            ),
        }
    }
}

/// Drive the same lifecycle owner from a normal schedule or an explicit test tick.
pub fn resolve_workspace_requests(world: &mut World) {
    process_workspace_requests(world);
    observe_context(world);
    autosave(world);
}
fn observe_context(world: &mut World) {
    let Some(context) = world.get_resource::<bedtime_files::FileContext>() else {
        return;
    };
    let identity = (context.provider.clone(), context.directory.clone());
    let previous = world.resource::<WorkspaceAutosave>().context.clone();
    world.resource_mut::<WorkspaceAutosave>().context = Some(identity.clone());
    if previous.as_ref() == Some(&identity) || previous.is_none() {
        return;
    }
    if let WorkspaceAttachment::Saved(spec) = world.resource::<WorkspaceAttachment>() {
        let matches = spec.root == identity.1.to_string_lossy()
            && match (&spec.target, &identity.0) {
                (WorkspaceTarget::Local, bedtime_files::FileProviderId::Local) => true,
                (
                    WorkspaceTarget::Ssh { host: a },
                    bedtime_files::FileProviderId::Ssh { host: b },
                ) => a == b,
                _ => false,
            };
        if !matches {
            world.insert_resource(WorkspaceAttachment::Temporary);
        }
    }
    if let Some(hook) = world.get_resource::<WorkspaceDirectoryEntered>().copied()
        && let Err(error) = world
            .run_system(hook.0)
            .map_err(|error| error.to_string())
            .and_then(|result| result)
    {
        world.resource_mut::<WorkspacesState>().error = Some(error);
    }
}
fn autosave(world: &mut World) {
    if !matches!(
        world.resource::<WorkspaceAttachment>(),
        WorkspaceAttachment::Saved(_)
    ) || world
        .resource::<WorkspaceAutosave>()
        .restore_error
        .is_some()
        || world.resource::<WorkspaceAutosave>().last_saved.elapsed() < Duration::from_secs(2)
        || world.contains_resource::<bedtime_core::HostRequests>()
            && bedtime_core::operation_barrier_active(world)
    {
        return;
    }
    // A failed adapter gets the same retry interval as a successful write.
    world.resource_mut::<WorkspaceAutosave>().last_saved = Instant::now();
    if let Err(error) = persist_workspace(world) {
        world.resource_mut::<WorkspacesState>().error = Some(error.to_string());
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
