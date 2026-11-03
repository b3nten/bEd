//! The search panel: a query with Match Case, Whole Word and Regular
//! Expression toggles, optional include/exclude filters, and the matches in
//! the workbench's `Root` grouped by file. A click opens the file with the
//! match selected. With the replace row shown, Replace changes one match and
//! Replace All every one: open documents as an undoable edit (left unsaved),
//! other files on disk if they haven't changed since they were read.
use crate::engine::{FileMatches, Match, Matcher, SearchQuery, Summary};
use bedtime_document::{BufferEdit, TextBuffer};
use bedtime_ui::{UiContext, egui, theme::color};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Action, ActionAppExt, Area, AreaFocusedAt, DocumentReference, EditOptions, InArea, LayoutAreas,
    LayoutsWorldExt, ManagedTask, ManagedWork, Notification, Panel, PanelInstance,
    PanelPresentation, PathOptions, Root, SplitAxis, Target, ViewerFile, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SearchPanel>()
            .register_type::<SearchQuery>()
            .register_action::<FindInFiles>()
            .init_resource::<Intents>()
            .add_systems(
                Update,
                (apply_intents, start_searches, collect_results)
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
pub struct FindInFiles;
impl Action for FindInFiles {
    const KEY: &'static str = "search.find_in_files";
    const LABEL: &'static str = "Find in Files";
    const DEFAULT_KEYS: &'static [bedtime_workbench::KeyChord] =
        &[bedtime_workbench::KeyChord::new(
            bedtime_workbench::Modifiers::PRIMARY.union(bedtime_workbench::Modifiers::SHIFT),
            KeyCode::KeyF,
        )];
}

/// The search panel. Its query, replacement and which rows show are saved
/// with it.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>(), SearchState)]
pub struct SearchPanel {
    pub query: SearchQuery,
    pub replacement: String,
    /// The replace row shows.
    pub replacing: bool,
    /// The include/exclude row shows.
    pub filtering: bool,
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

/// The search's results and work. Not saved.
#[derive(Component, Default)]
pub struct SearchState {
    /// The query last seen, and when it last changed: searching waits for
    /// typing to pause.
    seen: Option<SearchQuery>,
    changed: Option<Instant>,
    /// What the results are for.
    ran: Option<SearchQuery>,
    matcher: Option<Matcher>,
    task: Option<ManagedTask<Summary>>,
    incoming: Option<std::sync::Mutex<mpsc::Receiver<FileMatches>>>,
    files: Vec<FileMatches>,
    summary: Option<Summary>,
    error: Option<String>,
    collapsed: HashSet<PathBuf>,
    focus_query: bool,
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
}

/// Typing settles for this long before a search starts.
const SETTLE: Duration = Duration::from_millis(150);

/// What the user asked for while the panel drew.
#[derive(Debug)]
enum Intent {
    Open(PathBuf, Match),
    Replace(PathBuf, Match),
    ReplaceAll,
    Refresh,
}

#[derive(Resource, Default)]
struct Intents(Vec<(Entity, Intent)>);

/// Start searches whose query has settled.
fn start_searches(world: &mut World) {
    let now = Instant::now();
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<SearchPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let query = world.get::<SearchPanel>(panel).unwrap().query.clone();
        let mut state = world.get_mut::<SearchState>(panel).unwrap();
        if state.seen.as_ref() != Some(&query) {
            state.seen = Some(query.clone());
            state.changed = Some(now);
        }
        let settled = state.changed.is_none_or(|at| now - at >= SETTLE);
        if state.ran.as_ref() == Some(&query) || !settled {
            continue;
        }
        start(world, panel, query, root.clone());
    }
}

fn start(world: &mut World, panel: Entity, query: SearchQuery, root: Option<PathBuf>) {
    let open = open_texts(world);
    let mut state = world.get_mut::<SearchState>(panel).unwrap();
    if let Some(task) = state.task.take() {
        task.cancel();
    }
    state.incoming = None;
    state.files.clear();
    state.summary = None;
    state.error = None;
    state.ran = Some(query.clone());
    state.matcher = None;
    let matcher = match query.matcher() {
        None => return,
        Some(Err(error)) => {
            state.error = Some(error);
            return;
        }
        Some(Ok(matcher)) => matcher,
    };
    let Some(root) = root else {
        state.error = Some("No folder is open".into());
        return;
    };
    state.matcher = Some(matcher.clone());
    let (sender, receiver) = mpsc::channel();
    let spawned = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        format!("Searching for {}", query.text),
        move |cancel| {
            matcher.search(&root, &open, &|| cancel.is_cancelled(), &mut |found| {
                let _ = sender.send(found);
            })
        },
    );
    let mut state = world.get_mut::<SearchState>(panel).unwrap();
    match spawned {
        Ok(task) => {
            state.task = Some(task);
            state.incoming = Some(std::sync::Mutex::new(receiver));
        }
        Err(error) => state.error = Some(error),
    }
}

