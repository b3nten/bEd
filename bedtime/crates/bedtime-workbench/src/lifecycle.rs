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
    app.init_resource::<Notifications>()
        .add_message::<CloseRequested>()
        // Uncommitted input must be committed or cancelled before its panel
        // closes; closes the user asks for check for unsaved work first.
        .check_panel_close(|world, panels| {
            if panels
                .iter()
                .any(|&panel| world.get::<DocumentDraft>(panel).is_some())
            {
                return Err("Commit or cancel document drafts before closing their viewers".into());
            }
            Ok(())
        })
        .on_panel_close(closing::ask_before_closing)
        .insert_resource(PanelModified(|world, panel| {
            world.workbench().holder_unsaved(panel)
        }))
        .add_layout_validation(documents::validate_documents);
    // The native menu bar is installed and read while the session prepares,
    // before the frame's layout edits apply.
    #[cfg(feature = "native")]
    app.configure_sets(
        PreUpdate,
        (
            crate::NativeSystems::Poll,
            crate::menus::NativeMenuSystems::Poll,
        )
            .after(SessionSystems::Prepare)
            .before(LayoutSystems::Apply),
    );
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
    app.init_resource::<Openers>()
        .add_systems(Update, opening::pin_edited_previews);
    #[cfg(feature = "files")]
    app.register_type::<ViewerFile>()
        .add_systems(Update, opening::reopen_viewers);
    closing::install(app);
    document_actions::install(app);
    panel_actions::install(app);
    // A gesture's layout edit applies before shortcuts are read.
    app.configure_sets(
        PreUpdate,
        LayoutSystems::Apply.before(ActionSystems::CollectInput),
    );
    app.configure_sets(
        Last,
        crate::work::WorkSystems::Poll.in_set(SessionSystems::Close),
    );
    #[cfg(feature = "ui")]
    app.configure_sets(
        PreUpdate,
        SessionSystems::Prepare.before(bedtime_ui::UiSystems::Draw),
    );
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
    let layout = world.layouts().active()?;
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
