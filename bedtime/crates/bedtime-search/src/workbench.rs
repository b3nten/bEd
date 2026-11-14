//! The search panel: a query with Match Case, Whole Word and Regular
//! Expression toggles, optional include/exclude filters, and the matches in
//! the workbench's `Root` grouped by file. A click (or Enter) opens the file
//! with the match selected; the arrow keys move through the results. With the
//! replace row shown, each match shows what it would become; Replace changes
//! one match and Replace All (once confirmed) every one listed: open
//! documents as an undoable edit (left unsaved), other files on disk if they
//! haven't changed since they were searched. The results follow unsaved edits.
use crate::engine::{FileMatches, MAX_MATCHES, Match, Matcher, OpenFiles, SearchQuery, Summary};
use bedtime_document::{BufferEdit, TextBuffer};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::color, tree};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, DocumentId, DocumentReference,
    EditOptions, Handler, KeyChord, ManagedTask, ManagedWork, MenusAppExt, Modifiers, Notification,
    Panel, PanelFactory, PanelFactoryPayload, PanelPresentation, PathOptions, Root, StandardMenu,
    Target, ViewerFile, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SearchPanel>()
            .register_type::<SearchQuery>()
            .register_action::<SearchWorkspace>()
            .add_action_handler::<NextResult>(Handler::new(|world, invocation| {
                step_result(world, invocation, true)
            }))
            .add_action_handler::<PreviousResult>(Handler::new(|world, invocation| {
                step_result(world, invocation, false)
            }))
            .add_menu_item::<SearchWorkspace>(StandardMenu::Edit, "find")
            .add_menu_item::<NextResult>(StandardMenu::Edit, "find")
            .add_menu_item::<PreviousResult>(StandardMenu::Edit, "find")
            .init_resource::<Intents>()
            .add_systems(
                Update,
                (
                    apply_intents,
                    start_searches,
                    collect_results,
                    collect_replacements,
                )
                    .chain()
                    .in_set(SearchSystems),
            );
    }
}

/// Doing what the panels were asked, then searching, in `Update`.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SearchSystems;

/// Open (or focus) the search panel, with the selection as the query. The
/// app handles it: where the panel goes is its to say (`seed` fills it in).
pub struct SearchWorkspace;
impl Action for SearchWorkspace {
    // Kept from when it was "Find in Files": saved key bindings use it.
    const KEY: &'static str = "search.find_in_files";
    const LABEL: &'static str = "Search Workspace";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        Modifiers::PRIMARY.union(Modifiers::SHIFT),
        KeyCode::KeyF,
    )];
}

/// Open the match after the one selected in the search panel (the first,
/// after the last), from wherever the focus is.
pub struct NextResult;
impl Action for NextResult {
    const KEY: &'static str = "search.next_result";
    const LABEL: &'static str = "Next Search Result";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(Modifiers::NONE, KeyCode::F4)];
}

/// As `NextResult`, the other way.
pub struct PreviousResult;
impl Action for PreviousResult {
    const KEY: &'static str = "search.previous_result";
    const LABEL: &'static str = "Previous Search Result";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(Modifiers::SHIFT, KeyCode::F4)];
}

/// The search panel. Its query, replacement, earlier queries and which rows
/// show are saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(SearchState)]
pub struct SearchPanel {
    pub query: SearchQuery,
    pub replacement: String,
    /// The replace row shows.
    pub replacing: bool,
    /// The include/exclude row shows.
    pub filtering: bool,
    /// Queries that were used (Enter, or a result opened or replaced), the
    /// newest last: Up and Down in the query field go through them.
    pub history: Vec<String>,
}

/// How many earlier queries are kept.
const HISTORY: usize = 30;

impl SearchPanel {
    /// Keep the query as one that was used.
    fn remember(&mut self) {
        let text = self.query.text.clone();
        if text.is_empty() {
            return;
        }
        self.history.retain(|earlier| *earlier != text);
        self.history.push(text);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
    }
}

/// Fill in `panel`'s query (a selection, say) and give the query field focus.
pub fn seed(world: &mut World, panel: Entity, text: Option<String>) {
    if let Some(text) = text.filter(|t| !t.is_empty() && !t.contains('\n'))
        && let Some(mut search) = world.get_mut::<SearchPanel>(panel)
    {
        search.query.text = text;
    }
    if let Some(mut state) = world.get_mut::<SearchState>(panel) {
        state.focus_query = true;
    }
}

/// The row that's selected: a file, or a match by where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Selected {
    File(PathBuf),
    Match(PathBuf, usize),
}

/// A row of the results: a file and, for one of its matches, which.
type At = (usize, Option<usize>);

/// What was taken out of the results (so Replace All leaves it alone): a
/// whole file, or matches by where they start.
enum Dismissed {
    File,
    Matches(HashSet<usize>),
}

/// The search's results and work. Not saved.
#[derive(Component, Default)]
pub struct SearchState {
    /// The query last seen, and when it last changed: searching waits for
    /// typing to pause.
    seen: Option<SearchQuery>,
    changed: Option<Instant>,
    /// The query last searched for, and the one `files` are for: the same,
    /// unless a search is yet to start. Searching again for what's shown
    /// keeps it on show until the new results are in.
    ran: Option<SearchQuery>,
    shown: Option<SearchQuery>,
    matcher: Option<Matcher>,
    task: Option<ManagedTask<Summary>>,
    incoming: Option<std::sync::Mutex<mpsc::Receiver<FileMatches>>>,
    /// Sorted by path; none without matches.
    files: Vec<FileMatches>,
    /// How many matches `files` have.
    matches: usize,
    /// A search for what's already shown collects here.
    fresh: Option<Vec<FileMatches>>,
    summary: Option<Summary>,
    error: Option<String>,
    collapsed: HashSet<PathBuf>,
    dismissed: HashMap<PathBuf, Dismissed>,
    selected: Option<Selected>,
    /// Scroll the selection into view.
    reveal: bool,
    /// Rows that fit in the view, for Page Up and Down.
    page: usize,
    focus_query: bool,
    /// The files with unsaved text (and its revision) when the search ran,
    /// and as last seen, and when that changed: the results are searched for
    /// again once editing pauses.
    unsaved_ran: Vec<(PathBuf, u64)>,
    unsaved_seen: Vec<(PathBuf, u64)>,
    unsaved_changed: Option<Instant>,
    /// Replace All was asked for and waits for a yes.
    confirming: bool,
    /// Replace All's work on files that aren't open.
    replacing: Option<ManagedTask<Replaced>>,
    /// Which earlier query the field shows, and what was typed before going
    /// back through them.
    recalled: Option<usize>,
    draft: String,
}

impl SearchState {
    pub fn files(&self) -> &[FileMatches] {
        &self.files
    }
    pub fn summary(&self) -> Option<&Summary> {
        self.summary.as_ref()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn searching(&self) -> bool {
        self.task.is_some()
    }

    /// Search again now, for the current query.
    fn rerun(&mut self) {
        self.ran = None;
        self.changed = None;
    }

    /// Forget the results.
    fn clear(&mut self) {
        self.files.clear();
        self.matches = 0;
        self.fresh = None;
        self.summary = None;
        self.matcher = None;
        self.shown = None;
        self.dismissed.clear();
        self.selected = None;
        self.confirming = false;
    }

    /// The unsaved text the results were searched in has changed since.
    fn outdated(&self) -> bool {
        self.matcher.is_some() && self.unsaved_seen != self.unsaved_ran
    }

    /// Take in what the running search has found so far.
    fn take_incoming(&mut self) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        let found: Vec<FileMatches> = incoming.lock().unwrap().try_iter().collect();
        for file in found {
            match &mut self.fresh {
                Some(fresh) => fresh.push(file),
                None => self.insert(file),
            }
        }
    }

    /// Add a file's matches, keeping the files in path order.
    fn insert(&mut self, file: FileMatches) {
        if file.matches.is_empty() {
            return;
        }
        self.matches += file.matches.len();
        let at = self.files.partition_point(|f| f.path < file.path);
        self.files.insert(at, file);
    }

    /// Show `fresh` in place of the results, without what was dismissed.
    fn show(&mut self, mut fresh: Vec<FileMatches>) {
        fresh.sort_by(|a, b| a.path.cmp(&b.path));
        fresh.retain_mut(|file| {
            match self.dismissed.get(&file.path) {
                Some(Dismissed::File) => return false,
                Some(Dismissed::Matches(starts)) => {
                    file.matches.retain(|m| !starts.contains(&m.range.start));
                }
                None => {}
            }
            !file.matches.is_empty()
        });
        self.matches = fresh.iter().map(|f| f.matches.len()).sum();
        self.files = fresh;
    }

