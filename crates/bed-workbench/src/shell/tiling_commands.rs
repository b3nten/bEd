//! One owner for workbench area geometry and content placement.
use super::*;
use crate::workspace::tiling::{DockPlan, EXTENT, Layout};
use bed_workbench_api::PanelPlacement;

impl Workbench {
    pub(super) fn current_tiling(&self) -> &TilingState {
        self.pending_tiling.as_ref().unwrap_or(&self.tiling)
    }

    // Panel callbacks may request layout changes after DockSpaces were already
    // submitted. Commit those requests before the next frame's host submission.
    pub(super) fn edit_tiling(&mut self) -> &mut TilingState {
        if self.rendering_panels || self.pending_tiling.is_some() {
            self.pending_tiling
                .get_or_insert_with(|| self.tiling.clone())
        } else {
            &mut self.tiling
        }
    }

    pub(super) fn area_for_panel(&self, id: u64) -> Option<u32> {
        self.current_tiling().area_for(id)
    }

    pub(super) fn largest_area(&self) -> u32 {
        let mut areas = self.current_tiling().layout.areas.iter();
        let mut largest = areas.next().expect("workspace has an area");
        let size = |area: &crate::workspace::tiling::Area| {
            i64::from(area.rect.max[0] - area.rect.min[0])
                * i64::from(area.rect.max[1] - area.rect.min[1])
        };
        for area in areas {
            if size(area) > size(largest) {
                largest = area;
            }
        }
        largest.id
    }

    pub(super) fn focused_area(&self) -> Option<u32> {
        self.focus_history[0].and_then(|focus| self.focus_area(focus))
    }

    pub(super) fn previous_focused_area(&self) -> Option<u32> {
        self.focus_history[1].and_then(|focus| self.focus_area(focus))
    }

    fn focus_area(&self, focus: FocusedPanel) -> Option<u32> {
        match focus {
            FocusedPanel::Tab(id) => self.area_for_panel(id),
            FocusedPanel::EmptyArea(id) => self
                .current_tiling()
                .layout
                .areas
                .iter()
                .any(|area| area.id == id)
                .then_some(id),
        }
    }

    pub(super) fn remember_focus(&mut self, focus: FocusedPanel) {
        if self.focus_history[0] != Some(focus) {
            self.focus_history = [Some(focus), self.focus_history[0]];
            self.scene += 1;
        }
    }

    pub(super) fn forget_focus(&mut self, focus: FocusedPanel) {
        if self.focus_history[0] == Some(focus) {
            self.focus_history = [self.focus_history[1], None];
        } else if self.focus_history[1] == Some(focus) {
            self.focus_history[1] = None;
        }
    }

    pub(super) fn focused_or_last_area(&self) -> u32 {
        self.focused_area().unwrap_or_else(|| self.largest_area())
    }

    pub(super) fn focus_empty_area(&mut self, area: u32) {
        self.clear_focus_requests();
        self.focused = None;
        self.active = None;
        self.remember_focus(FocusedPanel::EmptyArea(area));
        if let Some(binding) = &self.context_binding {
            let name = CString::new(format!("###bed_empty_area_{area}")).unwrap();
            binding.with_bound_context(|| unsafe {
                let window = sys::igFindWindowByName(name.as_ptr());
                if !window.is_null() {
                    sys::igFocusWindow(window, sys::ImGuiFocusRequestFlags_UnlessBelowModal);
                }
            });
        }
    }

