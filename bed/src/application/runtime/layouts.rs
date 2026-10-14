//! Application layout ownership belongs to the application, not feature plugins.
use super::*;
use bedtime_api::workbench::PanelSuspended;
use bedtime_core::layouts::{DocumentLayouts, definition_snapshot};
use bedtime_plugin_layouts::{LayoutDefinition, PanelDefinition};
use bedtime_workbench::workspace::tiling::Layout;
#[cfg(test)]
use bedtime_workbench::workspace::tiling_state::TilingState;

type LiveLayout = bedtime_plugin_layouts::LiveLayout<DocumentNavigation>;
pub type LayoutSlots = bedtime_plugin_layouts::LayoutSlots<LayoutSlot, DocumentNavigation>;

pub(in crate::application) fn load_snapshot(
    world: &mut World,
    value: &Value,
) -> io::Result<Option<Value>> {
    let active = LayoutSlot::from_value(&value["active_layout_slot"])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid active layout slot"))?;
    // Startup belongs to a window without a selected directory, even if an older
    // workspace snapshot saved it as the active slot.
    let active = if active == LayoutSlot::Startup {
        LayoutSlot::Layout1
    } else {
        active
    };
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
    Ok(value)
}

/// A fresh window uses the global template, never a previous standalone session.
pub(in crate::application) fn initialize(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
    world.insert_resource(LayoutSlots::new(slot));
    restore_layout(world, &template(world, slot), false)?;
    bedtime_workbench::advance_layout_epoch(world);
    Ok(())
}

/// A new directory selects layout 1, preserving documents and numbered slots.
pub(in crate::application) fn enter_directory(world: &mut World) -> io::Result<()> {
    let previous = world.resource::<LayoutSlots>().active();
    if previous == LayoutSlot::Layout1 {
        return Ok(());
    }
    let mut launchers = world
        .resource::<LayoutSlots>()
        .live()
        .get(&LayoutSlot::Startup)
        .map(|startup| startup.panels.clone())
        .unwrap_or_default();
    launchers.extend(active_panel_entities(world));
    launchers.retain(|panel| {
        world
            .get::<PanelRecord>(*panel)
            .is_some_and(|record| record.panel_type == "bedtime.workspaces.panel")
    });
    launchers.sort_unstable();
    launchers.dedup();
    retire_panels(world, &launchers).map_err(io::Error::other)?;
    let mut live = world.resource_mut::<LayoutSlots>().take_live();
    if previous != LayoutSlot::Startup {
        let outgoing = LiveLayout {
            panels: active_panel_entities(world),
            layout: world.resource::<WorkbenchLayout>().clone(),
            context: world.resource::<DocumentNavigation>().clone(),
        };
        bedtime_plugin_layouts::suspend(world, &outgoing);
        live.insert(previous, outgoing);
    }
    if let Some(mut startup) = live.remove(&LayoutSlot::Startup) {
        bedtime_plugin_layouts::retain_live_panels(world, &mut startup);
        for panel in startup.panels {
            world.entity_mut(panel).remove::<PanelSuspended>();
        }
    }
    world
        .resource_mut::<LayoutSlots>()
        .select(LayoutSlot::Layout1);
    restore_directory_layout(world, live, None)?;
    bedtime_workbench::invalidate_layout_frontend(world);
    bedtime_workbench::advance_layout_epoch(world);
    Ok(())
}

/// Attachment keeps standalone panels and processes, but adopts workspace slots.
pub(in crate::application) fn attach(
    world: &mut World,
    terminal: Option<Entity>,
    snapshot: &Value,
) -> io::Result<()> {
    let previous = world.resource::<LayoutSlots>().active();
    let mut launchers = world
        .resource::<LayoutSlots>()
        .live()
        .get(&LayoutSlot::Startup)
        .map(|startup| startup.panels.clone())
        .unwrap_or_default();
    launchers.extend(active_panel_entities(world));
    launchers.retain(|panel| {
        world
            .get::<PanelRecord>(*panel)
            .is_some_and(|record| record.panel_type == "bedtime.workspaces.panel")
    });
    launchers.sort_unstable();
    launchers.dedup();
    retire_panels(world, &launchers).map_err(io::Error::other)?;
    let outgoing = LiveLayout {
        panels: active_panel_entities(world),
        layout: world.resource::<WorkbenchLayout>().clone(),
        context: world.resource::<DocumentNavigation>().clone(),
    };
    let mut live = world.resource_mut::<LayoutSlots>().take_live();
    // A fresh window's empty default slot must not mask its saved arrangement.
    if !outgoing.panels.is_empty() {
        live.insert(previous, outgoing);
    }
    for layout in live.values_mut() {
        bedtime_plugin_layouts::retain_live_panels(world, layout);
    }
    // A suspended Startup layout may also contain documents and terminals.
    // Carry these into the workspace rather than leaving a second Startup session.
    if let Some(startup) = live.remove(&LayoutSlot::Startup) {
        for panel in startup.panels {
            world.entity_mut(panel).remove::<PanelSuspended>();
        }
    }
    clear(world);
    load_snapshot(world, snapshot)?;
    if previous != world.resource::<LayoutSlots>().active()
        && let Some(outgoing) = live.get(&previous)
    {
        bedtime_plugin_layouts::suspend(world, outgoing);
    }
    restore_directory_layout(world, live, terminal)?;
    bedtime_workbench::invalidate_layout_frontend(world);
    bedtime_workbench::advance_layout_epoch(world);
    Ok(())
}