    /// Where the selection is in the results, if it's still there.
    fn at(&self) -> Option<At> {
        let (path, start) = match self.selected.as_ref()? {
            Selected::File(path) => (path, None),
            Selected::Match(path, start) => (path, Some(*start)),
        };
        let file = self
            .files
            .binary_search_by(|f| f.path.as_path().cmp(path))
            .ok()?;
        let Some(start) = start else {
            return Some((file, None));
        };
        let index = self.files[file]
            .matches
            .binary_search_by_key(&start, |m| m.range.start)
            .ok()?;
        Some((file, Some(index)))
    }

    /// As `at`, among the rows that show: a match in a closed file is its
    /// file's row.
    fn row(&self) -> Option<At> {
        let (file, index) = self.at()?;
        Some((file, index.filter(|_| self.open(file))))
    }

    fn open(&self, file: usize) -> bool {
        !self.collapsed.contains(&self.files[file].path)
    }

    fn select(&mut self, at: At) {
        let file = &self.files[at.0];
        self.selected = Some(match at.1 {
            None => Selected::File(file.path.clone()),
            Some(index) => Selected::Match(file.path.clone(), file.matches[index].range.start),
        });
        self.reveal = true;
    }

    /// The row under `at`, among those that show.
    fn below(&self, (file, index): At) -> Option<At> {
        let matches = self.files[file].matches.len();
        match index {
            None if self.open(file) => Some((file, Some(0))),
            Some(index) if index + 1 < matches => Some((file, Some(index + 1))),
            _ => (file + 1 < self.files.len()).then_some((file + 1, None)),
        }
    }

    /// The row over `at`, among those that show.
    fn above(&self, (file, index): At) -> Option<At> {
        match index {
            Some(0) => Some((file, None)),
            Some(index) => Some((file, Some(index - 1))),
            None => {
                let file = file.checked_sub(1)?;
                let last = self.files[file].matches.len() - 1;
                Some((file, self.open(file).then_some(last)))
            }
        }
    }

    /// The match after (or before) `from`, going round; from nowhere, the
    /// first (or last).
    fn next_match(&self, from: Option<At>, forward: bool) -> Option<At> {
        let files = self.files.len();
        if files == 0 {
            return None;
        }
        let last = |file: usize| (file, Some(self.files[file].matches.len() - 1));
        Some(match (from, forward) {
            (None, true) => (0, Some(0)),
            (None, false) => last(files - 1),
            (Some((file, None)), true) => (file, Some(0)),
            (Some((file, Some(index))), true) => {
                if index + 1 < self.files[file].matches.len() {
                    (file, Some(index + 1))
                } else {
                    ((file + 1) % files, Some(0))
                }
            }
            (Some((file, None | Some(0))), false) => last((file + files - 1) % files),
            (Some((file, Some(index))), false) => (file, Some(index - 1)),
        })
    }

    /// Take a file, or one match, out of the results. The selection, if it
    /// was there, moves to what takes its place.
    fn dismiss(&mut self, (file, index): At) {
        let selected = self.at().filter(|at| at.0 == file);
        let path = self.files[file].path.clone();
        let mut gone = index.is_none();
        match index {
            None => {
                self.dismissed.insert(path, Dismissed::File);
            }
            Some(index) => {
                let found = self.files[file].matches.remove(index);
                self.matches -= 1;
                if let Dismissed::Matches(starts) = self
                    .dismissed
                    .entry(path)
                    .or_insert_with(|| Dismissed::Matches(HashSet::new()))
                {
                    starts.insert(found.range.start);
                }
                gone = self.files[file].matches.is_empty();
            }
        }
        if gone {
            self.matches -= self.files[file].matches.len();
            self.files.remove(file);
        }
        let Some((_, was)) = selected else {
            return;
        };
        if gone {
            self.selected = None;
            if !self.files.is_empty() {
                self.select((file.min(self.files.len() - 1), None));
            }
        } else if let Some(index) = index
            && was == Some(index)
        {
            let last = self.files[file].matches.len() - 1;
            self.select((file, Some(index.min(last))));
        }
    }

    /// Take a match that was replaced out of the results, until they're
    /// searched for again.
    fn remove_match(&mut self, path: &Path, start: usize) {
        let Ok(file) = self.files.binary_search_by(|f| f.path.as_path().cmp(path)) else {
            return;
        };
        let matches = &mut self.files[file].matches;
        let Ok(index) = matches.binary_search_by_key(&start, |m| m.range.start) else {
            return;
        };
        matches.remove(index);
        self.matches -= 1;
        if matches.is_empty() {
            self.files.remove(file);
        }
    }
}

/// Typing settles for this long before a search starts.
const SETTLE: Duration = Duration::from_millis(150);
/// Editing settles for this long before the results are searched for again.
const REFRESH_SETTLE: Duration = Duration::from_millis(400);

/// What the user asked for while the panel drew.
#[derive(Debug)]
enum Intent {
    /// Open the file at a match; `keep_focus` leaves the focus in the results.
    Open {
        path: PathBuf,
        found: Match,
        keep_focus: bool,
    },
    Replace(PathBuf, Match),
    ReplaceAll,
    Refresh,
}

#[derive(Resource, Default)]
struct Intents(Vec<(Entity, Intent)>);

/// Start searches whose query has settled, and search again for results
/// whose files have been edited since.
fn start_searches(world: &mut World) {
    let now = Instant::now();
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<SearchPanel>>()
        .iter(world)
        .collect();
    // Worked out once, and only if some panel has results.
    let mut unsaved_now = None;
    for panel in panels {
        let query = world.get::<SearchPanel>(panel).unwrap().query.clone();
        let mut state = world.get_mut::<SearchState>(panel).unwrap();
        if state.seen.as_ref() != Some(&query) {
            state.seen = Some(query.clone());
            state.changed = Some(now);
        }
        if !state.changed.is_none_or(|at| now - at >= SETTLE) {
            continue;
        }
        if state.ran.as_ref() != Some(&query) {
            start(world, panel, query, root.clone());
            continue;
        }
        if state.matcher.is_none() || state.replacing.is_some() {
            continue;
        }
        let unsaved = unsaved_now.get_or_insert_with(|| open_files(world, false).1);
        let mut state = world.get_mut::<SearchState>(panel).unwrap();
        if state.unsaved_seen != *unsaved {
            state.unsaved_seen = unsaved.clone();
            state.unsaved_changed = Some(now);
        }
        if state.outdated()
            && state.task.is_none()
            && state
                .unsaved_changed
                .is_none_or(|at| now - at >= REFRESH_SETTLE)
        {
            start(world, panel, query, root.clone());
        }
    }
}

fn start(world: &mut World, panel: Entity, query: SearchQuery, root: Option<PathBuf>) {
    let (open, unsaved) = open_files(world, true);
    let mut state = world.get_mut::<SearchState>(panel).unwrap();
    if let Some(task) = state.task.take() {
        task.cancel();
    }
    state.incoming = None;
    state.fresh = None;
    state.error = None;
    state.ran = Some(query.clone());
    state.unsaved_seen = unsaved.clone();
    state.unsaved_ran = unsaved;
    state.unsaved_changed = None;
    // The same search again: what's shown stays until the new results are in.
    let again = state.matcher.is_some() && state.shown.as_ref() == Some(&query);
    if !again {
        state.clear();
    }
    let matcher = match query.matcher() {
        None => return,
        Some(Err(error)) => {
            state.error = Some(error);
            return;
        }
        Some(Ok(matcher)) => matcher,
    };
    let root = match root {
        Some(root) => root,
        // Open files are searched wherever they are.
        None if query.open_only => PathBuf::new(),
        None => {
            state.error = Some("No folder is open".into());
            return;
        }
    };
    state.matcher = Some(matcher.clone());
    state.shown = Some(query.clone());
    let (sender, receiver) = mpsc::channel();
    let spawned = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        format!("Searching for {}", query.text),
        move |cancel| {
            matcher.search(&root, &open, &|| cancel.is_cancelled(), &|found| {
                let _ = sender.send(found);
            })
        },
    );
    let mut state = world.get_mut::<SearchState>(panel).unwrap();
    match spawned {
        Ok(task) => {
            state.task = Some(task);
            state.incoming = Some(std::sync::Mutex::new(receiver));
            state.fresh = again.then(Vec::new);
        }
        Err(error) => state.error = Some(error),
    }
}

