//! Application-owned concrete workspace recipes, never document/process state.
use bedtime_workbench2::*;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const VERSION: u32 = 1;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct SessionPlugin;
impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Sessions>()
            .add_systems(PostUpdate, capture_sessions);
    }
}

/// Startup composition is remembered before asynchronous root resolution, so a
/// user mutation made while identity is loading isn't mistaken for a default.
#[derive(Component)]
struct InitialSession(Session);

pub fn remember_initial_state(world: &mut World, workspace: Entity) -> Result<(), String> {
    if world.get::<InitialSession>(workspace).is_none() {
        let initial = capture(world, workspace)?;
        world.entity_mut(workspace).insert(InitialSession(initial));
    }
    Ok(())
}

/// Root-derived naming is startup policy, not a user mutation. Preserve a name
/// the user already changed, while advancing the remembered startup baseline.
pub fn seed_initial_layout(world: &mut World, workspace: Entity) {
    if let Ok(current) = capture(world, workspace) {
        if let Some(mut initial) = world.get_mut::<InitialSession>(workspace) {
            initial.0.layouts = current.layouts.clone();
            initial.0.active_layout = current.active_layout;
        }
        // A missing workspace file is resolved before instantiating defaults:
        // otherwise a starter terminal could run before restoration. Seeding
        // this untouched composition is startup policy, not an autosave edit.
        if let Some(mut sessions) = world.get_resource_mut::<Sessions>()
            && let Some(file) = sessions.0.get_mut(&workspace)
            && file.policy_ready
            && file.pending_restore.is_none()
            && file.desired == file.saved
        {
            file.saved = current.clone();
            file.desired = current;
        }
    }
}

