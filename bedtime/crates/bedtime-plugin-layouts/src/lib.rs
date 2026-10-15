//! Retained layouts for Bevy workbenches. The application supplies templates,
//! serialized snapshots and persistence, and calls transitions between UI scopes.
mod definition;
mod requests;
use bedtime_api::workbench::PanelSuspended;
use bedtime_workbench::{PanelPresentation, PanelRecord, WorkbenchLayout};
use bevy::prelude::*;
pub use definition::{LayoutDefinition, PanelDefinition, capture_definition, rearrange};
pub use requests::{
    LayoutRequestId, cancel_requests, completed_request, poll, submit_rearrange, submit_transition,
    take_result,
};
use serde_json::{Map, Value, json};
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

/// Installs the plain numbered controller. Applications choose templates and
/// persistence; requests commit through one controller between frames.
#[derive(Default)]
pub struct LayoutsPlugin;

impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LayoutControllerPlugin::<LayoutId, WorkbenchLayouts>::new(
            LayoutId::default(),
        ));
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutSystems {
    Requests,
}

#[derive(Resource)]
struct ControllerInstalled;

/// Installs one controller for the application's chosen keys and panel lifecycle.
/// Requests commit between frames, after ordinary workbench/document operations.
pub struct LayoutControllerPlugin<Key: LayoutKey, Integration: LayoutIntegration> {
    initial: Key,
    integration: std::marker::PhantomData<fn() -> Integration>,
}
impl<Key: LayoutKey, Integration: LayoutIntegration> LayoutControllerPlugin<Key, Integration> {
    pub fn new(initial: Key) -> Self {
        Self {
            initial,
            integration: std::marker::PhantomData,
        }
    }
}
impl<Key: LayoutKey, Integration: LayoutIntegration + 'static> Plugin
    for LayoutControllerPlugin<Key, Integration>
{
    fn build(&self, app: &mut App) {
        assert!(
            !app.world().contains_resource::<ControllerInstalled>(),
            "A workbench has one layout controller"
        );
        if !app.is_plugin_added::<bedtime_workbench::WorkbenchPlugin>() {
            app.add_plugins(bedtime_workbench::WorkbenchPlugin);
        }
        if !app
            .world()
            .contains_resource::<LayoutSlots<Key, Integration::Context>>()
        {
            app.insert_resource(LayoutSlots::<Key, Integration::Context>::new(
                self.initial.clone(),
            ));
        }
        app.insert_resource(ControllerInstalled).add_systems(
            PostUpdate,
            poll::<Key, Integration>.in_set(LayoutSystems::Requests),
        );
    }
}

pub fn active_panels(world: &mut World) -> Vec<Entity> {
    let mut panels = world
        .query::<(Entity, &PanelPresentation, &PanelRecord)>()
        .iter(world)
        .filter(|(entity, _, _)| {
            bedtime_workbench::owns_panel(world, *entity)
                && bedtime_api::workbench::panel_active(world, *entity)
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

/// Panel/document lifecycle stages stay together in one integration. Preflight/save barriers run before `transition`; `finish`
/// can persist the resulting state and fail the transition without losing tabs.
pub trait LayoutIntegration {
    type Context: Clone + Send + Sync + 'static;
    fn capture_context(world: &World) -> Self::Context;
    fn activate_context(world: &mut World, context: Option<&Self::Context>) -> io::Result<()>;
    fn restore(world: &mut World, value: &Value) -> io::Result<()>;
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()>;
    fn ready(_: &World) -> bool {
        true
    }
    fn prepare(world: &mut World) -> io::Result<()> {
        persist(world)
    }
    fn finish(world: &mut World) -> io::Result<()> {
        persist(world)
    }
    fn describe(world: &World, panel: Entity) -> PanelDefinition {
        PanelDefinition::new(
            world
                .get::<PanelRecord>(panel)
                .expect("owned panel")
                .panel_type.clone(),
        )
    }
    fn create(world: &mut World, definition: &PanelDefinition) -> io::Result<Entity> {
        bedtime_workbench::open_panel(world, definition.record(1), None, None)
            .map_err(io::Error::other)
    }
    fn focus(world: &mut World, panel: Entity) -> io::Result<()> {
        bedtime_workbench::focus_panel(world, panel);
        Ok(())
    }
}

/// The application chooses its persistence adapter. The controller invokes it
/// before leaving an arrangement and after committing the next one, between UI
/// scopes. A failed write rolls the transition back.
#[derive(Resource, Clone, Copy)]
pub struct LayoutPersistence(pub bevy::ecs::system::SystemId<(), Result<(), String>>);

pub fn persist(world: &mut World) -> io::Result<()> {
    if let Some(callback) = world.get_resource::<LayoutPersistence>().copied() {
        world
            .run_system(callback.0)
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
    }
    Ok(())
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
        !bedtime_workbench::frontend_active(world),
        "layout transitions run between UI scopes"
    );
    let previous = world
        .resource::<LayoutSlots<Key, Integration::Context>>()
        .clone();
    if previous.active == target {
        return Ok(());
    }
    if !Integration::ready(world) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Layout transition is waiting for pending operations",
        ));
    }
    Integration::prepare(world)?;
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
        bedtime_workbench::invalidate_layout_frontend(world);
        cleanup?;
        navigation?;
        return Err(error);
    }
    bedtime_workbench::advance_layout_epoch(world);
    bedtime_workbench::invalidate_layout_frontend(world);
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
            bedtime_workbench::focus_panel(world, panel);
        }
        Ok(())
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        for &panel in panels {
            bedtime_workbench::commit_close(world, panel);
        }
        Ok(())
    }
}

pub fn save(world: &mut World) -> Value {
    snapshot::<LayoutId, ()>(
        world,
        |_| (),
        |world, panels, layout, _| Ok(bedtime_workbench::save_layout(world, panels, layout)),
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
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::from_value(
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
        bedtime_workbench::open_panel(world, record.clone(), tiling.area_for(id), Some(id))
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
        bedtime_workbench::focus_panel(world, panel);
    } else {
        world.resource_mut::<WorkbenchLayout>().focused = None;
        if let Some(area) = value["focused_area"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
        {
            bedtime_workbench::focus_area(world, area);
        }
    }
    Ok(())
}

/// Save only the arrangement and panel kinds, excluding transient panels and
/// feature state. Applications can add their own template metadata separately.
pub fn template(world: &mut World) -> Value {
    capture_definition::<WorkbenchLayouts>(world).to_value()
}

#[cfg(test)]
mod tests;
