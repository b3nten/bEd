//! The History panel: the commits reachable from HEAD, one to a row, with
//! the lines of history drawn beside them. Selecting one shows what it says
//! and changed (beside the list when there's the width, else under it);
//! clicking a file there opens it as that commit left it, with its changes
//! inline (`CommitDiffPanel`, read-only). The list is read again whenever
//! the session's status moves (a commit, a switch, a fetch, a refresh).
use super::*;
use crate::history::{self, Commit, Detail, FileChange, GraphRow, RefKind};
use bedtime_code::{EditorComparison, EditorPanel, editor::diff::Comparison};
use bedtime_document::{
    DocumentSource, MemorySource, SourceError, SourceId, SourceLoad, SourceVersion, TextBuffer,
};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use std::{hash::Hasher, ops::Range};

/// Open a History panel: a new one, in the focused area.
pub struct ShowHistory;
impl Action for ShowHistory {
    const KEY: &'static str = "git.history";
    const LABEL: &'static str = "Git History";
}

/// Commits read at a time; more are read as the list is scrolled.
const PAGE: usize = 400;
/// The panel's width from which the details sit beside the list.
const BESIDE_WIDTH: f32 = 640.0;
/// The list's width from which rows name the author, and show the hash.
const AUTHOR_WIDTH: f32 = 420.0;
const HASH_WIDTH: f32 = 600.0;
/// Between two lines of history.
const LANE: f32 = 12.0;
/// Files listed for one commit; the rest are counted.
const MAX_FILES: usize = 400;

/// The History panel.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct HistoryPanel {
    #[reflect(ignore)]
    view: View,
}

/// What a list was read for: it's read again when this moves.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    root: String,
    head: Option<String>,
    branch: String,
    ahead: usize,
    behind: usize,
    /// The session's refreshes: one follows every operation.
    epoch: u64,
}

struct Loaded {
    commits: Vec<Commit>,
    graph: Vec<GraphRow>,
    lanes: usize,
    /// There are no older commits to read.
    complete: bool,
}

#[derive(Default)]
struct View {
    key: Option<Key>,
    loaded: Option<Arc<Loaded>>,
    task: Option<(Key, ManagedTask<Result<Loaded, String>>)>,
    /// The list was scrolled near its end: read older commits.
    more: bool,
    error: Option<String>,
    /// The selected commit's hash.
    selected: Option<String>,
    detail: Option<(String, Result<Arc<Detail>, String>)>,
    detail_task: Option<(String, ManagedTask<Result<Detail, String>>)>,
    /// The file last opened from the details.
    file: Option<String>,
    /// A row the keyboard moved to: scrolled into view on the next draw.
    reveal: Option<usize>,
    /// The list's scroll offset and height, as last drawn.
    offset: f32,
    viewport: f32,
}

/// Run `work` with a flag that's raised when `cancel` is.
fn cancellable<T: Send>(
    cancel: &Cancellation,
    work: impl FnOnce(&Arc<AtomicBool>) -> T + Send,
) -> T {
    let flag = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| work(&flag));
        while !worker.is_finished() {
            if cancel.is_cancelled() {
                flag.store(true, Ordering::Release);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        worker.join().expect("the Git reader panicked")
    })
}

/// The newest `count` commits, or `previous` and the page after it.
fn read(
    root: &Path,
    previous: Option<Arc<Loaded>>,
    count: usize,
    cancel: &Cancellation,
) -> Result<Loaded, String> {
    let (mut commits, limit) = match &previous {
        Some(previous) => (previous.commits.clone(), PAGE),
        None => (Vec::new(), count),
    };
    let page = cancellable(cancel, |flag| {
        history::log(root, commits.len(), limit, flag)
    })
    .map_err(|error| error.to_string())?;
    let complete = page.len() < limit;
    commits.extend(page);
    let (graph, lanes) = history::graph(&commits);
    Ok(Loaded {
        commits,
        graph,
        lanes,
        complete,
    })
}

/// Read each History panel's commits, and the selected one's details.
pub(super) fn update(world: &mut World, session: &GitSession) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<HistoryPanel>>()
        .iter(world)
        .collect();
    let key = session.view.as_ref().map(|view| Key {
        root: view.status.root.clone(),
        head: view.status.head.clone(),
        branch: view.status.branch.clone(),
        ahead: view.status.ahead,
        behind: view.status.behind,
        epoch: session.view_epoch,
    });
    for panel in panels {
        let Some(mut state) = world.entity_mut(panel).take::<HistoryPanel>() else {
            continue;
        };
        update_panel(world, session, panel, key.as_ref(), &mut state.view);
        world.entity_mut(panel).insert(state);
    }
}

