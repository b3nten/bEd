//! Single-session scheduling and orderly managed-work shutdown.
use crate::*;
use crate::{Workbench, WorkbenchRef, WorkbenchWorldExt};
use bevy::prelude::*;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionSystems {
    Prepare,
    Sync,
    Close,
}

pub(crate) fn install(app: &mut App) {
    if !app.is_plugin_added::<bedtime_work::WorkPlugin>() {
        app.add_plugins(bedtime_work::WorkPlugin);
    }
    app.init_resource::<crate::layout::LayoutOrder>()
        .init_resource::<Notifications>()
        .add_message::<CloseRequested>();
    #[cfg(feature = "ui")]
    app.init_resource::<WorkbenchUi>();
    if !app.is_plugin_added::<bedtime_actions::ActionsPlugin>() {
        app.add_plugins(bedtime_actions::ActionsPlugin);
    }
    // Actions run for the focused panel.
    app.insert_resource(ActionFocus(focused_panel))
        .configure_sets(
            PreUpdate,
            ActionSystems::CollectInput.after(SessionSystems::Prepare),
        )
        .add_systems(
            PreUpdate,
            report_failed_actions.after(ActionSystems::Dispatch),
        );
    documents::install(app);
    closing::install(app);
    document_actions::install(app);
    #[cfg(feature = "ui")]
    layout::install(app);
    app.configure_sets(
        Last,
        bedtime_work::WorkSystems::Poll.in_set(SessionSystems::Close),
    );
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
impl WorkbenchRef {
    pub fn work_finished(&self) -> bool {
        let world = &self.world;
        world.resource::<ManagedWork>().is_finished()
    }
}

impl Workbench {
    pub fn shutdown_work(&mut self) -> Result<(), String> {
        let world = &mut self.world;
        // Saves in flight would be cancelled, and drafts lost. The quit flow waits
        // for both; this is the direct path for headless hosts and tests.
        for id in world.workbench().document_ids() {
            let info = world
                .workbench()
                .inspect_document(id)
                .map_err(|e| e.to_string())?;
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
}

/// The selected panel in the focused area of the active layout.
pub(crate) fn focused_panel(world: &World) -> Option<Entity> {
    let layout = world.workbench().active_layout()?;
    let area = world.get::<Layout>(layout)?.focused_area()?;
    world.get::<Area>(area)?.selected_panel()
}

/// Not being available isn't a failure (a shortcut pressed where it does
/// nothing); a handler that fails is, and the user hears about it.
fn report_failed_actions(
    world: &mut World,
    mut cursor: Local<bevy::ecs::message::MessageCursor<ActionFinished>>,
) {
    let failed: Vec<_> = cursor
        .read(world.resource::<Messages<ActionFinished>>())
        .filter(|finished| finished.ran)
        .filter_map(|finished| {
            let error = finished.result.as_ref().err()?;
            let label = world
                .resource::<ActionRegistry>()
                .label(&finished.invocation.action)
                .unwrap_or(finished.invocation.action.as_str());
            Some(format!("{label}: {error}"))
        })
        .collect();
    for failure in failed {
        world.workbench_mut().notify(Notification::error(failure));
    }
}
