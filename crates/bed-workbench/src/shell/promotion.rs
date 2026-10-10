//! Attach project services and a saved layout without replacing live work.
use super::*;

pub(super) struct WorkspaceGraft {
    carried: Vec<u64>,
    restored: Vec<u64>,
    focused: Option<u64>,
}
impl WorkspaceGraft {
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            carried: value
                .get("carried")?
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .collect(),
            restored: value
                .get("restored")?
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .collect(),
            focused: value["focused"].as_u64(),
        })
    }
    pub(super) fn to_value(&self) -> Value {
        json!({"carried":self.carried,"restored":self.restored,"focused":self.focused})
    }
}

impl Workbench {
    /// Attach a local workspace to this rootless window. Existing documents,
    /// panel instances and terminal processes stay alive. A saved workspace's
    /// layout is restored, with live panels added to its largest main dock.
    pub fn attach_workspace(&mut self, root: &Path) -> io::Result<bool> {
        self.attach_workspace_with_terminal(root, self.terminal.active_session_id())
    }

    pub(super) fn attach_workspace_with_terminal(
        &mut self,
        root: &Path,
        terminal: Option<u64>,
    ) -> io::Result<bool> {
        if self.workspace_spec.is_some() {
            return Ok(false);
        }
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Project must be a directory",
            ));
        }
        let path = root
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Project path is not UTF-8")
            })?
            .to_owned();
        if self.context_binding.as_ref().is_some_and(|binding| {
            binding.with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).WithinFrameScope })
        }) {
            self.pending_attach = Some((root, terminal));
            return Ok(true);
        }
        // Another window may have saved this workspace since we were opened.
        self.store = Some(WorkspaceStore::load(&self.settings.config_dir)?);
        let mut spec = self
            .store
            .as_ref()
            .and_then(|store| store.stored_spec(&WorkspaceSpec::local(&path)))
            .unwrap_or_else(|| WorkspaceSpec::local(&path));
        spec.root = path.clone();
        let saved = self
            .store
            .as_ref()
            .and_then(|store| store.layout(&spec))
            .cloned();
        self.persist_workspace()?;
        if let Some(store) = &mut self.store {
            spec = store.record_workspace(spec)?;
        }
        self.session
            .configure(self.session_options(Some(root.clone())))?;
        self.workspace_spec = Some(spec.clone());
        self.project_root = path.clone();
        self.directory = root;
        self.restore_module_settings();
        self.service_settings = None;
        self.sync_services()?;
        {
            let mut explorer = self.file_explorer();
            explorer.project_root = path.clone();
            explorer.file_tree = bed_module_explorer::file_tree::FileTree {
                root_node: bed_module_explorer::file_tree::FileNode {
                    name: Path::new(&path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    full_path: path.clone(),
                    is_directory: true,
                    is_open: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            explorer.file_finder.set_remote_client(None);
            explorer.file_finder.set_project_dir(&path);
        }
        self.restore_tree_preferences(&spec);
        self.terminal.set_project_root(&path);
        self.close_panels_of_type(bed_module_projects::PANEL_ID)?;
        let mut carried = self.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let mut focused = self.focused;
        if let Some(saved) = saved {
            let saved = self.prepare_attached_layout(saved)?;
            let restored = saved["panels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|panel| panel["id"].as_u64())
                .collect();
            self.reset_attached_docking();
            self.restore_workspace(&saved)?;
            if let Some(previous) = self.pending_graft.take() {
                carried.extend(previous.carried);
                carried.sort_unstable();
                carried.dedup();
                focused = focused.or(previous.focused);
            }
            self.pending_graft = Some(WorkspaceGraft {
                carried,
                restored,
                focused,
            });
            self.finish_workspace_graft();
        } else {
            self.compose_default_layout(terminal.and_then(|id| self.terminal_panel_id(id)))?;
        }
        self.finish_workspace_open()?;
        self.last_state = None;
        self.scene += 1;
        if let Err(error) = self.persist_workspace() {
            self.error = Some(format!(
                "Workspace attached, but its layout could not be saved: {error}"
            ));
        }
        Ok(true)
    }

    pub fn terminal_panel_id(&self, session: u64) -> Option<u64> {
        self.tabs
            .iter()
            .find(|tab| tab.panel.terminal_id() == Some(session))
            .map(|tab| tab.id)
    }

    pub(super) fn close_panels_of_type(&mut self, kind: &str) -> io::Result<()> {
        let indices = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (tab.panel.kind == kind).then_some(index))
            .collect();
        self.close_tabs(indices)?;
        Ok(())
    }

    /// Successful local and remote opens dismiss the launcher, including any
    /// accidentally saved by older sessions, while retaining other live panels.
    pub(super) fn finish_workspace_open(&mut self) -> io::Result<()> {
        self.close_panels_of_type(bed_module_projects::PANEL_ID)?;
        if self.tabs.is_empty() && self.remote_ui.restore.is_empty() {
            self.show_tool(Tool::Explorer);
        }
        self.refresh_projects();
        Ok(())
    }

    fn saved_panel_type<'a>(&'a self, panel: &'a Value) -> Option<&'a str> {
        panel["panel_type"]
            .as_str()
            .filter(|kind| self.modules.registry.panel(kind).is_some())
            .or_else(|| {
                let kind = panel["kind"].as_str()?;
                self.modules
                    .registry
                    .panels
                    .iter()
                    .find(|descriptor| descriptor.legacy_kind == Some(kind))
                    .map(|descriptor| descriptor.id)
            })
    }

    fn prepare_attached_layout(&mut self, mut state: Value) -> io::Result<Value> {
        let mut seen = HashSet::new();
        let valid = state["panels"].as_array().is_some_and(|panels| {
            panels.iter().all(|panel| {
                panel["id"]
                    .as_u64()
                    .is_some_and(|id| id > 0 && id <= u32::MAX as u64 && seen.insert(id))
            })
        });
        if !valid {
            if let Some(panels) = state["panels"].as_array_mut() {
                panels.retain(Value::is_object);
                for (index, panel) in panels.iter_mut().enumerate() {
                    panel["id"] = json!(index + 1);
                }
            }
            state["ini"] = Value::Null;
            state["tiling"] = Value::Null;
            state["focused"] = Value::Null;
            state["active_document_panel"] = Value::Null;
        }
        let saved_ids = state["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|panel| panel["id"].as_u64())
            .collect::<Vec<_>>();
        let saved_tiling = if !state["tiling"].is_null() {
            Some(crate::workspace::tiling_state::TilingState::from_value(
                &state["tiling"],
                saved_ids.iter().copied(),
            ))
        } else {
            state["ini"]
                .as_str()
                .filter(|ini| !ini.is_empty())
                .map(|ini| {
                    crate::workspace::tiling_state::TilingState::from_legacy_ini(
                        ini,
                        saved_ids.iter().copied(),
                    )
                })
        };
        let mut saved_tiling = match saved_tiling {
            Some(Ok(tiling)) => Some(tiling),
            Some(Err(error)) => {
                self.error = Some(format!(
                    "Workspace layout was invalid ({error}); restored panels with a default layout"
                ));
                None
            }
            None => None,
        };
        let mut singletons = HashSet::new();
        if let Some(panels) = state["panels"].as_array_mut() {
            panels.retain(|panel| {
                self.saved_panel_type(panel)
                    .and_then(|kind| self.modules.registry.panel(kind))
                    .is_none_or(|descriptor| {
                        !descriptor.singleton || singletons.insert(descriptor.id)
                    })
            });
        }
        let mut next = self
            .next_tab
            .max(self.tabs.iter().map(|tab| tab.id + 1).max().unwrap_or(1))
            .max(
                state["panels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|panel| panel["id"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1,
            );
        let mut used = self.tabs.iter().map(|tab| tab.id).collect::<HashSet<_>>();
        let mut ids = HashMap::new();
        for panel in state["panels"].as_array().into_iter().flatten() {
            let old = panel["id"].as_u64().unwrap();
            let existing = self
                .saved_panel_type(panel)
                .and_then(|kind| self.modules.registry.panel(kind))
                .filter(|descriptor| descriptor.singleton)
                .and_then(|descriptor| self.tabs.iter().find(|tab| tab.panel.kind == descriptor.id))
                .map(|tab| tab.id);
            let id = if let Some(id) = existing {
                id
            } else if used.insert(old) {
                old
            } else {
                if next > u32::MAX as u64 {
                    return Err(io::Error::other("Too many workspace panels"));
                }
                let id = next;
                next += 1;
                used.insert(id);
                id
            };
            ids.insert(old, id);
        }
        state["ini"] = Value::Null;
        state["tiling"] = Value::Null;
        if let Some(tiling) = &mut saved_tiling {
            tiling.remap_panel_ids(&ids);
            state["tiling"] = tiling.to_value(ids.values().copied());
        }
        for panel in state["panels"].as_array_mut().into_iter().flatten() {
            panel["id"] = json!(ids[&panel["id"].as_u64().unwrap()]);
        }
        for key in ["focused", "active_document_panel"] {
            state[key] = state[key]
                .as_u64()
                .and_then(|id| ids.get(&id))
                .map_or(Value::Null, |id| json!(id));
        }
        if let Some(mut graft) = WorkspaceGraft::from_value(&state["graft"]) {
            graft.carried = graft
                .carried
                .iter()
                .filter_map(|id| ids.get(id).copied())
                .collect();
            graft.restored = graft
                .restored
                .iter()
                .filter_map(|id| ids.get(id).copied())
                .collect();
            graft.focused = graft.focused.and_then(|id| ids.get(&id).copied());
            state["graft"] = graft.to_value();
        }
        self.next_tab = next;
        Ok(state)
    }

    fn reset_attached_docking(&mut self) {
        self.pending_ini = None;
        self.pending_tiling = None;
        self.dock_built = false;
        self.reset_tiling_docks();
    }

    pub(super) fn finish_workspace_graft(&mut self) {
        if self.pending_ini.is_some() {
            return;
        }
        let Some(graft) = self.pending_graft.take() else {
            return;
        };
        let destination = self
            .current_tiling()
            .layout
            .areas
            .iter()
            .max_by_key(|area| {
                i64::from(area.rect.max[0] - area.rect.min[0])
                    * i64::from(area.rect.max[1] - area.rect.min[1])
            })
            .expect("workspace has at least one area")
            .id;
        // A singleton already restored into the saved layout belongs there;
        // only other carried work joins its largest area.
        let carried = self
            .tabs
            .iter()
            .filter(|tab| graft.carried.contains(&tab.id) && !graft.restored.contains(&tab.id))
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        for panel in carried {
            self.place_panel_in_area(panel, destination);
        }
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == graft.focused)
        {
            self.switch_to_tab(index);
        }
        self.last_state = None;
        self.scene += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        shell::tests::{frame, workspace},
        test_support::TempDir,
    };

    #[test]
    fn opening_files_keeps_the_window_rootless_and_uses_shared_settings() {
        let dir = TempDir::new();
        let file = dir.write("file.txt", b"rootless");
        let mut workbench = workspace(&dir);
        let session = workbench.session.workspace_id();
        let config = workbench.settings.config_dir.clone();
        workbench.open_or_focus(&file).unwrap();
        assert!(workbench.workspace_spec.is_none());
        assert!(workbench.project_root.is_empty());
        assert_eq!(workbench.session.workspace_id(), session);
        assert_eq!(workbench.settings.config_dir, config);
        assert!(workbench.session.options().monitoring);
        assert!(!workbench.session.options().persistent_history);
        let ids = workbench
            .toolbar_commands()
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        assert!(ids.contains(&"bed.terminal.new".into()));
        assert!(ids.contains(&"bed.editor.split_right".into()));
        assert!(ids.contains(&"bed.files.new".into()));
        assert!(ids.contains(&"bed.search.new".into()));
        assert!(ids.contains(&"bed.git.show".into()));
        assert!(!ids.contains(&"bed.debug.show".into()));
        assert!(!ids.contains(&"bed.terminal.promote".into()));
    }

    #[test]
    fn other_local_workspaces_open_separately_without_changing_live_work() {
        let dir = TempDir::new();
        let file = dir.write("first/file.txt", b"live work");
        std::fs::create_dir_all(dir.path("second")).unwrap();
        let mut workbench = workspace(&dir);
        workbench.set_project(&dir.path("first")).unwrap();
        workbench.open_or_focus(&file).unwrap();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let session = workbench.session.workspace_id();
        let view = workbench.active_view();
        let terminals = workbench.terminal.session_ids();
        let panels: Vec<_> = workbench.tabs.iter().map(|tab| tab.id).collect();
        let base = workbench.working_directory();
        let config = workbench.settings.config_dir.clone();
        assert!(!workbench.set_project(&dir.path("first")).unwrap());
        assert!(workbench.take_workspace_windows().is_empty());
        assert!(workbench.set_project(&dir.path("second")).unwrap());
        let second = dir.path("second").canonicalize().unwrap();
        assert_eq!(
            workbench
                .take_workspace_windows()
                .into_iter()
                .map(|spec| PathBuf::from(spec.root))
                .collect::<Vec<_>>(),
            vec![second.clone()]
        );
        assert_eq!(workbench.session.workspace_id(), session);
        assert_eq!(workbench.active_view(), view);
        assert_eq!(workbench.terminal.session_ids(), terminals);
        assert_eq!(
            workbench.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            panels
        );
        assert_eq!(workbench.working_directory(), base);
        let spec = WorkspaceSpec::local(second.to_str().unwrap());
        let mut store = WorkspaceStore::load(&config).unwrap();
        assert!(store.stored_spec(&spec).is_some());
        assert!(
            store.layout(&spec).is_none(),
            "fresh windows use the default layout"
        );
        // The ordinary startup path recognizes a workspace created by a request.
        let mut next = workspace(&dir);
        assert!(next.open_startup_workspace(&second).unwrap());
        assert_eq!(next.working_directory(), second);
        assert!(next.session.document_ids().is_empty());
        next.cleanup().unwrap();
        // Preserve a saved workspace's identity and layout when requesting it.
        let named = store.rename_workspace(&spec, "Second Workspace").unwrap();
        let saved = WorkspaceStore::load(&config)
            .unwrap()
            .layout(&named)
            .unwrap()
            .clone();
        workbench.set_workspace(named.clone()).unwrap();
        assert_eq!(
            workbench
                .take_workspace_windows()
                .into_iter()
                .map(|spec| PathBuf::from(spec.root))
                .collect::<Vec<_>>(),
            vec![second]
        );
        let current = WorkspaceStore::load(&config).unwrap();
        assert_eq!(
            current.stored_spec(&named).unwrap().name,
            "Second Workspace"
        );
        assert_eq!(current.layout(&named), Some(&saved));
        assert!(workbench.set_project(&file).is_err());
        assert!(workbench.set_project(&dir.path("missing")).is_err());
        assert!(workbench.take_workspace_windows().is_empty());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn attachment_reads_a_workspace_saved_after_the_window_opened() {
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"saved from another window");
        let mut live = workspace(&dir);
        let mut other = workspace(&dir);
        other.open_or_focus(&file).unwrap();
        other.set_project(&dir.path("project")).unwrap();
        other.cleanup().unwrap();
        let carried: Vec<_> = live
            .tabs
            .iter()
            .filter(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
            .map(|tab| tab.id)
            .collect();
        live.set_project(&dir.path("project")).unwrap();
        assert!(
            carried
                .iter()
                .all(|id| live.tabs.iter().any(|tab| tab.id == *id))
        );
        assert_eq!(
            live.active_snapshot().unwrap().bytes,
            b"saved from another window"
        );
        assert_eq!(
            live.working_directory(),
            dir.path("project").canonicalize().unwrap()
        );
        live.cleanup().unwrap();
    }

    #[test]
    fn ordinary_split_commands_create_terminals_when_a_terminal_is_focused() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let mut workbench = workspace(&dir);
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let mut context = Context::create();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        frame(&mut context, &mut workbench);
        let original = workbench.terminal.active_session_id().unwrap();
        let pid = workbench.terminal.process_id(original).unwrap();
        // Give this success-path fixture enough height for Split Down. The
        // default bottom area cannot fit two terminal rows at this window size.
        let panel = workbench.terminal_panel_id(original).unwrap();
        let area = workbench.area_for_panel(panel).unwrap();
        let rect = workbench.current_tiling().layout.area(area).rect;
        let minimum = workbench.tiling_ui.minimum[1];
        workbench.edit_tiling().layout.move_border(
            1,
            rect.min[1],
            [rect.min[0], rect.max[0]],
            crate::workspace::tiling::EXTENT / 2,
            minimum,
        );
        let splits = workbench
            .toolbar_commands()
            .into_iter()
            .filter(|item| item.id.starts_with("bed.editor.split_"))
            .collect::<Vec<_>>();
        assert_eq!(splits.len(), 2);
        assert!(splits.iter().all(|item| item.enabled));
        workbench.dispatch(WindowCommand::SplitRight).unwrap();
        workbench.dispatch(WindowCommand::SplitDown).unwrap();
        assert_eq!(workbench.terminal.session_ids().len(), 3);
        assert_eq!(workbench.terminal.process_id(original), Some(pid));
        assert!(workbench.session.document_ids().is_empty());
        assert!(workbench.workspace_spec.is_none());
        workbench.cleanup().unwrap();
    }

    #[test]
    fn attaching_new_workspace_applies_default_and_preserves_dirty_shared_views() {
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"original");
        let mut workbench = workspace(&dir);
        workbench.open_or_focus(&file).unwrap();
        workbench.dispatch(WindowCommand::DuplicateView).unwrap();
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"dirty "))
            .unwrap();
        let session = workbench.session.workspace_id();
        let tabs = workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        assert!(workbench.set_project(&dir.path("project")).unwrap());
        assert_eq!(workbench.session.workspace_id(), session);
        assert_eq!(workbench.active_view(), Some(view));
        assert_eq!(workbench.session.document_ids(), vec![document]);
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert!(
            tabs.iter()
                .all(|id| workbench.tabs.iter().any(|tab| tab.id == *id))
        );
        assert_eq!(workbench.tiling.layout.areas.len(), 4);
        assert_eq!(workbench.panel_count(bed_module_explorer::PANEL_ID), 1);
        assert_eq!(workbench.terminal.session_count(), 1);
        assert_eq!(workbench.panel_count(bed_module_projects::PANEL_ID), 0);
        assert!(workbench.session.options().persistent_history);
        let state = workbench
            .store
            .as_ref()
            .unwrap()
            .layout(workbench.workspace_spec.as_ref().unwrap())
            .unwrap();
        assert_eq!(
            state["panels"].as_array().unwrap().len(),
            workbench.tabs.len()
        );
        assert!(!workbench.attach_workspace(dir.root()).unwrap());
    }

    #[test]
    fn attach_requested_during_a_frame_waits_until_tick_without_closing_documents() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let file = dir.write("project/file.txt", b"keep");
        let mut workbench = workspace(&dir);
        workbench.open_or_focus(&file).unwrap();
        let document = workbench.active_panel_id().unwrap();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let requested = workbench.terminal.active_session_id().unwrap();
        let requested_panel = workbench.terminal_panel_id(requested).unwrap();
        workbench.terminal.set_visible(true, false).unwrap();
        let pid = workbench.terminal.process_id(requested).unwrap();
        workbench.dispatch(WindowCommand::NewTerminal).unwrap();
        let mut context = Context::create();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 800.0],
            1.0 / 60.0,
        ));
        workbench.render(context.frame()).unwrap();
        assert!(
            workbench
                .attach_workspace_with_terminal(&dir.path("project"), Some(requested))
                .unwrap()
        );
        assert!(workbench.workspace_spec.is_none());
        drop(context.render_legacy());
        workbench.tick().unwrap();
        assert!(workbench.workspace_spec.is_some());
        assert_eq!(workbench.terminal.process_id(requested), Some(pid));
        assert_eq!(workbench.terminal.session_count(), 2);
        let area = workbench.area_for_panel(requested_panel).unwrap();
        assert_eq!(workbench.tiling.layout.area(area).rect.min, [1754, 7255]);
        let document = workbench
            .tabs
            .iter()
            .find(|tab| tab.id == document)
            .unwrap()
            .panel
            .document()
            .unwrap();
        assert_eq!(workbench.session.snapshot(document).unwrap().bytes, b"keep");
        workbench.cleanup().unwrap();
    }

    #[test]
    fn saved_layout_merges_live_panels_into_largest_main_dock_without_id_collisions() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let current = dir.write("current.txt", b"current");
        let saved = dir.write("project/saved.txt", b"saved");
        let detached = dir.write("project/detached.txt", b"detached");
        let mut workbench = workspace(&dir);
        workbench.open_or_focus(&current).unwrap();
        let document = workbench.active_document().unwrap();
        let view = workbench.active_view().unwrap();
        workbench
            .session
            .with_commands(view, |commands| commands.paste(b"dirty "))
            .unwrap();
        let singleton = workbench
            .open_plugin_panel(bed_module_debug::PANEL_ID, None, &Value::Null, None)
            .unwrap();
        let mut context = Context::create();
        workbench
            .initialize(&mut context, WorkbenchHostMode::Fullscreen)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        frame(&mut context, &mut workbench);
        let carried = workbench
            .tabs
            .iter()
            .filter(|tab| tab.panel.kind != bed_module_projects::PANEL_ID)
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        let root = workbench.dock_root;
        let spec = WorkspaceSpec::local(
            dir.path("project")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap(),
        );
        let ini = format!(
            "[Window][bed_tab_1]\nPos=0,0\nSize=240,800\nDockId=0x71231001,0\n\n\
             [Window][bed_tab_2]\nPos=240,0\nSize=960,800\nDockId=0x71231002,0\n\n\
             [Window][bed_tab_3]\nPos=0,0\nSize=2000,2000\nDockId=0x71231003,0\n\n\
             [Window][bed_tab_4]\nPos=240,0\nSize=960,800\nDockId=0x71231002,1\n\n\
             [Docking][Data]\nDockSpace ID=0x{root:08X} Window=0x5FD3F7B1 Pos=0,0 Size=1200,800 Split=X\n\
               DockNode ID=0x71231001 Parent=0x{root:08X} SizeRef=240,800\n\
               DockNode ID=0x71231002 Parent=0x{root:08X} SizeRef=960,800 CentralNode=1\n\
             DockNode ID=0x71231003 Pos=0,0 Size=2000,2000\n"
        );
        workbench
            .store
            .as_mut()
            .unwrap()
            .set_layout(
                &spec,
                json!({"version":1,"ini":ini,"panels":[
                    {"id":1,"kind":"explorer"}, {"id":2,"kind":"document","path":saved},
                    {"id":3,"kind":"document","path":detached}, {"id":4,"kind":"debug"}
                ]}),
            )
            .unwrap();
        let session = workbench.session.workspace_id();
        workbench.attach_workspace(&dir.path("project")).unwrap();
        assert_eq!(workbench.session.workspace_id(), session);
        assert!(workbench.session.snapshot(document).unwrap().dirty);
        assert!(
            workbench
                .tabs
                .iter()
                .any(|tab| tab.panel.view_id() == Some(view))
        );
        assert_eq!(workbench.panel_count("debug"), 1);
        assert_eq!(
            workbench
                .tabs
                .iter()
                .find(|tab| tab.panel.kind == bed_module_debug::PANEL_ID)
                .unwrap()
                .id,
            singleton
        );
        let ids = workbench
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), workbench.tab_count());
        workbench.apply_settings(&mut context).unwrap();
        for _ in 0..4 {
            frame(&mut context, &mut workbench);
        }
        let saved_doc = workbench.session.document_for_path(&saved).unwrap();
        let destination = workbench
            .tabs
            .iter()
            .find(|tab| tab.panel.document() == Some(saved_doc))
            .unwrap()
            .dock_id;
        assert_ne!(destination, 0);
        for id in carried {
            assert_eq!(
                workbench
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .unwrap()
                    .dock_id,
                destination
            );
        }
        let detached_doc = workbench.session.document_for_path(&detached).unwrap();
        assert_eq!(
            workbench
                .tabs
                .iter()
                .find(|tab| tab.panel.document() == Some(detached_doc))
                .unwrap()
                .dock_id,
            destination
        );
        assert!(workbench.pending_graft.is_none());
        workbench.cleanup().unwrap();
    }
}
