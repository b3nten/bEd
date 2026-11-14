//! Tab styling and layout graph validation.
use super::*;
use bevy::prelude::*;

/// Optional, runtime-only tab label appearance on a panel item.
///
/// Without this component, normal tabs inherit the theme unchanged and preview
/// tabs use italic text at 90% opacity. An explicit component replaces both
/// defaults without changing [`PanelInstance::preview`] or keep/move behavior.
/// Font size, color, selection and hover emphasis remain theme-owned. Reflected snapshots do not capture this component; applications reapply it.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PanelTabStyle {
    pub italic: bool,
    /// Multiplier on the themed label's opacity, not its icon or controls.
    /// Rendering clamps finite values to `0.0..=1.0`; nonfinite values use `1.0`.
    pub text_opacity: f32,
}

impl Default for PanelTabStyle {
    fn default() -> Self {
        Self {
            italic: false,
            text_opacity: 1.0,
        }
    }
}

/// Check the layout graph: relationships point at the right kinds of entities,
/// area rectangles tile their layout, and selections and focus are in range.
/// Hooks keep all of this true; debug builds check it every frame, and
/// persistence checks it before capturing or restoring.
pub(crate) fn validate_layouts(world: &mut World) -> Result<(), String> {
    world.flush();
    if world
        .query_filtered::<Entity, (With<InLayout>, Without<Area>)>()
        .iter(world)
        .next()
        .is_some()
    {
        return Err("InLayout requires Area".into());
    }
    if world
        .query_filtered::<Entity, (With<InArea>, Without<PanelInstance>)>()
        .iter(world)
        .next()
        .is_some()
    {
        return Err("InArea requires PanelInstance".into());
    }
    let layouts: Vec<_> = world
        .query_filtered::<Entity, With<Layout>>()
        .iter(world)
        .collect();
    let selected = layouts
        .iter()
        .filter(|&&e| world.get::<ActiveLayout>(e).is_some())
        .count();
    if selected > 1 {
        return Err("Multiple ActiveLayout entities".into());
    }
    for entity in world
        .query_filtered::<Entity, With<ActiveLayout>>()
        .iter(world)
    {
        if world.get::<Layout>(entity).is_none() {
            return Err("ActiveLayout requires Layout".into());
        }
    }
    let areas: Vec<_> = world
        .query_filtered::<Entity, With<Area>>()
        .iter(world)
        .collect();
    for &area in &areas {
        let layout = world
            .get::<InLayout>(area)
            .ok_or("Area requires InLayout")?
            .0;
        if world.get::<Layout>(layout).is_none() {
            return Err("Area references a missing Layout".into());
        }
    }
    for &layout in &layouts {
        let areas = world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities());
        if !areas.is_empty() {
            crate::layouts::geometry::Layout::from_areas(
                areas
                    .iter()
                    .enumerate()
                    .map(|(i, &e)| crate::layouts::geometry::Area {
                        id: i as u32 + 1,
                        rect: world.get::<Area>(e).expect("InLayout requires Area").rect,
                    })
                    .collect(),
            )?;
        }
        let focused = world.get::<Layout>(layout).unwrap().focused_area;
        if focused.is_some_and(|entity| !areas.contains(&entity)) {
            return Err("Focused area belongs to another layout or no longer exists".into());
        }
    }
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PanelInstance>>()
        .iter(world)
        .collect();
    for panel in panels {
        let area = world
            .get::<InArea>(panel)
            .ok_or("PanelInstance requires InArea")?
            .0;
        if world.get::<Area>(area).is_none() {
            return Err("Panel references a missing Area".into());
        }
    }
    for area in areas {
        let selected = world.get::<Area>(area).unwrap().selected;
        let tabs = world.layouts().panels_in_area(area);
        if selected.is_some_and(|entity| !tabs.contains(&entity)) {
            return Err("Selected panel belongs to another area or no longer exists".into());
        }
    }
    let extra = world
        .get_resource::<Validations>()
        .map(|validations| validations.0.clone())
        .unwrap_or_default();
    extra.into_iter().try_for_each(|validate| validate(world))
}
