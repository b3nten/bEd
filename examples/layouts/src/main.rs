//! A reusable desktop workbench without editor, terminal or content viewers.
use bed_desktop::DesktopPlugins;
use bed_plugin_layouts::{
    LayoutId, LayoutIntegration, LayoutSlots, LayoutsPlugin, WorkbenchLayouts,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};
use {
    bed_workbench::CommandInvocation, bed_workbench::Panel, bed_workbench::WorkbenchAction,
    bed_workbench::WorkbenchRequests, bed_workbench::api::CommandResult,
    bed_workbench::api::PanelCreation, bed_workbench::api::PanelResult,
};

#[derive(Resource)]
struct LayoutFile(PathBuf);
#[derive(Resource)]
struct SupportedLayouts(Vec<LayoutId>);
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
        requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let count = world
            .get::<Counter>(self.0)
            .expect("counter panel component")
            .0;
        ui.text(format!("Count: {count}"));
        ui.text(format!(
            "Layout {}",
            world.resource::<LayoutSlots>().active().number()
        ));
        ui.text_disabled("Switch layouts to retain separate counters and arrangements.");
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
            requests.push(WorkbenchAction::Invoke(CommandInvocation {
                command: "layout.increment".into(),
                source: Some(self.0),
                area,
                arguments: Value::Null,
            }));
        }
        if ui.button("Split right") {
            requests.push(WorkbenchAction::SplitPanel {
                panel: self.0,
                axis: 0,
                high: true,
            });
        }
        ui.same_line();
        if ui.button("Split below") {
            requests.push(WorkbenchAction::SplitPanel {
                panel: self.0,
                axis: 1,
                high: true,
            });
        }
        if ui.button("Close") {
            requests.push(WorkbenchAction::ClosePanel(self.0));
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
    Ok(vec![WorkbenchAction::OpenPanel {
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
fn persist(world: &mut World) -> io::Result<()> {
    let path = world.resource::<LayoutFile>().0.clone();
    let value = bed_plugin_layouts::save(world);
    let bytes = serde_json::to_vec_pretty(&value).map_err(io::Error::other)?;
    std::fs::write(path, bytes)
}

fn save_layout(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    persist(world).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

struct DemoLayouts;
impl LayoutIntegration for DemoLayouts {
    type Context = ();
    fn capture_context(_: &World) {}
    fn activate_context(world: &mut World, context: Option<&()>) -> io::Result<()> {
        WorkbenchLayouts::activate_context(world, context)
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        bed_plugin_layouts::restore(world, value)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        WorkbenchLayouts::retire(world, panels)
    }
    fn finish(world: &mut World) -> io::Result<()> {
        persist(world)
    }
}

fn switch(world: &mut World, slot: LayoutId) -> io::Result<()> {
    if !world.resource::<SupportedLayouts>().0.contains(&slot) {
        return Err(io::Error::other("This application has no such layout"));
    }
    if world.resource::<LayoutSlots>().active() == slot {
        return Ok(());
    }
    persist(world)?;
    bed_plugin_layouts::transition::<LayoutId, DemoLayouts>(world, slot, &initial_layout())
}

fn switch_layout(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let number = input
        .command
        .rsplit('.')
        .next()
        .and_then(|number| number.parse().ok())
        .and_then(LayoutId::new)
        .ok_or("Invalid layout command")?;
    switch(world, number).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

fn reset_layout(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    let panels = bed_plugin_layouts::active_panels(world);
    WorkbenchLayouts::retire(world, &panels).map_err(|error| error.to_string())?;
    bed_plugin_layouts::restore(world, &initial_layout()).map_err(|error| error.to_string())?;
    bed_workbench::advance_layout_epoch(world);
    persist(world).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

struct LayoutDemoPlugin {
    slots: u32,
}
impl Plugin for LayoutDemoPlugin {
    fn build(&self, app: &mut App) {
        // The example chooses three menu entries; the library accepts arbitrary
        // positive numbered IDs or application-defined LayoutKey enums.
        assert!((1..=3).contains(&self.slots));
        app.insert_resource(SupportedLayouts(
            (1..=self.slots)
                .map(|number| LayoutId::new(number).unwrap())
                .collect(),
        ));
        bed_workbench::register(app.world_mut(), "layout", |registrar| {
            registrar.panel("layout.counter", "Counter", create_counter);
            registrar.command("layout.new", "New counter", new_counter);
            registrar.command("layout.increment", "Increment", increment);
            registrar.command("layout.save", "Save all layouts", save_layout);
            registrar.command("layout.reset", "Reset current layout", reset_layout);
            for (id, label) in [
                ("layout.use.1", "Layout 1"),
                ("layout.use.2", "Layout 2"),
                ("layout.use.3", "Layout 3"),
            ]
            .into_iter()
            .take(self.slots as usize)
            {
                registrar.command(id, label, switch_layout);
                registrar.menu(id);
            }
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
fn restore_saved(world: &mut World, value: &Value) -> io::Result<()> {
    let (active, saved) = if value.get("panels").is_some() {
        // Read the example's original single-layout file without changing its
        // panel schema; its first subsequent save writes the numbered wrapper.
        (
            LayoutId::default(),
            serde_json::Map::from_iter([("1".into(), value.clone())]),
        )
    } else {
        let active = value["active_layout_slot"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .and_then(LayoutId::new)
            .ok_or_else(|| io::Error::other("Invalid active layout"))?;
        if !world.resource::<SupportedLayouts>().0.contains(&active) {
            return Err(io::Error::other(
                "Saved active layout is unavailable in this application",
            ));
        }
        let saved = value["layouts"]
            .as_object()
            .cloned()
            .ok_or_else(|| io::Error::other("Saved layouts must be an object"))?;
        (active, saved)
    };
    let selected = saved
        .get(&active.number().to_string())
        .cloned()
        .unwrap_or_else(initial_layout);
    let panels = world
        .query::<(Entity, &bed_workbench::PanelPresentation)>()
        .iter(world)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    WorkbenchLayouts::retire(world, &panels)?;
    world.insert_resource(LayoutSlots::<LayoutId>::from_saved(active, saved));
    bed_plugin_layouts::restore(world, &selected)?;
    bed_workbench::advance_layout_epoch(world);
    Ok(())
}

fn restore_layout(world: &mut World) {
    let path = world.resource::<LayoutFile>().0.clone();
    let saved = if world.contains_resource::<Smoke>() {
        initial_layout()
    } else {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("Could not load layouts: {error}");
                initial_layout()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => initial_layout(),
            Err(error) => {
                eprintln!("Could not read layouts: {error}");
                initial_layout()
            }
        }
    };
    if let Err(error) = restore_saved(world, &saved) {
        eprintln!("Could not restore layouts: {error}");
        restore_saved(world, &initial_layout()).expect("valid initial layout");
    }
}

fn restarted_app(path: PathBuf) -> App {
    let mut app = App::new();
    app.add_plugins((
        bed_workbench::WorkbenchPlugin,
        LayoutsPlugin,
        LayoutDemoPlugin { slots: 3 },
    ))
    .insert_resource(LayoutFile(path));
    app.update();
    app
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
            requests.push(WorkbenchAction::Invoke(invocation));
            requests.push(WorkbenchAction::SplitPanel {
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
            let invocation = bed_workbench::capture_command(world, "layout.use.2", Value::Null);
            world
                .resource_mut::<WorkbenchRequests>()
                .push_unscoped(WorkbenchAction::Invoke(invocation));
            world.resource_mut::<Smoke>().phase = 2;
        }
        2 if frames >= 8 => {
            let source = world.resource::<Smoke>().source.unwrap();
            assert_eq!(world.resource::<LayoutSlots>().active().number(), 2);
            assert!(world.get::<Counter>(source).is_some());
            assert!(!bed_api::workbench::panel_active(world, source));
            let increment = bed_workbench::capture_command(world, "layout.increment", Value::Null);
            let back = bed_workbench::capture_command(world, "layout.use.1", Value::Null);
            let mut requests = world.resource_mut::<WorkbenchRequests>();
            requests.push(WorkbenchAction::Invoke(increment));
            requests.push_unscoped(WorkbenchAction::Invoke(back));
            world.resource_mut::<Smoke>().phase = 3;
        }
        3 if frames >= 10 => {
            let source = world.resource::<Smoke>().source.unwrap();
            assert_eq!(world.resource::<LayoutSlots>().active().number(), 1);
            assert!(bed_api::workbench::panel_active(world, source));
            assert_eq!(
                world.get::<Counter>(source).unwrap().0,
                world.resource::<Smoke>().expected_count
            );
            assert_eq!(
                world.resource::<bed_workbench::WorkbenchLayout>().focused,
                Some(source)
            );
            let invocation = bed_workbench::capture_command(world, "layout.save", Value::Null);
            world
                .resource_mut::<WorkbenchRequests>()
                .push_unscoped(WorkbenchAction::Invoke(invocation));
            world.resource_mut::<Smoke>().phase = 4;
        }
        4 if frames >= 12 => {
            let path = world.resource::<LayoutFile>().0.clone();
            let saved: Value =
                serde_json::from_slice(&std::fs::read(&path).expect("smoke saved file")).unwrap();
            assert_eq!(saved["layouts"]["1"]["panels"].as_array().unwrap().len(), 2);
            assert_eq!(saved["layouts"]["2"]["panels"].as_array().unwrap().len(), 2);
            let mut restarted = restarted_app(path);
            assert_eq!(bed_plugin_layouts::save(restarted.world_mut()), saved);
            switch(restarted.world_mut(), LayoutId::new(2).unwrap()).unwrap();
            assert!(
                bed_plugin_layouts::active_panels(restarted.world_mut())
                    .into_iter()
                    .any(|panel| restarted.world().get::<Counter>(panel).unwrap().0 == 1)
            );
            switch(restarted.world_mut(), LayoutId::default()).unwrap();
            let source = world.resource::<Smoke>().source.unwrap();
            restore_saved(world, &saved).expect("saved layouts restore");
            assert!(
                world.get::<Counter>(source).is_none(),
                "restored panels have fresh live entities"
            );
            assert_eq!(bed_plugin_layouts::save(world), saved);
            if let Some(path) = world.resource::<Smoke>().capture.clone() {
                world
                    .resource_mut::<bed_desktop::DesktopScreenshotRequest>()
                    .path = Some(path);
            }
            world.resource_mut::<Smoke>().phase = 5;
        }
        5 if frames >= 16 => {
            if world.resource::<Smoke>().capture.is_some() {
                let screenshot = world.resource::<bed_desktop::DesktopScreenshotResult>();
                assert!(screenshot.error.is_none(), "{:?}", screenshot.error);
                if screenshot.completed.is_none() {
                    return;
                }
            }
            println!(
                "Layout smoke passed: ECS commands, splitting, live slot retention, disk restart and fresh panels ({frames} presented frames)"
            );
            world.write_message(bevy::app::AppExit::Success);
            world.resource_mut::<Smoke>().phase = 6;
        }
        _ => {}
    }
}
fn main() -> io::Result<()> {
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
    let launch = match bed_app::Application::new("example.layouts", "Layout workbench")?
        .dispatch(std::env::args_os())?
    {
        bed_app::LaunchOutcome::Desktop(launch) => *launch,
        bed_app::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let launch = launch.options(DesktopPlugins {
        window: Window {
            title: "Layout workbench".into(),
            ..default()
        },
        ..default()
    });
    let launch = if smoke_test {
        launch.config_directory(
            std::env::temp_dir().join(format!("bed-layout-example-config-{}", std::process::id())),
        )
    } else {
        launch
    };
    let exit = launch.run(|app| {
        app.add_plugins((LayoutsPlugin, LayoutDemoPlugin { slots: 3 }))
            .insert_resource(LayoutFile(path));
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
        Ok(())
    })?;
    if exit.is_error() {
        return Err(io::Error::other("Layout example exited with an error"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        std::env::temp_dir().join(format!(
            "bed-layout-app-test-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn application_storage_restores_multiple_layouts_after_restart() {
        let path = path();
        let mut original = restarted_app(path.clone());
        let first = original
            .world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        original.world_mut().get_mut::<Counter>(first).unwrap().0 = 37;
        switch(original.world_mut(), LayoutId::new(3).unwrap()).unwrap();
        let third = original
            .world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        original.world_mut().get_mut::<Counter>(third).unwrap().0 = 81;
        switch(original.world_mut(), LayoutId::default()).unwrap();
        assert_eq!(
            original
                .world()
                .resource::<bed_workbench::WorkbenchLayout>()
                .focused,
            Some(first)
        );
        assert_eq!(original.world().get::<Counter>(first).unwrap().0, 37);
        let saved = bed_plugin_layouts::save(original.world_mut());
        drop(original);
        let mut restored = restarted_app(path.clone());
        assert_eq!(bed_plugin_layouts::save(restored.world_mut()), saved);
        switch(restored.world_mut(), LayoutId::new(3).unwrap()).unwrap();
        let focus = restored
            .world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        assert_eq!(restored.world().get::<Counter>(focus).unwrap().0, 81);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn original_single_layout_files_migrate_on_the_next_save() {
        let path = path();
        let mut legacy = initial_layout();
        legacy["panels"][0]["state"]["count"] = json!(123);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut app = restarted_app(path.clone());
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            LayoutId::default()
        );
        let focus = app
            .world()
            .resource::<bed_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        assert_eq!(app.world().get::<Counter>(focus).unwrap().0, 123);
        persist(app.world_mut()).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["active_layout_slot"], 1);
        assert_eq!(saved["layouts"]["1"]["panels"], legacy["panels"]);
        assert_eq!(saved["layouts"]["1"]["tiling"], legacy["tiling"]);
        assert_eq!(saved["layouts"]["1"]["focused"], legacy["focused"]);
        std::fs::remove_file(path).unwrap();
    }
}
