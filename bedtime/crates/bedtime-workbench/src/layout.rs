//! ECS owns the tiling. Geometry snapshots are temporary calculations, never a
//! second live model. This module alone commits membership and topology changes.
#[cfg(feature = "persistence")]
use crate::persistence::ReflectWorkbenchPersist;
use crate::{Workbench, geometry};
#[cfg(feature = "persistence")]
use bevy::ecs::reflect::ReflectMapEntities;
use bevy::prelude::*;
pub use geometry::EXTENT as LAYOUT_EXTENT;
pub use geometry::Rect as AreaRect;
use std::collections::{HashMap, VecDeque};

#[derive(Component, Default)]
pub(crate) struct LayoutRevision(pub u64);

/// Stable layout order. Entity allocation and archetype iteration are not order.
#[derive(Resource, Default)]
#[cfg_attr(
    feature = "persistence",
    derive(Reflect, Clone, bevy::ecs::entity::MapEntities)
)]
#[cfg_attr(
    feature = "persistence",
    reflect(Resource, MapEntities, WorkbenchPersist)
)]
pub(crate) struct LayoutOrder(#[cfg_attr(feature = "persistence", entities)] pub Vec<Entity>);
pub fn layouts(world: &World) -> &[Entity] {
    &world.resource::<LayoutOrder>().0
}

/// Selected layout in this World. Zero or one may be selected.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[require(Layout)]
#[component(on_add = crate::membership::layout_selection_changed, on_remove = crate::membership::layout_selection_changed)]
pub struct ActiveLayout;

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[require(LayoutAreas, LayoutRevision)]
#[component(on_add = crate::membership::layout_added, on_remove = crate::membership::layout_removed)]
pub struct Layout {
    pub name: String,
    #[cfg_attr(feature = "persistence", entities)]
    pub focused_area: Option<Entity>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            name: "Layout".into(),
            focused_area: None,
        }
    }
}
impl Layout {
    pub fn focused_area(&self) -> Option<Entity> {
        self.focused_area
    }
}

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[require(AreaPresentation, AreaLocation)]
pub struct Area {
    pub rect: AreaRect,
    #[cfg_attr(feature = "persistence", entities)]
    pub selected: Option<Entity>,
}
impl Default for Area {
    fn default() -> Self {
        Self {
            rect: AreaRect {
                min: [0; 2],
                max: [LAYOUT_EXTENT; 2],
            },
            selected: None,
        }
    }
}
impl Area {
    pub fn rect(&self) -> AreaRect {
        self.rect
    }
    pub fn selected_panel(&self) -> Option<Entity> {
        self.selected
    }
}

#[derive(Component, Debug, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[require(PanelPresentation, PanelLocation)]
pub struct PanelInstance {
    /// Descriptive application identity, never used by workbench to create a panel.
    #[cfg_attr(feature = "persistence", reflect(ignore))]
    pub key: String,
    pub title: String,
    #[cfg_attr(feature = "persistence", reflect(ignore))]
    pub dirty: bool,
    pub preview: bool,
    /// Optional ordinary drawing capability, available as soon as composition applies.
    #[cfg(feature = "ui")]
    #[cfg_attr(feature = "persistence", reflect(ignore))]
    pub draw: Option<PanelDraw>,
}

#[cfg(feature = "ui")]
pub type PanelDraw = fn(&mut bedtime_ui::egui::Ui, &mut World, &bedtime_ui::UiContext<'_>, Entity);

/// Last applied parent, used only to distinguish attachment from reinsertion.
#[derive(Component, Default)]
pub(crate) struct PanelLocation(pub Option<Entity>);
#[derive(Component, Default)]
pub(crate) struct AreaLocation(pub Option<Entity>);

impl PanelInstance {
    /// Local presentation supplied by a concrete panel's required components.
    pub fn of<P: Panel>() -> Self {
        Self {
            key: P::KEY.into(),
            title: P::LABEL.into(),
            #[cfg(feature = "ui")]
            draw: Some(P::draw),
            ..default()
        }
    }

    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..default()
        }
    }
}