/// The unsaved text of open documents, by file: searched instead of disk.
fn open_texts(world: &mut World) -> HashMap<PathBuf, Arc<str>> {
    let documents: Vec<(PathBuf, bedtime_workbench::DocumentId)> = world
        .query::<(&ViewerFile, &DocumentReference)>()
        .iter(world)
        .map(|(file, reference)| (file.0.clone(), reference.document()))
        .collect();
    let workbench = world.workbench();
    documents
        .into_iter()
        .filter(|(_, id)| workbench.inspect_document(*id).is_ok_and(|info| info.dirty))
        .filter_map(|(path, id)| {
            let handle = workbench.cast_document::<TextBuffer>(id).ok()?;
            let text = workbench.read_document(handle).ok()?;
            Some((path, Arc::from(text.to_string())))
        })
        .collect()
}

/// Take in what running searches have found.
fn collect_results(mut states: Query<&mut SearchState>) {
    for mut state in &mut states {
        let state = &mut *state;
        if let Some(incoming) = &state.incoming {
            state.files.extend(incoming.lock().unwrap().try_iter());
        }
        let Some(task) = &state.task else {
            continue;
        };
        let result = match task.try_recv() {
            Ok(summary) => Ok(summary),
            Err(mpsc::TryRecvError::Empty) => continue,
            Err(mpsc::TryRecvError::Disconnected) => Err("The search stopped".to_owned()),
        };
        if let Some(incoming) = state.incoming.take() {
            state
                .files
                .extend(incoming.into_inner().unwrap().try_iter());
        }
        state.task = None;
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
            Intent::Open(path, found) => {
                let target = match beside(world, panel, &path) {
                    Ok(target) => target,
                    Err(error) => {
                        world.workbench_mut().notify(Notification::error(error));
                        continue;
                    }
                };
                let options = PathOptions {
                    line: Some(found.start),
                    select_to: Some(found.end),
                    target,
                    preview: true,
                    ..default()
                };
                if let Err(error) = world.workbench_mut().open_path(&path, options) {
                    let name = path.file_name().unwrap_or(path.as_os_str()).display();
                    world.workbench_mut().notify(Notification::error(format!(
                        "Couldn't open {name}: {error}"
                    )));
                }
            }
            Intent::Replace(path, found) => {
                let result = replace(world, panel, &path, Some(found));
                finish_replacing(world, panel, result.map(|count| (count, 1)));
            }
            Intent::ReplaceAll => {
                let paths: Vec<PathBuf> = world
                    .get::<SearchState>(panel)
                    .map(|state| state.files.iter().map(|f| f.path.clone()).collect())
                    .unwrap_or_default();
                let mut problems = Vec::new();
                let (mut count, mut files) = (0, 0);
                for path in paths {
                    match replace(world, panel, &path, None) {
                        Ok(0) => {}
                        Ok(n) => {
                            count += n;
                            files += 1;
                        }
                        Err(error) => problems.push(error),
                    }
                }
                let result = if problems.is_empty() {
                    Ok((count, files))
                } else {
                    Err(problems.join("; "))
                };
                finish_replacing(world, panel, result);
                if count > 0 {
                    world.workbench_mut().notify(Notification::info(format!(
                        "Replaced {count} {} in {files} {}",
                        if count == 1 { "match" } else { "matches" },
                        if files == 1 { "file" } else { "files" },
                    )));
                }
            }
            Intent::Refresh => {
                if let Some(mut state) = world.get_mut::<SearchState>(panel) {
                    state.rerun();
                }
            }
        }
    }
}

