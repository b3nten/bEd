//! A reusable desktop workbench without editor, terminal or content viewers.
use bedtime_desktop::DesktopPlugins;
use bedtime_plugin_layouts::{
    LayoutId, LayoutIntegration, LayoutSlots, LayoutsPlugin, WorkbenchLayouts,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use {
    bedtime_workbench::CommandInvocation, bedtime_workbench::Panel,
    bedtime_workbench::WorkbenchAction, bedtime_workbench::WorkbenchRequests,
    bedtime_workbench::api::CommandResult, bedtime_workbench::api::PanelCreation,
    bedtime_workbench::api::PanelResult,
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
        _shortcuts: &bedtime_api::workbench::ShortcutInput,
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
            let owner = bedtime_workbench::api::panel_owner(world, self.0)
                .expect("counter frontend belongs to a live layout panel");
            let id = world
                .get::<bedtime_workbench::api::PanelPresentation>(owner)
                .expect("counter panel presentation")
                .id;
            let area = world
                .resource::<bedtime_workbench::WorkbenchLayout>()
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
    frontends: Query<&bedtime_workbench::api::PanelFrontend>,
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
    let value = bedtime_plugin_layouts::save(world);
    let bytes = serde_json::to_vec_pretty(&value).map_err(io::Error::other)?;
    std::fs::write(path, bytes)
}

fn save_layout(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    persist(world).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}

fn switch(world: &mut World, slot: LayoutId) -> io::Result<()> {
    if !world.resource::<SupportedLayouts>().0.contains(&slot) {
        return Err(io::Error::other("This application has no such layout"));
    }
    if world.resource::<LayoutSlots>().active() == slot {
        return Ok(());
    }
    bedtime_plugin_layouts::transition::<LayoutId, WorkbenchLayouts>(world, slot, &initial_layout())
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
    bedtime_plugin_layouts::rearrange::<WorkbenchLayouts>(world, &initial_definition())
        .map_err(|error| error.to_string())?;
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
        // This app splits by moving counters into new areas. Select the plain
        // workbench behavior even when launched with the document foundation.
        app.world_mut()
            .remove_resource::<bedtime_workbench::WorkbenchSplitPolicy>();
        app.insert_resource(SupportedLayouts(
            (1..=self.slots)
                .map(|number| LayoutId::new(number).unwrap())
                .collect(),
        ));
        bedtime_workbench::register(app.world_mut(), "layout", |registrar| {
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
        let writer = app
            .world_mut()
            .register_system(|world: &mut World| -> Result<(), String> {
                persist(world).map_err(|error| error.to_string())
            });
        app.insert_resource(bedtime_plugin_layouts::LayoutPersistence(writer));
        app.add_systems(Startup, restore_layout);
    }
}
fn initial_definition() -> bedtime_plugin_layouts::LayoutDefinition {
    use bedtime_plugin_layouts::{LayoutDefinition, PanelDefinition};
    let mut geometry = bedtime_workbench::workspace::tiling::Layout::default();
    let right = geometry.split(1, 0, 5000, 1).unwrap();
    let mut definition = LayoutDefinition::new(geometry);
    let first = definition
        .add_panel(
            PanelDefinition::new("layout.counter").initial_state(json!({"count":0})),
            1,
        )
        .unwrap();
    definition
        .add_panel(
            PanelDefinition::new("layout.counter").initial_state(json!({"count":10})),
            right,
        )
        .unwrap();
    definition.focus(first).unwrap();
    definition
}
fn initial_layout() -> Value {
    initial_definition().snapshot()
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
        .query::<(Entity, &bedtime_workbench::PanelPresentation)>()
        .iter(world)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    WorkbenchLayouts::retire(world, &panels)?;
    world.insert_resource(LayoutSlots::<LayoutId>::from_saved(active, saved));
    bedtime_plugin_layouts::restore(world, &selected)?;
    bedtime_workbench::advance_layout_epoch(world);
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
        bedtime_workbench::WorkbenchPlugin,
        LayoutsPlugin,
        LayoutDemoPlugin { slots: 3 },
    ))
    .insert_resource(LayoutFile(path));
    app.update();
    app
}

#[derive(Resource)]
struct Smoke {
    verified: Arc<AtomicBool>,
    phase: u8,
    source: Option<Entity>,
    expected_count: i64,
    capture: Option<PathBuf>,
    started: Instant,
}
fn counter(world: &World, panel: Entity) -> Option<&Counter> {
    let frontend = world
        .get::<bedtime_api::workbench::PanelFrontend>(panel)
        .map_or(panel, |frontend| frontend.0);
    world.get::<Counter>(frontend)
}

