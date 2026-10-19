//! Entity-free storage. Applications own serialization formats, I/O and templates.
use crate::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const WORKSPACE_FORMAT_VERSION: u32 = 1;
pub const LAYOUT_FORMAT_VERSION: u32 = 1;

/// A workspace's current layouts, independent of application-owned layout templates.
/// Indices refer only to the adjacent vector, never to live Bevy entities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub version: u32,
    pub name: String,
    pub background_updates: bool,
    pub layouts: Vec<LayoutSnapshot>,
    pub active_layout: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    pub version: u32,
    pub name: String,
    pub areas: Vec<AreaSnapshot>,
    pub focused_area: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaSnapshot {
    pub rect: AreaRect,
    pub panels: Vec<PanelSnapshot>,
    pub selected_panel: Option<usize>,
}
/// Only workbench metadata is stored here. Application components are initialized
/// by the registered Panel type; project/document state is application-owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSnapshot {
    pub key: String,
    pub title: String,
    pub preview: bool,
}

/// Returned in snapshot order so applications can attach project or slot metadata.
#[derive(Debug)]
pub struct RestoredWorkspace {
    pub workspace: Entity,
    pub layouts: Vec<RestoredLayout>,
}
#[derive(Debug)]
pub struct RestoredLayout {
    pub layout: Entity,
    pub areas: Vec<RestoredArea>,
}
#[derive(Debug)]
pub struct RestoredArea {
    pub area: Entity,
    pub panels: Vec<Entity>,
}

