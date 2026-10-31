//! Language servers for the files editors show. A text document gets a
//! server when an editor shows it from a file (`ViewerFile`) whose language
//! has one (`LanguageServers`), under the workbench's `Root`. One server runs
//! per configuration and root, shared by its documents; each document is kept
//! in sync, revision by revision. Diagnostics go to the document's editors;
//! go-to-definition opens the answer with `open_path`.
use super::*;
use crate::editor::EditorPanel;
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, DocumentId, DocumentReference,
    DocumentSaveFinished, Handler, KeyChord, ManagedTask, ManagedWork, Notification, PathOptions,
    Root, ViewerFile, WorkStatus, WorkbenchWorldExt,
};
use bevy::{ecs::message::MessageCursor, prelude::*};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

/// The server for each language (by syntax's language names). Rust has
/// rust-analyzer unless the app says otherwise.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct LanguageServers(pub BTreeMap<String, ServerConfig>);

impl Default for LanguageServers {
    fn default() -> Self {
        Self([("rust".to_owned(), ServerConfig::default())].into())
    }
}

/// A document's connection to its server, on the document's entity.
#[derive(Component)]
pub struct DocumentServer {
    file: PathBuf,
    language: &'static str,
    key: DocumentKey,
    /// The server's entity (a `LanguageServer`).
    pub server: Entity,
    /// The revision the server has.
    revision: Option<u64>,
    /// The latest diagnostics, drawn while the text is at their revision.
    pub diagnostics: Option<Arc<Diagnostics>>,
    /// The latest problem with this document's connection.
    pub error: Option<String>,
}

/// A running server, on its own entity.
#[derive(Component)]
pub struct LanguageServer {
    launch: Launch,
    handle: ServerHandle,
    task: ManagedTask<()>,
    lifecycle: Lifecycle,
    documents: HashSet<DocumentKey>,
}

/// A stopped server keeps its entity so syncing doesn't start it again;
/// restarting replaces it once the old one has shut down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Running,
    Restarting,
    Stopping,
    Stopped,
}

impl LanguageServer {
    pub fn program(&self) -> &str {
        &self.launch.config.program
    }

    pub fn status(&self) -> ServerStatus {
        match self.lifecycle {
            Lifecycle::Running => self.handle.status(),
            _ => ServerStatus::Stopped,
        }
    }

    /// What it's doing: progress, messages, its stderr.
    pub fn activity(&self) -> Arc<ServerActivity> {
        self.handle.activity()
    }

    fn retiring(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Restarting | Lifecycle::Stopping)
    }

    fn retire(&mut self, lifecycle: Lifecycle) {
        self.lifecycle = lifecycle;
        self.task.cancel();
    }
}

pub struct GoToDefinition;
impl Action for GoToDefinition {
    const KEY: &'static str = "code.go_to_definition";
    const LABEL: &'static str = "Go to Definition";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        bedtime_workbench::Modifiers::NONE,
        KeyCode::F12,
    )];
}

pub struct RestartLanguageServer;
impl Action for RestartLanguageServer {
    const KEY: &'static str = "code.restart_language_server";
    const LABEL: &'static str = "Restart Language Server";
}

/// Go-to-definition requests waiting for the server to have their revision,
/// and the latest one (only its answer is opened).
#[derive(Resource, Default)]
struct Definitions {
    waiting: Vec<(RequestOrigin, usize)>,
    latest: Option<RequestOrigin>,
    next_ticket: u64,
}

/// Language servers for the files editors show (part of `CodePlugin`).
pub struct LanguageServersPlugin;

impl Plugin for LanguageServersPlugin {
    fn build(&self, app: &mut App) {
        install(app);
    }
}