/// The files open in panels (with their unsaved text, if `texts`), and which
/// of them have unsaved text, with its revision.
fn open_files(world: &mut World, texts: bool) -> (OpenFiles, Vec<(PathBuf, u64)>) {
    let documents: Vec<(PathBuf, DocumentId)> = world
        .query::<(&ViewerFile, &DocumentReference)>()
        .iter(world)
        .map(|(file, reference)| (file.0.clone(), reference.document()))
        .collect();
    let workbench = world.workbench();
    let mut open = OpenFiles::new();
    let mut unsaved = Vec::new();
    for (path, id) in documents {
        if open.contains_key(&path) {
            continue;
        }
        let revision = workbench
            .inspect_document(id)
            .ok()
            .filter(|info| info.dirty)
            .map(|info| info.revision);
        let text = revision.filter(|_| texts).and_then(|_| {
            let handle = workbench.cast_document::<TextBuffer>(id).ok()?;
            let text = workbench.read_document(handle).ok()?;
            Some(Arc::from(text.to_string()))
        });
        if let Some(revision) = revision {
            unsaved.push((path.clone(), revision));
        }
        open.insert(path, text);
    }
    unsaved.sort();
    (open, unsaved)
}

/// Take in what running searches have found.
fn collect_results(mut states: Query<&mut SearchState>) {
    for mut state in &mut states {
        if state.task.is_none() {
            continue;
        }
        let state = &mut *state;
        state.take_incoming();
        let result = match state.task.as_ref().unwrap().try_recv() {
            Ok(summary) => Ok(summary),
            Err(mpsc::TryRecvError::Empty) => continue,
            Err(mpsc::TryRecvError::Disconnected) => Err("The search stopped".to_owned()),
        };
        // What it found between the two looks.
        state.take_incoming();
        state.incoming = None;
        state.task = None;
        if let Some(fresh) = state.fresh.take() {
            state.show(fresh);
        }
        match result {
            Ok(summary) => state.summary = Some(summary),
            Err(error) => state.error = Some(error),
        }
    }
}

fn apply_intents(world: &mut World) {
    let intents = std::mem::take(&mut world.resource_mut::<Intents>().0);
    for (panel, intent) in intents {
        if world.get_entity(panel).is_err() {
            continue;
        }
        match intent {
            Intent::Open {
                path,
                found,
                keep_focus,
            } => open_match(world, panel, &path, &found, keep_focus),
            Intent::Replace(path, found) => {
                let mut done = Replaced::default();
                done.add(replace_one(world, panel, &path, &found));
                if done.matches > 0
                    && let Some(mut state) = world.get_mut::<SearchState>(panel)
                {
                    state.remove_match(&path, found.range.start);
                }
                finish_replacing(world, panel, done, false);
            }
            Intent::ReplaceAll => replace_all(world, panel),
            Intent::Refresh => {
                if let Some(mut state) = world.get_mut::<SearchState>(panel) {
                    state.rerun();
                }
            }
        }
    }
}

/// Open `path` with `found` selected: in a panel already showing it, else
/// somewhere other than the search panel.
fn open_match(world: &mut World, panel: Entity, path: &Path, found: &Match, keep_focus: bool) {
    let options = PathOptions {
        line: Some(found.start),
        select_to: Some(found.end),
        preview: true,
        keep_focus,
        place: Some(bedtime_workbench::Placement::From(panel)),
        ..default()
    };
    if let Err(error) = world.workbench_mut().open_path(path, options) {
        let name = path.file_name().unwrap_or(path.as_os_str()).display();
        world.workbench_mut().notify(Notification::error(format!(
            "Couldn't open {name}: {error}"
        )));
    }
}

/// Next (or Previous) Search Result: open the match after the one selected,
/// in the search panel it was asked from, else one that shows, else any.
fn step_result(world: &mut World, invocation: ActionInvocation, forward: bool) -> ActionResult {
    let asked = invocation
        .entity
        .filter(|entity| world.get::<SearchPanel>(*entity).is_some());
    let panel = asked.or_else(|| {
        let mut panels: Vec<(bool, Entity)> = world
            .query::<(Entity, &SearchState, Option<&PanelPresentation>)>()
            .iter(world)
            .filter(|(_, state, _)| !state.files.is_empty())
            .map(|(entity, _, shown)| (!shown.is_some_and(|shown| shown.visible), entity))
            .collect();
        panels.sort();
        panels.first().map(|(_, entity)| *entity)
    });
    let Some(panel) = panel else {
        return Ok(());
    };
    let mut state = world.get_mut::<SearchState>(panel).unwrap();
    let Some(to) = state.next_match(state.at(), forward) else {
        return Ok(());
    };
    let path = state.files[to.0].path.clone();
    let found = state.files[to.0].matches[to.1.unwrap_or(0)].clone();
    state.collapsed.remove(&path);
    state.select(to);
    open_match(world, panel, &path, &found, false);
    Ok(())
}

/// Opens `path` (at `found`, if given) in the area it's dropped in.
fn open_factory(path: PathBuf, found: Option<Match>) -> PanelFactory {
    PanelFactory::new("File", move |world, area| {
        let options = PathOptions {
            line: found.as_ref().map(|found| found.start),
            select_to: found.as_ref().map(|found| found.end),
            target: Target::New,
            place: Some(bedtime_workbench::Placement::In(area)),
            ..default()
        };
        match world.workbench_mut().open_path(&path, options) {
            Ok(panel) => panel,
            Err(error) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).display();
                world.workbench_mut().notify(Notification::error(format!(
                    "Couldn't open {name}: {error}"
                )));
                Entity::PLACEHOLDER
            }
        }
    })
}

/// What a replace did.
#[derive(Clone, Debug, Default)]
struct Replaced {
    matches: usize,
    files: usize,
    problems: Vec<String>,
}