fn update_panel(
    world: &mut World,
    session: &GitSession,
    panel: Entity,
    key: Option<&Key>,
    view: &mut View,
) {
    if let Some((read_for, task)) = view.task.take() {
        match task.try_recv() {
            // What was read for a status since moved on is read again.
            Ok(result) if Some(&read_for) == key => {
                match result {
                    Ok(loaded) => {
                        view.loaded = Some(Arc::new(loaded));
                        view.error = None;
                    }
                    Err(error) => view.error = Some(error),
                }
                view.key = Some(read_for);
            }
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => view.task = Some((read_for, task)),
            Err(_) => {
                view.error = Some("Reading the history stopped".into());
                view.key = Some(read_for);
            }
        }
    }
    if let Some(key) = key
        && view.task.is_none()
    {
        let stale = view.key.as_ref() != Some(key);
        let more = view.more && view.loaded.as_ref().is_some_and(|loaded| !loaded.complete);
        if stale && (key.root.is_empty() || key.head.is_none()) {
            // Not a repository, or one without commits: nothing to ask Git.
            view.loaded = Some(Arc::new(Loaded {
                commits: Vec::new(),
                graph: Vec::new(),
                lanes: 0,
                complete: true,
            }));
            view.error = None;
            view.key = Some(key.clone());
        } else if stale || more {
            // Read again, as far as was read; or the page after.
            let previous = if stale { None } else { view.loaded.clone() };
            let count = view
                .loaded
                .as_ref()
                .map_or(PAGE, |loaded| loaded.commits.len().max(PAGE));
            let root = session.root.clone();
            let task = world.resource_mut::<ManagedWork>().spawn_task(
                Some(panel),
                "Read Git history",
                move |cancel| read(&root, previous, count, &cancel),
            );
            match task {
                Ok(task) => view.task = Some((key.clone(), task)),
                Err(error) => {
                    view.error = Some(error);
                    view.key = Some(key.clone());
                }
            }
        }
    }
    view.more = false;

    // The selected commit, while it's still part of the history.
    let selected = view.selected.as_ref().and_then(|hash| {
        let loaded = view.loaded.as_ref()?;
        loaded.commits.iter().find(|commit| commit.hash == *hash)
    });
    let Some(commit) = selected else {
        if view.loaded.is_some() && view.task.is_none() {
            view.selected = None;
        }
        if let Some((_, task)) = view.detail_task.take() {
            task.cancel();
        }
        return;
    };
    if let Some((hash, task)) = view.detail_task.take() {
        if hash != commit.hash {
            // Another commit was selected since.
            task.cancel();
        } else {
            match task.try_recv() {
                Ok(result) => view.detail = Some((hash, result.map(Arc::new))),
                Err(std::sync::mpsc::TryRecvError::Empty) => view.detail_task = Some((hash, task)),
                Err(_) => view.detail = Some((hash, Err("Reading the commit stopped".into()))),
            }
        }
    }
    let shown = view
        .detail
        .as_ref()
        .is_some_and(|(hash, _)| *hash == commit.hash);
    let reading = view
        .detail_task
        .as_ref()
        .is_some_and(|(hash, _)| *hash == commit.hash);
    if !shown && !reading {
        let root = session.root.clone();
        let hash = commit.hash.clone();
        let parent = commit.parents.first().cloned();
        let task =
            world
                .resource_mut::<ManagedWork>()
                .spawn_task(Some(panel), "Read Git commit", {
                    let hash = hash.clone();
                    move |cancel| {
                        cancellable(&cancel, |flag| {
                            history::detail(&root, &hash, parent.as_deref(), flag)
                        })
                        .map_err(|error| error.to_string())
                    }
                });
        match task {
            Ok(task) => view.detail_task = Some((hash, task)),
            Err(error) => view.detail = Some((hash, Err(error))),
        }
    }
}

/// How long ago, briefly: "now", "5m", "3h", "2d", "6w", "4mo", "2y".
fn age(now: i64, time: i64) -> String {
    let seconds = (now - time).max(0);
    let (minutes, hours, days) = (seconds / 60, seconds / 3600, seconds / 86400);
    if minutes < 1 {
        "now".into()
    } else if hours < 1 {
        format!("{minutes}m")
    } else if days < 1 {
        format!("{hours}h")
    } else if days < 14 {
        format!("{days}d")
    } else if days < 60 {
        format!("{}w", days / 7)
    } else if days < 365 {
        format!("{}mo", days / 30)
    } else {
        format!("{}y", days / 365)
    }
}

