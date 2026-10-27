//! Five application slots and reusable tool arrangements. Entities stay live.
use crate::persistence::ReflectGlobalPersist;
use bedtime_workbench2::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(
    Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Reflect,
)]
#[reflect(opaque)]
#[reflect(Component, Serialize, Deserialize, WorkbenchPersist)]
pub enum LayoutSlot {
    One,
    Two,
    Three,
    Four,
    Five,
}

impl LayoutSlot {
    pub const ALL: [Self; 5] = [Self::One, Self::Two, Self::Three, Self::Four, Self::Five];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap()
    }
    pub fn label(self) -> &'static str {
        ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][self.index()]
    }
}

/// None selects the built-in arrangement (Terminal in slot one, empty elsewhere).
/// Saved defaults use the same reflected graph format as layout capture.
#[derive(Resource, Clone, Default, Reflect, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, GlobalPersist)]
pub struct LayoutDefaults(pub [Option<bedtime_workbench2::persistence::Snapshot>; 5]);

pub struct LayoutsPlugin;
impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_layouts", |world, _| {
                let entries: Vec<_> = world.query::<(Entity, &LayoutSlot)>().iter(world).map(|(entity, slot)|
                    serde_json::json!({ "layout": entity.to_bits().to_string(),
                        "slot": slot.index()+1, "active": world.get::<ActiveLayout>(entity).is_some() })).collect();
                Ok(serde_json::json!({ "layouts": entries, "defaults": world.resource::<LayoutDefaults>().0 }))
            }).register_automation_command("app.select_layout", |world, arguments| {
                let number = arguments.get("slot").and_then(|v| v.as_u64()).filter(|v| (1..=5).contains(v)).ok_or("slot must be 1–5")?;
                switch(world, LayoutSlot::ALL[number as usize-1])?;
                Ok(serde_json::json!({}))
            });
        }
        app.init_resource::<LayoutDefaults>()
            .add_systems(Update, initialize.after(crate::files::FilesSystems::Update));
    }
}

fn initialize(world: &mut World) {
    if !world.contains_resource::<crate::files::ProjectRoot>() || !crate::storage::ready(world) {
        return;
    }
    let has_slot = world
        .query_filtered::<Entity, With<LayoutSlot>>()
        .iter(world)
        .next()
        .is_some();
    if !has_slot {
        let result = (|| {
            if let Some(layout) = active_layout(world) {
                world.entity_mut(layout).insert(LayoutSlot::One);
                if Workbench::new(world).panels(layout).is_empty() {
                    apply_default(world, layout, LayoutSlot::One)?;
                }
            } else {
                let layout = create_default(world, LayoutSlot::One)?;
                Workbench::new(world).activate_layout(layout)?;
            }
            crate::session::seed_initial_layout(world);
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Layout".into(),
                    message: "The initial layout couldn't be created.".into(),
                    details: Some(error),
                },
            );
            return;
        }
    }
    // Asynchronous preference/session loading is complete. No later operation may
    // reconstruct a world; lazy slot creation and Reset use scoped layout data.
    Workbench::finish_world_restore(world);
}

pub fn switch(world: &mut World, slot: LayoutSlot) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world) {
        return Err("Workspace settings are loading or a close decision is pending".into());
    }
    let existing = world
        .query::<(Entity, &LayoutSlot)>()
        .iter(world)
        .find_map(|(e, current)| (*current == slot).then_some(e));
    let layout = match existing {
        Some(layout) => layout,
        None => create_default(world, slot)?,
    };
    Workbench::new(world).activate_layout(layout)
}

pub fn save_current(world: &mut World) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world) {
        return Err("Finish loading or cancel quit before saving a layout default".into());
    }
    let layout = active_layout(world).ok_or("Select a layout first")?;
    let slot = *world
        .get::<LayoutSlot>(layout)
        .ok_or("Current layout has no numbered slot")?;
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut captured =
        crate::persistence::capture_features(world)?.into_layout(layout, &registry)?;
    // Global arrangements do not contain project source identity or file viewers.
    let files: Vec<_> = captured
        .components::<crate::files::PersistedFile>()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    for panel in files {
        captured.remove_panel(panel, &registry)?;
    }
    crate::git::filter_layout_default(&mut captured, &registry)?;
    for (panel, _) in captured.components::<crate::files::PersistedTree>() {
        captured.insert_component(panel, crate::files::PersistedTree { root: None });
    }
    for entity in &mut captured.dynamic.entities {
        entity.components.retain(|v| {
            v.get_represented_type_info()
                .is_none_or(|info| info.type_id() != std::any::TypeId::of::<LayoutSlot>())
        });
    }
    let snapshot = captured.serialize(&registry)?;
    world.resource_mut::<LayoutDefaults>().0[slot.index()] = Some(snapshot);
    Ok(())
}