fn install(app: &mut App) {
    app.init_resource::<LanguageServers>()
        .init_resource::<Definitions>()
        .add_action_handler::<GoToDefinition>(
            Handler::new(go_to_definition_action).available_when(|world, panel| {
                panel.is_some_and(|p| server_of(world, p).is_some())
            }),
        )
        .add_action_handler::<RestartLanguageServer>(
            Handler::new(restart_action).available_when(|world, panel| {
                panel.is_some_and(|p| server_of(world, p).is_some())
            }),
        )
        .add_systems(Update, update);
}

/// The server of the document `panel` shows.
pub fn server_of(world: &World, panel: Entity) -> Option<Entity> {
    let document = world.get::<DocumentReference>(panel)?.document();
    Some(world.get::<DocumentServer>(document.entity())?.server)
}

fn go_to_definition_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let panel = invocation.entity.ok_or("No editor is focused")?;
    let byte = world
        .get::<EditorPanel>(panel)
        .ok_or("No editor is focused")?
        .view
        .selection()
        .head();
    go_to_definition(world, panel, byte)
}

fn restart_action(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let panel = invocation.entity.ok_or("No editor is focused")?;
    let server = server_of(world, panel).ok_or("No language server")?;
    restart(world, server)
}

/// Ask the server of `panel`'s document where the symbol at `byte` is
/// defined, and open it. A server that isn't ready is an error now, not later.
pub fn go_to_definition(world: &mut World, panel: Entity, byte: usize) -> Result<(), String> {
    let document = world
        .get::<DocumentReference>(panel)
        .ok_or("No document is shown")?
        .document();
    let info = world
        .workbench()
        .inspect_document(document)
        .map_err(|e| e.to_string())?;
    if info.has_drafts {
        return Err("Finish typing first".into());
    }
    let connection = world
        .get::<DocumentServer>(document.entity())
        .ok_or("No language server is set up for this file")?;
    let key = connection.key;
    let server = world
        .get::<LanguageServer>(connection.server)
        .ok_or("The language server has stopped")?;
    match server.status() {
        ServerStatus::Ready => {}
        ServerStatus::Starting => return Err(format!("{} is starting", server.program())),
        ServerStatus::Failed(error) => return Err(format!("{} failed: {error}", server.program())),
        ServerStatus::Stopped => return Err(format!("{} has stopped", server.program())),
    }
    let mut definitions = world.resource_mut::<Definitions>();
    definitions.next_ticket += 1;
    let origin = RequestOrigin {
        document: key,
        revision: info.revision,
        panel: panel.to_bits(),
        ticket: definitions.next_ticket,
    };
    // A newer request replaces an older one.
    definitions.waiting.clear();
    definitions.waiting.push((origin, byte));
    definitions.latest = Some(origin);
    Ok(())
}

/// Restart a server (it shuts down, then starts again for its documents).
pub fn restart(world: &mut World, server: Entity) -> Result<(), String> {
    world
        .get_mut::<LanguageServer>(server)
        .ok_or("The language server is gone")?
        .retire(Lifecycle::Restarting);
    Ok(())
}

/// Stop a server until it's restarted.
pub fn stop(world: &mut World, server: Entity) -> Result<(), String> {
    world
        .get_mut::<LanguageServer>(server)
        .ok_or("The language server is gone")?
        .retire(Lifecycle::Stopping);
    Ok(())
}

fn next_key(document: Entity) -> DocumentKey {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    DocumentKey {
        id: document.to_bits(),
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
    }
}

/// A document's file and language, from an editor showing it.
fn shown_files(world: &mut World) -> HashMap<Entity, (PathBuf, &'static str)> {
    world
        .query_filtered::<(&DocumentReference, &ViewerFile), With<EditorPanel>>()
        .iter(world)
        .filter_map(|(reference, file)| {
            let language = crate::syntax::language_name(&file.0.to_string_lossy())?;
            Some((reference.document().entity(), (file.0.clone(), language)))
        })
        .collect()
}

