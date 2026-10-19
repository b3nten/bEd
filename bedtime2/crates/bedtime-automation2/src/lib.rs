//! Ordered application control. Test scenarios and assertions belong to clients.
mod capture;
mod input;
mod inspect;
mod transport;
#[cfg(feature = "workbench")]
mod workbench;

#[cfg(feature = "workbench")]
use bevy::ecs::message::MessageCursor;
use bevy::{input::InputSystems, prelude::*};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

pub const PROTOCOL_VERSION: u32 = 1;
const QUEUE_LIMIT: usize = 128;
pub type AutomationResult = Result<Value, String>;
pub type AutomationHandler = fn(&mut World, Value) -> AutomationResult;

/// Install once, after the host plugins. Never adds a runner, window or renderer.
pub struct AutomationPlugin {
    stdio: bool,
}
impl AutomationPlugin {
    pub fn stdio() -> Self {
        Self { stdio: true }
    }
    /// Same protocol/scheduling without touching process streams; useful in tests.
    pub fn manual() -> Self {
        Self { stdio: false }
    }
}

/// In-process access to the same protocol. Do not use alongside a stdio client.
#[derive(Resource, Clone)]
pub struct Client {
    sender: SyncSender<Inbound>,
    receiver: Arc<Mutex<Receiver<Value>>>,
    unflushed: Arc<AtomicUsize>,
}
impl Client {
    pub fn submit(&self, request: Value) -> Result<(), String> {
        self.sender
            .try_send(Inbound::Request(request))
            .map_err(|e| e.to_string())
    }
    pub fn try_recv(&self) -> Option<Value> {
        let result = self.receiver.try_lock().ok()?.try_recv().ok();
        if result.is_some() {
            self.unflushed.fetch_sub(1, Ordering::AcqRel);
        }
        result
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutomationSystems {
    Apply,
    Observe,
    Complete,
}

#[derive(Resource, Default)]
struct Handlers(HashMap<String, AutomationHandler>);
#[derive(Resource, Clone, Default)]
struct RenderProgress {
    frames: Arc<AtomicUsize>,
    pending: Arc<AtomicUsize>,
}
pub trait AutomationAppExt {
    /// Synchronous app-specific operations; the receipt follows a subsequent frame.
    fn register_automation_command(&mut self, name: &str, handler: AutomationHandler) -> &mut Self;
}
impl AutomationAppExt for App {
    fn register_automation_command(&mut self, name: &str, handler: AutomationHandler) -> &mut Self {
        assert!(
            name.starts_with("app.") && name.len() > 4,
            "Custom automation names start with app."
        );
        self.init_resource::<Handlers>();
        assert!(
            self.world_mut()
                .resource_mut::<Handlers>()
                .0
                .insert(name.into(), handler)
                .is_none(),
            "Duplicate automation command"
        );
        self
    }
}

enum Inbound {
    Request(Value),
    Invalid(String),
    Eof,
    Disconnected,
    Captured(u64, AutomationResult),
}
#[derive(Deserialize)]
struct Request {
    id: Value,
    op: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(flatten)]
    args: serde_json::Map<String, Value>,
}
fn default_timeout() -> u64 {
    15_000
}
struct Pending {
    request: Request,
    deadline: Instant,
    token: u64,
    stage: Stage,
}
enum Stage {
    Complete {
        after: u64,
        value: Value,
    },
    Inspect {
        after: u64,
        entities: bool,
    },
    Input {
        window: Entity,
        frames: VecDeque<Vec<input::Event>>,
    },
    Capture {
        after: u64,
        args: Value,
        entity: Option<Entity>,
    },
    #[cfg(feature = "workbench")]
    Invoke {
        source: Entity,
    },
    #[cfg(feature = "workbench")]
    Closed {
        entity: Entity,
    },
}
#[derive(Resource)]
struct State {
    inbox: Mutex<Receiver<Inbound>>,
    output: SyncSender<Value>,
    unflushed: Arc<AtomicUsize>,
    queue: VecDeque<Request>,
    current: Option<Pending>,
    frame: u64,
    token: u64,
    ready: bool,
    eof: bool,
    disconnected: bool,
    exit_frame: Option<u64>,
    exclusive_input: bool,
    input: input::Held,
    #[cfg(feature = "ui")]
    ui: inspect::UiObservation,
    #[cfg(feature = "workbench")]
    commands: MessageCursor<bedtime_workbench2::ContextCommandFinished>,
    #[cfg(feature = "workbench")]
    detached: HashMap<Entity, bedtime_workbench2::WorkbenchSurface>,
}
impl State {
    fn send(&mut self, value: Value) {
        self.unflushed.fetch_add(1, Ordering::AcqRel);
        if self.output.try_send(value).is_err() {
            self.unflushed.fetch_sub(1, Ordering::AcqRel);
            self.disconnected = true;
        }
    }
    fn reply(&mut self, id: Value, result: AutomationResult) {
        self.send(match result {
            Ok(value) => json!({"id":id,"frame":self.frame,"result":value}),
            Err(message) => json!({"id":id,"frame":self.frame,"error":{"code":"operation_failed","message":message}}),
        });
    }
}
impl Plugin for AutomationPlugin {
    fn build(&self, app: &mut App) {
        let (sender, inbox) = mpsc::sync_channel(QUEUE_LIMIT);
        let (output, receiver) = mpsc::sync_channel(QUEUE_LIMIT);
        let unflushed = Arc::new(AtomicUsize::new(0));
        app.insert_resource(Client {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            unflushed: unflushed.clone(),
        })
        .insert_resource(State {
            inbox: Mutex::new(inbox),
            output,
            unflushed,
            queue: VecDeque::new(),
            current: None,
            frame: 0,
            token: 0,
            ready: false,
            eof: false,
            disconnected: false,
            exit_frame: None,
            exclusive_input: true,
            input: default(),
            #[cfg(feature = "ui")]
            ui: default(),
            #[cfg(feature = "workbench")]
            commands: default(),
            #[cfg(feature = "workbench")]
            detached: default(),
        })
        .init_resource::<Handlers>()
        .add_systems(
            PreUpdate,
            apply.in_set(AutomationSystems::Apply).before(InputSystems),
        )
        .add_systems(Last, complete.in_set(AutomationSystems::Complete));
        #[cfg(feature = "ui")]
        app.add_systems(
            PreUpdate,
            inspect::observe_ui
                .in_set(AutomationSystems::Observe)
                .after(bedtime_ui2::UiSystems::Draw),
        );
        if self.stdio {
            app.add_systems(
                Startup,
                |client: Res<Client>, proxy: Option<Res<bevy::winit::EventLoopProxyWrapper>>| {
                    transport::start(client.clone(), proxy.as_deref());
                },
            );
        }
    }
    fn finish(&self, app: &mut App) {
        if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
            let progress = RenderProgress::default();
            render.insert_resource(progress.clone()).add_systems(
                bevy::render::Render,
                (|progress: Res<RenderProgress>,
                  cache: Res<bevy::render::render_resource::PipelineCache>| {
                    progress
                        .pending
                        .store(cache.waiting_pipelines().count(), Ordering::Release);
                    progress.frames.fetch_add(1, Ordering::AcqRel);
                })
                .in_set(bevy::render::RenderSystems::Cleanup),
            );
            app.insert_resource(progress);
        }
    }
}

