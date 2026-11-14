//! Placing panels: `Place` says where a panel should go, and its insert hook
//! picks the area.
//!
//! Spawn a panel with `Place` (or insert it on a placed panel, to move it)
//! and by the next flush it has an `InArea` in the active layout, and
//! `Place` is gone. `InArea` is where `Place` ended up: read it, but place
//! panels with `Place`. Spawning with `InArea` directly still works, for
//! layouts' own code and restored layouts.
//!
//! Opening always makes a new panel, whatever is open already: several
//! panels of a type are ordinary. (To show an existing one instead, look for
//! it with `layouts().find::<P>(layout)`.) Where it goes depends on who
//! asked, not on what kinds of panels are where: `Placement`.
use super::{Area, InArea, InLayout, LayoutsWorldExt, PanelInstance, SplitAxis};
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};

/// Where `Place` puts a panel. None of these looks at what kinds of panels
/// the areas hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// The focused area of the active layout (the largest, with none
    /// focused): where the user is looking. For what has no place of its
    /// own to say otherwise: a menu item, a shortcut.
    #[default]
    Focused,
    /// The largest area of the active layout. For what the user can also
    /// drag to an exact place (a titlebar button).
    Largest,
    /// This area.
    In(Entity),
    /// Opened from this panel, so somewhere else: the largest area of its
    /// layout other than its own; with no other, its area split in two
    /// along its longer side (the new panel on the right, or below); too
    /// small to split, its own area. A panel that's gone is `Focused`.
    From(Entity),
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
        Self::focused()
    }
}

impl Place {
    pub fn focused() -> Self {
        Self::at(Placement::Focused)
    }
    pub fn largest() -> Self {
        Self::at(Placement::Largest)
    }
    pub fn in_area(area: Entity) -> Self {
        Self::at(Placement::In(area))
    }
    pub fn from(panel: Entity) -> Self {
        Self::at(Placement::From(panel))
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
    world
        .get::<PanelInstance>(panel)
        .ok_or("It isn't a panel (it has no PanelInstance)")?;
    match placement {
        Placement::In(area) => {
            world.get::<Area>(area).ok_or("That area doesn't exist")?;
            Ok(area)
        }
        Placement::Largest => {
            let layout = world.layouts_mut().ensure_active()?;
            world.layouts().destination_area(layout)
        }
        Placement::Focused => {
            let layout = world.layouts_mut().ensure_active()?;
            world.layouts().focused_or_largest_area(layout)
        }
        Placement::From(source) => {
            if world.get::<InArea>(source).is_none() {
                let layout = world.layouts_mut().ensure_active()?;
                return world.layouts().focused_or_largest_area(layout);
            }
            world.layouts_mut().area_from(source)
        }
    }
}

impl super::LayoutsRef {
    /// `layout`'s focused area, else its largest.
    pub fn focused_or_largest_area(&self, layout: Entity) -> Result<Entity, String> {
        let focused = self
            .world()
            .get::<super::Layout>(layout)
            .and_then(|layout| layout.focused_area)
            .filter(|&area| self.world().get::<Area>(area).is_some());
        match focused {
            Some(area) => Ok(area),
            None => self.destination_area(layout),
        }
    }
}

impl super::Layouts {
    /// Where `Placement::From(panel)` puts a panel: the largest area of
    /// `panel`'s layout other than its own; with no other, its area split in
    /// two (`split_in_two`); too small to split, its own area.
    pub fn area_from(&mut self, panel: Entity) -> Result<Entity, String> {
        let own = self
            .world
            .get::<InArea>(panel)
            .ok_or("The panel it's opened from isn't in an area")?
            .0;
        let layout = self
            .world
            .get::<InLayout>(own)
            .ok_or("The panel it's opened from isn't in a layout")?
            .0;
        let size = |area: &&Entity| {
            let rect = self.world.get::<Area>(**area).unwrap().rect();
            i64::from(rect.max[0] - rect.min[0]) * i64::from(rect.max[1] - rect.min[1])
        };
        let other = self
            .world
            .get::<super::LayoutAreas>(layout)
            .map_or(&[][..], |areas| areas.entities())
            .iter()
            .filter(|&&area| area != own)
            .max_by_key(size)
            .copied();
        Ok(match other {
            Some(area) => area,
            None => self.split_in_two(own).unwrap_or(own),
        })
    }

    /// Split `area` in half along its longer side as drawn (side by side
    /// when it's as tall as wide, or hasn't been drawn), returning the new
    /// half: the right or the lower. `None` if it's too small.
    pub fn split_in_two(&mut self, area: Entity) -> Option<Entity> {
        let rect = self.world.get::<Area>(area)?.rect();
        // Layout units are the same on both axes whatever the shape: scale
        // them by what the panels were drawn in.
        let drawn = {
            let mut surfaces = self
                .world
                .query_filtered::<&super::SurfacePresentation, With<super::LayoutSurface>>();
            surfaces
                .iter(&self.world)
                .map(|presentation| presentation.panels)
                .find(|panels| panels.width() > 0.0 && panels.height() > 0.0)
        };
        let (x, y) = drawn.map_or((1.0, 1.0), |panels| (panels.width(), panels.height()));
        let width = (rect.max[0] - rect.min[0]) as f32 * x;
        let height = (rect.max[1] - rect.min[1]) as f32 * y;
        let axis = if width >= height {
            SplitAxis::Horizontal
        } else {
            SplitAxis::Vertical
        };
        let index = usize::from(axis == SplitAxis::Vertical);
        let at = (rect.min[index] + rect.max[index]) / 2;
        self.split_area(area, axis, at, [1000, 1000]).ok()
    }
}

/// Opening panels on a `World`.
pub trait PanelsWorldExt {
    /// Spawn `bundle` (a panel and its state) with `Place::focused()`,
    /// placed and focused when this returns.
    fn open_panel(&mut self, bundle: impl Bundle) -> Entity;
    /// Spawn `bundle` at `placement`, placed and focused when this returns;
    /// an error if it couldn't be placed.
    fn open_panel_at(
        &mut self,
        bundle: impl Bundle,
        placement: Placement,
    ) -> Result<Entity, String>;
}

impl PanelsWorldExt for World {
    fn open_panel(&mut self, bundle: impl Bundle) -> Entity {
        let panel = self.spawn((bundle, Place::focused())).id();
        self.flush();
        panel
    }