pub fn capture_layout(world: &World, layout: Entity) -> Result<LayoutSnapshot, String> {
    let snapshot = layout_snapshot(world, layout)?;
    validate_layout(world, &snapshot)?;
    Ok(snapshot)
}
pub fn capture_workspace(world: &World, workspace: Entity) -> Result<WorkspaceSnapshot, String> {
    let snapshot = workspace_snapshot(world, workspace)?;
    validate_workspace(world, &snapshot)?;
    Ok(snapshot)
}
fn layout_snapshot(world: &World, layout: Entity) -> Result<LayoutSnapshot, String> {
    let data = world.get::<Layout>(layout).ok_or("Layout is unavailable")?;
    let areas = world
        .get::<LayoutAreas>(layout)
        .map_or(&[][..], |a| a.entities());
    let focused_area = local_index(
        areas,
        data.focused_area,
        "Focused area is outside this layout",
    )?;
    let areas = areas
        .iter()
        .map(|&area| {
            let data = world.get::<Area>(area).ok_or("InLayout requires Area")?;
            let tabs = panels_in_area(world, area);
            let selected_panel =
                local_index(tabs, data.selected, "Selected panel is outside this area")?;
            let panels = tabs
                .iter()
                .map(|&panel| {
                    let data = world
                        .get::<PanelInstance>(panel)
                        .ok_or("InArea requires PanelInstance")?;
                    Ok(PanelSnapshot {
                        key: data.key.clone(),
                        title: data.title.clone(),
                        preview: data.preview,
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(AreaSnapshot {
                rect: data.rect,
                panels,
                selected_panel,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(LayoutSnapshot {
        version: LAYOUT_FORMAT_VERSION,
        name: data.name.clone(),
        areas,
        focused_area,
    })
}
fn workspace_snapshot(world: &World, workspace: Entity) -> Result<WorkspaceSnapshot, String> {
    let data = world
        .get::<Workspace>(workspace)
        .ok_or("Workspace is unavailable")?;
    let layouts: Vec<_> = world
        .get::<WorkspaceMembers>(workspace)
        .map_or(&[][..], |m| m.entities())
        .iter()
        .copied()
        .filter(|&e| world.get::<Layout>(e).is_some())
        .collect();
    let active = active_layout(world, workspace);
    let active_layout = local_index(&layouts, active, "Active layout is outside this workspace")?;
    Ok(WorkspaceSnapshot {
        version: WORKSPACE_FORMAT_VERSION,
        name: data.name.clone(),
        background_updates: data.background_updates,
        active_layout,
        layouts: layouts
            .iter()
            .map(|&e| layout_snapshot(world, e))
            .collect::<Result<_, _>>()?,
    })
}
fn local_index(
    entities: &[Entity],
    entity: Option<Entity>,
    error: &str,
) -> Result<Option<usize>, String> {
    entity
        .map(|e| {
            entities
                .iter()
                .position(|&candidate| candidate == e)
                .ok_or_else(|| error.into())
        })
        .transpose()
}
fn valid_index(index: Option<usize>, len: usize, error: &str) -> Result<(), String> {
    if index.is_some_and(|i| i >= len) {
        return Err(error.into());
    }
    Ok(())
}
fn validate_workspace(world: &World, snapshot: &WorkspaceSnapshot) -> Result<(), String> {
    if snapshot.version != WORKSPACE_FORMAT_VERSION {
        return Err("Unsupported workspace format version".into());
    }
    valid_index(
        snapshot.active_layout,
        snapshot.layouts.len(),
        "Invalid active layout index",
    )?;
    for layout in &snapshot.layouts {
        validate_layout(world, layout)?;
    }
    Ok(())
}
fn validate_layout(world: &World, snapshot: &LayoutSnapshot) -> Result<(), String> {
    if snapshot.version != LAYOUT_FORMAT_VERSION {
        return Err("Unsupported layout format version".into());
    }
    valid_index(
        snapshot.focused_area,
        snapshot.areas.len(),
        "Invalid focused area index",
    )?;
    if !snapshot.areas.is_empty() && snapshot.focused_area.is_none() {
        return Err("Nonempty layout requires a focused area".into());
    }
    if !snapshot.areas.is_empty() {
        crate::geometry::Layout::from_areas(
            snapshot
                .areas
                .iter()
                .enumerate()
                .map(|(i, area)| crate::geometry::Area {
                    id: i as u32 + 1,
                    rect: area.rect,
                })
                .collect(),
        )?;
    }
    let kinds: HashSet<_> = world
        .iter_entities()
        .filter_map(|e| e.get::<PanelType>().map(|p| p.key))
        .collect();
    for area in &snapshot.areas {
        valid_index(
            area.selected_panel,
            area.panels.len(),
            "Invalid selected panel index",
        )?;
        if !area.panels.is_empty() && area.selected_panel.is_none() {
            return Err("Nonempty area requires a selected panel".into());
        }
        for panel in &area.panels {
            if panel.key.is_empty() {
                return Err(
                    "Anonymous panels cannot be saved; register a stable panel type".into(),
                );
            }
            if !kinds.contains(panel.key.as_str()) {
                return Err(format!("Unregistered panel type: {}", panel.key));
            }
        }
    }
    Ok(())
}

/// Validate the complete document before creating entities. Selection of the
/// workspace itself belongs to the application (insert ActiveWorkspace).
pub fn restore_workspace(
    world: &mut World,
    snapshot: &WorkspaceSnapshot,
) -> Result<RestoredWorkspace, String> {
    validate_workspace(world, snapshot)?;
    let workspace = world
        .spawn(Workspace {
            name: snapshot.name.clone(),
            background_updates: snapshot.background_updates,
        })
        .id();
    let mut layouts = Vec::new();
    for layout in &snapshot.layouts {
        let entity = world
            .spawn((
                Layout {
                    name: layout.name.clone(),
                    ..default()
                },
                InWorkspace(workspace),
            ))
            .id();
        layouts.push(materialize_layout(world, workspace, entity, layout));
    }
    if let Some(index) = snapshot.active_layout {
        world.entity_mut(layouts[index].layout).insert(ActiveLayout);
    }
    Ok(RestoredWorkspace { workspace, layouts })
}
/// Instantiate a saved layout/template with fresh entities. Does not change the
/// workspace's active layout; the caller may insert ActiveLayout on the result.
pub fn restore_layout(
    world: &mut World,
    workspace: Entity,
    snapshot: &LayoutSnapshot,
) -> Result<RestoredLayout, String> {
    crate::workspaces::validate(world, WorkspaceId(workspace))?;
    validate_layout(world, snapshot)?;
    let layout = world
        .spawn((
            Layout {
                name: snapshot.name.clone(),
                ..default()
            },
            InWorkspace(workspace),
        ))
        .id();
    Ok(materialize_layout(world, workspace, layout, snapshot))
}
/// Reset a layout to a template, retaining its identity, active marker and any
/// application slot/project components. Only core layout data and descendants change.
pub fn replace_layout(
    world: &mut World,
    layout: Entity,
    snapshot: &LayoutSnapshot,
) -> Result<RestoredLayout, String> {
    if world.get::<Layout>(layout).is_none() {
        return Err("Layout is unavailable".into());
    }
    let workspace = world
        .get::<InWorkspace>(layout)
        .ok_or("Layout has no workspace")?
        .0;
    crate::workspaces::validate(world, WorkspaceId(workspace))?;
    validate_layout(world, snapshot)?;
    let areas = world
        .get::<LayoutAreas>(layout)
        .map_or(Vec::new(), |a| a.entities().to_vec());
    crate::layout::check_panel_drafts(
        world,
        areas
            .iter()
            .flat_map(|&area| panels_in_area(world, area).iter().copied()),
    )?;
    for area in areas {
        world.despawn(area);
    }
    let mut data = world.get_mut::<Layout>(layout).unwrap();
    data.name = snapshot.name.clone();
    data.focused_area = None;
    world
        .get_mut::<crate::layout::LayoutRevision>(layout)
        .unwrap()
        .0 += 1;
    Ok(materialize_layout(world, workspace, layout, snapshot))
}
fn materialize_layout(
    world: &mut World,
    workspace: Entity,
    layout: Entity,
    snapshot: &LayoutSnapshot,
) -> RestoredLayout {
    let kinds: HashMap<_, _> = world
        .query::<(Entity, &PanelType)>()
        .iter(world)
        .map(|(e, p)| (p.key.to_owned(), e))
        .collect();
    let mut areas = Vec::new();
    for saved in &snapshot.areas {
        let area = world
            .spawn((
                Area {
                    rect: saved.rect,
                    selected: None,
                },
                InLayout(layout),
                InWorkspace(workspace),
            ))
            .id();
        let mut panels = Vec::new();
        for saved_panel in &saved.panels {
            let panel = world
                .spawn((
                    PanelInstance {
                        key: saved_panel.key.clone(),
                        title: saved_panel.title.clone(),
                        preview: saved_panel.preview,
                        ..default()
                    },
                    InArea(area),
                    InWorkspace(workspace),
                ))
                .id();
            crate::layout::initialize_panel(world, panel, kinds[&saved_panel.key]);
            panels.push(panel);
        }
        world.get_mut::<Area>(area).unwrap().selected = saved.selected_panel.map(|i| panels[i]);
        areas.push(RestoredArea { area, panels });
    }
    world.get_mut::<Layout>(layout).unwrap().focused_area =
        snapshot.focused_area.map(|i| areas[i].area);
    RestoredLayout { layout, areas }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceChange {
    Created,
    Modified,
    Removed,
}
/// Coalesced once per workspace per frame, after Update and reconciliation.
/// Includes direct component edits and removals, not only workbench helper calls.
/// Applications can capture the workspace now and apply their own debounce/I/O policy.
#[derive(Message, Clone, Copy, Debug)]
pub struct WorkspaceChanged {
    pub workspace: Entity,
    pub change: WorkspaceChange,
}
#[derive(Resource, Default)]
struct PreviousSnapshots(HashMap<Entity, WorkspaceSnapshot>);
pub(crate) fn install(app: &mut App) {
    app.init_resource::<PreviousSnapshots>()
        .add_message::<WorkspaceChanged>()
        .add_systems(
            PostUpdate,
            notify_changes
                .in_set(WorkspaceSystems::Notify)
                .after(crate::workspaces::reconcile),
        );
}
fn notify_changes(world: &mut World) {
    let current: HashMap<_, _> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .map(|e| {
            (
                e,
                workspace_snapshot(world, e).expect("Reconciled workspace"),
            )
        })
        .collect();
    let previous = &world.resource::<PreviousSnapshots>().0;
    let mut events = Vec::new();
    for (&workspace, snapshot) in &current {
        let change = match previous.get(&workspace) {
            None => Some(WorkspaceChange::Created),
            Some(old) if old != snapshot => Some(WorkspaceChange::Modified),
            _ => None,
        };
        if let Some(change) = change {
            events.push(WorkspaceChanged { workspace, change });
        }
    }
    for &workspace in previous.keys() {
        if !current.contains_key(&workspace) {
            events.push(WorkspaceChanged {
                workspace,
                change: WorkspaceChange::Removed,
            });
        }
    }
    world.resource_mut::<PreviousSnapshots>().0 = current;
    for event in events {
        world.write_message(event);
    }
}

#[cfg(test)]
mod tests;