pub fn reset_current(world: &mut World) -> Result<(), String> {
    if !crate::close::accepts_user_actions(world) || !crate::storage::ready(world) {
        return Err("Finish loading or cancel quit before resetting a layout".into());
    }
    let layout = active_layout(world).ok_or("Select a layout first")?;
    let slot = *world
        .get::<LayoutSlot>(layout)
        .ok_or("Current layout has no numbered slot")?;
    apply_default(world, layout, slot).map(|_| ())
}

fn create_default(world: &mut World, slot: LayoutSlot) -> Result<Entity, String> {
    let captured = default_capture(world, slot)?;
    let target = Workbench::new(world).create_layout(slot.label())?;
    let root = match Workbench::replace_layout(world, target, captured) {
        Ok(root) => root,
        Err(error) => {
            world.despawn(target);
            return Err(error);
        }
    };
    activate_default(world, root, slot);
    Ok(root)
}

fn apply_default(world: &mut World, layout: Entity, slot: LayoutSlot) -> Result<Entity, String> {
    let captured = default_capture(world, slot)?;
    let root = Workbench::replace_layout(world, layout, captured)?;
    activate_default(world, root, slot);
    Ok(root)
}

fn default_capture(
    world: &World,
    slot: LayoutSlot,
) -> Result<bedtime_workbench2::persistence::CapturedLayout, String> {
    use bedtime_workbench2::persistence::CapturedLayout;
    let registry = world.resource::<AppTypeRegistry>().clone();
    if let Some(snapshot) = &world.resource::<LayoutDefaults>().0[slot.index()] {
        let capture = CapturedLayout::deserialize(snapshot, &registry)?;
        crate::files::validate_layout_default(&capture)?;
        crate::git::validate_layout_default(&capture)?;
        return Ok(capture);
    }
    // Built-ins are ordinary concrete composition, captured through the same API.
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    let initial = app.world_mut();
    initial.insert_resource(registry.clone());
    let layout = initial
        .spawn(Layout {
            name: slot.label().into(),
            ..default()
        })
        .id();
    let area = initial.spawn((Area::default(), InLayout(layout))).id();
    if slot == LayoutSlot::One {
        initial.spawn((crate::terminal::PersistedTerminal, InArea(area)));
    }
    initial.flush();
    Workbench::capture_layout::<ReflectWorkbenchPersist>(initial, layout)
}

fn activate_default(world: &mut World, root: Entity, slot: LayoutSlot) {
    world.entity_mut(root).insert(slot);
    world.get_mut::<Layout>(root).unwrap().name = slot.label().into();
    crate::terminal::activate_persistence(world);
    crate::files::activate_persistence(world);
    crate::persistence::clear(world);
}

pub struct UseLayout<const N: usize>;
impl<const N: usize> ContextCommand for UseLayout<N> {
    const KEY: &'static str = [
        "bed.layout.1",
        "bed.layout.2",
        "bed.layout.3",
        "bed.layout.4",
        "bed.layout.5",
    ][N];
    const LABEL: &'static str = ["Layout 1", "Layout 2", "Layout 3", "Layout 4", "Layout 5"][N];
}

pub struct SaveCurrentLayout;
impl ContextCommand for SaveCurrentLayout {
    const KEY: &'static str = "bed.layout.save_default";
    const LABEL: &'static str = "Save Current Layout";
}

pub struct ResetCurrentLayout;
impl ContextCommand for ResetCurrentLayout {
    const KEY: &'static str = "bed.layout.reset";
    const LABEL: &'static str = "Reset Current Layout";
}

