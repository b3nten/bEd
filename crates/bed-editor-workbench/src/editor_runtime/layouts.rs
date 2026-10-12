//! Numbered layout ownership belongs to the application, not feature plugins.
use super::*;
use bed_workbench::workspace::{tiling::Layout, tiling_state::TilingState};
use bed_workbench_api::PanelSuspended;

#[derive(Clone)]
struct LiveLayout {
    panels: Vec<Entity>,
    layout: WorkbenchLayout,
    navigation: EditorNavigation,
}

#[derive(Resource, Clone)]
pub struct LayoutSlots {
    active: u8,
    live: HashMap<u8, LiveLayout>,
    saved: serde_json::Map<String, Value>,
    pending: VecDeque<(u8, Option<EditorRequestId>)>,
}
impl Default for LayoutSlots {
    fn default() -> Self {
        Self {
            active: 1,
            live: Default::default(),
            saved: Default::default(),
            pending: Default::default(),
        }
    }
}
impl LayoutSlots {
    pub fn active(&self) -> u8 {
        self.active
    }
}

fn validate_slot(slot: u8) -> io::Result<()> {
    if (1..=9).contains(&slot) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Layout slots are 1–9",
        ))
    }
}

pub(super) fn migrate_default(world: &mut World) -> io::Result<()> {
    let legacy = world
        .non_send::<WorkspacePersistence>()
        .store
        .default_layout()
        .cloned();
    if let Some(legacy) = legacy {
        if world.non_send::<Settings>().layout_default(1).is_none() {
            world
                .non_send_mut::<Settings>()
                .set_layout_default(1, Some(legacy))?;
        }
        world
            .non_send_mut::<WorkspacePersistence>()
            .store
            .reset_default_layout()?;
    }
    Ok(())
}

pub(super) fn load(world: &mut World) -> Option<Value> {
    let spec = world.resource::<EditorWorkspace>().spec.as_ref();
    let store = &world.non_send::<WorkspacePersistence>().store;
    let mut saved = store
        .session_layouts(spec)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if saved.is_empty() {
        let legacy = match spec {
            Some(spec) => store.layout(spec),
            None => store.standalone_layout(),
        };
        if let Some(legacy) = legacy {
            saved.insert("1".into(), legacy.clone());
        }
    }
    let active = store.active_layout_slot(spec);
    let value = saved.get(&active.to_string()).cloned();
    world.insert_resource(LayoutSlots {
        active,
        saved,
        ..Default::default()
    });
    world.non_send_mut::<WorkspacePersistence>().last_value = None;
    value
}

pub(super) fn snapshot(world: &mut World) -> io::Result<Value> {
    let slots = world.resource::<LayoutSlots>().clone();
    let mut saved = slots.saved;
    for (slot, live) in slots.live {
        let panels = live
            .panels
            .into_iter()
            .filter(|panel| world.entities().contains(*panel))
            .collect();
        saved.insert(
            slot.to_string(),
            snapshot_layout(world, panels, &live.layout, &live.navigation)?,
        );
    }
    let panels = active_panel_entities(world);
    let layout = world.resource::<WorkbenchLayout>().clone();
    let navigation = world.resource::<EditorNavigation>().clone();
    saved.insert(
        slots.active.to_string(),
        snapshot_layout(world, panels, &layout, &navigation)?,
    );
    Ok(json!({"active_layout_slot": slots.active, "layouts": saved}))
}

pub(super) fn blocked(world: &World) -> bool {
    let pending = world.resource::<PendingOperations>();
    !pending.batches.is_empty()
        || pending.saves.iter().any(|save| save.continuation.is_some())
        || !world.non_send::<RemoteOperations>().opens.is_empty()
        || world.non_send::<RemoteOperations>().connection.is_some()
}

pub(super) fn request_switch(
    world: &mut World,
    slot: u8,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    validate_slot(slot)?;
    if blocked(world) {
        world
            .resource_mut::<LayoutSlots>()
            .pending
            .push_back((slot, id));
        return Ok(None);
    }
    switch(world, slot)?;
    Ok(Some(EditorRequestOutcome::Applied))
}

pub(super) fn poll(world: &mut World) {
    while !blocked(world) {
        let Some((slot, id)) = world.resource_mut::<LayoutSlots>().pending.pop_front() else {
            break;
        };
        let result = switch(world, slot)
            .map(|_| EditorRequestOutcome::Applied)
            .map_err(EditorRequestError::from);
        if id.is_none()
            && let Err(error) = &result
        {
            world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
        }
        complete(world, id, result);
    }
}