/// Where a result opens: the panel already showing the file, else another
/// area showing documents (the most recently focused), so the results stay
/// in view. Without one, the search's area is split and the file opens on
/// the right (not in a file tree's area, say).
fn beside(world: &mut World, panel: Entity, path: &Path) -> Result<Target, String> {
    let layouts = world.layouts();
    let layout = layouts.active().ok_or("No layout is open")?;
    let own = world.get::<InArea>(panel).map(|area| area.0);
    if let Some(showing) = layouts.panels(layout).into_iter().find(|&other| {
        world
            .get::<ViewerFile>(other)
            .is_some_and(|file| file.0 == path)
            && world.get::<InArea>(other).map(|area| area.0) != own
    }) {
        return Ok(Target::Panel(showing));
    }
    let areas: Vec<Entity> = world
        .get::<LayoutAreas>(layout)
        .map_or(&[][..], |areas| areas.entities())
        .iter()
        .copied()
        .filter(|&area| Some(area) != own)
        .collect();
    let shows_documents = |area: Entity| {
        layouts
            .panels_in_area(area)
            .iter()
            .any(|&p| world.get::<DocumentReference>(p).is_some())
    };
    let focused_at = |area: Entity| {
        world
            .get::<AreaFocusedAt>(area)
            .copied()
            .unwrap_or_default()
    };
    let chosen = areas
        .iter()
        .copied()
        .filter(|&area| shows_documents(area))
        .max_by_key(|&area| focused_at(area));
    if let Some(area) = chosen {
        return Ok(Target::New { area: Some(area) });
    }
    let own = own.ok_or("The search panel isn't in an area")?;
    let rect = world
        .get::<Area>(own)
        .ok_or("The search panel isn't in an area")?
        .rect();
    let at = rect.min[0] + (rect.max[0] - rect.min[0]) * 2 / 5;
    let area = world
        .layouts_mut()
        .split_area(own, SplitAxis::Horizontal, at, [1000, 1000])?;
    Ok(Target::New { area: Some(area) })
}

/// Search again, and say what went wrong.
fn finish_replacing(world: &mut World, panel: Entity, result: Result<(usize, usize), String>) {
    if let Some(mut state) = world.get_mut::<SearchState>(panel) {
        state.rerun();
    }
    if let Err(error) = result {
        world
            .workbench_mut()
            .notify(Notification::warning(format!("Replace: {error}")));
    }
}

/// Replace `only` that match in the file at `path`, or every match: in its
/// open document if it has one, else on disk. Returns how many changed.
fn replace(
    world: &mut World,
    panel: Entity,
    path: &Path,
    only: Option<Match>,
) -> Result<usize, String> {
    let Some((matcher, replacement)) = world.get::<SearchState>(panel).and_then(|state| {
        let panel = world.get::<SearchPanel>(panel)?;
        Some((state.matcher.clone()?, panel.replacement.clone()))
    }) else {
        return Ok(0);
    };
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .display()
        .to_string();
    let edits = |text: &str| -> Result<Vec<(std::ops::Range<usize>, String)>, String> {
        match &only {
            Some(found) => matcher
                .replacement(text, found.range.clone(), &replacement)
                .map(|with| vec![(found.range.clone(), with)])
                .ok_or_else(|| format!("{name} changed since the search")),
            None => Ok(matcher.replacements(text, &replacement)),
        }
    };
    let document = world
        .query::<(&ViewerFile, &DocumentReference)>()
        .iter(world)
        .find(|(file, _)| file.0 == path)
        .map(|(_, reference)| reference.document());
    if let Some(document) = document {
        let workbench = world.workbench();
        let handle = workbench
            .cast_document::<TextBuffer>(document)
            .map_err(|e| e.to_string())?;
        let text = workbench
            .read_document(handle)
            .map_err(|e| e.to_string())?
            .to_string();
        let revision = workbench
            .inspect_document(document)
            .map_err(|e| e.to_string())?
            .revision;
        let edits = edits(&text)?;
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
                .map_err(|e| format!("{name}: {e}"))?;
        }
        return Ok(count);
    }
    let original = crate::engine::read_text(path).ok_or_else(|| format!("{name} isn't text"))?;
    let edits = edits(&original)?;
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
        .map_err(|e| format!("{name}: {e}"))?;
    Ok(edits.len())
}