fn apply(world: &mut World) {
    let mut state = world.remove_resource::<State>().unwrap();
    state.frame += 1;
    if state.exclusive_input {
        input::isolate(world);
    }
    // The greeting is always the first output, even when stdin was piped into
    // the process before startup finished.
    if !state.ready {
        world.insert_resource(state);
        return;
    }
    for _ in 0..QUEUE_LIMIT {
        let message = state.inbox.lock().unwrap().try_recv();
        let Ok(message) = message else {
            break;
        };
        match message {
            Inbound::Request(value) => {
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let request = serde_json::from_value::<Request>(value)
                    .map_err(|e| e.to_string())
                    .and_then(|r| {
                        if !(r.id.is_string() || r.id.as_u64().is_some()) {
                            return Err("id must be a string or unsigned integer".into());
                        }
                        if !(1..=120_000).contains(&r.timeout_ms) {
                            return Err("timeout_ms must be in 1..=120000".into());
                        }
                        if state.queue.len() >= QUEUE_LIMIT {
                            return Err("Request queue is full".into());
                        }
                        if state.current.as_ref().is_some_and(|p| p.request.id == r.id)
                            || state.queue.iter().any(|p| p.id == r.id)
                        {
                            return Err("Duplicate pending request id".into());
                        }
                        Ok(r)
                    });
                match request {
                    Ok(r) => state.queue.push_back(r),
                    Err(e) => state.reply(id, Err(e)),
                }
            }
            Inbound::Invalid(e) => state.reply(Value::Null, Err(e)),
            Inbound::Eof => state.eof = true,
            Inbound::Disconnected => state.disconnected = true,
            Inbound::Captured(token, result) => {
                if state.current.as_ref().is_some_and(|p| p.token == token) {
                    let pending = state.current.take().unwrap();
                    state.reply(pending.request.id, result);
                }
            }
        }
    }
    if state
        .current
        .as_ref()
        .is_some_and(|p| Instant::now() >= p.deadline)
    {
        let pending = state.current.take().unwrap();
        cleanup(world, &pending.stage);
        input::release(world, &mut state.input);
        state.reply(pending.request.id, Err("Operation timed out".into()));
    }
    if state.ready && !state.disconnected && state.current.is_none() && state.exit_frame.is_none() {
        if let Some(request) = state.queue.pop_front() {
            state.token += 1;
            match start(world, &mut state, &request) {
                Ok(stage) => {
                    state.current = Some(Pending {
                        deadline: Instant::now() + Duration::from_millis(request.timeout_ms),
                        request,
                        stage,
                        token: state.token,
                    })
                }
                Err(error) => state.reply(request.id, Err(error)),
            }
        }
    }
    if let Some(mut pending) = state.current.take() {
        if let Stage::Input { window, frames } = &mut pending.stage {
            if world.get::<Window>(*window).is_none() {
                state.reply(pending.request.id, Err("Input window was closed".into()));
                input::release(world, &mut state.input);
                world.insert_resource(state);
                return;
            }
            input::focus(world, *window);
            if let Some(events) = frames.pop_front() {
                input::emit(world, *window, events, &mut state.input);
            } else {
                input::release(world, &mut state.input);
                pending.stage = Stage::Complete {
                    after: state.frame + 1,
                    value: json!({"applied":true}),
                };
            }
        }
        state.current = Some(pending);
    }
    world.insert_resource(state);
}

