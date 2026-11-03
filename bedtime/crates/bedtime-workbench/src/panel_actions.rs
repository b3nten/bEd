//! Actions on the focused panel: Close Panel (Cmd/Ctrl+W, through the close
//! checks, as its tab's × does), and Zoom In, Zoom Out and Reset Zoom
//! (`PanelZoom`). A panel with a zoom of its own (an image's) handles those
//! itself, with an `ActionHandler` for each.
use crate::*;
use bevy::prelude::*;

pub struct ClosePanel;
impl Action for ClosePanel {
    const KEY: &'static str = "panel.close";
    const LABEL: &'static str = "Close Panel";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyW)];
}

pub struct ZoomIn;
impl Action for ZoomIn {
    const KEY: &'static str = "panel.zoom_in";
    const LABEL: &'static str = "Zoom In";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::primary(KeyCode::Equal),
        KeyChord::new(Modifiers::PRIMARY.union(Modifiers::SHIFT), KeyCode::Equal),
        KeyChord::primary(KeyCode::NumpadAdd),
    ];
    const REPEATS: bool = true;
}

pub struct ZoomOut;
impl Action for ZoomOut {
    const KEY: &'static str = "panel.zoom_out";
    const LABEL: &'static str = "Zoom Out";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::primary(KeyCode::Minus),
        KeyChord::primary(KeyCode::NumpadSubtract),
    ];
    const REPEATS: bool = true;
}

pub struct ResetZoom;
impl Action for ResetZoom {
    const KEY: &'static str = "panel.reset_zoom";
    const LABEL: &'static str = "Reset Zoom";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::primary(KeyCode::Digit0),
        KeyChord::primary(KeyCode::Numpad0),
    ];
}

pub(crate) fn install(app: &mut App) {
    app.add_action_handler::<ClosePanel>(
        Handler::new(close_panel)
            .available_when(|world, panel| focused_zoom(world, panel).is_some()),
    );
    app.add_action_handler::<ZoomIn>(
        Handler::new(|world, invocation| zoom(world, invocation, PanelZoom::bigger))
            .available_when(|world, panel| {
                focused_zoom(world, panel)
                    .and_then(PanelZoom::bigger)
                    .is_some()
            }),
    )
    .add_action_handler::<ZoomOut>(
        Handler::new(|world, invocation| zoom(world, invocation, PanelZoom::smaller))
            .available_when(|world, panel| {
                focused_zoom(world, panel)
                    .and_then(PanelZoom::smaller)
                    .is_some()
            }),
    )
    .add_action_handler::<ResetZoom>(
        Handler::new(|world, invocation| zoom(world, invocation, |_| Some(1.0)))
            .available_when(|world, panel| focused_zoom(world, panel).is_some_and(|z| z != 1.0)),
    );
}

/// Close the focused panel as its tab's × would: unsaved changes are asked
/// about, and a panel can refuse.
fn close_panel(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let panel = invocation
        .entity
        .filter(|&panel| world.get::<PanelInstance>(panel).is_some())
        .ok_or("No panel is focused")?;
    let layouts = world.layouts_mut();
    let layout = layouts.panel_layout(panel)?;
    layouts.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel]))
}

/// The zoom of `panel`, if it's a panel.
fn focused_zoom(world: &World, panel: Option<Entity>) -> Option<f32> {
    let panel = panel.filter(|&panel| world.get::<PanelInstance>(panel).is_some())?;
    Some(PanelZoom::of(world, panel))
}

fn zoom(
    world: &mut World,
    invocation: ActionInvocation,
    to: fn(f32) -> Option<f32>,
) -> ActionResult {
    let panel = invocation
        .entity
        .filter(|&panel| world.get::<PanelInstance>(panel).is_some())
        .ok_or("No panel is focused")?;
    let zoom = to(PanelZoom::of(world, panel)).ok_or("It can't zoom further")?;
    PanelZoom::set(world, panel, zoom);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TestPanels;

    #[derive(Component)]
    struct ZoomedItself;

    #[test]
    fn zooming_steps_a_panel_unless_it_zooms_itself() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins);
        let world = app.world_mut();
        let layout = crate::tests::layout(world);
        let area = world.layouts().destination_area(layout).unwrap();
        let panel = world
            .layouts_mut()
            .create_panel_in_area(area, "Plain")
            .unwrap();
        let run = |app: &mut App, action: ActionId, times: usize| {
            for _ in 0..times {
                app.world_mut()
                    .resource_mut::<ActionRequests>()
                    .submit(ActionRequest {
                        action: action.clone(),
                        context: InvocationContext::Entity(panel),
                        source: None,
                    });
                app.update();
            }
            app.world().get::<PanelZoom>(panel).map(|zoom| zoom.0)
        };
        assert_eq!(run(&mut app, ActionId::of::<ZoomIn>(), 2), Some(1.25));
        assert!(app.world().get::<ZoomChanged>(panel).is_some());
        assert_eq!(run(&mut app, ActionId::of::<ZoomOut>(), 1), Some(1.1));
        assert_eq!(run(&mut app, ActionId::of::<ResetZoom>(), 1), None);
        // It stops at the smallest step.
        assert_eq!(run(&mut app, ActionId::of::<ZoomOut>(), 20), Some(0.5));
        assert_eq!(run(&mut app, ActionId::of::<ResetZoom>(), 1), None);

        // A panel's own handler wins.
        app.world_mut()
            .entity_mut(panel)
            .insert(ActionHandler::<ZoomIn>::new(Handler::new(
                |world, invocation| {
                    world
                        .entity_mut(invocation.entity.unwrap())
                        .insert(ZoomedItself);
                    Ok(())
                },
            )));
        assert_eq!(run(&mut app, ActionId::of::<ZoomIn>(), 1), None);
        assert!(app.world().get::<ZoomedItself>(panel).is_some());
    }

    #[test]
    fn close_panel_closes_the_focused_panel() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins);
        let world = app.world_mut();
        let layout = crate::tests::layout(world);
        let area = world.layouts().destination_area(layout).unwrap();
        let first = world
            .layouts_mut()
            .create_panel_in_area(area, "First")
            .unwrap();
        let second = world
            .layouts_mut()
            .create_panel_in_area(area, "Second")
            .unwrap();
        world
            .resource_mut::<ActionRequests>()
            .submit(ActionRequest {
                action: ActionId::of::<ClosePanel>(),
                context: InvocationContext::Entity(second),
                source: None,
            });
        app.update();
        app.update();
        let world = app.world();
        assert!(world.get_entity(second).is_err(), "the panel closes");
        assert_eq!(world.layouts().panels_in_area(area), [first]);
    }

    #[test]
    fn steps_go_between_the_levels_and_odd_zooms_are_kept_in_range() {
        assert_eq!(PanelZoom::bigger(1.0), Some(1.1));
        assert_eq!(PanelZoom::bigger(1.17), Some(1.25));
        assert_eq!(PanelZoom::bigger(3.0), None);
        assert_eq!(PanelZoom::smaller(0.5), None);
        assert_eq!(PanelZoom::smaller(1.0), Some(0.9));
        let mut world = World::new();
        let panel = world.spawn_empty().id();
        assert_eq!(PanelZoom::set(&mut world, panel, 9.0), 3.0);
        assert_eq!(PanelZoom::set(&mut world, panel, f32::NAN), 1.0);
        assert!(world.get::<PanelZoom>(panel).is_none());
    }
}
