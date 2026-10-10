//! Workspace-owned area contents. ImGui only presents these tab groups.
use super::tiling::{Area, DockPlan, EXTENT, Layout, Rect};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AreaTabs {
    pub area: u32,
    pub tabs: Vec<u64>,
    pub selected: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TilingState {
    pub layout: Layout,
    pub areas: Vec<AreaTabs>,
}

impl Default for TilingState {
    fn default() -> Self {
        Self::new(Layout::default())
    }
}

impl TilingState {
    pub fn new(layout: Layout) -> Self {
        let areas = layout
            .areas
            .iter()
            .map(|area| AreaTabs {
                area: area.id,
                tabs: Vec::new(),
                selected: None,
            })
            .collect();
        Self { layout, areas }
    }

    pub fn area_for(&self, panel: u64) -> Option<u32> {
        self.areas
            .iter()
            .find(|area| area.tabs.contains(&panel))
            .map(|area| area.area)
    }

    pub fn assign(&mut self, panel: u64, area: u32) {
        if self.area_for(panel) == Some(area) {
            return;
        }
        self.remove(panel);
        let group = self
            .areas
            .iter_mut()
            .find(|group| group.area == area)
            .expect("live area has a tab group");
        group.tabs.push(panel);
        if group.selected.is_none() {
            group.selected = Some(panel);
        }
    }

    pub fn remove(&mut self, panel: u64) {
        for group in &mut self.areas {
            group.tabs.retain(|id| *id != panel);
            if group.selected == Some(panel) {
                group.selected = group.tabs.first().copied();
            }
        }
    }

    pub fn add_area(&mut self, area: u32) {
        assert!(self.layout.areas.iter().any(|value| value.id == area));
        assert!(!self.areas.iter().any(|group| group.area == area));
        self.areas.push(AreaTabs {
            area,
            tabs: Vec::new(),
            selected: None,
        });
    }

    /// The host closes eliminated areas' contents before committing geometry.
    /// Surviving groups keep their order and selection.
    pub fn apply_plan(&mut self, plan: DockPlan) {
        let mut previous = std::mem::take(&mut self.areas);
        let mut groups = Vec::with_capacity(plan.layout.areas.len());
        for area in &plan.layout.areas {
            groups.push(
                previous
                    .iter()
                    .position(|group| group.area == area.id)
                    .map(|index| previous.remove(index))
                    .unwrap_or(AreaTabs {
                        area: area.id,
                        tabs: Vec::new(),
                        selected: None,
                    }),
            );
        }
        assert!(
            previous.iter().all(|group| group.tabs.is_empty()),
            "eliminated areas' tabs are closed before committing geometry"
        );
        self.layout = plan.layout;
        self.areas = groups;
    }

    pub fn sync_area(&mut self, area: u32, tabs: Vec<u64>, selected: Option<u64>) {
        assert!(selected.is_none_or(|id| tabs.contains(&id)));
        for &panel in &tabs {
            self.assign(panel, area);
        }
        let group = self
            .areas
            .iter_mut()
            .find(|group| group.area == area)
            .expect("live area has a tab group");
        group.selected = selected.or_else(|| tabs.first().copied());
        group.tabs = tabs;
    }

    pub fn close_empty_area(&mut self, area: u32) -> bool {
        if !self
            .areas
            .iter()
            .any(|group| group.area == area && group.tabs.is_empty())
            || !self.layout.close_area(area)
        {
            return false;
        }
        self.areas.retain(|group| group.area != area);
        true
    }

    /// The insertion index refers to the destination after removing the tab,
    /// so moving within an area uses the same transition as moving between areas.
    pub fn move_tab(&mut self, panel: u64, target: u32, index: usize) {
        let source = self.move_tab_contents(panel, target, index);
        if source != target {
            self.close_empty_area(source);
        }
    }

    fn move_tab_contents(&mut self, panel: u64, target: u32, index: usize) -> u32 {
        let source = self.area_for(panel).expect("live tab belongs to an area");
        assert!(self.areas.iter().any(|group| group.area == target));
        self.remove(panel);
        let group = self
            .areas
            .iter_mut()
            .find(|group| group.area == target)
            .unwrap();
        group.tabs.insert(index.min(group.tabs.len()), panel);
        group.selected = Some(panel);
        source
    }

    /// Split the destination and move only this tab into the requested half.
    /// Geometry and membership are committed together after the split succeeds.
    pub fn split_tab(
        &mut self,
        panel: u64,
        target: u32,
        axis: usize,
        high: bool,
        minimum: [i32; 2],
    ) -> Option<u32> {
        assert!(axis < 2 && minimum.iter().all(|size| *size > 0));
        let source = self.area_for(panel).expect("live tab belongs to an area");
        let rect = self.layout.area(target).rect;
        let mut layout = self.layout.clone();
        let new_area = layout.split(
            target,
            axis,
            (rect.min[axis] + rect.max[axis]) / 2,
            minimum[axis],
        )?;
        if !high {
            let low = layout.area(target).rect;
            let upper = layout.area(new_area).rect;
            for area in &mut layout.areas {
                if area.id == target {
                    area.rect = upper;
                } else if area.id == new_area {
                    area.rect = low;
                }
            }
        }
        self.layout = layout;
        self.add_area(new_area);
        self.move_tab_contents(panel, new_area, 0);
        // Splitting within the source deliberately creates an empty half.
        // A cross-area drop instead removes the footprint it vacates.
        if source != target {
            self.close_empty_area(source);
        }
        Some(new_area)
    }

    pub fn retain_panels(&mut self, ids: impl IntoIterator<Item = u64>) {
        let ids = ids.into_iter().collect::<HashSet<_>>();
        for group in &mut self.areas {
            group.tabs.retain(|id| ids.contains(id));
            if group.selected.is_some_and(|id| !group.tabs.contains(&id)) {
                group.selected = group.tabs.first().copied();
            }
        }
    }

    /// Nonpersistent panels do not leave stale references in the saved state.
    /// Empty areas remain; closing content does not implicitly change geometry.
    pub fn to_value(&self, panel_ids: impl IntoIterator<Item = u64>) -> Value {
        let panel_ids = panel_ids.into_iter().collect::<HashSet<_>>();
        let mut assigned = HashSet::new();
        let groups = self
            .areas
            .iter()
            .map(|group| {
                let tabs = group
                    .tabs
                    .iter()
                    .copied()
                    .filter(|id| panel_ids.contains(id))
                    .collect::<Vec<_>>();
                for id in &tabs {
                    assert!(assigned.insert(*id), "panel belongs to exactly one area");
                }
                let selected = group
                    .selected
                    .filter(|id| tabs.contains(id))
                    .or_else(|| tabs.first().copied());
                json!({"area":group.area,"tabs":tabs,"selected":selected})
            })
            .collect::<Vec<_>>();
        assert_eq!(assigned, panel_ids, "every saved panel belongs to an area");
        json!({"version":1,"layout":self.layout.to_value(),"areas":groups})
    }

    /// Validate disk geometry and all membership relationships before they
    /// become live state. Internal transitions can then trust these invariants.
    pub fn from_value(
        value: &Value,
        panel_ids: impl IntoIterator<Item = u64>,
    ) -> Result<Self, &'static str> {
        if value["version"].as_u64() != Some(1) {
            return Err("Unsupported workspace area contents version");
        }
        let layout = Layout::from_value(&value["layout"])?;
        let values = value["areas"]
            .as_array()
            .filter(|groups| groups.len() == layout.areas.len())
            .ok_or("Missing workspace area contents")?;
        let panel_ids = panel_ids.into_iter().collect::<HashSet<_>>();
        if panel_ids
            .iter()
            .any(|id| *id == 0 || *id > u64::from(u32::MAX))
        {
            return Err("Invalid workspace panel ID");
        }
        let mut area_ids = HashSet::new();
        let mut assigned = HashSet::new();
        let mut areas = Vec::with_capacity(values.len());
        for group in values {
            let area = group["area"]
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .filter(|id| layout.areas.iter().any(|area| area.id == *id))
                .filter(|id| area_ids.insert(*id))
                .ok_or("Duplicate or unknown workspace area contents")?;
            let values = group["tabs"]
                .as_array()
                .filter(|tabs| tabs.len() <= panel_ids.len())
                .ok_or("Invalid workspace tab group")?;
            let mut tabs = Vec::with_capacity(values.len());
            for value in values {
                let id = value
                    .as_u64()
                    .filter(|id| panel_ids.contains(id) && assigned.insert(*id))
                    .ok_or("Duplicate or unknown workspace panel membership")?;
                tabs.push(id);
            }
            let selected = if group["selected"].is_null() {
                tabs.first().copied()
            } else {
                Some(
                    group["selected"]
                        .as_u64()
                        .filter(|id| tabs.contains(id))
                        .ok_or("Selected workspace panel is outside its area")?,
                )
            };
            areas.push(AreaTabs {
                area,
                tabs,
                selected,
            });
        }
        if assigned != panel_ids {
            return Err("Workspace panels are missing area membership");
        }
        Ok(Self { layout, areas })
    }

    /// Convert the old native split tree to workspace rectangles. Detached
    /// roots become tabs in the largest main area because v1 has one OS window.
    pub fn from_legacy_ini(
        ini: &str,
        panel_ids: impl IntoIterator<Item = u64>,
    ) -> Result<Self, &'static str> {
        let panel_ids = panel_ids.into_iter().collect::<Vec<_>>();
        let ini = super::layout::prepare(ini, panel_ids.iter().copied())?;
        LegacyLayout::parse(&ini, &panel_ids)
    }

    /// Attachment has already validated the layout and allocated distinct IDs.
    pub fn remap_panel_ids(&mut self, ids: &HashMap<u64, u64>) {
        assert_eq!(ids.values().collect::<HashSet<_>>().len(), ids.len());
        self.retain_panels(ids.keys().copied());
        for group in &mut self.areas {
            for id in &mut group.tabs {
                *id = ids[&*id];
            }
            group.selected = group.selected.map(|id| ids[&id]);
        }
    }
}