/// A message's prose, unwrapped: lines broken at 72 columns read badly in
/// a panel of another width. Lists, indented text, quotes and trailers
/// (`Signed-off-by: …`) keep their lines.
fn reflow(body: &str) -> String {
    let prose = |line: &str| {
        let trailer = line.split_once(": ").is_some_and(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
        let first = line.chars().next();
        first.is_some_and(|c| !c.is_whitespace() && !c.is_ascii_digit() && !"-*+>#|`".contains(c))
            && !trailer
    };
    let mut text = String::with_capacity(body.len());
    let mut previous: Option<&str> = None;
    for line in body.lines() {
        if let Some(previous) = previous {
            text.push(if prose(previous) && prose(line) {
                ' '
            } else {
                '\n'
            });
        }
        text.push_str(line.trim_end());
        previous = Some(line);
    }
    text
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// One line of text, cut short with an ellipsis at `width`.
fn line(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    ink: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, ink);
    job.wrap.max_width = width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// Paints `galley` from `x`, centred on `y`; returns its right edge.
fn paint_line(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    galley: std::sync::Arc<egui::Galley>,
    ink: egui::Color32,
) -> f32 {
    let width = galley.size().x;
    painter.galley(egui::pos2(x, y - galley.size().y / 2.0), galley, ink);
    x + width
}

/// The colours the lines of history take, lane by lane.
fn lane_inks(context: &UiContext<'_>) -> [egui::Color32; 6] {
    let palette = &context.theme.palette;
    // The accent, then the terminal's blue, magenta, yellow, cyan and red.
    let ansi = |index: usize| palette.terminal[index];
    [palette.accent, ansi(4), ansi(5), ansi(3), ansi(6), ansi(1)].map(theme::color)
}

/// What a row shows beside the subject, by the list's width.
struct Columns {
    font: egui::FontId,
    small: egui::FontId,
    /// The width the lines of history take.
    graph: f32,
    author: Option<f32>,
    hash: Option<f32>,
    age: f32,
    /// Names shown on a commit before the rest are counted.
    refs: usize,
    now: i64,
    inks: [egui::Color32; 6],
}

impl Columns {
    fn new(ui: &egui::Ui, context: &UiContext<'_>, width: f32, lanes: usize) -> Self {
        let size = context.metrics.font_size;
        let font = egui::FontId::proportional(size);
        let small = egui::FontId::proportional((size * 0.86).round());
        let measure = |text: &str, font: &egui::FontId| {
            line(ui, text, font.clone(), egui::Color32::WHITE, f32::INFINITY)
                .size()
                .x
        };
        let narrow = width < AUTHOR_WIDTH;
        Self {
            graph: lanes.clamp(1, if narrow { 4 } else { 8 }) as f32 * LANE,
            author: (!narrow).then(|| (width * 0.18).clamp(90.0, 170.0)),
            hash: (width >= HASH_WIDTH)
                .then(|| measure("0000000", &egui::FontId::monospace(small.size))),
            age: measure("00mo", &small),
            refs: if narrow { 1 } else { 3 },
            now: now(),
            inks: lane_inks(context),
            font,
            small,
        }
    }
}

const ROW_INSET: f32 = 10.0;
/// Clear of the floating scrollbar.
const ROW_TRAILING: f32 = 14.0;

fn paint_graph(
    painter: &egui::Painter,
    rect: egui::Rect,
    columns: &Columns,
    row: &GraphRow,
    merge: bool,
    backdrop: egui::Color32,
) {
    let left = rect.left() + ROW_INSET;
    let clip = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(left + columns.graph, rect.bottom()),
    );
    let painter = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    let x = |lane: usize| left + LANE / 2.0 + lane as f32 * LANE;
    let ink = |lane: usize| columns.inks[lane % columns.inks.len()];
    let stroke = |lane: usize| egui::Stroke::new(1.5, ink(lane));
    let (top, middle, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let node = egui::pos2(x(row.lane), middle);
    for &lane in &row.through {
        painter.vline(x(lane), top..=bottom, stroke(lane));
    }
    // A line that changes lanes bends into (or out of) the commit.
    let bend = |from: egui::Pos2, to: egui::Pos2, lane: usize| {
        let corner = if from == node {
            egui::pos2(to.x, from.y)
        } else {
            egui::pos2(from.x, to.y)
        };
        painter.add(egui::epaint::QuadraticBezierShape::from_points_stroke(
            [from, corner, to],
            false,
            egui::Color32::TRANSPARENT,
            stroke(lane),
        ));
    };
    for &lane in &row.above {
        if lane == row.lane {
            painter.vline(node.x, top..=middle, stroke(lane));
        } else {
            bend(egui::pos2(x(lane), top), node, lane);
        }
    }
    for &lane in &row.below {
        if lane == row.lane {
            painter.vline(node.x, middle..=bottom, stroke(lane));
        } else {
            bend(node, egui::pos2(x(lane), bottom), lane);
        }
    }
    // A merge is a ring; any other commit, a dot.
    if merge {
        painter.circle(node, 3.5, backdrop, stroke(row.lane));
    } else {
        painter.circle(node, 4.0, ink(row.lane), egui::Stroke::new(1.5, backdrop));
    }
}

/// A branch or tag's name, in a chip; returns its right edge.
#[allow(clippy::too_many_arguments)]
fn paint_ref(
    ui: &egui::Ui,
    context: &UiContext<'_>,
    columns: &Columns,
    kind: RefKind,
    name: &str,
    x: f32,
    y: f32,
    width: f32,
) -> f32 {
    let palette = &context.theme.palette;
    let (ink, fill, outline) = match kind {
        RefKind::Head | RefKind::Current => (palette.accent, 0.2, 0.0),
        RefKind::Branch => (palette.text, 0.1, 0.0),
        RefKind::Remote => (palette.muted, 0.0, 0.55),
        RefKind::Tag => (palette.warning, 0.18, 0.0),
    };
    let ink = theme::color(ink);
    let galley = line(ui, name, columns.small.clone(), ink, width - 12.0);
    let chip = egui::Rect::from_center_size(
        egui::pos2(x + galley.size().x / 2.0 + 6.0, y),
        egui::vec2(galley.size().x + 12.0, columns.small.size + 6.0),
    );
    let radius = chip.height() / 2.0;
    ui.painter()
        .rect_filled(chip, radius, ink.gamma_multiply(fill));
    if outline > 0.0 {
        ui.painter().rect_stroke(
            chip,
            radius,
            egui::Stroke::new(1.0, ink.gamma_multiply(outline)),
            egui::StrokeKind::Inside,
        );
    }
    paint_line(ui.painter(), chip.left() + 6.0, y, galley, ink);
    chip.right()
}

/// One commit: its place in the lines of history, the names on it, what it
/// says, and (as the width allows) who wrote it, when, and its hash.
fn draw_commit(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    columns: &Columns,
    commit: &Commit,
    graph: &GraphRow,
    selected: bool,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &commit.subject,
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let highlight = rect.shrink2(egui::vec2(4.0, 0.5));
    bedtime_ui::tree::paint_highlight(ui, ui.painter(), context, highlight, &response, selected);
    paint_graph(
        ui.painter(),
        rect,
        columns,
        graph,
        commit.parents.len() > 1,
        ui.visuals().panel_fill,
    );
    let y = rect.center().y;
    let weak = ui.visuals().weak_text_color();
    let gap = 12.0;
    // From the right: when, the hash, who.
    let mut right = rect.right() - ROW_TRAILING;
    let when = line(
        ui,
        &age(columns.now, commit.time),
        columns.small.clone(),
        weak,
        columns.age,
    );
    paint_line(ui.painter(), right - when.size().x, y, when, weak);
    right -= columns.age + gap;
    if let Some(width) = columns.hash {
        let font = egui::FontId::monospace(columns.small.size);
        let hash = line(ui, commit.short_hash(), font, weak, width);
        paint_line(ui.painter(), right - width, y, hash, weak);
        right -= width + gap;
    }
    if let Some(width) = columns.author {
        let author = line(ui, &commit.author, columns.small.clone(), weak, width);
        paint_line(ui.painter(), right - width, y, author, weak);
        right -= width + gap;
    }
    // The names on it, then what it says, in what's left.
    let mut x = rect.left() + ROW_INSET + columns.graph + 6.0;
    let room = (right - x).max(0.0);
    let shown = commit.refs.len().min(columns.refs);
    for name in &commit.refs[..shown] {
        // Names take up to two fifths of the room; the subject keeps the rest.
        let budget = (rect.left() + ROW_INSET + columns.graph + 6.0 + room * 0.4 - x).min(160.0);
        if budget < 40.0 {
            break;
        }
        x = paint_ref(ui, context, columns, name.kind, &name.name, x, y, budget) + 5.0;
    }
    if commit.refs.len() > shown {
        let more = format!("+{}", commit.refs.len() - shown);
        let more = line(ui, &more, columns.small.clone(), weak, 40.0);
        x = paint_line(ui.painter(), x, y, more, weak) + 6.0;
    }
    let ink = if selected {
        bedtime_ui::rainbow_ink(ui, context.motion)
    } else {
        ui.visuals().text_color()
    };
    let subject = line(ui, &commit.subject, columns.font.clone(), ink, right - x);
    paint_line(ui.painter(), x, y, subject, ink);
    response
}

enum Asked {
    Select(Option<String>),
    Open(FileChange, bool),
}

/// The commits. Up and Down move the selection while the panel has focus.
#[allow(clippy::too_many_arguments)]
fn draw_list(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    rect: egui::Rect,
    loaded: &Loaded,
    view: &mut View,
    focused: bool,
    asked: &mut Option<Asked>,
) {
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    ui.set_clip_rect(rect);
    ui.spacing_mut().item_spacing.y = 0.0;
    let height = bedtime_ui::tree::row_height(context);
    let columns = Columns::new(&ui, context, rect.width(), loaded.lanes);
    let commits = &loaded.commits;
    let selected = view
        .selected
        .as_ref()
        .and_then(|hash| commits.iter().position(|commit| commit.hash == *hash));
    if focused && !ui.ctx().egui_wants_keyboard_input() {
        let pressed = |key| ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key));
        let target = if pressed(egui::Key::ArrowDown) {
            Some(selected.map_or(0, |row| (row + 1).min(commits.len() - 1)))
        } else if pressed(egui::Key::ArrowUp) {
            Some(selected.map_or(0, |row| row.saturating_sub(1)))
        } else {
            None
        };
        if let Some(row) = target
            && selected != Some(row)
        {
            *asked = Some(Asked::Select(Some(commits[row].hash.clone())));
            view.reveal = Some(row);
        }
        if selected.is_some() && pressed(egui::Key::Escape) {
            *asked = Some(Asked::Select(None));
        }
    }
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("history", panel))
        .auto_shrink([false; 2]);
    if let Some(row) = view.reveal.take() {
        let top = row as f32 * height;
        if top < view.offset {
            area = area.vertical_scroll_offset(top);
        } else if top + height > view.offset + view.viewport {
            area = area.vertical_scroll_offset(top + height - view.viewport);
        }
    }
    // A last row stands for the older commits still to read.
    let rows = commits.len() + usize::from(!loaded.complete);
    let output = area.show_rows(&mut ui, height, rows, |ui, range: Range<usize>| {
        if !loaded.complete && range.end + PAGE / 4 >= commits.len() {
            view.more = true;
        }
        for row in range {
            let Some(commit) = commits.get(row) else {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                let weak = ui.visuals().weak_text_color();
                let text = line(
                    ui,
                    "Reading older commits…",
                    columns.small.clone(),
                    weak,
                    rect.width(),
                );
                let x = rect.left() + ROW_INSET + columns.graph + 6.0;
                paint_line(ui.painter(), x, rect.center().y, text, weak);
                continue;
            };
            let is_selected = selected == Some(row);
            let response = ui
                .push_id(row, |ui| {
                    draw_commit(
                        ui,
                        context,
                        &columns,
                        commit,
                        &loaded.graph[row],
                        is_selected,
                        height,
                    )
                })
                .inner;
            if response.clicked() {
                // Clicking the selected commit again puts its details away.
                *asked = Some(Asked::Select((!is_selected).then(|| commit.hash.clone())));
            }
        }
    });
    view.offset = output.state.offset.y;
    view.viewport = output.inner_rect.height();
}

