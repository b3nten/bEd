//! The search panel: a query with Match Case, Whole Word and Regular
//! Expression toggles, optional include/exclude filters, and the matches in
//! the workbench's `Root` grouped by file. A click opens the file with the
//! match selected. With the replace row shown, Replace changes one match and
//! Replace All every one: open documents as an undoable edit (left unsaved),
//! other files on disk if they haven't changed since they were read.
use crate::engine::{FileMatches, Match, Matcher, SearchQuery, Summary};
use bedtime_document::{BufferEdit, TextBuffer};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme::color, tree};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    Action, ActionAppExt, DocumentReference, EditOptions, ManagedTask, ManagedWork, Notification,
    Panel, PanelFactory, PanelFactoryPayload, PanelInstance, PanelPresentation, PathOptions, Root,
    Target, ViewerFile, WorkbenchWorldExt,
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
            .register_action::<SearchWorkspace>()
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
pub struct SearchWorkspace;
impl Action for SearchWorkspace {
    // Kept from when it was "Find in Files": saved key bindings use it.
    const KEY: &'static str = "search.find_in_files";
    const LABEL: &'static str = "Search Workspace";
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
    /// The match last opened: its file and where it starts.
    selected: Option<(PathBuf, usize)>,
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
                // Where any file opens: a panel already showing it, else an
                // area showing its kind of viewer, else the largest area.
                let options = PathOptions {
                    line: Some(found.start),
                    select_to: Some(found.end),
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

/// Opens `path` (at `found`, if given) in the area it's dropped in.
fn open_factory(path: PathBuf, found: Option<Match>) -> PanelFactory {
    PanelFactory::new("File", move |world, area| {
        let options = PathOptions {
            line: found.as_ref().map(|found| found.start),
            select_to: found.as_ref().map(|found| found.end),
            target: Target::New { area: Some(area) },
            ..default()
        };
        match world.workbench_mut().open_path(&path, options) {
            Ok(panel) => panel.unwrap_or(Entity::PLACEHOLDER),
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
    const LABEL: &'static str = "Search Workspace";
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
    intents: &mut Vec<Intent>,
) {
    let gap = ui.spacing().item_spacing.x;
    let button = button_width(ui, context);
    let width = (ui.available_width() - 2.0 * (button + gap)).max(80.0);
    let options = [
        (&mut search.query.match_case, "Aa", "Match Case"),
        (&mut search.query.whole_word, "ab", "Match Whole Word"),
        (&mut search.query.regex, ".*", "Use Regular Expression"),
    ];
    let side = (ui.spacing().interact_size.y - 2.0).max(16.0);
    let inset = options.len() as f32 * (side + 2.0) + 2.0;
    ui.horizontal(|ui| {
        let query = field(
            ui,
            id.with("query"),
            &mut search.query.text,
            "Search",
            width,
            inset,
        );
        if state.focus_query && focused {
            query.request_focus();
            state.focus_query = false;
        }
        if query.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            state.rerun();
        }
        // The options, inside the field's right end; after it, so they take
        // the clicks.
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
            field(
                ui,
                id.with("replacement"),
                &mut search.replacement,
                "Replace with",
                width,
                0.0,
            );
            let any = state.files.iter().any(|f| !f.matches.is_empty()) && state.task.is_none();
            ui.add_enabled_ui(any, |ui| {
                if context
                    .frame(ui)
                    .floating_button(BuiltinIcon::ReplaceAll, "Replace All")
                    .clicked()
                {
                    intents.push(Intent::ReplaceAll);
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
        context
            .frame(ui)
            .toggle(&mut search.query.ignored, "Files Git ignores");
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
    // Where each row goes (and how much of it shows): work it out once, then
    // draw only the rows in view.
    let mut rows: Vec<(Row, f32, f32)> = Vec::new();
    let mut top = 0.0;
    for (file, found) in state.files.iter().enumerate() {
        rows.push((Row::File(file), top, height));
        top += height;
        let openness = bedtime_ui::animate_expansion(
            ui.ctx(),
            egui::Id::new((panel, "search-open", &found.path)),
            !state.collapsed.contains(&found.path),
            context.motion,
        );
        if openness > 0.0 {
            let shown = height * openness;
            for index in 0..found.matches.len() {
                rows.push((Row::Match(file, index), top, shown));
                top += shown;
            }
        }
    }
    let mut toggle_file = None;
    let mut opened = None;
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
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min;
            ui.set_min_size(egui::vec2(viewport.width(), top));
            let first = rows.partition_point(|(_, y, h)| y + h < viewport.top());
            for &(row, y, shown) in &rows[first..] {
                if y > viewport.bottom() {
                    break;
                }
                // A closing row is drawn whole, cut to what still shows.
                let rect = egui::Rect::from_min_size(
                    origin + egui::vec2(0.0, y),
                    egui::vec2(viewport.width(), height),
                );
                let visible = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), shown));
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                ui.set_clip_rect(ui.clip_rect().intersect(visible));
                match row {
                    Row::File(file) => {
                        if file_row(&mut ui, context, panel, root, state, file, rect) {
                            toggle_file = Some(state.files[file].path.clone());
                        }
                    }
                    Row::Match(file, index) => {
                        if let Some(intent) =
                            match_row(&mut ui, context, panel, search, state, file, index, rect)
                        {
                            if let Intent::Open(path, found) = &intent {
                                opened = Some((path.clone(), found.range.start));
                            }
                            intents.push(intent);
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
    if opened.is_some() {
        state.selected = opened;
    }
}

/// A file's row: its name, folder and count of matches. Returns whether it
/// was clicked (to open or close it).
fn file_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    root: Option<&Path>,
    state: &SearchState,
    file: usize,
    rect: egui::Rect,
) -> bool {
    let palette = &context.theme.palette;
    let found = &state.files[file];
    let name = found
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let row = tree::Row {
        label: Some(&name),
        depth: 0,
        icon: Some(bedtime_filetree::file_icon(&found.path)),
        expanded: Some(!state.collapsed.contains(&found.path)),
        selected: false,
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
    let relative = found
        .path
        .strip_prefix(root.unwrap_or(Path::new("")))
        .unwrap_or(&found.path);
    let folder = relative
        .parent()
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
    let folder_left = item.right() + 6.0;
    if !folder.is_empty() && folder_left < count_at.x - 20.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(folder, font, color(palette.muted));
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
    // Drag it onto the layout to open it there.
    if response.drag_started() {
        response.dnd_set_drag_payload(PanelFactoryPayload::new(open_factory(
            found.path.clone(),
            None,
        )));
    }
    tree::tooltip(ui, context, &response, relative.display().to_string());
    response.clicked()
}

/// A match's row: its line number and line, the match marked; Replace on
/// the right while it's hovered. Returns what it was asked.
#[allow(clippy::too_many_arguments)]
fn match_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    search: &SearchPanel,
    state: &SearchState,
    file: usize,
    index: usize,
    rect: egui::Rect,
) -> Option<Intent> {
    let palette = &context.theme.palette;
    let found = &state.files[file];
    let m = &found.matches[index];
    // Replace sits over the row's end, without moving anything.
    let button = button_width(ui, context);
    let replacing = search.replacing && state.task.is_none() && ui.rect_contains_pointer(rect);
    let right = rect.right() - 8.0 - if replacing { button + 6.0 } else { 0.0 };
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
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(path, start)| *path == found.path && *start == m.range.start);
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
    if replacing {
        let at = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 8.0 - button / 2.0, rect.center().y),
            egui::vec2(button, rect.height()),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(at)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if context
            .frame(&mut child)
            .floating_button(BuiltinIcon::Replace, "Replace")
            .clicked()
        {
            return Some(Intent::Replace(found.path.clone(), m.clone()));
        }
    }
    response
        .clicked()
        .then(|| Intent::Open(found.path.clone(), m.clone()))
}
