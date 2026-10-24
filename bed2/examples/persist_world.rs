//! Isolated reflection persistence experiment; never reads bed2's real settings.
#[path = "persist_world/persistence.rs"]
mod persistence;
#[cfg(test)]
#[path = "persist_world/tests.rs"]
mod tests;

use bedtime_ui2::{UiContext, UiPlugin, egui};
use bedtime_workbench2::*;
use bevy::{prelude::*, window::PrimaryWindow};
use persistence::ReflectWorkbenchPersist;
use serde::{Deserialize, Serialize};

#[derive(Resource, Reflect, Default, Debug, PartialEq)]
#[reflect(Resource, WorkbenchPersist)]
struct GlobalSettings {
    large_text: bool,
}

/// The same marked component can live globally, on a workspace, area or panel.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, WorkbenchPersist)]
struct Preferences {
    label: String,
    value: i32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(PanelInstance = PanelInstance::of::<Self>(), RuntimeOnly)]
struct Notes {
    text: String,
}

#[derive(Component, Default)]
struct RuntimeOnly {
    clicks: u32,
}

/// Declarative binding data only; this prototype doesn't install a binding editor
/// or command handlers. Tests exercise global/workspace/panel ownership.
#[derive(Component, Reflect)]
#[reflect(Component, WorkbenchPersist)]
struct BindingPreferences {
    command: String,
    shortcut: String,
    #[entities]
    target: Option<Entity>,
}

impl Panel for Notes {
    const KEY: &'static str = "prototype.notes";
    const LABEL: &'static str = "Notes";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        let large = world.resource::<GlobalSettings>().large_text;
        if large {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(20.0));
        }
        ui.label("marked panel data (saved)");
        ui.text_edit_multiline(&mut world.get_mut::<Notes>(panel).unwrap().text);
        if let Some(mut p) = world.get_mut::<Preferences>(panel) {
            ui.horizontal(|ui| {
                ui.label(&p.label);
                ui.add(egui::DragValue::new(&mut p.value));
            });
        }
        let mut runtime = world.get_mut::<RuntimeOnly>(panel).unwrap();
        if ui
            .button(format!(
                "runtime-only clicks: {} (not saved)",
                runtime.clicks
            ))
            .clicked()
        {
            runtime.clicks += 1;
        }
        ui.separator();
        let workspace = world.get::<InWorkspace>(panel).unwrap().0;
        if let Some(mut p) = world.get_mut::<Preferences>(workspace) {
            ui.horizontal(|ui| {
                ui.label(&p.label);
                ui.add(egui::DragValue::new(&mut p.value));
            });
        }
        ui.checkbox(
            &mut world.resource_mut::<GlobalSettings>().large_text,
            "global large text (saved)",
        );
        ui.label("resize/split areas, reorder tabs, switch workspaces, then save.\nchange things and load to see what survives.");
    }
}

fn register(app: &mut App) {
    app.add_plugins(WorkbenchCorePlugin)
        .init_resource::<GlobalSettings>()
        .register_type::<GlobalSettings>()
        .register_type::<Preferences>()
        .register_type::<Notes>()
        .register_type::<BindingPreferences>();
    persistence::register(app);
}

