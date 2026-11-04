//! Placing panels: `Place` says where a panel should go, and its insert hook
//! picks the area.
//!
//! Spawn a panel with `Place` (or insert it on a placed panel, to move it)
//! and by the next flush it has an `InArea` in the active layout, and
//! `Place` is gone. `InArea` is where `Place` ended up: read it, but place
//! panels with `Place`. Spawning with `InArea` directly still works, for
//! layouts' own code and restored layouts.
//!
//! Opening always makes a new panel. To show an existing one instead, look
//! for it with `layouts().find::<P>(layout)`; `focus_or_open::<P>()` does
//! both for a panel built from `Default`.
use super::{Area, InArea, LayoutsWorldExt, PanelInstance};
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};

/// Where `Place` puts a panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// An area of the active layout that already holds a panel of the same
    /// type (the focused one, else the most recently focused), otherwise the
    /// largest area.
    #[default]
    Auto,
    /// The largest area of the active layout.
    Largest,
    /// This area.
    In(Entity),
}

/// Put this panel in an area: by the next flush it has `InArea`, and this is
/// removed. With `focus`, the panel is focused too; without, it's still its
/// area's selected tab, but the layout's focus stays where it was.
///
/// A new panel that can't be placed (no layout can be made, an area that
/// doesn't exist) is despawned with an error logged: a panel can't exist
/// outside an area. A placed panel that can't be moved stays where it is.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[component(on_insert = place_inserted)]
pub struct Place {
    pub at: Placement,
    pub focus: bool,
}

impl Default for Place {
    fn default() -> Self {
        Self::auto()
    }
}

impl Place {
    pub fn auto() -> Self {
        Self::at(Placement::Auto)
    }
    pub fn largest() -> Self {
        Self::at(Placement::Largest)
    }
    pub fn in_area(area: Entity) -> Self {
        Self::at(Placement::In(area))
    }
    /// At `placement`, focused.
    pub fn at(placement: Placement) -> Self {
        Self {
            at: placement,
            focus: true,
        }
    }
    pub fn focus(self, focus: bool) -> Self {
        Self { focus, ..self }
    }
}

/// What makes a layout active when a panel is placed and none is: by
/// default, a new one named "Layout 1". An app with its own layouts (numbered
/// slots, say) sets its own.
#[derive(Resource, Clone, Copy)]
pub struct LayoutFallback(pub fn(&mut World) -> Result<Entity, String>);

impl Default for LayoutFallback {
    fn default() -> Self {
        Self(|world| {
            let layouts = world.layouts_mut();
            let layout = layouts.create("Layout 1")?;
            layouts.activate(layout)?;
            Ok(layout)
        })
    }
}

impl super::Layouts {
    /// The active layout, making one with `LayoutFallback` if there's none.
    pub fn ensure_active(&mut self) -> Result<Entity, String> {
        if let Some(layout) = self.active() {
            return Ok(layout);
        }
        let fallback = self
            .world
            .get_resource::<LayoutFallback>()
            .copied()
            .unwrap_or_default();
        let layout = (fallback.0)(&mut self.world)?;
        match self.active() {
            Some(active) if active == layout => Ok(layout),
            _ => Err("The layout fallback didn't make a layout active".into()),
        }
    }
}

impl super::LayoutsRef {
    /// The first panel of type `P` in `layout`, in area then tab order.
    pub fn find<P: Component>(&self, layout: Entity) -> Option<Entity> {
        self.panels(layout)
            .into_iter()
            .find(|&panel| self.world().get::<P>(panel).is_some())
    }
}

fn place_inserted(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world
        .commands()
        .queue(move |world: &mut World| place(world, entity));
}

fn place(world: &mut World, panel: Entity) {
    let Ok(mut entity) = world.get_entity_mut(panel) else {
        return;
    };
    let Some(place) = entity.take::<Place>() else {
        return;
    };
    let placed = entity.contains::<InArea>();
    let result = area_for(world, panel, place.at).and_then(|area| {
        if placed {
            world.layouts_mut().relocate_panel(panel, area)?;
        } else {
            world.entity_mut(panel).insert(InArea(area));
        }
        if place.focus {
            world.layouts_mut().focus_panel(panel)?;
        }
        Ok(())
    });
    if let Err(error) = result {
        if placed {
            error!("Couldn't move panel {panel}: {error}");
        } else {
            error!("Couldn't place panel {panel}: {error}");
            world.despawn(panel);
        }
    }
}

fn area_for(world: &mut World, panel: Entity, placement: Placement) -> Result<Entity, String> {
    let kind = world
        .get::<PanelInstance>(panel)
        .ok_or("It isn't a panel (it has no PanelInstance)")?
        .kind;
    match placement {
        Placement::In(area) => {
            world.get::<Area>(area).ok_or("That area doesn't exist")?;
            Ok(area)
        }
        Placement::Largest => {
            let layout = world.layouts_mut().ensure_active()?;
            world.layouts().destination_area(layout)
        }
        Placement::Auto => {
            let layout = world.layouts_mut().ensure_active()?;
            world.layouts().area_for(layout, |world, other| {
                other != panel
                    && kind.is_some()
                    && world.get::<PanelInstance>(other).map(|p| p.kind) == Some(kind)
            })
        }
    }
}

/// Opening panels on a `World`.
pub trait PanelsWorldExt {
    /// Spawn `bundle` (a panel and its state) with `Place::auto()`, placed
    /// and focused when this returns.
    fn open_panel(&mut self, bundle: impl Bundle) -> Entity;
    /// Focus the first `P` in the active layout, else open a new one.
    fn focus_or_open<P: Component + Default>(&mut self) -> Result<Entity, String>;
}

impl PanelsWorldExt for World {
    fn open_panel(&mut self, bundle: impl Bundle) -> Entity {
        let panel = self.spawn((bundle, Place::auto())).id();
        self.flush();
        panel
    }

    fn focus_or_open<P: Component + Default>(&mut self) -> Result<Entity, String> {
        let layout = self.layouts_mut().ensure_active()?;
        if let Some(panel) = self.layouts().find::<P>(layout) {
            self.layouts_mut().focus_panel(panel)?;
            return Ok(panel);
        }
        let panel = self.open_panel(P::default());
        self.get_entity(panel)
            .map(|_| panel)
            .map_err(|_| "The panel couldn't be placed".into())
    }
}

/// Opening panels from `Commands`.
pub trait PanelsCommandsExt {
    /// Spawn `bundle` (a panel and its state) with `Place::auto()`: placed
    /// and focused once the commands apply.
    fn open_panel(&mut self, bundle: impl Bundle) -> EntityCommands<'_>;
}

impl PanelsCommandsExt for Commands<'_, '_> {
    fn open_panel(&mut self, bundle: impl Bundle) -> EntityCommands<'_> {
        self.spawn((bundle, Place::auto()))
    }
}

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
        // Nothing like it: the largest area, then beside it.
        let terminal = world.open_panel(Terminal);
        assert_eq!(area(world, terminal), right);
        let another = world.open_panel(Terminal);
        assert_eq!(area(world, another), right);
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
