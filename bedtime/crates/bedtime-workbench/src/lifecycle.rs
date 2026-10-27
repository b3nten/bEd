//! Single-session scheduling and orderly managed-work shutdown.
use crate::*;
use bevy::prelude::*;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionSystems {
    Prepare,
    Sync,
    Close,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<ManagedWork>()
        .init_resource::<crate::layout::LayoutOrder>()
        .add_message::<CloseRequested>();
    #[cfg(feature = "ui")]
    app.init_resource::<WorkbenchUi>();
    commands::install(app);
    documents::install(app);
    closing::install(app);
    document_commands::install(app);
    #[cfg(feature = "ui")]
    layout::install(app);
    #[cfg(feature = "ui")]
    shortcuts::install(app);
    app.add_systems(Last, poll_work.in_set(SessionSystems::Close));
    #[cfg(debug_assertions)]
    app.add_systems(Last, validate.before(SessionSystems::Close));
    #[cfg(feature = "ui")]
    app.configure_sets(
        PreUpdate,
        SessionSystems::Prepare.before(bedtime_ui::UiSystems::Draw),
    );
}
#[cfg(debug_assertions)]
fn validate(world: &mut World) {
    validate_workbench(world).expect("Invalid workbench component graph");
}
pub fn shutdown_work(world: &mut World) -> Result<(), String> {
    // Saves in flight would be cancelled, and drafts lost. The quit flow waits
    // for both; this is the direct path for headless hosts and tests.
    for id in document_ids(world) {
        let info = inspect_document(world, id).map_err(|e| e.to_string())?;
        if info.saving || info.has_drafts {
            return Err(format!(
                "{:?} is still saving or being typed in",
                info.title
            ));
        }
    }
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