struct LegacyNode {
    id: u32,
    parent: Option<u32>,
    axis: Option<usize>,
    size: [i32; 2],
    selected: Option<u32>,
    dock_space: bool,
    host: Option<u32>,
    children: Vec<u32>,
}

struct LegacyLayout;
impl LegacyLayout {
    fn parse(ini: &str, panel_ids: &[u64]) -> Result<TilingState, &'static str> {
        let mut nodes = Vec::<LegacyNode>::new();
        let mut windows = HashMap::<u64, (u32, i32)>::new();
        let mut window = None;
        let mut docking = false;
        for line in ini.lines().map(str::trim) {
            if let Some(name) = line
                .strip_prefix("[Window][")
                .and_then(|name| name.strip_suffix(']'))
            {
                let name = name.rsplit_once("###").map_or(name, |(_, name)| name);
                window = name
                    .strip_prefix("bed_tab_")
                    .and_then(|id| id.parse::<u64>().ok());
                docking = false;
            } else if line.starts_with('[') {
                window = None;
                docking = line == "[Docking][Data]";
            } else if let Some(id) = window {
                if let Some(value) = line.strip_prefix("DockId=") {
                    let (dock, order) = value.split_once(',').unwrap_or((value, "0"));
                    windows.insert(
                        id,
                        (
                            hex(dock)?,
                            order.parse().map_err(|_| "Invalid legacy tab order")?,
                        ),
                    );
                }
            } else if docking && !line.is_empty() {
                let mut fields = line.split_whitespace();
                let kind = fields.next().ok_or("Invalid legacy dock node")?;
                let fields = fields
                    .map(|field| field.split_once('=').ok_or("Invalid legacy dock field"))
                    .collect::<Result<HashMap<_, _>, _>>()?;
                let id = hex(fields["ID"])?;
                let parent = fields.get("Parent").map(|id| hex(id)).transpose()?;
                let size = pair(
                    fields
                        .get(if parent.is_some() { "SizeRef" } else { "Size" })
                        .ok_or("Missing legacy dock size")?,
                )?;
                nodes.push(LegacyNode {
                    id,
                    parent,
                    axis: fields.get("Split").map(|axis| usize::from(*axis == "Y")),
                    size,
                    selected: fields.get("Selected").map(|id| hex(id)).transpose()?,
                    dock_space: kind == "DockSpace",
                    host: fields.get("Window").map(|id| hex(id)).transpose()?,
                    children: Vec::new(),
                });
            }
        }
        for index in 0..nodes.len() {
            if let Some(parent) = nodes[index].parent {
                let child = nodes[index].id;
                let parent = nodes
                    .iter_mut()
                    .find(|node| node.id == parent)
                    .expect("prepared legacy parent exists");
                parent.children.push(child);
            }
        }
        let host = hash("##bed_workspace", 0);
        let root = nodes
            .iter()
            .find(|node| node.parent.is_none() && node.dock_space && node.host == Some(host))
            .or_else(|| {
                nodes
                    .iter()
                    .find(|node| node.parent.is_none() && node.dock_space)
            })
            .or_else(|| nodes.iter().find(|node| node.parent.is_none()))
            .ok_or("Missing legacy main dock")?;
        let mut leaves = Vec::new();
        let mut pending = vec![(
            root.id,
            Rect {
                min: [0, 0],
                max: [EXTENT, EXTENT],
            },
        )];
        while let Some((id, rect)) = pending.pop() {
            let node = nodes.iter().find(|node| node.id == id).unwrap();
            if let Some(axis) = node.axis {
                let first = nodes
                    .iter()
                    .find(|child| child.id == node.children[0])
                    .unwrap();
                let second = nodes
                    .iter()
                    .find(|child| child.id == node.children[1])
                    .unwrap();
                let weights = i64::from(first.size[axis]) + i64::from(second.size[axis]);
                let dimension = rect.max[axis] - rect.min[axis];
                let offset = (i64::from(dimension) * i64::from(first.size[axis]) / weights) as i32;
                if offset <= 0 || offset >= dimension {
                    return Err("Legacy dock proportions are too small");
                }
                let mut low = rect;
                let mut high = rect;
                low.max[axis] = rect.min[axis] + offset;
                high.min[axis] = low.max[axis];
                pending.push((second.id, high));
                pending.push((first.id, low));
            } else {
                leaves.push((id, rect, node.selected));
            }
        }
        let layout = Layout::from_areas(
            leaves
                .iter()
                .enumerate()
                .map(|(index, (_, rect, _))| Area {
                    id: index as u32 + 1,
                    rect: *rect,
                })
                .collect(),
        )?;
        let mut state = TilingState::new(layout);
        let largest = state
            .layout
            .areas
            .iter()
            .max_by_key(|area| {
                i64::from(area.rect.max[0] - area.rect.min[0])
                    * i64::from(area.rect.max[1] - area.rect.min[1])
            })
            .unwrap()
            .id;
        for (index, (dock, _, selected)) in leaves.iter().enumerate() {
            let mut tabs = panel_ids
                .iter()
                .copied()
                .filter(|id| windows.get(id).is_some_and(|(node, _)| node == dock))
                .collect::<Vec<_>>();
            tabs.sort_by_key(|id| windows[id].1);
            let selected = tabs
                .iter()
                .copied()
                .find(|id| Some(hash("#TAB", hash(&format!("bed_tab_{id}"), 0))) == *selected)
                .or_else(|| tabs.first().copied());
            state.sync_area(index as u32 + 1, tabs, selected);
        }
        for &panel in panel_ids {
            if state.area_for(panel).is_none() {
                state.assign(panel, largest);
            }
        }
        Ok(state)
    }
}

