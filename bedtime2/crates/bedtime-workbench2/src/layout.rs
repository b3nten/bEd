//! ECS owns the tiling. Geometry snapshots are temporary calculations, never a
//! second live model. This module alone commits membership and topology changes.
use crate::{Workbench, geometry};
use bevy::{ecs::system::SystemId, prelude::*};
pub use geometry::EXTENT as LAYOUT_EXTENT;
pub use geometry::Rect as AreaRect;
use std::collections::{HashMap, VecDeque};

#[derive(Resource, Debug)]
pub struct Workspace {
    pub name: String,
    active_layout: Entity,
    pub(crate) activation: u64,
}

impl Workspace {
    pub fn active_layout(&self) -> Entity {
        self.active_layout
    }
}

#[derive(Component, Debug)]
#[require(LayoutAreas)]
pub struct Layout {
    pub name: String,
    focused_area: Entity,
    revision: u64,
}

impl Layout {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn focused_area(&self) -> Entity {
        self.focused_area
    }
}

#[derive(Component, Debug)]
#[require(AreaPresentation)]
pub struct Area {
    rect: AreaRect,
    selected: Option<Entity>,
}

impl Area {
    pub fn rect(&self) -> AreaRect {
        self.rect
    }
    pub fn selected_panel(&self) -> Option<Entity> {
        self.selected
    }
}

#[derive(Component, Debug)]
#[require(PanelPresentation)]
pub struct Panel {
    pub title: String,
    pub dirty: bool,
    /// Preview is presentation metadata. Double-clicking/explicit movement keeps it.
    pub preview: bool,
    pub(crate) kind: Option<Entity>,
}

#[derive(Component, Debug)]
#[relationship(relationship_target = LayoutAreas)]
pub struct InLayout(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InLayout, linked_spawn)]
pub struct LayoutAreas(Vec<Entity>);

impl LayoutAreas {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Component, Debug)]
#[relationship(relationship_target = AreaPanels)]
pub struct InArea(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InArea, linked_spawn)]
pub struct AreaPanels(Vec<Entity>);

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
    OpenPanel {
        area: Entity,
        kind: Entity,
    },
    DropPanel {
        area: Entity,
        kind: Entity,
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
}

#[derive(Resource, Default)]
pub struct WorkbenchRequests(VecDeque<WorkbenchRequest>);
impl WorkbenchRequests {
    pub fn submit(&mut self, request: WorkbenchRequest) {
        self.0.push_back(request);
    }
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

/// Factories are ordinary registered systems. The picker projects these entities;
/// no second factory registry or application callback chain is involved.
pub trait PanelDefinition: Send + Sync + 'static {
    const KEY: &'static str;
    const LABEL: &'static str;
}

#[derive(Component)]
pub struct PanelType {
    pub key: &'static str,
    pub label: String,
}

type PanelFactory = SystemId<In<Entity>, Result<Entity, String>>;

pub(crate) fn install(app: &mut App) {
    app.init_resource::<WorkbenchRequests>()
        .add_message::<WorkbenchFinished>()
        .configure_sets(
            PreUpdate,
            WorkbenchSystems::Apply.after(crate::ContextCommandSystems::Dispatch),
        )
        .add_systems(PreUpdate, apply.in_set(WorkbenchSystems::Apply));
    let layout = new_layout(app.world_mut(), "Default".into());
    app.insert_resource(Workspace {
        name: "Untitled".into(),
        active_layout: layout,
        activation: 0,
    });
}

fn new_layout(world: &mut World, name: String) -> Entity {
    let layout = world.spawn_empty().id();
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
        focused_area: area,
        revision: 0,
    });
    layout
}

/// A short-lived, ordered geometry calculation; IDs are local to this snapshot.
#[derive(Clone)]
pub(crate) struct LayoutSnapshot {
    pub geometry: geometry::Layout,
    pub entities: Vec<Entity>,
}

