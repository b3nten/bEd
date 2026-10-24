use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::{
    ManagedProcess, ManagedTask, Panel, PanelInstance, ProcessSpec, WorkStatus, Workbench,
};
use bevy::prelude::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

pub struct TasksPlugin;

impl Plugin for TasksPlugin {
    fn build(&self, app: &mut App) {
        register_menu(app);
        app.add_systems(Update, poll_work);

        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_tasks", inspect);
        }
    }
}

#[derive(Component)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct TasksPanel {
    seconds: f32,
    task: Option<CountingTask>,
    process: Option<EchoProcess>,
    input: String,
    error: Option<String>,
}

impl Default for TasksPanel {
    fn default() -> Self {
        Self {
            seconds: 5.0,
            task: None,
            process: None,
            input: "Hello from bEd".into(),
            error: None,
        }
    }
}

struct CountingTask {
    handle: ManagedTask<u64>,
    progress: Arc<AtomicU32>,
    result: Option<u64>,
}

struct EchoProcess {
    handle: ManagedProcess,
    stdin_closed: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum Action {
    StartTask,
    CancelTask,
    StartProcess,
    Send,
    CloseStdin,
    StopProcess,
}

fn busy(status: &WorkStatus) -> bool {
    matches!(status, WorkStatus::Running | WorkStatus::Cancelling)
}

fn status_text(status: &WorkStatus) -> String {
    match status {
        WorkStatus::Running => "Running".into(),
        WorkStatus::Cancelling => "Cancelling".into(),
        WorkStatus::Cancelled => "Cancelled".into(),
        WorkStatus::Finished => "Finished".into(),
        WorkStatus::Failed(error) => format!("Failed: {error}"),
    }
}

impl Panel for TasksPanel {
    const KEY: &'static str = "bed.tasks";
    const LABEL: &'static str = "Tasks & Processes";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let surface = *world
            .get::<bedtime_workbench2::WorkbenchSurface>(context.surface)
            .unwrap();

        let mut state = world.get_mut::<Self>(panel).unwrap();
        let mut action = None;

        context
            .frame(ui)
            .scroll("tasks", surface.rounding, |mut frame| {
                frame.padded(surface.padding, |frame| {
                    let ui = frame.ui;
                    ui.heading(Self::LABEL);
                    ui.weak("Closing this panel or its workspace cancels its workers.");
                    if let Some(error) = &state.error {
                        ui.colored_label(
                            bedtime_ui2::theme::color(context.theme.palette.error),
                            error,
                        );
                    }

                    ui.separator();
                    ui.strong("Background task");
                    ui.weak("Sum 1 through 100 on a worker thread, with cooperative cancellation.");
                    let running = state
                        .task
                        .as_ref()
                        .is_some_and(|t| busy(&t.handle.status()));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(!running, egui::Button::new("Start task"))
                            .clicked()
                        {
                            action = Some(Action::StartTask);
                        }
                        if ui
                            .add_enabled(running, egui::Button::new("Cancel task"))
                            .clicked()
                        {
                            action = Some(Action::CancelTask);
                        }
                        ui.label("Duration");
                        ui.add_enabled(
                            !running,
                            egui::DragValue::new(&mut state.seconds)
                                .speed(0.1)
                                .range(0.5..=30.0)
                                .suffix(" s"),
                        );
                    });
                    if let Some(task) = &state.task {
                        let progress = task.progress.load(Ordering::Acquire);
                        ui.add(
                            egui::ProgressBar::new(progress as f32 / 100.0).text(format!(
                                "{} · {progress}/100 steps",
                                status_text(&task.handle.status())
                            )),
                        );
                        if let Some(result) = task.result {
                            ui.label(format!("Result: {result}"));
                        }
                    }

                    ui.separator();
                    ui.strong("Pipe process");
                    ui.weak("Send text to echo it. Try stderr, fail (exit 7), or exit (exit 0).");
                    let running = state
                        .process
                        .as_ref()
                        .is_some_and(|p| busy(&p.handle.status()));
                    let writable = state.process.as_ref().is_some_and(|p| {
                        p.handle.status() == WorkStatus::Running && !p.stdin_closed
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(!running, egui::Button::new("Start process"))
                            .clicked()
                        {
                            action = Some(Action::StartProcess);
                        }
                        if ui
                            .add_enabled(running, egui::Button::new("Stop process"))
                            .clicked()
                        {
                            action = Some(Action::StopProcess);
                        }
                        if ui
                            .add_enabled(writable, egui::Button::new("Close stdin"))
                            .clicked()
                        {
                            action = Some(Action::CloseStdin);
                        }
                    });
                    ui.horizontal(|ui| {
                        let input = ui.add_enabled(
                            writable,
                            egui::TextEdit::singleline(&mut state.input)
                                .desired_width((ui.available_width() - 60.0).max(40.0)),
                        );
                        if ui
                            .add_enabled(writable, egui::Button::new("Send"))
                            .clicked()
                            || (writable
                                && input.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            action = Some(Action::Send);
                        }
                    });
                    if let Some(process) = &state.process {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "{} · PID {}",
                                status_text(&process.handle.status()),
                                process.handle.process_id
                            ));
                            if process.stdin_closed {
                                ui.weak("stdin closed");
                            }
                            if let Some(exit) = process.handle.exit_status() {
                                let color = if exit.success() {
                                    context.theme.palette.success
                                } else {
                                    context.theme.palette.error
                                };
                                ui.colored_label(
                                    bedtime_ui2::theme::color(color),
                                    exit.to_string(),
                                );
                            }
                        });
                        ui.columns(2, |columns| {
                            for (ui, (label, bytes)) in columns
                                .iter_mut()
                                .zip([("stdout", &process.stdout), ("stderr", &process.stderr)])
                            {
                                ui.strong(label);
                                egui::ScrollArea::both()
                                    .id_salt(label)
                                    .max_height(160.0)
                                    .auto_shrink([false, false])
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(String::from_utf8_lossy(bytes))
                                                    .monospace(),
                                            )
                                            .selectable(true),
                                        );
                                    });
                            }
                        });
                        ui.weak(
                            "Newest 64 KiB retained per stream. This is a pipe, not a terminal.",
                        );
                    }
                });
            });

        if let Some(action) = action {
            // End the component borrow before accessing the workspace's work queue.
            drop(state);
            let result = apply_action(world, panel, action);
            world.get_mut::<Self>(panel).unwrap().error = result.err();
        }
    }
}