fn status_ink(context: &UiContext<'_>, status: char) -> egui::Color32 {
    let palette = &context.theme.palette;
    theme::color(match status {
        'A' => palette.success,
        'D' => palette.error,
        'R' | 'C' => palette.accent,
        _ => palette.warning,
    })
}

/// A changed file: its status, its name and folder, the lines it gained
/// and lost.
fn draw_file(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    file: &FileChange,
    selected: bool,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &file.path,
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }
    bedtime_ui::tree::paint_highlight(ui, ui.painter(), context, rect, &response, selected);
    let size = context.metrics.font_size;
    let font = egui::FontId::proportional(size);
    let small = egui::FontId::proportional((size * 0.86).round());
    let weak = ui.visuals().weak_text_color();
    let y = rect.center().y;
    let palette = &context.theme.palette;
    // From the right: lines removed, lines added.
    let mut right = rect.right() - 8.0;
    match file.lines {
        Some((added, removed)) => {
            for (count, sign, ink) in [(removed, '−', palette.error), (added, '+', palette.success)]
            {
                if count > 0 {
                    let ink = theme::color(ink);
                    let text = line(ui, &format!("{sign}{count}"), small.clone(), ink, 80.0);
                    right -= text.size().x;
                    paint_line(ui.painter(), right, y, text, ink);
                    right -= 6.0;
                }
            }
        }
        None => {
            let text = line(ui, "binary", small.clone(), weak, 80.0);
            right -= text.size().x;
            paint_line(ui.painter(), right, y, text, weak);
            right -= 6.0;
        }
    }
    let ink = status_ink(context, file.status);
    let letter = line(ui, &file.status.to_string(), small.clone(), ink, 20.0);
    paint_line(ui.painter(), rect.left() + 8.0, y, letter, ink);
    let mut x = rect.left() + 8.0 + size;
    let (folder, name) = match file.path.rsplit_once('/') {
        Some((folder, name)) => (folder, name),
        None => ("", file.path.as_str()),
    };
    let ink = if selected {
        bedtime_ui::rainbow_ink(ui, context.motion)
    } else {
        ui.visuals().text_color()
    };
    let name = line(ui, name, font, ink, right - x);
    x = paint_line(ui.painter(), x, y, name, ink) + 8.0;
    if !folder.is_empty() && right - x > 24.0 {
        let folder = line(ui, folder, small, weak, right - x);
        paint_line(ui.painter(), x, y, folder, weak);
    }
    response
}

