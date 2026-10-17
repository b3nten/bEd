//! Application policy lives in the host; plugin state lives in each workspace.
use bedtime_ui2::{UiFrame, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::time::Duration;

#[derive(Resource)]
pub(crate) struct DemoWorkbench {
    pub host: Entity,
    pub surface: Entity,
    pub gallery: bedtime_ui2::gallery::GalleryState,
}
#[derive(Resource, Default)]
pub(crate) struct Session {
    pub ticks: u64,
    pub angle: f32,
}
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Session>()
        .add_systems(Update, |mut session: ResMut<Session>| {
            session.ticks += 1;
            session.angle += 0.008;
        })
        .add_systems(Update, collect);
}
#[derive(Component)]
pub(crate) struct Notes {
    pub text: String,
}
impl PanelDefinition for Notes {
    const KEY: &'static str = "bed2.notes";
    const LABEL: &'static str = "Notes";
}
#[derive(Component, Default)]
pub(crate) struct Jobs {
    task: Option<WorkspaceTask<u64>>,
    process: Option<WorkspaceProcess>,
    pub output: String,
    input: String,
    result: String,
}
impl PanelDefinition for Jobs {
    const KEY: &'static str = "bed2.jobs";
    const LABEL: &'static str = "Background work";
}
pub(crate) fn runtime(
    name: &str,
    image: Handle<Image>,
    tools: crate::tool_panels::PanelTools,
) -> WorkspaceRuntime {
    let mut runtime = WorkspaceRuntime::new(name);
    runtime.add_plugins(plugin);
    let world = runtime.world_mut();
    crate::tool_panels::install(world, tools);
    let mut w = Workbench::new(world);
    w.register_panel_type::<Notes, _>(notes_factory).unwrap();
    w.register_panel_type::<Jobs, _>(jobs_factory).unwrap();
    let layout = w.active_layout();
    let area = w.destination_area(layout).unwrap();
    let notes = notes_factory(In(area), world).unwrap();
    world.get_mut::<Notes>(notes).unwrap().text = format!(
        "Notes for {name}\n\nEdit this text, switch workspaces, and return.\nEach workspace has its own entities, resources and systems."
    );
    let jobs = jobs_factory(In(area), world).unwrap();
    Workbench::new(world)
        .move_panel(jobs, notes, Some(SplitAxis::Horizontal))
        .unwrap();
    let viewport = crate::viewport_demo::spawn(world, layout, image);
    Workbench::new(world)
        .move_panel(viewport, jobs, Some(SplitAxis::Vertical))
        .unwrap();
    let reading = Workbench::new(world).create_layout("Reading").unwrap();
    let area = Workbench::new(world).destination_area(reading).unwrap();
    notes_factory(In(area), world).unwrap();
    setup_menu(world);
    crate::status_demo::setup(world);
    Workbench::new(world).focus_panel(notes).unwrap();
    runtime
}
fn notes_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, "Notes", draw_notes)?;
    world.entity_mut(panel).insert(Notes {
        text: String::new(),
    });
    Ok(panel)
}
fn jobs_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, "Background work", draw_jobs)?;
    world.entity_mut(panel).insert(Jobs::default());
    Ok(panel)
}
fn draw_notes(mut frame: UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    let padding = frame.panel_settings.padding;
    frame.padded(padding, |mut frame| {
        frame.ui.heading("Workspace notes");
        frame.text_edit(
            egui::TextEdit::multiline(&mut notes.text)
                .desired_width(f32::INFINITY)
                .desired_rows(22),
        );
    });
}