fn seed(world: &mut World) {
    world.spawn(Preferences {
        label: "global entity".into(),
        value: 7,
    });
    world.spawn(BindingPreferences {
        command: "prototype.save".into(),
        shortcut: "Primary+S".into(),
        target: None,
    });
    for (index, name) in ["alpha", "beta"].into_iter().enumerate() {
        let workspace = world
            .spawn((
                Workspace {
                    name: name.into(),
                    ..default()
                },
                Preferences {
                    label: "workspace value".into(),
                    value: index as i32 + 10,
                },
            ))
            .id();
        if index == 0 {
            world.entity_mut(workspace).insert(ActiveWorkspace);
        }
        let layout = world
            .spawn((
                Layout {
                    name: "writing".into(),
                    ..default()
                },
                InWorkspace(workspace),
                ActiveLayout,
            ))
            .id();
        let mut areas = Vec::new();
        for side in 0..2 {
            let area = world
                .spawn((
                    Area {
                        rect: AreaRect {
                            min: [side * LAYOUT_EXTENT / 2, 0],
                            max: [(side + 1) * LAYOUT_EXTENT / 2, LAYOUT_EXTENT],
                        },
                        selected: None,
                    },
                    InLayout(layout),
                    Preferences {
                        label: "area value".into(),
                        value: 20 + side,
                    },
                ))
                .id();
            areas.push(area);
            let mut panels = Vec::new();
            for tab in 0..2 {
                let panel = world
                    .spawn((
                        Notes {
                            text: format!("{name}: area {side}, tab {tab}"),
                        },
                        InArea(area),
                        Preferences {
                            label: "panel value".into(),
                            value: 30 + tab,
                        },
                    ))
                    .id();
                world.get_mut::<PanelInstance>(panel).unwrap().title =
                    format!("{name} {side}.{tab}");
                panels.push(panel);
                world.spawn((
                    BindingPreferences {
                        command: "prototype.action".into(),
                        shortcut: "Primary+K".into(),
                        target: Some(panel),
                    },
                    InPanel(panel),
                ));
            }
            // Deliberately remember a non-last tab to catch attachment-hook changes.
            world.get_mut::<Area>(area).unwrap().selected = Some(panels[0]);
        }
        world.get_mut::<Layout>(layout).unwrap().focused_area = Some(areas[0]);
        world.spawn((
            BindingPreferences {
                command: "prototype.action".into(),
                shortcut: "Primary+J".into(),
                target: None,
            },
            InWorkspace(workspace),
        ));
    }
    reconcile_workbench(world).expect("Valid prototype seed");
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    version: u32,
    global: String,
    workspaces: Vec<String>,
}
fn capture_archive(world: &mut World) -> Result<Archive, String> {
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    Ok(Archive {
        version: 1,
        global: persistence::capture(world, None)?,
        workspaces: workspaces
            .into_iter()
            .map(|w| persistence::capture(world, Some(w)))
            .collect::<Result<_, _>>()?,
    })
}
fn prepare_archive(
    archive: Archive,
    registry: &AppTypeRegistry,
) -> Result<Vec<persistence::Prepared>, String> {
    if archive.version != 1 || archive.workspaces.len() > 16 {
        return Err("Unsupported prototype archive".into());
    }
    std::iter::once((archive.global, false))
        .chain(archive.workspaces.into_iter().map(|w| (w, true)))
        .map(|(s, scoped)| persistence::prepare(&s, registry, scoped))
        .collect()
}
fn restore_archive(world: &mut World, parts: Vec<persistence::Prepared>) -> Result<(), String> {
    for part in parts {
        persistence::restore(world, part)?;
    }
    // Choosing the active workspace is application policy in this experiment.
    let first = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .next();
    if let Some(first) = first {
        world.entity_mut(first).insert(ActiveWorkspace);
    }
    reconcile_workbench(world).map_err(|e| format!("{e:?}"))?;
    Ok(())
}