    fn open_panel_at(
        &mut self,
        bundle: impl Bundle,
        placement: Placement,
    ) -> Result<Entity, String> {
        let panel = self.spawn((bundle, Place::at(placement))).id();
        self.flush();
        self.get_entity(panel)
            .map(|_| panel)
            .map_err(|_| "The panel couldn't be placed".into())
    }
}

/// Opening panels from `Commands`.
pub trait PanelsCommandsExt {
    /// Spawn `bundle` (a panel and its state) with `Place::focused()`:
    /// placed and focused once the commands apply.
    fn open_panel(&mut self, bundle: impl Bundle) -> EntityCommands<'_>;
}

impl PanelsCommandsExt for Commands<'_, '_> {
    fn open_panel(&mut self, bundle: impl Bundle) -> EntityCommands<'_> {
        self.spawn((bundle, Place::focused()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts::{LayoutAreas, Panel, SplitAxis};
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
    fn focused_is_the_focused_area_and_largest_the_largest_whatever_they_hold() {
        let mut app = app();
        let world = app.world_mut();
        let (left, right) = split(world);
        let layout = world.layouts().active().unwrap();
        // The small area is focused (it can be empty): panels open there,
        // one after another, though there's a larger area.
        world.layouts_mut().focus_area(left).unwrap();
        let notes = world.open_panel(Notes);
        assert_eq!(area(world, notes), left);
        let terminal = world.open_panel(Terminal);
        assert_eq!(area(world, terminal), left);
        // Largest ignores the focus, and where its kind is.
        let more = world.open_panel_at(Notes, Placement::Largest).unwrap();
        assert_eq!(area(world, more), right);
        // That focused the largest: the next focused one follows.
        let another = world.open_panel(Terminal);
        assert_eq!(area(world, another), right);
        assert_eq!(world.layouts().focused_panel(layout), Some(another));
    }

    #[test]
    fn from_is_the_largest_other_area_else_a_split_along_the_longer_side() {
        let mut app = app();
        let world = app.world_mut();
        // Alone: its area is split, side by side when its shape isn't known
        // (or it's wide), the new panel on the right.
        let notes = world.open_panel(Notes);
        let layout = world.layouts().active().unwrap();
        let own = area(world, notes);
        let count = |world: &World| world.get::<LayoutAreas>(layout).unwrap().entities().len();
        let first = world.spawn((Terminal, Place::from(notes))).id();
        world.flush();
        let beside = area(world, first);
        assert_ne!(beside, own);
        let (old, new) = (
            world.get::<Area>(own).unwrap().rect(),
            world.get::<Area>(beside).unwrap().rect(),
        );
        assert_eq!(old.max[0], new.min[0]);
        assert_eq!((old.min[1], old.max[1]), (new.min[1], new.max[1]));
        // Now there's another area: no more splits, whatever it holds.
        let second = world.spawn((Notes, Place::from(notes))).id();
        world.flush();
        assert_eq!(area(world, second), beside);
        assert_eq!(count(world), 2);
        // Of several others, the largest; never its own, though it's larger.
        let small = world
            .layouts_mut()
            .split_area(beside, SplitAxis::Vertical, 8000, [100, 100])
            .unwrap();
        let third = world.spawn((Terminal, Place::from(notes))).id();
        world.flush();
        assert_eq!(area(world, third), beside);
        let fourth = world.spawn((Terminal, Place::from(first))).id();
        world.flush();
        assert_eq!(area(world, fourth), own);
        assert!(world.layouts().panels_in_area(small).is_empty());

        // Drawn tall, a lone area is stacked: the new panel below.
        let other = world.layouts_mut().create("Tall").unwrap();
        world.layouts_mut().activate(other).unwrap();
        world.spawn((
            crate::layouts::LayoutSurface::default(),
            crate::layouts::SurfacePresentation {
                panels: bevy::math::Rect::new(0.0, 0.0, 400.0, 900.0),
                ..default()
            },
        ));
        let notes = world.open_panel(Notes);
        let terminal = world.spawn((Terminal, Place::from(notes))).id();
        world.flush();
        let (old, new) = (
            world.get::<Area>(area(world, notes)).unwrap().rect(),
            world.get::<Area>(area(world, terminal)).unwrap().rect(),
        );
        assert_eq!(old.max[1], new.min[1]);
        assert_eq!((old.min[0], old.max[0]), (new.min[0], new.max[0]));

        // From a panel that's gone: the focused area.
        world.despawn(notes);
        let lost = world.spawn((Notes, Place::from(notes))).id();
        world.flush();
        assert_eq!(area(world, lost), area(world, terminal));
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
    fn opening_makes_a_new_panel_though_one_like_it_is_open() {
        let mut app = app();
        let world = app.world_mut();
        let first = world.open_panel(Notes);
        let second = world.open_panel(Notes);
        assert_ne!(first, second);
        let layout = world.layouts().active().unwrap();
        assert_eq!(world.layouts().panels(layout), vec![first, second]);
        assert_eq!(world.layouts().focused_panel(layout), Some(second));
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