pub fn initialize_folder_name(world: &mut World, workspace: Entity, name: String) {
    let previous = world
        .get::<InitialSession>(workspace)
        .map(|initial| initial.0.name.clone());
    if previous
        .as_ref()
        .is_none_or(|previous| world.get::<Workspace>(workspace).unwrap().name == *previous)
    {
        world.get_mut::<Workspace>(workspace).unwrap().name = name.clone();
    }
    if let Some(mut initial) = world.get_mut::<InitialSession>(workspace) {
        initial.0.name = name;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Session {
    pub(crate) version: u32,
    pub(crate) root: Option<PathBuf>,
    pub(crate) settings: crate::settings::SettingsOverrides,
    pub(crate) visibility: bedtime_filetree2::FileTreePreferences,
    pub(crate) name: String,
    pub(crate) background_updates: bool,
    pub(crate) active_layout: Option<usize>,
    pub(crate) layouts: Vec<LayoutRecipe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayoutRecipe {
    pub(crate) slot: Option<crate::layouts::LayoutSlot>,
    pub(crate) name: String,
    pub(crate) focused_area: Option<usize>,
    pub(crate) areas: Vec<AreaRecipe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AreaRecipe {
    pub(crate) rect: AreaRect,
    pub(crate) selected_tab: Option<usize>,
    pub(crate) tabs: Vec<TabRecipe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TabRecipe {
    pub(crate) kind: PanelKind,
    pub(crate) title: String,
    pub(crate) preview: bool,
    pub(crate) file: Option<PathBuf>,
    pub(crate) tree_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PanelKind {
    Files,
    Code,
    Structure,
    Terminal,
    Viewport,
    Tasks,
    Settings,
}

pub(crate) fn validate(session: &Session) -> Result<(), String> {
    if session.version != VERSION {
        return Err("Unsupported workspace session version".into());
    }
    session.settings.validate()?;
    if session
        .root
        .as_ref()
        .is_some_and(|root| !root.is_absolute())
    {
        return Err("Workspace root must be absolute".into());
    }
    let mut slots = std::collections::BTreeSet::new();
    for layout in &session.layouts {
        if let Some(slot) = layout.slot {
            if !slots.insert(slot) {
                return Err("Duplicate layout slot".into());
            }
        }
    }
    let text = |s: &str| s.len() <= 4096 && !s.contains('\0');
    if !text(&session.name)
        || session.layouts.len() > 32
        || session
            .active_layout
            .is_some_and(|i| i >= session.layouts.len())
    {
        return Err("Invalid workspace name or layout selection/count".into());
    }
    let mut area_count = 0;
    let mut tab_count = 0;
    for layout in &session.layouts {
        area_count += layout.areas.len();
        if !text(&layout.name)
            || area_count > 256
            || layout.focused_area.is_some_and(|i| i >= layout.areas.len())
        {
            return Err("Invalid layout name, area selection or area count".into());
        }
        let mut coverage = 0_i64;
        for (i, area) in layout.areas.iter().enumerate() {
            let r = area.rect;
            if (0..2).any(|a| r.min[a] < 0 || r.max[a] > LAYOUT_EXTENT || r.min[a] >= r.max[a]) {
                return Err("Invalid workspace area geometry".into());
            }
            coverage += i64::from(r.max[0] - r.min[0]) * i64::from(r.max[1] - r.min[1]);
            if layout.areas[..i].iter().any(|other| {
                (0..2).all(|a| r.min[a] < other.rect.max[a] && other.rect.min[a] < r.max[a])
            }) {
                return Err("Overlapping workspace areas".into());
            }
            tab_count += area.tabs.len();
            if tab_count > 2048 || area.selected_tab.is_some_and(|i| i >= area.tabs.len()) {
                return Err("Invalid tab selection or tab count".into());
            }
            for tab in &area.tabs {
                if !text(&tab.title)
                    || tab.file.as_ref().is_some_and(|p| {
                        tab.kind != PanelKind::Code
                            || !p.is_absolute()
                            || p.as_os_str().len() > 16384
                    })
                    || tab.tree_root.as_ref().is_some_and(|p| {
                        tab.kind != PanelKind::Files
                            || !p.is_absolute()
                            || p.as_os_str().len() > 16384
                    })
                {
                    return Err("Invalid panel title or source descriptor".into());
                }
            }
        }
        if !layout.areas.is_empty() && coverage != i64::from(LAYOUT_EXTENT).pow(2) {
            return Err("Workspace areas do not completely cover the layout".into());
        }
    }
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<Session, String> {
    if bytes.len() > MAX_BYTES {
        return Err("Workspace session exceeds 4 MiB".into());
    }
    let session: Session = ron::de::from_bytes(bytes).map_err(|e| e.to_string())?;
    validate(&session)?;
    Ok(session)
}

pub(crate) fn capture(world: &World, workspace: Entity) -> Result<Session, String> {
    let owner = world
        .get::<Workspace>(workspace)
        .ok_or("Workspace retired")?;
    let layouts: Vec<_> = world
        .get::<WorkspaceMembers>(workspace)
        .map_or(&[][..], |m| m.entities())
        .iter()
        .copied()
        .filter(|&e| world.get::<Layout>(e).is_some())
        .collect();
    let mut recipes = Vec::new();
    for &layout in &layouts {
        let data = world.get::<Layout>(layout).unwrap();
        let areas = world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities());
        let mut area_recipes = Vec::new();
        for &area in areas {
            let data = world.get::<Area>(area).ok_or("Missing workspace area")?;
            let panels = panels_in_area(world, area);
            let mut tabs = Vec::new();
            for &panel in panels {
                let instance = world
                    .get::<PanelInstance>(panel)
                    .ok_or("Missing panel metadata")?;
                // Component identity, never a registry or a key-based factory.
                let kind = if world
                    .get::<bedtime_filetree2::FileTreePanel>(panel)
                    .is_some()
                {
                    PanelKind::Files
                } else if world.get::<crate::files::FilePreviewPanel>(panel).is_some() {
                    PanelKind::Code
                } else if world.get::<bed_structure2::StructurePanel>(panel).is_some() {
                    PanelKind::Structure
                } else if world
                    .get::<bedtime_terminal2::TerminalPanel>(panel)
                    .is_some()
                {
                    PanelKind::Terminal
                } else if world.get::<crate::cube::CubeViewport>(panel).is_some() {
                    PanelKind::Viewport
                } else if world.get::<crate::tasks::TasksPanel>(panel).is_some() {
                    PanelKind::Tasks
                } else if world.get::<crate::settings::SettingsPanel>(panel).is_some() {
                    PanelKind::Settings
                } else {
                    return Err(format!(
                        "Unsupported concrete panel {:?}; session not overwritten",
                        instance.key
                    ));
                };
                tabs.push(TabRecipe {
                    kind,
                    // Foreground-process names are observations, not workspace
                    // mutations. A starter shell must not create a session file
                    // merely because its first title observation arrived.
                    title: if kind == PanelKind::Terminal {
                        bedtime_terminal2::TerminalPanel::LABEL.into()
                    } else {
                        instance.title.clone()
                    },
                    preview: instance.preview,
                    file: crate::files::panel_path(world, panel),
                    tree_root: world
                        .get::<bedtime_filetree2::FileTreeRoot>(panel)
                        .map(|r| r.0.clone()),
                });
            }
            area_recipes.push(AreaRecipe {
                rect: data.rect,
                selected_tab: data
                    .selected
                    .and_then(|e| panels.iter().position(|&p| p == e)),
                tabs,
            });
        }
        recipes.push(LayoutRecipe {
            slot: world.get::<crate::layouts::LayoutSlot>(layout).copied(),
            name: data.name.clone(),
            focused_area: data
                .focused_area
                .and_then(|e| areas.iter().position(|&a| a == e)),
            areas: area_recipes,
        });
    }
    let session = Session {
        version: VERSION,
        root: world
            .get::<crate::files::WorkspaceRoot>(workspace)
            .map(|r| r.0.clone()),
        settings: world
            .get::<crate::settings::WorkspacePreferences>(workspace)
            .map(|p| p.0.clone())
            .unwrap_or_default(),
        visibility: world
            .get::<bedtime_filetree2::FileTreePreferences>(workspace)
            .cloned()
            .unwrap_or_default(),
        name: owner.name.clone(),
        background_updates: owner.background_updates,
        active_layout: active_layout(world, workspace)
            .and_then(|e| layouts.iter().position(|&l| l == e)),
        layouts: recipes,
    };
    validate(&session)?;
    Ok(session)
}

/// Validate everything before changing entities. Late loads never retire documents.
fn restore(world: &mut World, workspace: Entity, session: &Session) -> Result<(), String> {
    validate(session)?;
    if !document_ids(world, workspace).is_empty() {
        return Err("Documents opened while the workspace session was loading; existing work was preserved. Reopen the folder to restore its session.".into());
    }
    if world.get::<WorkspaceWork>(workspace).is_some_and(|work| {
        work.list().iter().any(|job| {
            job.owner.is_some_and(|owner| {
                world.get::<crate::tasks::TasksPanel>(owner).is_some()
                    || world
                        .get::<bedtime_terminal2::TerminalPanel>(owner)
                        .is_some()
            })
        })
    }) {
        return Err("Tasks or terminals started while the session was loading; live work was preserved. Explicitly retry to save the current workspace instead.".into());
    }
    let old: Vec<_> = world
        .get::<WorkspaceMembers>(workspace)
        .map_or(&[][..], |m| m.entities())
        .iter()
        .copied()
        .filter(|&e| world.get::<Layout>(e).is_some())
        .collect();
    // Nothing below can reject the graph: all kinds/references/geometry passed
    // validation. Retire startup composition before spawning so relationship
    // removal cannot reorder the newly restored layouts.
    for layout in old {
        world.despawn(layout);
    }
    world.flush();
    world.entity_mut(workspace).insert((
        crate::settings::WorkspacePreferences(session.settings.clone()),
        session.visibility.clone(),
    ));
    let mut layouts = Vec::new();
    let mut reopen = Vec::new();
    let mut selections = Vec::new();
    let mut focuses = Vec::new();
    for recipe in &session.layouts {
        let layout = world
            .spawn((
                Layout {
                    name: recipe.name.clone(),
                    focused_area: None,
                },
                InWorkspace(workspace),
            ))
            .id();
        if let Some(slot) = recipe.slot {
            world.entity_mut(layout).insert(slot);
        }
        layouts.push(layout);
        let mut areas = Vec::new();
        for recipe in &recipe.areas {
            let area = world
                .spawn((
                    Area {
                        rect: recipe.rect,
                        selected: None,
                    },
                    InLayout(layout),
                ))
                .id();
            areas.push(area);
            let mut panels = Vec::new();
            for tab in &recipe.tabs {
                let panel = match tab.kind {
                    PanelKind::Files => world
                        .spawn((bedtime_filetree2::FileTreePanel::default(), InArea(area)))
                        .id(),
                    PanelKind::Code => world
                        .spawn((crate::files::FilePreviewPanel::default(), InArea(area)))
                        .id(),
                    PanelKind::Structure => world
                        .spawn((bed_structure2::StructurePanel::default(), InArea(area)))
                        .id(),
                    PanelKind::Terminal => {
                        let mut options = bedtime_terminal2::TerminalOptions::default();
                        if crate::settings::effective_preferences(world, workspace)
                            .terminal
                            .unwrap_or_default()
                            .project_directory
                        {
                            options.working_directory = world
                                .get::<crate::files::WorkspaceRoot>(workspace)
                                .map(|r| r.0.clone());
                        }
                        world
                            .spawn((bedtime_terminal2::TerminalPanel::new(options), InArea(area)))
                            .id()
                    }
                    PanelKind::Viewport => {
                        world.spawn((crate::cube::CubeViewport, InArea(area))).id()
                    }
                    PanelKind::Tasks => world
                        .spawn((crate::tasks::TasksPanel::default(), InArea(area)))
                        .id(),
                    PanelKind::Settings => world
                        .spawn((crate::settings::SettingsPanel, InArea(area)))
                        .id(),
                };
                panels.push(panel);
                if let Some(root) = &tab.tree_root {
                    world
                        .entity_mut(panel)
                        .insert(bedtime_filetree2::FileTreeRoot(root.clone()));
                }
                if let Some(path) = &tab.file {
                    reopen.push((panel, path.clone(), tab.title.clone()));
                }
                let mut metadata = world.get_mut::<PanelInstance>(panel).unwrap();
                metadata.title = tab.title.clone();
                metadata.preview = tab.preview;
            }
            selections.push((area, recipe.selected_tab.map(|i| panels[i])));
        }
        focuses.push((layout, recipe.focused_area.map(|i| areas[i])));
    }
    // Drain attachment hooks before applying persisted selection/focus; hooks
    // correctly select newly spawned tabs during ordinary composition.
    world.flush();
    for (area, selected) in selections {
        world.get_mut::<Area>(area).unwrap().selected = selected;
    }
    for (layout, focused) in focuses {
        world.get_mut::<Layout>(layout).unwrap().focused_area = focused;
    }
    for &layout in &layouts {
        world.entity_mut(layout).remove::<ActiveLayout>();
    }
    if let Some(i) = session.active_layout {
        world.entity_mut(layouts[i]).insert(ActiveLayout);
    }
    let mut owner = world.get_mut::<Workspace>(workspace).unwrap();
    owner.name = session.name.clone();
    owner.background_updates = session.background_updates;
    // A missing file leaves the editor/error visible; the graph is still valid.
    for (panel, path, title) in reopen {
        if let Err(error) = crate::opening::restore_file_panel(world, panel, path) {
            request_error_dialog(
                world,
                ErrorDialog {
                    title: "Restore file".into(),
                    message: "The workspace was restored, but a file couldn't be reopened.".into(),
                    details: Some(error),
                },
            );
        }
        world.get_mut::<PanelInstance>(panel).unwrap().title = title;
    }
    Ok(())
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
struct Sessions(BTreeMap<Entity, SessionFile>);

fn read_source(path: &std::path::Path) -> Result<Option<Session>, String> {
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
    decode(&bytes).map(Some)
}

fn read_job(path: PathBuf) -> Job {
    Job::Read(std::thread::spawn(move || read_source(&path)))
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
        let owners: Vec<_> = world.query_filtered::<(Entity, &crate::files::WorkspaceRoot), (With<Workspace>, With<crate::files::FolderWorkspace>)>()
            .iter(world).map(|(e, r)| (e, r.0.clone())).collect();
        let mut capture_errors = Vec::new();
        for (owner, root) in owners {
            let path = crate::storage::workspace_path(world, &root).unwrap();
            if !sessions.0.contains_key(&owner)
                && let Some(previous) = sessions.0.iter().find_map(|(&previous, file)| (previous != owner && file.path == path).then_some(previous)) {
                if world.get::<Workspace>(previous).is_some() {
                    return Err("The same folder cannot have two workspace storage owners".into());
                }
                let current = capture(world, owner)?;
                let pristine = world.get::<InitialSession>(owner).is_none_or(|initial| {
                    let mut baseline = initial.0.clone(); baseline.root = current.root.clone(); baseline == current
                });
                if !pristine { continue; }
                // Reopen joins the retained writer and its latest admitted state,
                // including a pending write/error. It never starts a second writer.
                let mut predecessor = sessions.0.remove(&previous).unwrap();
                if let Err(error) = restore(world, owner, &predecessor.desired) {
                    predecessor.error = Some(error);
                    sessions.0.insert(previous, predecessor);
                    continue;
                }
                sessions.0.insert(owner, predecessor);
                world.entity_mut(owner).remove::<InitialSession>();
            }
            let desired = match capture(world, owner) {
                Ok(recipe) => recipe,
                Err(error) => {
                    if let Some(file) = sessions.0.get_mut(&owner) { file.error = Some(error.clone()); }
                    capture_errors.push(error);
                    continue;
                }
            };
            let initial = world.get::<InitialSession>(owner).map(|initial| { let mut recipe = initial.0.clone(); recipe.root = desired.root.clone(); recipe });
            world.entity_mut(owner).remove::<InitialSession>();
            let file = sessions.0.entry(owner).or_insert_with(|| {
                let path = crate::storage::workspace_path(world, &root).unwrap();
                SessionFile { job: Some(read_job(path.clone())), path, saved: initial.unwrap_or_else(|| desired.clone()), desired: desired.clone(), loaded: false, policy_ready: false,
                    retrying: false, pending_restore: None, changed_at: Instant::now(), error: None, reported: false }
            });
            if file.path != crate::storage::workspace_path(world, &root).unwrap() {
                file.error = Some("Workspace root changed while session I/O was pending; reopen the folder instead".into());
            }
            if file.desired != desired { file.desired = desired; file.changed_at = Instant::now(); }
        }
        let mut drained = true;
        let mut errors = capture_errors;
        // Jobs/last buffered recipes outlive their workspace entities.
        for (&owner, file) in &mut sessions.0 {
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
                if !crate::storage::global_ready(world) || !crate::files::root_ready(world, owner) {
                    file.pending_restore = Some(recipe);
                } else if world.get::<Workspace>(owner).is_none() {
                    file.desired = file.saved.clone();
                } else {
                    let disk = recipe.clone();
                    recipe.settings = crate::settings::SettingsOverrides::merge_loaded(&file.saved.settings, &file.desired.settings, &recipe.settings);
                    let roots: std::collections::BTreeSet<_> = file.saved.visibility.roots.keys().chain(file.desired.visibility.roots.keys()).cloned().collect();
                    for root in roots {
                        if file.saved.visibility.roots.get(&root) != file.desired.visibility.roots.get(&root) {
                            if let Some(value) = file.desired.visibility.roots.get(&root) { recipe.visibility.roots.insert(root, value.clone()); }
                            else { recipe.visibility.roots.remove(&root); }
                        }
                    }
                    let composition_changed = file.desired.name != file.saved.name || file.desired.background_updates != file.saved.background_updates
                        || file.desired.active_layout != file.saved.active_layout || file.desired.layouts != file.saved.layouts;
                    if composition_changed {
                        file.error = Some("Workspace changed while its session was loading. Existing work and the disk session were preserved; reopen the folder to restore, or explicitly retry after review.".into());
                    } else {
                        match restore(world, owner, &recipe).and_then(|()| capture(world, owner)) {
                            Ok(restored) => { file.saved = disk; file.desired = restored; file.policy_ready = true; }
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
                continue;
            }
            if file.pending_restore.is_some() {
                drained = false;
                continue;
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
                        continue;
                    }
                };
                if bytes.len() > MAX_BYTES { file.error = Some("Workspace session exceeds 4 MiB; previous file preserved".into()); drained = false; continue; }
                let path = file.path.clone();
                let expected_root = recipe.root.clone();
                file.job = Some(Job::Write { recipe, handle: std::thread::spawn(move || {
                    // Revalidate before replacement too: an externally introduced
                    // malformed/newer-version session is not overwrite consent.
                    if read_source(&path)?.is_some_and(|existing| existing.root != expected_root) {
                        return Err("Workspace file belongs to a different folder; previous data preserved".into());
                    }
                    crate::storage::atomic_write(&path, &bytes)
                }) });
            }
            if file.job.is_some() || file.desired != file.saved { drained = false; }
        }
        sessions.0.retain(|owner, file| world.get::<Workspace>(*owner).is_some() || file.job.is_some()
            || file.pending_restore.is_some() || file.desired != file.saved || file.error.is_some());
        if errors.is_empty() { Ok(drained) } else { Err(errors.join("\n")) }
    })
}

pub fn preferences_ready(world: &World, workspace: Entity) -> bool {
    if world
        .get::<crate::files::FolderWorkspace>(workspace)
        .is_none()
        || !world.contains_resource::<crate::storage::ConfigDirectory>()
    {
        return true;
    }
    world.get_resource::<Sessions>().is_some_and(|sessions| {
        sessions
            .0
            .get(&workspace)
            .is_some_and(|file| file.policy_ready && file.pending_restore.is_none())
    })
}

/// Loading readiness for application UI/automation, independent of whether a
/// buffered mutation has reached disk. Temporary workspaces never load sessions.
#[cfg(feature = "automation")]
pub fn ready(world: &World, workspace: Entity) -> bool {
    if world
        .get::<crate::files::FolderWorkspace>(workspace)
        .is_none()
    {
        return true;
    }
    world.get_resource::<Sessions>().is_none_or(|sessions| {
        sessions
            .0
            .get(&workspace)
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
pub fn retry(world: &mut World, workspace: Entity) -> Result<(), String> {
    let mut sessions = world
        .get_resource_mut::<Sessions>()
        .ok_or("Session storage is not installed")?;
    let file = sessions
        .0
        .get_mut(&workspace)
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
    file.job = Some(read_job(file.path.clone()));
    Ok(())
}

/// Application settings expose the same retry used by quit, without requiring
/// a quit attempt just to repair workspace storage.
pub fn draw_status(ui: &mut bedtime_ui2::egui::Ui, world: &mut World, workspace: Entity) {
    let Some(file) = world
        .get_resource::<Sessions>()
        .and_then(|sessions| sessions.0.get(&workspace))
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
            if let Err(error) = retry(world, workspace) {
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
    let owners: Vec<_> = world
        .get_resource::<Sessions>()
        .map(|s| {
            s.0.iter()
                .filter_map(|(&owner, file)| {
                    (file.error.is_some() && file.job.is_none()).then_some(owner)
                })
                .collect()
        })
        .unwrap_or_default();
    for owner in owners {
        let _ = retry(world, owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    fn host(root: &std::path::Path, folder: bool) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, SessionPlugin));
        app.insert_resource(crate::storage::ConfigDirectory(root.join("config")));
        let owner = Workspaces::new(app.world_mut()).open("Initial").entity();
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::files::WorkspaceRoot(root.into()));
        if folder {
            app.world_mut()
                .entity_mut(owner)
                .insert(crate::files::FolderWorkspace);
        }
        (app, owner)
    }
    fn root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed2-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn settle(app: &mut App) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if flush(app.world_mut())? {
                return Ok(());
            }
            assert!(Instant::now() < deadline, "Session I/O timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn default_workspace_is_memory_only_even_when_root_has_a_session() {
        let root = root();
        let (mut app, owner) = host(&root, false);
        app.update();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Memory only".into();
        settle(&mut app).unwrap();
        assert!(!root.join(".bed").exists());
        std::fs::create_dir_all(root.join(".bed")).unwrap();
        std::fs::write(root.join(".bed/workspace2.json"), b"invalid").unwrap();
        app.update();
        settle(&mut app).unwrap();
        assert!(app.world().resource::<Sessions>().0.is_empty());
        assert_eq!(
            std::fs::read(root.join(".bed/workspace2.json")).unwrap(),
            b"invalid"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn root_derived_startup_name_does_not_count_as_a_user_mutation() {
        let root = root();
        let (mut app, owner) = host(&root, false);
        remember_initial_state(app.world_mut(), owner).unwrap();
        initialize_folder_name(app.world_mut(), owner, "Resolved folder".into());
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::files::FolderWorkspace);
        settle(&mut app).unwrap();
        assert!(!root.join(".bed").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mutations_before_root_resolution_are_saved_rather_than_mistaken_for_startup_defaults() {
        let root = root();
        let (mut app, owner) = host(&root, false);
        remember_initial_state(app.world_mut(), owner).unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Edited while resolving".into();
        initialize_folder_name(app.world_mut(), owner, "Resolved folder".into());
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::files::FolderWorkspace);
        settle(&mut app).unwrap();
        let saved = decode(
            &std::fs::read(root.join("config/workspaces").join(format!(
                "{}.bed",
                blake3::hash(root.as_os_str().as_encoded_bytes())
            )))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved.name, "Edited while resolving");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_mutations_are_buffered_flushed_and_roundtrip_without_startup_write() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        assert!(!root.join(".bed").exists());
        let layout = active_layout(app.world(), owner).unwrap();
        let area = Workbench::for_entity(app.world_mut(), owner)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let first = app
            .world_mut()
            .spawn((crate::settings::SettingsPanel, InArea(area)))
            .id();
        let second = app
            .world_mut()
            .spawn((crate::settings::SettingsPanel, InArea(area)))
            .id();
        app.world_mut()
            .get_mut::<PanelInstance>(first)
            .unwrap()
            .preview = true;
        let mut workbench = Workbench::for_entity(app.world_mut(), owner).unwrap();
        let split = workbench
            .split_area(area, SplitAxis::Horizontal, 4000, [100, 100])
            .unwrap();
        workbench
            .world_mut()
            .spawn((crate::tasks::TasksPanel::default(), InArea(split)));
        workbench.select_panel(first).unwrap();
        workbench.focus_area(split).unwrap();
        let extra = workbench.create_layout("Second layout").unwrap();
        workbench.activate_layout(extra).unwrap();
        let mut metadata = app.world_mut().get_mut::<Workspace>(owner).unwrap();
        metadata.name = "Renamed".into();
        metadata.background_updates = false;
        assert_ne!(first, second);
        app.update();
        assert!(
            !root
                .join("config/workspaces")
                .join(format!(
                    "{}.bed",
                    blake3::hash(root.as_os_str().as_encoded_bytes())
                ))
                .exists()
        );
        let expected = capture(app.world(), owner).unwrap();
        settle(&mut app).unwrap();
        let bytes = std::fs::read(root.join("config/workspaces").join(format!(
            "{}.bed",
            blake3::hash(root.as_os_str().as_encoded_bytes())
        )))
        .unwrap();
        assert_eq!(decode(&bytes).unwrap(), expected);
        let (mut restored, owner) = host(&root, true);
        settle(&mut restored).unwrap();
        assert_eq!(capture(restored.world(), owner).unwrap(), expected);
        assert_eq!(
            std::fs::read(root.join("config/workspaces").join(format!(
                "{}.bed",
                blake3::hash(root.as_os_str().as_encoded_bytes())
            )))
            .unwrap(),
            bytes
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_session_is_preserved_and_blocks_automatic_overwrite() {
        let root = root();
        std::fs::create_dir_all(root.join("config/workspaces")).unwrap();
        let bad = br#"{"version":999,"name":"Future","background_updates":true,"active_layout":null,"layouts":[]}"#;
        std::fs::write(
            root.join("config/workspaces").join(format!(
                "{}.bed",
                blake3::hash(root.as_os_str().as_encoded_bytes())
            )),
            bad,
        )
        .unwrap();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Still editable".into();
        assert!(flush(app.world_mut()).is_err());
        assert_eq!(
            std::fs::read(root.join("config/workspaces").join(format!(
                "{}.bed",
                blake3::hash(root.as_os_str().as_encoded_bytes())
            )))
            .unwrap(),
            bad
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restore_waits_for_preferences_without_starting_terminals_or_overwriting_late_edits() {
        for late_edit in [false, true] {
            let root = root();
            let (mut app, owner) = host(&root, true);
            let mut recipe = capture(app.world(), owner).unwrap();
            recipe.name = "Restored".into();
            recipe.layouts[0].areas[0].tabs.push(TabRecipe {
                kind: PanelKind::Terminal,
                title: "Terminal".into(),
                preview: false,
                file: None,
                tree_root: None,
            });
            recipe.layouts[0].areas[0].selected_tab = Some(0);
            std::fs::create_dir_all(root.join("config/workspaces")).unwrap();
            let bytes = crate::storage::encode(&recipe).unwrap();
            std::fs::write(
                root.join("config/workspaces").join(format!(
                    "{}.bed",
                    blake3::hash(root.as_os_str().as_encoded_bytes())
                )),
                &bytes,
            )
            .unwrap();
            app.init_resource::<crate::settings::GlobalPreferences>();
            crate::storage::install(&mut app);
            crate::storage::configure_directory(&mut app, root.join("config"));
            assert!(!crate::storage::ready(app.world(), owner));
            let deadline = Instant::now() + Duration::from_secs(5);
            assert!(!flush(app.world_mut()).unwrap());
            assert_eq!(
                app.world_mut()
                    .query::<&bedtime_terminal2::TerminalPanel>()
                    .iter(app.world())
                    .count(),
                0
            );
            while !crate::storage::flush(app.world_mut()).unwrap() {
                assert!(Instant::now() < deadline, "Preference load timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(!flush(app.world_mut()).unwrap());
            if late_edit {
                app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Local edit".into();
                let deadline = Instant::now() + Duration::from_secs(5);
                while flush(app.world_mut()).is_ok() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }

                assert_eq!(
                    app.world().get::<Workspace>(owner).unwrap().name,
                    "Local edit"
                );
                assert_eq!(
                    app.world_mut()
                        .query::<&bedtime_terminal2::TerminalPanel>()
                        .iter(app.world())
                        .count(),
                    0
                );
            } else {
                settle(&mut app).unwrap();
                assert_eq!(capture(app.world(), owner).unwrap(), recipe);
                assert_eq!(
                    app.world_mut()
                        .query::<&bedtime_terminal2::TerminalPanel>()
                        .iter(app.world())
                        .count(),
                    1
                );
            }
            assert_eq!(
                std::fs::read(root.join("config/workspaces").join(format!(
                    "{}.bed",
                    blake3::hash(root.as_os_str().as_encoded_bytes())
                )))
                .unwrap(),
                bytes
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_descriptor_serialization_failure_is_visible_and_blocks_unsaved_quit() {
        use std::os::unix::ffi::OsStringExt;
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        let layout = active_layout(app.world(), owner).unwrap();
        let area = Workbench::for_entity(app.world_mut(), owner)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let path = root.join(std::ffi::OsString::from_vec(vec![0xff]));
        app.world_mut().spawn((
            bedtime_filetree2::FileTreePanel::default(),
            bedtime_filetree2::FileTreeRoot(path),
            InArea(area),
        ));
        app.update();
        assert!(!flush(app.world_mut()).unwrap());
        assert!(
            app.world().resource::<Sessions>().0[&owner]
                .error
                .as_ref()
                .unwrap()
                .contains("serialize")
        );
        app.update();
        assert!(app.world().resource::<ErrorDialogs>().front().is_some());
        assert!(flush(app.world_mut()).is_err());
        assert!(
            !root
                .join("config/workspaces")
                .join(format!(
                    "{}.bed",
                    blake3::hash(root.as_os_str().as_encoded_bytes())
                ))
                .exists()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_geometry_and_references_are_rejected_before_restore() {
        let root = root();
        let (mut app, owner) = host(&root, false);
        let original = capture(app.world(), owner).unwrap();
        let mut invalid = original.clone();
        invalid.layouts[0].areas[0].rect.max[0] -= 1;
        assert!(restore(app.world_mut(), owner, &invalid).is_err());
        invalid = original.clone();
        invalid.layouts[0].focused_area = Some(99);
        assert!(restore(app.world_mut(), owner, &invalid).is_err());
        assert_eq!(capture(app.world(), owner).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn buffered_write_survives_workspace_retirement() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Retired but saved".into();
        app.update();
        app.world_mut().despawn(owner);
        settle(&mut app).unwrap();
        assert_eq!(
            decode(
                &std::fs::read(root.join("config/workspaces").join(format!(
                    "{}.bed",
                    blake3::hash(root.as_os_str().as_encoded_bytes())
                )))
                .unwrap()
            )
            .unwrap()
            .name,
            "Retired but saved"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn workspace_settings_and_native_visibility_share_one_ron_file() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::settings::WorkspacePreferences(
                crate::settings::SettingsOverrides {
                    line_numbers: Some(false),
                    panel_gap: Some(9.0),
                    effects_enabled: Some(true),
                    curvature_intensity: Some(0.7),
                    ..default()
                },
            ));
        let mut visibility = bedtime_filetree2::FileTreePreferences::default();
        let mut hidden = bedtime_filetree2::FileTreeVisibility::default();
        hidden.hide("private".into()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            hidden
                .hide(std::ffi::OsString::from_vec(vec![0xff]).into())
                .unwrap();
        }
        visibility.roots.insert(root.clone(), hidden);
        app.world_mut().entity_mut(owner).insert(visibility.clone());
        let expected = capture(app.world(), owner).unwrap();
        settle(&mut app).unwrap();
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(decode(&bytes).unwrap(), expected);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        assert!(!root.join(".bed").exists());
        let (mut restored, owner) = host(&root, true);
        settle(&mut restored).unwrap();
        assert_eq!(
            restored
                .world()
                .get::<bedtime_filetree2::FileTreePreferences>(owner),
            Some(&visibility)
        );
        assert_eq!(
            restored
                .world()
                .get::<crate::settings::WorkspacePreferences>(owner)
                .unwrap()
                .0
                .line_numbers,
            Some(false)
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        let settings = &restored
            .world()
            .get::<crate::settings::WorkspacePreferences>(owner)
            .unwrap()
            .0;
        assert_eq!(settings.effects_enabled, Some(true));
        assert_eq!(settings.curvature_intensity, Some(0.7));
        assert_eq!(settings.bloom_intensity, None);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn late_workspace_read_merges_individual_settings_and_visibility_roots() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        let mut disk = capture(app.world(), owner).unwrap();
        disk.settings.line_numbers = Some(false);
        disk.settings.smart_indentation = Some(false);
        disk.visibility.roots.insert(
            root.join("untouched"),
            bedtime_filetree2::FileTreeVisibility::default(),
        );
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        crate::storage::atomic_write(&path, &crate::storage::encode(&disk).unwrap()).unwrap();
        assert!(!flush(app.world_mut()).unwrap());
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::settings::WorkspacePreferences(
                crate::settings::SettingsOverrides {
                    line_numbers: Some(true),
                    ..default()
                },
            ));
        settle(&mut app).unwrap();
        let restored = app
            .world()
            .get::<crate::settings::WorkspacePreferences>(owner)
            .unwrap();
        assert_eq!(restored.0.line_numbers, Some(true));
        assert_eq!(restored.0.smart_indentation, Some(false));
        assert!(
            app.world()
                .get::<bedtime_filetree2::FileTreePreferences>(owner)
                .unwrap()
                .roots
                .contains_key(&root.join("untouched"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn workspace_identity_and_write_failures_preserve_the_previous_file() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "First".into();
        settle(&mut app).unwrap();
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        let original = std::fs::read(&path).unwrap();
        let directory = path.parent().unwrap();
        let backup = root.join("backup");
        std::fs::rename(directory, &backup).unwrap();
        std::fs::write(directory, "not a directory").unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Pending".into();
        let deadline = Instant::now() + Duration::from_secs(5);
        while flush(app.world_mut()).is_ok() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read(backup.join(path.file_name().unwrap())).unwrap(),
            original
        );
        std::fs::remove_file(directory).unwrap();
        std::fs::rename(backup, directory).unwrap();
        retry(app.world_mut(), owner).unwrap();
        settle(&mut app).unwrap();
        assert_eq!(
            decode(&std::fs::read(&path).unwrap()).unwrap().name,
            "Pending"
        );
        let mut wrong = decode(&std::fs::read(&path).unwrap()).unwrap();
        wrong.root = Some(root.join("different"));
        let bytes = crate::storage::encode(&wrong).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let (mut reopened, owner) = host(&root, true);
        settle(&mut reopened).unwrap();
        assert!(!preferences_ready(reopened.world(), owner));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn layout_defaults_wait_for_restore_and_missing_files_do_not_save_startup() {
        for existing in [false, true] {
            let root = root();
            let (mut app, owner) = host(&root, true);
            app.add_plugins(crate::layouts::LayoutsPlugin);
            let path = crate::storage::workspace_path(app.world(), &root).unwrap();
            if existing {
                let mut disk = capture(app.world(), owner).unwrap();
                disk.layouts[0].slot = Some(crate::layouts::LayoutSlot::One);
                crate::storage::atomic_write(&path, &crate::storage::encode(&disk).unwrap())
                    .unwrap();
            }
            app.update();
            assert_eq!(
                app.world_mut()
                    .query::<&bedtime_terminal2::TerminalPanel>()
                    .iter(app.world())
                    .count(),
                0,
                "a starter shell must not be composed before the saved workspace is resolved"
            );
            settle(&mut app).unwrap();
            app.update();
            settle(&mut app).unwrap();
            if existing {
                assert_eq!(
                    app.world_mut()
                        .query::<&bedtime_terminal2::TerminalPanel>()
                        .iter(app.world())
                        .count(),
                    0
                );
            } else {
                assert_eq!(
                    app.world_mut()
                        .query::<&bedtime_terminal2::TerminalPanel>()
                        .iter(app.world())
                        .count(),
                    1
                );
                assert!(
                    !path.exists(),
                    "applying built-in defaults is not a user mutation"
                );
                let terminal = app
                    .world_mut()
                    .query_filtered::<Entity, With<bedtime_terminal2::TerminalPanel>>()
                    .single(app.world())
                    .unwrap();
                app.world_mut()
                    .get_mut::<PanelInstance>(terminal)
                    .unwrap()
                    .title = "observed foreground process".into();
                app.update();
                settle(&mut app).unwrap();
                assert!(
                    !path.exists(),
                    "derived roots and process labels are not user mutations"
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn reopening_a_retired_workspace_joins_its_pending_writer() {
        let root = root();
        let (mut app, owner) = host(&root, true);
        settle(&mut app).unwrap();
        app.world_mut().get_mut::<Workspace>(owner).unwrap().name = "Pending name".into();
        app.world_mut()
            .entity_mut(owner)
            .insert(crate::settings::WorkspacePreferences(
                crate::settings::SettingsOverrides {
                    panel_gap: Some(18.0),
                    ..default()
                },
            ));
        app.update();
        app.world_mut().despawn(owner);
        let reopened = Workspaces::new(app.world_mut())
            .open("Fresh startup")
            .entity();
        app.world_mut().entity_mut(reopened).insert((
            crate::files::WorkspaceRoot(root.clone()),
            crate::files::FolderWorkspace,
        ));
        remember_initial_state(app.world_mut(), reopened).unwrap();
        settle(&mut app).unwrap();
        assert_eq!(
            app.world().get::<Workspace>(reopened).unwrap().name,
            "Pending name"
        );
        assert_eq!(
            app.world()
                .get::<crate::settings::WorkspacePreferences>(reopened)
                .unwrap()
                .0
                .panel_gap,
            Some(18.0)
        );
        assert_eq!(app.world().resource::<Sessions>().0.len(), 1);
        let path = crate::storage::workspace_path(app.world(), &root).unwrap();
        assert_eq!(
            decode(&std::fs::read(path).unwrap()).unwrap().name,
            "Pending name"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "automation")]
pub fn inspect_storage(world: &World) -> Vec<serde_json::Value> {
    world
        .get_resource::<Sessions>()
        .map(|s| {
            s.0.iter().map(|(owner, f)| serde_json::json!({
        "workspace": owner.to_bits().to_string(), "root": f.desired.root,
        "path": f.path, "loaded": f.loaded, "pending": f.job.is_some() || f.pending_restore.is_some() || f.desired != f.saved, "error": f.error,
    })).collect()
        })
        .unwrap_or_default()
}