/// What the selected commit says and changed.
fn draw_detail(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    rect: egui::Rect,
    commit: &Commit,
    view: &View,
    asked: &mut Option<Asked>,
) {
    let inner = rect.shrink2(egui::vec2(12.0, 0.0));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    ui.set_clip_rect(rect);
    let palette = &context.theme.palette;
    egui::ScrollArea::vertical()
        .id_salt(("history-detail", panel, &commit.hash))
        .auto_shrink([false; 2])
        .show(&mut ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                let width = (ui.available_width() - 30.0).max(40.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(width);
                        ui.add(
                            egui::Label::new(egui::RichText::new(&commit.subject).strong()).wrap(),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.spacing_mut().button_padding = egui::vec2(3.0, 3.0);
                    let mut frame = context.frame(ui);
                    let close = frame.icon_button_sized(BuiltinIcon::Close, 14.0);
                    frame.tooltip(&close, "Close details");
                    if close.clicked() {
                        *asked = Some(Asked::Select(None));
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.weak(&commit.author);
                ui.weak("·");
                ui.weak(&commit.date);
                ui.weak("·");
                let hash = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(commit.short_hash()).monospace().weak(),
                        )
                        .selectable(false)
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Copy the full hash");
                if hash.clicked() {
                    ui.ctx().copy_text(commit.hash.clone());
                }
            });
            let detail = match &view.detail {
                Some((hash, detail)) if *hash == commit.hash => detail,
                _ => {
                    ui.add_space(8.0);
                    ui.weak("Reading commit…");
                    return;
                }
            };
            let detail = match detail {
                Ok(detail) => detail,
                Err(error) => {
                    ui.add_space(8.0);
                    ui.colored_label(theme::color(palette.error), error);
                    return;
                }
            };
            if !detail.body.is_empty() {
                ui.add_space(8.0);
                ui.add(egui::Label::new(reflow(&detail.body)).wrap());
            }
            ui.add_space(8.0);
            let (added, removed) = detail
                .files
                .iter()
                .filter_map(|file| file.lines)
                .fold((0, 0), |sum, lines| (sum.0 + lines.0, sum.1 + lines.1));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.weak(match detail.files.len() {
                    0 => "No files changed".to_owned(),
                    1 => "1 file changed".to_owned(),
                    count => format!("{count} files changed"),
                });
                if added > 0 {
                    ui.colored_label(theme::color(palette.success), format!("+{added}"));
                }
                if removed > 0 {
                    ui.colored_label(theme::color(palette.error), format!("−{removed}"));
                }
            });
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let height = bedtime_ui::tree::row_height(context);
            for file in detail.files.iter().take(MAX_FILES) {
                let selected = view.file.as_deref() == Some(file.path.as_str());
                let response = ui
                    .push_id(&file.path, |ui| {
                        draw_file(ui, context, file, selected, height)
                    })
                    .inner;
                if response.clicked() {
                    *asked = Some(Asked::Open(file.clone(), response.double_clicked()));
                }
                let tip = match &file.old_path {
                    Some(old) => format!("{}\nfrom {old}", file.path),
                    None => file.path.clone(),
                };
                bedtime_ui::tree::tooltip(ui, context, &response, &tip);
            }
            if detail.files.len() > MAX_FILES {
                ui.add_space(4.0);
                ui.weak(format!("and {} more", detail.files.len() - MAX_FILES));
            }
            ui.add_space(10.0);
        });
}