impl Replaced {
    fn add(&mut self, result: Result<usize, String>) {
        match result {
            Ok(0) => {}
            Ok(matches) => {
                self.matches += matches;
                self.files += 1;
            }
            Err(problem) => self.problems.push(problem),
        }
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .display()
        .to_string()
}

/// The document open for the file at `path`, if any.
fn document_at(world: &mut World, path: &Path) -> Option<DocumentId> {
    world
        .query::<(&ViewerFile, &DocumentReference)>()
        .iter(world)
        .find(|(file, _)| file.0 == path)
        .map(|(_, reference)| reference.document())
}

/// The edits that replace the matches at `ranges` in `text`, last first.
fn edits(
    matcher: &Matcher,
    path: &Path,
    text: &str,
    ranges: Vec<Range<usize>>,
    replacement: &str,
) -> Result<Vec<(Range<usize>, String)>, String> {
    matcher
        .replacements_at(text, ranges, replacement)
        .ok_or_else(|| format!("{} changed since the search", name(path)))
}

/// Replace the matches at `ranges` in an open document, as one undoable
/// edit. Returns how many.
fn replace_in_document(
    world: &mut World,
    document: DocumentId,
    path: &Path,
    matcher: &Matcher,
    ranges: Vec<Range<usize>>,
    replacement: &str,
) -> Result<usize, String> {
    let workbench = world.workbench();
    let handle = workbench
        .cast_document::<TextBuffer>(document)
        .map_err(|e| format!("{}: {e}", name(path)))?;
    let text = workbench
        .read_document(handle)
        .map_err(|e| e.to_string())?
        .to_string();
    let revision = workbench
        .inspect_document(document)
        .map_err(|e| e.to_string())?
        .revision;
    let edits = edits(matcher, path, &text, ranges, replacement)?;
    let count = edits.len();
    if count > 0 {
        world
            .workbench_mut()
            .edit_document_transaction(
                handle,
                revision,
                edits
                    .into_iter()
                    .map(|(range, with)| BufferEdit::new(range, with)),
                EditOptions::new("Replace"),
            )
            .map_err(|e| format!("{}: {e}", name(path)))?;
    }
    Ok(count)
}

/// Replace the matches at `ranges` in the file at `path`, if it's still as
/// it was searched. Returns how many. Plain: Replace All runs it on a worker.
fn replace_on_disk(
    path: &Path,
    matcher: &Matcher,
    ranges: Vec<Range<usize>>,
    replacement: &str,
) -> Result<usize, String> {
    let original =
        crate::engine::read_text(path).ok_or_else(|| format!("{} isn't text", name(path)))?;
    let edits = edits(matcher, path, &original, ranges, replacement)?;
    if edits.is_empty() {
        return Ok(0);
    }
    let mut edited = original.clone();
    for (range, with) in &edits {
        edited.replace_range(range.clone(), with);
    }
    bedtime_filesystem::Filesystem::local()
        .source(path)
        .prepare_write(&mut std::io::Cursor::new(edited.into_bytes()))
        .and_then(|pending| {
            pending.commit(bedtime_filesystem::WriteExpectation::Contents(
                &mut std::io::Cursor::new(original.into_bytes()),
            ))
        })
        .map_err(|e| format!("{}: {e}", name(path)))?;
    Ok(edits.len())
}

/// What the panel replaces with: its search's matcher and its replacement.
fn replacing_with(world: &World, panel: Entity) -> Option<(Matcher, String)> {
    let matcher = world.get::<SearchState>(panel)?.matcher.clone()?;
    Some((
        matcher,
        world.get::<SearchPanel>(panel)?.replacement.clone(),
    ))
}

/// Replace one match: in the file's open document if it has one, else on
/// disk.
fn replace_one(
    world: &mut World,
    panel: Entity,
    path: &Path,
    found: &Match,
) -> Result<usize, String> {
    let Some((matcher, replacement)) = replacing_with(world, panel) else {
        return Ok(0);
    };
    let ranges = vec![found.range.clone()];
    match document_at(world, path) {
        Some(document) => {
            replace_in_document(world, document, path, &matcher, ranges, &replacement)
        }
        None => replace_on_disk(path, &matcher, ranges, &replacement),
    }
}

/// Replace every match listed (not what was dismissed, nor what a search
/// that stopped early never reached): open documents now, the other files on
/// a worker.
fn replace_all(world: &mut World, panel: Entity) {
    let Some((matcher, replacement)) = replacing_with(world, panel) else {
        return;
    };
    let files: Vec<(PathBuf, Vec<Range<usize>>)> = world
        .get::<SearchState>(panel)
        .map(|state| {
            state
                .files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        f.matches.iter().map(|m| m.range.clone()).collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut done = Replaced::default();
    let mut on_disk = Vec::new();
    for (path, ranges) in files {
        match document_at(world, &path) {
            Some(document) => done.add(replace_in_document(
                world,
                document,
                &path,
                &matcher,
                ranges,
                &replacement,
            )),
            None => on_disk.push((path, ranges)),
        }
    }
    if on_disk.is_empty() {
        finish_replacing(world, panel, done, true);
        return;
    }
    let so_far = done.clone();
    let spawned = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Replacing in files",
        move |cancel| {
            let mut done = so_far;
            for (path, ranges) in on_disk {
                if cancel.is_cancelled() {
                    done.problems
                        .push("stopped before every file was changed".into());
                    break;
                }
                done.add(replace_on_disk(&path, &matcher, ranges, &replacement));
            }
            done
        },
    );
    match spawned {
        Ok(task) => {
            if let Some(mut state) = world.get_mut::<SearchState>(panel) {
                state.replacing = Some(task);
            }
        }
        Err(error) => {
            done.problems.push(error);
            finish_replacing(world, panel, done, true);
        }
    }
}

/// Take in what Replace All's workers have done.
fn collect_replacements(world: &mut World) {
    let finished: Vec<(Entity, Replaced)> = world
        .query::<(Entity, &mut SearchState)>()
        .iter_mut(world)
        .filter_map(|(panel, mut state)| {
            let done = match state.replacing.as_ref()?.try_recv() {
                Ok(done) => done,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => Replaced {
                    problems: vec!["it stopped before it was done".into()],
                    ..default()
                },
            };
            state.replacing = None;
            Some((panel, done))
        })
        .collect();
    for (panel, done) in finished {
        finish_replacing(world, panel, done, true);
    }
}

/// Search again, and say what was done (after Replace All) and what went
/// wrong.
fn finish_replacing(world: &mut World, panel: Entity, done: Replaced, all: bool) {
    if let Some(mut state) = world.get_mut::<SearchState>(panel) {
        state.rerun();
    }
    if !done.problems.is_empty() {
        // A few say enough.
        let mut text = done.problems[..done.problems.len().min(3)].join("; ");
        if done.problems.len() > 3 {
            text += &format!("; and {} more", done.problems.len() - 3);
        }
        world
            .workbench_mut()
            .notify(Notification::warning(format!("Replace: {text}")));
    }
    if all && done.matches > 0 {
        world.workbench_mut().notify(Notification::info(format!(
            "Replaced {} in {}",
            count(done.matches, "match", "matches"),
            count(done.files, "file", "files"),
        )));
    }
}

/// "1 match", "20,000 matches".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", number(n), if n == 1 { one } else { many })
}

/// `n` with its thousands apart.
fn number(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

impl Panel for SearchPanel {
    const LABEL: &'static str = "Search Workspace";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw(ui, world, context, panel);
    }
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
    // Replace All's question says how many files are changed on disk: those
    // that aren't open.
    let confirming = world
        .get::<SearchState>(panel)
        .is_some_and(|state| state.confirming);
    let open: HashSet<PathBuf> = if confirming {
        world
            .query::<&ViewerFile>()
            .iter(world)
            .map(|file| file.0.clone())
            .collect()
    } else {
        HashSet::new()
    };
    let Some(mut search) = world.entity_mut(panel).take::<SearchPanel>() else {
        return;
    };
    let Some(mut state) = world.entity_mut(panel).take::<SearchState>() else {
        world.entity_mut(panel).insert(search);
        return;
    };
    let mut intents = Vec::new();
    let id = egui::Id::new((panel, "search"));
    // Searching waits for typing to settle, and results stream in: come back.
    if state.task.is_some()
        || state.replacing.is_some()
        || state.seen.as_ref() != Some(&search.query)
        || state.changed.is_some_and(|at| at.elapsed() < SETTLE)
    {
        ui.ctx().request_repaint_after(Duration::from_millis(30));
    } else if state.outdated() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            fields(ui, context, id, &mut search, &mut state, focused);
            status(ui, context, &mut state, &open, &mut intents);
        });
    ui.separator();
    if focused && !ui.ctx().egui_wants_keyboard_input() {
        result_keys(ui, &mut state, &mut intents);
    }
    results(
        ui,
        context,
        panel,
        root.as_deref(),
        &search,
        &mut state,
        &mut intents,
    );
    // A query that was used is one to come back to.
    if intents
        .iter()
        .any(|intent| !matches!(intent, Intent::Refresh))
    {
        search.remember();
        state.recalled = None;
    }
    world.entity_mut(panel).insert((search, state));
    world
        .resource_mut::<Intents>()
        .0
        .extend(intents.into_iter().map(|intent| (panel, intent)));
}

/// The keys of the results, while the panel has the focus and no field does:
/// the arrows move the selection (Left and Right close and open a file),
/// Enter opens a match (Space too, staying in the results), Delete takes the
/// row out of the results and Escape goes back to the query.
fn result_keys(ui: &mut egui::Ui, state: &mut SearchState, intents: &mut Vec<Intent>) {
    if state.files.is_empty() {
        return;
    }
    let pressed = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
    let at = state.row();
    let last = {
        let file = state.files.len() - 1;
        let matches = state.files[file].matches.len();
        (file, state.open(file).then_some(matches - 1))
    };
    let page = state.page.max(1);
    let mut to = None;
    if pressed(egui::Key::ArrowDown) {
        to = at.map_or(Some((0, None)), |at| state.below(at));
    }
    if pressed(egui::Key::ArrowUp) {
        to = at.and_then(|at| state.above(at));
    }
    if pressed(egui::Key::PageDown) {
        let mut row = at.unwrap_or((0, None));
        for _ in 0..page {
            row = state.below(row).unwrap_or(row);
        }
        to = Some(row);
    }
    if pressed(egui::Key::PageUp) {
        let mut row = at.unwrap_or((0, None));
        for _ in 0..page {
            row = state.above(row).unwrap_or(row);
        }
        to = Some(row);
    }
    if pressed(egui::Key::Home) {
        to = Some((0, None));
    }
    if pressed(egui::Key::End) {
        to = Some(last);
    }
    if let Some(to) = to {
        state.select(to);
        return;
    }
    if pressed(egui::Key::Escape) {
        state.focus_query = true;
    }
    let Some((file, index)) = at else {
        return;
    };
    let path = state.files[file].path.clone();
    if pressed(egui::Key::ArrowRight) && index.is_none() && !state.collapsed.remove(&path) {
        state.select((file, Some(0)));
    }
    if pressed(egui::Key::ArrowLeft) {
        match index {
            Some(_) => state.select((file, None)),
            None => {
                state.collapsed.insert(path.clone());
            }
        }
    }
    let enter = pressed(egui::Key::Enter);
    if enter || pressed(egui::Key::Space) {
        match index {
            Some(index) => intents.push(Intent::Open {
                path,
                found: state.files[file].matches[index].clone(),
                keep_focus: !enter,
            }),
            None => {
                if !state.collapsed.remove(&path) {
                    state.collapsed.insert(path);
                }
            }
        }
        return;
    }
    if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
        state.dismiss((file, index));
    }
}

