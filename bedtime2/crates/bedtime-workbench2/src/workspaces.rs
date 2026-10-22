//! Workspace entities, activation, derived membership and asynchronous shutdown.
use crate::layout::WorkspaceRuntime;
use crate::*;
use bevy::prelude::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(pub(crate) Entity);
impl WorkspaceId {
    pub fn entity(self) -> Entity {
        self.0
    }
}

/// Transfer membership by replacing this immutable relationship.
#[derive(Component, Debug)]
#[component(on_insert = crate::membership::member_changed, on_discard = crate::membership::member_changed)]
#[relationship(relationship_target = WorkspaceMembers)]
pub struct InWorkspace(pub Entity);
#[derive(Component, Default)]
#[relationship_target(relationship = InWorkspace, linked_spawn)]
pub struct WorkspaceMembers(Vec<Entity>);
impl WorkspaceMembers {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}
/// Authoritative application selection. Zero active workspaces is valid.
#[derive(Component, Default)]
#[component(on_add = crate::membership::workspace_selection_changed, on_remove = crate::membership::workspace_selection_changed)]
#[require(Workspace)]
pub struct ActiveWorkspace;
/// Derived marker maintained when selection or membership commands are applied.
#[derive(Component)]
pub struct InActiveWorkspace;
#[derive(Component)]
#[component(on_insert = crate::membership::workspace_changed, on_remove = crate::membership::workspace_changed)]
pub struct WorkspaceRunning;
#[derive(Component)]
pub struct InRunningWorkspace;

pub(crate) fn reconcile(world: &mut World) {
    crate::reconcile_workbench(world).expect("Invalid workbench component graph");
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: WorkspaceId,
    pub name: String,
    pub active: bool,
    pub closing: bool,
    pub background_updates: bool,
}
#[derive(Clone, Debug)]
pub enum WorkspaceRequest {
    Activate(WorkspaceId),
    Close {
        workspace: WorkspaceId,
        replacement: Option<WorkspaceId>,
    },
    /// An explicit discard decision; pending saves and drafts still block it.
    DiscardAndClose {
        workspace: WorkspaceId,
        replacement: Option<WorkspaceId>,
    },
}
#[derive(Resource, Default)]
pub struct WorkspaceRequests(pub VecDeque<WorkspaceRequest>);
#[derive(Message, Debug)]
pub struct WorkspaceFinished {
    pub request: WorkspaceRequest,
    pub result: Result<(), String>,
}
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceSystems {
    Prepare,
    Dispatch,
    /// Validate dispatched edits before ordinary Update systems.
    Sync,
    Close,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<WorkspaceRequests>()
        .init_resource::<crate::composition::ReconciledState>()
        .add_message::<WorkspaceFinished>()
        .add_message::<CloseRequested>();
    commands::install(app);
    documents::install(app);
    document_commands::install(app);
    layout::install(app);
    #[cfg(feature = "ui")]
    shortcuts::install(app);
    app.add_systems(PreUpdate, reconcile.in_set(WorkspaceSystems::Prepare))
        .configure_sets(
            PreUpdate,
            WorkspaceSystems::Dispatch.after(WorkbenchSystems::Apply),
        )
        .add_systems(PreUpdate, dispatch.in_set(WorkspaceSystems::Dispatch))
        .add_systems(
            PreUpdate,
            reconcile
                .in_set(WorkspaceSystems::Sync)
                .after(WorkspaceSystems::Dispatch),
        )
        .add_systems(PostUpdate, reconcile)
        .add_systems(Last, close.in_set(WorkspaceSystems::Close));
    #[cfg(feature = "ui")]
    app.configure_sets(
        PreUpdate,
        (
            WorkspaceSystems::Prepare.before(bedtime_ui2::UiSystems::Draw),
            WorkspaceSystems::Dispatch.after(bedtime_ui2::UiSystems::Draw),
        ),
    );
}

pub(crate) fn selected(world: &World) -> Option<WorkspaceId> {
    let mut query =
        world.try_query_filtered::<Entity, (With<Workspace>, With<ActiveWorkspace>)>()?;
    let mut selected = query
        .iter(world)
        .filter(|&e| !world.get::<WorkspaceRuntime>(e).unwrap().closing);
    let active = selected.next().map(WorkspaceId);
    assert!(
        selected.next().is_none(),
        "Multiple ActiveWorkspace entities"
    );
    active
}
pub(crate) fn active(world: &World) -> WorkspaceId {
    selected(world).expect("No active workspace")
}
pub(crate) fn validate(world: &World, id: WorkspaceId) -> Result<(), String> {
    world
        .get::<Workspace>(id.0)
        .ok_or("Workspace is unavailable")?;
    if world.get::<WorkspaceRuntime>(id.0).unwrap().closing {
        return Err("Workspace is closing".into());
    }
    Ok(())
}
pub(crate) fn owner(world: &World, entity: Entity) -> Option<WorkspaceId> {
    world.get::<InWorkspace>(entity).map(|w| WorkspaceId(w.0))
}

