//! Canonical document adapter. Hosts project source/configuration components and
//! consume accepted navigation results. This plugin never opens files or edits.
use crate::Event;
use crate::*;
use bedtime_editor2::{TextBuffer, workbench::WorkbenchEditor};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Component, Clone, Debug, PartialEq)]
pub struct LspWorkspace {
    pub root: PathBuf,
    pub servers: BTreeMap<String, ServerConfig>,
}
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct LspDocument {
    pub uri: FileUri,
    pub language: String,
}
#[derive(Component)]
pub struct DocumentLsp {
    source: LspDocument,
    key: DocumentKey,
    server: Entity,
    revision: Option<u64>,
    pub diagnostics: Option<Arc<Diagnostics>>,
    pub error: Option<String>,
}
#[derive(Component)]
struct ManagedServer {
    launch: Launch,
    handle: ServerHandle,
    task: WorkspaceTask<()>,
    lifecycle: ServerLifecycle,
    bindings: HashSet<DocumentKey>,
}
// A stopped instance retains its launch identity so document reconciliation
// cannot immediately start it again. Restart retires that identity after cleanup.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServerLifecycle {
    Running,
    Restarting,
    Stopping,
    Stopped,
}
impl ManagedServer {
    fn retiring(&self) -> bool {
        matches!(
            self.lifecycle,
            ServerLifecycle::Restarting | ServerLifecycle::Stopping
        )
    }
}
pub struct DefinitionResult {
    pub origin: RequestOrigin,
    pub locations: Vec<Location>,
}
pub struct IntelligenceResult {
    pub origin: RequestOrigin,
    pub kind: QueryKind,
    pub result: Result<Intelligence, String>,
}
struct QueuedQuery {
    origin: RequestOrigin,
    query: LanguageQuery,
}
struct Saved {
    document: DocumentId,
    key: DocumentKey,
    text: TextBuffer,
}
struct Definition {
    origin: RequestOrigin,
    byte: usize,
}
#[derive(Resource, Default)]
struct Requests {
    definitions: VecDeque<Definition>,
    queries: VecDeque<QueuedQuery>,
    query_latest: HashMap<(Entity, QueryKind), RequestOrigin>,
    query_results: VecDeque<IntelligenceResult>,
    saved: VecDeque<Saved>,
    latest: HashMap<Entity, u64>,
    results: VecDeque<DefinitionResult>,
    next_ticket: u64,
}
#[derive(SystemSet, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum LspSystems {
    Update,
}
pub struct LspPlugin;
impl Plugin for LspPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Requests>()
            .add_systems(Update, update.in_set(LspSystems::Update));
    }
}
fn next_key(document: Entity) -> DocumentKey {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let generation = GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("LSP generations exhausted");
    DocumentKey {
        id: document.to_bits(),
        generation,
    }
}
pub fn server_status(world: &World, document: DocumentId) -> Option<ServerStatus> {
    let state = world.get::<DocumentLsp>(document.0)?;
    let server = world.get::<ManagedServer>(state.server)?;
    Some(if server.lifecycle == ServerLifecycle::Running {
        server.handle.status()
    } else {
        ServerStatus::Stopped
    })
}
pub fn document_server(world: &World, document: DocumentId) -> Option<Entity> {
    Some(world.get::<DocumentLsp>(document.0)?.server)
}
pub fn restart(world: &mut World, document: DocumentId) -> Result<(), String> {
    let server = world
        .get::<DocumentLsp>(document.0)
        .ok_or("No language server is attached")?
        .server;
    restart_instance(world, server)
}
/// Capture the source view, selection, document revision and ticket together.
pub fn request_definition(world: &mut World, panel: Entity) -> Result<(), String> {
    let byte = world
        .get::<WorkbenchEditor>(panel)
        .ok_or("No text editor is attached")?
        .view
        .selection()
        .head();
    request_definition_at(world, panel, byte)
}
pub fn request_definition_at(world: &mut World, panel: Entity, byte: usize) -> Result<(), String> {
    let document = world
        .get::<DocumentView>(panel)
        .ok_or("No document is attached")?
        .document();
    let info = inspect_document(world, document).map_err(|e| e.to_string())?;
    if info.has_drafts {
        return Err("Finish text composition before navigating".into());
    }
    let state = world
        .get::<DocumentLsp>(document.0)
        .ok_or("No language server is configured for this document")?;
    if server_status(world, document) != Some(ServerStatus::Ready) {
        return Err("Language server is not ready".into());
    }
    let key = state.key;
    let mut requests = world.resource_mut::<Requests>();
    if requests.definitions.len() >= 32 {
        return Err("Too many pending navigation requests".into());
    }
    requests.next_ticket = requests
        .next_ticket
        .checked_add(1)
        .expect("LSP request tickets exhausted");
    let ticket = requests.next_ticket;
    requests.latest.insert(panel, ticket);
    requests
        .definitions
        .retain(|r| r.origin.panel != panel.to_bits());
    requests.definitions.push_back(Definition {
        origin: RequestOrigin {
            document: key,
            revision: info.revision,
            panel: panel.to_bits(),
            ticket,
        },
        byte,
    });
    Ok(())
}
/// LanguageQuery state is independent of definition navigation: a hover cannot cancel F12.
pub fn language_activity(world: &World, panel: Entity) -> Option<Arc<ServerActivity>> {
    let document = world.get::<DocumentView>(panel)?.document();
    let binding = world.get::<DocumentLsp>(document.0)?;
    let server = world.get::<ManagedServer>(binding.server)?;
    (server.lifecycle == ServerLifecycle::Running && server.handle.status() == ServerStatus::Ready)
        .then(|| server.handle.activity())
}
pub fn request_query(
    world: &mut World,
    panel: Entity,
    query: LanguageQuery,
) -> Result<RequestOrigin, String> {
    let document = world
        .get::<DocumentView>(panel)
        .ok_or("No document is attached")?
        .document();
    let info = inspect_document(world, document).map_err(|e| e.to_string())?;
    if info.has_drafts {
        return Err("Finish text composition before requesting language information".into());
    }
    let state = world
        .get::<DocumentLsp>(document.0)
        .ok_or("No language server is configured")?;
    if server_status(world, document) != Some(ServerStatus::Ready) {
        return Err("Language server is not ready".into());
    }
    let key = state.key;
    let kind = query.kind();
    let mut requests = world.resource_mut::<Requests>();
    requests
        .queries
        .retain(|q| q.origin.panel != panel.to_bits() || q.query.kind() != kind);
    if requests.queries.len() >= 64 {
        return Err("Language query backlog is full".into());
    }
    requests.next_ticket = requests
        .next_ticket
        .checked_add(1)
        .expect("LSP tickets exhausted");
    let origin = RequestOrigin {
        document: key,
        revision: info.revision,
        panel: panel.to_bits(),
        ticket: requests.next_ticket,
    };
    requests.query_latest.insert((panel, kind), origin);
    requests.queries.push_back(QueuedQuery { origin, query });
    Ok(origin)
}
pub fn cancel_query(world: &mut World, panel: Entity, kind: QueryKind) {
    let mut requests = world.resource_mut::<Requests>();
    requests.query_latest.remove(&(panel, kind));
    requests
        .queries
        .retain(|q| q.origin.panel != panel.to_bits() || q.query.kind() != kind);
    if let Some(server) = world
        .get::<DocumentView>(panel)
        .and_then(|v| world.get::<DocumentLsp>(v.document().0))
        .and_then(|s| world.get::<ManagedServer>(s.server))
    {
        let _ = server.handle.cancel_query(panel.to_bits(), kind);
    }
}
pub fn query_is_current(world: &World, origin: &RequestOrigin, kind: QueryKind) -> bool {
    let Some(panel) = Entity::try_from_bits(origin.panel) else {
        return false;
    };
    binding_current(world, origin)
        && world
            .resource::<Requests>()
            .query_latest
            .get(&(panel, kind))
            == Some(origin)
}
pub fn take_intelligence(world: &mut World) -> VecDeque<IntelligenceResult> {
    std::mem::take(&mut world.resource_mut::<Requests>().query_results)
}
pub fn take_definitions(world: &mut World) -> VecDeque<DefinitionResult> {
    std::mem::take(&mut world.resource_mut::<Requests>().results)
}
/// Host calls this only for an acknowledged successful save, using the root
/// actually written. A failed admission is explicit and can be retried by host.
pub fn notify_saved(
    world: &mut World,
    document: DocumentId,
    text: TextBuffer,
) -> Result<(), String> {
    let Some(state) = world.get::<DocumentLsp>(document.0) else {
        return Ok(());
    };
    let key = state.key;
    let mut requests = world.resource_mut::<Requests>();
    if requests.saved.len() >= 128 {
        return Err("LSP save notification backlog is full".into());
    }
    requests.saved.push_back(Saved {
        document,
        key,
        text,
    });
    Ok(())
}
fn retire_binding(world: &mut World, document: Entity) {
    if let Some(state) = world.entity_mut(document).take::<DocumentLsp>() {
        if let Some(mut server) = world.get_mut::<ManagedServer>(state.server) {
            if server.lifecycle == ServerLifecycle::Running
                && server.handle.close(state.key).is_err()
            {
                server.lifecycle = ServerLifecycle::Restarting;
                server.task.cancel();
            }
            server.bindings.remove(&state.key);
        }
    }
}
fn origin_current(world: &World, origin: &RequestOrigin) -> bool {
    binding_current(world, origin)
        && Entity::try_from_bits(origin.panel).is_some_and(|panel| {
            world.resource::<Requests>().latest.get(&panel) == Some(&origin.ticket)
        })
}
fn binding_current(world: &World, origin: &RequestOrigin) -> bool {
    let Some(panel) = Entity::try_from_bits(origin.panel) else {
        return false;
    };
    let Some(view) = world.get::<DocumentView>(panel) else {
        return false;
    };
    let doc = view.document();
    world.get::<DocumentLsp>(doc.0).is_some_and(|s| {
        s.key == origin.document
            && world.get::<LspDocument>(doc.0) == Some(&s.source)
            && world
                .get::<ManagedServer>(s.server)
                .is_some_and(|server| server.lifecycle == ServerLifecycle::Running)
    }) && inspect_document(world, doc).is_ok_and(|i| i.revision == origin.revision && !i.has_drafts)
}
pub fn definition_is_current(world: &World, result: &DefinitionResult) -> bool {
    origin_current(world, &result.origin)
}
/// After an accepted result starts a file open, its preview may replace the
/// originating view. Hosts still gate completion by this ticket and their
/// captured document/load identities, rather than requiring the old binding.
pub fn definition_ticket_is_current(world: &World, origin: &RequestOrigin) -> bool {
    Entity::try_from_bits(origin.panel).is_some_and(|panel| {
        world.get_entity(panel).is_ok()
            && world.resource::<Requests>().latest.get(&panel) == Some(&origin.ticket)
    })
}
pub struct ServerObservation {
    pub entity: Entity,
    pub program: String,
    pub status: ServerStatus,
    pub root: PathBuf,
    pub languages: Vec<String>,
    pub retiring: bool,
    pub activity: Arc<ServerActivity>,
}
pub fn workspace_servers(world: &mut World, workspace: Entity) -> Vec<ServerObservation> {
    world
        .query::<(Entity, &ManagedServer, &InWorkspace)>()
        .iter(world)
        .filter(|(_, _, owner)| owner.0 == workspace)
        .map(|(entity, server, _)| ServerObservation {
            entity,
            program: server.launch.config.program.clone(),
            status: if server.lifecycle == ServerLifecycle::Stopped {
                ServerStatus::Stopped
            } else {
                server.handle.status()
            },
            root: server.launch.root.clone(),
            languages: world
                .get::<LspWorkspace>(workspace)
                .map(|p| {
                    p.servers
                        .iter()
                        .filter(|(_, config)| **config == server.launch.config)
                        .map(|(language, _)| language.clone())
                        .collect()
                })
                .unwrap_or_default(),
            retiring: server.retiring(),
            activity: server.handle.activity(),
        })
        .collect()
}
pub fn restart_instance(world: &mut World, entity: Entity) -> Result<(), String> {
    let mut managed = world
        .get_mut::<ManagedServer>(entity)
        .ok_or("Language server is unavailable")?;
    managed.lifecycle = ServerLifecycle::Restarting;
    managed.task.cancel();
    Ok(())
}