fn draw_jobs(mut frame: UiFrame, mut jobs: Query<&mut Jobs>, mut work: ResMut<WorkspaceWork>) {
    let panel = frame.view;
    let mut jobs = jobs.get_mut(panel).unwrap();
    let padding = frame.panel_settings.padding;
    frame.padded(padding, |mut frame| {
        frame.ui.heading("Owned background work");
        frame.ui.label("Work continues while this workspace is inactive. Closing this panel cancels its work.");
        frame.ui.horizontal(|ui| {
            if ui.button("Run 2-second task").clicked() {
                if let Some(task) = &jobs.task { task.cancel(); }
                jobs.result = "Working…".into();
                match work.spawn_task(Some(panel), "Example calculation", |cancel| {
                    if cancel.wait(Duration::from_secs(2)) { 0 } else { 42 }
                }) {
                    Ok(task) => jobs.task = Some(task), Err(error) => jobs.result = error,
                }
            }
            if ui.button("Cancel task").clicked() {
                if let Some(task) = &jobs.task { task.cancel(); }
            }
        });
        frame.ui.label(&jobs.result);
        frame.ui.separator();
        frame.ui.horizontal(|ui| {
            if ui.button("Start process").clicked() {
                if let Some(process) = &jobs.process { process.cancel(); }
                #[cfg(unix)]
                let mut spec = ProcessSpec::new("/bin/sh");
                #[cfg(unix)]
                { spec.arguments = ["-c", "printf 'Process ready. Type a line below.\\n'; while IFS= read -r line; do printf 'echo: %s\\n' \"$line\"; done"].map(Into::into).to_vec(); }
                #[cfg(windows)]
                let mut spec = ProcessSpec::new("cmd.exe");
                #[cfg(windows)]
                { spec.arguments = ["/Q"].map(Into::into).to_vec(); }
                match work.spawn_process(Some(panel), "Demo pipe process", spec) {
                    Ok(process) => { jobs.output.clear(); jobs.process = Some(process); },
                    Err(error) => jobs.output.push_str(&error),
                }
            }
            if ui.button("Stop process").clicked() {
                if let Some(process) = &jobs.process { process.cancel(); }
            }
        });
        if let Some(process) = &jobs.process { frame.ui.weak(format!("PID {} · {:?}", process.process_id, process.status())); }
        frame.ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut jobs.input);
            if ui.button("Send").clicked() {
                if let Some(process) = &jobs.process {
                    if let Err(error) = process.write(format!("{}\n", jobs.input).as_bytes()) { jobs.output.push_str(&format!("{error}\n")); }
                }
                jobs.input.clear();
            }
        });
        frame.ui.weak("This example uses pipes. A terminal plugin would supply a PTY backend.");
        frame.scroll("process-output", |frame| { frame.ui.monospace(&jobs.output); });
    });
}
fn collect(mut jobs: Query<&mut Jobs>) {
    for mut jobs in &mut jobs {
        if let Some(task) = &jobs.task {
            if let Ok(value) = task.try_recv() {
                jobs.result = format!("Result: {value}");
                jobs.task = None;
            } else if task.status() != WorkStatus::Running {
                jobs.result = format!("Task: {:?}", task.status());
            }
        }
        let output = jobs
            .process
            .as_ref()
            .map(|process| process.drain_output())
            .unwrap_or_default();
        for chunk in output {
            jobs.output.push_str(&String::from_utf8_lossy(&chunk.bytes));
        }
        if jobs.output.len() > 32_768 {
            let mut start = jobs.output.len() - 16_384;
            while !jobs.output.is_char_boundary(start) {
                start += 1;
            }
            jobs.output.drain(..start);
        }
    }
}
struct Quit;
impl ContextCommand for Quit {
    const KEY: &'static str = "bed2.quit";
    const LABEL: &'static str = "Quit bEd 2";
}
fn setup_menu(world: &mut World) {
    Workbench::new(world)
        .register_handler::<Quit, _>(
            |_: In<ContextCommandInvocation>,
             mut close: MessageWriter<CloseRequested>|
             -> ContextCommandResult {
                close.write(CloseRequested { window: None });
                Ok(())
            },
        )
        .unwrap();
    world.spawn(Binding::new::<Quit>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyQ,
    )));
    let root = world.spawn(MenuBar).id();
    if !std::env::args().any(|arg| arg == "--egui-menu" || arg == "--compose-workbench") {
        world.entity_mut(root).insert(NativeMenuBar);
    }
    let menu = world
        .spawn((
            Menu {
                label: "bEd 2".into(),
            },
            InMenu(root),
        ))
        .id();
    world.spawn((
        MenuItem::new(Quit::LABEL),
        InMenu(menu),
        Invokes::of::<Quit>(),
    ));
    Workbench::new(world).set_menu(Some(root)).unwrap();
}
/// Host controls deliberately select other workspace worlds through the explicit API.
pub(crate) fn draw_controls(frame: UiFrame, host: &mut World) {
    crate::tool_panels::sync(host);
    let workspaces = Workspaces::new(host).list();
    let active = Workspaces::new(host).active();
    let mut action = None;
    let mut create = false;
    let mut background = None;
    let mut layout_action = None;
    let (current, layouts, name, ticks) = {
        let mut w = Workbench::new(host);
        let current = w.active_layout();
        let name = w.world().resource::<Workspace>().name.clone();
        let ticks = w.world().resource::<Session>().ticks;
        let layouts = w
            .world_mut()
            .query::<(Entity, &Layout)>()
            .iter(w.world())
            .map(|(id, l)| (id, l.name.clone()))
            .collect::<Vec<_>>();
        (current, layouts, name, ticks)
    };
    let mut ui = frame
        .ui
        .new_child(egui::UiBuilder::new().id_salt("workspace-controls"));
    let background_shape = ui.painter().add(egui::Shape::Noop);
    let inset = host
        .get::<WindowTitlebarInset>(frame.surface)
        .map_or(0.0, |inset| inset.0);
    ui.add_space(inset / frame.ui.ctx().zoom_factor());
    egui::Frame::NONE
        .inner_margin(egui::Margin::same(12))
        .show(&mut ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Workbench");
                bedtime_ui2::ComboBox::from_label("Workspace")
                    .selected_text(&name)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for workspace in workspaces.iter().filter(|w| !w.closing) {
                            if ui
                                .selectable_label(workspace.id == active, &workspace.name)
                                .clicked()
                            {
                                action = Some(WorkspaceRequest::Activate(workspace.id));
                            }
                        }
                    });
                bedtime_ui2::ComboBox::from_label("Layout")
                    .selected_text(
                        layouts
                            .iter()
                            .find(|(id, _)| *id == current)
                            .map(|(_, n)| n.as_str())
                            .unwrap(),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &layouts {
                            if ui.selectable_label(*id == current, name).clicked() {
                                layout_action = Some(*id);
                            }
                        }
                    });
                create = ui.button("New workspace").clicked();
                let replacement = workspaces
                    .iter()
                    .find(|w| w.id != active && !w.closing)
                    .map(|w| w.id);
                if ui
                    .add_enabled(replacement.is_some(), egui::Button::new("Close workspace"))
                    .clicked()
                {
                    action = Some(WorkspaceRequest::Close {
                        workspace: active,
                        replacement,
                    });
                }
                let mut enabled = workspaces
                    .iter()
                    .find(|w| w.active)
                    .unwrap()
                    .background_updates;
                if ui.checkbox(&mut enabled, "Update in background").changed() {
                    background = Some(enabled);
                }
                ui.weak(format!("{ticks} workspace updates"));
            });
            ui.horizontal_wrapped(|ui| {
                for workspace in workspaces.iter().filter(|w| w.id != active) {
                    let state = if workspace.closing {
                        "closing"
                    } else if workspace.background_updates {
                        "running"
                    } else {
                        "paused"
                    };
                    ui.weak(format!("{}: {state}", workspace.name));
                }
                if ui.button("Widget gallery").clicked() {
                    crate::tool_panels::open::<crate::tool_panels::Gallery>(host);
                }
                if ui.button("Theme editor").clicked() {
                    crate::tool_panels::open::<crate::tool_panels::ThemeEditor>(host);
                }
            });
        });
    let bottom = ui.min_rect().bottom() + 6.0;
    ui.painter().set(
        background_shape,
        egui::Shape::rect_filled(
            egui::Rect::from_min_max(
                frame.ui.max_rect().min,
                egui::pos2(frame.ui.max_rect().right(), bottom),
            ),
            0.0,
            bedtime_ui2::theme::color(frame.theme.palette.background),
        ),
    );
    let host_entity = host.resource::<DemoWorkbench>().host;
    let available = frame.ui.max_rect();
    host.get_mut::<WorkbenchHost>(host_entity).unwrap().bounds = Some(Rect::from_corners(
        Vec2::new(available.left(), bottom),
        Vec2::new(available.right(), available.bottom()),
    ));
    if let Some(layout) = layout_action {
        Workbench::new(host).activate_layout(layout).unwrap();
    }
    if let Some(enabled) = background {
        Workspaces::new(host)
            .set_background_updates(active, enabled)
            .unwrap();
    }
    if let Some(action) = action {
        host.resource_mut::<WorkspaceRequests>().0.push_back(action);
    }
    if create {
        let id = crate::open_workspace(host, &format!("Workspace {}", workspaces.len() + 1));
        host.resource_mut::<WorkspaceRequests>()
            .0
            .push_back(WorkspaceRequest::Activate(id));
    }
}
