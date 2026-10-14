//! Local, user-approved control. Socket workers own bytes; only the app owns execution.
use super::*;
use bedtime_api::settings::{SettingDefinition, SettingsRegistry, SettingsSnapshot};
use dear_imgui_rs::Ui;
use serde_json::json;
use std::{
    ffi::OsString,
    fs::{self, DirBuilder},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
};

const MAX_CLIENTS: usize = 8;
const MAX_OPERATIONS: usize = 64;
const MAX_TOTAL_OPERATIONS: usize = 512;
const MAX_REQUEST: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 96 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Default, Debug)]
pub struct LaunchOptions {
    pub allow: bool,
    pub approved_names: Vec<String>,
    pub socket_path: Option<PathBuf>,
}
impl LaunchOptions {
    /// Remove only shared automation flags; application arguments remain in order.
    pub fn extract(arguments: &mut Vec<OsString>) -> io::Result<Self> {
        let mut result = Self::default();
        let mut retained = Vec::new();
        let mut iter = std::mem::take(arguments).into_iter();
        while let Some(arg) = iter.next() {
            match arg.to_str() {
                Some("--allow-automation") => result.allow = true,
                Some("--approve-automation") => result.approved_names.push(
                    iter.next()
                        .and_then(|v| v.into_string().ok())
                        .ok_or_else(|| {
                            io::Error::other("--approve-automation requires a client name")
                        })?,
                ),
                Some(text) if text.starts_with("--approve-automation=") => {
                    result.approved_names.push(text[21..].to_owned())
                }
                Some("--automation-socket") => {
                    result.socket_path =
                        Some(iter.next().map(PathBuf::from).ok_or_else(|| {
                            io::Error::other("--automation-socket requires a path")
                        })?)
                }
                Some(text) if text.starts_with("--automation-socket=") => {
                    result.socket_path = Some(text[20..].into())
                }
                _ => retained.push(arg),
            }
        }
        if result.approved_names.iter().any(|name| {
            name.is_empty()
                || name.len() > 256
                || name.chars().any(char::is_control)
                || name == "*"
                || name.starts_with("--")
        }) {
            return Err(io::Error::other(
                "automation client names must be nonempty labels without control characters or wildcards",
            ));
        }
        if result
            .socket_path
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty() || path.to_str().is_none())
        {
            return Err(io::Error::other(
                "--automation-socket requires a nonempty UTF-8 path",
            ));
        }
        if (!result.approved_names.is_empty() || result.socket_path.is_some()) && !result.allow {
            return Err(io::Error::other(
                "automation launch overrides require --allow-automation",
            ));
        }
        *arguments = retained;
        Ok(result)
    }
    pub fn approves(&self, name: &str) -> bool {
        self.approved_names.iter().any(|approved| approved == name)
    }
}

pub struct TransportPlugin {
    pub options: LaunchOptions,
    pub application: String,
}
impl Plugin for TransportPlugin {
    fn build(&self, app: &mut App) {
        let world = app.world_mut();
        world.init_resource::<SettingsRegistry>();
        world
            .resource_mut::<SettingsRegistry>()
            .register(
                "bedtime.automation",
                vec![SettingDefinition::new(
                    "enabled",
                    "Allow external automation",
                    false,
                )],
            )
            .expect("automation settings register before finalization");
        world.init_resource::<bedtime_api::features::FeatureRegistry>();
        world
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("bedtime.automation", |registrar| {
                registrar.settings_page("bedtime.automation.settings", "Automation", settings_page)
            })
            .expect("automation page registers once");
        world.insert_non_send(Control {
            options: self.options.clone(),
            application: self.application.clone(),
            server: None,
            error: None,
            blocked_inputs: false,
            orphans: Vec::new(),
        });
        world.init_resource::<InputBlocked>();
        world.spawn((
            bedtime_api::workbench::WorkbenchOverlay(draw),
            bedtime_api::workbench::OverlayPhase::AfterChrome,
            bedtime_api::workbench::OverlayOrder(200),
        ));
        app.add_systems(
            PreUpdate,
            update
                .before(AutomationSystems::Ingress)
                .before(DesktopSystems::InjectInput),
        )
        .add_systems(Last, stop.after(bevy::app::OnAppExitSystems));
    }
}
#[derive(Resource, Default)]
pub(super) struct InputBlocked(pub bool);