fn start(world: &mut World, state: &mut State, request: &Request) -> Result<Stage, String> {
    let args = Value::Object(request.args.clone());
    let value = match request.op.as_str() {
        "describe" => describe(world),
        "inspect" => {
            return Ok(Stage::Inspect {
                after: state.frame + 1,
                entities: optional(&args, "entities")?.unwrap_or(false),
            });
        }
        "wait_frames" => {
            let count: u64 = required(&args, "frames")?;
            if count > 10000 {
                return Err("frames must be at most 10000".into());
            }
            return Ok(Stage::Complete {
                after: state.frame + count,
                value: json!({"waited":count}),
            });
        }
        "input" => {
            if !world.contains_resource::<Messages<bevy::window::WindowEvent>>()
                || !world.contains_resource::<Messages<bevy::input::keyboard::KeyboardInput>>()
            {
                return Err("Input requires Bevy's InputPlugin and WindowPlugin".into());
            }
            let window = window(world, &args)?;
            let frames = input::prepare(world, &args, state)?;
            return Ok(Stage::Input { window, frames });
        }
        "capture" => {
            return Ok(Stage::Capture {
                after: state.frame + 1,
                args,
                entity: None,
            });
        }
        "configure" => {
            #[cfg(not(feature = "ui"))]
            if args.get("clipboard").is_some() {
                return Err("Memory clipboard configuration requires the ui feature".into());
            }
            let dt: Option<f64> = optional(&args, "fixed_dt")?;
            if dt.is_some_and(|dt| !dt.is_finite() || !(0.000001..=1.0).contains(&dt)) {
                return Err("fixed_dt must be in 0.000001..=1 seconds".into());
            }
            let exclusive = optional(&args, "exclusive_input")?;
            #[cfg(feature = "ui")]
            let clipboard = optional::<String>(&args, "clipboard")?;
            if args.get("fixed_dt").is_some() {
                world.insert_resource(dt.map_or(bevy::time::TimeUpdateStrategy::Automatic, |dt| {
                    bevy::time::TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(dt))
                }));
            }
            if let Some(exclusive) = exclusive {
                state.exclusive_input = exclusive;
            }
            #[cfg(feature = "ui")]
            if let Some(text) = clipboard {
                let mut clipboard = bedtime_clipboard2::Clipboard::memory();
                clipboard.write_text(&text).map_err(|e| e.to_string())?;
                world.insert_resource(clipboard);
            }
            json!({"exclusive_input":state.exclusive_input})
        }
        "window.resize" => {
            let window = window(world, &args)?;
            let [width, height]: [u32; 2] = required(&args, "size")?;
            if width == 0 || height == 0 || width > 8192 || height > 8192 {
                return Err("Window size must be in 1..=8192".into());
            }
            world
                .get_mut::<Window>(window)
                .unwrap()
                .resolution
                .set(width as f32, height as f32);
            json!({"window":entity_id(window)})
        }
        "entity.inspect" => {
            let entity = entity(world, &args, "entity")?;
            let e = world.entity(entity);
            let components: Vec<_> = e
                .archetype()
                .components()
                .iter()
                .filter_map(|id| {
                    world
                        .components()
                        .get_info(*id)
                        .map(|i| i.name().to_string())
                })
                .collect();
            json!({"entity":entity_id(entity),"components":components})
        }
        "exit" => {
            state.exit_frame = Some(state.frame + 1);
            json!({"exiting":true})
        }
        #[cfg(feature = "workbench")]
        op if workbench::OPERATIONS.contains(&op) => {
            return workbench::start(world, state, op, &args);
        }
        op => {
            let handler = world
                .resource::<Handlers>()
                .0
                .get(op)
                .copied()
                .ok_or_else(|| format!("Unknown operation: {op}"))?;
            handler(world, args)?
        }
    };
    Ok(Stage::Complete {
        after: state.frame + 1,
        value,
    })
}

