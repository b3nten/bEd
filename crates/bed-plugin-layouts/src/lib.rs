//! Retained layouts for Bevy workbenches. The application supplies templates,
//! serialized snapshots and persistence, and calls transitions between UI scopes.
mod slot;
use bed_api::workbench::PanelSuspended;
use bed_workbench::{PanelPresentation, PanelRecord, WorkbenchLayout};
use bevy::prelude::*;
use serde_json::{Map, Value, json};
pub use slot::LayoutSlot;
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    io,
    num::NonZeroU32,
};

/// An arbitrary positive numbered layout. Applications choose their own count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayoutId(NonZeroU32);

impl LayoutId {
    pub const fn new(number: u32) -> Option<Self> {
        match NonZeroU32::new(number) {
            Some(number) => Some(Self(number)),
            None => None,
        }
    }
    pub const fn number(self) -> u32 {
        self.0.get()
    }
}

impl Default for LayoutId {
    fn default() -> Self {
        Self(NonZeroU32::new(1).unwrap())
    }
}

/// Stable saved keys and their JSON identity are the only requirements on a
/// layout name. Custom applications can use their own enum instead of numbers.
pub trait LayoutKey: Clone + Eq + Hash + Send + Sync + 'static {
    fn saved_key(&self) -> String;
    fn saved_value(&self) -> Value;
}

impl LayoutKey for LayoutId {
    fn saved_key(&self) -> String {
        self.number().to_string()
    }
    fn saved_value(&self) -> Value {
        json!(self.number())
    }
}

impl LayoutKey for LayoutSlot {
    fn saved_key(&self) -> String {
        self.key().to_owned()
    }
    fn saved_value(&self) -> Value {
        self.to_value()
    }
}

/// A live arrangement retains its panels and the application's navigation data.
#[derive(Clone)]
pub struct LiveLayout<Context = ()> {
    pub panels: Vec<Entity>,
    pub layout: WorkbenchLayout,
    pub context: Context,
}

#[derive(Resource, Clone)]
pub struct LayoutSlots<Key: LayoutKey = LayoutId, Context: Clone + Send + Sync + 'static = ()> {
    active: Key,
    live: HashMap<Key, LiveLayout<Context>>,
    saved: Map<String, Value>,
}

impl<Key: LayoutKey, Context: Clone + Send + Sync + 'static> LayoutSlots<Key, Context> {
    pub fn new(active: Key) -> Self {
        Self {
            active,
            live: HashMap::new(),
            saved: Map::new(),
        }
    }
    pub fn from_saved(active: Key, saved: Map<String, Value>) -> Self {
        Self {
            saved,
            ..Self::new(active)
        }
    }
    pub fn active(&self) -> Key {
        self.active.clone()
    }
    pub fn live(&self) -> &HashMap<Key, LiveLayout<Context>> {
        &self.live
    }
    pub fn saved(&self) -> &Map<String, Value> {
        &self.saved
    }
    /// Host-selected activation during initialization or scope attachment.
    pub fn select(&mut self, active: Key) {
        self.active = active;
    }
    pub fn take_live(&mut self) -> HashMap<Key, LiveLayout<Context>> {
        std::mem::take(&mut self.live)
    }
    pub fn replace_live(&mut self, live: HashMap<Key, LiveLayout<Context>>) {
        self.live = live;
    }
}

impl<Key: LayoutKey + Default, Context: Clone + Send + Sync + 'static> Default
    for LayoutSlots<Key, Context>
{
    fn default() -> Self {
        Self::new(Key::default())
    }
}

/// Installs the default numbered-layout store. It does not read configuration,
/// select application templates, or add a competing transition schedule.
#[derive(Default)]
pub struct LayoutsPlugin;

impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bed_workbench::WorkbenchPlugin>() {
            app.add_plugins(bed_workbench::WorkbenchPlugin);
        }
        app.init_resource::<LayoutSlots>();
    }
}

pub fn active_panels(world: &mut World) -> Vec<Entity> {
    let mut panels = world
        .query::<(Entity, &PanelPresentation, &PanelRecord)>()
        .iter(world)
        .filter(|(entity, _, _)| {
            bed_workbench::owns_panel(world, *entity)
                && bed_api::workbench::panel_active(world, *entity)
        })
        .map(|(entity, panel, _)| (panel.id, entity))
        .collect::<Vec<_>>();
    panels.sort_by_key(|(id, _)| *id);
    panels.into_iter().map(|(_, entity)| entity).collect()
}

pub fn suspend<Context>(world: &mut World, live: &LiveLayout<Context>) {
    for &panel in &live.panels {
        if world.entities().contains(panel) {
            world.entity_mut(panel).insert(PanelSuspended);
        }
    }
}