pub fn install_menu(world: &mut World, view: Entity) {
    register_context_command::<SaveCurrentLayout>(world).unwrap();
    register_context_command::<ResetCurrentLayout>(world).unwrap();
    install_global_handler::<SaveCurrentLayout, _>(
        world,
        |_invocation: In<ContextCommandInvocation>, world: &mut World| save_current(world),
    )
    .unwrap();
    install_global_handler::<ResetCurrentLayout, _>(
        world,
        |_invocation: In<ContextCommandInvocation>, world: &mut World| reset_current(world),
    )
    .unwrap();
    let menu = world
        .spawn((
            Menu {
                label: "Layout".into(),
            },
            InMenu(view),
        ))
        .id();
    install_slot::<0>(world, menu, KeyCode::Digit1);
    install_slot::<1>(world, menu, KeyCode::Digit2);
    install_slot::<2>(world, menu, KeyCode::Digit3);
    install_slot::<3>(world, menu, KeyCode::Digit4);
    install_slot::<4>(world, menu, KeyCode::Digit5);
    world.spawn((MenuSeparator, InMenu(menu)));
    world.spawn((
        MenuItem::new(SaveCurrentLayout::LABEL),
        InMenu(menu),
        Invokes::of::<SaveCurrentLayout>(),
    ));
    world.spawn((
        MenuItem::new(ResetCurrentLayout::LABEL),
        InMenu(menu),
        Invokes::of::<ResetCurrentLayout>(),
    ));
}

fn install_slot<const N: usize>(world: &mut World, menu: Entity, key: KeyCode) {
    register_context_command::<UseLayout<N>>(world).unwrap();
    install_global_handler::<UseLayout<N>, _>(
        world,
        |_invocation: In<ContextCommandInvocation>, world: &mut World| {
            switch(world, LayoutSlot::ALL[N])
        },
    )
    .unwrap();
    world.spawn((
        MenuItem::new(LayoutSlot::ALL[N].label()),
        InMenu(menu),
        Invokes::of::<UseLayout<N>>(),
    ));
    world.spawn(Binding::new::<UseLayout<N>>(KeyChord::new(
        Modifiers::PRIMARY,
        key,
    )));
}

