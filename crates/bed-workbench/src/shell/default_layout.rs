//! Reusable arrangements contain panel identities, never resource/session state.
use super::*;
use bed_workbench_api::PanelInput;

impl Workbench {
    pub fn save_default_layout(&mut self) -> io::Result<()> {
        if !self.dock_built {
            self.default_tiling_layout();
        }
        let panels = self
            .tabs
            .iter()
            .map(|tab| json!({"id":tab.id,"panel_type":tab.panel.kind,"viewer":tab.panel.viewer}))
            .collect::<Vec<_>>();
        let value = json!({
            "version":1,
            "panels":panels,
            "tiling":self.current_tiling().to_value(self.tabs.iter().map(|tab| tab.id)),
            "focused":self.focused,
        });
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("Layout storage is unavailable"))?
            .set_default_layout(value)
    }

    pub fn reset_default_layout(&mut self) -> io::Result<()> {
        self.store
            .as_mut()
            .ok_or_else(|| io::Error::other("Layout storage is unavailable"))?
            .reset_default_layout()
    }

    /// Compose a fresh layout after its working directory is selected.
    /// Applying a default never discards work from an existing window.
    pub fn apply_default_layout(&mut self) -> io::Result<()> {
        if self
            .tabs
            .iter()
            .any(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
        {
            return Err(io::Error::other(
                "Default layouts can only initialize an empty window",
            ));
        }
        self.compose_default_layout(None)
    }

    /// Initialize a new workspace while retaining live work through attachment.
    pub(super) fn compose_default_layout(&mut self, mut terminal: Option<u64>) -> io::Result<()> {
        let saved = self
            .store
            .as_ref()
            .and_then(WorkspaceStore::default_layout)
            .cloned();
        let prepared = saved.as_ref().map(validate_default_layout).transpose();
        self.close_panels_of_type(bed_module_projects::PANEL_ID)?;
        let carried = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let carried_focus = self.focused;
        let carried_active = self.active;
        let carried_document = self.last_document;
        let next_tab = self.next_tab;
        self.reset_workspace_layout();
        // Panel identities belong to the live window, including carried panels.
        self.next_tab = next_tab;
        let (panels, mut tiling, focus) = match prepared {
            Ok(Some(value)) => value,
            Ok(None) if self.workspace_spec.is_some() => {
                // Fresh workspaces reserve an empty editor above the tool tabs,
                // with Files and Bed sharing the narrower left column.
                let mut layout = crate::workspace::tiling::Layout::default();
                let editor = layout.split(1, 0, 1754, 1).unwrap();
                let bottom = layout.split(editor, 1, 7255, 1).unwrap();
                // Bed splits the Files column at its own height. Aligned borders
                // form one connected divider in the tiling model.
                let bed = layout.split(1, 1, 7500, 1).unwrap();
                let mut tiling = TilingState::new(layout);
                let mut panels = Vec::new();
                for (kind, area) in [
                    (bed_module_explorer::PANEL_ID, 1),
                    (bed_module_terminal::PANEL_ID, bottom),
                    (bed_module_editor::DIAGNOSTICS_PANEL_TYPE, bottom),
                    (bed_module_debug::PANEL_ID, bottom),
                    ("bed.mascot.panel", bed),
                ] {
                    let id = panels.len() as u64 + 1;
                    panels.push((id, kind.to_owned(), None));
                    tiling.assign(id, area);
                }
                (panels, tiling, None)
            }
            Ok(None) => {
                self.show_tool(Tool::Projects);
                self.default_tiling_layout();
                return Ok(());
            }
            Err(error) => {
                self.show_tool(Tool::Projects);
                self.default_tiling_layout();
                self.error = Some(format!(
                    "Default layout was invalid ({error}); opened Startup"
                ));
                return Ok(());
            }
        };
        let mut ids = HashMap::new();
        let mut errors = Vec::new();
        for panel in panels {
            let viewer = if let Some(viewer) = panel.2 {
                let Some(descriptor) = self
                    .modules
                    .registry
                    .viewer(&viewer)
                    .filter(|descriptor| descriptor.panel_type == panel.1)
                else {
                    errors.push(format!("{}: viewer {viewer} is unavailable", panel.1));
                    continue;
                };
                Some(descriptor.id.to_owned())
            } else {
                self.modules
                    .registry
                    .viewers
                    .iter()
                    .find(|viewer| viewer.panel_type == panel.1)
                    .map(|viewer| viewer.id.to_owned())
            };
            let existing = if panel.1 == bed_module_terminal::PANEL_ID {
                terminal.take()
            } else {
                self.modules
                    .registry
                    .panel(&panel.1)
                    .filter(|descriptor| descriptor.singleton)
                    .and_then(|_| self.tabs.iter().find(|tab| tab.panel.kind == panel.1))
                    .map(|tab| tab.id)
            };
            let result = match existing {
                Some(id) => Ok(id),
                None => self.open_plugin_panel_at(
                    &panel.1,
                    PanelInput::None,
                    &Value::Null,
                    viewer,
                    None,
                ),
            };
            match result {
                Ok(id) if !ids.values().any(|value| *value == id) => {
                    ids.insert(panel.0, id);
                }
                Ok(_) => errors.push(format!("{}: duplicate singleton panel", panel.1)),
                Err(error) => errors.push(format!("{}: {error}", panel.1)),
            }
        }
        for group in &mut tiling.areas {
            group.tabs = group
                .tabs
                .iter()
                .filter_map(|id| ids.get(id).copied())
                .collect();
            group.selected = group
                .selected
                .and_then(|id| ids.get(&id).copied())
                .or_else(|| group.tabs.first().copied());
        }
        let destination = tiling
            .layout
            .areas
            .iter()
            .max_by_key(|area| {
                i64::from(area.rect.max[0] - area.rect.min[0])
                    * i64::from(area.rect.max[1] - area.rect.min[1])
            })
            .expect("default layout has at least one area")
            .id;
        for id in carried {
            if !ids.values().any(|mapped| *mapped == id) {
                tiling.assign(id, destination);
            }
        }
        if self.rendering_panels {
            self.pending_tiling = Some(tiling);
        } else {
            self.tiling = tiling;
            self.pending_tiling = None;
        }
        self.dock_built = true;
        self.reset_tiling_docks();
        // Keep the user's live focus on attachment; fresh windows use template focus.
        self.clear_focus_requests();
        self.focused = None;
        self.active = carried_active;
        self.last_document = carried_document;
        self.focus_history = [None; 2];
        if let Some(id) = carried_focus.or_else(|| focus.and_then(|id| ids.get(&id).copied()))
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == id)
        {
            self.switch_to_tab(index);
        }
        if !errors.is_empty() {
            self.error = Some(format!(
                "Some default panels could not open: {}",
                errors.join("; ")
            ));
        }
        self.scene += 1;
        Ok(())
    }

    pub(super) fn panel_input_empty(&self, panel: &Panel) -> bool {
        panel.instance.is_input_empty(&self.modules.frame.context())
            && panel.document().is_none_or(|doc| {
                self.session.view_ids(doc).len() <= 1
                    && self
                        .tabs
                        .iter()
                        .filter(|tab| tab.panel.document() == Some(doc))
                        .count()
                        <= 1
            })
    }
}