/// Stop one launch configuration in this workspace until explicitly restarted
/// or replaced by a different configuration. Shared-language bindings stop together.
pub fn stop_instance(world: &mut World, entity: Entity) -> Result<(), String> {
    let mut managed = world
        .get_mut::<ManagedServer>(entity)
        .ok_or("Language server is unavailable")?;
    managed.lifecycle = ServerLifecycle::Stopping;
    managed.task.cancel();
    Ok(())
}

fn update(world: &mut World) {
    let now = Instant::now();
    world.resource_scope(|world, mut requests: Mut<Requests>| {
        requests.query_latest.retain(|(panel, kind), origin| {
            if binding_current(world, origin) {
                return true;
            }
            if let Some(server) = Entity::try_from_bits(origin.document.id)
                .and_then(|doc| world.get::<DocumentLsp>(doc))
                .and_then(|s| world.get::<ManagedServer>(s.server))
            {
                let _ = server.handle.cancel_query(panel.to_bits(), *kind);
            }
            false
        });
        requests
            .latest
            .retain(|panel, _| world.get_entity(*panel).is_ok())
    });
    let servers: Vec<_> = world
        .query_filtered::<Entity, With<ManagedServer>>()
        .iter(world)
        .collect();
    for entity in servers {
        let keys: Vec<_> = world
            .get::<ManagedServer>(entity)
            .unwrap()
            .bindings
            .iter()
            .copied()
            .collect();
        for key in keys {
            let valid = Entity::try_from_bits(key.id)
                .and_then(|doc| world.get::<DocumentLsp>(doc))
                .is_some_and(|s| s.key == key && s.server == entity);
            if !valid {
                let mut server = world.get_mut::<ManagedServer>(entity).unwrap();
                if server.lifecycle == ServerLifecycle::Running && server.handle.close(key).is_err()
                {
                    server.lifecycle = ServerLifecycle::Restarting;
                    server.task.cancel();
                }
                server.bindings.remove(&key);
            }
        }
        let workspace = world.get::<InWorkspace>(entity).map(|w| w.0);
        let server = world.get::<ManagedServer>(entity).unwrap();
        let wanted = workspace
            .and_then(|w| world.get::<LspWorkspace>(w))
            .is_some_and(|p| {
                p.root == server.launch.root
                    && p.servers.values().any(|c| c == &server.launch.config)
            });
        if !wanted {
            let mut s = world.get_mut::<ManagedServer>(entity).unwrap();
            s.lifecycle = ServerLifecycle::Restarting;
            s.task.cancel();
        }
        let server = world.get::<ManagedServer>(entity).unwrap();
        if server.retiring()
            && !matches!(
                server.task.status(),
                WorkStatus::Running | WorkStatus::Cancelling
            )
        {
            if server.lifecycle == ServerLifecycle::Restarting {
                world.despawn(entity);
            } else {
                world.get_mut::<ManagedServer>(entity).unwrap().lifecycle =
                    ServerLifecycle::Stopped;
            }
            continue;
        }
        if server.lifecycle != ServerLifecycle::Running {
            continue;
        }
        // Time and count cap main-world acceptance. Parsing/indexing is already
        // complete; no single event contains an unbounded JSON traversal.
        for _ in 0..32 {
            if now.elapsed() > Duration::from_millis(2) {
                break;
            }
            let event = world
                .get::<ManagedServer>(entity)
                .unwrap()
                .handle
                .try_recv();
            let Ok(event) = event else {
                break;
            };
            match event {
                Event::Diagnostics { document, report } => {
                    if let Some(doc) = Entity::try_from_bits(document.id) {
                        if let Some(mut state) = world.get_mut::<DocumentLsp>(doc) {
                            if state.key == document && state.server == entity {
                                state.diagnostics = Some(report);
                                state.error = None;
                            }
                        }
                    }
                }
                Event::DocumentError { document, message } => {
                    if let Some(doc) = Entity::try_from_bits(document.id) {
                        if let Some(mut state) = world.get_mut::<DocumentLsp>(doc) {
                            if state.key == document && state.server == entity {
                                state.error = Some(message);
                                state.diagnostics = None;
                            }
                        }
                    }
                }
                Event::Intelligence {
                    origin,
                    kind,
                    result,
                } => {
                    if query_is_current(world, &origin, kind) {
                        let mut requests = world.resource_mut::<Requests>();
                        if requests.query_results.len() < 64 {
                            requests.query_results.push_back(IntelligenceResult {
                                origin,
                                kind,
                                result,
                            });
                        }
                    }
                }
                Event::Definition { origin, result } => {
                    if origin_current(world, &origin) {
                        match result {
                            Ok(locations) => {
                                let mut q = world.resource_mut::<Requests>();
                                if q.results.len() < 32 {
                                    q.results.push_back(DefinitionResult { origin, locations });
                                } else {
                                    drop(q);
                                    if let Some(doc) = Entity::try_from_bits(origin.document.id) {
                                        world.get_mut::<DocumentLsp>(doc).unwrap().error =
                                            Some("Definition result queue is full".into());
                                    }
                                }
                            }
                            Err(error) => {
                                if let Some(doc) = Entity::try_from_bits(origin.document.id) {
                                    world.get_mut::<DocumentLsp>(doc).unwrap().error = Some(error);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let viewers: HashSet<_> = world
        .query_filtered::<&DocumentView, With<WorkbenchEditor>>()
        .iter(world)
        .map(DocumentView::document)
        .collect();
    let attached: Vec<_> = world
        .query_filtered::<Entity, With<DocumentLsp>>()
        .iter(world)
        .collect();
    for doc in attached {
        let valid = (|| {
            let state = world.get::<DocumentLsp>(doc)?;
            let source = world.get::<LspDocument>(doc)?;
            let info = inspect_document(world, DocumentId(doc)).ok()?;
            let project = world.get::<LspWorkspace>(info.workspace)?;
            let server = world.get::<ManagedServer>(state.server)?;
            (source == &state.source
                && project.servers.get(&source.language) == Some(&server.launch.config)
                && project.root == server.launch.root
                && server.lifecycle == ServerLifecycle::Running
                && (viewers.contains(&DocumentId(doc)) || info.dirty))
                .then_some(())
        })()
        .is_some();
        if !valid {
            retire_binding(world, doc);
        }
    }
    let docs: Vec<_> = world
        .query_filtered::<Entity, With<LspDocument>>()
        .iter(world)
        .collect();
    for doc in docs {
        let Ok(handle) = cast_document::<TextBuffer>(world, DocumentId(doc)) else {
            continue;
        };
        let Ok(info) = inspect_document(world, DocumentId(doc)) else {
            continue;
        };
        if !viewers.contains(&DocumentId(doc)) && !info.dirty {
            continue;
        }
        if world
            .get::<WorkspaceWork>(info.workspace)
            .is_some_and(WorkspaceWork::is_closing)
        {
            continue;
        }
        if read_document(world, handle).unwrap().len() > MAX_SOURCE_BYTES {
            retire_binding(world, doc);
            continue;
        }
        let source = world.get::<LspDocument>(doc).unwrap().clone();
        if world.get::<DocumentLsp>(doc).is_none() {
            let Some(project) = world.get::<LspWorkspace>(info.workspace).cloned() else {
                continue;
            };
            let Some(config) = project.servers.get(&source.language).cloned() else {
                continue;
            };
            let existing = world
                .query::<(Entity, &ManagedServer, &InWorkspace)>()
                .iter(world)
                .find_map(|(entity, s, w)| {
                    (w.0 == info.workspace
                        && s.launch.config == config
                        && s.launch.root == project.root)
                        .then_some((entity, s.lifecycle != ServerLifecycle::Running))
                });
            let server = match existing {
                Some((_, true)) => continue,
                Some((entity, false)) => entity,
                None => {
                    let launch = Launch {
                        config,
                        root: project.root,
                    };
                    let Ok((handle, worker)) = ServerHandle::prepare(launch.clone()) else {
                        continue;
                    };
                    let entity = world.spawn(InWorkspace(info.workspace)).id();
                    let task = Workbench::for_entity(world, info.workspace).and_then(|mut w| {
                        w.work()
                            .spawn_task(Some(entity), "Language server", move |cancel| {
                                worker.run(|| cancel.is_cancelled())
                            })
                    });
                    let Ok(task) = task else {
                        world.despawn(entity);
                        continue;
                    };
                    world.entity_mut(entity).insert(ManagedServer {
                        launch,
                        handle,
                        task,
                        lifecycle: ServerLifecycle::Running,
                        bindings: HashSet::new(),
                    });
                    entity
                }
            };
            world.entity_mut(doc).insert(DocumentLsp {
                source: source.clone(),
                key: next_key(doc),
                server,
                revision: None,
                diagnostics: None,
                error: None,
            });
        }
        let state = world.get::<DocumentLsp>(doc).unwrap();
        if state.revision == Some(info.revision) {
            continue;
        }
        let server = state.server;
        let key = state.key;
        let base = state.revision;
        let changes = base.and_then(|base| {
            document_changes_since(world, handle, base)
                .ok()
                .flatten()
                .and_then(|commits| {
                    let edits: Option<Vec<_>> = commits.into_iter().map(|c| c.changes).collect();
                    edits.map(|edits| (base, edits.into_iter().flatten().collect::<Vec<_>>()))
                })
        });
        // Very large edit batches are cheaper as one worker-side refresh and
        // must not make an otherwise admissible root exceed the queue budget.
        let changes = changes.filter(|(_, edits)| {
            edits.len() <= 4096
                && edits.iter().map(|e| e.replacement().len()).sum::<usize>() <= MAX_SOURCE_BYTES
        });
        let text = read_document(world, handle).unwrap().clone();
        if text.len() > MAX_SOURCE_BYTES {
            let mut state = world.get_mut::<DocumentLsp>(doc).unwrap();
            state.error = Some("LSP is limited to 8 MiB; plain editing remains available".into());
            state.diagnostics = None;
            state.revision = Some(info.revision);
            continue;
        }
        let result = world
            .get::<ManagedServer>(server)
            .unwrap()
            .handle
            .update(DocumentUpdate {
                key,
                uri: source.uri,
                language: source.language,
                revision: info.revision,
                text,
                changes,
            });
        if result.is_ok() {
            world
                .get_mut::<ManagedServer>(server)
                .unwrap()
                .bindings
                .insert(key);
        }
        let mut state = world.get_mut::<DocumentLsp>(doc).unwrap();
        match result {
            Ok(()) => {
                state.revision = Some(info.revision);
                state.error = None;
            }
            Err(error) => state.error = Some(error),
        }
    }
    let (mut definitions, mut saved) = {
        let mut r = world.resource_mut::<Requests>();
        (
            std::mem::take(&mut r.definitions),
            std::mem::take(&mut r.saved),
        )
    };
    while let Some(request) = definitions.pop_front() {
        if !origin_current(world, &request.origin) {
            continue;
        }
        let doc = Entity::try_from_bits(request.origin.document.id).unwrap();
        let state = world.get::<DocumentLsp>(doc).unwrap();
        if state.revision != Some(request.origin.revision) {
            world
                .resource_mut::<Requests>()
                .definitions
                .push_back(request);
            continue;
        }
        let result = world
            .get::<ManagedServer>(state.server)
            .unwrap()
            .handle
            .definition(request.origin, request.byte);
        if let Err(error) = result {
            world.get_mut::<DocumentLsp>(doc).unwrap().error = Some(error);
        }
    }
    let mut queries = std::mem::take(&mut world.resource_mut::<Requests>().queries);
    while let Some(request) = queries.pop_front() {
        let kind = request.query.kind();
        if !query_is_current(world, &request.origin, kind) {
            continue;
        }
        let doc = Entity::try_from_bits(request.origin.document.id).unwrap();
        let state = world.get::<DocumentLsp>(doc).unwrap();
        if state.revision != Some(request.origin.revision) {
            world.resource_mut::<Requests>().queries.push_back(request);
            continue;
        }
        if let Err(error) = world
            .get::<ManagedServer>(state.server)
            .unwrap()
            .handle
            .query(request.origin, request.query)
        {
            world
                .resource_mut::<Requests>()
                .query_results
                .push_back(IntelligenceResult {
                    origin: request.origin,
                    kind,
                    result: Err(error),
                });
        }
    }
    while let Some(saved) = saved.pop_front() {
        let Some(state) = world
            .get::<DocumentLsp>(saved.document.0)
            .filter(|s| s.key == saved.key)
        else {
            continue;
        };
        let server = world.get::<ManagedServer>(state.server).unwrap();
        if matches!(
            server.handle.status(),
            ServerStatus::Failed(_) | ServerStatus::Stopped
        ) {
            continue;
        }
        if let Err(error) = server.handle.saved(saved.key, saved.text.clone()) {
            world
                .get_mut::<DocumentLsp>(saved.document.0)
                .unwrap()
                .error = Some(error);
            world.resource_mut::<Requests>().saved.push_back(saved);
        }
    }
    let panels: Vec<_> = world
        .query_filtered::<(Entity, &DocumentView), With<WorkbenchEditor>>()
        .iter(world)
        .map(|(p, v)| (p, v.document()))
        .collect();
    for (panel, doc) in panels {
        let report = world
            .get::<DocumentLsp>(doc.0)
            .filter(|s| {
                world
                    .get::<ManagedServer>(s.server)
                    .is_some_and(|server| server.handle.status() == ServerStatus::Ready)
            })
            .and_then(|s| s.diagnostics.clone());
        world
            .get_mut::<WorkbenchEditor>(panel)
            .unwrap()
            .view
            .set_diagnostics(report);
    }
}