/// Whether closing requires saved content or an explicit decision to discard it.
/// Neither mode permits dropping uncommitted drafts or cancelling a pending save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceCloseMode {
    Saved,
    Discard,
}

/// Read-only close preflight. Applications use the same gate before quit and
/// present their own save/discard/cancel UI. Run it again after asynchronous saves;
/// admission of a save is not proof that the latest content has been saved.
pub fn preflight_workspace_close(
    world: &World,
    workspace: Entity,
    mode: WorkspaceCloseMode,
) -> Result<(), String> {
    world
        .get::<Workspace>(workspace)
        .ok_or("Workspace is unavailable")?;
    for id in document_ids(world, workspace) {
        let info = inspect_document(world, id).map_err(|error| error.to_string())?;
        if info.saving {
            return Err(format!("Document {:?} is still saving", info.title));
        }
        if info.has_drafts {
            return Err(format!(
                "Commit or cancel viewer drafts for {:?} before closing",
                info.title
            ));
        }
        if mode == WorkspaceCloseMode::Saved && (info.dirty || info.conflict) {
            return Err(format!(
                "Save or explicitly discard document {:?} before closing",
                info.title
            ));
        }
    }
    Ok(())
}

/// Existing workspace lifecycle helpers. Direct component composition and storage
/// restoration do not require this facade.
pub struct Workspaces<'w> {
    world: &'w mut World,
}
impl<'w> Workspaces<'w> {
    pub fn new(world: &'w mut World) -> Self {
        assert!(
            world.contains_resource::<WorkspaceRequests>(),
            "Install WorkbenchCorePlugin"
        );
        Self { world }
    }
    /// Legacy strict accessor; use selected() when zero active workspaces is valid.
    pub fn active(&self) -> WorkspaceId {
        active(self.world)
    }
    pub fn selected(&self) -> Option<WorkspaceId> {
        selected(self.world)
    }
    pub fn list(&mut self) -> Vec<WorkspaceInfo> {
        let active = selected(self.world);
        let mut entries: Vec<_> = self
            .world
            .query::<(Entity, &Workspace)>()
            .iter(self.world)
            .map(|(entity, w)| WorkspaceInfo {
                id: WorkspaceId(entity),
                name: w.name.clone(),
                active: active == Some(WorkspaceId(entity)),
                closing: self.world.get::<WorkspaceRuntime>(entity).unwrap().closing,
                background_updates: w.background_updates,
            })
            .collect();
        entries.sort_by_key(|w| w.id);
        entries
    }
    /// Convenience: create a workspace with a default layout and area.
    /// The first becomes active; raw Workspace composition creates none of these.
    pub fn open(&mut self, name: impl Into<String>) -> WorkspaceId {
        let first = selected(self.world).is_none();
        let entity = self
            .world
            .spawn(Workspace {
                name: name.into(),
                ..default()
            })
            .id();
        let layout = layout::new_layout(self.world, WorkspaceId(entity), "Default".into());
        self.world.entity_mut(layout).insert(ActiveLayout);
        if first {
            self.world.entity_mut(entity).insert(ActiveWorkspace);
        }
        reconcile(self.world);
        WorkspaceId(entity)
    }
    pub fn activate(&mut self, id: WorkspaceId) -> Result<(), String> {
        validate(self.world, id)?;
        let previous = selected(self.world);
        if previous == Some(id) {
            return Ok(());
        }
        if let Some(previous) = previous {
            #[cfg(feature = "ui")]
            {
                if self
                    .world
                    .get::<WorkspaceUi>(previous.0)
                    .is_some_and(|ui| ui.drawing)
                {
                    return Err("Queue WorkspaceRequest::Activate while drawing UI".into());
                }
                frontend::deactivate(self.world);
            }
            self.world
                .entity_mut(previous.0)
                .remove::<ActiveWorkspace>();
        }
        self.world.entity_mut(id.0).insert(ActiveWorkspace);
        reconcile(self.world);
        Ok(())
    }
    pub fn set_background_updates(&mut self, id: WorkspaceId, enabled: bool) -> Result<(), String> {
        validate(self.world, id)?;
        self.world
            .get_mut::<Workspace>(id.0)
            .unwrap()
            .background_updates = enabled;
        reconcile(self.world);
        Ok(())
    }
    /// Call after choosing save/discard/cancel for application exit. Never cancel
    /// an in-flight document write; its durable result still needs acknowledgement.
    pub fn shutdown_work(&mut self) -> Result<(), String> {
        crate::reconcile_workbench(self.world)?;
        let workspaces: Vec<_> = self
            .world
            .query_filtered::<Entity, With<Workspace>>()
            .iter(self.world)
            .collect();
        for workspace in workspaces {
            preflight_workspace_close(self.world, workspace, WorkspaceCloseMode::Discard)?;
        }
        for mut work in self
            .world
            .query::<&mut WorkspaceWork>()
            .iter_mut(self.world)
        {
            work.shutdown();
        }
        Ok(())
    }
    pub fn work_finished(&mut self) -> bool {
        self.world
            .query::<&WorkspaceWork>()
            .iter(self.world)
            .all(|w| w.is_finished())
    }
    /// Close only after all documents are saved and drafts are resolved. Members
    /// remain owned until their asynchronous work has stopped. A failed preflight
    /// leaves selection, workers, documents and panels untouched.
    pub fn close(
        &mut self,
        id: WorkspaceId,
        replacement: Option<WorkspaceId>,
    ) -> Result<(), String> {
        self.close_with_mode(id, replacement, WorkspaceCloseMode::Saved)
    }