#[derive(Resource)]
struct Controls {
    path: String,
    message: String,
    action: Option<Action>,
    preview: String,
    globals: Vec<Entity>,
}
enum Action {
    Save,
    Load,
    Preview,
    Switch(Entity),
}
fn toolbar(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>) {
    let workspaces: Vec<_> = world
        .query::<(Entity, &Workspace)>()
        .iter(world)
        .map(|(e, w)| (e, w.name.clone()))
        .collect();
    let mut controls = world.resource_mut::<Controls>();
    ui.horizontal(|ui| {
        for (e, name) in workspaces {
            if ui.button(name).clicked() {
                controls.action = Some(Action::Switch(e));
            }
        }
        ui.label("prototype file:");
        ui.add(egui::TextEdit::singleline(&mut controls.path).desired_width(240.0));
        if ui.button("save all").clicked() {
            controls.action = Some(Action::Save);
        }
        if ui.button("load / replace").clicked() {
            controls.action = Some(Action::Load);
        }
        if ui.button("inspect snapshot").clicked() {
            controls.action = Some(Action::Preview);
        }
    });
    ui.label(&controls.message);
    if !controls.preview.is_empty() {
        egui::Window::new("reflected partitions").show(ui.ctx(), |ui| {
            egui::ScrollArea::both().max_height(450.0).show(ui, |ui| {
                ui.monospace(&controls.preview);
            });
            if ui.button("close").clicked() {
                controls.preview.clear();
            }
        });
    }
}
fn actions(world: &mut World) {
    let Some(action) = world.resource_mut::<Controls>().action.take() else {
        return;
    };
    let result = (|| -> Result<String, String> {
        match action {
            Action::Switch(e) => {
                let previous: Vec<_> = world
                    .query_filtered::<Entity, With<ActiveWorkspace>>()
                    .iter(world)
                    .collect();
                for p in previous {
                    world.entity_mut(p).remove::<ActiveWorkspace>();
                }
                world.entity_mut(e).insert(ActiveWorkspace);
                Ok("switched workspace".into())
            }
            Action::Save => {
                let archive = capture_archive(world)?;
                let text =
                    ron::ser::to_string_pretty(&archive, default()).map_err(|e| e.to_string())?;
                std::fs::write(&world.resource::<Controls>().path, text)
                    .map_err(|e| e.to_string())?;
                Ok("saved global + independently restorable workspace partitions".into())
            }
            Action::Preview => {
                let archive = capture_archive(world)?;
                world.resource_mut::<Controls>().preview = format!(
                    "GLOBAL\n{}\n{}",
                    archive.global,
                    archive
                        .workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, s)| format!("WORKSPACE {i}\n{s}\n"))
                        .collect::<String>()
                );
                Ok("snapshot captured in memory; no file written".into())
            }
            Action::Load => {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(&world.resource::<Controls>().path)
                    .map_err(|e| e.to_string())?
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err("Archive exceeds 16 MiB".into());
                }
                let archive = ron::de::from_bytes(&bytes).map_err(|e| e.to_string())?;
                let parts = prepare_archive(archive, world.resource::<AppTypeRegistry>())?;
                // Explicit destructive prototype action, after all partitions validate.
                // No real documents/processes live in this app.
                let roots: Vec<_> = world
                    .query_filtered::<Entity, With<Workspace>>()
                    .iter(world)
                    .collect();
                for e in roots {
                    world.despawn(e);
                }
                let globals = std::mem::take(&mut world.resource_mut::<Controls>().globals);
                for e in globals {
                    world.despawn(e);
                }
                let mut parts = parts.into_iter();
                let globals = persistence::restore(world, parts.next().unwrap())?;
                world.resource_mut::<Controls>().globals = globals;
                restore_archive(world, parts.collect())?;
                Ok("restored; runtime counters reset, selection/focus reapplied".into())
            }
        }
    })();
    world.resource_mut::<Controls>().message = result.unwrap_or_else(|e| format!("error: {e}"));
}
fn close_panels(mut requests: MessageReader<PanelCloseRequested>, mut commands: Commands) {
    for request in requests.read().cloned() {
        commands.queue(move |world: &mut World| {
            // Prototype notes have no external workers or document save gates.
            if let Err(e) = Workbench::for_entity(world, request.workspace.entity())
                .and_then(|mut workbench| workbench.commit_close(&request))
            {
                world.resource_mut::<Controls>().message = format!("close rejected: {e}");
            }
        });
    }
}
fn setup(world: &mut World) {
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    world.entity_mut(window).insert((
        WorkbenchSurface::default(),
        Titlebar {
            height: 68.0,
            draw: toolbar,
        },
    ));
    world.spawn((Camera2d, WorkbenchCamera));
    seed(world);
    let globals: Vec<_> = world
        .query_filtered::<Entity, (
            Or<(With<Preferences>, With<BindingPreferences>)>,
            Without<InWorkspace>,
            Without<Workspace>,
        )>()
        .iter(world)
        .collect();
    world.resource_mut::<Controls>().globals = globals;
}

