//! A reusable desktop workbench without editor, terminal or content viewers.
use bed_desktop::{DesktopPlugins, DesktopSystems, DesktopUi};
use bed_workbench::{
    Action, CommandInvocation, Panel, WorkbenchPlugin, WorkbenchRequests, WorkbenchSystems,
    api::{CommandResult, PanelCreation, PanelResult},
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Resource)]
struct LayoutFile(PathBuf);
#[derive(Component)]
struct Counter(i64);
struct CounterPanel(Entity);
impl Panel for CounterPanel {
    fn title(&self, _world: &World) -> String {
        "Counter".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let count = world
            .get::<Counter>(self.0)
            .expect("counter panel component")
            .0;
        ui.text(format!("Count: {count}"));
        ui.text_disabled("This panel's state lives in the Bevy world.");
        if ui.button("Increment") {
            let owner = bed_workbench::api::panel_owner(world, self.0)
                .expect("counter frontend belongs to a live layout panel");
            let id = world
                .get::<bed_workbench::api::PanelPresentation>(owner)
                .expect("counter panel presentation")
                .id;
            let area = world
                .resource::<bed_workbench::WorkbenchLayout>()
                .tiling
                .area_for(id)
                .expect("counter panel area");
            requests.push(Action::Invoke(CommandInvocation {
                command: "layout.increment".into(),
                source: Some(self.0),
                area,
                arguments: Value::Null,
            }));
        }
        if ui.button("Split right") {
            requests.push(Action::SplitPanel {
                panel: self.0,
                axis: 0,
                high: true,
            });
        }
        ui.same_line();
        if ui.button("Split below") {
            requests.push(Action::SplitPanel {
                panel: self.0,
                axis: 1,
                high: true,
            });
        }
        if ui.button("Close") {
            requests.push(Action::ClosePanel(self.0));
        }
    }
    fn save(&self, world: &World) -> Value {
        json!({"count":world.get::<Counter>(self.0).unwrap().0})
    }
}
fn create_counter(In(input): In<PanelCreation>, world: &mut World) -> PanelResult {
    world
        .entity_mut(input.entity)
        .insert(Counter(input.state["count"].as_i64().unwrap_or(0)));
    Ok(Box::new(CounterPanel(input.entity)))
}
fn new_counter(In(input): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: "layout.counter".into(),
        state: json!({"count":0}),
        area: Some(input.area),
    }])
}
fn increment(
    In(input): In<CommandInvocation>,
    frontends: Query<&bed_workbench::api::PanelFrontend>,
    mut counters: Query<&mut Counter>,
) -> CommandResult {
    let source = input
        .source
        .map(|entity| frontends.get(entity).map_or(entity, |frontend| frontend.0));
    if let Some(entity) = source
        && let Ok(mut counter) = counters.get_mut(entity)
    {
        counter.0 += 1;
    }
    Ok(Vec::new())
}
fn save_layout(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    let path = world.resource::<LayoutFile>().0.clone();
    let value = bed_workbench::save(world);
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}
fn reset_layout(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    bed_workbench::restore(world, &initial_layout())?;
    Ok(Vec::new())
}
struct LayoutDemoPlugin;
impl Plugin for LayoutDemoPlugin {
    fn build(&self, app: &mut App) {
        bed_workbench::register(app.world_mut(), "layout", |registrar| {
            registrar.panel("layout.counter", "Counter", create_counter);
            registrar.command("layout.new", "New counter", new_counter);
            registrar.command("layout.increment", "Increment", increment);
            registrar.command("layout.save", "Save layout", save_layout);
            registrar.command("layout.reset", "Reset layout", reset_layout);
            registrar.menu("layout.new");
            registrar.menu("layout.save");
            registrar.menu("layout.reset");
        })
        .expect("valid layout contributions");
        app.add_systems(Startup, restore_layout);
    }
}
fn initial_layout() -> Value {
    let mut tiling = bed_workbench::workspace::tiling_state::TilingState::default();
    tiling.assign(1, 1);
    tiling.assign(2, 1);
    tiling.split_tab(2, 1, 0, true, [1200, 800]);
    json!({"version":1,"panels":[
        {"id":1,"panel_type":"layout.counter","state":{"count":0}},
        {"id":2,"panel_type":"layout.counter","state":{"count":10}}
    ],"focused":1,"tiling":tiling.to_value([1,2])})
}
fn restore_layout(world: &mut World) {
    let path = &world.resource::<LayoutFile>().0;
    let saved = if world.contains_resource::<Smoke>() {
        initial_layout()
    } else {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("Could not load layout: {error}");
                initial_layout()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => initial_layout(),
            Err(error) => {
                eprintln!("Could not read layout: {error}");
                initial_layout()
            }
        }
    };
    if let Err(error) = bed_workbench::restore(world, &saved) {
        eprintln!("Could not restore layout: {error}");
        bed_workbench::restore(world, &initial_layout()).expect("valid initial layout");
    }
}
fn draw(world: &mut World, ui: &Ui) {
    if let Some(_bar) = ui.begin_main_menu_bar() {
        let commands = world
            .resource::<bed_workbench::Registry>()
            .menus()
            .iter()
            .map(|id| {
                world
                    .resource::<bed_workbench::Registry>()
                    .command(id)
                    .unwrap()
                    .clone()
            })
            .collect::<Vec<_>>();
        for command in commands {
            if ui.menu_item(command.label) {
                let invocation = bed_workbench::capture_command(world, command.id, Value::Null);
                world
                    .resource_mut::<WorkbenchRequests>()
                    .push(Action::Invoke(invocation));
            }
        }
    }
    bed_workbench::render_ui(world, ui);
    let errors = world.resource_mut::<WorkbenchRequests>().take_errors();
    for error in errors {
        eprintln!("{error}");
    }
}
#[derive(Resource)]
struct Smoke {
    phase: u8,
    source: Option<Entity>,
    expected_count: i64,
    capture: Option<PathBuf>,
    started: Instant,
}
fn smoke(world: &mut World) {
    let frames = world
        .resource::<bed_desktop::DesktopRenderStatus>()
        .read()
        .completed_frames;
    assert!(
        world.resource::<Smoke>().started.elapsed() < Duration::from_secs(30),
        "layout smoke timed out"
    );
    match world.resource::<Smoke>().phase {
        0 if frames >= 3 => {
            let source = world
                .resource::<bed_workbench::WorkbenchLayout>()
                .focused
                .unwrap();
            let expected_count = world.get::<Counter>(source).unwrap().0 + 1;
            let invocation = bed_workbench::capture_command(world, "layout.increment", Value::Null);
            let mut requests = world.resource_mut::<WorkbenchRequests>();
            requests.push(Action::Invoke(invocation));
            requests.push(Action::SplitPanel {
                panel: source,
                axis: 1,
                high: true,
            });
            let mut smoke = world.resource_mut::<Smoke>();
            smoke.source = Some(source);
            smoke.expected_count = expected_count;
            smoke.phase = 1;
        }
        1 if frames >= 6 => {
            let source = world.resource::<Smoke>().source.unwrap();
            assert_eq!(
                world.get::<Counter>(source).unwrap().0,
                world.resource::<Smoke>().expected_count
            );
            assert_eq!(
                world
                    .resource::<bed_workbench::WorkbenchLayout>()
                    .tiling
                    .areas
                    .len(),
                3
            );
            let invocation = bed_workbench::capture_command(world, "layout.save", Value::Null);
            world
                .resource_mut::<WorkbenchRequests>()
                .push(Action::Invoke(invocation));
            world.resource_mut::<Smoke>().phase = 2;
        }
        2 if frames >= 8 => {
            let path = &world.resource::<LayoutFile>().0;
            let saved: Value =
                serde_json::from_slice(&std::fs::read(path).expect("smoke saved file")).unwrap();
            assert_eq!(saved["panels"].as_array().unwrap().len(), 2);
            let source = world.resource::<Smoke>().source.unwrap();
            bed_workbench::restore(world, &saved).expect("saved split layout restores");
            assert!(
                world.get::<Counter>(source).is_none(),
                "restored panels have fresh live entities"
            );
            assert_eq!(bed_workbench::save(world), saved);
            if let Some(path) = world.resource::<Smoke>().capture.clone() {
                world
                    .resource_mut::<bed_desktop::DesktopScreenshotRequest>()
                    .path = Some(path);
            }
            world.resource_mut::<Smoke>().phase = 3;
        }
        3 if frames >= 12 => {
            if world.resource::<Smoke>().capture.is_some() {
                let screenshot = world.resource::<bed_desktop::DesktopScreenshotResult>();
                assert!(screenshot.error.is_none(), "{:?}", screenshot.error);
                if screenshot.completed.is_none() {
                    return;
                }
            }
            println!(
                "Layout smoke passed: ECS commands, splitting, disk roundtrip and restored panels ({frames} presented frames)"
            );
            world.write_message(bevy::app::AppExit::Success);
            world.resource_mut::<Smoke>().phase = 4;
        }
        _ => {}
    }
}
fn main() {
    let mut path = None;
    let mut smoke_test = false;
    let mut capture = None;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--smoke-test") => smoke_test = true,
            Some("--capture-frame") => {
                capture = Some(PathBuf::from(
                    arguments.next().expect("--capture-frame needs a path"),
                ))
            }
            Some("--layout") => {
                path = Some(PathBuf::from(
                    arguments.next().expect("--layout needs a path"),
                ))
            }
            _ => path = Some(PathBuf::from(argument)),
        }
    }
    let path = path.unwrap_or_else(|| {
        if smoke_test {
            std::env::temp_dir().join(format!("bed-layout-smoke-{}.json", std::process::id()))
        } else {
            PathBuf::from("layout-app.json")
        }
    });
    let mut app = App::new();
    app.add_plugins(DesktopPlugins {
        window: Window {
            title: "Layout workbench".into(),
            ..default()
        },
        ..default()
    })
    .add_plugins((WorkbenchPlugin, LayoutDemoPlugin))
    .insert_resource(LayoutFile(path))
    .insert_resource(DesktopUi(draw))
    .configure_sets(
        PreUpdate,
        WorkbenchSystems::Requests.in_set(DesktopSystems::AfterUi),
    );
    if smoke_test {
        app.insert_resource(Smoke {
            phase: 0,
            source: None,
            expected_count: 0,
            capture,
            started: Instant::now(),
        })
        .add_systems(Update, smoke);
    }
    app.run();
}