pub(super) fn waiting(world: &World) -> bool {
    !world.resource::<LayoutSlots>().pending.is_empty()
}

pub(super) fn clear(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<LayoutSlots>().pending);
    for (_, id) in pending {
        complete(
            world,
            id,
            Err("The workspace changed before the layout switch completed".into()),
        );
    }
    *world.resource_mut::<LayoutSlots>() = LayoutSlots::default();
}

fn activate(world: &mut World, live: &LiveLayout) {
    for &panel in &live.panels {
        if world.entities().contains(panel) {
            world.entity_mut(panel).remove::<PanelSuspended>();
        }
    }
    *world.resource_mut::<WorkbenchLayout>() = live.layout.clone();
    *world.resource_mut::<EditorNavigation>() = live.navigation.clone();
    if let Some(panel) = live.layout.focused {
        let _ = focus_panel(world, panel, true);
    }
}

fn switch(world: &mut World, slot: u8) -> io::Result<()> {
    let previous = world.resource::<LayoutSlots>().clone();
    if previous.active == slot {
        return Ok(());
    }
    let panels = active_panel_entities(world);
    let documents = panels
        .iter()
        .filter_map(|panel| {
            world
                .get::<EditorPanelBinding>(*panel)
                .and_then(|binding| binding.document)
        })
        .collect();
    commit_documents(world, &documents)?;
    persist_workspace(world)?;
    let outgoing = LiveLayout {
        panels,
        layout: world.resource::<WorkbenchLayout>().clone(),
        navigation: world.resource::<EditorNavigation>().clone(),
    };
    for &panel in &outgoing.panels {
        world.entity_mut(panel).insert(PanelSuspended);
    }
    {
        let mut slots = world.resource_mut::<LayoutSlots>();
        slots.active = slot;
        slots.live.insert(previous.active, outgoing.clone());
    }
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout::default();
    *world.resource_mut::<EditorNavigation>() = EditorNavigation::default();
    let target = world.resource_mut::<LayoutSlots>().live.remove(&slot);
    let result = (|| {
        if let Some(live) = &target {
            activate(world, live);
        } else if let Some(saved) = previous.saved.get(&slot.to_string()) {
            restore_layout(world, saved, false)?;
        } else {
            restore_layout(world, &template(world, slot), false)?;
        }
        persist_workspace(world)
    })();
    if let Err(error) = result {
        if let Some(target) = &target {
            for &panel in &target.panels {
                if world.entities().contains(panel) {
                    world.entity_mut(panel).insert(PanelSuspended);
                }
            }
        } else {
            let created = active_panel_entities(world);
            retire_panels(world, &created)?;
        }
        world.insert_resource(previous);
        activate(world, &outgoing);
        bed_workbench::invalidate_layout_frontend(world);
        return Err(error);
    }
    bed_workbench::invalidate_layout_frontend(world);
    Ok(())
}

pub(super) fn save_default(world: &mut World, slot: u8) -> io::Result<()> {
    validate_slot(slot)?;
    let panels = active_panel_entities(world).into_iter().filter(|panel| {
        !panel_presentation(world, *panel).and_then(|surface| world.get::<bed_workbench_api::PanelPersistence>(surface)).is_some_and(|policy| !policy.0)
    }).map(|panel| {
        json!({"id": world.get::<PanelPresentation>(panel).unwrap().id,
            "panel_type": world.get::<PanelRecord>(panel).unwrap().panel_type,
            "viewer": world.get::<EditorPanelBinding>(panel).and_then(|binding| binding.viewer.as_ref())})
    }).collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>();
    let value = json!({"version":1,"panels":panels,"tiling":layout.tiling.to_value(panels.iter().filter_map(|panel|panel["id"].as_u64())),
        "focused_area":layout.focused_area,"focused":layout.focused.and_then(|panel|world.get::<PanelPresentation>(panel)).map(|panel|panel.id)});
    world
        .non_send_mut::<Settings>()
        .set_layout_default(slot, Some(value))
}

