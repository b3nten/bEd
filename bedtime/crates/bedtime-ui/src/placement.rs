#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts::{Panel, SplitAxis};
    use crate::{UiContext, egui};

    #[crate::layouts::panel]
    #[derive(Default)]
    struct Notes;
    impl Panel for Notes {
        const LABEL: &'static str = "Notes";
        fn draw(_: &mut egui::Ui, _: &mut World, _: &UiContext<'_>, _: Entity) {}
    }

    #[crate::layouts::panel]
    #[derive(Default)]
    struct Terminal;
    impl Panel for Terminal {
        const LABEL: &'static str = "Terminal";
        fn draw(_: &mut egui::Ui, _: &mut World, _: &UiContext<'_>, _: Entity) {}
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(crate::layouts::LayoutsPlugin);
        app
    }

    /// The active layout split in two: a narrow left area and a wide right one.
    fn split(world: &mut World) -> (Entity, Entity) {
        let layout = world.layouts_mut().ensure_active().unwrap();
        let left = world.layouts().destination_area(layout).unwrap();
        let right = world
            .layouts_mut()
            .split_area(left, SplitAxis::Horizontal, 2500, [100, 100])
            .unwrap();
        (left, right)
    }

    fn area(world: &World, panel: Entity) -> Entity {
        world.get::<InArea>(panel).unwrap().0
    }

    #[test]
    fn the_macro_requires_the_panel_instance() {
        let mut app = app();
        let panel = app.world_mut().spawn(Notes).id();
        let instance = app.world().get::<PanelInstance>(panel).unwrap();
        assert_eq!(instance.title, "Notes");
        assert_eq!(instance.kind, Some(std::any::TypeId::of::<Notes>()));
    }

    #[test]
    fn placing_without_a_layout_makes_one() {
        let mut app = app();
        let world = app.world_mut();
        assert_eq!(world.layouts().active(), None);
        let panel = world.open_panel(Notes);
        let layout = world.layouts().active().unwrap();
        assert_eq!(world.layouts().panels(layout), vec![panel]);
        assert!(world.get::<Place>(panel).is_none());
        assert_eq!(world.layouts().focused_panel(layout), Some(panel));
    }

    #[test]
    fn auto_goes_beside_its_kind_else_to_the_largest_area() {
        let mut app = app();
        let world = app.world_mut();
        let (left, right) = split(world);
        let notes = world.spawn((Notes, Place::in_area(left))).id();
        world.flush();
        assert_eq!(area(world, notes), left);

        // Beside the other notes, though the left area is the smaller.
        let more = world.open_panel(Notes);
        assert_eq!(area(world, more), left);
        // Nothing like it: the largest area.
        let terminal = world.open_panel(Terminal);
        assert_eq!(area(world, terminal), right);
        assert_eq!(
            area(world, world.open_panel(Terminal)),
            right,
            "beside the first terminal"
        );
    }

    #[test]
    fn unfocused_placement_selects_the_tab_but_keeps_focus() {
        let mut app = app();
        let world = app.world_mut();
        let (left, right) = split(world);
        let notes = world.spawn((Notes, Place::in_area(left))).id();
        world.flush();
        let layout = world.layouts().active().unwrap();
        assert_eq!(world.layouts().focused_panel(layout), Some(notes));

        let terminal = world
            .spawn((Terminal, Place::in_area(right).focus(false)))
            .id();
        world.flush();
        assert_eq!(world.layouts().focused_panel(layout), Some(notes));
        let selected = world.get::<Area>(right).unwrap().selected_panel();
        assert_eq!(selected, Some(terminal));
    }

    #[test]
    fn placing_a_placed_panel_moves_it() {
        let mut app = app();
        let world = app.world_mut();
        let (left, right) = split(world);
        let notes = world.spawn((Notes, Place::in_area(left))).id();
        world.flush();
        world.entity_mut(notes).insert(Place::in_area(right));
        world.flush();
        assert_eq!(area(world, notes), right);
        assert!(world.layouts().panels_in_area(left).is_empty());
    }

    #[test]
    fn a_new_panel_that_cant_be_placed_is_despawned() {
        let mut app = app();
        let world = app.world_mut();
        let (left, _) = split(world);
        let missing = world.spawn_empty().id();
        world.despawn(missing);
        let panel = world.spawn((Notes, Place::in_area(missing))).id();
        world.flush();
        assert!(world.get_entity(panel).is_err());

        // A placed one stays where it was.
        let notes = world.spawn((Notes, Place::in_area(left))).id();
        world.flush();
        world.entity_mut(notes).insert(Place::in_area(missing));
        world.flush();
        assert_eq!(area(world, notes), left);
        app.update();
    }

    #[test]
    fn focus_or_open_focuses_the_one_in_the_active_layout() {
        let mut app = app();
        let world = app.world_mut();
        let first = world.focus_or_open::<Notes>().unwrap();
        world.open_panel(Terminal);
        assert_eq!(world.focus_or_open::<Notes>().unwrap(), first);
        let layout = world.layouts().active().unwrap();
        assert_eq!(world.layouts().focused_panel(layout), Some(first));

        // Not one from another layout.
        let other = world.layouts_mut().create("Other").unwrap();
        world.layouts_mut().activate(other).unwrap();
        assert_ne!(world.focus_or_open::<Notes>().unwrap(), first);
    }

    #[test]
    fn commands_open_panels_too() {
        let mut app = app();
        app.add_systems(Update, |mut commands: Commands| {
            commands.open_panel(Notes);
        });
        app.update();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let panel = world.layouts().find::<Notes>(layout).unwrap();
        assert_eq!(world.layouts().focused_panel(layout), Some(panel));
    }
}