fn complete(world: &mut World) {
    let mut state = world.remove_resource::<State>().unwrap();
    if !state.ready {
        state.ready = true;
        state.send(json!({"event":"ready","protocol":PROTOCOL_VERSION,"frame":state.frame,"capabilities":capabilities()}));
    }
    if let Some(mut pending) = state.current.take() {
        let result = match &mut pending.stage {
            Stage::Complete { after, value } if state.frame >= *after => Some(Ok(value.take())),
            Stage::Inspect { after, entities } if state.frame >= *after => {
                Some(Ok(inspect::snapshot(world, &state, *entities)))
            }
            Stage::Capture {
                after,
                args,
                entity,
            } if state.frame >= *after && entity.is_none() => {
                match capture::start(world, args, pending.token, state.frame) {
                    Ok(e) => {
                        *entity = Some(e);
                        None
                    }
                    Err(e) => Some(Err(e)),
                }
            }
            #[cfg(feature = "workbench")]
            Stage::Invoke { source } => world
                .get_resource::<Messages<bedtime_workbench2::ContextCommandFinished>>()
                .and_then(|messages| {
                    state
                        .commands
                        .read(messages)
                        .find(|m| m.invocation.source == Some(*source))
                        .map(|m| m.result.clone().map(|_| json!({"applied":true})))
                }),
            #[cfg(feature = "workbench")]
            Stage::Closed { entity } if world.get_entity(*entity).is_err() => {
                Some(Ok(json!({"closed":entity_id(*entity)})))
            }
            _ => None,
        };
        if let Some(result) = result {
            cleanup(world, &pending.stage);
            state.reply(pending.request.id, result);
        } else {
            state.current = Some(pending);
        }
    }
    // Exit only after the writer acknowledges flushing every receipt.
    let exit_requested = state.exit_frame.is_some_and(|f| state.frame >= f)
        || (state.eof && state.current.is_none() && state.queue.is_empty());
    if state.exit_frame.is_some() && state.current.is_none() {
        while let Some(request) = state.queue.pop_front() {
            state.reply(request.id, Err("Application is exiting".into()));
        }
    }
    if state.disconnected || (exit_requested && state.unflushed.load(Ordering::Acquire) == 0) {
        input::release(world, &mut state.input);
        world.write_message(if state.disconnected {
            AppExit::error()
        } else {
            AppExit::Success
        });
    } else if state.current.is_some() || !state.queue.is_empty() || state.exit_frame.is_some() {
        if let Some(proxy) = world.get_resource::<bevy::winit::EventLoopProxyWrapper>() {
            let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
        }
    }
    world.insert_resource(state);
}

