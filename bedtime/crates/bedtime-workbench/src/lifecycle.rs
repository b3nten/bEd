//! Single-session scheduling: what the session installs, with and without
//! documents.
use crate::*;
use crate::WorkbenchWorldExt;
use bevy::prelude::*;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionSystems {
    Prepare,
    Sync,
    Close,
}

/// The session without documents: notifications, quitting, which panel
/// actions are for, and the order of a frame.
pub(crate) fn install(app: &mut App) {
    app.init_resource::<Notifications>()
        .add_message::<CloseRequested>();
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
    exit::install(app);
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

/// Documents in the session: their store and viewers, what closing a panel
/// or exiting does about unsaved ones, and New, Save, Undo and Redo.
#[cfg(feature = "documents")]
pub(crate) fn install_documents(app: &mut App) {
    // Uncommitted input must be committed or cancelled before its panel
    // closes; closes the user asks for check for unsaved work first.
    app.check_panel_close(|world, panels| {
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
    documents::install(app);
    #[cfg(feature = "files")]
    app.init_resource::<Formats>();
    app.init_resource::<Viewers>()
        .add_observer(opening::check_viewer_model)
        .add_systems(Update, opening::pin_edited_previews);
    #[cfg(feature = "files")]
    app.register_type::<ViewerFile>()
        .add_systems(Update, opening::reopen_viewers);
    closing::install(app);
    document_actions::install(app);
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