fn apply_action(world: &mut World, panel: Entity, action: Action) -> Result<(), String> {
    match action {
        Action::StartTask => {
            let seconds = world.get::<TasksPanel>(panel).unwrap().seconds;
            let progress = Arc::new(AtomicU32::new(0));
            let completed = progress.clone();
            let handle = Workbench::for_entity(world, panel)?.work().spawn_task(
                Some(panel),
                "Count to 100",
                move |cancel| {
                    let mut total = 0;
                    for step in 1..=100 {
                        if cancel.wait(Duration::from_secs_f32(seconds / 100.0)) {
                            return total;
                        }
                        total += u64::from(step);
                        completed.store(step, Ordering::Release);
                    }
                    total
                },
            )?;
            world.get_mut::<TasksPanel>(panel).unwrap().task = Some(CountingTask {
                handle,
                progress,
                result: None,
            });
        }
        Action::StartProcess => {
            if !crate::files::root_ready(world) {
                return Err("Workspace root is still loading".into());
            }
            let mut spec = echo_spec();
            spec.directory = crate::files::project_directory(world);
            let handle = Workbench::for_entity(world, panel)?.work().spawn_process(
                Some(panel),
                "Interactive echo",
                spec,
            )?;
            world.get_mut::<TasksPanel>(panel).unwrap().process = Some(EchoProcess {
                handle,
                stdin_closed: false,
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }
        action => {
            let mut state = world.get_mut::<TasksPanel>(panel).unwrap();
            match action {
                Action::CancelTask => state.task.as_ref().unwrap().handle.cancel(),
                Action::Send => {
                    let mut line = state.input.clone();
                    line.push('\n');
                    state
                        .process
                        .as_ref()
                        .unwrap()
                        .handle
                        .write(line.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                Action::CloseStdin => {
                    let process = state.process.as_mut().unwrap();
                    process.handle.close_stdin();
                    process.stdin_closed = true;
                }
                Action::StopProcess => state.process.as_ref().unwrap().handle.cancel(),
                Action::StartTask | Action::StartProcess => unreachable!(),
            }
        }
    }
    Ok(())
}

/// Fixed scripts interpret complete input lines as data, never as shell commands.
fn echo_spec() -> ProcessSpec {
    #[cfg(not(windows))]
    {
        let mut spec = ProcessSpec::new("/bin/sh");
        spec.arguments = [
            "-c",
            r#"
printf 'Ready. Send text, stderr, fail, or exit.\n'
while IFS= read -r line; do
    case "$line" in
        stderr) printf 'A message from stderr.\n' >&2 ;;
        fail) printf 'Requested failure.\n' >&2; exit 7 ;;
        exit) printf 'Goodbye.\n'; exit 0 ;;
        *) printf 'echo: %s\n' "$line" ;;
    esac
done
printf 'stdin closed.\n'
"#,
        ]
        .map(Into::into)
        .to_vec();
        spec
    }
    #[cfg(windows)]
    {
        let mut spec = ProcessSpec::new("powershell.exe");
        spec.arguments = [
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            r#"
[Console]::WriteLine('Ready. Send text, stderr, fail, or exit.')
while ($null -ne ($line = [Console]::ReadLine())) {
    switch -Exact ($line) {
        'stderr' { [Console]::Error.WriteLine('A message from stderr.') }
        'fail' { [Console]::Error.WriteLine('Requested failure.'); exit 7 }
        'exit' { [Console]::WriteLine('Goodbye.'); exit 0 }
        default { [Console]::WriteLine('echo: ' + $line) }
    }
}
[Console]::WriteLine('stdin closed.')
"#,
        ]
        .map(Into::into)
        .to_vec();
        spec
    }
}