fn hex(value: &str) -> Result<u32, &'static str> {
    u32::from_str_radix(
        value.strip_prefix("0x").ok_or("Invalid legacy dock ID")?,
        16,
    )
    .map_err(|_| "Invalid legacy dock ID")
}

fn pair(value: &str) -> Result<[i32; 2], &'static str> {
    let (x, y) = value
        .split_once(',')
        .ok_or("Invalid legacy dock dimensions")?;
    Ok([
        x.parse().map_err(|_| "Invalid legacy dock dimensions")?,
        y.parse().map_err(|_| "Invalid legacy dock dimensions")?,
    ])
}

fn hash(value: &str, seed: u32) -> u32 {
    unsafe { dear_imgui_rs::sys::igImHashStr(value.as_ptr().cast(), value.len(), seed) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tiling::DockPlacement;

    fn contents() -> TilingState {
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 5000, 100).unwrap();
        layout.split(right, 1, 5000, 100).unwrap();
        let mut state = TilingState::new(layout);
        state.sync_area(1, vec![12, 11], Some(11));
        state.sync_area(2, vec![21, 22], Some(22));
        state.sync_area(3, vec![31], Some(31));
        state
    }

    #[test]
    fn disk_roundtrip_preserves_rectangles_order_selection_and_empty_areas() {
        let mut state = contents();
        state.remove(31);
        let value = state.to_value([11, 12, 21, 22]);
        assert_eq!(
            TilingState::from_value(&value, [11, 12, 21, 22]).unwrap(),
            state
        );
        assert_eq!(value["areas"][2]["tabs"], json!([]));
        assert!(value["areas"][2]["selected"].is_null());
    }

    #[test]
    fn legacy_tab_visibility_settings_are_ignored_and_no_longer_saved() {
        let state = contents();
        for legacy in [json!(true), json!(false), json!("obsolete")] {
            let mut value = state.to_value([11, 12, 21, 22, 31]);
            for group in value["areas"].as_array_mut().unwrap() {
                group["tab_bar_visible"] = legacy.clone();
            }
            let restored = TilingState::from_value(&value, [11, 12, 21, 22, 31]).unwrap();
            assert_eq!(restored, state);
            let saved = restored.to_value([11, 12, 21, 22, 31]);
            assert!(
                saved["areas"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|group| { !group.as_object().unwrap().contains_key("tab_bar_visible") })
            );
        }
    }

    #[test]
    fn moving_the_last_tab_closes_its_old_area_and_preserves_the_destination() {
        let mut state = contents();
        state.move_tab(31, 1, 2);
        assert_eq!(state.areas[0].tabs, vec![12, 11, 31]);
        assert_eq!(state.areas[0].selected, Some(31));
        assert_eq!(state.layout.areas.len(), 2);
        assert!(!state.areas.iter().any(|group| group.area == 3));
        assert_eq!(
            state.layout.area(2).rect,
            Rect {
                min: [5000, 0],
                max: [EXTENT, EXTENT]
            }
        );
        assert_eq!(state.areas[1].tabs, vec![21, 22]);
        assert_eq!(state.areas[1].selected, Some(22));
        assert_eq!(
            TilingState::from_value(&state.to_value([11, 12, 21, 22, 31]), [11, 12, 21, 22, 31])
                .unwrap(),
            state
        );
    }

    #[test]
    fn closing_a_t_layout_empty_area_preserves_other_groups_and_selection() {
        let mut state = contents();
        state.remove(11);
        state.remove(12);
        let groups = state.areas[1..].to_vec();
        assert!(state.close_empty_area(1));
        assert_eq!(state.areas, groups);
        assert_eq!(
            state.layout.area(2).rect,
            Rect {
                min: [0, 0],
                max: [EXTENT, 5000]
            }
        );
        assert_eq!(
            state.layout.area(3).rect,
            Rect {
                min: [0, 5000],
                max: [EXTENT, EXTENT]
            }
        );
        assert_eq!(
            TilingState::from_value(&state.to_value([21, 22, 31]), [21, 22, 31]).unwrap(),
            state
        );
    }

    #[test]
    fn closing_a_filled_unknown_or_final_area_leaves_state_unchanged() {
        let mut state = contents();
        let before = state.clone();
        assert!(!state.close_empty_area(1));
        assert!(!state.close_empty_area(99));
        assert_eq!(state, before);
        let mut state = TilingState::default();
        let before = state.clone();
        assert!(!state.close_empty_area(1));
        assert_eq!(state, before);
    }

    #[test]
    fn moving_one_tab_keeps_its_nonempty_source_geometry() {
        let mut state = contents();
        let before = state.layout.clone();
        state.move_tab(11, 2, 1);
        assert_eq!(state.layout, before);
        assert_eq!(state.areas[0].tabs, vec![12]);
        assert_eq!(state.areas[0].selected, Some(12));
        assert_eq!(state.areas[1].tabs, vec![21, 11, 22]);
        assert_eq!(state.areas[1].selected, Some(11));
    }

    #[test]
    fn reordering_uses_the_final_index_and_selects_the_moved_tab() {
        let mut state = contents();
        state.move_tab(12, 1, 1);
        assert_eq!(state.areas[0].tabs, vec![11, 12]);
        assert_eq!(state.areas[0].selected, Some(12));
        state.move_tab(12, 1, 0);
        assert_eq!(state.areas[0].tabs, vec![12, 11]);
        assert_eq!(state.areas[0].selected, Some(12));
        assert_eq!(state.areas[1].tabs, vec![21, 22]);
    }

    #[test]
    fn tab_splits_preserve_target_contents_and_put_the_tab_on_each_requested_side() {
        for axis in 0..2 {
            for high in [false, true] {
                let mut state = contents();
                state.assign(32, 3);
                let target = state.layout.area(1).rect;
                let created = state.split_tab(31, 1, axis, high, [100, 100]).unwrap();
                assert_eq!(
                    Layout::from_value(&state.layout.to_value()).unwrap(),
                    state.layout
                );
                let destination = state.layout.area(created).rect;
                let remaining = state.layout.area(1).rect;
                let midpoint = (target.min[axis] + target.max[axis]) / 2;
                if high {
                    assert_eq!(destination.min[axis], midpoint);
                    assert_eq!(destination.max[axis], target.max[axis]);
                    assert_eq!(remaining.max[axis], midpoint);
                } else {
                    assert_eq!(destination.min[axis], target.min[axis]);
                    assert_eq!(destination.max[axis], midpoint);
                    assert_eq!(remaining.min[axis], midpoint);
                }
                assert_eq!(destination.min[1 - axis], target.min[1 - axis]);
                assert_eq!(destination.max[1 - axis], target.max[1 - axis]);
                assert_eq!(state.areas[0].tabs, vec![12, 11]);
                assert_eq!(state.areas[0].selected, Some(11));
                assert_eq!(state.areas[2].tabs, vec![32]);
                let group = state
                    .areas
                    .iter()
                    .find(|group| group.area == created)
                    .unwrap();
                assert_eq!(group.tabs, vec![31]);
                assert_eq!(group.selected, Some(31));
            }
        }
    }

    #[test]
    fn cross_area_edge_drops_close_the_empty_source_in_every_direction() {
        for axis in 0..2 {
            for high in [false, true] {
                let mut state = contents();
                let created = state.split_tab(31, 1, axis, high, [100, 100]).unwrap();
                assert!(!state.areas.iter().any(|group| group.area == 3));
                assert_eq!(state.area_for(31), Some(created));
                assert_eq!(
                    state
                        .areas
                        .iter()
                        .find(|group| group.area == created)
                        .unwrap()
                        .selected,
                    Some(31)
                );
                assert_eq!(
                    state
                        .areas
                        .iter()
                        .find(|group| group.area == 1)
                        .unwrap()
                        .tabs,
                    vec![12, 11]
                );
                assert_eq!(
                    state
                        .areas
                        .iter()
                        .find(|group| group.area == 2)
                        .unwrap()
                        .tabs,
                    vec![21, 22]
                );
                assert_eq!(
                    TilingState::from_value(
                        &state.to_value([11, 12, 21, 22, 31]),
                        [11, 12, 21, 22, 31]
                    )
                    .unwrap(),
                    state
                );
            }
        }
    }

    #[test]
    fn splitting_a_group_moves_one_tab_and_selects_a_remaining_tab_in_the_source() {
        let mut state = contents();
        let created = state.split_tab(11, 1, 0, true, [100, 100]).unwrap();
        assert_eq!(state.areas[0].tabs, vec![12]);
        assert_eq!(state.areas[0].selected, Some(12));
        assert_eq!(state.area_for(11), Some(created));
    }

    #[test]
    fn rejected_tab_splits_preserve_geometry_membership_and_selection() {
        let mut state = contents();
        let original = state.clone();
        assert_eq!(state.split_tab(31, 1, 0, false, [3000, 100]), None);
        assert_eq!(state, original);
    }

    #[test]
    fn splitting_the_only_tab_of_its_area_leaves_an_empty_half_without_duplication() {
        let mut state = contents();
        let before = state.layout.area(3).rect;
        let created = state.split_tab(31, 3, 0, false, [100, 100]).unwrap();
        let original = state.areas.iter().find(|group| group.area == 3).unwrap();
        assert!(original.tabs.is_empty());
        assert_eq!(original.selected, None);
        assert_eq!(state.area_for(31), Some(created));
        assert_eq!(state.areas.iter().flat_map(|group| &group.tabs).count(), 5);
        assert_eq!(state.layout.area(created).rect.min, before.min);
        assert_eq!(state.layout.area(created).rect.max[0], 7500);
        assert_eq!(state.layout.area(3).rect.min[0], 7500);
        assert_eq!(state.layout.area(3).rect.max, before.max);
        assert_eq!(
            TilingState::from_value(&state.to_value([11, 12, 21, 22, 31]), [11, 12, 21, 22, 31])
                .unwrap(),
            state
        );
    }

    #[test]
    fn disk_boundary_rejects_invalid_membership_without_repairing_geometry() {
        let original = contents().to_value([11, 12, 21, 22, 31]);
        for change in 0..5 {
            let mut invalid = original.clone();
            match change {
                0 => invalid["areas"][1]["tabs"][0] = json!(11),
                1 => invalid["areas"][0]["selected"] = json!(21),
                2 => invalid["areas"][1]["area"] = json!(1),
                3 => invalid["areas"][0]["tabs"] = json!([11]),
                4 => invalid["layout"]["areas"][0]["max"][0] = json!(5001),
                _ => unreachable!(),
            }
            assert!(
                TilingState::from_value(&invalid, [11, 12, 21, 22, 31]).is_err(),
                "invalid case {change}"
            );
        }
    }

    #[test]
    fn committing_a_plan_preserves_survivors_after_eliminated_contents_are_closed() {
        let mut state = contents();
        let plan = state
            .layout
            .plan_dock(1, 2, DockPlacement::Replace, [100, 100])
            .unwrap();
        state.remove(21);
        state.remove(22);
        state.apply_plan(plan);
        let moved = state.areas.iter().find(|group| group.area == 1).unwrap();
        assert_eq!(moved.tabs, vec![12, 11]);
        assert_eq!(moved.selected, Some(11));
        assert_eq!(state.area_for(21), None);
        assert_eq!(state.area_for(22), None);
        assert_eq!(
            state
                .areas
                .iter()
                .find(|group| group.area == 3)
                .unwrap()
                .tabs,
            vec![31]
        );
        let value = state.to_value([11, 12, 31]);
        assert_eq!(
            TilingState::from_value(&value, [11, 12, 31]).unwrap(),
            state
        );
    }

    #[test]
    fn serialization_filters_ephemeral_contents_and_repairs_the_saved_selection() {
        let state = contents();
        let saved = state.to_value([12, 21, 31]);
        let restored = TilingState::from_value(&saved, [12, 21, 31]).unwrap();
        assert_eq!(restored.areas[0].tabs, vec![12]);
        assert_eq!(restored.areas[0].selected, Some(12));
        assert_eq!(restored.areas[1].selected, Some(21));
        assert_eq!(state.areas[0].selected, Some(11));
    }

    #[test]
    fn attachment_remaps_membership_and_selected_ids_without_changing_geometry() {
        let original = contents();
        let ids = HashMap::from([(11, 111), (12, 112), (21, 121), (22, 122), (31, 131)]);
        let mut renamed = original.clone();
        renamed.remap_panel_ids(&ids);
        let value = renamed.to_value(ids.values().copied());
        let restored = TilingState::from_value(&value, ids.values().copied()).unwrap();
        assert_eq!(restored.layout, original.layout);
        assert_eq!(restored.areas[0].tabs, vec![112, 111]);
        assert_eq!(restored.areas[1].selected, Some(122));
    }

    #[test]
    fn legacy_t_layout_preserves_native_order_and_moves_detached_panels_inside() {
        let selected = hash("#TAB", hash("bed_tab_12", 0));
        let ini = format!(
            "[Window][bed_tab_11]\nDockId=0x11,1\n\n\
             [Window][bed_tab_12]\nDockId=0x11,0\n\n\
             [Window][bed_tab_21]\nDockId=0x21,0\n\n\
             [Window][bed_tab_31]\nDockId=0x31,0\n\n\
             [Window][bed_tab_41]\nDockId=0x41,0\n\n\
             [Docking][Data]\n\
             DockSpace ID=0x10 Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
             DockNode ID=0x11 Parent=0x10 SizeRef=480,800 Selected=0x{selected:08X}\n\
             DockNode ID=0x20 Parent=0x10 SizeRef=720,800 Split=Y\n\
             DockNode ID=0x21 Parent=0x20 SizeRef=720,400 CentralNode=1\n\
             DockNode ID=0x31 Parent=0x20 SizeRef=720,400\n\
             DockNode ID=0x41 Pos=3000,2000 Size=2000,2000\n"
        );
        let restored = TilingState::from_legacy_ini(&ini, [11, 12, 21, 31, 41]).unwrap();
        assert_eq!(restored.layout.area(1).rect.max, [4000, EXTENT]);
        assert_eq!(restored.layout.area(2).rect.min, [4000, 0]);
        assert_eq!(restored.layout.area(2).rect.max, [EXTENT, 5000]);
        assert_eq!(restored.layout.area(3).rect.min, [4000, 5000]);
        assert_eq!(restored.areas[0].tabs, vec![12, 11, 41]);
        assert_eq!(restored.areas[0].selected, Some(12));
        assert_eq!(restored.area_for(41), Some(1));
        let value = restored.to_value([11, 12, 21, 31, 41]);
        assert_eq!(
            TilingState::from_value(&value, [11, 12, 21, 31, 41]).unwrap(),
            restored
        );
    }
}