fn update(world: &mut World, mut saves: Local<MessageCursor<DocumentSaveFinished>>) {
    let shown = shown_files(world);
    retire_servers(world);
    receive(world);
    disconnect(world, &shown);
    connect_and_sync(world, &shown);
    send_definitions(world);
    // Servers check saved files (rust-analyzer runs `cargo check`). Only a
    // document with nothing unsaved has its saved text in memory.
    let saved: Vec<DocumentId> = saves
        .read(world.resource::<Messages<DocumentSaveFinished>>())
        .filter(|saved| saved.result.is_ok())
        .map(|saved| saved.document)
        .collect();
    for document in saved {
        let Some(connection) = world.get::<DocumentServer>(document.entity()) else {
            continue;
        };
        let workbench = world.workbench();
        let Some(text) = workbench
            .inspect_document(document)
            .ok()
            .filter(|info| !info.dirty)
            .and_then(|_| workbench.cast_document::<TextBuffer>(document).ok())
            .and_then(|handle| workbench.read_document(handle).ok().cloned())
        else {
            continue;
        };
        if let Some(server) = world.get::<LanguageServer>(connection.server) {
            let _ = server.handle.saved(connection.key, text);
        }
    }
}

/// Finish restarting and stopping servers, and stop servers whose
/// configuration or root is no longer wanted.
fn retire_servers(world: &mut World) {
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let servers: Vec<Entity> = world
        .query_filtered::<Entity, With<LanguageServer>>()
        .iter(world)
        .collect();
    for entity in servers {
        let wanted = {
            let server = world.get::<LanguageServer>(entity).unwrap();
            root.as_ref() == Some(&server.launch.root)
                && world
                    .resource::<LanguageServers>()
                    .0
                    .values()
                    .any(|config| config == &server.launch.config)
        };
        let mut server = world.get_mut::<LanguageServer>(entity).unwrap();
        if !wanted && server.lifecycle == Lifecycle::Running {
            server.retire(Lifecycle::Restarting);
        }
        let finished = !matches!(
            server.task.status(),
            WorkStatus::Running | WorkStatus::Cancelling
        );
        if server.retiring() && finished {
            if server.lifecycle == Lifecycle::Restarting {
                world.despawn(entity);
            } else {
                server.lifecycle = Lifecycle::Stopped;
            }
        }
    }
}

/// Take what servers have sent: diagnostics, errors and definitions.
fn receive(world: &mut World) {
    let started = std::time::Instant::now();
    let servers: Vec<Entity> = world
        .query::<(Entity, &LanguageServer)>()
        .iter(world)
        .filter(|(_, server)| server.lifecycle == Lifecycle::Running)
        .map(|(entity, _)| entity)
        .collect();
    for entity in servers {
        // Bounded so a busy server can't stall a frame.
        for _ in 0..32 {
            if started.elapsed() > std::time::Duration::from_millis(2) {
                return;
            }
            let Ok(event) = world
                .get::<LanguageServer>(entity)
                .unwrap()
                .handle
                .try_recv()
            else {
                break;
            };
            match event {
                super::Event::Diagnostics { document, report } => {
                    if let Some(mut connection) = connection_mut(world, document, entity) {
                        connection.diagnostics = Some(report);
                        connection.error = None;
                    }
                }
                super::Event::DocumentError { document, message } => {
                    if let Some(mut connection) = connection_mut(world, document, entity) {
                        connection.error = Some(message);
                        connection.diagnostics = None;
                    }
                }
                super::Event::Definition { origin, result } => {
                    if world.resource::<Definitions>().latest == Some(origin) {
                        world.resource_mut::<Definitions>().latest = None;
                        open_definition(world, result);
                    }
                }
                // Hover and completion come with their UI.
                super::Event::Intelligence { .. } => {}
            }
        }
    }
}

fn connection_mut(
    world: &mut World,
    document: DocumentKey,
    server: Entity,
) -> Option<Mut<'_, DocumentServer>> {
    let entity = Entity::try_from_bits(document.id)?;
    world
        .get_mut::<DocumentServer>(entity)
        .filter(|connection| connection.key == document && connection.server == server)
}

