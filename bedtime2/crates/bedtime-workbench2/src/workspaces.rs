//! The host owns workspace lifetimes. Each runtime owns a separate ECS world.
use crate::*;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::{
    app::{Plugins, PluginsState, SubApp},
    prelude::*,
};
use std::collections::BTreeMap;

/// A host-world entity, never an entity in a workspace's world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(Entity);
impl WorkspaceId {
    pub fn entity(self) -> Entity {
        self.0
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct WorkspaceIdentity(WorkspaceId);
impl WorkspaceIdentity {
    pub fn id(&self) -> WorkspaceId {
        self.0
    }
}

/// Application-defined plugins and resources can be installed before opening.
/// Window, renderer and native plugins belong to the host instead.
pub struct WorkspaceRuntime {
    app: SubApp,
    pub background_updates: bool,
    closing: bool,
    started: bool,
    frame: FramePhase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FramePhase {
    Pending,
    Prepared,
    Dispatched,
}

impl WorkspaceRuntime {
    pub fn new(name: impl Into<String>) -> Self {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, WorkspacePlugin));
        #[cfg(feature = "ui")]
        input::install(&mut app);
        app.world_mut().resource_mut::<Workspace>().name = name.into();
        Self {
            app: std::mem::replace(app.main_mut(), SubApp::new()),
            background_updates: true,
            closing: false,
            started: false,
            frame: FramePhase::Pending,
        }
    }

    pub fn add_plugins<M>(&mut self, plugins: impl Plugins<M>) -> &mut Self {
        // Use a temporary App so unsupported nested SubApps fail at registration
        // rather than being silently discarded by SubApp::add_plugins.
        let mut app = App::empty();
        std::mem::swap(&mut self.app, app.main_mut());
        app.add_plugins(plugins);
        assert!(
            app.sub_apps().sub_apps.is_empty(),
            "Workspace plugins cannot create SubApps; install window/render plugins in the host"
        );
        std::mem::swap(&mut self.app, app.main_mut());
        self
    }

    pub fn world(&self) -> &World {
        self.app.world()
    }
    pub fn world_mut(&mut self) -> &mut World {
        self.app.world_mut()
    }

    fn prepare_frame(&mut self) {
        if self.frame != FramePhase::Pending {
            return;
        }
        let phases = self
            .world()
            .resource::<bevy::app::MainScheduleOrder>()
            .labels
            .clone();
        for phase in phases
            .into_iter()
            .take_while(|phase| *phase != PreUpdate.intern())
        {
            let _ = self.world_mut().try_run_schedule(phase);
        }
        self.frame = FramePhase::Prepared;
    }

    fn initialize(&mut self) -> bool {
        assert_eq!(
            self.app.update_schedule,
            Some(Main.intern()),
            "Workspace runtimes use MainScheduleOrder; do not replace the update schedule"
        );
        match self.app.plugins_state() {
            PluginsState::Adding => return false,
            PluginsState::Ready => {
                self.app.finish();
                self.app.cleanup();
            }
            PluginsState::Finished => self.app.cleanup(),
            PluginsState::Cleaned => {}
        }
        if !self.started {
            let phases = self
                .world()
                .resource::<bevy::app::MainScheduleOrder>()
                .startup_labels
                .clone();
            for phase in phases {
                let _ = self.world_mut().try_run_schedule(phase);
            }
            self.started = true;
        }
        assert!(
            self.world()
                .resource::<bevy::app::MainScheduleOrder>()
                .labels
                .contains(&PreUpdate.intern()),
            "Workspace schedules must include PreUpdate"
        );
        true
    }
}

/// Core installed in each workspace, also useful for standalone headless tests.
pub struct WorkspacePlugin;
impl Plugin for WorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CloseRequested>()
            .init_resource::<WorkspaceWork>();
        commands::install(app);
        layout::install(app);
        #[cfg(feature = "native")]
        app.add_message::<NativeError>();
        #[cfg(feature = "ui")]
        {
            app.add_message::<bedtime_ui2::UiError>();
            app.add_message::<bevy::window::RequestRedraw>();
            frontend::install_workspace(app.world_mut());
        }
    }
}