type SavedPanel = (u64, String, Option<String>);
fn validate_default_layout(
    value: &Value,
) -> Result<(Vec<SavedPanel>, TilingState, Option<u64>), &'static str> {
    if value["version"].as_u64() != Some(1) {
        return Err("Unsupported default layout version");
    }
    let values = value["panels"]
        .as_array()
        .filter(|panels| panels.len() <= 4096)
        .ok_or("Invalid default panels")?;
    let mut ids = HashSet::new();
    let mut panels = Vec::new();
    for panel in values {
        let id = panel["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u64::from(u32::MAX) && ids.insert(*id))
            .ok_or("Duplicate or invalid default panel ID")?;
        let kind = panel["panel_type"]
            .as_str()
            .filter(|kind| !kind.is_empty())
            .ok_or("Missing default panel type")?;
        let viewer = match &panel["viewer"] {
            Value::Null => None,
            Value::String(value) => Some(value.clone()),
            _ => return Err("Invalid default viewer"),
        };
        panels.push((id, kind.to_owned(), viewer));
    }
    let focus = match &value["focused"] {
        Value::Null => None,
        value => Some(
            value
                .as_u64()
                .filter(|id| ids.contains(id))
                .ok_or("Invalid default focus")?,
        ),
    };
    let tiling = TilingState::from_value(&value["tiling"], ids)?;
    Ok((panels, tiling, focus))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{shell::tests::workspace, test_support::TempDir, workspace::tiling::Layout};

    #[test]
    fn terminal_upgrade_reuses_default_terminal_slot_and_restores_its_own_layout() {
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"live document");
        let mut template = workspace(&dir);
        template
            .close_panels_of_type(bed_module_projects::PANEL_ID)
            .unwrap();
        let color = template
            .open_plugin_panel(
                bed_module_color::PANEL_ID,
                PanelInput::None,
                &Value::Null,
                None,
            )
            .unwrap();
        template.dispatch(WindowCommand::NewTerminal).unwrap();
        let first = template.active_panel_id().unwrap();
        template.dispatch(WindowCommand::NewTerminal).unwrap();
        let second = template.active_panel_id().unwrap();
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 2500, 1).unwrap();
        let bottom = layout.split(right, 1, 6500, 1).unwrap();
        template.tiling = TilingState::new(layout.clone());
        for (id, area) in [(color, 1), (first, right), (second, bottom)] {
            template.tiling.assign(id, area);
        }
        template.dock_built = true;
        template.save_default_layout().unwrap();
        template.cleanup().unwrap();

        let mut live = workspace(&dir);
        let color = live
            .open_plugin_panel(
                bed_module_color::PANEL_ID,
                PanelInput::None,
                &Value::Null,
                None,
            )
            .unwrap();
        live.dispatch(WindowCommand::NewTerminal).unwrap();
        live.terminal.set_visible(true, false).unwrap();
        let requested = live.terminal.active_session_id().unwrap();
        let requested_panel = live.terminal_panel_id(requested).unwrap();
        let pid = live.terminal.process_id(requested).unwrap();
        live.dispatch(WindowCommand::NewTerminal).unwrap();
        let other = live.terminal.active_session_id().unwrap();
        let other_panel = live.terminal_panel_id(other).unwrap();
        live.open_or_focus(&file).unwrap();
        let document = live.active_panel_id().unwrap();
        let other_index = live
            .tabs
            .iter()
            .position(|tab| tab.id == other_panel)
            .unwrap();
        live.switch_to_tab(other_index);
        live.attach_workspace_with_terminal(&dir.path("project"), Some(requested))
            .unwrap();
        assert_eq!(live.terminal.process_id(requested), Some(pid));
        assert_eq!(live.terminal.session_count(), 2);
        assert!(live.terminal.process_id(other).is_none());
        assert_eq!(live.panel_count(bed_module_color::PANEL_ID), 1);
        assert!(
            live.tabs
                .iter()
                .all(|tab| ![color, other_panel, document].contains(&tab.id))
        );
        let restored_color = live
            .tabs
            .iter()
            .find(|tab| tab.panel.kind == bed_module_color::PANEL_ID)
            .unwrap()
            .id;
        assert_eq!(live.area_for_panel(restored_color), Some(1));
        assert_eq!(live.area_for_panel(requested_panel), Some(right));
        assert_eq!(live.active_panel_id(), Some(requested_panel));
        assert!(live.session.document_ids().is_empty());
        let restored_terminal = live
            .tabs
            .iter()
            .find(|tab| {
                tab.panel.kind == bed_module_terminal::PANEL_ID && tab.id != requested_panel
            })
            .unwrap()
            .id;
        assert_eq!(live.area_for_panel(restored_terminal), Some(bottom));
        assert_eq!(live.tiling.layout, layout);
        assert_eq!(
            live.tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<HashSet<_>>()
                .len(),
            live.tabs.len()
        );
        live.reset_default_layout().unwrap();
        live.cleanup().unwrap();
        let mut reopened = workspace(&dir);
        reopened
            .open_startup_workspace(&dir.path("project"))
            .unwrap();
        assert_eq!(reopened.tiling.layout, layout);
        assert_eq!(reopened.terminal.session_count(), 2);
        assert!(reopened.session.document_ids().is_empty());
        reopened.cleanup().unwrap();
    }

    #[test]
    fn attachment_keeps_live_terminal_when_default_has_no_terminal_slot() {
        let dir = TempDir::new();
        dir.write("project/file.txt", b"");
        for empty in [false, true] {
            let mut template = workspace(&dir);
            template
                .close_panels_of_type(bed_module_projects::PANEL_ID)
                .unwrap();
            if !empty {
                template.dispatch(WindowCommand::NewSettings).unwrap();
            }
            template.save_default_layout().unwrap();
            template.cleanup().unwrap();
            let mut live = workspace(&dir);
            live.dispatch(WindowCommand::NewTerminal).unwrap();
            live.terminal.set_visible(true, false).unwrap();
            let terminal = live.terminal.active_session_id().unwrap();
            let panel = live.terminal_panel_id(terminal).unwrap();
            let pid = live.terminal.process_id(terminal).unwrap();
            let root = dir.path(if empty { "empty-project" } else { "project" });
            std::fs::create_dir_all(&root).unwrap();
            live.attach_workspace_with_terminal(&root, Some(terminal))
                .unwrap();
            assert_eq!(live.terminal.process_id(terminal), Some(pid));
            assert_eq!(live.terminal.session_count(), 1);
            assert_eq!(live.area_for_panel(panel), Some(1));
            assert_eq!(live.active_panel_id(), Some(panel));
            assert_eq!(live.tabs.len(), if empty { 1 } else { 2 });
            live.cleanup().unwrap();
        }
    }

    #[test]
    fn fresh_workspace_layout_matches_the_four_panes_and_survives_reopening() {
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"new workspace document");
        let root = dir.path("project");
        let mut workbench = workspace(&dir);
        workbench.set_project(&root).unwrap();
        assert!(workbench.error.is_none(), "{:?}", workbench.error);
        assert_eq!(workbench.tiling.areas.len(), 4);
        assert!(workbench.session.document_ids().is_empty());
        assert_eq!(workbench.terminal.session_count(), 1);
        for (min, max, kinds) in [
            ([0, 0], [1754, 7500], vec![bed_module_explorer::PANEL_ID]),
            ([1754, 0], [10000, 7255], vec![]),
            (
                [1754, 7255],
                [10000, 10000],
                vec![
                    bed_module_terminal::PANEL_ID,
                    bed_module_editor::DIAGNOSTICS_PANEL_TYPE,
                    bed_module_debug::PANEL_ID,
                ],
            ),
            ([0, 7500], [1754, 10000], vec!["bed.mascot.panel"]),
        ] {
            let area = workbench
                .tiling
                .layout
                .areas
                .iter()
                .find(|area| area.rect.min == min && area.rect.max == max)
                .unwrap();
            let group = workbench
                .tiling
                .areas
                .iter()
                .find(|group| group.area == area.id)
                .unwrap();
            let actual = group
                .tabs
                .iter()
                .map(|id| {
                    workbench
                        .tabs
                        .iter()
                        .find(|tab| tab.id == *id)
                        .unwrap()
                        .panel
                        .kind
                        .as_str()
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, kinds);
            assert_eq!(group.selected, group.tabs.first().copied());
        }
        workbench.open_or_focus(&file).unwrap();
        let editor = workbench
            .area_for_panel(workbench.active_panel_id().unwrap())
            .unwrap();
        assert_eq!(workbench.tiling.layout.area(editor).rect.min, [1754, 0]);
        assert_eq!(workbench.tiling.layout.area(editor).rect.max, [10000, 7255]);
        let expected = workbench.tiling.clone();
        workbench.cleanup().unwrap();
        let mut reopened = workspace(&dir);
        reopened.open_startup_workspace(&root).unwrap();
        assert_eq!(reopened.tiling, expected);
        assert_eq!(
            reopened.active_snapshot().unwrap().bytes,
            b"new workspace document"
        );
        reopened.cleanup().unwrap();
    }

    #[test]
    fn fresh_workspace_terminal_and_bed_dividers_resize_independently() {
        let dir = TempDir::new();
        dir.write("project/file.txt", b"");
        let mut workbench = workspace(&dir);
        workbench.set_project(&dir.path("project")).unwrap();
        let original = workbench.tiling.layout.clone();
        let files = original
            .areas
            .iter()
            .find(|area| area.rect.min == [0, 0])
            .unwrap()
            .id;
        let bed = original
            .areas
            .iter()
            .find(|area| area.rect.min == [0, 7500])
            .unwrap()
            .id;
        let terminal = original
            .areas
            .iter()
            .find(|area| area.rect.min == [1754, 7255])
            .unwrap()
            .id;
        let mut layout = original.clone();
        layout.move_border(1, 7255, [1754, 10000], 6000, 1);
        assert_eq!(layout.area(files), original.area(files));
        assert_eq!(layout.area(bed), original.area(bed));
        assert_eq!(layout.area(terminal).rect.min[1], 6000);
        let mut layout = original.clone();
        layout.move_border(1, 7500, [0, 1754], 6500, 1);
        assert_eq!(layout.area(terminal), original.area(terminal));
        assert_eq!(layout.area(files).rect.max[1], 6500);
        assert_eq!(layout.area(bed).rect.min[1], 6500);
        workbench.cleanup().unwrap();
    }

    #[test]
    fn fresh_workspaces_prefer_a_saved_default_until_it_is_reset() {
        let dir = TempDir::new();
        dir.write("first/file.txt", b"");
        dir.write("second/file.txt", b"");
        let mut original = workspace(&dir);
        original
            .close_panels_of_type(bed_module_projects::PANEL_ID)
            .unwrap();
        original.dispatch(WindowCommand::NewSettings).unwrap();
        original.save_default_layout().unwrap();
        let mut custom = workspace(&dir);
        custom.set_project(&dir.path("first")).unwrap();
        assert_eq!(custom.tabs.len(), 1);
        assert_eq!(custom.tabs[0].panel.kind, original.tabs[0].panel.kind);
        assert_eq!(custom.tiling.layout.areas.len(), 1);
        custom.reset_default_layout().unwrap();
        let mut reset = workspace(&dir);
        reset.set_project(&dir.path("second")).unwrap();
        assert_eq!(reset.tiling.layout.areas.len(), 4);
        assert_eq!(reset.panel_count("bed.mascot.panel"), 1);
        original.cleanup().unwrap();
        custom.cleanup().unwrap();
        reset.cleanup().unwrap();
    }

    #[test]
    fn a_default_without_focus_keeps_saved_selection_and_cancels_factory_focus() {
        let dir = TempDir::new();
        let mut original = workspace(&dir);
        original.dispatch(WindowCommand::NewDocument).unwrap();
        original.dispatch(WindowCommand::NewDocument).unwrap();
        original.default_tiling_layout();
        let selected = original.tabs[0].id;
        original.tiling.areas[0].selected = Some(selected);
        original.focused = None;
        original.save_default_layout().unwrap();
        let mut restored = workspace(&dir);
        restored.apply_default_layout().unwrap();
        assert_eq!(restored.focused, None);
        assert_eq!(restored.active, None);
        assert!(restored.tabs.iter().all(|tab| !tab.focus));
        assert_eq!(restored.tiling.areas[0].selected, Some(restored.tabs[0].id));
        original.cleanup().unwrap();
        restored.cleanup().unwrap();
    }

    #[test]
    fn default_round_trip_keeps_panels_and_empty_geometry_without_session_content() {
        let dir = TempDir::new();
        let path = dir.write("document.txt", b"private session contents");
        let mut original = workspace(&dir);
        original.open_or_focus(&path).unwrap();
        original.dispatch(WindowCommand::NewSettings).unwrap();
        let mut layout = Layout::default();
        let right = layout.split(1, 0, 6000, 100).unwrap();
        layout.split(right, 1, 5000, 100).unwrap();
        original.tiling = TilingState::new(layout.clone());
        for tab in &original.tabs {
            original.tiling.assign(
                tab.id,
                if tab.panel.document().is_some() {
                    right
                } else {
                    1
                },
            );
        }
        original.dock_built = true;
        original.save_default_layout().unwrap();
        let saved = original
            .store
            .as_ref()
            .unwrap()
            .default_layout()
            .unwrap()
            .clone();
        let text = saved.to_string();
        assert!(!text.contains("private session"));
        assert!(!text.contains("document.txt"));
        assert!(
            saved["panels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|panel| panel.get("state").is_none())
        );
        assert_eq!(
            original.active_snapshot().unwrap().bytes,
            b"private session contents"
        );

        let mut restored = workspace(&dir);
        restored.apply_default_layout().unwrap();
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert_eq!(restored.tiling.layout, layout);
        assert_eq!(restored.tabs.len(), original.tabs.len());
        assert!(
            restored
                .tiling
                .areas
                .iter()
                .any(|group| group.tabs.is_empty())
        );
        let document = restored.session.document_ids()[0];
        let snapshot = restored.session.snapshot(document).unwrap();
        assert!(snapshot.path.is_empty() && snapshot.bytes.is_empty() && !snapshot.dirty);
        let id = restored
            .tabs
            .iter()
            .find(|tab| tab.panel.document() == Some(document))
            .unwrap()
            .id;
        restored.open_or_focus(&path).unwrap();
        assert_eq!(restored.tabs.len(), original.tabs.len());
        assert_eq!(restored.active_panel_id(), Some(id));
        assert_eq!(restored.area_for_panel(id), Some(right));
        assert!(!restored.session.document_ids().contains(&document));
    }

    #[test]
    fn opening_files_preserves_used_buffers_and_explicit_duplicates() {
        let dir = TempDir::new();
        let path = dir.write("one.txt", b"one");
        let mut workbench = workspace(&dir);
        workbench.dispatch(WindowCommand::NewDocument).unwrap();
        let id = workbench.active_panel_id().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"keep me"))
            .unwrap();
        workbench.open_or_focus(&path).unwrap();
        assert_ne!(workbench.active_panel_id(), Some(id));
        assert_eq!(
            workbench
                .session
                .snapshot(workbench.session.document_for_view(view).unwrap())
                .unwrap()
                .bytes,
            b"keep me"
        );
        let before = workbench.tabs.len();
        workbench.dispatch(WindowCommand::DuplicateView).unwrap();
        assert_eq!(workbench.tabs.len(), before + 1);
    }

    #[test]
    fn workspace_persistence_retains_pristine_default_editors() {
        let dir = TempDir::new();
        let mut original = workspace(&dir);
        original.set_project(dir.root()).unwrap();
        original.dispatch(WindowCommand::NewDocument).unwrap();
        original.persist_workspace().unwrap();
        assert!(
            original.last_state.as_ref().unwrap()["panels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|panel| panel["default_input"] == true)
        );
        let mut restored = workspace(&dir);
        restored
            .set_workspace(original.workspace_spec.clone().unwrap())
            .unwrap();
        assert_eq!(restored.session.document_ids().len(), 1);
        let snapshot = restored
            .session
            .snapshot(restored.session.document_ids()[0])
            .unwrap();
        assert!(snapshot.bytes.is_empty() && snapshot.path.is_empty());
    }

    #[test]
    fn missing_invalid_and_reset_defaults_open_projects_but_valid_empty_stays_empty() {
        let dir = TempDir::new();
        let mut workbench = workspace(&dir);
        workbench.apply_default_layout().unwrap();
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 1);
        workbench
            .store
            .as_mut()
            .unwrap()
            .set_default_layout(json!({"version":999}))
            .unwrap();
        workbench.apply_default_layout().unwrap();
        assert!(workbench.error.as_ref().unwrap().contains("invalid"));
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 1);
        workbench
            .close_panels_of_type(bed_module_projects::PANEL_ID)
            .unwrap();
        workbench.save_default_layout().unwrap();
        let mut empty = workspace(&dir);
        empty.apply_default_layout().unwrap();
        assert!(empty.tabs.is_empty());
        empty.reset_default_layout().unwrap();
        assert!(empty.tabs.is_empty());
        let mut reset = workspace(&dir);
        reset.apply_default_layout().unwrap();
        assert_eq!(reset.panel_count(bed_module_projects::PANEL_ID), 1);
    }

    #[test]
    fn independent_store_writers_preserve_default_and_workspace_layouts() {
        let dir = TempDir::new();
        let mut first = WorkspaceStore::load(&dir.path("config")).unwrap();
        let mut second = WorkspaceStore::load(&dir.path("config")).unwrap();
        let spec = WorkspaceSpec::local(dir.root().to_str().unwrap());
        first.set_default_layout(json!({"version":1})).unwrap();
        second
            .set_layout(&spec, json!({"workspace":"keep"}))
            .unwrap();
        first.reset_default_layout().unwrap();
        let reloaded = WorkspaceStore::load(&dir.path("config")).unwrap();
        assert!(reloaded.default_layout().is_none());
        assert_eq!(reloaded.layout(&spec).unwrap()["workspace"], "keep");
    }
}