fn smoke(world: &mut World) {
    let frames = world
        .resource::<bedtime_desktop::DesktopRenderStatus>()
        .read()
        .completed_frames;
    assert!(
        world.resource::<Smoke>().started.elapsed() < Duration::from_secs(30),
        "layout smoke timed out"
    );
    match world.resource::<Smoke>().phase {
        0 if frames >= 3 => {
            let source = world
                .resource::<bedtime_workbench::WorkbenchLayout>()
                .focused
                .unwrap();
            let expected_count = counter(world, source).unwrap().0 + 1;
            let invocation =
                bedtime_workbench::capture_command(world, "layout.increment", Value::Null);
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
                counter(world, source).unwrap().0,
                world.resource::<Smoke>().expected_count
            );
            assert_eq!(
                world
                    .resource::<bedtime_workbench::WorkbenchLayout>()
                    .tiling
                    .areas
                    .len(),
                3
            );
            let invocation = bedtime_workbench::capture_command(world, "layout.use.2", Value::Null);
            // Docking the split can change native focus. Explicitly focus the
            // retained source before switching, like an application command.
            let mut requests = world.resource_mut::<WorkbenchRequests>();
            requests.push(WorkbenchAction::FocusPanel(source));
            requests.push_unscoped(WorkbenchAction::Invoke(invocation));
            world.resource_mut::<Smoke>().phase = 2;
        }
        2 if frames >= 8 => {
            let source = world.resource::<Smoke>().source.unwrap();
            assert_eq!(world.resource::<LayoutSlots>().active().number(), 2);
            assert!(counter(world, source).is_some());
            assert!(!bedtime_api::workbench::panel_active(world, source));
            let increment =
                bedtime_workbench::capture_command(world, "layout.increment", Value::Null);
            let back = bedtime_workbench::capture_command(world, "layout.use.1", Value::Null);
            let mut requests = world.resource_mut::<WorkbenchRequests>();
            requests.push(WorkbenchAction::Invoke(increment));
            requests.push_unscoped(WorkbenchAction::Invoke(back));
            world.resource_mut::<Smoke>().phase = 3;
        }
        3 if frames >= 10 => {
            let source = world.resource::<Smoke>().source.unwrap();
            assert_eq!(world.resource::<LayoutSlots>().active().number(), 1);
            assert!(bedtime_api::workbench::panel_active(world, source));
            assert_eq!(
                counter(world, source).unwrap().0,
                world.resource::<Smoke>().expected_count
            );
            assert_eq!(
                world
                    .resource::<bedtime_workbench::WorkbenchLayout>()
                    .focused,
                Some(source)
            );
            let invocation = bedtime_workbench::capture_command(world, "layout.save", Value::Null);
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
            assert_eq!(bedtime_plugin_layouts::save(restarted.world_mut()), saved);
            switch(restarted.world_mut(), LayoutId::new(2).unwrap()).unwrap();
            assert!(
                bedtime_plugin_layouts::active_panels(restarted.world_mut())
                    .into_iter()
                    .any(|panel| counter(restarted.world(), panel).unwrap().0 == 1)
            );
            switch(restarted.world_mut(), LayoutId::default()).unwrap();
            let source = world.resource::<Smoke>().source.unwrap();
            restore_saved(world, &saved).expect("saved layouts restore");
            assert!(
                counter(world, source).is_none(),
                "restored panels have fresh live entities"
            );
            assert_eq!(bedtime_plugin_layouts::save(world), saved);
            if let Some(path) = world.resource::<Smoke>().capture.clone() {
                world
                    .resource_mut::<bedtime_desktop::DesktopScreenshotRequest>()
                    .path = Some(path);
            }
            world.resource_mut::<Smoke>().phase = 5;
        }
        5 if frames >= 16 => {
            if world.resource::<Smoke>().capture.is_some() {
                let screenshot = world.resource::<bedtime_desktop::DesktopScreenshotResult>();
                assert!(screenshot.error.is_none(), "{:?}", screenshot.error);
                if screenshot.completed.is_none() {
                    return;
                }
            }
            println!(
                "Layout smoke passed: ECS commands, splitting, live slot retention, disk restart and fresh panels ({frames} presented frames)"
            );
            world
                .resource::<Smoke>()
                .verified
                .store(true, Ordering::SeqCst);
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
    let launch = match bedtime::awaken("example.layouts", "Layout workbench")?.dispatch()? {
        bedtime::LaunchOutcome::Desktop(launch) => *launch,
        bedtime::LaunchOutcome::Handled(exit) => std::process::exit(exit),
    };
    let options = bedtime::DesktopOptions {
        desktop: DesktopPlugins {
            window: Window {
                title: "Layout workbench".into(),
                ..default()
            },
            ..default()
        },
        config_directory: smoke_test.then(|| {
            std::env::temp_dir().join(format!("bed-layout-example-config-{}", std::process::id()))
        }),
        ..Default::default()
    };
    let verified = Arc::<AtomicBool>::default();
    let exit = launch.run(options, |app| {
        app.add_plugins((LayoutsPlugin, LayoutDemoPlugin { slots: 3 }))
            .insert_resource(LayoutFile(path));
        if smoke_test {
            app.insert_resource(Smoke {
                verified: Arc::clone(&verified),
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
    if smoke_test {
        assert!(
            verified.load(Ordering::SeqCst),
            "layout smoke exited before completing its assertions"
        );
    }
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
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        original.world_mut().get_mut::<Counter>(first).unwrap().0 = 37;
        switch(original.world_mut(), LayoutId::new(3).unwrap()).unwrap();
        let third = original
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
            .focused
            .unwrap();
        original.world_mut().get_mut::<Counter>(third).unwrap().0 = 81;
        switch(original.world_mut(), LayoutId::default()).unwrap();
        assert_eq!(
            original
                .world()
                .resource::<bedtime_workbench::WorkbenchLayout>()
                .focused,
            Some(first)
        );
        assert_eq!(original.world().get::<Counter>(first).unwrap().0, 37);
        let saved = bedtime_plugin_layouts::save(original.world_mut());
        drop(original);
        let mut restored = restarted_app(path.clone());
        assert_eq!(bedtime_plugin_layouts::save(restored.world_mut()), saved);
        switch(restored.world_mut(), LayoutId::new(3).unwrap()).unwrap();
        let focus = restored
            .world()
            .resource::<bedtime_workbench::WorkbenchLayout>()
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
            .resource::<bedtime_workbench::WorkbenchLayout>()
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