/// Install retained membership and geometry. The host restores its own
/// navigation/lifecycle state immediately afterward at the same transition point.
pub fn activate<Context>(world: &mut World, live: &LiveLayout<Context>) {
    for &panel in &live.panels {
        if world.entities().contains(panel) {
            world.entity_mut(panel).remove::<PanelSuspended>();
        }
    }
    *world.resource_mut::<WorkbenchLayout>() = live.layout.clone();
}

/// Remove references to panels retired by the host while retaining other tabs.
pub fn retain_live_panels<Context>(world: &World, live: &mut LiveLayout<Context>) {
    live.panels
        .retain(|panel| world.get::<PanelPresentation>(*panel).is_some());
    live.layout.tiling.retain_panels(
        live.panels
            .iter()
            .map(|panel| world.get::<PanelPresentation>(*panel).unwrap().id),
    );
    live.layout.focused = live
        .layout
        .focused
        .filter(|panel| live.panels.contains(panel));
}

/// The domain-specific parts of a transition stay together in one application
/// implementation. Preflight/save barriers run before `transition`; `finish`
/// can persist the resulting state and fail the transition without losing tabs.
pub trait LayoutIntegration {
    type Context: Clone + Send + Sync + 'static;
    fn capture_context(world: &World) -> Self::Context;
    fn activate_context(world: &mut World, context: Option<&Self::Context>) -> io::Result<()>;
    fn restore(world: &mut World, value: &Value) -> io::Result<()>;
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()>;
    fn finish(world: &mut World) -> io::Result<()>;
}

/// One controlled layout transition, with rollback of membership, geometry and
/// host navigation if restoring or persisting the target fails. Callers serialize
/// requests and finish their document save barriers before entering this function.
pub fn transition<Key: LayoutKey, Integration: LayoutIntegration>(
    world: &mut World,
    target: Key,
    template: &Value,
) -> io::Result<()> {
    assert!(
        !bed_workbench::frontend_active(world),
        "layout transitions run between UI scopes"
    );
    let previous = world
        .resource::<LayoutSlots<Key, Integration::Context>>()
        .clone();
    if previous.active == target {
        return Ok(());
    }
    let outgoing = LiveLayout {
        panels: active_panels(world),
        layout: world.resource::<WorkbenchLayout>().clone(),
        context: Integration::capture_context(world),
    };
    suspend(world, &outgoing);
    let retained = {
        let mut slots = world.resource_mut::<LayoutSlots<Key, Integration::Context>>();
        slots.active = target.clone();
        slots.live.insert(previous.active.clone(), outgoing.clone());
        slots.live.remove(&target)
    };
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    let result = (|| {
        Integration::activate_context(world, None)?;
        if let Some(live) = &retained {
            activate(world, live);
            Integration::activate_context(world, Some(&live.context))?;
        } else {
            Integration::restore(
                world,
                previous.saved.get(&target.saved_key()).unwrap_or(template),
            )?;
        }
        Integration::finish(world)
    })();
    if let Err(error) = result {
        let cleanup = if let Some(live) = &retained {
            suspend(world, live);
            Ok(())
        } else {
            let created = active_panels(world);
            Integration::retire(world, &created)
        };
        world.insert_resource(previous);
        activate(world, &outgoing);
        let navigation = Integration::activate_context(world, Some(&outgoing.context));
        bed_workbench::invalidate_layout_frontend(world);
        cleanup?;
        navigation?;
        return Err(error);
    }
    bed_workbench::advance_layout_epoch(world);
    bed_workbench::invalidate_layout_frontend(world);
    Ok(())
}

/// Serialize all live slots with a host-provided panel serializer. Unavailable
/// saved slots remain unchanged; taking a snapshot never activates a layout.
pub fn snapshot<Key: LayoutKey, Context: Clone + Send + Sync + 'static>(
    world: &mut World,
    capture_context: impl FnOnce(&World) -> Context,
    mut serialize: impl FnMut(&mut World, Vec<Entity>, &WorkbenchLayout, &Context) -> io::Result<Value>,
) -> io::Result<Value> {
    let slots = world.resource::<LayoutSlots<Key, Context>>().clone();
    let mut saved = slots.saved;
    for (key, mut live) in slots.live {
        retain_live_panels(world, &mut live);
        saved.insert(
            key.saved_key(),
            serialize(world, live.panels, &live.layout, &live.context)?,
        );
    }
    let panels = active_panels(world);
    let layout = world.resource::<WorkbenchLayout>().clone();
    let context = capture_context(world);
    saved.insert(
        slots.active.saved_key(),
        serialize(world, panels, &layout, &context)?,
    );
    Ok(json!({"active_layout_slot": slots.active.saved_value(), "layouts": saved}))
}