/// Up in the query field: the query used before the one it shows.
fn recall_older(search: &mut SearchPanel, state: &mut SearchState) {
    let at = match state.recalled {
        Some(0) => return,
        Some(at) => at - 1,
        None => {
            // From the newest that isn't what's already there.
            let Some(mut at) = search.history.len().checked_sub(1) else {
                return;
            };
            if search.history[at] == search.query.text {
                let Some(before) = at.checked_sub(1) else {
                    return;
                };
                at = before;
            }
            state.draft = search.query.text.clone();
            at
        }
    };
    state.recalled = Some(at);
    search.query.text = search.history[at].clone();
}

/// Down in the query field: the query used after the one it shows, then what
/// was typed. Returns whether there was one to go to.
fn recall_newer(search: &mut SearchPanel, state: &mut SearchState) -> bool {
    let Some(at) = state.recalled else {
        return false;
    };
    if at + 1 < search.history.len() {
        state.recalled = Some(at + 1);
        search.query.text = search.history[at + 1].clone();
    } else {
        state.recalled = None;
        search.query.text = std::mem::take(&mut state.draft);
    }
    true
}

/// An icon button's width in the bar (and on a match's row).
fn button_width(ui: &egui::Ui, context: &UiContext<'_>) -> f32 {
    context.metrics.font_size + 2.0 * ui.spacing().button_padding.x
}

/// A text field `width` wide, its text kept clear of `inset` points on the
/// right (for the toggles drawn there).
fn field(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    hint: &str,
    width: f32,
    inset: f32,
) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .desired_width(width)
            .margin(egui::Margin {
                left: 6,
                right: (6.0 + inset).min(127.0) as i8,
                top: 3,
                bottom: 3,
            }),
    )
}

/// A small text toggle ("Aa") drawn inside a field, with what it does on
/// hover.
fn option_toggle(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    on: &mut bool,
    text: &str,
    tip: &str,
) {
    let response = ui.interact(rect, ui.id().with(("option", text)), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, *on, text)
    });
    if response.clicked() {
        *on = !*on;
    }
    let palette = &context.theme.palette;
    let seconds = if context.motion.enabled {
        context.motion.seconds
    } else {
        0.0
    };
    let t = ui
        .ctx()
        .animate_bool_with_time(response.id.with("on"), *on, seconds);
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), seconds);
    let mut fill = color(palette.accent).gamma_multiply(0.22 * t);
    if hover > 0.0 {
        fill = fill.blend(
            ui.visuals()
                .widgets
                .hovered
                .weak_bg_fill
                .gamma_multiply(hover),
        );
    }
    ui.painter().rect_filled(rect, 3.0, fill);
    let ink = bedtime_ui::theme::mix(
        bedtime_ui::theme::mix(palette.muted, palette.text, hover),
        palette.accent,
        t,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::monospace(context.metrics.font_size * 0.8),
        color(ink),
    );
    context.frame(ui).tooltip(&response, tip);
}

/// The bar, as Zed's: the query with its options inside it, then Replace and
/// Filter; under it the replacement with Replace All, and the filters. The
/// fields share their edges; the buttons are a column on the right.
fn fields(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    id: egui::Id,
    search: &mut SearchPanel,
    state: &mut SearchState,
    focused: bool,
) {
    let gap = ui.spacing().item_spacing.x;
    let button = button_width(ui, context);
    let width = (ui.available_width() - 2.0 * (button + gap)).max(80.0);
    let side = (ui.spacing().interact_size.y - 2.0).max(16.0);
    let inset = 3.0 * (side + 2.0) + 2.0;
    // Up and Down in the query go through earlier queries; Down past the
    // newest goes to the results.
    let query_id = id.with("query");
    if ui.memory(|memory| memory.has_focus(query_id)) {
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        if up {
            recall_older(search, state);
        }
        if down && !recall_newer(search, state) && !state.files.is_empty() {
            ui.memory_mut(|memory| memory.surrender_focus(query_id));
            match state.row() {
                Some(_) => state.reveal = true,
                None => state.select((0, None)),
            }
        }
    }
    ui.horizontal(|ui| {
        let query = field(ui, query_id, &mut search.query.text, "Search", width, inset);
        if state.focus_query && focused {
            query.request_focus();
            state.focus_query = false;
        }
        if query.changed() {
            state.recalled = None;
        }
        if query.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            state.rerun();
            search.remember();
            state.recalled = None;
        }
        // The options, inside the field's right end; after it, so they take
        // the clicks.
        let options = [
            (&mut search.query.match_case, "Aa", "Match Case"),
            (&mut search.query.whole_word, "ab", "Match Whole Word"),
            (&mut search.query.regex, ".*", "Use Regular Expression"),
        ];
        let side = side.min(query.rect.height() - 4.0);
        let mut right = query.rect.right() - 3.0;
        for (on, text, tip) in options.into_iter().rev() {
            let rect = egui::Rect::from_min_max(
                egui::pos2(right - side, query.rect.center().y - side / 2.0),
                egui::pos2(right, query.rect.center().y + side / 2.0),
            );
            option_toggle(ui, context, rect, on, text, tip);
            right -= side + 2.0;
        }
        let mut frame = context.frame(ui);
        if frame
            .floating_toggle(BuiltinIcon::Replace, "Replace", search.replacing)
            .clicked()
        {
            search.replacing = !search.replacing;
            state.confirming = false;
        }
        if frame
            .floating_toggle(
                BuiltinIcon::Filter,
                "Include and Exclude Files",
                search.filtering,
            )
            .clicked()
        {
            search.filtering = !search.filtering;
        }
    });
    if search.replacing {
        ui.horizontal(|ui| {
            let replacement = field(
                ui,
                id.with("replacement"),
                &mut search.replacement,
                "Replace with",
                width,
                0.0,
            );
            if replacement.changed() {
                state.confirming = false;
            }
            let ready =
                !state.files.is_empty() && state.task.is_none() && state.replacing.is_none();
            ui.add_enabled_ui(ready, |ui| {
                if context
                    .frame(ui)
                    .floating_button(BuiltinIcon::ReplaceAll, "Replace All")
                    .clicked()
                {
                    // Asked first: see `status`.
                    state.confirming = true;
                }
            });
        });
    }
    if search.filtering {
        field(
            ui,
            id.with("include"),
            &mut search.query.include,
            "Include: *.rs, src/",
            width,
            0.0,
        );
        field(
            ui,
            id.with("exclude"),
            &mut search.query.exclude,
            "Exclude",
            width,
            0.0,
        );
        let mut frame = context.frame(ui);
        frame.toggle(&mut search.query.ignored, "Files Git ignores");
        frame.toggle(&mut search.query.open_only, "Only open files");
    }
}