impl Panel for HistoryPanel {
    const LABEL: &'static str = "History";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let status = world
            .get_resource::<GitSession>()
            .and_then(|session| session.view.as_ref().map(|view| view.status.clone()));
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut state) = world.entity_mut(panel).take::<Self>() else {
            return;
        };
        let view = &mut state.view;
        let bounds = ui.available_rect_before_wrap();
        let says = |ui: &mut egui::Ui, draw: &dyn Fn(&mut egui::Ui)| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| draw(ui));
        };
        let mut asked = None;
        match (&status, view.loaded.clone()) {
            (Some(status), _) if status.root.is_empty() => {
                says(ui, &|ui| {
                    ui.weak("This folder isn't in a Git repository.");
                });
            }
            (_, None) => match &view.error {
                Some(error) => says(ui, &|ui| {
                    ui.colored_label(theme::color(context.theme.palette.error), error);
                }),
                None => says(ui, &|ui| {
                    ui.weak("Reading history…");
                }),
            },
            (_, Some(loaded)) if loaded.commits.is_empty() => {
                says(ui, &|ui| {
                    ui.label("No commits yet");
                    ui.weak("The first commit will show here.");
                });
            }
            (_, Some(loaded)) => {
                let commit = view
                    .selected
                    .as_ref()
                    .and_then(|hash| loaded.commits.iter().find(|commit| commit.hash == *hash));
                // The details: beside the list when there's the width, else under it.
                let (list, detail) = match commit {
                    None => (bounds, None),
                    Some(_) if bounds.width() >= BESIDE_WIDTH => {
                        let width = (bounds.width() * 0.42).clamp(300.0, 480.0);
                        let (left, right) = bounds.split_left_right_at_x(bounds.right() - width);
                        ui.painter().vline(
                            left.right(),
                            bounds.y_range(),
                            ui.visuals().widgets.noninteractive.bg_stroke,
                        );
                        (left, Some(right))
                    }
                    Some(_) => {
                        let (top, bottom) = bounds.split_top_bottom_at_fraction(0.45);
                        ui.painter().hline(
                            bounds.x_range(),
                            top.bottom(),
                            ui.visuals().widgets.noninteractive.bg_stroke,
                        );
                        (top, Some(bottom.shrink2(egui::vec2(0.0, 1.0))))
                    }
                };
                draw_list(ui, context, panel, list, &loaded, view, focused, &mut asked);
                if let (Some(commit), Some(rect)) = (commit, detail) {
                    draw_detail(ui, context, panel, rect, commit, view, &mut asked);
                }
            }
        }
        let mut open = None;
        match asked {
            None => {}
            Some(Asked::Select(hash)) => {
                view.selected = hash;
                view.file = None;
            }
            Some(Asked::Open(file, pin)) => {
                view.file = Some(file.path.clone());
                let commit = view.selected.as_ref().and_then(|hash| {
                    let loaded = view.loaded.as_ref()?;
                    loaded.commits.iter().find(|commit| commit.hash == *hash)
                });
                if let (Some(commit), Some(status)) = (commit, &status) {
                    open = Some((
                        CommitDiffPanel {
                            root: PathBuf::from(&status.root),
                            commit: commit.hash.clone(),
                            parent: commit.parents.first().cloned(),
                            added: file.status == 'A',
                            deleted: file.status == 'D',
                            path: file.path,
                            old_path: file.old_path,
                            ..default()
                        },
                        pin,
                    ));
                }
            }
        }
        world.entity_mut(panel).insert(state);
        if let Some((diff, pin)) = open {
            let result = open_diff(world, panel, diff, pin);
            world
                .workbench_mut()
                .notify_failure("Git History", result.map(|_| ()));
        }
    }
}

