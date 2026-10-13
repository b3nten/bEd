//! Application layout ownership belongs to the application, not feature plugins.
use super::*;
use bed_api::workbench::PanelSuspended;
use bed_workbench::workspace::{tiling::Layout, tiling_state::TilingState};

type LiveLayout = bed_plugin_layouts::LiveLayout<DocumentNavigation>;
pub type LayoutSlots = bed_plugin_layouts::LayoutSlots<LayoutSlot, DocumentNavigation>;

// Tracked editor requests and save barriers remain application-owned.
#[derive(Resource, Default)]
struct PendingLayoutSwitches(VecDeque<(LayoutSlot, Option<AppRequestId>)>);

pub(super) fn load(world: &mut World) -> io::Result<Option<Value>> {
    let spec = world
        .resource::<WorkspaceState>()
        .spec
        .clone()
        .expect("Only saved workspaces restore snapshots");
    let snapshot = world
        .resource::<WorkspaceRepositoryResource>()
        .0
        .load(&spec)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Saved workspace snapshot is missing",
            )
        })?;
    let value = snapshot.layouts;
    let active = LayoutSlot::from_value(&value["active_layout_slot"])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid active layout slot"))?;
    let saved = value["layouts"]
        .as_object()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Saved layouts must be an object",
            )
        })?
        .clone();
    let value = saved.get(active.key()).cloned();
    world.insert_resource(LayoutSlots::from_saved(active, saved));
    world.non_send_mut::<WorkspacePersistence>().last_value = None;
    Ok(value)
}

/// A fresh window uses the global template, never a previous standalone session.
pub(super) fn initialize(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
    world.insert_resource(LayoutSlots::new(slot));
    restore_layout(world, &template(world, slot), false)?;
    bed_workbench::advance_layout_epoch(world);
    Ok(())
}

/// Attachment keeps standalone panels and processes, but adopts workspace slots.
pub(super) fn attach(world: &mut World, terminal: Option<Entity>) -> io::Result<()> {
    let mut live = world.resource::<LayoutSlots>().live().clone();
    for layout in live.values_mut() {
        bed_plugin_layouts::retain_live_panels(world, layout);
    }
    // A suspended Startup layout may also contain documents and terminals.
    // Carry these into the workspace rather than leaving a second Startup session.
    if let Some(startup) = live.remove(&LayoutSlot::Startup) {
        for panel in startup.panels {
            world.entity_mut(panel).remove::<PanelSuspended>();
        }
    }
    clear(world);
    load(world)?;
    let mut slot = world.resource::<LayoutSlots>().active();
    if slot == LayoutSlot::Startup {
        slot = LayoutSlot::Layout1;
        world.resource_mut::<LayoutSlots>().select(slot);
    }
    // Live numbered layouts take precedence over saved versions on attachment.
    // Merge the active standalone panels into the destination's arrangement.
    if let Some(target) = live.remove(&slot) {
        activate(world, &target);
        let ids = active_panel_entities(world)
            .into_iter()
            .map(|panel| world.get::<PanelPresentation>(panel).unwrap().id)
            .collect::<Vec<_>>();
        let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
        let destination = tiling.layout.largest_area();
        for &id in &ids {
            if tiling.area_for(id).is_none() {
                tiling.assign(id, destination);
            }
        }
        let layout = tiling.to_value(ids);
        bed_workbench::apply_layout(world, &layout).map_err(io::Error::other)?;
    } else {
        let value = world
            .resource::<LayoutSlots>()
            .saved()
            .get(slot.key())
            .cloned()
            .unwrap_or_else(|| template(world, slot));
        restore_layout_with_terminal(world, &value, true, terminal)?;
    }
    world.resource_mut::<LayoutSlots>().replace_live(live);
    bed_workbench::invalidate_layout_frontend(world);
    bed_workbench::advance_layout_epoch(world);
    Ok(())
}

pub(super) fn snapshot(world: &mut World) -> io::Result<Value> {
    bed_plugin_layouts::snapshot::<LayoutSlot, DocumentNavigation>(
        world,
        |world| world.resource::<DocumentNavigation>().clone(),
        snapshot_layout,
    )
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
    slot: LayoutSlot,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if blocked(world) {
        world.init_resource::<PendingLayoutSwitches>();
        world
            .resource_mut::<PendingLayoutSwitches>()
            .0
            .push_back((slot, id));
        return Ok(None);
    }
    switch(world, slot)?;
    Ok(Some(AppRequestOutcome::Applied))
}