impl Panel for SearchPanel {
    const LABEL: &'static str = "Search";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw(ui, world, context, panel);
    }
}

/// A row of the results: a file, or one of its matches.
#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Match(usize, usize),
}

fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let root = world.get_resource::<Root>().map(|root| root.0.clone());
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|presentation| presentation.focused);
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
        || state.seen.as_ref() != Some(&search.query)
        || state.changed.is_some_and(|at| at.elapsed() < SETTLE)
    {
        ui.ctx().request_repaint_after(Duration::from_millis(30));
    }
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            fields(
                ui,
                context,
                id,
                &mut search,
                &mut state,
                focused,
                &mut intents,
            );
            status(ui, context, &state);
        });
    ui.separator();
    results(
        ui,
        context,
        panel,
        root.as_deref(),
        &search,
        &mut state,
        &mut intents,
    );
    world.entity_mut(panel).insert((search, state));
    world
        .resource_mut::<Intents>()
        .0
        .extend(intents.into_iter().map(|intent| (panel, intent)));
}

/// A small text toggle ("Aa"), with what it does on hover.
fn toggle(ui: &mut egui::Ui, on: &mut bool, text: &str, tip: &str) {
    let label = egui::RichText::new(text).monospace();
    if ui.selectable_label(*on, label).on_hover_text(tip).clicked() {
        *on = !*on;
    }
}

fn fields(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    id: egui::Id,
    search: &mut SearchPanel,
    state: &mut SearchState,
    focused: bool,
    intents: &mut Vec<Intent>,
) {
    let row_height = ui.spacing().interact_size.y;
    ui.horizontal(|ui| {
        let chevron = if search.replacing {
            bedtime_ui::BuiltinIcon::ChevronDown
        } else {
            bedtime_ui::BuiltinIcon::ChevronRight
        };
        if context
            .frame(ui)
            .icon_button_sized(chevron, 14.0)
            .on_hover_text("Replace")
            .clicked()
        {
            search.replacing = !search.replacing;
        }
        // The toggles on the right; the query takes the rest.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let filter = context
                .frame(ui)
                .icon_button_sized(bedtime_ui::BuiltinIcon::Filter, 14.0)
                .on_hover_text("Include and Exclude Files");
            if filter.clicked() {
                search.filtering = !search.filtering;
            }
            toggle(ui, &mut search.query.regex, ".*", "Use Regular Expression");
            toggle(ui, &mut search.query.whole_word, "ab", "Match Whole Word");
            toggle(ui, &mut search.query.match_case, "Aa", "Match Case");
            let query = ui.add(
                egui::TextEdit::singleline(&mut search.query.text)
                    .id(id.with("query"))
                    .hint_text("Search")
                    .desired_width(ui.available_width().max(60.0)),
            );
            if state.focus_query && focused {
                query.request_focus();
                state.focus_query = false;
            }
            if query.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                state.rerun();
            }
        });
    });
    if search.replacing {
        ui.horizontal(|ui| {
            ui.add_space(row_height);
            let button = "Replace All";
            let width = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    button.into(),
                    egui::TextStyle::Button.resolve(ui.style()),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            }) + 2.0 * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x;
            ui.add(
                egui::TextEdit::singleline(&mut search.replacement)
                    .id(id.with("replacement"))
                    .hint_text("Replace with")
                    .desired_width((ui.available_width() - width).max(60.0)),
            );
            let any = state.files.iter().any(|f| !f.matches.is_empty()) && state.task.is_none();
            if ui.add_enabled(any, egui::Button::new(button)).clicked() {
                intents.push(Intent::ReplaceAll);
            }
        });
    }
    if search.filtering {
        ui.horizontal(|ui| {
            ui.add_space(row_height);
            let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            ui.add(
                egui::TextEdit::singleline(&mut search.query.include)
                    .id(id.with("include"))
                    .hint_text("Include: *.rs, src/")
                    .desired_width(half),
            );
            ui.add(
                egui::TextEdit::singleline(&mut search.query.exclude)
                    .id(id.with("exclude"))
                    .hint_text("Exclude")
                    .desired_width(half),
            );
        });
        ui.horizontal(|ui| {
            ui.add_space(row_height);
            context
                .frame(ui)
                .toggle(&mut search.query.ignored, "Files Git ignores");
        });
    }
}