/// Under the fields: what went wrong, else Replace All's question, else how
/// many results there are.
fn status(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    state: &mut SearchState,
    open: &HashSet<PathBuf>,
    intents: &mut Vec<Intent>,
) {
    let palette = &context.theme.palette;
    if let Some(error) = &state.error {
        ui.colored_label(color(palette.error), error);
        return;
    }
    if state.replacing.is_some() {
        ui.horizontal(|ui| {
            ui.weak("Replacing…");
            ui.spinner();
        });
        return;
    }
    if state.confirming && state.files.is_empty() {
        state.confirming = false;
    }
    if state.confirming {
        ui.label(format!(
            "Replace {} in {}?",
            count(state.matches, "match", "matches"),
            count(state.files.len(), "file", "files"),
        ));
        let on_disk = state
            .files
            .iter()
            .filter(|file| !open.contains(&file.path))
            .count();
        if on_disk > 0 {
            ui.weak(format!(
                "{} changed on disk, which can't be undone.",
                if on_disk == 1 {
                    "1 file isn't open: it's".to_owned()
                } else {
                    format!("{} files aren't open: they're", number(on_disk))
                }
            ));
        }
        ui.horizontal(|ui| {
            // Not while the results are being searched for again.
            let ready = state.task.is_none();
            let replace = ui.add_enabled_ui(ready, |ui| {
                context.frame(ui).primary_button("Replace All").clicked()
            });
            if replace.inner {
                intents.push(Intent::ReplaceAll);
                state.confirming = false;
            }
            if ui.button("Cancel").clicked() {
                state.confirming = false;
            }
        });
        return;
    }
    ui.horizontal(|ui| {
        let limited = state.summary.as_ref().is_some_and(|s| s.limited);
        let text = if state.shown.is_none() {
            String::new()
        } else if state.matches == 0 && state.task.is_none() {
            "No results".into()
        } else {
            format!(
                "{}{} in {}",
                if limited { "First " } else { "" },
                count(state.matches, "result", "results"),
                count(state.files.len(), "file", "files"),
            )
        };
        let label = ui.weak(text);
        if limited {
            context.frame(ui).tooltip(
                &label,
                format!(
                    "The search stopped at {} results. Narrow it to see the rest.",
                    number(MAX_MATCHES)
                ),
            );
        }
        let skipped = state.summary.as_ref().map_or(0, |s| s.skipped);
        if skipped > 0 {
            let label = ui.weak(format!("· {} skipped", number(skipped)));
            context.frame(ui).tooltip(
                &label,
                "Files over 16 MiB, and files that aren't UTF-8, aren't searched.",
            );
        }
        // Not for searching again what's shown: that's as quiet as it can be.
        if state.task.is_some() && state.fresh.is_none() {
            ui.spinner();
        }
    });
}

/// What a row (or its menu) was asked.
enum Ask {
    /// Open or close a file.
    Toggle(usize),
    Open(usize, usize),
    Replace(usize, usize),
    Dismiss(At),
    Copy(String),
    CollapseAll,
    ExpandAll,
    Refresh,
}

/// The menu items every row and the space under them share.
fn shared_menu(ui: &mut egui::Ui, ask: &mut Option<Ask>) {
    if ui.button("Collapse All").clicked() {
        *ask = Some(Ask::CollapseAll);
    }
    if ui.button("Expand All").clicked() {
        *ask = Some(Ask::ExpandAll);
    }
    if ui.button("Search Again").clicked() {
        *ask = Some(Ask::Refresh);
    }
}

/// The results as a tree, drawn as Structure draws its outline: a file's
/// matches slide in and out under it as it opens and closes.
fn results(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    root: Option<&Path>,
    search: &SearchPanel,
    state: &mut SearchState,
    intents: &mut Vec<Intent>,
) {
    let height = tree::row_height(context);
    // Where each file's row is, and how open it is: its matches follow it,
    // each as tall as that. Only the rows in view are drawn.
    let files = state.files.len();
    let mut tops = Vec::with_capacity(files + 1);
    let mut openness = Vec::with_capacity(files);
    let mut top = 0.0;
    for found in &state.files {
        let open = bedtime_ui::animate_expansion(
            ui.ctx(),
            egui::Id::new((panel, "search-open", &found.path)),
            !state.collapsed.contains(&found.path),
            context.motion,
        );
        tops.push(top);
        openness.push(open);
        top += height * (1.0 + open * found.matches.len() as f32);
    }
    tops.push(top);
    // Where the selection is to be scrolled to: where its row will be once
    // its file is open.
    let reveal = std::mem::take(&mut state.reveal)
        .then(|| state.row())
        .flatten()
        .map(|(file, index)| tops[file] + height * index.map_or(0.0, |index| 1.0 + index as f32));
    let mut ask = None;
    let mut page = state.page;
    // Under the rows (which take clicks first): the menu, a right-click away.
    let background = ui.interact(
        ui.available_rect_before_wrap(),
        egui::Id::new((panel, "search-background")),
        egui::Sense::click(),
    );
    background.context_menu(|ui| shared_menu(ui, &mut ask));
    egui::ScrollArea::vertical()
        .id_salt((panel, "search-results"))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min;
            ui.set_min_size(egui::vec2(viewport.width(), top));
            page = ((viewport.height() / height) as usize)
                .saturating_sub(1)
                .max(1);
            if let Some(y) = reveal {
                ui.scroll_to_rect(
                    egui::Rect::from_min_size(
                        origin + egui::vec2(0.0, y),
                        egui::vec2(viewport.width(), height),
                    ),
                    None,
                );
            }
            // A row `shown` tall at `y`: drawn whole, cut to what shows of it
            // (a closing file's matches).
            let row = |ui: &mut egui::Ui, y: f32, shown: f32| {
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, y),
                    egui::vec2(viewport.width(), height),
                );
                let visible = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), shown));
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                ui.set_clip_rect(ui.clip_rect().intersect(visible));
                (ui, rect)
            };
            let first = tops[1..].partition_point(|bottom| *bottom < viewport.top());
            for file in first..files {
                let y = tops[file];
                if y > viewport.bottom() {
                    break;
                }
                let (mut row_ui, rect) = row(ui, y, height);
                file_row(
                    &mut row_ui,
                    context,
                    panel,
                    root,
                    state,
                    file,
                    rect,
                    &mut ask,
                );
                let shown = height * openness[file];
                if shown <= 0.0 {
                    continue;
                }
                let base = y + height;
                let from = ((viewport.top() - base) / shown).floor().max(0.0) as usize;
                for index in from..state.files[file].matches.len() {
                    let y = base + index as f32 * shown;
                    if y > viewport.bottom() {
                        break;
                    }
                    let (mut row_ui, rect) = row(ui, y, shown);
                    match_row(
                        &mut row_ui,
                        context,
                        panel,
                        root,
                        search,
                        state,
                        file,
                        index,
                        rect,
                        &mut ask,
                    );
                }
            }
        });
    state.page = page;
    match ask {
        None => {}
        Some(Ask::Toggle(file)) => {
            let path = state.files[file].path.clone();
            if !state.collapsed.remove(&path) {
                state.collapsed.insert(path.clone());
            }
            state.selected = Some(Selected::File(path));
        }
        Some(Ask::Open(file, index)) => {
            let found = state.files[file].matches[index].clone();
            let path = state.files[file].path.clone();
            state.selected = Some(Selected::Match(path.clone(), found.range.start));
            intents.push(Intent::Open {
                path,
                found,
                keep_focus: false,
            });
        }
        Some(Ask::Replace(file, index)) => intents.push(Intent::Replace(
            state.files[file].path.clone(),
            state.files[file].matches[index].clone(),
        )),
        Some(Ask::Dismiss(at)) => state.dismiss(at),
        Some(Ask::Copy(text)) => ui.ctx().copy_text(text),
        Some(Ask::CollapseAll) => {
            state.collapsed = state.files.iter().map(|f| f.path.clone()).collect();
        }
        Some(Ask::ExpandAll) => state.collapsed.clear(),
        Some(Ask::Refresh) => intents.push(Intent::Refresh),
    }
}