/// Exactly one entry is active; closing runtimes remain owned until work stops.
pub(crate) struct WorkspaceRuntimes {
    active: WorkspaceId,
    entries: BTreeMap<WorkspaceId, WorkspaceRuntime>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceInfo {
    pub id: WorkspaceId,
    pub name: String,
    pub active: bool,
    pub closing: bool,
    pub background_updates: bool,
    pub ready: bool,
}

#[derive(Clone, Debug)]
pub enum WorkspaceRequest {
    Activate(WorkspaceId),
    Close {
        workspace: WorkspaceId,
        replacement: Option<WorkspaceId>,
    },
}

#[derive(Resource, Default)]
pub struct WorkspaceRequests(pub std::collections::VecDeque<WorkspaceRequest>);

#[derive(Message, Debug)]
pub struct WorkspaceFinished {
    pub request: WorkspaceRequest,
    pub result: Result<(), String>,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceSystems {
    Prepare,
    Dispatch,
    Update,
    Close,
}

pub(crate) fn install(app: &mut App) {
    let id = WorkspaceId(app.world_mut().spawn_empty().id());
    let mut initial = WorkspaceRuntime::new("Untitled");
    initial.world_mut().insert_resource(WorkspaceIdentity(id));
    assert!(initial.initialize());
    app.world_mut().insert_non_send(WorkspaceRuntimes {
        active: id,
        entries: BTreeMap::from([(id, initial)]),
    });
    app.configure_sets(
        PreUpdate,
        (WorkspaceSystems::Prepare, WorkspaceSystems::Dispatch).chain(),
    )
    .init_resource::<WorkspaceRequests>()
    .add_message::<WorkspaceFinished>()
    .add_message::<CloseRequested>()
    .add_systems(PreUpdate, prepare.in_set(WorkspaceSystems::Prepare))
    .add_systems(PreUpdate, dispatch.in_set(WorkspaceSystems::Dispatch))
    .add_systems(Update, update.in_set(WorkspaceSystems::Update))
    .add_systems(Last, close.in_set(WorkspaceSystems::Close));
    #[cfg(feature = "ui")]
    app.configure_sets(
        PreUpdate,
        (
            WorkspaceSystems::Prepare.before(bedtime_ui2::UiSystems::Draw),
            WorkspaceSystems::Dispatch
                .after(bedtime_ui2::UiSystems::Draw)
                .before(bedtime_ui2::UiSystems::PublishInput),
        ),
    );
}

/// Host-only lifecycle API. Application policy chooses activation and replacement.
pub struct Workspaces<'w> {
    world: &'w mut World,
}
impl<'w> Workspaces<'w> {
    pub fn new(world: &'w mut World) -> Self {
        assert!(
            world.get_non_send::<WorkspaceRuntimes>().is_some(),
            "Install WorkbenchCorePlugin in the host"
        );
        Self { world }
    }
    pub fn active(&self) -> WorkspaceId {
        self.world.non_send::<WorkspaceRuntimes>().active
    }
    pub fn list(&self) -> Vec<WorkspaceInfo> {
        let runtimes = self.world.non_send::<WorkspaceRuntimes>();
        runtimes
            .entries
            .iter()
            .map(|(&id, runtime)| {
                let world = runtime.world();
                WorkspaceInfo {
                    id,
                    name: world.resource::<Workspace>().name.clone(),
                    active: id == runtimes.active,
                    closing: runtime.closing,
                    background_updates: runtime.background_updates,
                    ready: runtime.started,
                }
            })
            .collect()
    }
    pub fn open(&mut self, mut runtime: WorkspaceRuntime) -> WorkspaceId {
        let id = WorkspaceId(self.world.spawn_empty().id());
        runtime.world_mut().insert_resource(WorkspaceIdentity(id));
        copy_host_services(self.world, runtime.world_mut());
        runtime.initialize();
        self.world
            .non_send_mut::<WorkspaceRuntimes>()
            .entries
            .insert(id, runtime);
        id
    }
    pub fn activate(&mut self, id: WorkspaceId) -> Result<(), String> {
        {
            let runtimes = self.world.non_send::<WorkspaceRuntimes>();
            let runtime = runtimes
                .entries
                .get(&id)
                .ok_or("Workspace is unavailable")?;
            if runtime.closing {
                return Err("Workspace is closing".into());
            }
            if !runtime.started {
                return Err("Workspace plugins are not ready".into());
            }
            if runtimes.active == id {
                return Ok(());
            }
        }
        #[cfg(feature = "ui")]
        if self
            .world
            .contains_resource::<bedtime_ui2::input::RoutedInput>()
            && !self
                .world
                .contains_resource::<bedtime_ui2::input::InputRouter>()
        {
            return Err("Queue WorkspaceRequest::Activate while drawing UI".into());
        }
        #[cfg(feature = "ui")]
        let mut router = self
            .world
            .remove_resource::<bedtime_ui2::input::InputRouter>();
        let mut runtimes = self.world.non_send_mut::<WorkspaceRuntimes>();
        #[cfg(feature = "ui")]
        if let Some(router) = &mut router {
            let old = runtimes.active;
            input::deactivate(runtimes.entries.get_mut(&old).unwrap().world_mut(), router);
        }
        let previous = runtimes.active;
        for target in [previous, id] {
            let world = runtimes.entries.get_mut(&target).unwrap().world_mut();
            world.resource_mut::<Workspace>().activation += 1;
            #[cfg(feature = "ui")]
            frontend::deactivate(world);
        }
        runtimes.active = id;
        drop(runtimes);
        #[cfg(feature = "ui")]
        if let Some(router) = router {
            self.world.insert_resource(router);
        }
        Ok(())
    }
    pub fn set_background_updates(&mut self, id: WorkspaceId, enabled: bool) -> Result<(), String> {
        let mut runtimes = self.world.non_send_mut::<WorkspaceRuntimes>();
        let runtime = runtimes
            .entries
            .get_mut(&id)
            .ok_or("Workspace is unavailable")?;
        if runtime.closing {
            return Err("Workspace is closing".into());
        }
        runtime.background_updates = enabled;
        Ok(())
    }
    /// Application exit preflight: stop work in every workspace, then keep
    /// updating the host until work_finished() before emitting AppExit.
    pub fn shutdown_work(&mut self) {
        for runtime in self
            .world
            .non_send_mut::<WorkspaceRuntimes>()
            .entries
            .values_mut()
        {
            runtime
                .world_mut()
                .resource_mut::<WorkspaceWork>()
                .shutdown();
        }
    }
    pub fn work_finished(&self) -> bool {
        self.world
            .non_send::<WorkspaceRuntimes>()
            .entries
            .values()
            .all(|runtime| runtime.world().resource::<WorkspaceWork>().is_finished())
    }
    /// Call after application save/discard preflight. Shutdown is polled by the host.
    pub fn close(
        &mut self,
        id: WorkspaceId,
        replacement: Option<WorkspaceId>,
    ) -> Result<(), String> {
        {
            let runtimes = self.world.non_send::<WorkspaceRuntimes>();
            let runtime = runtimes
                .entries
                .get(&id)
                .ok_or("Workspace is unavailable")?;
            if runtime.closing {
                return Ok(());
            }
            if id == runtimes.active {
                let next = replacement
                    .filter(|next| *next != id)
                    .ok_or("Closing the active workspace requires a replacement")?;
                let next_runtime = runtimes
                    .entries
                    .get(&next)
                    .ok_or("Replacement workspace is unavailable")?;
                if next_runtime.closing {
                    return Err("Replacement workspace is closing".into());
                }
                if !next_runtime.started {
                    return Err("Replacement workspace plugins are not ready".into());
                }
            }
        }
        if id == self.active() {
            self.activate(replacement.unwrap())?;
        }
        let mut runtimes = self.world.non_send_mut::<WorkspaceRuntimes>();
        let runtime = runtimes.entries.get_mut(&id).unwrap();
        runtime.closing = true;
        runtime
            .world_mut()
            .resource_mut::<WorkspaceWork>()
            .shutdown();
        Ok(())
    }
}

pub(crate) fn selected_world(
    host: &mut World,
    id: Option<WorkspaceId>,
) -> Result<&mut World, String> {
    if host.get_non_send::<WorkspaceRuntimes>().is_none() {
        if id.is_some_and(|id| {
            host.get_resource::<WorkspaceIdentity>()
                .map(|identity| identity.0)
                != Some(id)
        }) {
            return Err("Workspace is unavailable".into());
        }
        return Ok(host);
    }
    let runtimes = host.non_send_mut::<WorkspaceRuntimes>().into_inner();
    let id = id.unwrap_or(runtimes.active);
    let runtime = runtimes
        .entries
        .get_mut(&id)
        .ok_or("Workspace is unavailable")?;
    if runtime.closing {
        return Err("Workspace is closing".into());
    }
    Ok(runtime.world_mut())
}

// Temporarily take the collection so host adapters can borrow both worlds.
#[cfg(any(feature = "ui", feature = "native"))]
pub(crate) fn with_active<R>(host: &mut World, f: impl FnOnce(&mut World, &mut World) -> R) -> R {
    let mut runtimes = host.remove_non_send::<WorkspaceRuntimes>().unwrap();
    let world = runtimes
        .entries
        .get_mut(&runtimes.active)
        .unwrap()
        .world_mut();
    let result = f(host, world);
    host.insert_non_send(runtimes);
    result
}

fn copy_host_services(host: &World, workspace: &mut World) {
    #[cfg(feature = "clipboard")]
    if let Some(clipboard) = host.get_resource::<Clipboard>() {
        workspace.insert_resource(clipboard.clone());
    }
    if let Some(time) = host.get_resource::<Time<Real>>() {
        workspace.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(time.delta()));
    }
}