struct Control {
    options: LaunchOptions,
    application: String,
    server: Option<Server>,
    error: Option<String>,
    blocked_inputs: bool,
    orphans: Vec<OperationId>,
}
struct Server {
    listener: UnixListener,
    endpoint: Endpoint,
    clients: Vec<Client>,
    next: u64,
    orphans: Vec<OperationId>,
}

/// Own the filesystem entry from bind through shutdown, including setup errors.
struct Endpoint {
    path: PathBuf,
    directory: Option<PathBuf>,
    identity: Option<(u64, u64)>,
}
struct Client {
    id: u64,
    name: Option<String>,
    approved: bool,
    revoked: bool,
    since: Instant,
    incoming: Receiver<Value>,
    outgoing: SyncSender<Output>,
    operations: HashSet<OperationId>,
    released: HashSet<OperationId>,
    dependencies: HashMap<OperationId, OperationId>,
}
use std::collections::HashSet;
enum Output {
    Json(Value),
    Completed {
        request: u64,
        operation: OperationId,
        frame: u64,
        result: OperationResult,
    },
}

impl Server {
    fn bind(options: &LaunchOptions) -> io::Result<Self> {
        let (path, directory) = if let Some(path) = &options.socket_path {
            (path.clone(), None)
        } else {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("bta-{}-{nonce:x}", std::process::id()));
            DirBuilder::new().mode(0o700).create(&directory)?;
            (directory.join("control.sock"), Some(directory))
        };
        let mut endpoint = Endpoint {
            path,
            directory,
            identity: None,
        };
        let path = &endpoint.path;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("socket requires a private directory"))?;
        let metadata = fs::symlink_metadata(parent)?;
        // SAFETY: geteuid has no arguments or memory access.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "automation socket directory must be owned by this user with mode 0700",
            ));
        }
        // Never unlink a caller-supplied existing path.
        let listener = UnixListener::bind(path)?;
        let metadata = fs::symlink_metadata(path)?;
        endpoint.identity = Some((metadata.dev(), metadata.ino()));
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        eprintln!(
            "AUTOMATION_ENDPOINT {}",
            json!({"protocol":1,"pid":std::process::id(),"socket":path})
        );
        Ok(Self {
            listener,
            endpoint,
            clients: Vec::new(),
            next: 0,
            orphans: Vec::new(),
        })
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| Some((metadata.dev(), metadata.ino())) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
        if let Some(directory) = &self.directory {
            let _ = fs::remove_dir(directory);
        }
    }
}

fn same_user(stream: &UnixStream) -> io::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: pointers refer to initialized uid/gid storage; fd is live.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid == unsafe { libc::geteuid() })
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: zero is valid for the plain credential structure.
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of_val(&credentials) as libc::socklen_t;
        // SAFETY: buffer and length point to valid appropriately sized storage.
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut size,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(credentials.uid == unsafe { libc::geteuid() })
    }
}

