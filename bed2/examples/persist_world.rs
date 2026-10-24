//! Small native/headless consumer of the optional Workbench persistence layer.
use bedtime_ui2::{UiContext, UiPlugin, egui};
use bedtime_workbench2::{
    persistence::{CapturedWorld, ReflectWorkbenchPersist, Snapshot},
    *,
};
use bevy::reflect::CreateTypeData;
use bevy::{prelude::*, window::PrimaryWindow};
use serde::{Deserialize, Serialize};
#[derive(Clone)]
struct ReflectGlobalPersist;
impl<T> CreateTypeData<T> for ReflectGlobalPersist {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource, GlobalPersist)]
struct GlobalSettings {
    large_text: bool,
}
#[derive(Resource, Reflect, Default)]
#[reflect(Resource, WorkbenchPersist)]
struct ProjectSettings {
    value: i32,
}
#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(PanelInstance=PanelInstance::of::<Self>(),RuntimeOnly)]
struct Notes {
    text: String,
}
#[derive(Component, Default)]
struct RuntimeOnly {
    clicks: u32,
}
impl Panel for Notes {
    const LABEL: &'static str = "Notes";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        ui.text_edit_multiline(&mut world.get_mut::<Notes>(panel).unwrap().text);
        let mut runtime = world.get_mut::<RuntimeOnly>(panel).unwrap();
        if ui
            .button(format!("Runtime clicks: {}", runtime.clicks))
            .clicked()
        {
            runtime.clicks += 1;
        }
        ui.add(egui::DragValue::new(
            &mut world.resource_mut::<ProjectSettings>().value,
        ));
        ui.checkbox(
            &mut world.resource_mut::<GlobalSettings>().large_text,
            "Global setting",
        );
    }
}
fn register(app: &mut App) {
    app.add_plugins(WorkbenchCorePlugin)
        .init_resource::<GlobalSettings>()
        .init_resource::<ProjectSettings>()
        .register_type::<GlobalSettings>()
        .register_type::<ProjectSettings>()
        .register_type::<Notes>();
}
fn seed(world: &mut World) {
    for name in ["Writing", "Review"] {
        let mut w = Workbench::new(world);
        let layout = w.create_layout(name).unwrap();
        if active_layout(w.world()).is_none() {
            w.activate_layout(layout).unwrap();
        }
        let first = w.destination_area(layout).unwrap();
        let second = w
            .split_area(first, SplitAxis::Horizontal, 5000, [1, 1])
            .unwrap();
        for area in [first, second] {
            for i in 0..2 {
                w.world_mut().spawn((
                    Notes {
                        text: format!("{name} note {i}"),
                    },
                    InArea(area),
                ));
            }
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    version: u32,
    global: Snapshot,
    project: Snapshot,
}
fn capture_archive(world: &mut World) -> Result<Archive, String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    Ok(Archive {
        version: 1,
        global: Workbench::capture_world::<ReflectGlobalPersist>(world)?.serialize(&registry)?,
        project: Workbench::capture_world::<ReflectWorkbenchPersist>(world)?
            .serialize(&registry)?,
    })
}
fn prepare_archive(
    archive: Archive,
    registry: &AppTypeRegistry,
) -> Result<(CapturedWorld<ReflectGlobalPersist>, CapturedWorld), String> {
    if archive.version != 1 {
        return Err("Unsupported example archive".into());
    }
    Ok((
        CapturedWorld::<ReflectGlobalPersist>::deserialize(&archive.global, registry)?,
        CapturedWorld::deserialize(&archive.project, registry)?,
    ))
}
fn restore_archive(
    world: &mut World,
    (global, project): (CapturedWorld<ReflectGlobalPersist>, CapturedWorld),
) -> Result<(), String> {
    Workbench::restore_world(world, global)?;
    Workbench::restore_world(world, project)?;
    Ok(())
}
fn roundtrip() -> Result<(), String> {
    let mut source = App::new();
    register(&mut source);
    seed(source.world_mut());
    let archive = capture_archive(source.world_mut())?;
    let mut target = App::new();
    register(&mut target);
    for _ in 0..20 {
        target.world_mut().spawn_empty();
    }
    let prepared = prepare_archive(archive, target.world().resource::<AppTypeRegistry>())?;
    restore_archive(target.world_mut(), prepared)?;
    assert_eq!(
        target
            .world_mut()
            .query::<&Notes>()
            .iter(target.world())
            .count(),
        8
    );
    assert!(
        target
            .world_mut()
            .query::<&RuntimeOnly>()
            .iter(target.world())
            .all(|r| r.clicks == 0)
    );
    println!(
        "round trip passed: one session, global/project resources, two layouts and eight panels"
    );
    Ok(())
}
#[derive(Resource, Default)]
struct Controls {
    message: String,
    action: Option<bool>,
    archive: Option<Archive>,
    frame: u32,
    captured: bool,
}
fn toolbar(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>) {
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            world.resource_mut::<Controls>().action = Some(false);
        }
        if ui.button("Reload layout").clicked() {
            world.resource_mut::<Controls>().action = Some(true);
        }
        let layouts = layouts(world).to_vec();
        for layout in layouts {
            let name = world.get::<Layout>(layout).unwrap().name.clone();
            if ui.button(name).clicked() {
                Workbench::new(world)
                    .requests()
                    .submit(WorkbenchRequest::ActivateLayout(layout));
            }
        }
    });
    ui.label(&world.resource::<Controls>().message);
}
fn actions(world: &mut World) {
    let Some(load) = world.resource_mut::<Controls>().action.take() else {
        return;
    };
    let result = (|| -> Result<(), String> {
        if !load {
            let archive = capture_archive(world)?;
            let bytes =
                ron::ser::to_string_pretty(&archive, default()).map_err(|e| e.to_string())?;
            std::fs::write("target/persist-world-smoke.ron", bytes).map_err(|e| e.to_string())?;
            world.resource_mut::<Controls>().archive = Some(archive);
        } else {
            let bytes =
                std::fs::read("target/persist-world-smoke.ron").map_err(|e| e.to_string())?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("Archive too large".into());
            }
            let archive = ron::de::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let (_, project) = prepare_archive(archive, world.resource::<AppTypeRegistry>())?;
            let target = active_layout(world).ok_or("Select a layout first")?;
            let name = &world.get::<Layout>(target).unwrap().name;
            let source = project
                .components::<Layout>()
                .into_iter()
                .find_map(|(entity, layout)| (&layout.name == name).then_some(entity))
                .ok_or("Saved archive has no matching layout")?;
            let capture = project.into_layout(source, world.resource::<AppTypeRegistry>())?;
            Workbench::replace_layout(world, target, capture)?;
        }
        Ok(())
    })();
    world.resource_mut::<Controls>().message = result.map_or_else(
        |e| format!("error: {e}"),
        |_| {
            if load {
                "restored".into()
            } else {
                "saved".into()
            }
        },
    );
}
fn setup(world: &mut World) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world.entity_mut(window).insert((
        WorkbenchSurface::default(),
        Titlebar {
            height: 48.0,
            draw: toolbar,
        },
    ));
    world.spawn((Camera2d, WorkbenchCamera));
    if std::env::args().any(|argument| argument == "--load") {
        let result = (|| {
            let bytes =
                std::fs::read("target/persist-world-smoke.ron").map_err(|e| e.to_string())?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("Archive too large".into());
            }
            let archive = ron::de::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let prepared = prepare_archive(archive, world.resource::<AppTypeRegistry>())?;
            restore_archive(world, prepared)
        })();
        if let Err(error) = result {
            world.resource_mut::<Controls>().message = format!("Startup load failed: {error}");
            seed(world);
        }
    } else {
        seed(world);
    }
    Workbench::finish_world_restore(world);
}
fn smoke(world: &mut World) {
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
    world.resource_mut::<Controls>().frame += 1;
    let frame = world.resource::<Controls>().frame;
    match frame {
        5 => world.resource_mut::<Controls>().action = Some(false),
        8 => {
            assert_eq!(world.resource::<Controls>().message, "saved");
            let target = active_layout(world).unwrap();
            for panel in Workbench::new(world).panels(target) {
                world.get_mut::<Notes>(panel).unwrap().text = "mutation".into();
            }
            world.resource_mut::<Controls>().action = Some(true);
        }
        12 => {
            assert_eq!(world.resource::<Controls>().message, "restored");
            assert!(
                world
                    .query::<&Notes>()
                    .iter(world)
                    .all(|n| n.text != "mutation")
            );
            world
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("target/persist-world-smoke.png"))
                .observe(
                    |_: On<ScreenshotCaptured>, mut controls: ResMut<Controls>| {
                        controls.captured = true;
                    },
                );
        }
        _ => {}
    }
    if frame > 15 && world.resource::<Controls>().captured {
        println!("native persistence smoke passed");
        world.write_message(AppExit::Success);
    }
    assert!(frame < 300, "Native persistence smoke timed out");
}
fn main() -> AppExit {
    if std::env::args().any(|a| a == "--roundtrip") {
        return roundtrip().map_or_else(
            |e| {
                eprintln!("{e}");
                AppExit::error()
            },
            |_| AppExit::Success,
        );
    }
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Reflected single-session persistence".into(),
            resolution: (1200, 760).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((UiPlugin, WorkbenchUiPlugin));
    register(&mut app);
    app.init_resource::<Controls>()
        .add_systems(Startup, setup)
        .add_systems(Update, actions);
    if std::env::args().any(|a| a == "--smoke") {
        app.add_systems(Update, smoke.after(actions));
    }
    app.run()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_roundtrip() {
        roundtrip().unwrap();
    }
    #[test]
    fn runtime_is_excluded() {
        let mut app = App::new();
        register(&mut app);
        seed(app.world_mut());
        let archive = capture_archive(app.world_mut()).unwrap();
        assert!(!archive.project.data.contains("RuntimeOnly"));
        assert!(!archive.project.data.contains("PanelPresentation"));
    }
    #[test]
    fn partitions_reject_wrong_resource_destination() {
        let mut app = App::new();
        register(&mut app);
        seed(app.world_mut());
        let archive = capture_archive(app.world_mut()).unwrap();
        assert!(
            CapturedWorld::<ReflectWorkbenchPersist>::deserialize(
                &archive.global,
                app.world().resource::<AppTypeRegistry>(),
            )
            .is_err()
        );
    }
}
