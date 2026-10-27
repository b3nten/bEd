//! Durable mascot declarations; scene/GPU/audio state is reconstructed normally.
use bedtime_workbench2::persistence::ReflectWorkbenchPersist;
use bevy::prelude::*;

pub struct MascotsPlugin;
impl Plugin for MascotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bedtime_mascots2::MascotPlugin);
        register_menu(app);
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(bedtime_mascots2::BedPanel)]
struct PersistedBed;

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(bedtime_mascots2::DuckPanel)]
struct PersistedDuck;

pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedBed>()
        .register_type::<PersistedDuck>();
}

pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench2::persistence::CapturedWorld,
) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<bedtime_mascots2::BedPanel>>()
        .iter(world)
        .collect();
    for e in panels {
        captured.insert_component(e, PersistedBed);
    }
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<bedtime_mascots2::DuckPanel>>()
        .iter(world)
        .collect();
    for e in panels {
        captured.insert_component(e, PersistedDuck);
    }
}

pub(crate) fn clear_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, Or<(With<PersistedBed>, With<PersistedDuck>)>>()
        .iter(world)
        .collect();
    for e in panels {
        world
            .entity_mut(e)
            .remove::<(PersistedBed, PersistedDuck)>();
    }
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<bedtime_mascots2::BedPanel>(
        app,
        Some(60),
        bedtime_workbench2::PanelFactory::of::<bedtime_mascots2::BedPanel>(),
        crate::storage::ready,
    );
    crate::menus::register_panel::<bedtime_mascots2::DuckPanel>(
        app,
        None,
        bedtime_workbench2::PanelFactory::of::<bedtime_mascots2::DuckPanel>(),
        crate::storage::ready,
    );
}