use bedtime_workbench2::persistence::ReflectWorkbenchPersist;

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench2::persistence::{CapturedLayout, ReflectWorkbenchPersist};
    fn host() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<LayoutDefaults>();
        crate::persistence::register(&mut app);
        let layout = crate::test_layout(app.world_mut());
        app.world_mut().entity_mut(layout).insert(LayoutSlot::One);
        let area = Workbench::new(app.world_mut())
            .destination_area(layout)
            .unwrap();
        (app, layout, area)
    }
    
    #[test]
    fn switching_slots_keeps_live_panels_and_documents_after_world_restore_is_closed() {
        let (mut app, first, area) = host();
        let panel = app
            .world_mut()
            .spawn((crate::files::FilePanel::default(), InArea(area)))
            .id();
        let document = open_memory_document(
            app.world_mut(),
            "retained".to_owned(),
            DocumentOptions::loaded("Document"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), panel, document).unwrap();
        Workbench::finish_world_restore(app.world_mut());
        switch(app.world_mut(), LayoutSlot::Two).unwrap();
        assert_ne!(active_layout(app.world()), Some(first));
        switch(app.world_mut(), LayoutSlot::One).unwrap();
        assert_eq!(active_layout(app.world()), Some(first));
        assert_eq!(read_document(app.world(), document).unwrap(), "retained");
        assert_eq!(app.world().get::<InArea>(panel).unwrap().0, area);
    }
    
    #[test]
    fn reset_recreates_panels_and_retires_old_layout_while_retaining_canonical_documents() {
        let (mut app, layout, area) = host();
        let terminal = app
            .world_mut()
            .spawn((bedtime_terminal2::TerminalPanel::default(), InArea(area)))
            .id();
        let editor = app
            .world_mut()
            .spawn((crate::files::FilePanel::default(), InArea(area)))
            .id();
        let document = open_memory_document(
            app.world_mut(),
            "dirty".to_owned(),
            DocumentOptions::unsaved("Draft"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), editor, document).unwrap();
        reset_current(app.world_mut()).unwrap();
        let root = active_layout(app.world()).unwrap();
        assert_ne!(root, layout);
        assert!(app.world().get_entity(terminal).is_err());
        assert!(app.world().get_entity(editor).is_err());
        let panels = Workbench::new(app.world_mut()).panels(root);
        assert_eq!(panels.len(), 1);
        assert!(
            app.world()
                .get::<bedtime_terminal2::TerminalPanel>(panels[0])
                .is_some()
        );
        assert_eq!(read_document(app.world(), document).unwrap(), "dirty");
        assert_eq!(app.world().get::<LayoutSlot>(root), Some(&LayoutSlot::One));
    }
    
    #[test]
    fn reflected_defaults_preserve_panel_kinds_and_data_but_exclude_project_sources() {
        let (mut app, _layout, area) = host();
        app.world_mut()
            .spawn((bedtime_mascots2::BedPanel, InArea(area)));
        app.world_mut()
            .spawn((bedtime_mascots2::DuckPanel, InArea(area)));
        app.world_mut().spawn((
            bedtime_filetree2::FileTreePanel::default(),
            bedtime_filetree2::FileTreeRoot("/private-project".into()),
            InArea(area),
        ));
        app.world_mut()
            .spawn((crate::files::FilePanel::default(), InArea(area)));
        app.world_mut().spawn((
            crate::git::diff::DiffPanel::new(crate::git::diff::Descriptor {
                root: "/another-private-project".into(),
                path: "file.rs".into(),
                side: bedtime_git::DiffSide::Unstaged,
            }),
            InArea(area),
        ));
        save_current(app.world_mut()).unwrap();
        let saved = app.world().resource::<LayoutDefaults>().0[0]
            .clone()
            .unwrap();
        assert!(!saved.data.contains("private-project"));
        assert!(!saved.data.contains("PersistedDiff"));
        assert!(!saved.data.contains("PersistedFile\""));
        let capture = CapturedLayout::<ReflectWorkbenchPersist>::deserialize(
            &saved,
            app.world().resource::<AppTypeRegistry>(),
        )
        .unwrap();
        assert!(capture.components::<LayoutSlot>().is_empty());
        assert!(
            capture
                .components::<crate::files::PersistedFile>()
                .is_empty()
        );
        reset_current(app.world_mut()).unwrap();
        let root = active_layout(app.world()).unwrap();
        let panels = Workbench::new(app.world_mut()).panels(root);
        assert_eq!(panels.len(), 3);
        assert!(
            panels
                .iter()
                .any(|&p| app.world().get::<bedtime_mascots2::BedPanel>(p).is_some())
        );
        assert!(
            panels
                .iter()
                .any(|&p| app.world().get::<bedtime_mascots2::DuckPanel>(p).is_some())
        );
    }
    
    #[test]
    fn invalid_reset_and_uncommitted_drafts_preserve_the_original_layout() {
        let (mut app, layout, area) = host();
        let editor = app
            .world_mut()
            .spawn((crate::files::FilePanel::default(), InArea(area)))
            .id();
        let document = open_memory_document(
            app.world_mut(),
            String::new(),
            DocumentOptions::unsaved("Draft"),
        )
        .unwrap();
        bind_document_view(app.world_mut(), editor, document).unwrap();
        app.world_mut().entity_mut(editor).insert(DocumentDraft);
        assert!(reset_current(app.world_mut()).is_err());
        assert!(app.world().get::<DocumentDraft>(editor).is_some());
        app.world_mut().entity_mut(editor).remove::<DocumentDraft>();
        app.world_mut().resource_mut::<LayoutDefaults>().0[0] =
            Some(bedtime_workbench2::persistence::Snapshot {
                version: 2,
                data: "invalid".into(),
            });
        assert!(reset_current(app.world_mut()).is_err());
        assert_eq!(active_layout(app.world()), Some(layout));
        assert_eq!(app.world().get::<InArea>(editor).unwrap().0, area);
    }
    
    #[test]
    fn titlebar_creation_and_drag_use_the_feature_factory() {
        let (mut app, layout, _) = host();
        crate::settings::register_menu(&mut app);
        let key = crate::settings::SettingsPanel::KEY;
        let first = crate::menus::open_or_focus(app.world_mut(), layout, key).unwrap();
        assert_eq!(
            crate::menus::open_or_focus(app.world_mut(), layout, key).unwrap(),
            first
        );
        let area = app.world().get::<InArea>(first).unwrap().0;
        let factory = crate::menus::panel_menu(app.world(), key).unwrap().factory;
        let second = (factory.create)(app.world_mut(), area);
        assert_ne!(first, second);
    }
}