pub(super) fn poll(world: &mut World) {
    world.init_resource::<PendingLayoutSwitches>();
    while !blocked(world) {
        let Some((slot, id)) = world.resource_mut::<PendingLayoutSwitches>().0.pop_front() else {
            break;
        };
        let result = switch(world, slot)
            .map(|_| AppRequestOutcome::Applied)
            .map_err(AppRequestError::from);
        if id.is_none()
            && let Err(error) = &result
        {
            world.resource_mut::<WorkspaceState>().error = Some(error.to_string());
        }
        complete(world, id, result);
    }
}

pub(super) fn waiting(world: &World) -> bool {
    world
        .get_resource::<PendingLayoutSwitches>()
        .is_some_and(|pending| !pending.0.is_empty())
}

pub(super) fn clear(world: &mut World) {
    world.init_resource::<PendingLayoutSwitches>();
    let pending = std::mem::take(&mut world.resource_mut::<PendingLayoutSwitches>().0);
    for (_, id) in pending {
        complete(
            world,
            id,
            Err("The workspace changed before the layout switch completed".into()),
        );
    }
    *world.resource_mut::<LayoutSlots>() = LayoutSlots::default();
}

struct BedLayouts;

impl bed_plugin_layouts::LayoutIntegration for BedLayouts {
    type Context = DocumentNavigation;

    fn capture_context(world: &World) -> DocumentNavigation {
        world.resource::<DocumentNavigation>().clone()
    }

    fn activate_context(world: &mut World, context: Option<&DocumentNavigation>) -> io::Result<()> {
        *world.resource_mut::<DocumentNavigation>() = context.cloned().unwrap_or_default();
        if context.is_some()
            && let Some(panel) = world.resource::<WorkbenchLayout>().focused
        {
            // Retained focus must use the normal document/feature lifecycle.
            let _ = focus_panel(world, panel, true);
        }
        Ok(())
    }

    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        restore_layout(world, value, false)
    }

    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        retire_panels(world, panels)
    }

    fn finish(world: &mut World) -> io::Result<()> {
        persist_workspace(world)
    }
}

fn activate(world: &mut World, live: &LiveLayout) {
    bed_plugin_layouts::activate(world, live);
    <BedLayouts as bed_plugin_layouts::LayoutIntegration>::activate_context(
        world,
        Some(&live.context),
    )
    .expect("retained layout navigation is valid");
}

fn switch(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
    if world.resource::<LayoutSlots>().active() == slot {
        return Ok(());
    }
    let documents = active_panel_entities(world)
        .into_iter()
        .filter_map(|panel| {
            world
                .get::<PanelBinding>(panel)
                .and_then(|binding| binding.document)
        })
        .collect();
    commit_documents(world, &documents)?;
    persist_workspace(world)?;
    let template = template(world, slot);
    bed_plugin_layouts::transition::<LayoutSlot, BedLayouts>(world, slot, &template)
}

pub(super) fn save_default(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
    let panels = active_panel_entities(world)
        .into_iter()
        .filter(|panel| {
            !panel_presentation(world, *panel)
                .and_then(|surface| world.get::<bed_api::workbench::PanelPersistence>(surface))
                .is_some_and(|policy| !policy.0)
        })
        .map(|panel| {
            json!({"id": world.get::<PanelPresentation>(panel).unwrap().id,
            "panel_type": world.get::<PanelRecord>(panel).unwrap().panel_type,
            "viewer": world.get::<PanelBinding>(panel).and_then(|binding| binding.viewer.as_ref())})
        })
        .collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>();
    let value = json!({"version":2,"panels":panels,"tiling":layout.tiling.to_value(panels.iter().filter_map(|panel|panel["id"].as_u64())),
        "focused_area":layout.focused_area,"focused":layout.focused.and_then(|panel|world.get::<PanelPresentation>(panel)).map(|panel|panel.id)});
    world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .set_layout_default(slot, Some(value))
}

