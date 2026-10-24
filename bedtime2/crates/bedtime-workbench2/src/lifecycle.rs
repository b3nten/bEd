//! Single-session scheduling and orderly managed-work shutdown.
use crate::*;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub(crate) struct LayoutSelectionRevision(pub u64);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionSystems {
    Prepare,
    Sync,
    Close,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ManagedWork>()
        .init_resource::<WorkbenchRequests>()
        .init_resource::<LayoutSelectionRevision>()
        .init_resource::<crate::layout::LayoutOrder>()
        .init_resource::<crate::composition::ReconciledState>()
        .add_message::<CloseRequested>();
    #[cfg(feature = "ui")]
    app.init_resource::<WorkbenchUi>();
    commands::install(app);
    documents::install(app);
    document_commands::install(app);
    layout::install(app);
    #[cfg(feature = "ui")]
    shortcuts::install(app);
    app.add_systems(PreUpdate, reconcile.in_set(SessionSystems::Prepare))
        .add_systems(
            PreUpdate,
            reconcile
                .in_set(SessionSystems::Sync)
                .after(WorkbenchSystems::Apply),
        )
        .add_systems(PostUpdate, reconcile)
        .add_systems(Last, poll_work.in_set(SessionSystems::Close));
    #[cfg(feature = "ui")]
    app.configure_sets(
        PreUpdate,
        SessionSystems::Prepare.before(bedtime_ui2::UiSystems::Draw),
    );
}
fn reconcile(world: &mut World) {
    reconcile_workbench(world).expect("Invalid workbench component graph");
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCloseMode {
    Saved,
    Discard,
}

pub fn preflight_session_close(world: &World, mode: SessionCloseMode) -> Result<(), String> {
    for id in document_ids(world) {
        let info = inspect_document(world, id).map_err(|e| e.to_string())?;
        if info.saving {
            return Err(format!("Document {:?} is still saving", info.title));
        }
        if info.has_drafts {
            return Err(format!(
                "Commit or cancel viewer drafts for {:?} before closing",
                info.title
            ));
        }
        if mode == SessionCloseMode::Saved && (info.dirty || info.conflict) {
            return Err(format!(
                "Save or explicitly discard document {:?} before closing",
                info.title
            ));
        }
    }
    Ok(())
}
pub fn shutdown_work(world: &mut World) -> Result<(), String> {
    reconcile_workbench(world)?;
    preflight_session_close(world, SessionCloseMode::Discard)?;
    world.resource_mut::<ManagedWork>().shutdown();
    Ok(())
}
pub fn work_finished(world: &World) -> bool {
    world.resource::<ManagedWork>().is_finished()
}
fn poll_work(world: &mut World) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|m| !m.is_empty());
    world.resource_scope(|world, mut work: Mut<ManagedWork>| {
        if exiting {
            work.shutdown();
        }
        work.poll(|owner| world.get_entity(owner).is_ok());
    });
}
