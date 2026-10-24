//! One application-owned reflected project record, with bounded asynchronous I/O.
use crate::persistence::ReflectProjectPersist;
use bedtime_workbench2::persistence::Snapshot;
use bedtime_workbench2::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    thread::JoinHandle,
    time::{Duration, Instant},
};
const VERSION: u32 = 2;
const MAX_BYTES: usize = crate::storage::MAX_BYTES;
const DEBOUNCE: Duration = Duration::from_millis(250);
#[derive(Resource, Reflect, Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Resource, Serialize, Deserialize, ProjectPersist)]
pub(crate) struct ProjectTitle(pub String);
pub struct SessionPlugin;
impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        crate::persistence::register(app);
        app.init_resource::<Sessions>()
            .init_resource::<ProjectTitle>()
            .init_resource::<crate::settings::ProjectPreferences>()
            .init_resource::<crate::keybindings::ProjectBindings>()
            .add_systems(PostUpdate, capture_sessions);
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Session {
    version: u32,
    root: Option<PathBuf>,
    graph: Snapshot,
    local: Snapshot,
}
#[derive(Resource)]
struct InitialSession(Session);
pub fn remember_initial_state(world: &mut World) -> Result<(), String> {
    if !world.contains_resource::<InitialSession>() {
        let initial = capture(world)?;
        world.insert_resource(InitialSession(initial));
    }
    Ok(())
}
pub fn seed_initial_layout(world: &mut World) {
    if let Ok(current) = capture(world) {
        if let Some(mut initial) = world.get_resource_mut::<InitialSession>() {
            initial.0.graph = current.graph.clone();
        }
        if let Some(mut sessions) = world.get_resource_mut::<Sessions>()
            && let Some(file) = sessions.0.as_mut()
            && file.policy_ready
            && file.pending_restore.is_none()
            && file.desired == file.saved
        {
            file.saved = current.clone();
            file.desired = current;
        }
    }
}
pub fn initialize_folder_name(world: &mut World, name: String) {
    world.init_resource::<ProjectTitle>();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let initial = world
        .get_resource::<InitialSession>()
        .and_then(|i| crate::persistence::prepare_local(&i.0.local, &registry).ok())
        .and_then(|p| p.resource::<ProjectTitle>());
    if initial
        .as_ref()
        .is_none_or(|previous| world.resource::<ProjectTitle>() == previous)
    {
        world.resource_mut::<ProjectTitle>().0 = name.clone();
    }
    if let Some(initial) = world.get_resource::<InitialSession>()
        && let Ok(mut prepared) = crate::persistence::prepare_local(&initial.0.local, &registry)
    {
        let _ = prepared.replace_resource(ProjectTitle(name));
        if let Ok(local) = prepared.serialize(&registry) {
            world.resource_mut::<InitialSession>().0.local = local;
        }
    }
}
pub(crate) fn validate(session: &Session) -> Result<(), String> {
    if session.version != VERSION {
        return Err("Unsupported project session version; original file preserved".into());
    }
    if session
        .root
        .as_ref()
        .is_some_and(|r| !r.is_absolute() || r.as_os_str().len() > 16384)
    {
        return Err("Invalid project root".into());
    }
    Ok(())
}
pub(crate) fn capture(world: &mut World) -> Result<Session, String> {
    Ok(Session {
        version: VERSION,
        root: world
            .get_resource::<crate::files::ProjectRoot>()
            .map(|r| r.0.clone()),
        graph: crate::persistence::capture_graph(world)?,
        local: crate::persistence::capture_local(world)?,
    })
}
fn merge_loaded(
    world: &World,
    baseline: &Session,
    live: &Session,
    disk: &Session,
) -> Result<Snapshot, String> {
    let registry = world.resource::<AppTypeRegistry>();
    let baseline = crate::persistence::prepare_local(&baseline.local, registry)?;
    let live = crate::persistence::prepare_local(&live.local, registry)?;
    let mut disk = crate::persistence::prepare_local(&disk.local, registry)?;
    if baseline.resource::<ProjectTitle>() != live.resource::<ProjectTitle>()
        || baseline.resource::<crate::git::PersistedGit>()
            != live.resource::<crate::git::PersistedGit>()
    {
        return Err(
            "Project state changed while its session was loading; existing work preserved".into(),
        );
    }
    let a = baseline
        .resource::<crate::settings::ProjectPreferences>()
        .unwrap();
    let b = live
        .resource::<crate::settings::ProjectPreferences>()
        .unwrap();
    let c = disk
        .resource::<crate::settings::ProjectPreferences>()
        .unwrap();
    disk.replace_resource(crate::settings::ProjectPreferences(
        crate::settings::SettingsOverrides::merge_loaded(&a.0, &b.0, &c.0),
    ))?;
    let a = baseline
        .resource::<crate::files::PersistedVisibility>()
        .unwrap();
    let b = live
        .resource::<crate::files::PersistedVisibility>()
        .unwrap();
    let mut c = disk
        .resource::<crate::files::PersistedVisibility>()
        .unwrap();
    let roots: std::collections::BTreeSet<_> =
        a.0.roots.keys().chain(b.0.roots.keys()).cloned().collect();
    for root in roots {
        if a.0.roots.get(&root) != b.0.roots.get(&root) {
            if let Some(value) = b.0.roots.get(&root) {
                c.0.roots.insert(root, value.clone());
            } else {
                c.0.roots.remove(&root);
            }
        }
    }
    disk.replace_resource(c)?;
    let a = baseline
        .resource::<crate::keybindings::ProjectBindings>()
        .unwrap();
    let b = live
        .resource::<crate::keybindings::ProjectBindings>()
        .unwrap();
    let c = disk
        .resource::<crate::keybindings::ProjectBindings>()
        .unwrap();
    disk.replace_resource(crate::keybindings::ProjectBindings(
        crate::keybindings::merge(&a.0, &b.0, &c.0),
    ))?;
    disk.serialize(registry)
}
fn restore(world: &mut World, session: &Session) -> Result<(), String> {
    validate(session)?;
    let registry = world.resource::<AppTypeRegistry>();
    // All partitions and application descriptors pass validation before mutation.
    let graph = crate::persistence::prepare_graph(&session.graph, registry)?;
    let local = crate::persistence::prepare_local(&session.local, registry)?;
    if !document_ids(world).is_empty() {
        return Err(
            "Documents opened while session data was loading; existing work preserved".into(),
        );
    }
    if world.resource::<ManagedWork>().list().iter().any(|job| {
        job.owner.is_some_and(|owner| {
            world.get::<crate::tasks::TasksPanel>(owner).is_some()
                || world
                    .get::<bedtime_terminal2::TerminalPanel>(owner)
                    .is_some()
        })
    }) || crate::git::has_pending_operation(world)
        || bedtime_filetree2::filetree_operation_count(world) > 0
    {
        return Err(
            "Live work started while session data was loading; existing work preserved".into(),
        );
    }
    for layout in layouts(world).to_vec() {
        world.despawn(layout);
    }
    Workbench::restore_world(world, local)?;
    crate::persistence::activate_local(world);
    crate::persistence::restore_graph(world, graph)
}
enum Job {
    Read(JoinHandle<Result<Option<Session>, String>>),
    Write {
        recipe: Session,
        handle: JoinHandle<Result<(), String>>,
    },
}