pub(super) fn reset(world: &mut World) -> io::Result<()> {
    let slot = world.resource::<LayoutSlots>().active();
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
                    .get::<PanelBinding>(*panel)
                    .and_then(|binding| binding.viewer.as_deref())
                    == record["viewer"].as_str()
        });
        let panel = if let Some(index) = found {
            remaining.remove(index)
        } else {
            let id = record["id"].as_u64().unwrap();
            let mut single = TilingState::default();
            single.assign(id, 1);
            let value = json!({"version":2,"panels":[record],"tiling":single.to_value([id])});
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
    bed_workbench::advance_layout_epoch(world);
    persist_workspace(world)
}

pub(super) fn template(world: &World, slot: LayoutSlot) -> Value {
    let mut value = world
        .non_send::<WorkspacePersistence>()
        .store
        .layout_default(slot)
        .unwrap_or_else(|| {
            let registry = world.resource::<bed_api::workbench::WorkbenchRegistry>();
            if slot != LayoutSlot::Startup || registry.panel("bed.workspaces.panel").is_some() {
                return builtin(slot);
            }
            let panel_type = ["bed.editor.text-panel", "bed.terminal.panel"]
                .into_iter()
                .find(|panel| registry.panel(panel).is_some());
            let panels = panel_type
                .map(|panel| vec![json!({"id":1,"panel_type":panel})])
                .unwrap_or_default();
            let mut tiling = TilingState::default();
            if !panels.is_empty() {
                tiling.assign(1, 1);
            }
            json!({
                "version":2,
                "panels":panels,
                "tiling":tiling.to_value(panel_type.map(|_|1)),
                "focused":panel_type.map(|_|1)
            })
        });
    if let Some(panels) = value["panels"].as_array_mut() {
        for panel in panels {
            panel["state"] = Value::Null;
            panel["default_input"] = json!(true);
        }
    }
    value
}

