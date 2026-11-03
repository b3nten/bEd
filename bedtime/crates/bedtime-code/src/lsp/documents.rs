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

/// The server for each language (by syntax's language names), worked out
/// from `ServerCatalog` and the programs found (`config`).
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LanguageServers(pub BTreeMap<String, ServerConfig>);

/// A document's connection to its server, on the document's entity.
#[derive(Component)]
pub struct DocumentServer {
    pub(crate) file: PathBuf,
    /// Syntax's name for its language.
    pub language: &'static str,
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
    pub(super) handle: ServerHandle,
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

    /// Its name in the catalog (`rust-analyzer`), for showing.
    pub fn name(&self) -> &str {
        &self.launch.config.name
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

/// The latest go-to-definition request: only its answer is opened.
#[derive(Resource, Default)]
struct Definitions {
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
    super::actions::install(app);
    app.init_resource::<LanguageServers>()
        .init_resource::<super::InstalledServers>()
        .init_resource::<Definitions>()
        .init_resource::<Queries>()
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
        .add_systems(Update, (super::config::update_servers, update).chain());
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

/// The server of `panel`'s document, if it can answer a question now: it's
/// running, it isn't loading the project, and it has the document as it is
/// (sent now if it hadn't yet). Otherwise why not: nothing waits.
pub(super) fn answering_server(
    world: &mut World,
    panel: Entity,
) -> Result<(Entity, RequestOrigin), String> {
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
    let (server, key) = (connection.server, connection.key);
    let state = world
        .get::<LanguageServer>(server)
        .ok_or("The language server has stopped")?;
    let program = state.name().to_owned();
    match state.status() {
        ServerStatus::Ready => {}
        ServerStatus::Starting => return Err(format!("{program} is starting")),
        ServerStatus::Failed(error) => return Err(format!("{program} failed: {error}")),
        ServerStatus::Stopped => return Err(format!("{program} has stopped")),
    }
    if state
        .activity()
        .project_status
        .as_ref()
        .is_some_and(|status| !status.quiescent)
    {
        return Err(format!("{program} is loading the project"));
    }
    send_revision(world, document.entity());
    let connection = world.get::<DocumentServer>(document.entity()).unwrap();
    if connection.revision != Some(info.revision) {
        return Err(connection
            .error
            .clone()
            .unwrap_or_else(|| format!("{program} doesn't have this text yet")));
    }
    Ok((
        server,
        RequestOrigin {
            document: key,
            revision: info.revision,
            panel: panel.to_bits(),
            ticket: 0,
        },
    ))
}

/// Ask the server of `panel`'s document where the symbol at `byte` is
/// defined, and open it. A server that can't answer now is an error now.
pub fn go_to_definition(world: &mut World, panel: Entity, byte: usize) -> Result<(), String> {
    let (server, mut origin) = answering_server(world, panel)?;
    let mut definitions = world.resource_mut::<Definitions>();
    definitions.next_ticket += 1;
    origin.ticket = definitions.next_ticket;
    // Only the latest request's answer is opened.
    definitions.latest = Some(origin);
    world
        .get::<LanguageServer>(server)
        .unwrap()
        .handle
        .definition(origin, byte)?;
    Ok(())
}

/// Each editor's latest hover, completion and resolve request, and the
/// answers not taken yet.
#[derive(Resource, Default)]
struct Queries {
    latest: HashMap<(Entity, QueryKind), RequestOrigin>,
    answers: Vec<QueryAnswer>,
    next_ticket: u64,
}

/// A server's answer to a query.
pub struct QueryAnswer {
    pub origin: RequestOrigin,
    pub kind: QueryKind,
    pub result: Result<Intelligence, String>,
}

/// What the server of `panel`'s document can do and is doing, while it's ready.
pub fn language_activity(world: &World, panel: Entity) -> Option<Arc<ServerActivity>> {
    let server = world.get::<LanguageServer>(server_of(world, panel)?)?;
    (server.status() == ServerStatus::Ready).then(|| server.activity())
}

/// Ask `panel`'s server a question about its document as it is now; an
/// error if it can't answer now. It replaces that editor's previous query of
/// the same kind.
pub fn request_query(
    world: &mut World,
    panel: Entity,
    query: LanguageQuery,
) -> Result<RequestOrigin, String> {
    let (server, mut origin) = answering_server(world, panel)?;
    let kind = query.kind();
    let mut queries = world.resource_mut::<Queries>();
    queries.next_ticket += 1;
    origin.ticket = queries.next_ticket;
    queries.latest.insert((panel, kind), origin);
    world
        .get::<LanguageServer>(server)
        .unwrap()
        .handle
        .query(origin, query)?;
    Ok(origin)
}

/// Forget `panel`'s query of `kind`, and tell the server.
pub fn cancel_query(world: &mut World, panel: Entity, kind: QueryKind) {
    let mut queries = world.resource_mut::<Queries>();
    queries.latest.remove(&(panel, kind));
    if let Some(server) = server_of(world, panel).and_then(|s| world.get::<LanguageServer>(s)) {
        let _ = server.handle.cancel_query(panel.to_bits(), kind);
    }
}

/// Whether `origin` is still its editor's latest query of `kind`, for the
/// document as it is now.
pub fn query_is_current(world: &World, origin: &RequestOrigin, kind: QueryKind) -> bool {
    let Some(panel) = Entity::try_from_bits(origin.panel) else {
        return false;
    };
    world.resource::<Queries>().latest.get(&(panel, kind)) == Some(origin)
        && document_is_at(world, panel, origin)
}

/// `panel` still shows the document `origin` asked about, at its revision.
fn document_is_at(world: &World, panel: Entity, origin: &RequestOrigin) -> bool {
    let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) else {
        return false;
    };
    world
        .get::<DocumentServer>(document.entity())
        .is_some_and(|connection| connection.key == origin.document)
        && world
            .workbench()
            .inspect_document(document)
            .is_ok_and(|info| info.revision == origin.revision && !info.has_drafts)
}

/// The answers that arrived since last taken.
pub fn take_query_answers(world: &mut World) -> Vec<QueryAnswer> {
    std::mem::take(&mut world.resource_mut::<Queries>().answers)
}

/// Restart a server (it shuts down, then starts again for its documents).
pub fn restart(world: &mut World, server: Entity) -> Result<(), String> {
    world
        .get_mut::<LanguageServer>(server)
        .ok_or("The language server is gone")?
        .retire(Lifecycle::Restarting);
    Ok(())
}

/// Each running server's state, by its name in the catalog: the server,
/// its status, and how many documents it has.
pub fn server_states(world: &mut World) -> HashMap<String, (Entity, ServerStatus, usize)> {
    world
        .query::<(Entity, &LanguageServer)>()
        .iter(world)
        .map(|(entity, server)| {
            (
                server.launch.config.name.clone(),
                (entity, server.status(), server.documents.len()),
            )
        })
        .collect()
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
    forget_stale_queries(world);
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
                super::Event::Answer {
                    origin,
                    kind,
                    result,
                } => super::actions::answered(world, origin, kind, result),
                super::Event::ApplyEdit(edit) => {
                    if let Err(error) = super::actions::apply_workspace_edit(world, &edit, "Edit") {
                        world
                            .workbench_mut()
                            .notify(Notification::warning(format!("Edit: {error}")));
                    }
                }
                super::Event::Intelligence {
                    origin,
                    kind,
                    result,
                } => {
                    if query_is_current(world, &origin, kind) {
                        let answer = QueryAnswer {
                            origin,
                            kind,
                            result,
                        };
                        // The editor's own queries go straight to it.
                        match Entity::try_from_bits(origin.panel) {
                            Some(panel) if super::assist::assists(world, panel) => {
                                super::assist::accept(world, answer)
                            }
                            _ => world.resource_mut::<Queries>().answers.push(answer),
                        }
                    }
                }
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
        let ready = workbench.cast_document::<TextBuffer>(id).is_ok()
            && workbench
                .inspect_document(id)
                .is_ok_and(|info| info.status == bedtime_workbench::DocumentStatus::Ready);
        if !ready {
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
        send_revision(world, document);
    }
}

/// Bring the server's copy of `document` up to date: its changes since the
/// last revision sent when they're all known, else the whole text. Problems
/// are kept on the connection (`error`).
fn send_revision(world: &mut World, document: Entity) {
    let id = DocumentId(document);
    let workbench = world.workbench();
    let (Ok(handle), Ok(info)) = (
        workbench.cast_document::<TextBuffer>(id),
        workbench.inspect_document(id),
    ) else {
        return;
    };
    let Some(connection) = world.get::<DocumentServer>(document) else {
        return;
    };
    if connection.revision == Some(info.revision) {
        return;
    }
    let (server, key, base) = (connection.server, connection.key, connection.revision);
    let (file, language) = (connection.file.clone(), connection.language);
    let text = workbench.read_document(handle).unwrap().clone();
    if text.len() > MAX_SOURCE_BYTES {
        let mut connection = world.get_mut::<DocumentServer>(document).unwrap();
        connection.error = Some("Language servers see files up to 8 MiB".into());
        connection.diagnostics = None;
        connection.revision = Some(info.revision);
        return;
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
    let result = FileUri::from_path(&file).and_then(|uri| {
        world
            .get::<LanguageServer>(server)
            .ok_or_else(|| "The language server has stopped".to_owned())?
            .handle
            .update(DocumentUpdate {
                key,
                uri,
                language: protocol_language(language).to_owned(),
                revision: info.revision,
                text,
                changes,
            })
    });
    if result.is_ok()
        && let Some(mut server) = world.get_mut::<LanguageServer>(server)
    {
        server.documents.insert(key);
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

/// Forget (and cancel) queries about text that has changed since.
fn forget_stale_queries(world: &mut World) {
    let stale: Vec<(Entity, QueryKind)> = {
        let queries = world.resource::<Queries>();
        queries
            .latest
            .iter()
            .filter(|((panel, _), origin)| !document_is_at(world, *panel, origin))
            .map(|(key, _)| *key)
            .collect()
    };
    for (panel, kind) in stale {
        cancel_query(world, panel, kind);
    }
}