struct SessionFile {
    path: PathBuf,
    desired: Session,
    saved: Session,
    loaded: bool,
    policy_ready: bool,
    retrying: bool,
    pending_restore: Option<Session>,
    job: Option<Job>,
    changed_at: Instant,
    error: Option<String>,
    reported: bool,
}

#[derive(Resource, Default)]
struct Sessions(Option<SessionFile>, Option<String>);

fn decode(bytes: &[u8]) -> Result<Session, String> {
    let session: Session = ron::de::from_bytes(bytes)
        .map_err(|e| format!("Unsupported or invalid project session; original preserved: {e}"))?;
    validate(&session)?;
    Ok(session)
}
fn read_source(
    path: &std::path::Path,
    registry: &AppTypeRegistry,
) -> Result<Option<Session>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err("Workspace session is not a regular file".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Workspace session is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Project session exceeds 4 MiB".into());
    }
    let recipe = decode(&bytes)?;
    crate::persistence::prepare_graph(&recipe.graph, registry)?;
    crate::persistence::prepare_local(&recipe.local, registry)?;
    Ok(Some(recipe))
}

fn read_job(path: PathBuf, registry: AppTypeRegistry) -> Job {
    Job::Read(std::thread::spawn(move || read_source(&path, &registry)))
}

pub fn admit(world: &mut World) {
    if !crate::close::shutting_down(world) {
        let _ = tick(world, false);
    }
}

