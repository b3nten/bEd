//! Application projection/activation order. Workbench owns graph correctness;
//! each feature owns its durable declarations and runtime reconstruction.
use bedtime_workbench::{
    Workbench,
    persistence::{self as core, CapturedWorld, ReflectWorkbenchPersist, Snapshot},
};
use bevy::prelude::*;
use bevy::reflect::CreateTypeData;

#[derive(Clone)]
pub(crate) struct ReflectGlobalPersist;
impl<T> CreateTypeData<T> for ReflectGlobalPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

#[derive(Clone)]
pub(crate) struct ReflectProjectPersist;
impl<T> CreateTypeData<T> for ReflectProjectPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

pub(crate) fn register(app: &mut App) {
    core::register(app);
    crate::files::register_persistence(app);
    crate::git::register_persistence(app);
    crate::settings::register_persistence(app);
    crate::terminal::register_persistence(app);
    crate::code::register_persistence(app);
    crate::lsp_panel::register_persistence(app);
    crate::mascots::register_persistence(app);
    app.register_type::<crate::session::ProjectTitle>()
        .register_type::<crate::settings::ProjectPreferences>()
        .register_type::<crate::layouts::LayoutSlot>()
        .register_type::<crate::keybindings::ProjectBindings>();
}

pub(crate) fn capture_graph(world: &mut World) -> Result<Snapshot, String> {
    capture_features(world)?.serialize(world.resource::<AppTypeRegistry>())
}

pub(crate) fn capture_features(world: &mut World) -> Result<CapturedWorld, String> {
    let mut captured = Workbench::capture_world::<ReflectWorkbenchPersist>(world)?;
    crate::files::project_persistence(world, &mut captured);
    crate::git::project_persistence(world, &mut captured);
    crate::settings::project_persistence(world, &mut captured);
    crate::terminal::project_persistence(world, &mut captured);
    crate::code::project_persistence(world, &mut captured);
    crate::lsp_panel::project_persistence(world, &mut captured);
    crate::mascots::project_persistence(world, &mut captured);
    Ok(captured)
}

pub(crate) fn clear(world: &mut World) {
    crate::files::clear_persistence(world);
    crate::git::clear_persistence(world);
    crate::settings::clear_persistence(world);
    crate::terminal::clear_persistence(world);
    crate::code::clear_persistence(world);
    crate::lsp_panel::clear_persistence(world);
    crate::mascots::clear_persistence(world);
}

pub(crate) fn prepare_graph(
    snapshot: &Snapshot,
    registry: &AppTypeRegistry,
) -> Result<CapturedWorld, String> {
    let prepared = CapturedWorld::deserialize(snapshot, registry)?;

    crate::files::validate_persistence(&prepared)?;
    crate::git::validate_persistence(&prepared)?;
    let mut slots = std::collections::HashSet::new();
    for (entity, slot) in prepared.components::<crate::layouts::LayoutSlot>() {
        if !prepared.layouts().contains(&entity) || !slots.insert(slot.index()) {
            return Err("Invalid owner or duplicate numbered layout slot".into());
        }
    }
    Ok(prepared)
}

pub(crate) fn restore_graph(world: &mut World, prepared: CapturedWorld) -> Result<(), String> {
    Workbench::restore_world(world, prepared)?;
    crate::terminal::activate_persistence(world);
    crate::files::activate_persistence(world);
    crate::git::activate_persistence(world);
    // Required defaults exist now; runtime work starts only on normal schedules.
    clear(world);
    Ok(())
}

pub(crate) fn capture_local(world: &mut World) -> Result<Snapshot, String> {
    let visibility = world
        .get_resource::<bedtime_filetree::FileTreePreferences>()
        .cloned()
        .unwrap_or_default();
    let mut captured = Workbench::capture_world::<ReflectProjectPersist>(world)?;
    captured
        .dynamic
        .resources
        .push(Box::new(crate::files::PersistedVisibility(visibility)));
    captured
        .dynamic
        .resources
        .push(Box::new(crate::git::PersistedGit {
            draft: crate::git::commit_draft(world),
            options: crate::git::panel_options(world),
        }));
    captured.serialize(world.resource::<AppTypeRegistry>())
}

pub(crate) fn prepare_local(
    snapshot: &Snapshot,
    registry: &AppTypeRegistry,
) -> Result<CapturedWorld<ReflectProjectPersist>, String> {
    let prepared = CapturedWorld::deserialize(snapshot, registry)?;
    if prepared.has_entities() {
        return Err("Project resource partition contains entities".into());
    }
    crate::keybindings::validate(
        &prepared
            .resource::<crate::keybindings::ProjectBindings>()
            .ok_or("Missing project bindings")?
            .0,
    )?;
    prepared
        .resource::<crate::settings::ProjectPreferences>()
        .ok_or("Missing project settings")?
        .0
        .validate()?;
    if prepared
        .resource::<crate::session::ProjectTitle>()
        .ok_or("Missing project title")?
        .0
        .len()
        > 4096
    {
        return Err("Project title too long".into());
    }
    if prepared
        .resource::<crate::git::PersistedGit>()
        .ok_or("Missing Git preferences")?
        .draft
        .len()
        > 65536
    {
        return Err("Git draft too long".into());
    }
    prepared
        .resource::<crate::files::PersistedVisibility>()
        .ok_or("Missing Files visibility")?;
    Ok(prepared)
}

pub(crate) fn activate_local(world: &mut World) {
    if let Some(p) = world.remove_resource::<crate::files::PersistedVisibility>() {
        world.insert_resource(p.0);
    }
    if let Some(p) = world.remove_resource::<crate::git::PersistedGit>() {
        crate::git::restore_draft(world, p.draft);
        crate::git::restore_options(world, p.options);
    }
}

// Application source identities are absolute and bounded. The shared wire codec
// only owns lossless native units and NUL rejection; visibility has other rules.
pub(crate) fn validate_absolute_path(path: &std::path::Path) -> Result<(), String> {
    bedtime_workbench::native_path::validate(path)?;
    if !path.is_absolute() || path.as_os_str().len() > 16384 {
        return Err("Expected an absolute native path of at most 16384 units".into());
    }
    Ok(())
}