    pub(super) fn place_panel_in_area(&mut self, id: u64, area: u32) {
        self.edit_tiling().assign(id, area);
        let group = self
            .edit_tiling()
            .areas
            .iter_mut()
            .find(|group| group.area == area)
            .unwrap();
        group.selected = Some(id);
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.dock_next = true;
        }
        self.scene += 1;
    }

    fn tiling_minimum(&self) -> [i32; 2] {
        if self.tiling_ui.minimum.iter().all(|size| *size > 0) {
            self.tiling_ui.minimum
        } else {
            [500; 2]
        }
    }

    pub(super) fn can_split_area(&self, area: u32, axis: usize, at: i32) -> bool {
        self.current_tiling()
            .layout
            .clone()
            .split(area, axis, at, self.tiling_minimum()[axis])
            .is_some()
    }

    pub(super) fn split_area(&mut self, area: u32, axis: usize, at: i32) -> Option<u32> {
        let minimum = self.tiling_minimum();
        let new = self
            .edit_tiling()
            .layout
            .split(area, axis, at, minimum[axis])?;
        self.edit_tiling().add_area(new);
        self.tiling_ui.cancel_interaction();
        self.dock_built = true;
        self.scene += 1;
        Some(new)
    }

    pub(super) fn place_panel_beside(
        &mut self,
        source: u64,
        panel: u64,
        down: Option<bool>,
    ) -> bool {
        let Some(area) = self.area_for_panel(source) else {
            return false;
        };
        let destination = if let Some(down) = down {
            let axis = usize::from(down);
            let rect = self.current_tiling().layout.area(area).rect;
            let Some(new) = self.split_area(area, axis, (rect.min[axis] + rect.max[axis]) / 2)
            else {
                return false;
            };
            new
        } else {
            area
        };
        self.place_panel_in_area(panel, destination);
        true
    }

    pub(super) fn default_tiling_layout(&mut self) {
        let mut state = TilingState::new(Layout::default());
        let mut center = 1;
        let sidebar = self
            .tabs
            .iter()
            .any(|tab| self.panel_placement(&tab.panel) == PanelPlacement::Sidebar);
        let terminal_only = self.workspace_spec.is_none()
            && self
                .tabs
                .iter()
                .all(|tab| tab.panel.terminal_id().is_some());
        let bottom = !terminal_only
            && self
                .tabs
                .iter()
                .any(|tab| self.panel_placement(&tab.panel) == PanelPlacement::Bottom);
        if sidebar {
            center = state.layout.split(center, 0, EXTENT / 5, 1).unwrap();
            state.add_area(center);
        }
        let bottom_area = if bottom {
            let at = if self.tabs.iter().any(|tab| {
                tab.panel.kind != bed_module_terminal::PANEL_ID
                    && self.panel_placement(&tab.panel) == PanelPlacement::Bottom
            }) {
                EXTENT * 3 / 5
            } else {
                EXTENT * 3 / 4
            };
            let area = state.layout.split(center, 1, at, 1).unwrap();
            state.add_area(area);
            area
        } else {
            center
        };
        for tab in &self.tabs {
            state.assign(
                tab.id,
                match self.panel_placement(&tab.panel) {
                    PanelPlacement::Sidebar => 1,
                    PanelPlacement::Center => center,
                    PanelPlacement::Bottom => bottom_area,
                },
            );
        }
        if let Some(focused) = self.focused {
            for group in &mut state.areas {
                if group.tabs.contains(&focused) {
                    group.selected = Some(focused);
                }
            }
        }
        if self.rendering_panels {
            self.pending_tiling = Some(state);
        } else {
            self.pending_tiling = None;
            self.tiling = state;
        }
        self.dock_built = true;
        self.reset_tiling_docks();
    }

    pub(super) fn reset_tiling_docks(&mut self) {
        self.tiling_ui.cancel_interaction();
        self.tiling_ui.clear_roots = true;
        for tab in &mut self.tabs {
            tab.dock_next = true;
        }
        self.center_dock = 0;
        self.explorer_dock = 0;
        self.terminal_dock = 0;
    }

    pub(super) fn replace_panel_id(&mut self, old: u64, new: u64) {
        if old == new {
            return;
        }
        let rename = |state: &mut TilingState| {
            if state.area_for(new).is_some() {
                state.remove(old);
            } else {
                for group in &mut state.areas {
                    for id in &mut group.tabs {
                        if *id == old {
                            *id = new;
                        }
                    }
                    if group.selected == Some(old) {
                        group.selected = Some(new);
                    }
                }
            }
        };
        rename(&mut self.tiling);
        if let Some(state) = &mut self.pending_tiling {
            rename(state);
        }
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == old)
            .expect("restored panel exists");
        tab.id = new;
        tab.dock_next = true;
        if self.focused == Some(old) {
            self.focused = Some(new);
        }
        for focus in &mut self.focus_history {
            if *focus == Some(FocusedPanel::Tab(old)) {
                *focus = Some(FocusedPanel::Tab(new));
            }
        }
        if self.focus_history[0] == self.focus_history[1] {
            self.focus_history[1] = None;
        }
        self.next_tab = self
            .next_tab
            .max(new.checked_add(1).expect("validated panel ID"));
    }

    pub(super) fn duplicate_panel(&mut self, id: u64, area: u32) -> io::Result<Option<u64>> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return Ok(None);
        };
        let (kind, input, state, viewer) = (
            tab.panel.kind.clone(),
            tab.panel.input.clone(),
            if tab.panel.terminal_id().is_some() {
                // Corner splits create fresh shells at the window base.
                Value::Null
            } else {
                tab.panel.instance.save_state()
            },
            tab.panel.viewer.clone(),
        );
        self.open_plugin_panel_in_area(&kind, input, &state, viewer, area)
            .map(Some)
    }

    // Closing an area and committing its geometry are one operation. Remote
    // saves resume this same transition; cancellation leaves the layout intact.
    pub(super) fn close_tiling_area(
        &mut self,
        area: u32,
        original: Layout,
        plan: DockPlan,
        panels: Vec<u64>,
    ) -> io::Result<bool> {
        if self.pending_tab_close.is_some() || self.current_tiling().layout != original {
            return Ok(false);
        }
        let removed_areas = plan
            .remap
            .iter()
            .map(|(old, _)| *old)
            .collect::<HashSet<_>>();
        let removed_panels = panels.iter().copied().collect::<HashSet<_>>();
        let current_panels = |state: &TilingState| {
            state
                .areas
                .iter()
                .filter(|group| removed_areas.contains(&group.area))
                .flat_map(|group| group.tabs.iter().copied())
                .collect::<HashSet<_>>()
        };
        if current_panels(self.current_tiling()) != removed_panels {
            return Ok(false);
        }
        let indices = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| removed_panels.contains(&tab.id).then_some(index))
            .collect::<Vec<_>>();
        if !indices.is_empty() && !self.preflight_close(&indices)? {
            if self.current_tiling().layout == original
                && current_panels(self.current_tiling()) == removed_panels
                && self.close_is_pending(&panels)
            {
                self.pending_tab_close = Some(PendingTabClose::Area {
                    panels,
                    area,
                    original,
                    plan,
                });
            }
            return Ok(false);
        }
        // Committing staged panel input can process other layout requests.
        // Discard this plan if it no longer describes the current workspace.
        if self.current_tiling().layout != original
            || current_panels(self.current_tiling()) != removed_panels
        {
            return Ok(false);
        }
        for id in &panels {
            if let Some(index) = self.tabs.iter().position(|tab| tab.id == *id) {
                self.remove_tab(index)?;
            }
        }
        if self.current_tiling().layout != original
            || current_panels(self.current_tiling())
                .iter()
                .any(|id| !removed_panels.contains(id))
        {
            return Ok(false);
        }
        self.cancel_remote_area_contents(&removed_areas, &removed_panels);
        for removed in &removed_areas {
            self.forget_focus(FocusedPanel::EmptyArea(*removed));
        }
        for id in panels {
            self.edit_tiling().remove(id);
        }
        // A viewer switch awaiting a closed area's contents must not recreate
        // them in the survivor after its close operation finishes.
        if self
            .modules
            .pending_switch
            .as_ref()
            .is_some_and(|(_, _, area)| area.is_some_and(|area| removed_areas.contains(&area)))
        {
            self.modules.pending_switch = None;
        }
        let state = self.edit_tiling();
        state.apply_plan(plan);
        let selected = state
            .areas
            .iter()
            .find(|group| group.area == area)
            .and_then(|group| group.selected);
        for tab in &mut self.tabs {
            tab.dock_next = true;
        }
        if let Some(index) = selected.and_then(|id| self.tabs.iter().position(|tab| tab.id == id)) {
            self.switch_to_tab(index);
        } else {
            self.focus_empty_area(area);
        }
        self.scene += 1;
        Ok(true)
    }

    pub(super) fn close_empty_area(&mut self, area: u32) -> bool {
        let focused = self.focused_area() == Some(area);
        if !self.edit_tiling().close_empty_area(area) {
            return false;
        }
        self.finish_empty_area_close(area);
        if focused {
            let destination = self.focused_or_last_area();
            let selected = self
                .current_tiling()
                .areas
                .iter()
                .find(|group| group.area == destination)
                .and_then(|group| group.selected);
            if let Some(index) =
                selected.and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            {
                self.switch_to_tab(index);
            } else {
                self.focus_empty_area(destination);
            }
        }
        true
    }

    fn finish_empty_area_close(&mut self, area: u32) {
        self.cancel_remote_area_contents(&HashSet::from([area]), &HashSet::new());
        if self
            .file_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.target_area == Some(area))
        {
            self.file_dialog = None;
        }
        if self
            .modules
            .pending_switch
            .as_ref()
            .is_some_and(|(_, _, target)| *target == Some(area))
        {
            self.modules.pending_switch = None;
        }
        self.forget_focus(FocusedPanel::EmptyArea(area));
        for tab in &mut self.tabs {
            tab.dock_next = true;
        }
        self.tiling_ui.cancel_interaction();
        self.scene += 1;
    }

    pub(super) fn prepare_tiling(&mut self) -> io::Result<()> {
        self.rendering_panels = false;
        if let Some(state) = self.pending_tiling.take() {
            if state.layout != self.tiling.layout {
                self.tiling_ui.cancel_interaction();
            }
            self.tiling = state;
            for tab in &mut self.tabs {
                tab.dock_next = true;
            }
        }
        if !self.dock_built {
            self.default_tiling_layout();
        }
        for action in std::mem::take(&mut self.tiling_ui.actions) {
            // UI requests may outlive their source: a module or close popup can
            // remove content later in the release frame. Validate at this boundary.
            let destination = match &action {
                tiling_host::TilingAction::MoveTab { panel, area, .. }
                | tiling_host::TilingAction::SplitTab { panel, area, .. } => {
                    if !self.tabs.iter().any(|tab| tab.id == *panel) {
                        continue;
                    }
                    Some(*area)
                }
                tiling_host::TilingAction::AddPanel { area, .. } => Some(*area),
                tiling_host::TilingAction::Split { area, .. } => Some(*area),
                tiling_host::TilingAction::Plan { area, .. } => Some(*area),
                tiling_host::TilingAction::CloseArea(area) => Some(*area),
                tiling_host::TilingAction::CloseTab(_) => None,
            };
            if destination
                .is_some_and(|id| !self.tiling.layout.areas.iter().any(|area| area.id == id))
            {
                continue;
            }
            let result = (|| -> io::Result<()> {
                match action {
                    tiling_host::TilingAction::Split { area, axis, at } => {
                        self.split_area(area, axis, at);
                    }
                    tiling_host::TilingAction::Plan { area, plan } => {
                        let panels = self
                            .tiling
                            .areas
                            .iter()
                            .filter(|group| plan.remap.iter().any(|(old, _)| *old == group.area))
                            .flat_map(|group| group.tabs.iter().copied())
                            .collect();
                        self.close_tiling_area(area, self.tiling.layout.clone(), plan, panels)?;
                    }
                    tiling_host::TilingAction::MoveTab { panel, area, index } => {
                        let source = self
                            .tiling
                            .area_for(panel)
                            .expect("live tab belongs to an area");
                        self.tiling.move_tab(panel, area, index);
                        if !self
                            .tiling
                            .layout
                            .areas
                            .iter()
                            .any(|area| area.id == source)
                        {
                            self.finish_empty_area_close(source);
                        }
                        if let Some(index) = self.tabs.iter().position(|tab| tab.id == panel) {
                            self.tabs[index].dock_next = true;
                            self.switch_to_tab(index);
                        }
                        self.scene += 1;
                    }
                    tiling_host::TilingAction::SplitTab {
                        panel,
                        area,
                        axis,
                        high,
                    } => {
                        let source = self
                            .tiling
                            .area_for(panel)
                            .expect("live tab belongs to an area");
                        if self
                            .tiling
                            .split_tab(panel, area, axis, high, self.tiling_minimum())
                            .is_some()
                        {
                            if !self
                                .tiling
                                .layout
                                .areas
                                .iter()
                                .any(|area| area.id == source)
                            {
                                self.finish_empty_area_close(source);
                            }
                            if let Some(index) = self.tabs.iter().position(|tab| tab.id == panel) {
                                self.tabs[index].dock_next = true;
                                self.switch_to_tab(index);
                            }
                            self.scene += 1;
                        }
                    }
                    tiling_host::TilingAction::CloseTab(panel) => {
                        if let Some(index) = self.tabs.iter().position(|tab| tab.id == panel) {
                            self.close_tab(index)?;
                        }
                    }
                    tiling_host::TilingAction::CloseArea(area) => {
                        self.close_empty_area(area);
                    }
                    tiling_host::TilingAction::AddPanel { area, kind } => {
                        let viewer = self
                            .modules
                            .registry
                            .viewers
                            .iter()
                            .find(|viewer| viewer.panel_type == kind)
                            .cloned();
                        if let Some(viewer) = viewer {
                            let bed_workbench_api::ViewerBacking::Document { kind, .. } =
                                viewer.backing
                            else {
                                return Err(io::Error::other("Open a file to use this viewer"));
                            };
                            if self.project_root.is_empty() {
                                self.session.configure(self.session_options(None))?;
                            }
                            let document = self.session.create_document_with_kind(b"", kind)?;
                            if let Err(error) = self.add_document_panel_at(
                                document,
                                Some(viewer.id),
                                &Value::Null,
                                Some(area),
                            ) {
                                self.session
                                    .close_document(document, ClosePolicy::Discard)?;
                                return Err(error);
                            }
                        } else {
                            self.open_plugin_panel_in_area(&kind, None, &Value::Null, None, area)?;
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        Ok(())
    }

    /// Saved/native window state cannot change model membership. Return any
    /// stale floating window to its assigned flat dock without importing tabs.
    pub(super) fn sync_tiling_tabs(&mut self, _ui: &Ui) {
        if self.pending_tiling.is_some() {
            return;
        }
        for tab in &mut self.tabs {
            let area = self
                .tiling
                .area_for(tab.id)
                .expect("panel belongs to an area");
            let name = CString::new(format!("###bed_tab_{}", tab.id)).unwrap();
            let window = unsafe { sys::igFindWindowByName(name.as_ptr()).as_ref() };
            if window.is_some_and(|window| window.DockId != self.tiling_ui.docks[&area]) {
                tab.dock_next = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn files_opened_during_and_after_render_share_the_pending_layout() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let first = directory.write("first.rs", b"first");
        let second = directory.write("second.rs", b"second");
        let mut workbench = super::super::tests::workspace(&directory);
        workbench.dock_built = true;
        workbench.rendering_panels = true;
        let destination = workbench.split_area(1, 0, EXTENT / 2).unwrap();
        workbench.open_file_in_area(&first, destination).unwrap();
        workbench.rendering_panels = false;
        workbench.open_file_in_area(&second, destination).unwrap();
        workbench.prepare_tiling().unwrap();
        for path in [&first, &second] {
            let document = workbench.session.document_for_path(path).unwrap();
            let panel = workbench
                .tabs
                .iter()
                .find(|tab| tab.panel.document() == Some(document))
                .unwrap();
            assert_eq!(workbench.area_for_panel(panel.id), Some(destination));
        }
        assert_eq!(workbench.tiling.layout.areas.len(), 2);
        workbench.cleanup().unwrap();
    }
}