impl LayoutSnapshot {
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
        self.world.resource::<Workspace>().active_layout
    }

    pub fn stamp(&self) -> LayoutStamp {
        let layout = self.active_layout();
        LayoutStamp {
            layout,
            revision: self.world.get::<Layout>(layout).unwrap().revision,
            activation: self.world.resource::<Workspace>().activation,
        }
    }

    pub fn create_layout(&mut self, name: impl Into<String>) -> Result<Entity, String> {
        Ok(new_layout(self.world, name.into()))
    }

    pub fn activate_layout(&mut self, layout: Entity) -> Result<(), String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        let mut workspace = self.world.resource_mut::<Workspace>();
        if workspace.active_layout != layout {
            workspace.active_layout = layout;
            workspace.activation += 1;
        }
        Ok(())
    }

    pub fn panel_layout(&self, panel: Entity) -> Result<Entity, String> {
        self.world
            .get::<Panel>(panel)
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
            .expect("Layout areas")
            .entities()
            .iter()
            .flat_map(|&area| panels_in_area(self.world, area).iter().copied())
            .collect()
    }

    pub fn focused_panel(&self, layout: Entity) -> Option<Entity> {
        self.world
            .get::<Area>(self.world.get::<Layout>(layout)?.focused_area)?
            .selected
    }

    pub fn destination_area(&self, layout: Entity) -> Result<Entity, String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        Ok(*self
            .world
            .get::<LayoutAreas>(layout)
            .unwrap()
            .entities()
            .iter()
            .max_by_key(|&&area| {
                let r = self.world.get::<Area>(area).unwrap().rect;
                (r.max[0] - r.min[0]) * (r.max[1] - r.min[1])
            })
            .expect("Layout has an area"))
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
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        let panel = self.world.spawn_empty().id();
        self.attach_panel(panel, area, title.into());
        Ok(panel)
    }

    pub(crate) fn attach_panel(&mut self, panel: Entity, area: Entity, title: String) {
        self.world.entity_mut(panel).insert((
            Panel {
                title,
                dirty: false,
                preview: false,
                kind: None,
            },
            InArea(area),
        ));
        self.world
            .get_mut::<Area>(area)
            .expect("Validated area")
            .selected = Some(panel);
        let layout = self.world.get::<InLayout>(area).unwrap().0;
        self.world.get_mut::<Layout>(layout).unwrap().revision += 1;
        self.focus_area(area).unwrap();
    }

    pub fn focus_area(&mut self, area: Entity) -> Result<(), String> {
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        let layout = self.world.get::<InLayout>(area).unwrap().0;
        self.world.get_mut::<Layout>(layout).unwrap().focused_area = area;
        Ok(())
    }

    pub fn focus_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        let area = self.world.get::<InArea>(panel).unwrap().0;
        self.world.get_mut::<Area>(area).unwrap().selected = Some(panel);
        self.focus_area(area)
    }

    pub fn close_panel(&mut self, panel: Entity) -> Result<(), String> {
        let layout = self.panel_layout(panel)?;
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
        self.world.get_mut::<Layout>(layout).unwrap().revision += 1;
        Ok(())
    }

    pub(crate) fn snapshot(&self, layout: Entity) -> Result<LayoutSnapshot, String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        let entities = self
            .world
            .get::<LayoutAreas>(layout)
            .unwrap()
            .entities()
            .to_vec();
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
        Ok(LayoutSnapshot { geometry, entities })
    }

    fn commit_geometry(
        &mut self,
        layout: Entity,
        snapshot: &LayoutSnapshot,
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
        if !mapping.values().any(|&e| e == focused) {
            let fallback = self.destination_area(layout).unwrap();
            self.world.get_mut::<Layout>(layout).unwrap().focused_area = fallback;
        }
        self.world.get_mut::<Layout>(layout).unwrap().revision += 1;
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
        self.commit_geometry(layout, &snapshot, next);
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
        self.world.get_mut::<Panel>(panel).unwrap().preview = false;
        self.world.get_mut::<Area>(destination).unwrap().selected = Some(panel);
        self.world.get_mut::<Layout>(layout).unwrap().focused_area = destination;
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

    fn relocate_area(
        &mut self,
        source: Entity,
        target: Entity,
        placement: Option<AreaPlacement>,
        minimum: [i32; 2],
    ) -> Result<(), String> {
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

    pub fn register_panel_type<P: PanelDefinition, M>(
        &mut self,
        factory: impl IntoSystem<In<Entity>, Result<Entity, String>, M> + 'static,
    ) -> Result<Entity, String> {
        if P::LABEL.trim().is_empty() || !crate::valid_key(P::KEY) {
            return Err("Panel type needs a namespaced key and label".into());
        }
        if self
            .world
            .query::<&PanelType>()
            .iter(self.world)
            .any(|p| p.key == P::KEY)
        {
            return Err("Panel type is already registered".into());
        }
        let system = self.world.register_system(factory);
        self.world.entity_mut(system.entity()).insert(PanelType {
            key: P::KEY,
            label: P::LABEL.into(),
        });
        Ok(system.entity())
    }

    pub fn open_panel(&mut self, kind: Entity, area: Entity) -> Result<Entity, String> {
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        self.world
            .get::<PanelType>(kind)
            .ok_or("Panel type is unavailable")?;
        if self
            .world
            .get::<crate::ActionEnabled>(kind)
            .is_some_and(|a| !a.0)
        {
            return Err("Panel type is unavailable".into());
        }
        let panel = self
            .world
            .run_system_with(PanelFactory::from_entity(kind), area)
            .map_err(|e| e.to_string())??;
        if self.world.get::<InArea>(panel).map(|r| r.0) != Some(area) {
            return Err("Panel factory must return a panel in its requested area".into());
        }
        self.world
            .get_mut::<Panel>(panel)
            .expect("Factory returned a panel")
            .kind = Some(kind);
        self.focus_panel(panel)?;
        Ok(panel)
    }

    pub fn delete_layout(&mut self, layout: Entity) -> Result<(), String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        let replacement = self
            .world
            .query_filtered::<Entity, With<Layout>>()
            .iter(self.world)
            .find(|&entity| entity != layout)
            .ok_or("Cannot delete the last layout")?;
        let mut workspace = self.world.resource_mut::<Workspace>();
        if workspace.active_layout == layout {
            workspace.active_layout = replacement;
            workspace.activation += 1;
        }
        self.world.despawn(layout);
        Ok(())
    }

    pub(crate) fn edit(&mut self, stamp: LayoutStamp, edit: &LayoutEdit) -> Result<(), String> {
        if self.stamp() != stamp {
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
            LayoutEdit::SplitArea { area, .. }
            | LayoutEdit::OpenPanel { area, .. }
            | LayoutEdit::DropPanel { area, .. } => vec![*area],
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
        match edit {
            LayoutEdit::FocusArea(e) => self.focus_area(*e),
            LayoutEdit::FocusPanel(e) => self.focus_panel(*e),
            LayoutEdit::KeepPanel(e) => {
                self.world
                    .get_mut::<Panel>(*e)
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
            LayoutEdit::OpenPanel { area, kind } => self.open_panel(*kind, *area).map(|_| ()),
            LayoutEdit::DropPanel {
                area,
                kind,
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
                let panel = self.open_panel(*kind, *area)?;
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
        };
        world.write_message(WorkbenchFinished { request, result });
    }
}

/// Focus observations from the completed UI pass precede keyboard routing.
/// Structural edits retain their single commit stage after command dispatch.
#[cfg(feature = "ui")]
pub(crate) fn apply_focus(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<WorkbenchRequests>().0);
    for request in requests {
        let result = match &request {
            WorkbenchRequest::FocusPanel(panel) => Some(Workbench::new(world).focus_panel(*panel)),
            WorkbenchRequest::Edit {
                stamp,
                edit: edit @ (LayoutEdit::FocusArea(_) | LayoutEdit::FocusPanel(_)),
            } => Some(Workbench::new(world).edit(*stamp, edit)),
            _ => None,
        };
        if let Some(result) = result {
            world.write_message(WorkbenchFinished { request, result });
        } else {
            world
                .resource_mut::<WorkbenchRequests>()
                .0
                .push_back(request);
        }
    }
}