fn capture_sessions(world: &mut World) {
    if !crate::close::shutting_down(world) {
        let _ = tick(world, false);
    }
}

fn tick(world: &mut World, flush: bool) -> Result<bool, String> {
    if !world.contains_resource::<Sessions>() {
        return Ok(true);
    }
    world.resource_scope(|world, mut sessions: Mut<Sessions>| {
        if world.get_resource::<crate::storage::ConfigDirectory>().is_none() { return Ok(true); }
        if !crate::storage::global_ready(world) { return Ok(false); }
        if world.get_resource::<crate::files::ProjectFolder>().is_none() { return Ok(true); }
        let Some(root) = world.get_resource::<crate::files::ProjectRoot>().map(|r| r.0.clone()) else { return Ok(false); };
        let desired = match capture(world) {
            Ok(desired) => { sessions.1 = None; desired },
            Err(error) => {
                if sessions.1.as_ref()!=Some(&error) {request_error_dialog(world,ErrorDialog {title:"Project session".into(),message:"Session data could not be captured. Previous storage and live work are preserved.".into(),details:Some(error.clone())});}
                sessions.1=Some(error.clone());return Err(error);
            }
        };
        let initial = world.get_resource::<InitialSession>().map(|initial| { let mut recipe = initial.0.clone(); recipe.root = desired.root.clone(); recipe });
        world.remove_resource::<InitialSession>();
        let file = sessions.0.get_or_insert_with(|| {
            let path = crate::storage::workspace_path(world, &root).unwrap();
            SessionFile { job: Some(read_job(path.clone(), world.resource::<AppTypeRegistry>().clone())), path, saved: initial.unwrap_or_else(|| desired.clone()), desired: desired.clone(), loaded: false, policy_ready: false,
                retrying: false, pending_restore: None, changed_at: Instant::now(), error: None, reported: false }
        });
            if file.path != crate::storage::workspace_path(world, &root).unwrap() {
                file.error = Some("Workspace root changed while session I/O was pending; reopen the folder instead".into());
            }
            if file.desired != desired { file.desired = desired; file.changed_at = Instant::now(); }
        let mut drained = true;
        let mut errors = Vec::new();
        {
            let finished = match &file.job {
                Some(Job::Read(handle)) => handle.is_finished(),
                Some(Job::Write { handle, .. }) => handle.is_finished(),
                None => false,
            };
            if finished {
                match file.job.take().unwrap() {
                    Job::Read(handle) => {
                        let result = handle.join().unwrap_or_else(|_| Err("Workspace session reader panicked".into()));
                        file.loaded = true;
                        match result {
                            Ok(Some(recipe)) if recipe.root != file.desired.root => {
                                file.error = Some("Workspace file belongs to a different folder; existing data preserved".into());
                            }
                            Ok(Some(recipe)) => {
                                if file.retrying {
                                    // Explicit retry after repair validates disk first,
                                    // then saves live work rather than replacing it.
                                    file.saved = recipe;
                                    file.policy_ready = true;
                                } else {
                                    file.pending_restore = Some(recipe);
                                }
                            }
                            Ok(None) => { file.policy_ready = true; }
                            Err(error) => file.error = Some(error),
                        }
                        file.retrying = false;
                    }
                    Job::Write { recipe, handle } => match handle.join().unwrap_or_else(|_| Err("Workspace session writer panicked".into())) {
                        Ok(()) => { file.saved = recipe; file.error = None; file.reported = false; }
                        Err(error) => file.error = Some(error),
                    },
                }
            }
            if let Some(mut recipe) = file.pending_restore.take() {
                if !crate::storage::global_ready(world) || !crate::files::root_ready(world) {
                    file.pending_restore = Some(recipe);

                } else {
                    let _disk = recipe.clone();
                    let merged=merge_loaded(world,&file.saved,&file.desired,&recipe);
                    let composition_changed = file.desired.graph != file.saved.graph;
                    if let Ok(local)=&merged { recipe.local=local.clone(); }
                    if let Err(error)=merged { file.error=Some(error); }
                    if composition_changed || file.error.is_some() {
                        file.error = Some("Workspace changed while its session was loading. Existing work and the disk session were preserved; reopen the folder to restore, or explicitly retry after review.".into());
                    } else {
                        match restore(world, &recipe).and_then(|()| capture(world)) {
                            Ok(restored) => { file.saved = restored.clone(); file.desired = restored; file.policy_ready = true; }
                            Err(error) => file.error = Some(error),
                        }
                    }
                }
            }
            if let Some(error) = &file.error {
                let details = format!("{}: {error}", file.path.display());
                // A rejected startup file with no local mutation has nothing to
                // flush. Report it, but do not turn clean quit into a save prompt.
                if file.desired != file.saved { errors.push(details.clone()); }
                if !file.reported {
                    request_error_dialog(world, ErrorDialog { title: "Workspace session".into(), message: "The workspace session couldn't be read or saved. Your open work is preserved. Repair the file/access problem, then retry; an invalid session is never overwritten automatically.".into(), details: Some(details) });
                    file.reported = true;
                }
                if file.job.is_some() { drained = false; }
                return if errors.is_empty() { Ok(drained) } else { Err(errors.join("\n")) };
            }
            if file.pending_restore.is_some() {
                drained = false;
                return if errors.is_empty() { Ok(drained) } else { Err(errors.join("\n")) };
            }
            if file.loaded && file.job.is_none() && file.desired != file.saved && (flush || file.changed_at.elapsed() >= DEBOUNCE) {
                let recipe = file.desired.clone();
                let bytes = match crate::storage::encode(&recipe) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        // Capture runs from an ordinary system which cannot
                        // return errors to its caller. Keep diagnostics owned by
                        // the file so the next tick reports it and quit blocks.
                        file.error = Some(format!("Couldn't serialize workspace session: {error}. Previous file preserved."));
                        drained = false;
                        return if errors.is_empty() { Ok(drained) } else { Err(errors.join("\n")) };
                    }
                };
                if bytes.len() > MAX_BYTES { file.error = Some("Workspace session exceeds 4 MiB; previous file preserved".into()); return Ok(false); }
                let path = file.path.clone();
                let expected_root = recipe.root.clone();
                let registry=world.resource::<AppTypeRegistry>().clone();
                file.job = Some(Job::Write { recipe, handle: std::thread::spawn(move || {
                    // Revalidate before replacement too: an externally introduced
                    // malformed/newer-version session is not overwrite consent.
                    if read_source(&path, &registry)?.is_some_and(|existing| existing.root != expected_root) {
                        return Err("Workspace file belongs to a different folder; previous data preserved".into());
                    }
                    crate::storage::atomic_write(&path, &bytes)
                }) });
            }
            if file.job.is_some() || file.desired != file.saved { drained = false; }
        }
        if errors.is_empty() { Ok(drained) } else { Err(errors.join("\n")) }
    })
}