fn prepare(host: &mut World) {
    let mut runtimes = host.remove_non_send::<WorkspaceRuntimes>().unwrap();
    let active = runtimes.active;
    for (&id, runtime) in runtimes.entries.iter_mut().filter(|(_, r)| !r.closing) {
        runtime.frame = FramePhase::Pending;
        copy_host_services(host, runtime.world_mut());
        if !runtime.initialize() {
            continue;
        }
        #[cfg(feature = "ui")]
        frontend::reset(runtime.world_mut());
        if id == active || runtime.background_updates {
            runtime.prepare_frame();
        }
    }
    host.insert_non_send(runtimes);
}

fn dispatch(host: &mut World) {
    let mut runtimes = host.remove_non_send::<WorkspaceRuntimes>().unwrap();
    {
        let runtime = runtimes.entries.get_mut(&runtimes.active).unwrap();
        runtime.prepare_frame();
        runtime.frame = FramePhase::Dispatched;
        let world = runtime.world_mut();
        #[cfg(feature = "ui")]
        if let Some(router) = host.remove_resource::<bedtime_ui2::input::InputRouter>() {
            world.insert_resource(router);
        }
        let _ = world.try_run_schedule(PreUpdate);
        #[cfg(feature = "ui")]
        if let Some(router) = world.remove_resource::<bedtime_ui2::input::InputRouter>() {
            host.insert_resource(router);
        }

        for request in world.resource_mut::<Messages<CloseRequested>>().drain() {
            host.write_message(request);
        }
    }
    host.insert_non_send(runtimes);
    let requests = std::mem::take(&mut host.resource_mut::<WorkspaceRequests>().0);
    for request in requests {
        let mut workspaces = Workspaces::new(host);
        let result = match request {
            WorkspaceRequest::Activate(id) => workspaces.activate(id),
            WorkspaceRequest::Close {
                workspace,
                replacement,
            } => workspaces.close(workspace, replacement),
        };
        host.write_message(WorkspaceFinished { request, result });
    }
}

