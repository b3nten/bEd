//! Definitions describe initial arrangements; snapshots describe actual instances.
use super::*;
use bedtime_workbench::workspace::{tiling::Layout, tiling_state::TilingState};

#[derive(Clone, Debug, PartialEq)]
pub struct PanelDefinition {
    panel_type: String,
    viewer: Option<String>,
    initial_state: Value,
}
impl PanelDefinition {
    pub fn new(panel_type: impl Into<String>) -> Self {
        Self {
            panel_type: panel_type.into(),
            viewer: None,
            initial_state: Value::Null,
        }
    }
    pub fn viewer(mut self, viewer: impl Into<String>) -> Self {
        self.viewer = Some(viewer.into());
        self
    }
    pub fn initial_state(mut self, state: Value) -> Self {
        self.initial_state = state;
        self
    }
    pub fn panel_type(&self) -> &str {
        &self.panel_type
    }
    pub fn viewer_id(&self) -> Option<&str> {
        self.viewer.as_deref()
    }
    pub(super) fn record(&self, id: u64) -> Value {
        json!({"id":id,"panel_type":self.panel_type,"viewer":self.viewer,"state":self.initial_state})
    }
}

/// Content-independent geometry and initial panels. Definition IDs are local to
/// this arrangement; applying it remaps them to the retained live instances.
#[derive(Clone, Debug)]
pub struct LayoutDefinition {
    panels: Vec<(u64, PanelDefinition)>,
    tiling: TilingState,
    focused: Option<u64>,
    focused_area: Option<u32>,
}
impl LayoutDefinition {
    pub fn new(layout: Layout) -> Self {
        Self {
            panels: Vec::new(),
            tiling: TilingState::new(layout),
            focused: None,
            focused_area: None,
        }
    }
    pub fn add_panel(&mut self, panel: PanelDefinition, area: u32) -> io::Result<u64> {
        if !bedtime_api::workbench::valid_id(panel.panel_type())
            || panel
                .viewer_id()
                .is_some_and(|id| !bedtime_api::workbench::valid_id(id))
        {
            return Err(io::Error::other("Invalid panel or viewer type"));
        }
        if !self
            .tiling
            .layout
            .areas
            .iter()
            .any(|candidate| candidate.id == area)
        {
            return Err(io::Error::other("Definition area does not exist"));
        }
        let id = self
            .panels
            .iter()
            .map(|(id, _)| *id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|id| *id <= u32::MAX as u64)
            .ok_or_else(|| io::Error::other("Definition panel IDs exhausted"))?;
        self.panels.push((id, panel));
        self.tiling.assign(id, area);
        Ok(id)
    }
    pub fn focus(&mut self, id: u64) -> io::Result<()> {
        if !self.panels.iter().any(|(candidate, _)| *candidate == id) {
            return Err(io::Error::other("Definition focused panel does not exist"));
        }
        self.focused = Some(id);
        self.focused_area = self.tiling.area_for(id);
        for group in &mut self.tiling.areas {
            if group.tabs.contains(&id) {
                group.selected = Some(id);
            }
        }
        Ok(())
    }
    pub fn focus_area(&mut self, area: u32) -> io::Result<()> {
        if !self
            .tiling
            .layout
            .areas
            .iter()
            .any(|candidate| candidate.id == area)
        {
            return Err(io::Error::other("Definition focused area does not exist"));
        }
        self.focused = None;
        self.focused_area = Some(area);
        Ok(())
    }
    /// Persistence boundary for application-defined defaults. Document bindings,
    /// paths, histories and saved feature state are deliberately not definitions.
    pub fn from_value(value: &Value) -> io::Result<Self> {
        if !matches!(value["version"].as_u64(), Some(1 | 2)) {
            return Err(io::Error::other("Unsupported layout definition version"));
        }
        let records = value["panels"]
            .as_array()
            .ok_or_else(|| io::Error::other("Definition panels must be an array"))?;
        let mut ids = HashSet::new();
        let mut panels = Vec::new();
        for record in records {
            let id = record["id"]
                .as_u64()
                .filter(|id| *id > 0 && *id <= u32::MAX as u64)
                .ok_or_else(|| io::Error::other("Invalid definition panel ID"))?;
            if !ids.insert(id) {
                return Err(io::Error::other("Duplicate definition panel ID"));
            }
            let kind = record["panel_type"]
                .as_str()
                .filter(|kind| bedtime_api::workbench::valid_id(kind))
                .ok_or_else(|| io::Error::other("Invalid definition panel type"))?;
            let mut panel = PanelDefinition::new(kind);
            if let Some(viewer) = record.get("viewer").filter(|value| !value.is_null()) {
                let viewer = viewer
                    .as_str()
                    .filter(|id| bedtime_api::workbench::valid_id(id))
                    .ok_or_else(|| io::Error::other("Invalid definition viewer"))?;
                panel = panel.viewer(viewer);
            }
            // Initial state is explicit. Captured defaults omit it; serialized
            // snapshots must not accidentally become live document instances.
            if let Some(state) = record.get("initial_state") {
                panel.initial_state = state.clone();
            }
            panels.push((id, panel));
        }
        let tiling = TilingState::from_value(&value["tiling"], ids.iter().copied())
            .map_err(io::Error::other)?;
        let focused = value["focused"].as_u64();
        if focused.is_some_and(|id| !ids.contains(&id)) {
            return Err(io::Error::other("Invalid definition focus"));
        }
        let focused_area = value["focused_area"]
            .as_u64()
            .map(|id| u32::try_from(id).map_err(io::Error::other))
            .transpose()?;
        if focused_area.is_some_and(|id| !tiling.layout.areas.iter().any(|area| area.id == id)) {
            return Err(io::Error::other("Invalid definition focused area"));
        }
        Ok(Self {
            panels,
            tiling,
            focused,
            focused_area,
        })
    }
    pub fn to_value(&self) -> Value {
        let panels = self
            .panels
            .iter()
            .map(|(id, panel)| {
                let mut record = panel.record(*id);
                record.as_object_mut().unwrap().remove("state");
                if !panel.initial_state.is_null() {
                    record["initial_state"] = panel.initial_state.clone();
                }
                record
            })
            .collect::<Vec<_>>();
        json!({"version":1,"panels":panels,"tiling":self.tiling.to_value(self.panels.iter().map(|(id, _)| *id)),"focused":self.focused,"focused_area":self.focused_area})
    }
    /// Produce a plain workbench's initial snapshot. Core supplies the document
    /// equivalent without introducing document dependencies into this crate.
    pub fn snapshot(&self) -> Value {
        let mut value = self.to_value();
        value["panels"] = json!(
            self.panels
                .iter()
                .map(|(id, panel)| panel.record(*id))
                .collect::<Vec<_>>()
        );
        value
    }
}