fn open_definition(world: &mut World, result: Result<Vec<Location>, String>) {
    let opened = result.and_then(|locations| {
        let location = locations.first().ok_or("No definition found")?;
        let start = location.range.start;
        world
            .workbench_mut()
            .open_path(
                &location.uri.path(),
                PathOptions {
                    // Code units, taken as characters: exact for ASCII.
                    line: Some((start.line, start.character)),
                    ..default()
                },
            )
            .map(|_| ())
    });
    if let Err(error) = opened {
        world
            .workbench_mut()
            .notify(Notification::warning(format!("Go to Definition: {error}")));
    }
}

/// Close documents no editor shows any more (unless they have unsaved
/// changes), whose file or language changed, or whose server is gone.
fn disconnect(world: &mut World, shown: &HashMap<Entity, (PathBuf, &'static str)>) {
    let connected: Vec<Entity> = world
        .query_filtered::<Entity, With<DocumentServer>>()
        .iter(world)
        .collect();
    for document in connected {
        let connection = world.get::<DocumentServer>(document).unwrap();
        let dirty = world
            .workbench()
            .inspect_document(DocumentId(document))
            .is_ok_and(|info| info.dirty);
        let same_file = shown.get(&document).is_some_and(|(file, language)| {
            *file == connection.file && *language == connection.language
        });
        let server_running = world
            .get::<LanguageServer>(connection.server)
            .is_some_and(|server| server.lifecycle == Lifecycle::Running);
        let kept = server_running && (same_file || (dirty && !shown.contains_key(&document)));
        if !kept {
            let connection = world.entity_mut(document).take::<DocumentServer>().unwrap();
            if let Some(mut server) = world.get_mut::<LanguageServer>(connection.server) {
                if server.lifecycle == Lifecycle::Running
                    && server.handle.close(connection.key).is_err()
                {
                    server.retire(Lifecycle::Restarting);
                }
                server.documents.remove(&connection.key);
            }
        }
    }
}

/// Connect shown documents to their servers (starting one if needed), and
/// send each new revision: its changes when they're all known, else the text.
fn connect_and_sync(world: &mut World, shown: &HashMap<Entity, (PathBuf, &'static str)>) {
    if world.resource::<ManagedWork>().is_closing() {
        return;
    }
    let Some(root) = world.get_resource::<Root>().map(|root| root.0.clone()) else {
        return;
    };
    for (&document, (file, language)) in shown {
        let id = DocumentId(document);
        let workbench = world.workbench();
        let (Ok(handle), Ok(info)) = (
            workbench.cast_document::<TextBuffer>(id),
            workbench.inspect_document(id),
        ) else {
            continue;
        };
        if info.status != bedtime_workbench::DocumentStatus::Ready {
            continue;
        }
        if world.get::<DocumentServer>(document).is_none() {
            let Some(config) = world
                .resource::<LanguageServers>()
                .0
                .get(*language)
                .cloned()
            else {
                continue;
            };
            let Some(server) = server_for(world, config, root.clone()) else {
                continue;
            };
            world.entity_mut(document).insert(DocumentServer {
                file: file.clone(),
                language,
                key: next_key(document),
                server,
                revision: None,
                diagnostics: None,
                error: None,
            });
        }
        let connection = world.get::<DocumentServer>(document).unwrap();
        if connection.revision == Some(info.revision) {
            continue;
        }
        let (server, key, base) = (connection.server, connection.key, connection.revision);
        let workbench = world.workbench();
        let text = workbench.read_document(handle).unwrap().clone();
        if text.len() > MAX_SOURCE_BYTES {
            let mut connection = world.get_mut::<DocumentServer>(document).unwrap();
            connection.error = Some("Language servers see files up to 8 MiB".into());
            connection.diagnostics = None;
            connection.revision = Some(info.revision);
            continue;
        }
        let changes = base.and_then(|base| {
            let commits = workbench.document_changes_since(handle, base).ok()??;
            let edits: Vec<BufferEdit> = commits
                .into_iter()
                .map(|commit| commit.changes)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect();
            // A huge batch is cheaper sent whole.
            (edits.len() <= 4096
                && edits.iter().map(|e| e.replacement().len()).sum::<usize>() <= MAX_SOURCE_BYTES)
                .then_some((base, edits))
        });
        let uri = match FileUri::from_path(file) {
            Ok(uri) => uri,
            Err(error) => {
                world.get_mut::<DocumentServer>(document).unwrap().error = Some(error);
                continue;
            }
        };
        let result = world
            .get::<LanguageServer>(server)
            .unwrap()
            .handle
            .update(DocumentUpdate {
                key,
                uri,
                language: protocol_language(language).to_owned(),
                revision: info.revision,
                text,
                changes,
            });
        if result.is_ok() {
            world
                .get_mut::<LanguageServer>(server)
                .unwrap()
                .documents
                .insert(key);
        }
        let mut connection = world.get_mut::<DocumentServer>(document).unwrap();
        match result {
            Ok(()) => {
                connection.revision = Some(info.revision);
                connection.error = None;
            }
            Err(error) => connection.error = Some(error),
        }
    }
}

/// The running server for `config` under `root`, started if there isn't one.
/// A stopped one stays stopped.
fn server_for(world: &mut World, config: ServerConfig, root: PathBuf) -> Option<Entity> {
    let existing = world
        .query::<(Entity, &LanguageServer)>()
        .iter(world)
        .find(|(_, server)| server.launch.config == config && server.launch.root == root)
        .map(|(entity, server)| (entity, server.lifecycle == Lifecycle::Running));
    match existing {
        Some((entity, true)) => return Some(entity),
        Some((_, false)) => return None,
        None => {}
    }
    let launch = Launch { config, root };
    let (handle, worker) = ServerHandle::prepare(launch.clone()).ok()?;
    let entity = world.spawn_empty().id();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(entity),
        format!("Language server {}", launch.config.program),
        move |cancel| worker.run(|| cancel.is_cancelled()),
    );
    let Ok(task) = task else {
        world.despawn(entity);
        return None;
    };
    world.entity_mut(entity).insert(LanguageServer {
        launch,
        handle,
        task,
        lifecycle: Lifecycle::Running,
        documents: HashSet::new(),
    });
    Some(entity)
}

/// The protocol's language identifier for a syntax language name.
fn protocol_language(language: &str) -> &str {
    match language {
        "c-sharp" => "csharp",
        "bash" => "shellscript",
        "tsx" => "typescriptreact",
        language => language,
    }
}

/// Send waiting go-to-definition requests once the server has their revision.
fn send_definitions(world: &mut World) {
    let waiting = std::mem::take(&mut world.resource_mut::<Definitions>().waiting);
    for (origin, byte) in waiting {
        if world.resource::<Definitions>().latest != Some(origin) {
            continue;
        }
        let Some(document) = Entity::try_from_bits(origin.document.id) else {
            continue;
        };
        let Some(connection) = world
            .get::<DocumentServer>(document)
            .filter(|connection| connection.key == origin.document)
        else {
            continue;
        };
        if connection.revision != Some(origin.revision) {
            // Edited since: the request is stale.
            if connection.revision > Some(origin.revision) {
                continue;
            }
            world
                .resource_mut::<Definitions>()
                .waiting
                .push((origin, byte));
            continue;
        }
        let server = connection.server;
        if let Some(server) = world.get::<LanguageServer>(server)
            && let Err(error) = server.handle.definition(origin, byte)
        {
            world
                .workbench_mut()
                .notify(Notification::warning(format!("Go to Definition: {error}")));
        }
    }
}