    /// Explicitly discard unsaved document contents when the workspace retires.
    /// Pending writes and uncommitted viewer drafts still block closure.
    pub fn discard_and_close(
        &mut self,
        id: WorkspaceId,
        replacement: Option<WorkspaceId>,
    ) -> Result<(), String> {
        self.close_with_mode(id, replacement, WorkspaceCloseMode::Discard)
    }

    fn close_with_mode(
        &mut self,
        id: WorkspaceId,
        replacement: Option<WorkspaceId>,
        mode: WorkspaceCloseMode,
    ) -> Result<(), String> {
        self.world
            .get::<Workspace>(id.0)
            .ok_or("Workspace is unavailable")?;
        if self.world.get::<WorkspaceRuntime>(id.0).unwrap().closing {
            return Ok(());
        }
        #[cfg(feature = "ui")]
        if self
            .world
            .get::<WorkspaceUi>(id.0)
            .is_some_and(|ui| ui.drawing)
        {
            return Err("Queue workspace closure while drawing UI".into());
        }
        // Validate the graph before committing any shutdown effects.
        crate::reconcile_workbench(self.world)?;
        preflight_workspace_close(self.world, id.entity(), mode)?;
        if selected(self.world) == Some(id) {
            if let Some(next) = replacement {
                if next == id {
                    return Err("Replacement must be another workspace".into());
                }
                self.activate(next)?;
            } else {
                self.world.entity_mut(id.0).remove::<ActiveWorkspace>();
            }
        }
        self.world
            .get_mut::<WorkspaceRuntime>(id.0)
            .unwrap()
            .closing = true;
        self.world
            .get_mut::<WorkspaceWork>(id.0)
            .unwrap()
            .shutdown();
        reconcile(self.world);
        Ok(())
    }
}

fn dispatch(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<WorkspaceRequests>().0);
    for request in requests {
        let mut workspaces = Workspaces::new(world);
        let result = match request {
            WorkspaceRequest::Activate(id) => workspaces.activate(id),
            WorkspaceRequest::Close {
                workspace,
                replacement,
            } => workspaces.close(workspace, replacement),
            WorkspaceRequest::DiscardAndClose {
                workspace,
                replacement,
            } => workspaces.discard_and_close(workspace, replacement),
        };
        world.write_message(WorkspaceFinished { request, result });
    }
}
fn close(world: &mut World) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|m| !m.is_empty());
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    for entity in workspaces {
        let mut work = world
            .entity_mut(entity)
            .take::<WorkspaceWork>()
            .expect("Workspace work");
        if exiting {
            work.shutdown();
        }
        work.poll(|owner| world.get_entity(owner).is_ok());
        let retire = world.get::<WorkspaceRuntime>(entity).unwrap().closing && work.is_finished();
        if retire {
            world.despawn(entity);
        } else {
            world.entity_mut(entity).insert(work);
        }
    }
}

#[cfg(test)]
mod tests;