fn restore_directory_layout(
    world: &mut World,
    live: HashMap<LayoutSlot, LiveLayout>,
    terminal: Option<Entity>,
) -> io::Result<()> {
    let fallback = template(world, world.resource::<LayoutSlots>().active());
    bedtime_core::layouts::attach_retained(world, live, &fallback, terminal)
}

pub(in crate::application) fn snapshot(world: &mut World) -> io::Result<Value> {
    bedtime_plugin_layouts::snapshot::<LayoutSlot, DocumentNavigation>(
        world,
        |world| world.resource::<DocumentNavigation>().clone(),
        snapshot_layout,
    )
}

pub(in crate::application) fn request_switch(
    world: &mut World,
    slot: LayoutSlot,
) -> LayoutRequestId {
    let template = template(world, slot);
    bedtime_plugin_layouts::submit_transition::<LayoutSlot, DocumentLayouts>(world, slot, template)
}

pub(in crate::application) fn clear(world: &mut World) {
    bedtime_plugin_layouts::cancel_requests::<LayoutSlot>(
        world,
        "The workspace changed before the layout switch completed",
    );
    *world.resource_mut::<LayoutSlots>() = LayoutSlots::default();
}

pub(in crate::application) fn save_default(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
    let definition = bedtime_plugin_layouts::capture_definition::<DocumentLayouts>(world);
    world
        .non_send_mut::<WorkspacePersistence>()
        .store
        .set_layout_default(slot, Some(definition.to_value()))
}

pub(in crate::application) fn request_reset(world: &mut World) -> LayoutRequestId {
    let slot = world.resource::<LayoutSlots>().active();
    let definition = definition(world, slot);
    bedtime_plugin_layouts::submit_rearrange::<LayoutSlot, DocumentLayouts>(world, definition)
}
#[cfg(test)]
fn reset(world: &mut World) -> io::Result<()> {
    let id = request_reset(world);
    take_layout_result(world, id)
        .expect("fixture rearrangement completes synchronously")
        .map_err(io::Error::other)
}

fn definition(world: &World, slot: LayoutSlot) -> LayoutDefinition {
    if let Some(value) = world
        .non_send::<WorkspacePersistence>()
        .store
        .layout_default(slot)
    {
        return LayoutDefinition::from_value(&value)
            .expect("validated application layout definition");
    }
    let registry = world.resource::<bedtime_api::workbench::WorkbenchRegistry>();
    if slot != LayoutSlot::Startup || registry.panel("bedtime.workspaces.panel").is_some() {
        return builtin(slot);
    }
    let mut definition = LayoutDefinition::new(Layout::default());
    if let Some(kind) = ["bedtime.editor.text-panel", "bedtime.terminal.panel"]
        .into_iter()
        .find(|kind| registry.panel(kind).is_some())
    {
        let id = definition
            .add_panel(PanelDefinition::new(kind), 1)
            .expect("registered startup panel");
        definition.focus(id).expect("startup panel exists");
    }
    definition
}
pub(in crate::application) fn template(world: &World, slot: LayoutSlot) -> Value {
    definition_snapshot(&definition(world, slot))
}