// Poll every panel, including hidden tabs and inactive/suspended workspaces.
// Only ManagedWork owns worker lifetime; these components own observations.
fn poll_work(mut panels: Query<&mut TasksPanel>) {
    for mut panel in &mut panels {
        if let Some(task) = &mut panel.task
            && let Ok(result) = task.handle.try_recv()
        {
            task.result = Some(result);
        }
        if let Some(process) = &mut panel.process {
            for chunk in process.handle.drain_output() {
                let bytes = if chunk.stderr {
                    &mut process.stderr
                } else {
                    &mut process.stdout
                };
                bytes.extend(chunk.bytes);
                if bytes.len() > 65_536 {
                    bytes.drain(..bytes.len() - 65_536);
                }
            }
        }
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    use bedtime_workbench2::ManagedWork;
    use serde_json::json;
    let panels: Vec<_> = world.query::<(Entity, &TasksPanel)>().iter(world)
        .map(|(entity, state)| json!({
            "entity": entity.to_bits().to_string(),
            "seconds": state.seconds, "input": state.input, "error": state.error,
            "task": state.task.as_ref().map(|t| json!({
                "id": t.handle.id.0, "status": status_text(&t.handle.status()),
                "progress": t.progress.load(Ordering::Acquire), "result": t.result,
            })),
            "process": state.process.as_ref().map(|p| json!({
                "id": p.handle.id.0, "pid": p.handle.process_id, "status": status_text(&p.handle.status()),
                "stdin_closed": p.stdin_closed,
                "exit_code": p.handle.exit_status().and_then(|s| s.code()),
                "success": p.handle.exit_status().map(|s| s.success()),
                "stdout": String::from_utf8_lossy(&p.stdout), "stderr": String::from_utf8_lossy(&p.stderr),
            })),
        })).collect();
    let jobs: Vec<_> = world.resource::<ManagedWork>().list().into_iter().map(|job| json!({"id":job.id.0,"label":job.label,"owner":job.owner.map(|e|e.to_bits().to_string()),"status":status_text(&job.status)})).collect();
    Ok(json!({"panels":panels,"jobs":jobs}))
}

use bedtime_workbench2::persistence::ReflectWorkbenchPersist;

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(TasksPanel)]
pub(crate) struct PersistedTasks;

pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedTasks>();
}
pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench2::persistence::CapturedWorld,
) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<TasksPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        captured.insert_component(panel, PersistedTasks);
    }
}
pub(crate) fn clear_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PersistedTasks>>()
        .iter(world)
        .collect();
    for panel in panels {
        world.entity_mut(panel).remove::<PersistedTasks>();
    }
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<TasksPanel>(
        app,
        Some(70),
        bedtime_workbench2::PanelFactory::of::<TasksPanel>(),
        crate::storage::ready,
    );
}