/// A file as a commit left it, with what the commit changed in it inline:
/// the lines it removed between the ones it kept and added. Read-only.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(EditorPanel)]
pub struct CommitDiffPanel {
    pub root: PathBuf,
    pub commit: String,
    /// What it's compared with: the commit's first parent, if it has one.
    pub parent: Option<String>,
    /// Relative to the repository.
    pub path: String,
    /// Where the file was before, when the commit moved it.
    pub old_path: Option<String>,
    /// The commit added the file, or deleted it.
    pub added: bool,
    pub deleted: bool,
    #[reflect(ignore)]
    task: Option<ManagedTask<Result<Sides, String>>>,
    /// Read, and given to the editor; `Some(false)`: nothing to show as text.
    #[reflect(ignore)]
    shown: Option<Result<bool, String>>,
}

/// The file before and after the commit; `None` when either isn't text.
type Sides = Option<(TextBuffer, TextBuffer)>;

impl CommitDiffPanel {
    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn title(&self) -> String {
        format!("{} ({})", self.name(), history::short(&self.commit))
    }

    fn same(&self, other: &Self) -> bool {
        self.root == other.root && self.commit == other.commit && self.path == other.path
    }

    /// The comparison's identity: the same for the same file of a commit.
    fn identity(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(self.commit.as_bytes());
        hasher.write(self.path.as_bytes());
        hasher.finish().max(1)
    }
}

/// Text that can't be saved: a file as a commit has it.
struct Committed(MemorySource<TextBuffer>);

impl DocumentSource<TextBuffer> for Committed {
    fn id(&self) -> SourceId {
        self.0.id()
    }
    fn title(&self) -> String {
        self.0.title()
    }
    fn load(&self) -> Result<SourceLoad<TextBuffer>, SourceError> {
        self.0.load()
    }
    fn save(
        &self,
        _: TextBuffer,
        _: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        Err(SourceError::Failed("History isn't edited here".into()))
    }
    fn writable(&self) -> bool {
        false
    }
}

/// Show a commit's changes to a file: the panel already showing them, else
/// in place of the preview (a file clicked before), else a new panel
/// somewhere other than the History panel. `pin` keeps and focuses it.
fn open_diff(
    world: &mut World,
    origin: Entity,
    diff: CommitDiffPanel,
    pin: bool,
) -> Result<Entity, String> {
    let layout = world.layouts().active().ok_or("No layout is open")?;
    let panels = world.layouts().panels(layout);
    let is_preview = |world: &World, panel: Entity| {
        world
            .get::<PanelInstance>(panel)
            .is_some_and(|instance| instance.preview)
    };
    if let Some(&panel) = panels.iter().find(|&&panel| {
        world
            .get::<CommitDiffPanel>(panel)
            .is_some_and(|open| open.same(&diff))
    }) {
        if pin {
            if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
                instance.preview = false;
            }
            world.layouts_mut().focus_panel(panel)?;
        } else if !is_preview(world, panel) {
            world.layouts_mut().focus_panel(panel)?;
        }
        return Ok(panel);
    }
    let preview = panels
        .iter()
        .copied()
        .find(|&panel| world.get::<CommitDiffPanel>(panel).is_some() && is_preview(world, panel));
    let area = preview.and_then(|panel| world.get::<InArea>(panel).map(|area| area.0));
    if let Some(old) = preview {
        world.layouts_mut().close_panel(old)?;
    }
    let area = match area {
        Some(area) => area,
        None => world.layouts_mut().area_from(origin)?,
    };
    let title = diff.title();
    // A preview leaves the History panel focused, for the next commit.
    let panel = world.spawn((diff, Place::in_area(area).focus(pin))).id();
    world.flush();
    if world.get::<InArea>(panel).is_none() {
        return Err("The changes couldn't be placed".into());
    }
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
        instance.title = title;
        instance.preview = !pin;
    }
    Ok(panel)
}