pub fn preferences_ready(world: &World) -> bool {
    if world
        .get_resource::<crate::files::ProjectFolder>()
        .is_none()
        || !world.contains_resource::<crate::storage::ConfigDirectory>()
    {
        return true;
    }
    world.get_resource::<Sessions>().is_some_and(|sessions| {
        sessions
            .0
            .as_ref()
            .is_some_and(|file| file.policy_ready && file.pending_restore.is_none())
    })
}

/// Loading readiness for application UI/automation, independent of whether a
/// buffered mutation has reached disk. Temporary workspaces never load sessions.
#[cfg(feature = "automation")]
pub fn ready(world: &World) -> bool {
    if world
        .get_resource::<crate::files::ProjectFolder>()
        .is_none()
    {
        return true;
    }
    world.get_resource::<Sessions>().is_none_or(|sessions| {
        sessions
            .0
            .as_ref()
            .is_some_and(|file| file.loaded && file.pending_restore.is_none())
    })
}

/// False means pending; errors preserve the old file and block quit only when
/// local mutations remain unsaved. Rejected startup files alone don't block quit.
pub fn flush(world: &mut World) -> Result<bool, String> {
    tick(world, true)
}

/// After repair, validate disk again before retrying the current live recipe.
/// Malformed/unsupported sessions are never overwritten by a retry.
pub fn retry(world: &mut World) -> Result<(), String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut sessions = world
        .get_resource_mut::<Sessions>()
        .ok_or("Session storage is not installed")?;
    let file = sessions
        .0
        .as_mut()
        .ok_or("Workspace has no folder session")?;
    if file.job.is_some() {
        return Err("Workspace session I/O is still pending".into());
    }
    if file.error.is_none() {
        return Ok(());
    }
    file.loaded = false;
    file.retrying = true;
    file.error = None;
    file.reported = false;
    file.job = Some(read_job(file.path.clone(), registry));
    Ok(())
}