fn builtin(slot: LayoutSlot) -> LayoutDefinition {
    let mut layout = Layout::default();
    let placements: Vec<(&str, u32, Option<&str>)> = match slot {
        LayoutSlot::Layout1 => {
            let docs = layout.split(1, 0, 8200, 1).unwrap();
            let editor = layout.split(1, 0, 1438, 1).unwrap();
            let bottom = layout.split(editor, 1, 7255, 1).unwrap();
            let mascot = layout.split(1, 1, 7500, 1).unwrap();
            vec![
                ("bedtime.explorer.panel", 1, None),
                ("bedtime.terminal.panel", bottom, None),
                ("bedtime.editor.diagnostics-panel", bottom, None),
                ("bedtime.debug.panel", bottom, None),
                ("bedtime.mascot.panel", mascot, None),
                ("bedtime.docs.panel", docs, None),
            ]
        }
        LayoutSlot::Layout2 => {
            let diff = layout.split(1, 0, 2500, 1).unwrap();
            let bottom = layout.split(diff, 1, 7500, 1).unwrap();
            vec![
                ("bedtime.git.panel", 1, None),
                ("bedtime.git.diff", diff, None),
                ("bedtime.terminal.panel", bottom, None),
            ]
        }
        LayoutSlot::Layout3 => {
            let right = layout.split(1, 0, 5000, 1).unwrap();
            let left_bottom = layout.split(1, 1, 5000, 1).unwrap();
            let right_bottom = layout.split(right, 1, 5000, 1).unwrap();
            vec![
                ("bedtime.terminal.panel", 1, None),
                ("bedtime.terminal.panel", right, None),
                ("bedtime.terminal.panel", left_bottom, None),
                ("bedtime.terminal.panel", right_bottom, None),
            ]
        }
        LayoutSlot::Startup => vec![("bedtime.workspaces.panel", 1, None)],
        _ => vec![("bedtime.editor.text-panel", 1, Some("bedtime.editor.text"))],
    };
    let mut definition = LayoutDefinition::new(layout);
    for (kind, area, viewer) in placements {
        let mut panel = PanelDefinition::new(kind);
        if let Some(viewer) = viewer {
            panel = panel.viewer(viewer);
        }
        definition
            .add_panel(panel, area)
            .expect("builtin layout is valid");
    }
    definition
        .focus(1)
        .expect("builtin layout has a first panel");
    definition
}

#[cfg(test)]
mod tests {
    use bedtime_api::documents::PanelBinding;
    fn switch(world: &mut World, slot: LayoutSlot) -> io::Result<()> {
        let id = super::request_switch(world, slot);
        take_layout_result(world, id)
            .expect("fixture transition completes synchronously")
            .map_err(io::Error::other)
    }
    use super::*;
    use crate::application::test_support::TempDir;
    use crate::application::testing::{
        AppAction, AppRequestOutcome, WindowCommand, attach_workspace, capture_app_request,
        capture_app_request_from, focus_panel, open_file, open_panel, panel_presentation,
        poll_app_request, process_app_requests, replace_workspace, submit_app_request,
        tick_app_services, wait_app_request,
    };
    use bedtime_api::{
        storage::{FeatureStateContribution, FeatureStateRestore},
        workbench::{Panel, PanelClose, PanelCreation, PanelResult, PanelTarget, WorkbenchAction},
    };
    use bedtime_documents::ByteEdit;
    use dear_imgui_rs::Ui;
    use std::collections::HashSet;