/// Plain workbench integration: panels retain their own state and lifecycle.
/// Applications persist the resulting `snapshot` wherever they choose.
pub struct WorkbenchLayouts;

impl LayoutIntegration for WorkbenchLayouts {
    type Context = ();
    fn capture_context(_: &World) {}
    fn activate_context(world: &mut World, _: Option<&()>) -> io::Result<()> {
        if let Some(panel) = world.resource::<WorkbenchLayout>().focused {
            bed_workbench::focus_panel(world, panel);
        }
        Ok(())
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        for &panel in panels {
            bed_workbench::commit_close(world, panel);
        }
        Ok(())
    }
    fn finish(_: &mut World) -> io::Result<()> {
        Ok(())
    }
}

pub fn save(world: &mut World) -> Value {
    snapshot::<LayoutId, ()>(
        world,
        |_| (),
        |world, panels, layout, _| Ok(bed_workbench::save_layout(world, panels, layout)),
    )
    .expect("plain workbench snapshots are infallible")
}

/// Restore the incoming arrangement without retiring panels suspended in other
/// slots. Saved IDs are remapped when they collide with retained live panels.
pub fn restore(world: &mut World, value: &Value) -> io::Result<()> {
    let records = value["panels"]
        .as_array()
        .ok_or_else(|| io::Error::other("Saved panels must be an array"))?;
    if value["version"].as_u64() != Some(1) {
        return Err(io::Error::other("Unsupported workbench version"));
    }
    let mut ids = HashSet::new();
    for record in records {
        let id = record["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .ok_or_else(|| io::Error::other("Invalid saved panel ID"))?;
        if !ids.insert(id) {
            return Err(io::Error::other("Duplicate saved panel ID"));
        }
        if record["panel_type"].as_str().is_none_or(str::is_empty) {
            return Err(io::Error::other("Saved panel has no type"));
        }
    }
    if !value["focused"].is_null()
        && value["focused"]
            .as_u64()
            .is_none_or(|id| !ids.contains(&id))
    {
        return Err(io::Error::other("Saved focused panel is unavailable"));
    }
    let mut tiling = bed_workbench::workspace::tiling_state::TilingState::from_value(
        &value["tiling"],
        ids.iter().copied(),
    )
    .map_err(io::Error::other)?;
    let mut used = world
        .query::<&PanelPresentation>()
        .iter(world)
        .map(|panel| panel.id)
        .collect::<HashSet<_>>();
    let mut next = used.iter().copied().max().unwrap_or(0) + 1;
    let mut remap = HashMap::new();
    for id in ids {
        let live_id = if used.contains(&id) {
            let id = next;
            next += 1;
            id
        } else {
            id
        };
        if live_id > u32::MAX as u64 {
            return Err(io::Error::other("Panel IDs exhausted"));
        }
        used.insert(live_id);
        remap.insert(id, live_id);
    }
    tiling.remap_panel_ids(&remap);
    let old = active_panels(world);
    if !old.is_empty() {
        return Err(io::Error::other(
            "Suspend or retire the active layout before restoring another",
        ));
    }
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    world.resource_mut::<WorkbenchLayout>().tiling = tiling.clone();
    for record in records {
        let id = remap[&record["id"].as_u64().unwrap()];
        bed_workbench::open_panel(world, record.clone(), tiling.area_for(id), Some(id))
            .map_err(io::Error::other)?;
    }
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    let panels = active_panels(world);
    for &panel in &panels {
        let id = world.get::<PanelPresentation>(panel).unwrap().id;
        let selected = world
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .any(|area| area.selected == Some(id));
        world.get_mut::<PanelPresentation>(panel).unwrap().focus = selected;
    }
    let focus = value["focused"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .and_then(|id| {
            panels
                .iter()
                .copied()
                .find(|panel| world.get::<PanelPresentation>(*panel).unwrap().id == *id)
        });
    if let Some(panel) = focus {
        bed_workbench::focus_panel(world, panel);
    } else {
        world.resource_mut::<WorkbenchLayout>().focused = None;
        if let Some(area) = value["focused_area"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
        {
            bed_workbench::focus_area(world, area);
        }
    }
    Ok(())
}

/// Save only the arrangement and panel kinds, excluding transient panels and
/// feature state. Applications can add their own template metadata separately.
pub fn template(world: &mut World) -> Value {
    let mut value = bed_workbench::save(world);
    if let Some(panels) = value["panels"].as_array_mut() {
        for panel in panels {
            *panel = json!({"id": panel["id"], "panel_type": panel["panel_type"]});
        }
    }
    value
}

#[cfg(test)]
mod tests;