pub fn capture_definition<Integration: LayoutIntegration>(world: &mut World) -> LayoutDefinition {
    let panels = active_panels(world)
        .into_iter()
        .filter(|panel| {
            let frontend = world
                .get::<bedtime_api::workbench::PanelFrontend>(*panel)
                .map_or(*panel, |frontend| frontend.0);
            !world
                .get::<bedtime_api::workbench::PanelPersistence>(frontend)
                .is_some_and(|policy| !policy.0)
        })
        .collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>().clone();
    let definitions = panels
        .iter()
        .map(|panel| {
            (
                world.get::<PanelPresentation>(*panel).unwrap().id,
                Integration::describe(world, *panel),
            )
        })
        .collect::<Vec<_>>();
    let ids = definitions
        .iter()
        .map(|(id, _)| *id)
        .collect::<HashSet<_>>();
    let tiling = TilingState::from_value(
        &layout.tiling.to_value(ids.iter().copied()),
        ids.iter().copied(),
    )
    .expect("captured live arrangement is valid");
    LayoutDefinition {
        panels: definitions,
        tiling,
        focused: layout
            .focused
            .and_then(|panel| world.get::<PanelPresentation>(panel))
            .map(|panel| panel.id)
            .filter(|id| ids.contains(id)),
        focused_area: layout.focused_area,
    }
}

/// Match existing instances in tab order, create missing initial tools, and move
/// extra tabs into the largest area. No existing document or process is retired.
pub fn rearrange<Integration: LayoutIntegration>(
    world: &mut World,
    definition: &LayoutDefinition,
) -> io::Result<()> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "layout changes run between UI scopes"
    );
    if !Integration::ready(world) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Layout is waiting for pending operations",
        ));
    }
    Integration::prepare(world)?;
    let previous = world.resource::<WorkbenchLayout>().clone();
    let context = Integration::capture_context(world);
    let mut remaining = active_panels(world);
    let original_panels = remaining.clone();
    let tab_order = previous
        .tiling
        .areas
        .iter()
        .flat_map(|area| area.tabs.iter().copied())
        .collect::<Vec<_>>();
    remaining.sort_by_key(|panel| {
        tab_order
            .iter()
            .position(|id| *id == world.get::<PanelPresentation>(*panel).unwrap().id)
            .unwrap_or(usize::MAX)
    });
    let result = (|| {
        let mut remap = HashMap::new();
        for (id, spec) in &definition.panels {
            let found = remaining.iter().position(|panel| {
                let existing = Integration::describe(world, *panel);
                existing.panel_type == spec.panel_type && existing.viewer == spec.viewer
            });
            let panel = if let Some(index) = found {
                remaining.remove(index)
            } else {
                Integration::create(world, spec)?
            };
            remap.insert(
                *id,
                world
                    .get::<PanelPresentation>(panel)
                    .expect("created panel has presentation")
                    .id,
            );
        }
        let mut tiling = definition.tiling.clone();
        tiling.remap_panel_ids(&remap);
        let destination = tiling.layout.largest_area();
        for panel in remaining {
            tiling.assign(
                world.get::<PanelPresentation>(panel).unwrap().id,
                destination,
            );
        }
        let panels = active_panels(world);
        bedtime_workbench::apply_layout(
            world,
            &tiling.to_value(
                panels
                    .iter()
                    .map(|panel| world.get::<PanelPresentation>(*panel).unwrap().id),
            ),
        )
        .map_err(io::Error::other)?;
        if let Some(id) = definition.focused.and_then(|id| remap.get(&id)) {
            let panel = panels
                .into_iter()
                .find(|panel| world.get::<PanelPresentation>(*panel).unwrap().id == *id)
                .expect("remapped focus exists");
            Integration::focus(world, panel)?;
        } else if let Some(area) = definition.focused_area {
            bedtime_workbench::focus_area(world, area);
        }
        Integration::finish(world)
    })();
    if let Err(error) = result {
        // A failed factory may already have allocated its owner or frontend.
        // Account for those partial instances as well as successful creations.
        let created = active_panels(world)
            .into_iter()
            .filter(|panel| !original_panels.contains(panel))
            .collect::<Vec<_>>();
        let retirement = Integration::retire(world, &created);
        *world.resource_mut::<WorkbenchLayout>() = previous;
        let navigation = Integration::activate_context(world, Some(&context));
        bedtime_workbench::invalidate_layout_frontend(world);
        retirement?;
        navigation?;
        return Err(error);
    }
    bedtime_workbench::advance_layout_epoch(world);
    bedtime_workbench::invalidate_layout_frontend(world);
    Ok(())
}