fn read_sides(diff: &FileRequest, cancel: &Cancellation) -> Result<Sides, String> {
    let read = |commit: &str, path: &str| {
        cancellable(cancel, |flag| {
            history::file_at(&diff.root, commit, path, flag)
        })
        .map_err(|error| error.to_string())
    };
    let old = match (&diff.parent, diff.added) {
        (Some(parent), false) => read(parent, diff.old_path.as_deref().unwrap_or(&diff.path))?,
        _ => Vec::new(),
    };
    let new = if diff.deleted {
        Vec::new()
    } else {
        read(&diff.commit, &diff.path)?
    };
    let text = |bytes: &[u8]| {
        (!bytes.contains(&0))
            .then(|| std::str::from_utf8(bytes).ok().map(TextBuffer::from))
            .flatten()
    };
    Ok(text(&old).zip(text(&new)))
}

/// What a read needs of its panel.
struct FileRequest {
    root: PathBuf,
    commit: String,
    parent: Option<String>,
    path: String,
    old_path: Option<String>,
    added: bool,
    deleted: bool,
}

/// Read each commit diff once, and give its editor the text and comparison.
pub(super) fn update_diffs(world: &mut World) {
    if world
        .get_resource::<ManagedWork>()
        .is_none_or(ManagedWork::is_closing)
    {
        return;
    }
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<CommitDiffPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let diff = world.get::<CommitDiffPanel>(panel).unwrap();
        let title = diff.title();
        let (name, identity) = (diff.name().to_owned(), diff.identity());
        let request = (diff.shown.is_none() && diff.task.is_none()).then(|| FileRequest {
            root: diff.root.clone(),
            commit: diff.commit.clone(),
            parent: diff.parent.clone(),
            path: diff.path.clone(),
            old_path: diff.old_path.clone(),
            added: diff.added,
            deleted: diff.deleted,
        });
        if let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
            && instance.title != title
        {
            instance.title = title;
        }
        if let Some(request) = request {
            let task = world.resource_mut::<ManagedWork>().spawn_task(
                Some(panel),
                "Read Git commit file",
                move |cancel| read_sides(&request, &cancel),
            );
            let mut diff = world.get_mut::<CommitDiffPanel>(panel).unwrap();
            match task {
                Ok(task) => diff.task = Some(task),
                Err(error) => diff.shown = Some(Err(error)),
            }
            continue;
        }
        let mut diff = world.get_mut::<CommitDiffPanel>(panel).unwrap();
        let Some(task) = diff.task.take() else {
            continue;
        };
        let sides = match task.try_recv() {
            Ok(sides) => sides,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                diff.task = Some(task);
                continue;
            }
            Err(_) => Err("Reading the file stopped".into()),
        };
        let shown = match sides {
            Ok(Some((old, new))) => world
                .workbench_mut()
                .load_document::<TextBuffer>(Committed(MemorySource::new(name, new)))
                .map_err(|error| error.to_string())
                .map(|document| {
                    world.entity_mut(panel).insert((
                        DocumentReference::new(document.id()),
                        EditorComparison {
                            comparison: Comparison::new(identity, old),
                            actions: Vec::new(),
                        },
                    ));
                    true
                }),
            Ok(None) => Ok(false),
            Err(error) => Err(error),
        };
        world.get_mut::<CommitDiffPanel>(panel).unwrap().shown = Some(shown);
    }
}

impl Panel for CommitDiffPanel {
    const LABEL: &'static str = "Commit Changes";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(diff) = world.get::<CommitDiffPanel>(panel) else {
            return;
        };
        let shown = diff.shown.clone();
        let margin = egui::Margin::symmetric(12, 6);
        egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&diff.path).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(egui::RichText::new(history::short(&diff.commit)).monospace());
                });
            });
        });
        let says = |ui: &mut egui::Ui, text: egui::RichText| {
            egui::Frame::NONE
                .inner_margin(margin)
                .show(ui, |ui| ui.label(text));
        };
        match shown {
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            Some(Err(error)) => says(
                ui,
                egui::RichText::new(error).color(theme::color(context.theme.palette.error)),
            ),
            Some(Ok(false)) => says(
                ui,
                egui::RichText::new("Binary file: no lines to compare.").weak(),
            ),
            Some(Ok(true)) => bedtime_code::draw_editor(ui, world, context, panel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{age, reflow};

    #[test]
    fn prose_is_unwrapped_and_lists_are_kept() {
        let body =
            "One line\nand its end.\n\n- a list\n- of two\n\nLast words\nSigned-off-by: A <a@b.c>";
        assert_eq!(
            reflow(body),
            "One line and its end.\n\n- a list\n- of two\n\nLast words\nSigned-off-by: A <a@b.c>"
        );
    }

    #[test]
    fn ages_are_brief() {
        let ago = |seconds: i64| age(1_000_000_000, 1_000_000_000 - seconds);
        assert_eq!(ago(-5), "now");
        assert_eq!(ago(59), "now");
        assert_eq!(ago(5 * 60), "5m");
        assert_eq!(ago(3 * 3600), "3h");
        assert_eq!(ago(2 * 86400), "2d");
        assert_eq!(ago(21 * 86400), "3w");
        assert_eq!(ago(120 * 86400), "4mo");
        assert_eq!(ago(800 * 86400), "2y");
    }
}