/// Icon buttons over a row's right end, the first rightmost, without moving
/// anything. Returns which was clicked.
fn row_buttons(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    rect: egui::Rect,
    buttons: &[(BuiltinIcon, &str)],
) -> Option<usize> {
    let width = button_width(ui, context);
    let mut clicked = None;
    for (at, (icon, tip)) in buttons.iter().enumerate() {
        let center = rect.right() - 8.0 - width / 2.0 - at as f32 * (width + 2.0);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_center_size(
                    egui::pos2(center, rect.center().y),
                    egui::vec2(width, rect.height()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if context
            .frame(&mut child)
            .floating_button(*icon, tip)
            .clicked()
        {
            clicked = Some(at);
        }
    }
    clicked
}

/// The room `row_buttons` takes at a row's right end.
fn row_buttons_width(ui: &egui::Ui, context: &UiContext<'_>, buttons: usize) -> f32 {
    buttons as f32 * (button_width(ui, context) + 2.0) + 4.0
}

/// `folder`, with its start left out ("…/crates/search/src") if it's wider
/// than `width`: its end says more.
fn folder_galley(
    ui: &egui::Ui,
    folder: &Path,
    font: egui::FontId,
    ink: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let whole = ui
        .painter()
        .layout_no_wrap(folder.display().to_string(), font.clone(), ink);
    if whole.size().x <= width {
        return whole;
    }
    let parts: Vec<_> = folder.components().collect();
    let shortened = |skip: usize| {
        let rest: PathBuf = parts[skip..].iter().collect();
        format!("…/{}", rest.display())
    };
    for skip in 1..parts.len() {
        let galley = ui
            .painter()
            .layout_no_wrap(shortened(skip), font.clone(), ink);
        if galley.size().x <= width {
            return galley;
        }
    }
    // Even its last part is too wide: cut that.
    let mut job = egui::text::LayoutJob::simple_singleline(
        shortened(parts.len().saturating_sub(1)),
        font,
        ink,
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.fonts_mut(|f| f.layout_job(job))
}

/// `path` as it's shown: from the folder searched, if it's in it.
fn relative<'a>(root: Option<&Path>, path: &'a Path) -> &'a Path {
    root.and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
}

/// A file's row: its name, folder and count of matches; while it's hovered,
/// Dismiss in the count's place. A click on its name opens or closes it.
#[allow(clippy::too_many_arguments)]
fn file_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    root: Option<&Path>,
    state: &SearchState,
    file: usize,
    rect: egui::Rect,
    ask: &mut Option<Ask>,
) {
    let palette = &context.theme.palette;
    let found = &state.files[file];
    let name = name(&found.path);
    let row = tree::Row {
        label: Some(&name),
        depth: 0,
        icon: Some(bedtime_file_explorer::file_icon(&found.path)),
        expanded: Some(!state.collapsed.contains(&found.path)),
        selected: matches!(&state.selected, Some(Selected::File(path)) if *path == found.path),
        ink: color(palette.text),
        icon_ink: palette.text,
    };
    let item = tree::item_rect(ui, context, &row, rect, tree::INDENT);
    let response = ui.interact(
        item,
        egui::Id::new((panel, "file", &found.path)),
        egui::Sense::click_and_drag(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    tree::paint_row(ui, context, row, rect, Some(&response), 1.0, tree::INDENT);
    // Where it is, and how many, after the name.
    let relative = relative(root, &found.path);
    let hovered = ui.rect_contains_pointer(rect);
    let font = egui::FontId::proportional(context.metrics.font_size * 0.85);
    let count = ui.painter().layout_no_wrap(
        number(found.matches.len()),
        font.clone(),
        color(palette.muted),
    );
    let right = if hovered {
        rect.right() - row_buttons_width(ui, context, 1)
    } else {
        rect.right() - 10.0 - count.size().x
    };
    let folder_left = item.right() + 6.0;
    if let Some(folder) = relative.parent().filter(|p| !p.as_os_str().is_empty())
        && folder_left < right - 20.0
    {
        let galley = folder_galley(
            ui,
            folder,
            font,
            color(palette.muted),
            right - 10.0 - folder_left,
        );
        ui.painter().galley(
            egui::pos2(folder_left, rect.center().y - galley.size().y / 2.0),
            galley,
            color(palette.muted),
        );
    }
    if hovered {
        if row_buttons(ui, context, rect, &[(BuiltinIcon::Close, "Dismiss")]).is_some() {
            *ask = Some(Ask::Dismiss((file, None)));
        }
    } else {
        ui.painter().galley(
            egui::pos2(right, rect.center().y - count.size().y / 2.0),
            count,
            color(palette.muted),
        );
    }
    // Drag it onto the layout to open it there.
    if response.drag_started() {
        response.dnd_set_drag_payload(PanelFactoryPayload::new(open_factory(
            found.path.clone(),
            None,
        )));
    }
    response.context_menu(|ui| {
        if ui.button("Copy Path").clicked() {
            *ask = Some(Ask::Copy(found.path.display().to_string()));
        }
        if ui.button("Copy Relative Path").clicked() {
            *ask = Some(Ask::Copy(relative.display().to_string()));
        }
        if ui.button("Dismiss").clicked() {
            *ask = Some(Ask::Dismiss((file, None)));
        }
        ui.separator();
        shared_menu(ui, ask);
    });
    tree::tooltip(ui, context, &response, relative.display().to_string());
    if response.clicked() {
        *ask = Some(Ask::Toggle(file));
    }
}

/// A match's row: its line number and line, the match marked (with the
/// replace row shown: struck out, what replaces it after it). While it's
/// hovered, Dismiss and (with the replace row) Replace on the right. A click
/// opens it.
#[allow(clippy::too_many_arguments)]
fn match_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    root: Option<&Path>,
    search: &SearchPanel,
    state: &SearchState,
    file: usize,
    index: usize,
    rect: egui::Rect,
    ask: &mut Option<Ask>,
) {
    let palette = &context.theme.palette;
    let found = &state.files[file];
    let m = &found.matches[index];
    // The buttons sit over the row's end, without moving anything.
    let hovered = ui.rect_contains_pointer(rect);
    let replaces = search.replacing && state.task.is_none() && state.replacing.is_none();
    let buttons: &[(BuiltinIcon, &str)] = match (hovered, replaces) {
        (false, _) => &[],
        (true, false) => &[(BuiltinIcon::Close, "Dismiss")],
        (true, true) => &[
            (BuiltinIcon::Close, "Dismiss"),
            (BuiltinIcon::Replace, "Replace"),
        ],
    };
    let right = rect.right() - 8.0 - row_buttons_width(ui, context, buttons.len());
    let x = rect.left() + tree::INSET + tree::INDENT + 18.0;
    let mono = egui::FontId::monospace(context.metrics.font_size * 0.9);
    let number = ui.painter().layout_no_wrap(
        format!("{}", m.start.0 + 1),
        mono.clone(),
        color(palette.muted),
    );
    let number_width = number.size().x.max(context.metrics.font_size * 2.0);
    // The line, its match marked.
    let text_x = x + number_width + 10.0;
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat {
        font_id: mono,
        color: color(palette.text),
        ..default()
    };
    let span = m.preview_range.clone();
    job.append(&m.preview[..span.start], 0.0, plain.clone());
    if search.replacing {
        // What's there struck out, and what would replace it.
        let old = egui::TextFormat {
            background: color(palette.error).gamma_multiply(0.3),
            strikethrough: egui::Stroke::new(1.0, color(palette.text)),
            ..plain.clone()
        };
        job.append(&m.preview[span.clone()], 0.0, old);
        let with = state
            .matcher
            .as_ref()
            .and_then(|matcher| matcher.preview_replacement(m, &search.replacement))
            .unwrap_or_else(|| search.replacement.clone());
        if !with.is_empty() {
            let new = egui::TextFormat {
                background: color(palette.success).gamma_multiply(0.3),
                ..plain.clone()
            };
            job.append(&with.replace('\n', "⏎"), 0.0, new);
        }
    } else {
        let marked = egui::TextFormat {
            background: ui.visuals().selection.bg_fill,
            ..plain.clone()
        };
        job.append(&m.preview[span.clone()], 0.0, marked);
    }
    job.append(&m.preview[span.end..], 0.0, plain.clone());
    // A match over several lines shows its first.
    let more = m.end.0 - m.start.0;
    if more > 0 {
        let muted = egui::TextFormat {
            color: color(palette.muted),
            ..plain
        };
        job.append(
            &format!("  +{}", count(more as usize, "line", "lines")),
            0.0,
            muted,
        );
    }
    job.wrap.max_width = (right - text_x).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    // Only the number and line take the pointer, as a tree's item does.
    let item = egui::Rect::from_min_max(
        egui::pos2(x - 4.0, rect.top()),
        egui::pos2(text_x + galley.size().x + 4.0, rect.bottom()),
    );
    let response = ui.interact(
        item,
        egui::Id::new((panel, "match", &found.path, m.range.start)),
        egui::Sense::click_and_drag(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &m.preview));
    let selected = matches!(
        &state.selected,
        Some(Selected::Match(path, start)) if *path == found.path && *start == m.range.start
    );
    tree::paint_highlight(
        ui,
        &ui.painter().clone(),
        context,
        item.shrink2(egui::vec2(0.0, 1.0)),
        &response,
        selected,
    );
    ui.painter().galley(
        egui::pos2(
            x + number_width - number.size().x,
            rect.center().y - number.size().y / 2.0,
        ),
        number,
        color(palette.muted),
    );
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y - galley.size().y / 2.0),
        galley,
        color(palette.text),
    );
    if response.drag_started() {
        response.dnd_set_drag_payload(PanelFactoryPayload::new(open_factory(
            found.path.clone(),
            Some(m.clone()),
        )));
    }
    response.context_menu(|ui| {
        if ui.button("Copy Line").clicked() {
            *ask = Some(Ask::Copy(m.preview.clone()));
        }
        if ui.button("Copy Path").clicked() {
            *ask = Some(Ask::Copy(found.path.display().to_string()));
        }
        if ui.button("Copy Relative Path").clicked() {
            *ask = Some(Ask::Copy(relative(root, &found.path).display().to_string()));
        }
        if ui.button("Dismiss").clicked() {
            *ask = Some(Ask::Dismiss((file, Some(index))));
        }
        ui.separator();
        shared_menu(ui, ask);
    });
    match row_buttons(ui, context, rect, buttons) {
        Some(0) => *ask = Some(Ask::Dismiss((file, Some(index)))),
        Some(_) => *ask = Some(Ask::Replace(file, index)),
        None => {}
    }
    if response.clicked() {
        *ask = Some(Ask::Open(file, index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Results: each file with matches starting at the given bytes.
    fn state(files: &[(&str, &[usize])]) -> SearchState {
        let mut state = SearchState::default();
        for (path, starts) in files {
            state.insert(FileMatches {
                path: PathBuf::from(path),
                matches: starts
                    .iter()
                    .map(|&start| Match {
                        range: start..start + 1,
                        start: (0, start as u32),
                        end: (0, start as u32 + 1),
                        preview: "x".into(),
                        preview_range: 0..1,
                    })
                    .collect(),
            });
        }
        state
    }

    #[test]
    fn files_stay_in_path_order_however_they_arrive() {
        let state = state(&[("b/z", &[1]), ("a", &[1, 5]), ("b/c", &[2])]);
        let paths: Vec<_> = state.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, ["a", "b/c", "b/z"].map(PathBuf::from));
        assert_eq!(state.matches, 4);
    }

    #[test]
    fn the_arrows_walk_the_rows_that_show() {
        let mut state = state(&[("a", &[1, 5]), ("b", &[2])]);
        let mut walked = vec![(0, None)];
        while let Some(next) = state.below(*walked.last().unwrap()) {
            walked.push(next);
        }
        assert_eq!(
            walked,
            [
                (0, None),
                (0, Some(0)),
                (0, Some(1)),
                (1, None),
                (1, Some(0))
            ]
        );
        let mut back = vec![*walked.last().unwrap()];
        while let Some(previous) = state.above(*back.last().unwrap()) {
            back.push(previous);
        }
        back.reverse();
        assert_eq!(back, walked);
        // A closed file's matches are stepped over, both ways.
        state.collapsed.insert("a".into());
        assert_eq!(state.below((0, None)), Some((1, None)));
        assert_eq!(state.above((1, None)), Some((0, None)));
        // A match selected in it is its file's row.
        state.select((0, Some(1)));
        assert_eq!(state.at(), Some((0, Some(1))));
        assert_eq!(state.row(), Some((0, None)));
    }

    #[test]
    fn next_and_previous_result_go_round() {
        let state = state(&[("a", &[1, 5]), ("b", &[2])]);
        assert_eq!(state.next_match(None, true), Some((0, Some(0))));
        assert_eq!(state.next_match(Some((0, None)), true), Some((0, Some(0))));
        assert_eq!(
            state.next_match(Some((0, Some(1))), true),
            Some((1, Some(0)))
        );
        assert_eq!(
            state.next_match(Some((1, Some(0))), true),
            Some((0, Some(0)))
        );
        assert_eq!(state.next_match(None, false), Some((1, Some(0))));
        assert_eq!(
            state.next_match(Some((0, Some(0))), false),
            Some((1, Some(0)))
        );
        assert_eq!(state.next_match(Some((1, None)), false), Some((0, Some(1))));
        assert_eq!(SearchState::default().next_match(None, true), None);
    }

    #[test]
    fn dismissed_rows_leave_and_stay_away_when_searched_again() {
        let mut state = state(&[("a", &[1, 5, 9]), ("b", &[2]), ("c", &[3])]);
        // The selected match: the next takes its place.
        state.select((0, Some(1)));
        state.dismiss((0, Some(1)));
        assert_eq!(state.matches, 4);
        assert_eq!(state.selected, Some(Selected::Match("a".into(), 9)));
        // Another row of the file: the selection stays on its match.
        state.dismiss((0, Some(0)));
        assert_eq!(state.selected, Some(Selected::Match("a".into(), 9)));
        // A file's last match takes the file with it.
        state.dismiss((0, Some(0)));
        assert_eq!(state.files.len(), 2);
        assert_eq!(state.selected, Some(Selected::File("b".into())));
        // A whole file; the selection elsewhere is left alone.
        state.select((0, None));
        state.dismiss((1, None));
        assert_eq!(state.selected, Some(Selected::File("b".into())));
        assert_eq!((state.files.len(), state.matches), (1, 1));

        // Searched again: what was dismissed doesn't come back, what's new does.
        let fresh = super::tests::state(&[("a", &[1, 5, 9, 12]), ("b", &[2]), ("c", &[3])]).files;
        state.show(fresh);
        let shown: Vec<_> = state
            .files
            .iter()
            .map(|f| {
                let starts: Vec<_> = f.matches.iter().map(|m| m.range.start).collect();
                (f.path.clone(), starts)
            })
            .collect();
        assert_eq!(
            shown,
            [
                (PathBuf::from("a"), vec![12]),
                (PathBuf::from("b"), vec![2])
            ]
        );
        assert_eq!(state.matches, 2);
    }

    #[test]
    fn a_replaced_match_leaves_the_results() {
        let mut state = state(&[("a", &[1, 5]), ("b", &[2])]);
        state.remove_match(Path::new("a"), 5);
        state.remove_match(Path::new("b"), 2);
        state.remove_match(Path::new("b"), 2);
        assert_eq!((state.files.len(), state.matches), (1, 1));
    }

    #[test]
    fn up_and_down_go_through_earlier_queries() {
        let mut search = SearchPanel::default();
        let mut state = SearchState::default();
        for text in ["one", "two", "one", "three"] {
            search.query.text = text.into();
            search.remember();
        }
        // Used again moves to the end, not twice in the list.
        assert_eq!(search.history, ["two", "one", "three"]);
        search.query.text = "typed".into();
        recall_older(&mut search, &mut state);
        assert_eq!(search.query.text, "three");
        recall_older(&mut search, &mut state);
        recall_older(&mut search, &mut state);
        recall_older(&mut search, &mut state);
        assert_eq!(search.query.text, "two");
        assert!(recall_newer(&mut search, &mut state));
        assert_eq!(search.query.text, "one");
        assert!(recall_newer(&mut search, &mut state));
        assert!(recall_newer(&mut search, &mut state));
        // Past the newest: what was typed, and Down is free for the results.
        assert_eq!(search.query.text, "typed");
        assert!(!recall_newer(&mut search, &mut state));
        // From the newest itself, Up skips to the one before.
        search.query.text = "three".into();
        recall_older(&mut search, &mut state);
        assert_eq!(search.query.text, "one");
        for n in 0..HISTORY + 5 {
            search.query.text = n.to_string();
            search.remember();
        }
        assert_eq!(search.history.len(), HISTORY);
    }

    /// `history` and `open_only` came later: a panel saved without them
    /// restores, with them empty and off.
    #[test]
    fn a_panel_saved_before_its_newer_fields_still_restores() {
        use bevy::reflect::{FromReflect, TypeRegistry, serde::TypedReflectDeserializer};
        use serde::de::DeserializeSeed;
        let mut registry = TypeRegistry::default();
        registry.register::<SearchPanel>();
        registry.register::<SearchQuery>();
        let saved = r#"(
            query: (
                text: "old", match_case: true, whole_word: false, regex: false,
                include: "*.rs", exclude: "", ignored: false,
            ),
            replacement: "new", replacing: true, filtering: false,
        )"#;
        let registration = registry.get(std::any::TypeId::of::<SearchPanel>()).unwrap();
        let mut deserializer = ron::de::Deserializer::from_str(saved).unwrap();
        let value = TypedReflectDeserializer::new(registration, &registry)
            .deserialize(&mut deserializer)
            .unwrap();
        let panel = SearchPanel::from_reflect(value.as_partial_reflect()).unwrap();
        assert_eq!(panel.query.text, "old");
        assert!(panel.query.match_case && !panel.query.open_only);
        assert_eq!(panel.query.include, "*.rs");
        assert!(panel.replacing && panel.history.is_empty());
    }

    #[test]
    fn numbers_have_their_thousands_apart() {
        assert_eq!(number(5), "5");
        assert_eq!(number(1234), "1,234");
        assert_eq!(number(20_000), "20,000");
        assert_eq!(number(1_234_567), "1,234,567");
        assert_eq!(count(1, "match", "matches"), "1 match");
    }
}