fn cleanup(world: &mut World, stage: &Stage) {
    let entity = match stage {
        Stage::Capture { entity, .. } => *entity,
        #[cfg(feature = "workbench")]
        Stage::Invoke { source } => Some(*source),
        _ => None,
    };
    if let Some(entity) = entity {
        if world.get_entity(entity).is_ok() {
            world.despawn(entity);
        }
    }
}
fn capabilities() -> Vec<&'static str> {
    let result = vec![
        "describe",
        "inspect",
        "configure",
        "wait_frames",
        "input",
        "capture",
        "window.resize",
        "entity.inspect",
        "exit",
    ];
    #[cfg(feature = "workbench")]
    {
        let mut result = result;
        result.extend(workbench::OPERATIONS);
        result
    }
    #[cfg(not(feature = "workbench"))]
    {
        result
    }
}
fn describe(world: &World) -> Value {
    json!({"protocol":PROTOCOL_VERSION,"operations":capabilities(),"custom_operations":world.resource::<Handlers>().0.keys().collect::<Vec<_>>(),"coordinates":"logical window coordinates; inspect ui pixels_per_point for egui coordinates","input_steps":input::KINDS})
}
fn required<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(
        args.get(key)
            .cloned()
            .ok_or_else(|| format!("Missing {key}"))?,
    )
    .map_err(|e| format!("{key}: {e}"))
}
fn optional<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>, String> {
    args.get(key)
        .filter(|v| !v.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("{key}: {e}"))
}
fn entity_id(entity: Entity) -> String {
    entity.to_bits().to_string()
}
fn entity(world: &World, args: &Value, key: &str) -> Result<Entity, String> {
    let id: String = required(args, key)?;
    let entity = Entity::try_from_bits(id.parse().map_err(|_| format!("Invalid {key} entity id"))?)
        .ok_or_else(|| format!("Invalid {key} entity id"))?;
    world
        .get_entity(entity)
        .map_err(|_| format!("Stale {key} entity id"))?;
    Ok(entity)
}
fn window(world: &mut World, args: &Value) -> Result<Entity, String> {
    if args.get("window").is_some() {
        let e = entity(world, args, "window")?;
        world.get::<Window>(e).ok_or("Entity is not a window")?;
        return Ok(e);
    }
    let windows: Vec<_> = world
        .query_filtered::<(Entity, Option<&bevy::window::PrimaryWindow>), With<Window>>()
        .iter(world)
        .collect();
    windows
        .iter()
        .find(|(_, p)| p.is_some())
        .or_else(|| (windows.len() == 1).then(|| &windows[0]))
        .map(|(e, _)| *e)
        .ok_or_else(|| "Specify a window; there is no unique primary window".into())
}

#[cfg(test)]
mod tests;