fn worker(mut stream: UnixStream, incoming: SyncSender<Value>, outgoing: Receiver<Output>) {
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let mut read = Vec::new();
    let mut write = Vec::new();
    let mut offset = 0;
    let mut partial_since = None;
    let mut writing_since = Instant::now();
    loop {
        let mut buffer = [0u8; 8192];
        // Drain only bounded bytes each iteration.
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                if partial_since.is_none() {
                    partial_since = Some(Instant::now());
                }
                read.extend_from_slice(&buffer[..n]);
                if read.len() > MAX_REQUEST {
                    return;
                }
                while let Some(end) = read.iter().position(|byte| *byte == b'\n') {
                    let request = match serde_json::from_slice::<Value>(&read[..end]) {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    if incoming.try_send(request).is_err() {
                        return;
                    }
                    read.drain(..=end);
                    partial_since = if read.is_empty() {
                        None
                    } else {
                        Some(Instant::now())
                    };
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        if partial_since.is_some_and(|since: Instant| since.elapsed() > HANDSHAKE_TIMEOUT) {
            return;
        }
        if offset == write.len() {
            match outgoing.try_recv() {
                Ok(output) => {
                    let value = match output {
                        Output::Json(value) => value,
                        Output::Completed {
                            request,
                            operation,
                            frame,
                            result,
                        } => match result.and_then(wire::encode) {
                            Ok(value) => {
                                json!({"id":request,"result":{"state":"complete","operation":operation.0,"frame":frame,"value":value}})
                            }
                            Err(error) => {
                                json!({"id":request,"result":{"state":"complete","operation":operation.0,"frame":frame,"error":wire::error(&error)}})
                            }
                        },
                    };
                    write = match serde_json::to_vec(&value) {
                        Ok(bytes) if bytes.len() <= MAX_OUTPUT => bytes,
                        _ => return,
                    };
                    write.push(b'\n');
                    offset = 0;
                    writing_since = Instant::now();
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
        }
        if offset < write.len() {
            match stream.write(&write[offset..]) {
                Ok(0) => return,
                Ok(n) => {
                    offset += n;
                    writing_since = Instant::now();
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
        if offset < write.len() && writing_since.elapsed() > Duration::from_secs(30) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn send(client: &mut Client, value: Value) {
    if client.outgoing.try_send(Output::Json(value)).is_err() {
        client.revoked = true;
    }
}
fn failure(client: &mut Client, id: u64, message: &str) {
    send(
        client,
        json!({"id":id,"error":{"kind":"InvalidRequest","message":message}}),
    );
}

fn request(
    world: &mut World,
    client: &mut Client,
    value: Value,
    options: &LaunchOptions,
    application: &str,
) {
    let Some(id) = value["id"].as_u64() else {
        client.revoked = true;
        return;
    };
    let Some(method) = value["method"].as_str() else {
        failure(client, id, "method must be a string");
        return;
    };
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    if method == "hello" {
        if client.name.is_some() {
            failure(client, id, "handshake already completed");
            return;
        }
        let Some(name) = params["name"].as_str().filter(|name| {
            !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
        }) else {
            failure(client, id, "invalid client name");
            client.revoked = true;
            return;
        };
        if params["protocol"].as_u64() != Some(1) {
            failure(client, id, "unsupported protocol version");
            client.revoked = true;
            return;
        }
        client.name = Some(name.into());
        client.approved = options.approves(name);
        client.since = Instant::now();
    } else if client.name.is_none() {
        failure(client, id, "hello must be the first request");
        return;
    }
    if method == "hello" || method == "connection_status" {
        send(
            client,
            json!({"id":id,"result":{"protocol":1,"application":application,"connection":client.id,"state":if client.approved {"approved"} else {"pending"}}}),
        );
        return;
    }
    if !client.approved {
        failure(client, id, "connection requires user approval");
        return;
    }
    match method {
        "submit" => {
            let total = world
                .resource::<AutomationResults>()
                .statuses
                .lock()
                .expect("automation status mutex poisoned")
                .len();
            if client.operations.len() >= MAX_OPERATIONS || total >= MAX_TOTAL_OPERATIONS {
                failure(
                    client,
                    id,
                    "release receipts before submitting more operations",
                );
                return;
            }
            let result = wire::string(&params, "operation").and_then(|name| {
                wire::decode(
                    name,
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
            });
            match result {
                Err(error) => send(client, json!({"id":id,"error":wire::error(&error)})),
                Ok(operation) => {
                    if matches!(operation, Operation::Capture { .. }) {
                        let active = world
                            .resource::<ActiveOperations>()
                            .0
                            .iter()
                            .filter(|(_, pending)| matches!(pending, Pending::Capture { .. }))
                            .count();
                        let queued = world
                            .resource::<AutomationRequests>()
                            .pending
                            .iter()
                            .filter(|(_, op)| matches!(op, Operation::Capture { .. }))
                            .count();
                        let retained = world
                            .resource::<AutomationResults>()
                            .complete
                            .values()
                            .filter(|receipt| {
                                matches!(&receipt.payload, Some(Ok(OperationValue::Captured(_))))
                            })
                            .count();
                        if active + queued + retained >= 4 {
                            failure(client, id, "too many outstanding captures");
                            return;
                        }
                    }
                    if matches!(operation, Operation::Input(_))
                        && world.resource::<InputBlocked>().0
                    {
                        failure(
                            client,
                            id,
                            "input is blocked while a connection awaits approval",
                        );
                        return;
                    }
                    let dependency = if let Operation::Capture {
                        after: Some(after), ..
                    } = &operation
                    {
                        if !client.operations.contains(after) || client.released.contains(after) {
                            failure(
                                client,
                                id,
                                "capture dependency is not owned by this connection",
                            );
                            return;
                        }
                        Some(*after)
                    } else {
                        None
                    };
                    let operation = world.resource_mut::<AutomationRequests>().submit(operation);
                    client.operations.insert(operation);
                    if let Some(after) = dependency {
                        client.dependencies.insert(operation, after);
                    }
                    send(client, json!({"id":id,"result":{"operation":operation.0}}));
                }
            }
        }
        "poll" | "release" => {
            let Some(operation) = params["operation"]
                .as_u64()
                .map(OperationId)
                .filter(|id| client.operations.contains(id) && !client.released.contains(id))
            else {
                failure(client, id, "operation is unknown or released");
                return;
            };
            if method == "release" {
                client.released.insert(operation);
                send(client, json!({"id":id,"result":{"released":true}}));
            } else if let Some(result) = world.resource_mut::<AutomationResults>().take(operation) {
                let frame = world
                    .resource::<AutomationResults>()
                    .completion_frame(operation)
                    .expect("completed receipt")
                    .0;
                client.dependencies.remove(&operation);
                if client
                    .outgoing
                    .try_send(Output::Completed {
                        request: id,
                        operation,
                        frame,
                        result,
                    })
                    .is_err()
                {
                    client.revoked = true;
                }
            } else if world.resource::<AutomationResults>().status(operation)
                == Some(OperationStatus::Complete)
            {
                failure(client, id, "result already consumed");
            } else {
                send(
                    client,
                    json!({"id":id,"result":{"state":"pending","operation":operation.0}}),
                );
            }
        }
        _ => failure(client, id, "unknown transport method"),
    }
}

fn detach(world: &mut World, owned: &HashSet<OperationId>) {
    let pending = std::mem::take(&mut world.resource_mut::<AutomationRequests>().pending);
    for (id, operation) in pending {
        if owned.contains(&id) {
            let frame = super::frame(world);
            world.resource_mut::<AutomationResults>().finish(
                id,
                Err(AutomationError::new(
                    ErrorKind::Failed,
                    "connection closed before submission",
                )),
                frame,
            );
        } else {
            world
                .resource_mut::<AutomationRequests>()
                .pending
                .push_back((id, operation));
        }
    }
    cancel_inputs(world, Some(owned));
}

fn cancel_inputs(world: &mut World, owned: Option<&HashSet<OperationId>>) {
    let pending = std::mem::take(&mut world.resource_mut::<AutomationRequests>().pending);
    for (id, operation) in pending {
        if matches!(operation, Operation::Input(_)) && owned.is_none_or(|owned| owned.contains(&id))
        {
            let frame = super::frame(world);
            world.resource_mut::<AutomationResults>().finish(
                id,
                Err(AutomationError::new(ErrorKind::Failed, "input cancelled")),
                frame,
            );
        } else {
            world
                .resource_mut::<AutomationRequests>()
                .pending
                .push_back((id, operation));
        }
    }
    let ids = world
        .resource::<ActiveOperations>()
        .0
        .iter()
        .filter_map(|(id, pending)| {
            if owned.is_none_or(|owned| owned.contains(id))
                && let Pending::Input(input) = pending
            {
                Some(*input)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if let Some(mut input) = world.get_resource_mut::<DesktopInputRequests>() {
        for id in ids {
            input.cancel(id);
        }
    }
}
fn input_busy(world: &World) -> bool {
    world
        .resource::<ActiveOperations>()
        .0
        .iter()
        .any(|(_, operation)| matches!(operation, Pending::Input(_)))
}

fn update(world: &mut World) {
    let mut control = world.remove_non_send::<Control>().expect("transport owner");
    let enabled = control.options.allow
        || world
            .get_resource::<SettingsSnapshot>()
            .and_then(|settings| settings.get("bedtime.automation", "enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if enabled && control.server.is_none() && control.error.is_none() {
        match Server::bind(&control.options) {
            Ok(server) => control.server = Some(server),
            Err(error) => {
                eprintln!("automation: {error}");
                control.error = Some(error.to_string());
            }
        }
    }
    if !enabled {
        if let Some(mut server) = control.server.take() {
            for client in server.clients.drain(..) {
                detach(world, &client.operations);
                server.orphans.extend(client.operations);
            }
            control.orphans.extend(std::mem::take(&mut server.orphans));
        }
        control.error = None;
    }
    if let Some(server) = &mut control.server {
        for _ in 0..MAX_CLIENTS {
            match server.listener.accept() {
                Ok((stream, _))
                    if server.clients.len() < MAX_CLIENTS
                        && same_user(&stream).unwrap_or(false) =>
                {
                    let (tx, incoming) = mpsc::sync_channel(32);
                    let (outgoing, rx) = mpsc::sync_channel(8);
                    thread::spawn(move || worker(stream, tx, rx));
                    server.next += 1;
                    server.clients.push(Client {
                        id: server.next,
                        name: None,
                        approved: false,
                        revoked: false,
                        since: Instant::now(),
                        incoming,
                        outgoing,
                        operations: HashSet::new(),
                        released: HashSet::new(),
                        dependencies: HashMap::new(),
                    });
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    control.error = Some(error.to_string());
                    break;
                }
            }
        }
        // Process handshakes first, then freeze injection before rendering a prompt.
        for client in &mut server.clients {
            for _ in 0..16 {
                match client.incoming.try_recv() {
                    Ok(value) if !client.revoked => {
                        request(world, client, value, &control.options, &control.application)
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        client.revoked = true;
                        break;
                    }
                }
            }
            let expired = if client.name.is_none() {
                client.since.elapsed() > HANDSHAKE_TIMEOUT
            } else {
                !client.approved && client.since.elapsed() > APPROVAL_TIMEOUT
            };
            client.revoked |= expired;
            client.dependencies.retain(|capture, _| {
                world.resource::<AutomationResults>().status(*capture)
                    != Some(OperationStatus::Complete)
            });
            let complete = client
                .released
                .iter()
                .copied()
                .filter(|id| {
                    !client.dependencies.values().any(|after| after == id)
                        && world.resource::<AutomationResults>().status(*id)
                            == Some(OperationStatus::Complete)
                })
                .collect::<Vec<_>>();
            for id in complete {
                client.dependencies.remove(&id);
                world.resource_mut::<AutomationResults>().release(id);
                client.operations.remove(&id);
                client.released.remove(&id);
            }
        }
        let pending = server
            .clients
            .iter()
            .any(|client| client.name.is_some() && !client.approved && !client.revoked);
        world.resource_mut::<InputBlocked>().0 = pending;
        if pending {
            cancel_inputs(world, None);
        }
        control.blocked_inputs = input_busy(world);
        let mut kept = Vec::new();
        for client in server.clients.drain(..) {
            if client.revoked {
                detach(world, &client.operations);
                server.orphans.extend(client.operations);
            } else {
                kept.push(client);
            }
        }
        server.clients = kept;
        // Wait for all orphaned dependencies before retiring their receipts.
        if server.orphans.iter().all(|id| {
            world.resource::<AutomationResults>().status(*id) != Some(OperationStatus::Pending)
        }) {
            for id in server.orphans.drain(..) {
                world.resource_mut::<AutomationResults>().release(id);
            }
        }
    } else {
        world.resource_mut::<InputBlocked>().0 = false;
    }
    if control.orphans.iter().all(|id| {
        world.resource::<AutomationResults>().status(*id) != Some(OperationStatus::Pending)
    }) {
        for id in control.orphans.drain(..) {
            world.resource_mut::<AutomationResults>().release(id);
        }
    }
    world.insert_non_send(control);
}

/// Decisions are tied to the live connection, never its claimed name.
pub fn answer_connection(world: &mut World, id: u64, allow: bool) -> bool {
    let Some(mut control) = world.get_non_send_mut::<Control>() else {
        return false;
    };
    let Some(client) = control.server.as_mut().and_then(|server| {
        server.clients.iter_mut().find(|client| {
            client.id == id && !client.revoked && !client.approved && client.name.is_some()
        })
    }) else {
        return false;
    };
    client.approved = allow;
    client.revoked = !allow;
    true
}
pub fn disconnect(world: &mut World, id: u64) -> bool {
    let Some(mut control) = world.get_non_send_mut::<Control>() else {
        return false;
    };
    let Some(client) = control
        .server
        .as_mut()
        .and_then(|server| server.clients.iter_mut().find(|client| client.id == id))
    else {
        return false;
    };
    client.revoked = true;
    true
}
fn draw(world: &mut World, ui: &Ui) {
    let Some(control) = world.get_non_send::<Control>() else {
        return;
    };
    let Some(server) = &control.server else {
        return;
    };
    let path = server.endpoint.path.display().to_string();
    let approved_names = control.options.approved_names.clone();
    let clients = server
        .clients
        .iter()
        .filter(|client| !client.revoked)
        .filter_map(|client| {
            client
                .name
                .as_ref()
                .map(|name| (client.id, name.clone(), client.approved))
        })
        .collect::<Vec<_>>();
    let pending = if control.blocked_inputs {
        None
    } else {
        clients.iter().find(|(_, _, approved)| !approved).cloned()
    };
    if let Some((id, name, _)) = pending {
        ui.open_popup("Allow automation?##bedtime-automation");
        if let Some(_popup) = ui.begin_modal_popup("Allow automation?##bedtime-automation") {
            ui.text_wrapped(format!(
                "Client {name:?} requests control of this application."
            ));
            ui.text_wrapped("Allows reading documents, screenshots, editing files and invoking actions, including terminal input. The client name is self-reported.");
            if ui.button("Deny") {
                answer_connection(world, id, false);
                ui.close_current_popup();
            }
            ui.same_line();
            if ui.button("Allow this connection") {
                answer_connection(world, id, true);
                ui.close_current_popup();
            }
        }
    } else {
        // Close a modal whose connection disappeared while awaiting approval.
        if let Some(_popup) = ui.begin_modal_popup("Allow automation?##bedtime-automation") {
            ui.close_current_popup();
        }
    }
    let visible_clients = clients
        .into_iter()
        .filter(|(_, name, _)| !approved_names.iter().any(|approved| approved == name))
        .collect::<Vec<_>>();
    if !visible_clients.is_empty() {
        ui.window("Automation connections")
            .flags(dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                ui.text_wrapped(&path);
                for (id, name, approved) in visible_clients {
                    let _id = ui.push_id(id as i32);
                    ui.text(format!(
                        "{name} ({})",
                        if approved {
                            "approved"
                        } else {
                            "awaiting approval"
                        }
                    ));
                    ui.same_line();
                    if ui.button("Disconnect") {
                        disconnect(world, id);
                    }
                }
            });
    }
}
fn stop(world: &mut World) {
    if world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|exit| !exit.is_empty())
        && let Some(mut control) = world.get_non_send_mut::<Control>()
    {
        control.server = None;
    }
}

fn settings_page(
    world: &mut World,
    ui: &Ui,
    context: bedtime_api::settings::SettingsPageContext,
    changes: &mut Vec<bedtime_api::settings::SettingsChange>,
) {
    let mut enabled = context
        .snapshot
        .get("bedtime.automation", "enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if ui.checkbox("Allow external automation", &mut enabled) {
        changes.push(bedtime_api::settings::SettingsChange::set(
            "bedtime.automation",
            "enabled",
            enabled,
        ));
    }
    ui.text_wrapped("Each client requires approval. Approved clients can read documents, capture panels and control application actions.");
    if let Some(control) = world.get_non_send::<Control>() {
        if control.options.allow {
            ui.text_wrapped("Automation is enabled by a launch override for this instance.");
        }
        if let Some(server) = &control.server {
            ui.text_wrapped(format!("Socket: {}", server.endpoint.path.display()));
        }

        if let Some(error) = &control.error {
            ui.text_wrapped(error);
        }
    }
    let clients = world
        .get_non_send::<Control>()
        .and_then(|control| control.server.as_ref())
        .map(|server| {
            server
                .clients
                .iter()
                .filter_map(|client| {
                    client
                        .name
                        .as_ref()
                        .map(|name| (client.id, name.clone(), client.approved))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (id, name, approved) in clients {
        ui.text(format!(
            "{name} ({})",
            if approved {
                "approved"
            } else {
                "awaiting approval"
            }
        ));
        ui.same_line();
        let _id = ui.push_id(id as i32);
        if ui.button("Disconnect") {
            disconnect(world, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn app(approved_names: &[&str]) -> App {
        let mut app = crate::tests::app();
        app.add_plugins(TransportPlugin {
            options: LaunchOptions {
                allow: true,
                approved_names: approved_names.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            application: "fixture".into(),
        });
        app.update();
        app
    }
    fn connect(app: &App) -> UnixStream {
        let control = app.world().non_send::<Control>();
        let stream = UnixStream::connect(&control.server.as_ref().unwrap().endpoint.path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
    }
    fn exchange(app: &mut App, stream: &mut UnixStream, value: Value) -> Value {
        writeln!(stream, "{value}").unwrap();
        for _ in 0..20 {
            app.update();
            thread::sleep(Duration::from_millis(2));
        }
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn hello(app: &mut App, stream: &mut UnixStream, name: &str) -> Value {
        exchange(
            app,
            stream,
            json!({"id":1,"method":"hello","params":{"protocol":1,"name":name}}),
        )["result"]
            .clone()
    }
    #[test]
    fn flags_require_enablement_and_match_names_exactly() {
        let mut args = [
            "--approve-automation=smoke test",
            "--allow-automation",
            "--approve-automation",
            "second",
            "file.txt",
        ]
        .map(OsString::from)
        .to_vec();
        let options = LaunchOptions::extract(&mut args).unwrap();
        assert!(options.approves("smoke test"));
        assert!(options.approves("second"));
        assert!(!options.approves("Smoke test"));
        assert!(!options.approves("smoke test "));
        assert_eq!(args, vec![OsString::from("file.txt")]);
        for args in [
            vec!["--approve-automation=x"],
            vec!["--allow-automation", "--approve-automation"],
            vec!["--allow-automation", "--approve-automation="],
            vec!["--allow-automation", "--approve-automation=*"],
            vec![
                "--allow-automation",
                "--approve-automation",
                "--allow-automation",
            ],
        ] {
            assert!(
                LaunchOptions::extract(&mut args.into_iter().map(OsString::from).collect())
                    .is_err()
            );
        }
    }
    #[test]
    fn unapproved_clients_cannot_execute_or_inspect_and_reconnections_need_approval() {
        let mut app = app(&[]);
        let mut stream = connect(&app);
        let status = hello(&mut app, &mut stream, "agent");
        assert_eq!(status["state"], "pending");
        let result = exchange(
            &mut app,
            &mut stream,
            json!({"id":2,"method":"submit","params":{"operation":"open_panel","arguments":{"panel_type":"fixture.panel"}}}),
        );
        assert!(result.get("error").is_some());
        assert!(
            app.world()
                .resource::<AutomationRequests>()
                .pending
                .is_empty()
        );
        let id = status["connection"].as_u64().unwrap();
        assert!(answer_connection(app.world_mut(), id, true));
        assert!(!answer_connection(app.world_mut(), id, true));
        assert_eq!(
            exchange(
                &mut app,
                &mut stream,
                json!({"id":3,"method":"connection_status"})
            )["result"]["state"],
            "approved"
        );
        assert!(disconnect(app.world_mut(), id));
        app.update();
        let mut another = connect(&app);
        assert_eq!(hello(&mut app, &mut another, "agent")["state"], "pending");
        let id = hello_id(&app);
        assert!(answer_connection(app.world_mut(), id, false));
        app.update();
        assert!(
            app.world()
                .non_send::<Control>()
                .server
                .as_ref()
                .unwrap()
                .clients
                .iter()
                .all(|client| client.name.is_none())
        );
    }
    fn hello_id(app: &App) -> u64 {
        app.world()
            .non_send::<Control>()
            .server
            .as_ref()
            .unwrap()
            .clients
            .iter()
            .find(|c| c.name.is_some())
            .unwrap()
            .id
    }
    #[test]
    fn exact_named_override_preserves_ui_gate_for_other_clients() {
        let mut app = app(&["smoke test"]);
        let mut allowed = connect(&app);
        assert_eq!(
            hello(&mut app, &mut allowed, "smoke test")["state"],
            "approved"
        );
        let mut pending = connect(&app);
        assert_eq!(hello(&mut app, &mut pending, "other")["state"], "pending");
        let result = exchange(
            &mut app,
            &mut allowed,
            json!({"id":2,"method":"submit","params":{"operation":"input","arguments":{"steps":[[{"type":"text","text":"approve"}]]}}}),
        );
        assert!(result.get("error").is_some());
    }
    #[test]
    fn receipts_belong_to_connection_and_release_retires_them() {
        let mut app = app(&["smoke test"]);
        let mut stream = connect(&app);
        hello(&mut app, &mut stream, "smoke test");
        let operation = exchange(
            &mut app,
            &mut stream,
            json!({"id":2,"method":"submit","params":{"operation":"inspect"}}),
        )["result"]["operation"]
            .as_u64()
            .unwrap();
        let mut second = connect(&app);
        hello(&mut app, &mut second, "smoke test");
        assert!(
            exchange(
                &mut app,
                &mut second,
                json!({"id":2,"method":"poll","params":{"operation":operation}})
            )
            .get("error")
            .is_some()
        );
        let result = exchange(
            &mut app,
            &mut stream,
            json!({"id":3,"method":"poll","params":{"operation":operation}}),
        );
        assert_eq!(result["result"]["state"], "complete");
        assert!(result["result"]["value"]["panels"].is_array());
        exchange(
            &mut app,
            &mut stream,
            json!({"id":4,"method":"release","params":{"operation":operation}}),
        );
        assert_eq!(
            app.world()
                .resource::<AutomationResults>()
                .status(OperationId(operation)),
            None
        );
    }
    #[test]
    fn malformed_frames_and_version_mismatches_close_without_execution() {
        let mut app = app(&["smoke test"]);
        let mut malformed = connect(&app);
        writeln!(malformed, "not json").unwrap();
        for _ in 0..20 {
            app.update();
            thread::sleep(Duration::from_millis(2));
        }
        let mut byte = [0u8; 1];
        assert_eq!(malformed.read(&mut byte).unwrap(), 0);
        let mut mismatch = connect(&app);
        let response = exchange(
            &mut app,
            &mut mismatch,
            json!({"id":1,"method":"hello","params":{"protocol":2,"name":"smoke test"}}),
        );
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("protocol")
        );
        assert!(
            app.world()
                .resource::<AutomationRequests>()
                .pending
                .is_empty()
        );
    }
    #[test]
    fn receipt_capacity_is_bounded_and_disconnect_drains_owned_results() {
        let mut app = app(&["smoke test"]);
        let mut stream = connect(&app);
        hello(&mut app, &mut stream, "smoke test");
        for id in 2..(MAX_OPERATIONS as u64 + 2) {
            let response = exchange(
                &mut app,
                &mut stream,
                json!({"id":id,"method":"submit","params":{"operation":"inspect"}}),
            );
            assert!(response.get("error").is_none());
        }
        let response = exchange(
            &mut app,
            &mut stream,
            json!({"id":100,"method":"submit","params":{"operation":"inspect"}}),
        );
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("release")
        );
        drop(stream);
        for _ in 0..20 {
            app.update();
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            app.world()
                .resource::<AutomationResults>()
                .complete
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<AutomationResults>()
                .statuses
                .lock()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn expired_unidentified_and_pending_connections_are_closed() {
        let mut app = app(&[]);
        let _unidentified = connect(&app);
        app.update();
        app.world_mut()
            .non_send_mut::<Control>()
            .server
            .as_mut()
            .unwrap()
            .clients[0]
            .since = Instant::now() - HANDSHAKE_TIMEOUT - Duration::from_secs(1);
        app.update();
        assert!(
            app.world()
                .non_send::<Control>()
                .server
                .as_ref()
                .unwrap()
                .clients
                .is_empty()
        );
        let mut pending = connect(&app);
        hello(&mut app, &mut pending, "agent");
        app.world_mut()
            .non_send_mut::<Control>()
            .server
            .as_mut()
            .unwrap()
            .clients[0]
            .since = Instant::now() - APPROVAL_TIMEOUT - Duration::from_secs(1);
        app.update();
        assert!(
            app.world()
                .non_send::<Control>()
                .server
                .as_ref()
                .unwrap()
                .clients
                .is_empty()
        );
        assert!(!app.world().resource::<InputBlocked>().0);
    }

    #[test]
    fn endpoint_is_private_and_removed_on_shutdown() {
        let app = app(&[]);
        let path = app
            .world()
            .non_send::<Control>()
            .server
            .as_ref()
            .unwrap()
            .endpoint
            .path
            .clone();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        drop(app);
        assert!(!path.exists());
    }
    #[test]
    fn disabled_setting_creates_no_listener_and_override_does_not_write_settings() {
        let mut app = crate::tests::app();
        app.insert_resource(SettingsSnapshot::default());
        app.add_plugins(TransportPlugin {
            options: LaunchOptions::default(),
            application: "fixture".into(),
        });
        app.update();
        assert!(app.world().non_send::<Control>().server.is_none());
        app.world_mut().non_send_mut::<Control>().options.allow = true;
        app.update();
        assert!(app.world().non_send::<Control>().server.is_some());
        assert_eq!(
            app.world()
                .resource::<SettingsSnapshot>()
                .get("bedtime.automation", "enabled"),
            None
        );
    }
}