    #[test]
    fn a_new_temporary_directory_selects_one_and_keeps_other_numbered_panels() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let first = fixture(app.world_mut());
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        let fourth = fixture(app.world_mut());
        let root = dir.path("new-project");
        std::fs::create_dir(&root).unwrap();
        let request = capture_app_request(
            app.world(),
            AppAction::HostAction(bedtime_api::features::HostAction::OpenDirectory {
                location: bedtime_files::FileLocation {
                    provider: bedtime_files::FileProviderId::Local,
                    path: root.clone(),
                },
            }),
            PanelTarget::LargestArea,
        );
        let id = submit_app_request(app.world_mut(), request);
        wait_app_request(app.world_mut(), id).unwrap();
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            LayoutSlot::Layout1
        );
        assert!(active_panel_entities(app.world_mut()).contains(&first));
        assert!(app.world().entities().contains(fourth));
        assert!(!root.join(".bed").exists());
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        assert!(active_panel_entities(app.world_mut()).contains(&fourth));
    }

    #[test]
    fn builtin_layout_one_reserves_a_full_height_docs_column() {
        let value = builtin(LayoutSlot::Layout1).to_value();
        let panels = value["panels"].as_array().unwrap();
        let docs = panels
            .iter()
            .find(|panel| panel["panel_type"] == "bedtime.docs.panel")
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
                !builtin(slot).to_value()["panels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|panel| panel["panel_type"] == "bedtime.docs.panel")
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
            _: &bedtime_api::workbench::ShortcutInput,
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
            bedtime_api::workbench::register(app.world_mut(), "fixture", |registrar| {
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
            bedtime_plugin_editor::EditorPlugin,
            FixturePlugin,
        ));
        app.update();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while bedtime_core::host_requests_pending(app.world()) {
            tick_app_services(app.world_mut()).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "Startup workspace did not settle: {:?}",
                app.world().resource::<ApplicationState>().error
            );
        }
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
        assert!(!bedtime_api::workbench::panel_active(app.world(), original));
        assert!(!bedtime_api::workbench::panel_active(app.world(), editor));
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .dirty
        );
        assert_eq!(app.world().resource::<Closed>().0, 0);
        assert!(bedtime_workbench::recent_panel(app.world_mut(), "fixture.panel").is_none());
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
            HostAction::OpenDocumentViewer {
                tab,
                document,
                viewer: bedtime_plugin_editor::HEX_VIEWER.into(),
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
            bedtime_workbench::recent_panel(app.world_mut(), "fixture.panel"),
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
    fn suspended_startup_terminals_survive_attachment_without_restarting() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        fixture(app.world_mut());
        save_default(app.world_mut(), LayoutSlot::Startup).unwrap();
        switch(app.world_mut(), LayoutSlot::Startup).unwrap();
        let terminal = crate::application::testing::terminal_tests::open_terminal(app.world_mut());
        let surface = panel_presentation(app.world(), terminal).unwrap();
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        assert!(!bedtime_api::workbench::panel_active(app.world(), terminal));
        let root = dir.path("project");
        std::fs::create_dir(&root).unwrap();
        attach_workspace(app.world_mut(), &root).unwrap();
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            LayoutSlot::Layout1
        );
        assert!(bedtime_api::workbench::panel_active(app.world(), terminal));
        assert_eq!(panel_presentation(app.world(), terminal), Some(surface));
        switch(app.world_mut(), LayoutSlot::Layout4).unwrap();
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
        assert!(bedtime_api::workbench::panel_active(app.world(), terminal));
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
        let terminal = crate::application::testing::terminal_tests::open_terminal(app.world_mut());
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
        let expected = builtin(LayoutSlot::Layout3).to_value();
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
            .insert(bedtime_api::workbench::PanelPersistence(false));
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
    fn persistence_failure_keeps_current_slot_and_panels_active() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let panel = fixture(app.world_mut());
        save_current_workspace(app.world_mut(), None).unwrap();
        let spec = app
            .world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .cloned()
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
        assert!(bedtime_api::workbench::panel_active(app.world(), panel));
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
        assert!(bedtime_workbench::focus_area(app.world_mut(), area));
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
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .shortcuts();
        assert_eq!(
            actions
                .iter()
                .filter(|action| action.id.starts_with("bed.app.layout_"))
                .count(),
            10
        );
        let bindings = bedtime_keybindings::Bindings::new(
            actions.to_vec(),
            &Default::default(),
            bedtime_keybindings::Platform::Mac,
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
        for slot in LayoutSlot::ALL {
            for id in [
                format!("bed.app.set_layout_{slot}"),
                format!("bed.app.restore_layout_{slot}"),
            ] {
                assert!(actions.iter().any(|action| action.id == id));
                assert!(bindings.for_action(&id).is_empty());
            }
        }
    }

    #[test]
    fn a_pending_open_defers_switching_and_stale_ui_requests_do_not_move_slots() {
        let dir = TempDir::new();
        let mut app = app(&dir, false);
        let _panel = fixture(app.world_mut());
        let stale = capture_app_request(
            app.world(),
            AppAction::Command(WindowCommand::NewDocument),
            PanelTarget::LargestArea,
        );
        let token = bedtime_core::capture_operation_barrier(app.world_mut(), None);
        bedtime_core::resolve_documents(app.world_mut());
        let request = capture_app_request(
            app.world(),
            AppAction::Command(WindowCommand::UseLayout(LayoutSlot::Layout4)),
            PanelTarget::LargestArea,
        );
        let id = submit_app_request(app.world_mut(), request);
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            crate::config::LayoutSlot::Layout1
        );
        assert!(poll_app_request(app.world_mut(), id).is_none());
        bedtime_core::finish_operation_barrier(app.world_mut(), token, Ok(())).unwrap();
        bedtime_core::resolve_documents(app.world_mut());
        bedtime_plugin_layouts::poll::<LayoutSlot, DocumentLayouts>(app.world_mut());
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
    fn saved_workspace_autosaves_added_tabs_and_restores_them_after_restart() {
        let dir = TempDir::new();
        let start = |restore: bool| {
            let mut app = App::new();
            install_test_infrastructure(&mut app, &dir.path("config"));
            // Match native composition: the picker is installed before app runtime.
            // Its in-memory default must not replace the selected filesystem repository.
            app.add_plugins(bedtime_plugin_workspaces::WorkspacesPlugin);
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
                bedtime_plugin_editor::EditorPlugin,
                FixturePlugin,
            ));
            app.update();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while bedtime_core::host_requests_pending(app.world()) {
                tick_app_services(app.world_mut()).unwrap();
                assert!(
                    std::time::Instant::now() < deadline,
                    "Startup workspace did not settle: {:?}",
                    app.world().resource::<ApplicationState>().error
                );
            }
            app
        };
        let mut app = start(false);
        switch(app.world_mut(), LayoutSlot::Layout1).unwrap();
        let first = fixture(app.world_mut());
        let first_id = app.world().get::<PanelPresentation>(first).unwrap().id;
        let area = app
            .world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(first_id)
            .unwrap();
        let spec = save_current_workspace(app.world_mut(), Some("Example Project".into())).unwrap();
        let second = open_panel(
            app.world_mut(),
            "fixture.panel",
            PanelInput::None,
            json!({"draft":"added after saving"}),
            None,
            Some(area),
            false,
        )
        .unwrap();
        let second_id = app.world().get::<PanelPresentation>(second).unwrap().id;
        let expected_tabs = app
            .world()
            .resource::<WorkbenchLayout>()
            .tiling
            .areas
            .iter()
            .find(|group| group.area == area)
            .unwrap()
            .tabs
            .clone();
        assert!(expected_tabs.ends_with(&[first_id, second_id]));

        // Exercise normal autosave rather than calling the persistence helper.
        std::thread::sleep(std::time::Duration::from_millis(2100));
        tick_app_services(app.world_mut()).unwrap();
        assert!(app.world().resource::<ApplicationState>().error.is_none());
        let paths = test_application_paths(&dir.path("config"));
        let saved = WorkspaceStore::load(&paths).unwrap();
        assert_eq!(
            saved.recent_workspaces().as_slice(),
            std::slice::from_ref(&spec)
        );
        assert_eq!(saved.last_workspace(), Some(spec.clone()));
        assert_eq!(saved.active_layout_slot(&spec), LayoutSlot::Layout1);
        assert!(
            paths
                .workspace_directory(dir.root())
                .join("workspace.json")
                .is_file()
        );
        drop(app);

        let mut restarted = start(true);
        assert!(
            restarted
                .world()
                .resource::<ApplicationState>()
                .error
                .is_none()
        );
        assert_eq!(
            restarted
                .world()
                .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
                .saved(),
            Some(&spec)
        );
        assert_eq!(
            restarted
                .world()
                .resource::<bedtime_plugin_workspaces::WorkspacesState>()
                .recent_workspaces,
            [spec]
        );
        assert_eq!(
            restarted.world().resource::<LayoutSlots>().active(),
            LayoutSlot::Layout1
        );
        let layout = restarted.world().resource::<WorkbenchLayout>();
        let group = layout
            .tiling
            .areas
            .iter()
            .find(|group| group.area == area)
            .unwrap();
        assert_eq!(group.tabs, expected_tabs);
        assert_eq!(group.selected, Some(second_id));
        let restored = active_panel_entities(restarted.world_mut())
            .into_iter()
            .filter(|panel| {
                restarted
                    .world()
                    .get::<PanelRecord>(*panel)
                    .unwrap()
                    .panel_type
                    == "fixture.panel"
            })
            .collect::<Vec<_>>();
        assert_eq!(restored.len(), 2);
        for (id, state) in [
            (first_id, json!({"draft":"retained"})),
            (second_id, json!({"draft":"added after saving"})),
        ] {
            let owner = restored
                .iter()
                .find(|panel| {
                    restarted
                        .world()
                        .get::<PanelPresentation>(**panel)
                        .unwrap()
                        .id
                        == id
                })
                .copied()
                .unwrap();
            let frontend = panel_presentation(restarted.world(), owner).unwrap();
            assert_eq!(
                restarted.world().get::<FixtureState>(frontend).unwrap().0,
                state
            );
            if id == second_id {
                assert_eq!(
                    restarted.world().resource::<WorkbenchLayout>().focused,
                    Some(owner)
                );
            }
        }
    }

    #[test]
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