/// A renderer-free executable smoke route, sharing the same capture/load code.
fn roundtrip() -> Result<(), String> {
    let mut source = App::new();
    register(&mut source);
    seed(source.world_mut());
    let archive = capture_archive(source.world_mut())?;
    let mut destination = App::new();
    register(&mut destination);
    for _ in 0..20 {
        destination.world_mut().spawn_empty();
    }
    let parts = prepare_archive(archive, destination.world().resource::<AppTypeRegistry>())?;
    restore_archive(destination.world_mut(), parts)?;
    let world = destination.world_mut();
    let count = world.query::<&Notes>().iter(world).count();
    if count != 8 {
        return Err(format!("Expected eight restored panels, got {count}"));
    }
    if world
        .query::<&RuntimeOnly>()
        .iter(world)
        .any(|r| r.clicks != 0)
    {
        return Err("Runtime state was persisted".into());
    }
    println!(
        "round trip passed: global state, 2 workspaces, 4 areas, {count} concrete panels; required runtime components recreated"
    );
    Ok(())
}
#[derive(Resource, Default)]
struct Smoke {
    frame: u32,
    captured: bool,
}
fn smoke(world: &mut World) {
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
    world.resource_mut::<Smoke>().frame += 1;
    let frame = world.resource::<Smoke>().frame;
    match frame {
        5 => world.resource_mut::<Controls>().action = Some(Action::Save),
        8 => {
            assert!(world.resource::<Controls>().message.starts_with("saved"));
            for mut note in world.query::<&mut Notes>().iter_mut(world) {
                note.text = "unsaved mutation".into();
            }
            world.resource_mut::<Controls>().action = Some(Action::Load);
        }
        12 => {
            assert!(world.resource::<Controls>().message.starts_with("restored"));
            assert!(
                world
                    .query::<&Notes>()
                    .iter(world)
                    .all(|n| n.text != "unsaved mutation")
            );
            world
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("target/persist-world-smoke.png"))
                .observe(|_: On<ScreenshotCaptured>, mut s: ResMut<Smoke>| {
                    s.captured = true;
                });
        }
        _ => {}
    }
    if frame > 15 && world.resource::<Smoke>().captured {
        println!("native prototype smoke passed: save, mutate, load, render capture");
        world.write_message(AppExit::Success);
    }
    assert!(frame < 300, "Native smoke timed out");
}
fn main() -> AppExit {
    if std::env::args().any(|a| a == "--roundtrip") {
        return match roundtrip() {
            Ok(()) => AppExit::Success,
            Err(e) => {
                eprintln!("{e}");
                AppExit::error()
            }
        };
    }
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "workbench reflected world prototype".into(),
            resolution: (1200, 760).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((WorkbenchUiPlugin, UiPlugin));
    register(&mut app);
    if std::env::args().any(|a| a == "--smoke") {
        app.init_resource::<Smoke>()
            .add_systems(Update, smoke.after(actions));
    }
    app.insert_resource(Controls {
        path: if std::env::args().any(|a| a == "--smoke") {
            "target/persist-world-smoke.ron".into()
        } else {
            std::env::temp_dir()
                .join("bed2-persist-prototype.ron")
                .display()
                .to_string()
        },
        message: "isolated experiment; save/load only touches the file above".into(),
        action: None,
        preview: String::new(),
        globals: Vec::new(),
    })
    .add_systems(Startup, setup)
    .add_systems(Update, (actions, close_panels).chain())
    .run()
}