fn update(host: &mut World) {
    let mut runtimes = host.remove_non_send::<WorkspaceRuntimes>().unwrap();
    let active = runtimes.active;
    for (&id, runtime) in &mut runtimes.entries {
        if runtime.closing {
            continue;
        }
        if !runtime.started {
            poll_work(runtime.world_mut());
            continue;
        }
        if id == active || runtime.background_updates {
            runtime.prepare_frame();
            if runtime.frame != FramePhase::Dispatched {
                let _ = runtime.world_mut().try_run_schedule(PreUpdate);
                runtime.frame = FramePhase::Dispatched;
            }
            let phases = runtime
                .world()
                .resource::<bevy::app::MainScheduleOrder>()
                .labels
                .clone();
            for phase in phases
                .into_iter()
                .skip_while(|phase| *phase != PreUpdate.intern())
                .skip(1)
            {
                let _ = runtime.world_mut().try_run_schedule(phase);
            }
            for request in runtime
                .world_mut()
                .resource_mut::<Messages<CloseRequested>>()
                .drain()
            {
                host.write_message(request);
            }
            runtime.world_mut().clear_trackers();
        }
        poll_work(runtime.world_mut());
    }
    host.insert_non_send(runtimes);
}

fn poll_work(world: &mut World) {
    world.resource_scope(|world, mut work: Mut<WorkspaceWork>| {
        work.poll(|owner| world.get_entity(owner).is_ok());
    });
}

fn close(host: &mut World) {
    let exiting = host
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|m| !m.is_empty());
    let mut runtimes = host.remove_non_send::<WorkspaceRuntimes>().unwrap();
    let mut retired = Vec::new();
    for (&id, runtime) in &mut runtimes.entries {
        if exiting {
            runtime
                .world_mut()
                .resource_mut::<WorkspaceWork>()
                .shutdown();
        }
        if runtime.closing {
            poll_work(runtime.world_mut());
            if runtime.world().resource::<WorkspaceWork>().is_finished() {
                retired.push(id);
            }
        }
    }
    for id in retired {
        runtimes.entries.remove(&id);
        host.despawn(id.0);
    }
    host.insert_non_send(runtimes);
}

#[cfg(test)]
mod tests;