fn status(ui: &mut egui::Ui, context: &UiContext<'_>, state: &SearchState) {
    let palette = &context.theme.palette;
    ui.horizontal(|ui| {
        if let Some(error) = &state.error {
            ui.colored_label(color(palette.error), error);
            return;
        }
        let matches: usize = state.files.iter().map(|f| f.matches.len()).sum();
        let text = if state.ran.as_ref().is_none_or(|q| q.text.is_empty()) {
            String::new()
        } else if matches == 0 && state.task.is_none() {
            "No results".into()
        } else {
            format!(
                "{matches} {} in {} {}{}",
                if matches == 1 { "result" } else { "results" },
                state.files.len(),
                if state.files.len() == 1 {
                    "file"
                } else {
                    "files"
                },
                if state.summary.as_ref().is_some_and(|s| s.limited) {
                    " (stopped there)"
                } else {
                    ""
                },
            )
        };
        ui.weak(text);
        if state.task.is_some() {
            ui.spinner();
        }
    });
}

fn results(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    root: Option<&Path>,
    search: &SearchPanel,
    state: &mut SearchState,
    intents: &mut Vec<Intent>,
) {
    let rows: Vec<Row> = state
        .files
        .iter()
        .enumerate()
        .flat_map(|(file, found)| {
            let open = !state.collapsed.contains(&found.path);
            std::iter::once(Row::File(file)).chain(
                (0..found.matches.len())
                    .filter(move |_| open)
                    .map(move |index| Row::Match(file, index)),
            )
        })
        .collect();
    let height = bedtime_ui::tree::row_height(context);
    let palette = &context.theme.palette;
    let mut toggle_file = None;
    // Under the rows (which take clicks first): Search Again, a right-click away.
    let background = ui.interact(
        ui.available_rect_before_wrap(),
        egui::Id::new((panel, "search-background")),
        egui::Sense::click(),
    );
    background.context_menu(|ui| {
        if ui.button("Search Again").clicked() {
            intents.push(Intent::Refresh);
        }
    });
    egui::ScrollArea::vertical()
        .id_salt((panel, "search-results"))
        .auto_shrink([false, false])
        .show_rows(ui, height, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                match *row {
                    Row::File(file) => {
                        let found = &state.files[file];
                        let response = ui.interact(
                            rect,
                            egui::Id::new((panel, "file", &found.path)),
                            egui::Sense::click(),
                        );
                        let name = found
                            .path
                            .file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name)
                        });
                        bedtime_ui::tree::paint_row(
                            ui,
                            context,
                            bedtime_ui::tree::Row {
                                label: Some(&name),
                                depth: 0,
                                icon: Some(bedtime_filetree::file_icon(&found.path)),
                                expanded: Some(!state.collapsed.contains(&found.path)),
                                selected: false,
                                ink: color(palette.text),
                                icon_ink: palette.text,
                            },
                            rect,
                            Some(&response),
                            1.0,
                            bedtime_ui::tree::INDENT,
                        );
                        // Where it is, and how many, after the name.
                        let folder = found
                            .path
                            .parent()
                            .and_then(|parent| parent.strip_prefix(root?).ok())
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        let font = egui::FontId::proportional(context.metrics.font_size * 0.85);
                        let count = ui.painter().layout_no_wrap(
                            found.matches.len().to_string(),
                            font.clone(),
                            color(palette.muted),
                        );
                        let count_at = egui::pos2(
                            rect.right() - 10.0 - count.size().x,
                            rect.center().y - count.size().y / 2.0,
                        );
                        let name_width = ui
                            .painter()
                            .layout_no_wrap(
                                name.clone(),
                                egui::FontId::proportional(context.metrics.font_size),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x;
                        let folder_left = rect.left()
                            + bedtime_ui::tree::INSET
                            + 18.0
                            + context.metrics.font_size.min(18.0)
                            + 5.0
                            + name_width
                            + 10.0;
                        if !folder.is_empty() && folder_left < count_at.x - 20.0 {
                            let mut job = egui::text::LayoutJob::simple_singleline(
                                folder,
                                font,
                                color(palette.muted),
                            );
                            job.wrap.max_width = count_at.x - 10.0 - folder_left;
                            job.wrap.max_rows = 1;
                            job.wrap.break_anywhere = true;
                            let galley = ui.fonts_mut(|f| f.layout_job(job));
                            ui.painter().galley(
                                egui::pos2(folder_left, rect.center().y - galley.size().y / 2.0),
                                galley,
                                color(palette.muted),
                            );
                        }
                        ui.painter().galley(count_at, count, color(palette.muted));
                        if response.clicked() {
                            toggle_file = Some(found.path.clone());
                        }
                    }
                    Row::Match(file, index) => {
                        let found = &state.files[file];
                        let m = &found.matches[index];
                        let response = ui.interact(
                            rect,
                            egui::Id::new((panel, "match", &found.path, m.range.start)),
                            egui::Sense::click(),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &m.preview)
                        });
                        if response.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(egui::vec2(4.0, 1.0)),
                                context.metrics.rounding,
                                ui.visuals().widgets.hovered.weak_bg_fill,
                            );
                        }
                        let mut right = rect.right() - 8.0;
                        let mut replace = false;
                        if search.replacing && response.hovered() && state.task.is_none() {
                            let button = egui::Rect::from_min_max(
                                egui::pos2(right - 64.0, rect.top() + 2.0),
                                egui::pos2(right, rect.bottom() - 2.0),
                            );
                            replace = ui
                                .put(button, egui::Button::new("Replace").small())
                                .clicked();
                            right = button.left() - 6.0;
                        }
                        let x =
                            rect.left() + bedtime_ui::tree::INSET + bedtime_ui::tree::INDENT + 18.0;
                        let mono = egui::FontId::monospace(context.metrics.font_size * 0.9);
                        let number = ui.painter().layout_no_wrap(
                            format!("{}", m.start.0 + 1),
                            mono.clone(),
                            color(palette.muted),
                        );
                        let number_width = number.size().x.max(context.metrics.font_size * 2.0);
                        ui.painter().galley(
                            egui::pos2(
                                x + number_width - number.size().x,
                                rect.center().y - number.size().y / 2.0,
                            ),
                            number,
                            color(palette.muted),
                        );
                        // The line, its match marked.
                        let text_x = x + number_width + 10.0;
                        let mut job = egui::text::LayoutJob::default();
                        let plain = egui::TextFormat {
                            font_id: mono.clone(),
                            color: color(palette.text),
                            ..default()
                        };
                        let marked = egui::TextFormat {
                            background: ui.visuals().selection.bg_fill,
                            ..plain.clone()
                        };
                        let span = m.preview_range.clone();
                        job.append(&m.preview[..span.start], 0.0, plain.clone());
                        job.append(&m.preview[span.clone()], 0.0, marked);
                        job.append(&m.preview[span.end..], 0.0, plain);
                        job.wrap.max_width = (right - text_x).max(0.0);
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        ui.painter().with_clip_rect(rect).galley(
                            egui::pos2(text_x, rect.center().y - galley.size().y / 2.0),
                            galley,
                            color(palette.text),
                        );
                        if replace {
                            intents.push(Intent::Replace(found.path.clone(), m.clone()));
                        } else if response.clicked() {
                            intents.push(Intent::Open(found.path.clone(), m.clone()));
                        }
                    }
                }
            }
        });
    if let Some(path) = toggle_file
        && !state.collapsed.remove(&path)
    {
        state.collapsed.insert(path);
    }
}
