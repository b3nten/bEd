//! bEd's panel opening policy. Callers say how to build a panel (a `PanelFactory`);
//! this module decides whether an existing panel is focused instead, and where a
//! new one goes. Drag-and-drop bypasses this on purpose: dragging a panel out
//! always creates a new one where the user dropped it.
use bedtime_workbench::{
    ActiveLayout, Area, ErrorDialog, InLayout, Layout, Panel, PanelCloseRequested, PanelFactory,
    Workbench, active_layout, request_error_dialog,
};
use bevy::{ecs::message::MessageCursor, prelude::*};

/// What makes two panels of one type show the same thing.
pub trait PanelPolicy: Panel {
    type Key: PartialEq;
    fn key(world: &World, panel: Entity) -> Self::Key;
}

/// Focus an equivalent panel in the active layout, or create one in its largest area.
pub fn open_panel<P: PanelPolicy>(
    world: &mut World,
    key: P::Key,
    factory: PanelFactory,
) -> Result<Entity, String> {
    let layout = active_layout(world).unwrap_or_else(|| new_layout(world));
    let mut workbench = Workbench::new(world);
    let existing = workbench.panels(layout).into_iter().find(|&panel| {
        let world = workbench.world();
        world.get::<P>(panel).is_some() && P::key(world, panel) == key
    });
    let panel = match existing {
        Some(panel) => panel,
        None => {
            let area = workbench.destination_area(layout)?;
            (factory.create)(workbench.world_mut(), area)
        }
    };
    workbench.focus_panel(panel)?;
    Ok(panel)
}

/// Workbench only *requests* closes (closing a tab or split, merging areas); the
/// app approves them. Nothing can be lost yet, so every request is approved.
/// Unsaved-document prompts belong here once documents exist.
pub fn approve_closes(world: &mut World, mut cursor: Local<MessageCursor<PanelCloseRequested>>) {
    let requests: Vec<_> = cursor
        .read(world.resource::<Messages<PanelCloseRequested>>())
        .cloned()
        .collect();
    for request in requests {
        if let Err(error) = Workbench::new(world).commit_close(&request) {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Close panel".into(),
                    message: "This couldn't be closed.".into(),
                    details: Some(error),
                },
            );
        }
    }
}

/// A selected layout with one full-size area.
fn new_layout(world: &mut World) -> Entity {
    let layout = world
        .spawn((
            Layout {
                name: "Layout 1".into(),
                ..default()
            },
            ActiveLayout,
        ))
        .id();
    world.spawn((Area::default(), InLayout(layout)));
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsPanel;
    use bedtime_workbench::{InArea, LayoutEdit, WorkbenchCorePlugin};

    fn open_settings(world: &mut World) -> Entity {
        open_panel::<SettingsPanel>(world, (), PanelFactory::of::<SettingsPanel>()).unwrap()
    }

    #[test]
    fn opening_focuses_an_equivalent_panel_in_the_active_layout() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let world = app.world_mut();

        let first = open_settings(world);
        assert_eq!(open_settings(world), first);

        // A dropped copy is allowed; opening still focuses one rather than adding a third.
        let area = world.get::<InArea>(first).unwrap().0;
        (PanelFactory::of::<SettingsPanel>().create)(world, area);
        open_settings(world);
        assert_eq!(world.query::<&SettingsPanel>().iter(world).count(), 2);
    }

    #[test]
    fn opening_ignores_panels_in_other_layouts() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let world = app.world_mut();

        let first = open_settings(world);
        let layout = Workbench::new(world).create_layout("Other").unwrap();
        world.spawn((Area::default(), InLayout(layout)));
        Workbench::new(world).activate_layout(layout).unwrap();
        assert_ne!(open_settings(world), first);
    }

    #[test]
    fn close_requests_are_approved() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .add_systems(Update, approve_closes);
        let world = app.world_mut();
        let panel = open_settings(world);
        let mut workbench = Workbench::new(world);
        let stamp = workbench.stamp();
        workbench
            .request_panel_close(stamp, LayoutEdit::ClosePanels(vec![panel]))
            .unwrap();
        app.update();
        assert!(app.world().get_entity(panel).is_err());
    }
}