/// Application settings expose the same retry used by quit, without requiring
/// a quit attempt just to repair workspace storage.
pub fn draw_status(ui: &mut bedtime_ui2::egui::Ui, world: &mut World) {
    let Some(file) = world
        .get_resource::<Sessions>()
        .and_then(|sessions| sessions.0.as_ref())
    else {
        return;
    };
    let error = file.error.clone();
    if let Some(error) = error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Workspace storage: {error}"),
        );
        if ui.button("Retry workspace storage").clicked() {
            if let Err(error) = retry(world) {
                request_error_dialog(
                    world,
                    ErrorDialog {
                        title: "Workspace storage".into(),
                        message: "Workspace storage couldn't be retried yet.".into(),
                        details: Some(error),
                    },
                );
            }
        }
    }
}

/// Retry all failed sessions, including retired workspaces retained by storage.
/// Used by the quit error UI after an explicit user retry.
pub fn retry_failed(world: &mut World) {
    let _ = retry(world);
}

#[cfg(feature = "automation")]
pub fn inspect_storage(world: &World) -> Vec<serde_json::Value> {
    world.get_resource::<Sessions>().map(|s|s.0.iter().map(|f|serde_json::json!({"root":f.desired.root,"path":f.path,"loaded":f.loaded,"pending":f.job.is_some()||f.pending_restore.is_some()||f.desired!=f.saved,"error":f.error.as_ref().or(s.1.as_ref())})).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    fn fixture() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed2-reflect-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    fn host(root: &std::path::Path, persistent: bool) -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, SessionPlugin));
        app.insert_resource(crate::storage::ConfigDirectory(root.join("config")))
            .insert_resource(crate::files::ProjectRoot(root.into()));
        if persistent {
            app.insert_resource(crate::files::ProjectFolder);
        }
        remember_initial_state(app.world_mut()).unwrap();
        app
    }
    fn settle(app: &mut App) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            if flush(app.world_mut())? {
                return Ok(());
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn settings_panel(world: &mut World) -> Entity {
        let layout = Workbench::new(world)
            .create_layout("Settings layout")
            .unwrap();
        Workbench::new(world).activate_layout(layout).unwrap();
        let area = Workbench::new(world).destination_area(layout).unwrap();
        world
            .spawn((crate::settings::SettingsPanel, InArea(area)))
            .id()
    }
    #[test]
    fn startup_is_memory_only_without_an_explicit_project_and_pristine_projects_do_not_write() {
        let root = fixture();
        let mut memory = host(&root, false);
        settings_panel(memory.world_mut());
        settle(&mut memory).unwrap();
        assert!(!root.join("config").exists());
        let mut project = host(&root, true);
        settle(&mut project).unwrap();
        assert!(
            !crate::storage::workspace_path(project.world(), &root)
                .unwrap()
                .exists()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reflected_tools_resources_layout_order_and_selection_roundtrip_without_a_startup_write() {
        let root = fixture();
        let mut app = host(&root, true);
        settle(&mut app).unwrap();
        let panel = settings_panel(app.world_mut());
        app.world_mut()
            .get_mut::<PanelInstance>(panel)
            .unwrap()
            .preview = true;
        let layout = Workbench::new(app.world_mut())
            .create_layout("Second")
            .unwrap();
        Workbench::new(app.world_mut())
            .activate_layout(layout)
            .unwrap();
        app.world_mut()
            .resource_mut::<crate::settings::ProjectPreferences>()
            .0
            .line_numbers = Some(false);
        let bindings = crate::keybindings::ProjectBindings(vec![crate::keybindings::Declaration {
            chord: KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS),
            command: None,
            repeat: false,
        }]);
        app.insert_resource(bindings.clone());
        crate::git::restore_draft(app.world_mut(), "saved draft".into());
        settle(&mut app).unwrap();
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let before = std::fs::read(&path).unwrap();
        let stored = read_source(&path, app.world().resource::<AppTypeRegistry>())
            .unwrap()
            .unwrap();
        assert_eq!(stored.version, 2);
        assert!(!stored.graph.data.contains("ManagedWork"));
        assert!(stored.graph.data.contains("PersistedSettings"));
        drop(app);
        let mut restored = host(&root, true);
        settle(&mut restored).unwrap();
        let names: Vec<_> = layouts(restored.world())
            .iter()
            .map(|&e| restored.world().get::<Layout>(e).unwrap().name.as_str())
            .collect();
        assert_eq!(names, ["Settings layout", "Second"]);
        assert_eq!(
            restored
                .world()
                .get::<Layout>(active_layout(restored.world()).unwrap())
                .unwrap()
                .name,
            "Second"
        );
        assert_eq!(crate::git::commit_draft(restored.world()), "saved draft");
        assert_eq!(
            restored
                .world()
                .resource::<crate::keybindings::ProjectBindings>(),
            &bindings
        );
        assert_eq!(
            restored
                .world()
                .resource::<crate::settings::ProjectPreferences>()
                .0
                .line_numbers,
            Some(false)
        );
        assert_eq!(
            restored
                .world_mut()
                .query::<&crate::settings::SettingsPanel>()
                .iter(restored.world())
                .count(),
            1
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rejected_old_or_malformed_project_data_is_preserved_and_new_mutations_block_flush() {
        let root = fixture();
        let mut app = host(&root, true);
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = b"(version:1,root:None,layouts:[])";
        std::fs::write(&path, old).unwrap();
        // Clean rejected startup storage alone has nothing new to flush.
        settle(&mut app).unwrap();
        assert!(!preferences_ready(app.world()));
        settings_panel(app.world_mut());
        assert!(settle(&mut app).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), old);
        assert_eq!(
            app.world_mut()
                .query::<&crate::settings::SettingsPanel>()
                .iter(app.world())
                .count(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unknown_live_panels_cannot_silently_replace_a_good_snapshot() {
        let root = fixture();
        let mut app = host(&root, true);
        settle(&mut app).unwrap();
        let panel = settings_panel(app.world_mut());
        settle(&mut app).unwrap();
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let before = std::fs::read(&path).unwrap();
        let area = app.world().get::<InArea>(panel).unwrap().0;
        app.world_mut()
            .spawn((PanelInstance::new("unpersisted.plugin"), InArea(area)));
        assert!(flush(app.world_mut()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restore_rejects_live_documents_and_invalid_graph_before_retiring_anything() {
        let root = fixture();
        let mut source = host(&root, false);
        settings_panel(source.world_mut());
        let stored = capture(source.world_mut()).unwrap();
        let mut target = host(&root, false);
        let live = settings_panel(target.world_mut());
        let document = open_document(
            target.world_mut(),
            "unsaved".to_owned(),
            DocumentOptions::unsaved("Draft"),
        )
        .unwrap();
        assert!(restore(target.world_mut(), &stored).is_err());
        assert!(target.world().get::<PanelInstance>(live).is_some());
        assert_eq!(read_document(target.world(), document).unwrap(), "unsaved");
        let mut invalid = stored;
        invalid.graph.version += 1;
        assert!(restore(target.world_mut(), &invalid).is_err());
        assert!(target.world().get::<PanelInstance>(live).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resource_partitions_and_numbered_slots_reject_entities_before_live_retirement() {
        let root = fixture();
        let mut app = host(&root, false);
        let panel = settings_panel(app.world_mut());
        let mut session = capture(app.world_mut()).unwrap();
        app.world_mut().spawn(crate::layouts::LayoutSlot::Two);
        let graph = crate::persistence::capture_graph(app.world_mut()).unwrap();
        session.local = graph.clone();
        assert!(restore(app.world_mut(), &session).is_err());
        assert!(app.world().get::<PanelInstance>(panel).is_some());

        assert!(
            crate::persistence::prepare_graph(&graph, app.world().resource::<AppTypeRegistry>())
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn late_reads_merge_individual_project_values_and_keep_newer_visibility_roots() {
        let root = fixture();
        let mut app = host(&root, false);
        let baseline = capture(app.world_mut()).unwrap();
        let chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS);
        app.world_mut()
            .insert_resource(crate::keybindings::ProjectBindings(vec![
                crate::keybindings::Declaration {
                    chord,
                    command: None,
                    repeat: false,
                },
            ]));
        let mut visibility = bedtime_filetree2::FileTreePreferences::default();
        visibility.roots.insert(
            root.clone(),
            bedtime_filetree2::FileTreeVisibility {
                hide_hidden: false,
                ..default()
            },
        );
        app.world_mut().insert_resource(visibility);
        app.world_mut()
            .resource_mut::<crate::settings::ProjectPreferences>()
            .0
            .font_size = Some(22.0);
        let live = capture(app.world_mut()).unwrap();
        app.world_mut()
            .resource_mut::<crate::settings::ProjectPreferences>()
            .0 = crate::settings::SettingsOverrides {
            line_numbers: Some(false),
            ..default()
        };
        app.world_mut()
            .insert_resource(crate::keybindings::ProjectBindings::default());
        let mut visibility = bedtime_filetree2::FileTreePreferences::default();
        visibility.roots.insert(
            root.clone(),
            bedtime_filetree2::FileTreeVisibility::default(),
        );
        visibility.roots.insert(
            root.join("other"),
            bedtime_filetree2::FileTreeVisibility::default(),
        );
        app.world_mut().insert_resource(visibility);
        let disk = capture(app.world_mut()).unwrap();
        let merged = merge_loaded(app.world(), &baseline, &live, &disk).unwrap();
        let prepared =
            crate::persistence::prepare_local(&merged, app.world().resource::<AppTypeRegistry>())
                .unwrap();
        let settings = prepared
            .resource::<crate::settings::ProjectPreferences>()
            .unwrap();
        assert_eq!(settings.0.font_size, Some(22.0));
        assert_eq!(settings.0.line_numbers, Some(false));
        let visibility = prepared
            .resource::<crate::files::PersistedVisibility>()
            .unwrap()
            .0;
        assert!(!visibility.roots[&root].hide_hidden);
        assert!(visibility.roots.contains_key(&root.join("other")));
        assert!(
            prepared
                .resource::<crate::keybindings::ProjectBindings>()
                .unwrap()
                .0[0]
                .command
                .is_none()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_write_preserves_previous_file_and_retries_after_repair() {
        let root = fixture();
        let mut app = host(&root, true);
        settle(&mut app).unwrap();
        settings_panel(app.world_mut());
        settle(&mut app).unwrap();
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"malformed replacement").unwrap();
        app.world_mut().resource_mut::<ProjectTitle>().0 = "pending".into();
        assert!(settle(&mut app).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"malformed replacement");
        std::fs::write(&path, before).unwrap();
        retry(app.world_mut()).unwrap();
        settle(&mut app).unwrap();
        let stored = read_source(&path, app.world().resource::<AppTypeRegistry>())
            .unwrap()
            .unwrap();
        let prepared = crate::persistence::prepare_local(
            &stored.local,
            app.world().resource::<AppTypeRegistry>(),
        )
        .unwrap();
        assert_eq!(prepared.resource::<ProjectTitle>().unwrap().0, "pending");
        std::fs::remove_dir_all(root).unwrap();
    }
}