fn builtin(slot: LayoutSlot) -> Value {
    let mut layout = Layout::default();
    let placements: Vec<(&str, u32, Option<&str>)> = match slot {
        LayoutSlot::Layout1 => {
            let docs = layout.split(1, 0, 8200, 1).unwrap();
            let editor = layout.split(1, 0, 1438, 1).unwrap();
            let bottom = layout.split(editor, 1, 7255, 1).unwrap();
            let mascot = layout.split(1, 1, 7500, 1).unwrap();
            vec![
                ("bed.explorer.panel", 1, None),
                ("bed.terminal.panel", bottom, None),
                ("bed.editor.diagnostics-panel", bottom, None),
                ("bed.debug.panel", bottom, None),
                ("bed.mascot.panel", mascot, None),
                ("bed.docs.panel", docs, None),
            ]
        }
        LayoutSlot::Layout2 => {
            let diff = layout.split(1, 0, 2500, 1).unwrap();
            let bottom = layout.split(diff, 1, 7500, 1).unwrap();
            vec![
                ("bed.git.panel", 1, None),
                ("bed.git.diff", diff, None),
                ("bed.terminal.panel", bottom, None),
            ]
        }
        LayoutSlot::Layout3 => {
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
        LayoutSlot::Startup => vec![("bed.workspaces.panel", 1, None)],
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
    json!({"version":2,"panels":panels.iter().map(|(panel,_)|panel).collect::<Vec<_>>(),
        "tiling":tiling.to_value(panels.iter().map(|(panel,_)|panel["id"].as_u64().unwrap())),"focused":1})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::TempDir;
    use bed_api::workbench::{Panel, PanelResult};
    use dear_imgui_rs::Ui;

    #[test]
    fn builtin_layout_one_reserves_a_full_height_docs_column() {
        let value = builtin(LayoutSlot::Layout1);
        let panels = value["panels"].as_array().unwrap();
        let docs = panels
            .iter()
            .find(|panel| panel["panel_type"] == "bed.docs.panel")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let tiling = TilingState::from_value(
            &value["tiling"],
            panels.iter().map(|p| p["id"].as_u64().unwrap()),
        )
        .unwrap();
        let area = tiling.layout.area(tiling.area_for(docs).unwrap());
        assert_eq!(area.rect.min, [8200, 0]);
        assert_eq!(area.rect.max, [10000, 10000]);
        assert_eq!(
            tiling
                .areas
                .iter()
                .find(|group| group.area == area.id)
                .unwrap()
                .tabs,
            vec![docs]
        );
        for slot in LayoutSlot::ALL.into_iter().take(9).skip(1) {
            assert!(
                !builtin(slot)["panels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|panel| panel["panel_type"] == "bed.docs.panel")
            );
        }
    }

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
            _: &mut Vec<WorkbenchAction>,
            _: &bed_api::workbench::ShortcutInput,
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
            bed_api::workbench::register(app.world_mut(), "fixture", |registrar| {
                registrar.panel(
                    "fixture.panel",
                    "Fixture",
                    move |In(creation): In<PanelCreation>, world: &mut World| -> PanelResult {
                        world
                            .entity_mut(creation.entity)
                            .insert((FixtureState(creation.state), PanelClose(close)));
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
            (bevy::MinimalPlugins, bevy::input::InputPlugin),
            AppRuntimePlugin {
                options: AppOptions {
                    working_directory: Some(dir.root().to_owned()),
                    workspace: restore
                        .then(|| {
                            WorkspaceStore::load(&test_application_paths(&dir.path("config")))
                                .unwrap()
                                .last_workspace()
                        })
                        .flatten(),
                    ..Default::default()
                },
            },
            bed_plugin_editor::EditorPlugin,
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
            .get::<PanelBinding>(editor)
            .unwrap()
            .document
            .unwrap();
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        let request = capture_app_request(
            app.world(),
            AppAction::ApplyEdits {
                document,
                revision,
                edits: vec![ByteEdit {
                    range: 0..0,
                    bytes: b"dirty ".to_vec(),
                }],
            },
            PanelTarget::LargestArea,
        );
        let id = submit_app_request(app.world_mut(), request);
        process_app_requests(app.world_mut()).unwrap();
        poll_app_request(app.world_mut(), id).unwrap().unwrap();
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        assert!(!bed_api::workbench::panel_active(app.world(), original));
        assert!(!bed_api::workbench::panel_active(app.world(), editor));
        assert!(
            app.world()
                .non_send::<DocumentSession>()
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
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        let tab = app.world().get::<PanelPresentation>(editor).unwrap().id;
        for action in [
            HostAction::Navigate {
                document,
                byte_offset: 0,
                revision,
            },
            HostAction::SwitchViewer {
                tab,
                document,
                viewer: bed_plugin_editor::HEX_VIEWER.into(),
            },
        ] {
            let request = capture_app_request_from(
                app.world(),
                AppAction::HostAction(action),
                PanelTarget::LargestArea,
                None,
            );
            let id = submit_app_request(app.world_mut(), request);
            process_app_requests(app.world_mut()).unwrap();
            assert_eq!(
                poll_app_request(app.world_mut(), id).unwrap().unwrap(),
                AppRequestOutcome::Ignored
            );
            assert_eq!(app.world().resource::<WorkbenchLayout>().focused, active);
        }
        let second = open_file(app.world_mut(), &path, None, false, None).unwrap();
        let original_binding = app.world().get::<PanelBinding>(editor).unwrap();
        let second_binding = app.world().get::<PanelBinding>(second).unwrap();
        assert_ne!(editor, second);
        assert_eq!(second_binding.document, Some(document));
        assert_ne!(original_binding.view, second_binding.view);
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
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
    #[cfg(feature = "workspaces")]
    fn slots_roundtrip_and_global_updates_wait_for_workspace_reset() {
        let dir = TempDir::new();
        let mut original = app(&dir, false);
        fixture(original.world_mut());
        save_default(original.world_mut(), LayoutSlot::Layout4).unwrap();
        switch(original.world_mut(), LayoutSlot::Layout4).unwrap();
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
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_layout_default(LayoutSlot::Layout4, None)
            .unwrap();
        save_current_workspace(original.world_mut(), None).unwrap();
        drop(original);
        let mut restored = app(&dir, true);
        assert_eq!(
            restored.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout4
        );
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
        switch(restored.world_mut(), LayoutSlot::Layout1).unwrap();
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
    #[cfg(feature = "workspaces")]
    fn suspended_startup_terminals_survive_attachment_without_restarting() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        fixture(app.world_mut());
        save_default(app.world_mut(), LayoutSlot::Startup).unwrap();
        switch(app.world_mut(), LayoutSlot::Startup).unwrap();
        let terminal = crate::application::runtime::terminal_tests::open_terminal(app.world_mut());
        let surface = panel_presentation(app.world(), terminal).unwrap();
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        assert!(!bed_api::workbench::panel_active(app.world(), terminal));
        let root = dir.path("project");
        std::fs::create_dir(&root).unwrap();
        attach_workspace(app.world_mut(), &root).unwrap();
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            LayoutSlot::Layout1
        );
        assert!(bed_api::workbench::panel_active(app.world(), terminal));
        assert_eq!(panel_presentation(app.world(), terminal), Some(surface));
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
        assert!(bed_api::workbench::panel_active(app.world(), terminal));
        assert_eq!(panel_presentation(app.world(), terminal), Some(surface));
        assert!(
            !app.world()
                .resource::<LayoutSlots>()
                .live()
                .contains_key(&LayoutSlot::Startup)
        );
    }

    #[test]
    fn terminal_grid_reuses_four_live_sessions_when_reset_and_switched() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let terminal = super::super::terminal_tests::open_terminal(app.world_mut());
        switch(app.world_mut(), LayoutSlot::Layout3).unwrap();
        let panels = active_panel_entities(app.world_mut());
        assert_eq!(panels.len(), 4);
        let sessions = panels
            .iter()
            .map(|panel| {
                app.world()
                    .get::<PanelBinding>(*panel)
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
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
        assert!(active_panel_entities(app.world_mut()).contains(&terminal));
        switch(app.world_mut(), LayoutSlot::Layout3).unwrap();
        assert_eq!(active_panel_entities(app.world_mut()), panels);
        for panel in panels {
            assert!(
                sessions.contains(
                    &app.world()
                        .get::<PanelBinding>(panel)
                        .unwrap()
                        .terminal
                        .unwrap()
                )
            );
        }
    }

    #[test]
    fn global_templates_roundtrip_without_settings_or_workspace_state() {
        let dir = TempDir::new();
        let mut first = app(&dir, false);
        let expected = builtin(LayoutSlot::Layout3);
        first
            .world_mut()
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_layout_default(LayoutSlot::Layout1, Some(expected.clone()))
            .unwrap();
        drop(first);
        let loaded = app(&dir, false);
        assert_eq!(
            loaded
                .world()
                .non_send::<WorkspacePersistence>()
                .store
                .layout_default(LayoutSlot::Layout1),
            Some(expected)
        );
        assert!(dir.path("config/layouts.json").is_file());
        assert!(!dir.path("config/registry.json").exists());
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
            .insert(bed_api::workbench::PanelPersistence(false));
        save_default(app.world_mut(), LayoutSlot::Layout8).unwrap();
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout1
        );
        let saved = app
            .world()
            .non_send::<WorkspacePersistence>()
            .store
            .layout_default(LayoutSlot::Layout8)
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
    #[cfg(feature = "workspaces")]
    fn persistence_failure_keeps_current_slot_and_panels_active() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        save_current_workspace(app.world_mut(), None).unwrap();
        let spec = app
            .world()
            .resource::<WorkspaceState>()
            .spec
            .clone()
            .unwrap();
        let path = app
            .world()
            .non_send::<WorkspacePersistence>()
            .store
            .workspace_path(&spec)
            .unwrap()
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(switch(app.world_mut(), LayoutSlot::Layout4).is_err());
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout1
        );
        assert!(bed_api::workbench::panel_active(app.world(), panel));
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
        save_default(app.world_mut(), LayoutSlot::Layout1).unwrap();
        focus_panel(app.world_mut(), panel, true).unwrap();
        reset(app.world_mut()).unwrap();
        assert_eq!(app.world().resource::<WorkbenchLayout>().focused, None);
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused_area,
            Some(area)
        );
    }

    #[test]
    fn numbered_switch_shortcuts_remain_and_startup_is_unbound() {
        let dir = TempDir::new();
        let app = app(&dir, false);
        let actions = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .shortcuts();
        assert_eq!(
            actions
                .iter()
                .filter(|action| action.id.starts_with("bed.app.layout_"))
                .count(),
            10
        );
        let bindings = bed_keybindings::Bindings::new(
            actions.to_vec(),
            &Default::default(),
            bed_keybindings::Platform::Mac,
        );
        assert!(bindings.for_action("bed.app.layout_startup").is_empty());
        assert!(bindings.for_action("bed.app.SetCurrentLayout").is_empty());
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
        let stale = capture_app_request(
            app.world(),
            AppAction::Command(WindowCommand::NewDocument),
            PanelTarget::LargestArea,
        );
        let session = app.world().non_send::<DocumentSession>().session_id().0;
        app.world_mut()
            .non_send_mut::<RemoteOperations>()
            .opens
            .push(RemoteOpen {
                session_id: session,
                id: None,
                path: dir.path("opening.txt"),
                viewer: None,
                additional: true,
                mode: FileOpenMode::Keep,
                layout_slot: LayoutSlot::Layout1,
                move_existing: false,
                area: Some(1),
                tab_index: None,
                restore: Some(panel),
                follow_up: None,
            });
        let id = internal_request_id(app.world_mut());
        assert!(
            request_switch(app.world_mut(), LayoutSlot::Layout4, Some(id))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout1
        );
        assert!(poll_app_request(app.world_mut(), id).is_none());
        app.world_mut()
            .non_send_mut::<RemoteOperations>()
            .opens
            .clear();
        poll(app.world_mut());
        assert_eq!(
            poll_app_request(app.world_mut(), id).unwrap().unwrap(),
            AppRequestOutcome::Applied
        );
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout4
        );
        let active = active_panel_entities(app.world_mut());
        let id = submit_app_request(app.world_mut(), stale);
        process_app_requests(app.world_mut()).unwrap();
        assert_eq!(
            poll_app_request(app.world_mut(), id).unwrap().unwrap(),
            AppRequestOutcome::Ignored
        );
        assert_eq!(active_panel_entities(app.world_mut()), active);
    }

    #[test]
    #[cfg(feature = "workspaces")]
    fn persistence_calls_workspace_hooks_once_with_multiple_live_slots() {
        #[derive(Resource, Default)]
        struct Saves(usize);
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let unavailable = json!({"version":7,"custom":["preserve",{"draft":true}]});
        let spec = WorkspaceSpec::local(dir.root().canonicalize().unwrap().to_string_lossy());
        app.world_mut()
            .non_send_mut::<WorkspacePersistence>()
            .store
            .record_workspace(spec.clone())
            .unwrap();
        app.world_mut()
            .non_send_mut::<WorkspacePersistence>()
            .store
            .set_plugin_state(&spec, "unavailable.feature", unavailable.clone())
            .unwrap();
        fixture(app.world_mut());
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        app.init_resource::<Saves>();
        let save = app.world_mut().register_system(|mut saves: ResMut<Saves>| {
            saves.0 += 1;
            json!({"shared":true})
        });
        let restore = app
            .world_mut()
            .register_system(|In(_): In<FeatureStateRestore>| Ok(()));
        app.world_mut().spawn(FeatureStateContribution {
            owner: "fixture",
            save,
            restore,
        });
        save_current_workspace(app.world_mut(), None).unwrap();
        assert_eq!(app.world().resource::<Saves>().0, 1);
        assert_eq!(
            app.world()
                .non_send::<WorkspacePersistence>()
                .store
                .plugin_state(&spec, "fixture"),
            Some(&json!({"shared":true}))
        );
        assert_eq!(
            app.world()
                .non_send::<WorkspacePersistence>()
                .store
                .plugin_state(&spec, "unavailable.feature"),
            Some(&unavailable)
        );
        drop(app);
        let mut restarted = self::app(&dir, false);
        // Neither contribution is installed in this fresh host. Saving its
        // layouts must retain both owners for a later plugin installation.
        persist_workspace(restarted.world_mut()).unwrap();
        let store = &restarted.world().non_send::<WorkspacePersistence>().store;
        assert_eq!(
            store.plugin_state(&spec, "unavailable.feature"),
            Some(&unavailable)
        );
        assert_eq!(
            store.plugin_state(&spec, "fixture"),
            Some(&json!({"shared":true}))
        );
    }

    #[test]
    #[cfg(feature = "workspaces")]
    fn workspace_attachment_and_replacement_account_for_inactive_slots() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let retained = fixture(app.world_mut());
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        let root = dir.path("project");
        std::fs::create_dir(&root).unwrap();
        attach_workspace(app.world_mut(), &root).unwrap();
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
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
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout1
        );
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