/// Find the selected layout from its authoritative marker.
pub fn active_layout(world: &World) -> Option<Entity> {
    let mut query = world.try_query_filtered::<Entity, With<ActiveLayout>>()?;
    let mut layouts = query.iter(world);
    let selected = layouts.next();
    assert!(layouts.next().is_none(), "Multiple ActiveLayout entities");
    selected
}

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component))]
#[component(on_insert = crate::membership::area_inserted, on_discard = crate::membership::area_replaced, on_remove = crate::membership::area_removed)]
#[relationship(relationship_target = LayoutAreas)]
pub struct InLayout(#[cfg_attr(feature = "persistence", entities)] pub Entity);

/// Ordered area membership. Persistence stores this order and rebuilds InLayout
/// sources through attachment hooks instead of persisting ownership twice.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[relationship_target(relationship = InLayout, linked_spawn)]
pub struct LayoutAreas(#[cfg_attr(feature = "persistence", entities)] Vec<Entity>);

impl LayoutAreas {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component))]
#[component(on_insert = crate::membership::panel_inserted, on_discard = crate::membership::panel_replaced, on_remove = crate::membership::panel_removed)]
#[relationship(relationship_target = AreaPanels)]
pub struct InArea(#[cfg_attr(feature = "persistence", entities)] pub Entity);

/// Authoritative tab order as well as reverse membership. Persistence stores this
/// list and rebuilds InArea sources in order; missing and empty lists are equivalent.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[relationship_target(relationship = InArea, linked_spawn)]
pub struct AreaPanels(#[cfg_attr(feature = "persistence", entities)] Vec<Entity>);

impl AreaPanels {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

/// Bevy removes a relationship target when its last source is detached. An
/// absent target is the ordinary representation of an empty area's tab list.
pub fn panels_in_area(world: &World, area: Entity) -> &[Entity] {
    world.get::<AreaPanels>(area).map_or(&[], |p| p.entities())
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct PanelPresentation {
    pub visible: bool,
    pub focused: bool,
    pub rect: Rect,
    pub tab_rect: Rect,
    pub close_rect: Rect,
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct AreaPresentation {
    pub visible: bool,
    pub rect: Rect,
    pub surface: Rect,
    pub tab_bar: Rect,
    pub content: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

impl SplitAxis {
    pub(crate) fn index(self) -> usize {
        usize::from(self == Self::Vertical)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AreaPlacement {
    Replace,
    Split {
        axis: SplitAxis,
        high: bool,
        fraction: f32,
    },
}

impl AreaPlacement {
    fn geometry(self) -> Result<geometry::DockPlacement, String> {
        Ok(match self {
            Self::Replace => geometry::DockPlacement::Replace,
            Self::Split {
                axis,
                high,
                fraction,
            } => {
                if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
                    return Err("Invalid placement fraction".into());
                }
                geometry::DockPlacement::Split {
                    axis: axis.index(),
                    high,
                    fraction: (fraction * LAYOUT_EXTENT as f32).round() as i32,
                }
            }
        })
    }
}

/// Captured on pointer admission. Leaving and returning to the same layout also
/// invalidates a gesture, even if its geometry did not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutStamp {
    pub(crate) layout: Entity,
    pub(crate) revision: u64,
    pub(crate) activation: u64,
}

#[derive(Debug, Clone)]
pub enum LayoutEdit {
    FocusArea(Entity),
    FocusPanel(Entity),
    KeepPanel(Entity),
    ClosePanels(Vec<Entity>),
    CloseArea(Entity),
    MoveTab {
        panel: Entity,
        area: Entity,
        index: usize,
    },
    SplitTab {
        panel: Entity,
        area: Entity,
        axis: SplitAxis,
        high: bool,
        minimum: [i32; 2],
    },
    SplitArea {
        area: Entity,
        axis: SplitAxis,
        at: i32,
        minimum: [i32; 2],
    },
    JoinAreas {
        source: Entity,
        target: Entity,
        minimum: [i32; 2],
    },
    DockArea {
        source: Entity,
        target: Entity,
        placement: AreaPlacement,
        minimum: [i32; 2],
    },
    ResizeBorder {
        axis: SplitAxis,
        at: i32,
        span: [i32; 2],
        to: i32,
        minimum: i32,
    },
    DropPanel {
        area: Entity,
        factory: PanelFactory,
        index: usize,
        split: Option<(SplitAxis, bool)>,
        minimum: [i32; 2],
    },
}

#[derive(Debug, Clone)]
pub enum WorkbenchRequest {
    ActivateLayout(Entity),
    FocusPanel(Entity),
    ClosePanel(Entity),
    Edit {
        stamp: LayoutStamp,
        edit: LayoutEdit,
    },
    #[cfg(feature = "ui")]
    UserEdit {
        stamp: LayoutStamp,
        edit: LayoutEdit,
    },
}

#[derive(Resource, Default)]
pub struct WorkbenchRequests(VecDeque<WorkbenchRequest>);
impl WorkbenchRequests {
    pub fn submit(&mut self, request: WorkbenchRequest) {
        self.0.push_back(request);
    }
}

/// A user intention, not permission to destroy entities. Applications may retain
/// it while prompting/saving, reject it, or call `Workbench::commit_close`.
/// The captured stamp prevents delayed approval from targeting a changed layout.
#[derive(Message, Clone, Debug)]
pub struct PanelCloseRequested {
    pub stamp: LayoutStamp,
    pub edit: LayoutEdit,
}

#[derive(Message, Debug)]
pub struct WorkbenchFinished {
    pub request: WorkbenchRequest,
    pub result: Result<(), String>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkbenchSystems {
    Apply,
}

/// Presentation contract for a concrete panel component. No catalog or initializer
/// is installed: require `PanelInstance::of::<Self>` on the concrete component.
pub trait Panel: Component {
    /// Optional descriptive identity for application inspection/persistence policy.
    /// Workbench never resolves it to a type or constructs a panel from it.
    const KEY: &'static str = "";
    const LABEL: &'static str;
    #[cfg(feature = "ui")]
    fn draw(
        ui: &mut bedtime_ui::egui::Ui,
        world: &mut World,
        context: &bedtime_ui::UiContext<'_>,
        panel: Entity,
    );
}

/// An application-owned, infallible panel constructor for a creation drag.
/// Validate application-specific inputs before offering the factory. It must
/// create exactly one panel in the supplied area, and return that entity.
#[derive(Clone)]
pub struct PanelFactory {
    pub label: &'static str,
    pub create: std::sync::Arc<dyn Fn(&mut World, Entity) -> Entity + Send + Sync>,
}
impl std::fmt::Debug for PanelFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelFactory")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}
impl PanelFactory {
    pub fn new(
        label: &'static str,
        create: impl Fn(&mut World, Entity) -> Entity + Send + Sync + 'static,
    ) -> Self {
        Self {
            label,
            create: std::sync::Arc::new(create),
        }
    }
    pub fn of<P: Panel + Component + Default>() -> Self {
        Self::new(P::LABEL, |world, area| {
            world.spawn((P::default(), InArea(area))).id()
        })
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_message::<WorkbenchFinished>()
        .add_message::<PanelCloseRequested>()
        .configure_sets(
            PreUpdate,
            WorkbenchSystems::Apply.after(crate::ContextCommandSystems::Dispatch),
        )
        .add_systems(PreUpdate, apply.in_set(WorkbenchSystems::Apply));
}

pub(crate) fn new_layout(world: &mut World, name: String) -> Entity {
    let layout = world.spawn(Layout::default()).id();
    let area = world
        .spawn((
            Area {
                rect: AreaRect {
                    min: [0, 0],
                    max: [LAYOUT_EXTENT; 2],
                },
                selected: None,
            },
            InLayout(layout),
        ))
        .id();
    world.entity_mut(layout).insert(Layout {
        name,
        focused_area: Some(area),
    });
    layout
}

/// Uncommitted viewer input must be committed or explicitly cancelled before its
/// panel is retired. Check a whole batch before making structural changes.
pub(crate) fn check_panel_drafts(
    world: &World,
    panels: impl IntoIterator<Item = Entity>,
) -> Result<(), String> {
    if panels
        .into_iter()
        .any(|panel| world.get::<crate::DocumentDraft>(panel).is_some())
    {
        return Err("Commit or cancel document drafts before closing their viewers".into());
    }
    Ok(())
}

/// A short-lived, ordered geometry calculation; IDs are local to this snapshot.
#[derive(Clone)]
pub(crate) struct GeometrySnapshot {
    pub geometry: geometry::Layout,
    pub entities: Vec<Entity>,
}

impl GeometrySnapshot {
    pub fn id(&self, entity: Entity) -> Result<u32, String> {
        self.entities
            .iter()
            .position(|&e| e == entity)
            .map(|i| i as u32 + 1)
            .ok_or("Area is unavailable".into())
    }
    pub fn entity(&self, id: u32) -> Option<Entity> {
        self.entities.get(id as usize - 1).copied()
    }
}

/// Preview and commit use the same final tiling, including the gap left by a
/// last-tab move. IDs belong to the supplied snapshot, not persistent entities.
pub(crate) fn plan_tab_geometry(
    geometry: &geometry::Layout,
    source: u32,
    target: u32,
    last_tab: bool,
    split: Option<(SplitAxis, bool)>,
    minimum: [i32; 2],
) -> Result<(geometry::Layout, u32), String> {
    let mut next = geometry.clone();
    let destination = if let Some((axis, high)) = split {
        let rect = next.area(target).rect;
        let a = axis.index();
        let id = next
            .split(target, a, (rect.min[a] + rect.max[a]) / 2, minimum[a])
            .ok_or("Area is too small to split")?;
        if !high {
            let low = next.area(target).rect;
            let upper = next.area(id).rect;
            next.areas.iter_mut().find(|v| v.id == target).unwrap().rect = upper;
            next.areas.iter_mut().find(|v| v.id == id).unwrap().rect = low;
        }
        id
    } else {
        target
    };
    if source != target && last_tab {
        // Splitting before closing preserves v1's gap-filling choice at
        // T-junctions. A same-area split deliberately leaves an empty half.
        assert!(
            next.close_area(source),
            "Cross-area move can close its source"
        );
    }
    Ok((next, destination))
}

impl Workbench<'_> {
    pub fn active_layout(&self) -> Entity {
        active_layout(self.world).expect("No active layout")
    }

    pub fn stamp(&self) -> LayoutStamp {
        let layout = self.active_layout();
        LayoutStamp {
            layout,
            revision: self.world.get::<LayoutRevision>(layout).unwrap().0,
            activation: self
                .world
                .resource::<crate::lifecycle::LayoutSelectionRevision>()
                .0,
        }
    }

    pub fn create_layout(&mut self, name: impl Into<String>) -> Result<Entity, String> {
        Ok(new_layout(self.world, name.into()))
    }

    pub fn activate_layout(&mut self, layout: Entity) -> Result<(), String> {
        self.require_owned(layout)?;
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        if active_layout(self.world) != Some(layout) {
            if let Some(previous) = active_layout(self.world) {
                self.world.entity_mut(previous).remove::<ActiveLayout>();
            }
            self.world.entity_mut(layout).insert(ActiveLayout);
        }
        Ok(())
    }

    pub fn panel_layout(&self, panel: Entity) -> Result<Entity, String> {
        self.require_owned(panel)?;
        self.world
            .get::<PanelInstance>(panel)
            .ok_or("Panel is unavailable")?;
        Ok(self
            .world
            .get::<InLayout>(self.world.get::<InArea>(panel).expect("Panel area").0)
            .expect("Area layout")
            .0)
    }

    pub fn panels(&self, layout: Entity) -> Vec<Entity> {
        self.world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .iter()
            .flat_map(|&area| panels_in_area(self.world, area).iter().copied())
            .collect()
    }

    pub fn focused_panel(&self, layout: Entity) -> Option<Entity> {
        self.world
            .get::<Area>(self.world.get::<Layout>(layout)?.focused_area?)?
            .selected
    }

    pub fn destination_area(&self, layout: Entity) -> Result<Entity, String> {
        self.require_owned(layout)?;
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        Ok(*self
            .world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .iter()
            .max_by_key(|&&area| {
                let r = self.world.get::<Area>(area).unwrap().rect;
                (r.max[0] - r.min[0]) * (r.max[1] - r.min[1])
            })
            .ok_or("Layout has no areas")?)
    }

    pub fn create_panel(
        &mut self,
        layout: Entity,
        title: impl Into<String>,
    ) -> Result<Entity, String> {
        let area = self.destination_area(layout)?;
        self.create_panel_in_area(area, title)
    }

    pub fn create_panel_in_area(
        &mut self,
        area: Entity,
        title: impl Into<String>,
    ) -> Result<Entity, String> {
        self.require_owned(area)?;
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        let panel = self.world.spawn_empty().id();
        self.attach_panel(panel, area, title.into());
        Ok(panel)
    }

    pub(crate) fn attach_panel(&mut self, panel: Entity, area: Entity, title: String) {
        self.world.entity_mut(panel).insert((
            PanelInstance {
                key: String::new(),
                title,
                dirty: false,
                preview: false,
                ..default()
            },
            InArea(area),
        ));
        self.world
            .get_mut::<Area>(area)
            .expect("Validated area")
            .selected = Some(panel);
        let layout = self.world.get::<InLayout>(area).unwrap().0;
        crate::membership::topology_changed(self.world, layout);
    }

    pub fn focus_area(&mut self, area: Entity) -> Result<(), String> {
        self.require_owned(area)?;
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        let layout = self.world.get::<InLayout>(area).unwrap().0;
        self.world.get_mut::<Layout>(layout).unwrap().focused_area = Some(area);
        Ok(())
    }

    /// Select a tab without changing the layout's focused area.
    pub fn select_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        let area = self.world.get::<InArea>(panel).unwrap().0;
        self.world.get_mut::<Area>(area).unwrap().selected = Some(panel);
        Ok(())
    }

    /// Explicit convenience for selecting a tab and focusing its area.
    pub fn focus_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.select_panel(panel)?;
        self.focus_area(self.world.get::<InArea>(panel).unwrap().0)
    }

    pub fn close_panel(&mut self, panel: Entity) -> Result<(), String> {
        let layout = self.panel_layout(panel)?;
        check_panel_drafts(self.world, [panel])?;
        let area = self.world.get::<InArea>(panel).unwrap().0;
        let index = panels_in_area(self.world, area)
            .iter()
            .position(|&p| p == panel)
            .unwrap();
        self.world.despawn(panel);
        let tabs = panels_in_area(self.world, area);
        let next = tabs.get(index.min(tabs.len().saturating_sub(1))).copied();
        if self.world.get::<Area>(area).unwrap().selected == Some(panel) {
            self.world.get_mut::<Area>(area).unwrap().selected = next;
        }
        crate::membership::topology_changed(self.world, layout);
        Ok(())
    }

    pub(crate) fn snapshot(&self, layout: Entity) -> Result<GeometrySnapshot, String> {
        self.require_owned(layout)?;
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        let entities = self
            .world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .to_vec();
        if entities.is_empty() {
            return Err("Layout has no areas".into());
        }
        let geometry = geometry::Layout::from_live(
            entities
                .iter()
                .enumerate()
                .map(|(i, &e)| geometry::Area {
                    id: i as u32 + 1,
                    rect: self.world.get::<Area>(e).expect("Owned area").rect,
                })
                .collect(),
        );
        Ok(GeometrySnapshot { geometry, entities })
    }

    fn commit_geometry(
        &mut self,
        layout: Entity,
        snapshot: &GeometrySnapshot,
        geometry: geometry::Layout,
    ) -> HashMap<u32, Entity> {
        let mut mapping = HashMap::new();
        for a in &geometry.areas {
            let entity = if let Some(entity) = snapshot.entity(a.id) {
                self.world.get_mut::<Area>(entity).unwrap().rect = a.rect;
                entity
            } else {
                self.world
                    .spawn((
                        Area {
                            rect: a.rect,
                            selected: None,
                        },
                        InLayout(layout),
                    ))
                    .id()
            };
            mapping.insert(a.id, entity);
        }
        for (i, &area) in snapshot.entities.iter().enumerate() {
            if !mapping.contains_key(&(i as u32 + 1)) {
                self.world.despawn(area);
            }
        }
        let focused = self.world.get::<Layout>(layout).unwrap().focused_area;
        if !mapping.values().any(|&e| Some(e) == focused) {
            let fallback = self.destination_area(layout).unwrap();
            self.world.get_mut::<Layout>(layout).unwrap().focused_area = Some(fallback);
        }
        crate::membership::topology_changed(self.world, layout);
        mapping
    }

    pub fn split_area(
        &mut self,
        area: Entity,
        axis: SplitAxis,
        at: i32,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        valid_minimum(minimum)?;
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0;
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        let id = next
            .split(snapshot.id(area)?, axis.index(), at, minimum[axis.index()])
            .ok_or("Area is too small to split")?;
        Ok(self.commit_geometry(layout, &snapshot, next)[&id])
    }

    pub fn close_area(&mut self, area: Entity) -> Result<(), String> {
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0;
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        if !next.close_area(snapshot.id(area)?) {
            return Err("Cannot close the final area".into());
        }
        check_panel_drafts(self.world, panels_in_area(self.world, area).iter().copied())?;
        self.commit_geometry(layout, &snapshot, next);
        Ok(())
    }

    /// Move between layouts while retaining canonical document links and work.
    /// The old area remains, and relationship hooks maintain tab/focus fallbacks.
    pub fn relocate_panel(&mut self, panel: Entity, area: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        self.world
            .get::<Area>(area)
            .ok_or("Destination area is unavailable")?;
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Destination area has no layout")?
            .0;
        self.world
            .get::<Layout>(layout)
            .ok_or("Destination layout is unavailable")?;
        self.world.entity_mut(panel).insert(InArea(area));
        Ok(())
    }

    pub fn move_tab(&mut self, panel: Entity, area: Entity, index: usize) -> Result<(), String> {
        self.place_tab(panel, area, index, None, [1, 1]).map(|_| ())
    }

    pub fn split_tab(
        &mut self,
        panel: Entity,
        area: Entity,
        axis: SplitAxis,
        high: bool,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        self.place_tab(panel, area, 0, Some((axis, high)), minimum)
    }

    fn place_tab(
        &mut self,
        panel: Entity,
        area: Entity,
        index: usize,
        split: Option<(SplitAxis, bool)>,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        valid_minimum(minimum)?;
        let layout = self.panel_layout(panel)?;
        if self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0
            != layout
        {
            return Err("Panels cannot move between layouts".into());
        }
        let source = self.world.get::<InArea>(panel).unwrap().0;
        if source == area && split.is_none() {
            // Reordering is not attachment: preserve the open tab and area focus.
            let mut tabs = panels_in_area(self.world, area).to_vec();
            tabs.retain(|&tab| tab != panel);
            tabs.insert(index.min(tabs.len()), panel);
            self.world.entity_mut(area).replace_related::<InArea>(&tabs);
            self.world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
            crate::membership::topology_changed(self.world, layout);
            return Ok(area);
        }
        let snapshot = self.snapshot(layout)?;
        let (next, id) = plan_tab_geometry(
            &snapshot.geometry,
            snapshot.id(source)?,
            snapshot.id(area)?,
            panels_in_area(self.world, source).len() == 1,
            split,
            minimum,
        )?;
        // All fallible planning precedes mutation. Detach the moving instance
        // before retiring its source so linked despawning cannot destroy it.
        // This synchronous transition restores ownership before returning.
        self.world.entity_mut(panel).remove::<InArea>();
        if self.world.get::<Area>(source).unwrap().selected == Some(panel) {
            let first = panels_in_area(self.world, source).first().copied();
            self.world.get_mut::<Area>(source).unwrap().selected = first;
        }
        let mapping = self.commit_geometry(layout, &snapshot, next);
        let destination = mapping[&id];
        let mut tabs = panels_in_area(self.world, destination).to_vec();
        tabs.insert(index.min(tabs.len()), panel);
        self.world.entity_mut(panel).insert(InArea(destination));
        self.world
            .entity_mut(destination)
            .replace_related::<InArea>(&tabs);
        self.world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
        self.world.get_mut::<Area>(destination).unwrap().selected = Some(panel);
        crate::membership::topology_changed(self.world, layout);
        Ok(destination)
    }

    pub fn move_panel(
        &mut self,
        panel: Entity,
        beside: Entity,
        split: Option<SplitAxis>,
    ) -> Result<(), String> {
        if panel == beside {
            return Err("A panel cannot move relative to itself".into());
        }
        self.panel_layout(beside)?;
        let area = self.world.get::<InArea>(beside).unwrap().0;
        if let Some(axis) = split {
            self.split_tab(panel, area, axis, true, [1, 1])?;
        } else {
            let index = panels_in_area(self.world, area).len();
            self.move_tab(panel, area, index)?;
        }
        Ok(())
    }

    pub fn join_areas(
        &mut self,
        source: Entity,
        target: Entity,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        self.relocate_area(source, target, None, minimum)
    }

    pub fn dock_area(
        &mut self,
        source: Entity,
        target: Entity,
        placement: AreaPlacement,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        self.relocate_area(source, target, Some(placement), minimum)
    }

    fn plan_area_relocation(
        &self,
        source: Entity,
        target: Entity,
        placement: Option<AreaPlacement>,
        minimum: [i32; 2],
    ) -> Result<(Entity, GeometrySnapshot, geometry::DockPlan), String> {
        valid_minimum(minimum)?;
        let layout = self
            .world
            .get::<InLayout>(source)
            .ok_or("Source area is unavailable")?
            .0;
        if self
            .world
            .get::<InLayout>(target)
            .ok_or("Target area is unavailable")?
            .0
            != layout
        {
            return Err("Areas belong to different layouts".into());
        }
        let snapshot = self.snapshot(layout)?;
        let a = snapshot.id(source)?;
        let b = snapshot.id(target)?;
        let plan = if let Some(placement) = placement {
            snapshot
                .geometry
                .plan_dock(a, b, placement.geometry()?, minimum)
        } else {
            snapshot.geometry.plan_join(a, b, minimum)
        }
        .ok_or("Area placement cannot satisfy the tiling/minimum sizes")?;
        Ok((layout, snapshot, plan))
    }

    fn relocate_area(
        &mut self,
        source: Entity,
        target: Entity,
        placement: Option<AreaPlacement>,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        let (layout, snapshot, plan) =
            self.plan_area_relocation(source, target, placement, minimum)?;
        // A join/dock can replace target areas and their panels. Validate every
        // removed area before changing geometry or focus.
        check_panel_drafts(
            self.world,
            snapshot
                .entities
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    !plan
                        .layout
                        .areas
                        .iter()
                        .any(|area| area.id == *index as u32 + 1)
                })
                .flat_map(|(_, &area)| panels_in_area(self.world, area).iter().copied()),
        )?;
        self.commit_geometry(layout, &snapshot, plan.layout);
        self.focus_area(source)
    }

    pub fn resize_border(
        &mut self,
        layout: Entity,
        axis: SplitAxis,
        at: i32,
        span: [i32; 2],
        to: i32,
        minimum: i32,
    ) -> Result<(), String> {
        if !(1..LAYOUT_EXTENT).contains(&at)
            || minimum <= 0
            || minimum > LAYOUT_EXTENT
            || span[0] < 0
            || span[0] >= span[1]
            || span[1] > LAYOUT_EXTENT
        {
            return Err("Invalid border coordinates".into());
        }
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        next.move_border(axis.index(), at, span, to, minimum);
        self.commit_geometry(layout, &snapshot, next);
        Ok(())
    }

    pub fn delete_layout(&mut self, layout: Entity) -> Result<(), String> {
        self.require_owned(layout)?;
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        check_panel_drafts(self.world, self.panels(layout))?;
        let replacement = self
            .world
            .query::<(Entity, &Layout)>()
            .iter(self.world)
            .find_map(|(e, _)| (e != layout).then_some(e));
        if active_layout(self.world) == Some(layout) {
            if let Some(next) = replacement {
                self.world.entity_mut(next).insert(ActiveLayout);
            }
        }
        self.world.despawn(layout);
        Ok(())
    }

    pub(crate) fn validate_edit(
        &self,
        stamp: LayoutStamp,
        edit: &LayoutEdit,
    ) -> Result<(), String> {
        if active_layout(self.world).is_none() || self.stamp() != stamp {
            return Err("Layout changed during interaction".into());
        }
        // Entity-bearing edits must belong to the admitted layout, even when a
        // caller supplies a current stamp with a foreign target.
        let area_of =
            |world: &World, entity: Entity| world.get::<InArea>(entity).map_or(entity, |p| p.0);
        let targets: Vec<Entity> = match edit {
            LayoutEdit::FocusArea(e)
            | LayoutEdit::FocusPanel(e)
            | LayoutEdit::KeepPanel(e)
            | LayoutEdit::CloseArea(e) => vec![*e],
            LayoutEdit::ClosePanels(es) => es.clone(),
            LayoutEdit::MoveTab { panel, area, .. } | LayoutEdit::SplitTab { panel, area, .. } => {
                vec![*panel, *area]
            }
            LayoutEdit::SplitArea { area, .. } | LayoutEdit::DropPanel { area, .. } => vec![*area],
            LayoutEdit::JoinAreas { source, target, .. }
            | LayoutEdit::DockArea { source, target, .. } => vec![*source, *target],
            LayoutEdit::ResizeBorder { .. } => Vec::new(),
        };
        for e in targets {
            if self
                .world
                .get::<InLayout>(area_of(self.world, e))
                .map(|r| r.0)
                != Some(stamp.layout)
            {
                return Err("Interaction target is no longer in this layout".into());
            }
        }
        Ok(())
    }

    /// Submit a close intention for application policy. This never destroys data.
    pub fn request_panel_close(
        &mut self,
        stamp: LayoutStamp,
        edit: LayoutEdit,
    ) -> Result<(), String> {
        self.validate_edit(stamp, &edit)?;
        if !matches!(
            edit,
            LayoutEdit::ClosePanels(_)
                | LayoutEdit::CloseArea(_)
                | LayoutEdit::JoinAreas { .. }
                | LayoutEdit::DockArea { .. }
        ) {
            return Err("Not a destructive layout edit".into());
        }
        self.world
            .write_message(PanelCloseRequested { stamp, edit });
        Ok(())
    }

    /// Read-only preflight for application close policy, including the exact
    /// panels removed by joins/docks. Uses the same geometry planner as commit.
    pub fn close_targets(&self, request: &PanelCloseRequested) -> Result<Vec<Entity>, String> {
        self.validate_edit(request.stamp, &request.edit)?;
        match &request.edit {
            LayoutEdit::ClosePanels(panels) => Ok(panels.clone()),
            LayoutEdit::CloseArea(area) => Ok(panels_in_area(self.world, *area).to_vec()),
            LayoutEdit::JoinAreas {
                source,
                target,
                minimum,
            }
            | LayoutEdit::DockArea {
                source,
                target,
                minimum,
                ..
            } => {
                let placement = if let LayoutEdit::DockArea { placement, .. } = &request.edit {
                    Some(*placement)
                } else {
                    None
                };
                let (_, snapshot, plan) =
                    self.plan_area_relocation(*source, *target, placement, *minimum)?;
                Ok(snapshot
                    .entities
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        !plan
                            .layout
                            .areas
                            .iter()
                            .any(|area| area.id == *index as u32 + 1)
                    })
                    .flat_map(|(_, &area)| panels_in_area(self.world, area).iter().copied())
                    .collect())
            }
            _ => Err("Not a destructive layout edit".into()),
        }
    }

    /// Commit an application-approved intention, revalidating its captured target.
    pub fn commit_close(&mut self, request: &PanelCloseRequested) -> Result<(), String> {
        if !matches!(
            request.edit,
            LayoutEdit::ClosePanels(_)
                | LayoutEdit::CloseArea(_)
                | LayoutEdit::JoinAreas { .. }
                | LayoutEdit::DockArea { .. }
        ) {
            return Err("Not a destructive layout edit".into());
        }
        self.edit(request.stamp, &request.edit)
    }

    pub(crate) fn edit(&mut self, stamp: LayoutStamp, edit: &LayoutEdit) -> Result<(), String> {
        self.validate_edit(stamp, edit)?;
        match edit {
            LayoutEdit::FocusArea(e) => self.focus_area(*e),
            LayoutEdit::FocusPanel(e) => self.focus_panel(*e),
            LayoutEdit::KeepPanel(e) => {
                self.world
                    .get_mut::<PanelInstance>(*e)
                    .ok_or("Panel is unavailable")?
                    .preview = false;
                Ok(())
            }
            LayoutEdit::ClosePanels(es) => {
                if es
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != es.len()
                {
                    return Err("Close request contains duplicate panels".into());
                }
                for &e in es {
                    self.panel_layout(e)?;
                }
                check_panel_drafts(self.world, es.iter().copied())?;
                for &e in es {
                    self.close_panel(e)?;
                }
                Ok(())
            }
            LayoutEdit::CloseArea(e) => self.close_area(*e),
            LayoutEdit::MoveTab { panel, area, index } => self.move_tab(*panel, *area, *index),
            LayoutEdit::SplitTab {
                panel,
                area,
                axis,
                high,
                minimum,
            } => self
                .split_tab(*panel, *area, *axis, *high, *minimum)
                .map(|_| ()),
            LayoutEdit::SplitArea {
                area,
                axis,
                at,
                minimum,
            } => self.split_area(*area, *axis, *at, *minimum).map(|_| ()),
            LayoutEdit::JoinAreas {
                source,
                target,
                minimum,
            } => self.join_areas(*source, *target, *minimum),
            LayoutEdit::DockArea {
                source,
                target,
                placement,
                minimum,
            } => self.dock_area(*source, *target, *placement, *minimum),
            LayoutEdit::ResizeBorder {
                axis,
                at,
                span,
                to,
                minimum,
            } => self.resize_border(stamp.layout, *axis, *at, *span, *to, *minimum),
            LayoutEdit::DropPanel {
                area,
                factory,
                index,
                split,
                minimum,
            } => {
                valid_minimum(*minimum)?;
                if let Some((axis, _)) = split {
                    let rect = self.world.get::<Area>(*area).unwrap().rect;
                    if rect.max[axis.index()] - rect.min[axis.index()] < minimum[axis.index()] * 2 {
                        return Err("Area is too small to split".into());
                    }
                }
                // Geometry is validated before invoking the infallible app factory.
                let panel = (factory.create)(self.world, *area);
                assert!(
                    self.world.get::<PanelInstance>(panel).is_some()
                        && self.world.get::<InArea>(panel).map(|owner| owner.0) == Some(*area),
                    "PanelFactory must create a panel in its supplied destination"
                );
                if let Some((axis, high)) = split {
                    self.split_tab(panel, *area, *axis, *high, *minimum)
                        .map(|_| ())
                } else {
                    self.move_tab(panel, *area, *index)
                }
            }
        }
    }
}

fn valid_minimum(minimum: [i32; 2]) -> Result<(), String> {
    if minimum.iter().any(|&v| v <= 0 || v > LAYOUT_EXTENT) {
        Err("Invalid area minimum".into())
    } else {
        Ok(())
    }
}

fn apply(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<WorkbenchRequests>().0);
    for request in requests {
        let mut w = Workbench::new(world);
        let result = match &request {
            WorkbenchRequest::ActivateLayout(e) => w.activate_layout(*e),
            WorkbenchRequest::FocusPanel(e) => w.focus_panel(*e),
            WorkbenchRequest::ClosePanel(e) => w.close_panel(*e),
            WorkbenchRequest::Edit { stamp, edit } => w.edit(*stamp, edit),
            #[cfg(feature = "ui")]
            WorkbenchRequest::UserEdit { stamp, edit } => w.user_edit(*stamp, edit),
        };
        world.write_message(WorkbenchFinished { request, result });
    }
}

/// Admit current UI focus before keyboard routing; structural edits stay queued.
#[cfg(feature = "ui")]
pub(crate) fn apply_focus(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<WorkbenchRequests>().0);
    let mut structural_pending = false;
    for request in requests {
        if structural_pending {
            world
                .resource_mut::<WorkbenchRequests>()
                .0
                .push_back(request);
            continue;
        }
        let mut w = Workbench::new(world);
        let result = match &request {
            WorkbenchRequest::FocusPanel(panel) => Some(w.focus_panel(*panel)),
            WorkbenchRequest::Edit {
                stamp,
                edit: edit @ (LayoutEdit::FocusArea(_) | LayoutEdit::FocusPanel(_)),
            } => Some(w.edit(*stamp, edit)),
            WorkbenchRequest::UserEdit {
                stamp,
                edit: edit @ (LayoutEdit::FocusArea(_) | LayoutEdit::FocusPanel(_)),
            } => Some(w.user_edit(*stamp, edit)),
            _ => None,
        };
        if let Some(result) = result {
            world.write_message(WorkbenchFinished { request, result });
        } else {
            structural_pending = true;
            world
                .resource_mut::<WorkbenchRequests>()
                .0
                .push_back(request);
        }
    }
}