pub(super) fn reset(world: &mut World) -> io::Result<()> {
    let slot = world.resource::<LayoutSlots>().active;
    let template = template(world, slot);
    let records = template["panels"]
        .as_array()
        .ok_or_else(|| io::Error::other("Layout panels must be an array"))?;
    let mut tiling = TilingState::from_value(
        &template["tiling"],
        records.iter().filter_map(|record| record["id"].as_u64()),
    )
    .map_err(io::Error::other)?;
    let mut remaining = active_panel_entities(world);
    let tab_order = world
        .resource::<WorkbenchLayout>()
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
    let mut remap = HashMap::new();
    for record in records {
        let found = remaining.iter().position(|panel| {
            world.get::<PanelRecord>(*panel).unwrap().panel_type
                == record["panel_type"].as_str().unwrap_or("")
                && world
                    .get::<EditorPanelBinding>(*panel)
                    .and_then(|binding| binding.viewer.as_deref())
                    == record["viewer"].as_str()
        });
        let panel = if let Some(index) = found {
            remaining.remove(index)
        } else {
            let id = record["id"].as_u64().unwrap();
            let mut single = TilingState::default();
            single.assign(id, 1);
            let value = json!({"panels":[record],"tiling":single.to_value([id])});
            let before = active_panel_entities(world);
            restore_layout(world, &value, true)?;
            active_panel_entities(world)
                .into_iter()
                .find(|panel| !before.contains(panel))
                .expect("restoring one panel creates one panel")
        };
        remap.insert(
            record["id"].as_u64().unwrap(),
            world.get::<PanelPresentation>(panel).unwrap().id,
        );
    }
    tiling.remap_panel_ids(&remap);
    let destination = tiling.layout.largest_area();
    for panel in remaining {
        tiling.assign(
            world.get::<PanelPresentation>(panel).unwrap().id,
            destination,
        );
    }
    let focused = template["focused"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .copied();
    let ids = active_panel_entities(world)
        .into_iter()
        .map(|panel| world.get::<PanelPresentation>(panel).unwrap().id)
        .collect::<Vec<_>>();
    let value = tiling.to_value(ids);
    bed_workbench::apply_layout(world, &value).map_err(io::Error::other)?;
    if let Some(id) = focused {
        let panel = active_panel_entities(world)
            .into_iter()
            .find(|panel| world.get::<PanelPresentation>(*panel).unwrap().id == id)
            .unwrap();
        focus_panel(world, panel, true)?;
    } else if let Some(area) = template["focused_area"]
        .as_u64()
        .and_then(|area| u32::try_from(area).ok())
    {
        bed_workbench::focus_area(world, area);
    }
    persist_workspace(world)
}

pub(super) fn template(world: &World, slot: u8) -> Value {
    let mut value = world
        .non_send::<Settings>()
        .layout_default(slot)
        .cloned()
        .unwrap_or_else(|| builtin(slot));
    if let Some(panels) = value["panels"].as_array_mut() {
        for panel in panels {
            panel["state"] = Value::Null;
            panel["default_input"] = json!(true);
        }
    }
    value
}

fn builtin(slot: u8) -> Value {
    let mut layout = Layout::default();
    let placements: Vec<(&str, u32, Option<&str>)> = match slot {
        1 => {
            let editor = layout.split(1, 0, 1754, 1).unwrap();
            let bottom = layout.split(editor, 1, 7255, 1).unwrap();
            let mascot = layout.split(1, 1, 7500, 1).unwrap();
            vec![
                ("bed.explorer.panel", 1, None),
                ("bed.terminal.panel", bottom, None),
                ("bed.editor.diagnostics-panel", bottom, None),
                ("bed.debug.panel", bottom, None),
                ("bed.mascot.panel", mascot, None),
            ]
        }
        2 => {
            let diff = layout.split(1, 0, 2500, 1).unwrap();
            let bottom = layout.split(diff, 1, 7500, 1).unwrap();
            vec![
                ("bed.git.panel", 1, None),
                ("bed.git.diff", diff, None),
                ("bed.terminal.panel", bottom, None),
            ]
        }
        3 => {
            let right = layout.split(1, 0, 5000, 1).unwrap();
            let left_bottom = layout.split(1, 1, 5000, 1).unwrap();
            let right_bottom = layout.split(right, 1, 5000, 1).unwrap();
            vec![
                ("bed.terminal.panel", 1, None),
                ("bed.terminal.panel", right, None),
                ("bed.terminal.panel", left_bottom, None),
                ("bed.terminal.panel", right_bottom, None),
            ]
        }
        _ => vec![("bed.editor.text-panel", 1, Some("bed.editor.text"))],
    };
    let panels = placements
        .into_iter()
        .enumerate()
        .map(|(index, (kind, area, viewer))| {
            (
                json!({"id":index+1,"panel_type":kind,"viewer":viewer}),
                area,
            )
        })
        .collect::<Vec<_>>();
    let mut tiling = TilingState::new(layout);
    for (panel, area) in &panels {
        tiling.assign(panel["id"].as_u64().unwrap(), *area);
    }
    json!({"version":1,"panels":panels.iter().map(|(panel,_)|panel).collect::<Vec<_>>(),
        "tiling":tiling.to_value(panels.iter().map(|(panel,_)|panel["id"].as_u64().unwrap())),"focused":1})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bed_workbench_api::{Panel, PanelResult};
    use dear_imgui_rs::Ui;

    #[derive(Component)]
    struct FixtureState(Value);
    #[derive(Resource, Default)]
    struct Closed(usize);
    struct Fixture(Entity);
    impl Panel for Fixture {
        fn title(&self, _: &World) -> String {
            "Fixture".into()
        }
        fn draw(
            &mut self,
            _: &Ui,
            _: &mut World,
            _: &mut Vec<Action>,
            _: &bed_workbench_api::ShortcutInput,
        ) {
        }
        fn save(&self, world: &World) -> Value {
            world.get::<FixtureState>(self.0).unwrap().0.clone()
        }
    }
    struct FixturePlugin;
    impl Plugin for FixturePlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<Closed>();
            let close =
                app.world_mut()
                    .register_system(|In(_): In<Entity>, mut closed: ResMut<Closed>| {
                        closed.0 += 1;
                        Ok(())
                    });
            bed_workbench_api::register(app.world_mut(), "fixture", |registrar| {
                registrar.panel(
                    "fixture.panel",
                    "Fixture",
                    move |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                        world
                            .entity_mut(creation.entity)
                            .insert((FixtureState(creation.state), EditorPanelClose(close)));
                        Ok(Box::new(Fixture(creation.entity)))
                    },
                );
            })
            .unwrap();
        }
    }
    fn app(dir: &TempDir, restore: bool) -> App {
        let mut app = App::new();
        install_test_infrastructure(&mut app, &dir.path("config"));
        app.add_plugins((
            bed_runtime::BedRuntimePlugin,
            EditorWorkbenchPlugin {
                options: EditorWorkbenchOptions {
                    working_directory: Some(dir.root().to_owned()),
                    restore_standalone: restore,
                    ..Default::default()
                },
            },
            bed_editor::EditorPlugin,
            FixturePlugin,
        ));
        app.update();
        app
    }
    fn fixture(world: &mut World) -> Entity {
        open_panel(
            world,
            "fixture.panel",
            PanelInput::None,
            json!({"draft":"retained"}),
            None,
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn switching_retains_plugin_entities_dirty_documents_and_active_focus() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let original = fixture(app.world_mut());
        let path = dir.path("note.txt");
        std::fs::write(&path, "original").unwrap();
        let editor = open_file(app.world_mut(), &path, None, false, None).unwrap();
        let document = app
            .world()
            .get::<EditorPanelBinding>(editor)
            .unwrap()
            .document
            .unwrap();
        let revision = app
            .world()
            .non_send::<EditorSession>()
            .document_revision(document)
            .unwrap();
        let request = capture_editor_request(
            app.world(),
            EditorAction::ApplyEdits {
                document,
                revision,
                edits: vec![ByteEdit {
                    range: 0..0,
                    bytes: b"dirty ".to_vec(),
                }],
            },
            PanelTarget::LargestArea,
        );
        let id = submit_editor_request(app.world_mut(), request);
        process_editor_requests(app.world_mut()).unwrap();
        poll_editor_request(app.world_mut(), id).unwrap().unwrap();
        switch(app.world_mut(), 4).unwrap();
        assert!(!bed_workbench_api::panel_active(app.world(), original));
        assert!(!bed_workbench_api::panel_active(app.world(), editor));
        assert!(
            app.world()
                .non_send::<EditorSession>()
                .snapshot(document)
                .unwrap()
                .dirty
        );
        assert_eq!(app.world().resource::<Closed>().0, 0);
        assert!(bed_workbench::recent_panel(app.world_mut(), "fixture.panel").is_none());
        let active = app.world().resource::<WorkbenchLayout>().focused;
        focus_panel(app.world_mut(), original, true).unwrap();
        assert_eq!(app.world().resource::<WorkbenchLayout>().focused, active);
        let revision = app
            .world()
            .non_send::<EditorSession>()
            .document_revision(document)
            .unwrap();
        let tab = app.world().get::<PanelPresentation>(editor).unwrap().id;
        for action in [
            HostRequest::Navigate {
                document,
                byte_offset: 0,
                revision,
            },
            HostRequest::SwitchViewer {
                tab,
                document,
                viewer: bed_editor::HEX_VIEWER.into(),
            },
        ] {
            let request = capture_editor_request_from(
                app.world(),
                EditorAction::HostRequest(action),
                PanelTarget::LargestArea,
                None,
            );
            let id = submit_editor_request(app.world_mut(), request);
            process_editor_requests(app.world_mut()).unwrap();
            assert_eq!(
                poll_editor_request(app.world_mut(), id).unwrap().unwrap(),
                EditorRequestOutcome::Ignored
            );
            assert_eq!(app.world().resource::<WorkbenchLayout>().focused, active);
        }
        let second = open_file(app.world_mut(), &path, None, false, None).unwrap();
        let original_binding = app.world().get::<EditorPanelBinding>(editor).unwrap();
        let second_binding = app.world().get::<EditorPanelBinding>(second).unwrap();
        assert_ne!(editor, second);
        assert_eq!(second_binding.document, Some(document));
        assert_ne!(original_binding.view, second_binding.view);
        switch(app.world_mut(), 1).unwrap();
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(editor)
        );
        assert_eq!(
            bed_workbench::recent_panel(app.world_mut(), "fixture.panel"),
            Some(original)
        );
        let surface = panel_presentation(app.world(), original).unwrap();
        assert_eq!(
            app.world().get::<FixtureState>(surface).unwrap().0["draft"],
            "retained"
        );
        assert_eq!(app.world().resource::<Closed>().0, 0);
    }

    #[test]
    fn slots_roundtrip_and_global_updates_wait_for_workspace_reset() {
        let dir = TempDir::new();
        let mut original = app(&dir, false);
        fixture(original.world_mut());
        save_default(original.world_mut(), 4).unwrap();
        switch(original.world_mut(), 4).unwrap();
        let slot4 = active_panel_entities(original.world_mut())[0];
        assert_eq!(
            original
                .world()
                .get::<PanelRecord>(slot4)
                .unwrap()
                .panel_type,
            "fixture.panel"
        );
        original
            .world_mut()
            .non_send_mut::<Settings>()
            .set_layout_default(4, None)
            .unwrap();
        persist_workspace(original.world_mut()).unwrap();
        drop(original);
        let mut restored = app(&dir, true);
        assert_eq!(restored.world().resource::<LayoutSlots>().active(), 4);
        assert_eq!(active_panel_entities(restored.world_mut()).len(), 1);
        let restored_fixture = active_panel_entities(restored.world_mut())[0];
        assert_eq!(
            restored
                .world()
                .get::<PanelRecord>(restored_fixture)
                .unwrap()
                .panel_type,
            "fixture.panel"
        );
        reset(restored.world_mut()).unwrap();
        assert!(active_panel_entities(restored.world_mut()).contains(&restored_fixture));
        assert_eq!(active_panel_entities(restored.world_mut()).len(), 2);
        assert_eq!(restored.world().resource::<Closed>().0, 0);
        switch(restored.world_mut(), 1).unwrap();
        let panel = active_panel_entities(restored.world_mut())[0];
        assert_eq!(
            restored
                .world()
                .get::<PanelRecord>(panel)
                .unwrap()
                .panel_type,
            "fixture.panel"
        );
    }

    #[test]
    fn terminal_grid_reuses_four_live_sessions_when_reset_and_switched() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let terminal = super::super::terminal_tests::open_terminal(app.world_mut());
        switch(app.world_mut(), 3).unwrap();
        let panels = active_panel_entities(app.world_mut());
        assert_eq!(panels.len(), 4);
        let sessions = panels
            .iter()
            .map(|panel| {
                app.world()
                    .get::<EditorPanelBinding>(*panel)
                    .unwrap()
                    .terminal
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        assert_eq!(sessions.len(), 4);
        reset(app.world_mut()).unwrap();
        assert_eq!(active_panel_entities(app.world_mut()), panels);
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .layout
                .areas
                .len(),
            4
        );
        switch(app.world_mut(), 1).unwrap();
        assert!(active_panel_entities(app.world_mut()).contains(&terminal));
        switch(app.world_mut(), 3).unwrap();
        assert_eq!(active_panel_entities(app.world_mut()), panels);
        for panel in panels {
            assert!(
                sessions.contains(
                    &app.world()
                        .get::<EditorPanelBinding>(panel)
                        .unwrap()
                        .terminal
                        .unwrap()
                )
            );
        }
    }

    #[test]
    fn legacy_default_migrates_to_settings_without_overwriting_slot_one() {
        let dir = TempDir::new();
        let mut first = app(&dir, false);
        let legacy = builtin(3);
        first
            .world_mut()
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_default_layout(legacy.clone())
            .unwrap();
        drop(first);
        let mut migrated = app(&dir, false);
        assert_eq!(
            migrated.world().non_send::<Settings>().layout_default(1),
            Some(&legacy)
        );
        assert!(
            migrated
                .world()
                .non_send::<WorkspacePersistence>()
                .store
                .default_layout()
                .is_none()
        );
        migrated
            .world_mut()
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_default_layout(builtin(4))
            .unwrap();
        migrate_default(migrated.world_mut()).unwrap();
        assert_eq!(
            migrated.world().non_send::<Settings>().layout_default(1),
            Some(&legacy)
        );
    }

    #[test]
    fn save_template_omits_plugin_state_paths_and_transient_panels() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        let transient = fixture(app.world_mut());
        let surface = panel_presentation(app.world(), transient).unwrap();
        app.world_mut()
            .entity_mut(surface)
            .insert(bed_workbench_api::PanelPersistence(false));
        save_default(app.world_mut(), 8).unwrap();
        assert_eq!(app.world().resource::<LayoutSlots>().active(), 1);
        let saved = app
            .world()
            .non_send::<Settings>()
            .layout_default(8)
            .unwrap();
        assert_eq!(saved["panels"].as_array().unwrap().len(), 1);
        assert_eq!(
            saved["panels"][0]["id"],
            app.world().get::<PanelPresentation>(panel).unwrap().id
        );
        for field in ["state", "path", "document", "session"] {
            assert!(saved["panels"][0].get(field).is_none());
        }
    }

    #[test]
    fn persistence_failure_keeps_current_slot_and_panels_active() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        persist_workspace(app.world_mut()).unwrap();
        let path = dir.path("config/workspaces.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(switch(app.world_mut(), 4).is_err());
        assert_eq!(app.world().resource::<LayoutSlots>().active(), 1);
        assert!(bed_workbench_api::panel_active(app.world(), panel));
        assert_eq!(active_panel_entities(app.world_mut()), vec![panel]);
    }

    #[test]
    fn reset_restores_focus_to_a_saved_empty_area() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        let area = {
            let mut layout = app.world_mut().resource_mut::<WorkbenchLayout>();
            let area = layout.tiling.layout.split(1, 0, 5000, 1).unwrap();
            layout.tiling.add_area(area);
            area
        };
        assert!(bed_workbench::focus_area(app.world_mut(), area));
        save_default(app.world_mut(), 1).unwrap();
        focus_panel(app.world_mut(), panel, true).unwrap();
        reset(app.world_mut()).unwrap();
        assert_eq!(app.world().resource::<WorkbenchLayout>().focused, None);
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused_area,
            Some(area)
        );
    }

    #[test]
    fn only_nine_switch_actions_are_added_and_focus_bindings_move() {
        let dir = TempDir::new();
        let app = app(&dir, false);
        let actions = app
            .world()
            .resource::<bed_workbench_api::Registry>()
            .shortcuts();
        assert_eq!(
            actions
                .iter()
                .filter(|action| action.id.starts_with("bed.app.layout_"))
                .count(),
            9
        );
        let bindings = bed_keybindings::Bindings::new(
            actions.to_vec(),
            &Default::default(),
            bed_keybindings::Platform::Mac,
        );
        for slot in 1..=9 {
            assert_eq!(
                bindings.for_action(&format!("bed.app.layout_{slot}"))[0]
                    .shortcut
                    .to_string(),
                format!("Command+{slot}")
            );
            assert_eq!(
                bindings.for_action(&format!("bed.app.focus_{slot}"))[0]
                    .shortcut
                    .to_string(),
                format!("Command+Shift+{slot}")
            );
        }
        assert!(
            !actions
                .iter()
                .any(|action| action.id.starts_with("bed.app.set_layout_")
                    || action.id.starts_with("bed.app.restore_layout_"))
        );
    }

    #[test]
    fn a_pending_open_defers_switching_and_stale_ui_requests_do_not_move_slots() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        let stale = capture_editor_request(
            app.world(),
            EditorAction::Command(WindowCommand::NewDocument),
            PanelTarget::LargestArea,
        );
        let session = app.world().non_send::<EditorSession>().workspace_id().0;
        app.world_mut()
            .non_send_mut::<RemoteOperations>()
            .opens
            .push(RemoteOpen {
                session,
                id: None,
                path: dir.path("opening.txt"),
                viewer: None,
                additional: true,
                move_existing: false,
                area: Some(1),
                tab_index: None,
                restore: Some(panel),
                follow_up: None,
            });
        let id = internal_request_id(app.world_mut());
        assert!(
            request_switch(app.world_mut(), 4, Some(id))
                .unwrap()
                .is_none()
        );
        assert_eq!(app.world().resource::<LayoutSlots>().active(), 1);
        assert!(poll_editor_request(app.world_mut(), id).is_none());
        app.world_mut()
            .non_send_mut::<RemoteOperations>()
            .opens
            .clear();
        poll(app.world_mut());
        assert_eq!(
            poll_editor_request(app.world_mut(), id).unwrap().unwrap(),
            EditorRequestOutcome::Applied
        );
        assert_eq!(app.world().resource::<LayoutSlots>().active(), 4);
        let active = active_panel_entities(app.world_mut());
        let id = submit_editor_request(app.world_mut(), stale);
        process_editor_requests(app.world_mut()).unwrap();
        assert_eq!(
            poll_editor_request(app.world_mut(), id).unwrap().unwrap(),
            EditorRequestOutcome::Ignored
        );
        assert_eq!(active_panel_entities(app.world_mut()), active);
    }

    #[test]
    fn persistence_calls_workspace_hooks_once_with_multiple_live_slots() {
        #[derive(Resource, Default)]
        struct Saves(usize);
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        fixture(app.world_mut());
        switch(app.world_mut(), 4).unwrap();
        app.init_resource::<Saves>();
        let save = app.world_mut().register_system(|mut saves: ResMut<Saves>| {
            saves.0 += 1;
            json!({"shared":true})
        });
        let restore = app
            .world_mut()
            .register_system(|In(_): In<EditorWorkspaceRestore>| Ok(()));
        app.world_mut().spawn(EditorWorkspaceContribution {
            owner: "fixture",
            save,
            restore,
        });
        persist_workspace(app.world_mut()).unwrap();
        assert_eq!(app.world().resource::<Saves>().0, 1);
        assert_eq!(
            app.world()
                .non_send::<WorkspacePersistence>()
                .store
                .standalone_module_settings("fixture"),
            Some(&json!({"shared":true}))
        );
    }

    #[test]
    fn workspace_attachment_and_replacement_account_for_inactive_slots() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let retained = fixture(app.world_mut());
        switch(app.world_mut(), 4).unwrap();
        let root = dir.path("project");
        std::fs::create_dir(&root).unwrap();
        attach_workspace(app.world_mut(), &root).unwrap();
        switch(app.world_mut(), 1).unwrap();
        assert!(active_panel_entities(app.world_mut()).contains(&retained));
        let other = dir.path("other");
        std::fs::create_dir(&other).unwrap();
        replace_workspace(
            app.world_mut(),
            WorkspaceSpec::local(other.to_string_lossy()),
        )
        .unwrap();
        assert!(!app.world().entities().contains(retained));
        assert_eq!(app.world().resource::<Closed>().0, 1);
        replace_workspace(
            app.world_mut(),
            WorkspaceSpec::local(root.to_string_lossy()),
        )
        .unwrap();
        assert_eq!(app.world().resource::<LayoutSlots>().active(), 1);
        assert!(
            active_panel_entities(app.world_mut())
                .into_iter()
                .any(
                    |panel| app.world().get::<PanelRecord>(panel).unwrap().panel_type
                        == "fixture.panel"
                )
        );
    }
}
