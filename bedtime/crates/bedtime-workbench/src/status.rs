//! The status bar (`ui` feature): a window body (`SurfaceBody(status::body)`)
//! drawing the panels with a bar along the bottom.
//!
//! ```text
//! bed  ⑂ main  ◉  60 fps   Ln 12, Col 4 · Rust · 2 errors ····· ⟳ rust-analyzer: indexing · ⚠ Saved
//! └ global status, one item each ┘ └ the panel's status ┘                         updates ┘
//! ```
//!
//! - **Global status** is about the workspace (its name, language servers,
//!   the repository, the frame rate). Each item is small, by convention an
//!   icon or a short text or both, coloured by its most urgent row (a
//!   spinner while one is busy), with its rows on hover. The app chooses
//!   which items show and their order (`add_global_status`); plugins export
//!   them.
//! - **The panel's status** is about the panel the bar describes
//!   (`StatusSource`): the focused one, or, while a panel without items has
//!   focus (a sidebar), the last focused that had some. The plugin that
//!   provides a panel kind adds its items (`add_panel_status::<P>`).
//! - **Updates** are what's going on (`StatusActivity`, any plugin can report
//!   to it) and the latest notification, for a few seconds.
//!
//! Items don't draw: they say what to show (`PanelStatus`, `GlobalStatus`),
//! and the bar measures it all and picks how much of each zone fits
//! (`fit`). Short of room, updates give way first, then the panel's
//! status: each drops its least important pieces, then cuts its text, then
//! becomes an icon, with what's hidden on hover. A fresh warning or error
//! keeps some of its text until the panel's status has given way.
use crate::*;
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
use bevy::prelude::*;
use std::{collections::BTreeMap, time::Duration};

/// Room above and below the status bar's text.
const STATUS_PADDING: f32 = 5.0;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How long a warning or error keeps some of its text however narrow the
/// bar.
const URGENT_FOR: Duration = Duration::from_secs(3);

/// How many recent notifications a tooltip lists.
const RECENT: usize = 10;

/// The narrowest a cut text gets (points) before its zone becomes an icon.
const NARROWEST_CUT: f32 = 60.0;

/// How a piece of status reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    /// The weak text colour: most status.
    #[default]
    Weak,
    /// The normal text colour.
    Normal,
    Warning,
    Error,
}

impl Tone {
    fn color(self, ui: &egui::Ui, context: &UiContext<'_>) -> egui::Color32 {
        let palette = &context.theme.palette;
        match self {
            Tone::Weak => ui.visuals().weak_text_color(),
            Tone::Normal => ui.visuals().text_color(),
            Tone::Warning => theme::color(palette.warning),
            Tone::Error => theme::color(palette.error),
        }
    }
}

/// A piece of a panel's status: a short text, with more on hover.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusPiece {
    pub text: String,
    pub tone: Tone,
    pub tooltip: Option<String>,
}

impl StatusPiece {
    pub fn new(tone: Tone, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone,
            tooltip: None,
        }
    }

    /// Show `text` on hover.
    pub fn tooltip(&mut self, text: impl Into<String>) -> &mut Self {
        self.tooltip = Some(text.into());
        self
    }
}

/// What items say about the panel the bar describes: pieces, the most
/// important first (the last are dropped first when there's no room).
#[derive(Debug, Default)]
pub struct PanelStatus {
    pieces: Vec<StatusPiece>,
    refresh: Option<Duration>,
}

impl PanelStatus {
    pub fn add(&mut self, tone: Tone, text: impl Into<String>) -> &mut StatusPiece {
        self.pieces.push(StatusPiece::new(tone, text));
        self.pieces.last_mut().expect("just pushed")
    }

    pub fn weak(&mut self, text: impl Into<String>) -> &mut StatusPiece {
        self.add(Tone::Weak, text)
    }

    pub fn normal(&mut self, text: impl Into<String>) -> &mut StatusPiece {
        self.add(Tone::Normal, text)
    }

    pub fn warning(&mut self, text: impl Into<String>) -> &mut StatusPiece {
        self.add(Tone::Warning, text)
    }

    pub fn error(&mut self, text: impl Into<String>) -> &mut StatusPiece {
        self.add(Tone::Error, text)
    }

    /// Draw the bar again within `after`: for what changes without input
    /// ("sending… 2s").
    pub fn refresh_after(&mut self, after: Duration) {
        self.refresh = Some(self.refresh.map_or(after, |current| current.min(after)));
    }

    pub fn pieces(&self) -> &[StatusPiece] {
        &self.pieces
    }

    /// The pieces' texts, in order.
    pub fn texts(&self) -> Vec<&str> {
        self.pieces
            .iter()
            .map(|piece| piece.text.as_str())
            .collect()
    }
}

/// A row of the global status: "Branch" "main ↑2".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRow {
    pub label: String,
    pub value: String,
    pub tone: Tone,
    /// Working: the icon becomes a spinner, and the row has one.
    pub busy: bool,
}

impl GlobalRow {
    pub fn tone(&mut self, tone: Tone) -> &mut Self {
        self.tone = tone;
        self
    }

    pub fn busy(&mut self) -> &mut Self {
        self.busy = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GlobalLine {
    Heading(String),
    Row(GlobalRow),
}

/// What a global item shows: an icon, a short text, or both, and rows
/// under headings on hover.
#[derive(Debug, Default)]
pub struct GlobalStatus {
    icon: Option<BuiltinIcon>,
    text: Option<String>,
    tone: Option<Tone>,
    lines: Vec<GlobalLine>,
    refresh: Option<Duration>,
}

impl GlobalStatus {
    /// Show `icon` (before the text, if there's one).
    pub fn icon(&mut self, icon: BuiltinIcon) -> &mut Self {
        self.icon = Some(icon);
        self
    }

    /// Show `text`: keep it short ("bed", "60 fps", "main").
    pub fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.text = Some(text.into());
        self
    }

    /// Colour the item `tone`, whatever its rows say.
    pub fn tone(&mut self, tone: Tone) -> &mut Self {
        self.tone = Some(tone);
        self
    }

    /// Start a group of rows ("Git").
    pub fn heading(&mut self, title: impl Into<String>) {
        self.lines.push(GlobalLine::Heading(title.into()));
    }

    /// A row in the normal colour.
    pub fn row(&mut self, label: impl Into<String>, value: impl Into<String>) -> &mut GlobalRow {
        self.lines.push(GlobalLine::Row(GlobalRow {
            label: label.into(),
            value: value.into(),
            tone: Tone::Normal,
            busy: false,
        }));
        match self.lines.last_mut() {
            Some(GlobalLine::Row(row)) => row,
            _ => unreachable!("just pushed a row"),
        }
    }

    /// Draw the bar again within `after`: for what changes without input
    /// ("60 fps").
    pub fn refresh_after(&mut self, after: Duration) {
        self.refresh = Some(self.refresh.map_or(after, |current| current.min(after)));
    }

    pub fn rows(&self) -> impl Iterator<Item = &GlobalRow> {
        self.lines.iter().filter_map(|line| match line {
            GlobalLine::Row(row) => Some(row),
            GlobalLine::Heading(_) => None,
        })
    }

    /// Nothing to show in the bar.
    pub fn is_empty(&self) -> bool {
        self.icon.is_none() && self.text.is_none() && !self.summary().1
    }

    /// The item's text, then a line per heading and row ("Branch: main"):
    /// for automation and screen readers, which can't hover.
    pub fn described(&self) -> String {
        let mut lines: Vec<String> = self.text.iter().cloned().collect();
        lines.extend(self.lines.iter().map(|line| match line {
            GlobalLine::Heading(title) => title.clone(),
            GlobalLine::Row(row) => format!("{}: {}", row.label, row.value),
        }));
        lines.join("\n")
    }

    /// The item's colour (its own, or its most urgent row's: warning and
    /// error only), and whether anything is busy.
    pub fn summary(&self) -> (Tone, bool) {
        let tone = self.tone.unwrap_or_else(|| {
            self.rows()
                .map(|row| row.tone)
                .filter(|&tone| tone >= Tone::Warning)
                .max()
                .unwrap_or(Tone::Weak)
        });
        (tone, self.rows().any(|row| row.busy))
    }
}

/// An item of the global status: says what it shows, reading the world.
/// Nothing (no icon or text) leaves it out.
pub type GlobalStatusItem = fn(&World, &mut GlobalStatus);

/// The global status's items, in the order added.
#[derive(Resource, Default)]
pub struct GlobalStatusItems(Vec<GlobalStatusItem>);

/// An item about one panel: adds its pieces while `panel` is the one the
/// bar describes.
pub type PanelStatusItem = fn(&World, Entity, &mut PanelStatus);

/// The panel items, each with the panel kind it's for, in the order added.
#[derive(Resource, Default)]
pub struct PanelStatusItems(Vec<(IsFor, PanelStatusItem)>);

/// Whether a panel is of the kind an item is for.
type IsFor = fn(&World, Entity) -> bool;

impl PanelStatusItems {
    /// Whether any item is for `panel`'s kind.
    pub fn describes(&self, world: &World, panel: Entity) -> bool {
        self.0.iter().any(|(is_for, _)| is_for(world, panel))
    }
}

fn is_a<P: Component>(world: &World, panel: Entity) -> bool {
    world.get::<P>(panel).is_some()
}

/// The panel the status bar describes: the focused panel when it has
/// items, otherwise the last one that did while it's still around. Updated
/// in `Update`.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusSource(pub Option<Entity>);

/// Follow focus to panels with items, in `Update`.
pub(crate) fn follow_focus(world: &mut World) {
    let Some(items) = world.get_resource::<PanelStatusItems>() else {
        return;
    };
    let describes =
        |panel: Entity| world.get_entity(panel).is_ok() && items.describes(world, panel);
    let focused = world
        .layouts()
        .active()
        .and_then(|layout| world.layouts().focused_panel(layout))
        .filter(|&panel| describes(panel));
    let remembered = world
        .get_resource::<StatusSource>()
        .and_then(|source| source.0)
        .filter(|&panel| describes(panel));
    let source = StatusSource(focused.or(remembered));
    if world.get_resource::<StatusSource>() != Some(&source) {
        world.insert_resource(source);
    }
}

/// What's going on, by who's doing it ("rust-analyzer": "indexing"): shown
/// with a spinner among the updates, before the latest notification. A
/// plugin sets its entry while working and clears it after.
#[derive(Resource, Default, Debug)]
pub struct StatusActivity(BTreeMap<String, String>);

impl StatusActivity {
    /// Say what `source` is doing; `None` says it's done.
    pub fn set(&mut self, source: impl Into<String>, doing: Option<String>) {
        let source = source.into();
        match doing {
            Some(doing) => {
                if self.0.get(&source) != Some(&doing) {
                    self.0.insert(source, doing);
                }
            }
            None => {
                self.0.remove(&source);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(source, doing)| (source.as_str(), doing.as_str()))
    }
}

pub trait StatusAppExt {
    /// Show `item` in the global status, right of those added before.
    fn add_global_status(&mut self, item: GlobalStatusItem) -> &mut Self;

    /// Add `item`'s pieces while a `P` panel is the one the bar describes,
    /// after those of `P`'s items added before. The plugin that provides
    /// `P` adds these.
    fn add_panel_status<P: Component>(&mut self, item: PanelStatusItem) -> &mut Self;
}

impl StatusAppExt for App {
    fn add_global_status(&mut self, item: GlobalStatusItem) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<GlobalStatusItems>()
            .0
            .push(item);
        self
    }

    fn add_panel_status<P: Component>(&mut self, item: PanelStatusItem) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<PanelStatusItems>()
            .0
            .push((is_a::<P>, item));
        self
    }
}

/// The global items that show something, in order.
pub fn global_status(world: &World) -> Vec<GlobalStatus> {
    let Some(items) = world.get_resource::<GlobalStatusItems>() else {
        return Vec::new();
    };
    items
        .0
        .iter()
        .map(|item| {
            let mut status = GlobalStatus::default();
            item(world, &mut status);
            status
        })
        .filter(|status| !status.is_empty())
        .collect()
}

/// The status of the panel the bar describes, from the items for its kind.
pub fn panel_status(world: &World) -> PanelStatus {
    let mut status = PanelStatus::default();
    let Some(panel) = described(world) else {
        return status;
    };
    if let Some(items) = world.get_resource::<PanelStatusItems>() {
        for (is_for, item) in &items.0 {
            if is_for(world, panel) {
                item(world, panel, &mut status);
            }
        }
    }
    status
}

/// The panel the bar describes, if it's still around.
fn described(world: &World) -> Option<Entity> {
    world
        .get_resource::<StatusSource>()
        .and_then(|source| source.0)
        .filter(|&panel| world.get_entity(panel).is_ok())
}

/// The window's body: the panels, with the status bar along the bottom.
pub fn body(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let available = ui.available_rect_before_wrap();
    // A compact strip: a line of text, with the same room above and below.
    // The panels already leave their gap above the bar, so that counts
    // towards the room above the text.
    let line = ui.text_style_height(&egui::TextStyle::Body);
    let above = (STATUS_PADDING - panel_gap(world, context)).max(0.0);
    let height = (line + above + STATUS_PADDING).min(available.height().max(0.0));
    let bar = egui::Rect::from_min_max(
        egui::pos2(available.left(), available.bottom() - height),
        available.max,
    );
    let panels = egui::Rect::from_min_max(available.min, egui::pos2(available.right(), bar.top()));
    let mut panels_ui = ui.new_child(egui::UiBuilder::new().id_salt("panels").max_rect(panels));
    view::show(&mut panels_ui, world, context);

    let background = bedtime_ui::window_fill(world, context.surface, context.theme);
    ui.painter().rect_filled(bar, 0.0, background);
    let content = egui::Rect::from_min_max(
        egui::pos2(bar.left() + 10.0, bar.top() + above),
        egui::pos2(bar.right() - 10.0, bar.bottom() - STATUS_PADDING),
    );
    if content.is_positive() {
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("status-content")
                .max_rect(content),
        );
        row.set_clip_rect(content);
        draw(&mut row, world, context, content);
    }
    ui.advance_cursor_after_rect(available);
}

/// The gap the panels leave between themselves and the bar: none when one
/// area fills the window edge to edge (as the layout view draws it).
fn panel_gap(world: &World, context: &UiContext<'_>) -> f32 {
    let Some(surface) = world.get::<LayoutSurface>(context.surface) else {
        return 0.0;
    };
    let single = world
        .layouts()
        .active()
        .and_then(|layout| world.get::<LayoutAreas>(layout))
        .is_some_and(|areas| areas.entities().len() == 1);
    if surface.single_area_edge_to_edge && single {
        0.0
    } else {
        surface.panel_gap
    }
}

/// The room either side of a "·", and between the bar's zones.
fn item_gap(context: &UiContext<'_>) -> f32 {
    context.metrics.spacing[0] * 2.0
}

/// What separates pieces.
const SEPARATOR: &str = "·";

/// How wide a zone's form is: fixed (`min == max`), or text that can be cut
/// down to `min`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Form {
    min: f32,
    max: f32,
}

impl Form {
    #[cfg(test)]
    fn fixed(width: f32) -> Self {
        Self {
            min: width,
            max: width,
        }
    }
}

/// Which form of the panel's status and of the updates fits in `available`
/// (each zone's forms widest first), and how wide each is drawn.
///
/// Updates give way first, down to their form `updates_floor`; then the
/// panel's status, down to its last form; then the updates the rest of the
/// way. A cut form takes what's left, the panel's status first.
fn fit(
    available: f32,
    gap: f32,
    panel: &[Form],
    updates: &[Form],
    updates_floor: usize,
) -> ([usize; 2], [f32; 2]) {
    let gap_between = |a: f32, b: f32| if a > 0.0 && b > 0.0 { gap } else { 0.0 };
    let need = |p: usize, u: usize| {
        let (a, b) = (panel[p].min, updates[u].min);
        a + b + gap_between(a, b)
    };
    let floor = updates_floor.min(updates.len() - 1);
    let (mut p, mut u) = (0, 0);
    while need(p, u) > available {
        if u < floor {
            u += 1;
        } else if p + 1 < panel.len() {
            p += 1;
        } else if u + 1 < updates.len() {
            u += 1;
        } else {
            break;
        }
    }
    let (panel_form, updates_form) = (panel[p], updates[u]);
    let room = (available - gap_between(panel_form.max, updates_form.max)).max(0.0);
    let panel_width = panel_form
        .max
        .min(room - updates_form.min)
        .max(panel_form.min);
    let updates_width = updates_form
        .max
        .min(room - panel_width)
        .max(updates_form.min);
    ([p, u], [panel_width, updates_width])
}

/// Something drawn in a zone.
#[derive(Clone, Debug)]
enum Bit {
    Text(StatusPiece),
    Spinner(Tone),
    Icon(BuiltinIcon, Tone),
    Separator,
}

/// One form of a zone: its bits, which one is cut to fit (if any), and
/// what the whole shows on hover (what it hides).
#[derive(Clone, Debug, Default)]
struct Shape {
    bits: Vec<Bit>,
    cut: Option<usize>,
    tooltip: Vec<StatusPiece>,
}

/// Measures bits.
struct Measure<'a, 'c> {
    ui: &'a egui::Ui,
    context: &'a UiContext<'c>,
}

impl Measure<'_, '_> {
    fn font(&self) -> egui::FontId {
        egui::TextStyle::Body.resolve(self.ui.style())
    }

    fn icon_side(&self) -> f32 {
        self.context.metrics.font_size
    }

    /// Room between neighbouring bits; a separator carries its own.
    fn spacing(&self) -> f32 {
        self.context.metrics.spacing[0]
    }

    fn galley(&self, piece: &StatusPiece, cut_to: Option<f32>) -> std::sync::Arc<egui::Galley> {
        let color = piece.tone.color(self.ui, self.context);
        let mut job =
            egui::text::LayoutJob::simple_singleline(piece.text.clone(), self.font(), color);
        if let Some(width) = cut_to {
            job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        }
        self.ui.painter().layout_job(job)
    }

    fn separator(&self) -> std::sync::Arc<egui::Galley> {
        let color = self.ui.visuals().weak_text_color();
        self.ui
            .painter()
            .layout_no_wrap(SEPARATOR.into(), self.font(), color)
    }

    fn width(&self, bit: &Bit) -> f32 {
        match bit {
            Bit::Text(piece) => self.galley(piece, None).size().x,
            Bit::Spinner(_) | Bit::Icon(..) => self.icon_side(),
            Bit::Separator => self.separator().size().x + 2.0 * item_gap(self.context),
        }
    }

    /// The room between `a` and `b`.
    fn between(&self, a: &Bit, b: &Bit) -> f32 {
        if matches!(a, Bit::Separator) || matches!(b, Bit::Separator) {
            0.0
        } else {
            self.spacing()
        }
    }

    /// How wide `shape` is: its whole width, and the least its cut bit
    /// leaves it.
    fn form(&self, shape: &Shape) -> Form {
        let widths: Vec<f32> = shape.bits.iter().map(|bit| self.width(bit)).collect();
        let spaces: f32 = shape
            .bits
            .windows(2)
            .map(|pair| self.between(&pair[0], &pair[1]))
            .sum();
        let max = widths.iter().sum::<f32>() + spaces;
        let min = match shape.cut {
            Some(cut) => max - widths[cut] + widths[cut].min(NARROWEST_CUT),
            None => max,
        };
        Form { min, max }
    }
}

/// The panel's status, widest first: every piece; fewer, with "…" for the
/// rest; the first, cut to fit; an icon.
fn panel_shapes(pieces: &[StatusPiece]) -> Vec<Shape> {
    if pieces.is_empty() {
        return vec![Shape::default()];
    }
    let mut shapes = Vec::new();
    for shown in (1..=pieces.len()).rev() {
        let mut bits = joined(pieces[..shown].iter().cloned().map(Bit::Text));
        if shown < pieces.len() {
            let mut more = StatusPiece::new(Tone::Weak, "…");
            more.tooltip = Some(
                pieces[shown..]
                    .iter()
                    .map(|piece| piece.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            bits.extend([Bit::Separator, Bit::Text(more)]);
        }
        shapes.push(Shape { bits, ..default() });
    }
    shapes.push(Shape {
        bits: vec![Bit::Text(pieces[0].clone())],
        cut: Some(0),
        tooltip: pieces.to_vec(),
    });
    let tone = pieces
        .iter()
        .map(|piece| piece.tone)
        .filter(|&tone| tone >= Tone::Warning)
        .max()
        .unwrap_or(Tone::Weak);
    shapes.push(Shape {
        bits: vec![Bit::Icon(BuiltinIcon::Info, tone)],
        cut: None,
        tooltip: pieces.to_vec(),
    });
    shapes
}

/// The latest notification while it shows: how it reads, and whether it's
/// still urgent (a fresh warning or error).
struct Shown {
    piece: StatusPiece,
    urgent: bool,
}

/// The updates, widest first, and the form they keep until the panel's
/// status has given way: every activity, then the notification; the first
/// activity and how many more; the notification cut to fit; icons.
fn update_shapes(
    activity: &[(String, String)],
    latest: Option<&Shown>,
    recent: &[StatusPiece],
) -> (Vec<Shape>, usize) {
    if activity.is_empty() && latest.is_none() {
        return (vec![Shape::default()], 0);
    }
    let doing = |(source, doing): &(String, String)| {
        [
            Bit::Spinner(Tone::Weak),
            Bit::Text(StatusPiece::new(Tone::Weak, format!("{source}: {doing}"))),
        ]
    };
    let notification = |cut: bool| -> Vec<Bit> {
        latest
            .map(|shown| {
                let mut piece = shown.piece.clone();
                piece.tooltip = (!cut && !recent.is_empty()).then(|| {
                    recent
                        .iter()
                        .map(|piece| piece.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                vec![
                    Bit::Icon(BuiltinIcon::CircleAlert, notification_tint(piece.tone)),
                    Bit::Text(piece),
                ]
            })
            .unwrap_or_default()
    };
    let with_notification = |activities: Vec<Bit>, cut: bool| -> Shape {
        let mut bits = activities;
        let ending = notification(cut);
        if !bits.is_empty() && !ending.is_empty() {
            bits.push(Bit::Separator);
        }
        bits.extend(ending);
        let cut_at = (cut && latest.is_some()).then(|| bits.len() - 1);
        Shape {
            bits,
            cut: cut_at,
            tooltip: if cut {
                everything(activity, recent)
            } else {
                Vec::new()
            },
        }
    };

    let mut shapes = Vec::new();
    let all = {
        let mut bits = Vec::new();
        for (i, entry) in activity.iter().enumerate() {
            if i > 0 {
                bits.push(Bit::Separator);
            }
            bits.extend(doing(entry));
        }
        bits
    };
    shapes.push(with_notification(all, false));
    let condensed = activity
        .first()
        .map(|first| {
            let mut bits = doing(first).to_vec();
            if activity.len() > 1 {
                let mut more = StatusPiece::new(Tone::Weak, format!("+{}", activity.len() - 1));
                more.tooltip = Some(
                    activity
                        .iter()
                        .map(|(source, doing)| format!("{source}: {doing}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                bits.push(Bit::Text(more));
            }
            bits
        })
        .unwrap_or_default();
    if activity.len() > 1 {
        shapes.push(with_notification(condensed.clone(), false));
    }
    let mut floor = None;
    if latest.is_some() {
        if latest.is_some_and(|shown| shown.urgent) {
            floor = Some(shapes.len());
        }
        shapes.push(with_notification(condensed, true));
    }
    let mut icons = Vec::new();
    if !activity.is_empty() {
        icons.push(Bit::Spinner(Tone::Weak));
    }
    if let Some(shown) = latest {
        icons.push(Bit::Icon(
            BuiltinIcon::CircleAlert,
            notification_tint(shown.piece.tone),
        ));
    }
    shapes.push(Shape {
        bits: icons,
        cut: None,
        tooltip: everything(activity, recent),
    });
    let last = shapes.len() - 1;
    (shapes, floor.unwrap_or(last))
}

/// An info notification's icon is weak, its text normal.
fn notification_tint(tone: Tone) -> Tone {
    match tone {
        Tone::Normal => Tone::Weak,
        tone => tone,
    }
}

/// Every activity and the recent notifications, for a collapsed tooltip.
fn everything(activity: &[(String, String)], recent: &[StatusPiece]) -> Vec<StatusPiece> {
    activity
        .iter()
        .map(|(source, doing)| StatusPiece::new(Tone::Weak, format!("{source}: {doing}")))
        .chain(recent.iter().cloned())
        .collect()
}

/// `bits` with a separator between each.
fn joined(bits: impl Iterator<Item = Bit>) -> Vec<Bit> {
    let mut out = Vec::new();
    for bit in bits {
        if !out.is_empty() {
            out.push(Bit::Separator);
        }
        out.push(bit);
    }
    out
}

fn notification_tone(level: NotificationLevel) -> Tone {
    match level {
        NotificationLevel::Info => Tone::Normal,
        NotificationLevel::Warning => Tone::Warning,
        NotificationLevel::Error => Tone::Error,
    }
}

fn draw(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, content: egui::Rect) {
    let gap = item_gap(context);
    let mut left = content.left();

    for (index, item) in global_status(world).iter().enumerate() {
        let right = draw_global(ui, context, index, item, left, content);
        left = right + gap;
    }

    let panel = panel_status(world);
    if let Some(after) = panel.refresh {
        ui.ctx().request_repaint_after(after);
    }
    let panel_shapes = panel_shapes(panel.pieces());

    let activity: Vec<(String, String)> = world
        .get_resource::<StatusActivity>()
        .map(|activity| {
            activity
                .iter()
                .map(|(source, doing)| (source.to_owned(), doing.to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let notifications = world.get_resource::<Notifications>();
    let latest = notifications.and_then(|n| n.latest()).and_then(|latest| {
        let elapsed = latest.at.elapsed();
        let left = SHOWN_FOR.checked_sub(elapsed)?;
        let urgent = latest.level > NotificationLevel::Info && elapsed < URGENT_FOR;
        ui.ctx().request_repaint_after(match urgent {
            true => URGENT_FOR - elapsed,
            false => left,
        });
        Some(Shown {
            piece: StatusPiece::new(notification_tone(latest.level), latest.text.clone()),
            urgent,
        })
    });
    let recent: Vec<StatusPiece> = notifications
        .map(|n| {
            n.iter()
                .rev()
                .take(RECENT)
                .map(|n| StatusPiece::new(notification_tone(n.level), n.text.clone()))
                .collect()
        })
        .unwrap_or_default();
    let (update_shapes, floor) = update_shapes(&activity, latest.as_ref(), &recent);

    let measure = Measure { ui, context };
    let panel_forms: Vec<Form> = panel_shapes.iter().map(|s| measure.form(s)).collect();
    let update_forms: Vec<Form> = update_shapes.iter().map(|s| measure.form(s)).collect();
    let ([p, u], [panel_width, updates_width]) = fit(
        (content.right() - left).max(0.0),
        gap * 2.0,
        &panel_forms,
        &update_forms,
        floor,
    );
    let row = |from: f32, width: f32| {
        egui::Rect::from_min_max(
            egui::pos2(from, content.top()),
            egui::pos2(from + width, content.bottom()),
        )
    };
    paint(
        ui,
        context,
        "status-panel",
        &panel_shapes[p],
        row(left, panel_width),
    );
    paint(
        ui,
        context,
        "status-updates",
        &update_shapes[u],
        row(content.right() - updates_width, updates_width),
    );
}

/// A global item at `left`: its icon (a spinner while busy) and text, and
/// its rows on hover. Returns its right edge.
fn draw_global(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    index: usize,
    item: &GlobalStatus,
    left: f32,
    content: egui::Rect,
) -> f32 {
    let (tone, busy) = item.summary();
    let mut bits = Vec::new();
    match (busy, item.icon) {
        (true, _) => bits.push(Bit::Spinner(tone)),
        (false, Some(icon)) => bits.push(Bit::Icon(icon, tone)),
        (false, None) => {}
    }
    if let Some(text) = &item.text {
        // Plain text is weak; a warning or error colours it.
        let text_tone = if tone >= Tone::Warning { tone } else { Tone::Weak };
        bits.push(Bit::Text(StatusPiece::new(text_tone, text.clone())));
    }
    let measure = Measure { ui, context };
    let width = measure.form(&Shape {
        bits: bits.clone(),
        ..default()
    });
    let rect = egui::Rect::from_min_max(
        egui::pos2(left, content.top()),
        egui::pos2(left + width.max, content.bottom()),
    );
    let widths: Vec<f32> = bits.iter().map(|bit| measure.width(bit)).collect();
    let spaces: Vec<f32> = bits
        .windows(2)
        .map(|pair| measure.between(&pair[0], &pair[1]))
        .collect();
    let mut x = left;
    for (i, bit) in bits.iter().enumerate() {
        if i > 0 {
            x += spaces[i - 1];
        }
        let bit_width = widths[i];
        let at = egui::Rect::from_min_max(
            egui::pos2(x, rect.top()),
            egui::pos2(x + bit_width, rect.bottom()),
        );
        match bit {
            Bit::Text(piece) => {
                let galley = Measure { ui, context }.galley(piece, None);
                let top = at.center().y - galley.size().y / 2.0;
                let color = piece.tone.color(ui, context);
                ui.painter().galley(egui::pos2(at.left(), top), galley, color);
            }
            bit => {
                let side = context.metrics.font_size;
                let square = egui::Rect::from_center_size(at.center(), egui::vec2(side, side));
                paint_bit(ui, context, bit, square);
            }
        }
        x += bit_width;
    }
    let response = ui.interact(rect, ui.id().with(("status-global", index)), egui::Sense::hover());
    // Its text and rows, for automation and screen readers (they can't hover).
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, item.described())
    });
    if !item.lines.is_empty() {
        context.frame(ui).tooltip_ui(&response, |frame| {
            global_tooltip(frame.ui, context, index, item);
        });
    }
    if let Some(after) = item.refresh {
        ui.ctx().request_repaint_after(after);
    }
    rect.right()
}

fn global_tooltip(ui: &mut egui::Ui, context: &UiContext<'_>, index: usize, global: &GlobalStatus) {
    let side = context.metrics.font_size;
    egui::Grid::new(("status-global-rows", index))
        .num_columns(2)
        .spacing([context.metrics.spacing[0] * 3.0, context.metrics.spacing[1]])
        .show(ui, |ui| {
            for line in &global.lines {
                match line {
                    GlobalLine::Heading(title) => {
                        ui.strong(title);
                        ui.end_row();
                    }
                    GlobalLine::Row(row) => {
                        ui.weak(&row.label);
                        ui.horizontal(|ui| {
                            if row.busy {
                                ui.add(
                                    egui::Spinner::new()
                                        .size(side)
                                        .color(row.tone.color(ui, context)),
                                );
                            }
                            ui.colored_label(row.tone.color(ui, context), &row.value);
                        });
                        ui.end_row();
                    }
                }
            }
        });
}

/// Draw `shape` left to right in `rect`, its cut bit taking what's left;
/// hover the whole for what it hides, or a piece for its tooltip.
fn paint(ui: &mut egui::Ui, context: &UiContext<'_>, id: &str, shape: &Shape, rect: egui::Rect) {
    if shape.bits.is_empty() || rect.width() <= 0.0 {
        return;
    }
    let measure = Measure { ui, context };
    let widths: Vec<f32> = shape.bits.iter().map(|bit| measure.width(bit)).collect();
    let spaces: Vec<f32> = shape
        .bits
        .windows(2)
        .map(|pair| measure.between(&pair[0], &pair[1]))
        .collect();
    let fixed: f32 = widths
        .iter()
        .enumerate()
        .filter(|&(i, _)| Some(i) != shape.cut)
        .map(|(_, width)| width)
        .sum::<f32>()
        + spaces.iter().sum::<f32>();
    let cut_to = (rect.width() - fixed).max(0.0);

    let mut x = rect.left();
    for (i, bit) in shape.bits.iter().enumerate() {
        if i > 0 {
            x += spaces[i - 1];
        }
        let width = if Some(i) == shape.cut {
            widths[i].min(cut_to)
        } else {
            widths[i]
        };
        let at = egui::Rect::from_min_max(
            egui::pos2(x, rect.top()),
            egui::pos2(x + width, rect.bottom()),
        );
        match bit {
            Bit::Text(piece) => {
                let cut = (Some(i) == shape.cut).then_some(width);
                let galley = Measure { ui, context }.galley(piece, cut);
                let top = at.center().y - galley.size().y / 2.0;
                let color = piece.tone.color(ui, context);
                ui.painter()
                    .galley(egui::pos2(at.left(), top), galley, color);
                // Text for automation and screen readers, its whole even
                // when cut; its own tooltip unless the whole has one.
                let response = ui.interact(at, ui.id().with((id, i)), egui::Sense::hover());
                let text = piece.text.clone();
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text.clone())
                });
                if let Some(tooltip) = &piece.tooltip
                    && shape.tooltip.is_empty()
                {
                    context.frame(ui).tooltip(&response, tooltip.clone());
                }
            }
            Bit::Separator => {
                let dot = Measure { ui, context }.separator();
                let color = ui.visuals().weak_text_color();
                ui.painter()
                    .galley(at.center() - dot.size() / 2.0, dot, color);
            }
            bit => {
                let side = context.metrics.font_size;
                let square = egui::Rect::from_center_size(at.center(), egui::vec2(side, side));
                paint_bit(ui, context, bit, square);
            }
        }
        x += width;
    }

    // Over the pieces, so hovering anywhere shows what's hidden.
    if !shape.tooltip.is_empty() {
        let whole =
            egui::Rect::from_min_max(rect.min, egui::pos2(x.min(rect.right()), rect.bottom()));
        let response = ui.interact(whole, ui.id().with(id), egui::Sense::hover());
        context.frame(ui).tooltip_ui(&response, |frame| {
            for piece in &shape.tooltip {
                let color = piece.tone.color(frame.ui, context);
                frame.ui.colored_label(color, &piece.text);
            }
        });
    }
}

/// A spinner or an icon, `rect` square.
fn paint_bit(ui: &mut egui::Ui, context: &UiContext<'_>, bit: &Bit, rect: egui::Rect) {
    match bit {
        Bit::Spinner(tone) => {
            egui::Spinner::new()
                .size(rect.width())
                .color(tone.color(ui, context))
                .paint_at(ui, rect);
        }
        Bit::Icon(icon, tone) => {
            let tint = tone.color(ui, context);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            context.frame(&mut child).icon(*icon, rect.width(), tint);
        }
        Bit::Text(_) | Bit::Separator => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_set_and_cleared_by_source() {
        let mut activity = StatusActivity::default();
        activity.set("rust-analyzer", Some("indexing".into()));
        activity.set("git", Some("fetching".into()));
        activity.set("rust-analyzer", Some("checking".into()));
        assert_eq!(
            activity.iter().collect::<Vec<_>>(),
            [("git", "fetching"), ("rust-analyzer", "checking")]
        );
        activity.set("git", None);
        assert_eq!(activity.iter().count(), 1);
    }

    #[derive(Component)]
    struct Described;

    #[derive(Component)]
    struct Plain;

    fn noop(_: &World, _: Entity, _: &mut PanelStatus) {}

    fn focus(world: &mut World, layout: Entity, area: Entity, panel: Entity) {
        world.get_mut::<Area>(area).unwrap().selected = Some(panel);
        world.get_mut::<Layout>(layout).unwrap().focused_area = Some(area);
    }

    fn source(world: &mut World) -> Option<Entity> {
        follow_focus(world);
        world.resource::<StatusSource>().0
    }

    #[test]
    fn the_source_follows_focus_and_stays_on_the_last_panel_with_items() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .init_resource::<StatusSource>()
            .add_panel_status::<Described>(noop);
        let world = app.world_mut();
        let layout = crate::tests::layout(world);
        let left = world.layouts().destination_area(layout).unwrap();
        let right = world
            .layouts_mut()
            .split_area(left, SplitAxis::Horizontal, 5000, [100, 100])
            .unwrap();
        let first = world.spawn((Described, InArea(left))).id();
        let sidebar = world.spawn((Plain, InArea(right))).id();
        let second = world.spawn((Described, InArea(right))).id();

        focus(world, layout, left, first);
        assert_eq!(source(world), Some(first));
        focus(world, layout, right, sidebar);
        assert_eq!(source(world), Some(first), "a panel without items keeps it");
        focus(world, layout, right, second);
        assert_eq!(source(world), Some(second));
        world.despawn(second);
        focus(world, layout, right, sidebar);
        assert_eq!(source(world), None, "a despawned panel isn't kept");
    }

    #[test]
    fn items_for_the_described_panel_add_their_pieces_in_order() {
        fn first(_: &World, _: Entity, status: &mut PanelStatus) {
            status.weak("Ln 1, Col 1");
            status.refresh_after(Duration::from_secs(2));
        }
        fn second(_: &World, _: Entity, status: &mut PanelStatus) {
            status.error("2 errors").tooltip("in this file");
            status.refresh_after(Duration::from_secs(1));
        }
        fn other(_: &World, _: Entity, status: &mut PanelStatus) {
            status.weak("not mine");
        }
        let mut app = App::new();
        app.add_panel_status::<Described>(first)
            .add_panel_status::<Plain>(other)
            .add_panel_status::<Described>(second);
        let panel = app.world_mut().spawn(Described).id();
        assert!(
            panel_status(app.world()).pieces().is_empty(),
            "nothing described"
        );
        app.insert_resource(StatusSource(Some(panel)));
        let status = panel_status(app.world());
        assert_eq!(status.texts(), ["Ln 1, Col 1", "2 errors"]);
        assert_eq!(status.pieces()[1].tone, Tone::Error);
        assert_eq!(status.pieces()[1].tooltip.as_deref(), Some("in this file"));
        assert_eq!(status.refresh, Some(Duration::from_secs(1)), "the soonest");
    }

    #[test]
    fn each_global_item_shows_alone_coloured_by_its_most_urgent_row() {
        fn workspace(_: &World, status: &mut GlobalStatus) {
            status.text("bed");
            status.row("Workspace", "bed");
        }
        fn hidden(_: &World, status: &mut GlobalStatus) {
            status.row("Rows only", "nothing in the bar");
        }
        fn git(_: &World, status: &mut GlobalStatus) {
            status.icon(BuiltinIcon::GitBranch).text("main");
            status.heading("Git");
            status.row("Branch", "main");
            status.row("Conflicts", "1 conflict").tone(Tone::Error);
            status.row("Fetching", "origin").busy();
        }
        let mut app = App::new();
        app.add_global_status(workspace)
            .add_global_status(hidden)
            .add_global_status(git);
        let items = global_status(app.world());
        assert_eq!(items.len(), 2, "an item with no icon or text is left out");
        assert_eq!(items[0].summary(), (Tone::Weak, false));
        assert_eq!(items[1].rows().count(), 3, "headings aren't rows");
        assert_eq!(items[1].summary(), (Tone::Error, true));
        assert_eq!(
            items[1].described(),
            "main\nGit\nBranch: main\nConflicts: 1 conflict\nFetching: origin"
        );
        let mut calm = GlobalStatus::default();
        calm.text("ok").tone(Tone::Normal);
        calm.row("Problem", "ignored").tone(Tone::Error);
        assert_eq!(calm.summary().0, Tone::Normal, "its own tone wins");
        assert!(GlobalStatus::default().is_empty());
    }

    fn fixed(widths: &[f32]) -> Vec<Form> {
        widths.iter().copied().map(Form::fixed).collect()
    }

    #[test]
    fn everything_fits_in_its_widest_form() {
        let panel = fixed(&[100.0, 60.0, 10.0]);
        let updates = fixed(&[80.0, 10.0]);
        assert_eq!(
            fit(200.0, 10.0, &panel, &updates, 1),
            ([0, 0], [100.0, 80.0])
        );
    }

    #[test]
    fn updates_give_way_before_the_panel() {
        let panel = fixed(&[100.0, 60.0, 10.0]);
        let updates = fixed(&[80.0, 40.0, 10.0]);
        assert_eq!(fit(150.0, 10.0, &panel, &updates, 2).0, [0, 1]);
        assert_eq!(fit(120.0, 10.0, &panel, &updates, 2).0, [0, 2]);
        assert_eq!(fit(80.0, 10.0, &panel, &updates, 2).0, [1, 2]);
        assert_eq!(
            fit(5.0, 10.0, &panel, &updates, 2).0,
            [2, 2],
            "the least, clipped"
        );
    }

    #[test]
    fn an_urgent_notification_keeps_its_floor_until_the_panel_has_given_way() {
        let panel = fixed(&[100.0, 60.0, 10.0]);
        let updates = fixed(&[80.0, 40.0, 10.0]);
        assert_eq!(fit(120.0, 10.0, &panel, &updates, 1).0, [1, 1]);
        assert_eq!(fit(60.0, 10.0, &panel, &updates, 1).0, [2, 1]);
        assert_eq!(fit(30.0, 10.0, &panel, &updates, 1).0, [2, 2]);
    }

    #[test]
    fn a_cut_form_takes_what_is_left_the_panel_first() {
        let panel = [
            Form::fixed(300.0),
            Form {
                min: 60.0,
                max: 200.0,
            },
            Form::fixed(10.0),
        ];
        let updates = [
            Form::fixed(50.0),
            Form {
                min: 30.0,
                max: 50.0,
            },
            Form::fixed(10.0),
        ];
        // 150 = panel cut to 90 + gap 10 + updates' cut form at its least.
        assert_eq!(
            fit(130.0, 10.0, &panel, &updates, 1),
            ([1, 1], [90.0, 30.0])
        );
        // Room for the panel's whole cut form: the rest goes to the updates.
        assert_eq!(
            fit(255.0, 10.0, &panel, &updates, 1),
            ([1, 1], [200.0, 45.0])
        );
        let nothing = [Form::fixed(0.0)];
        assert_eq!(
            fit(100.0, 10.0, &panel, &nothing, 0),
            ([1, 0], [100.0, 0.0]),
            "no gap without updates"
        );
    }

    fn texts(shape: &Shape) -> Vec<String> {
        shape
            .bits
            .iter()
            .map(|bit| match bit {
                Bit::Text(piece) => piece.text.clone(),
                Bit::Separator => "·".into(),
                Bit::Spinner(_) => "spinner".into(),
                Bit::Icon(icon, _) => format!("{icon:?}"),
            })
            .collect()
    }

    #[test]
    fn the_panel_drops_its_last_pieces_then_cuts_the_first_then_is_an_icon() {
        let pieces = [
            StatusPiece::new(Tone::Weak, "Ln 1, Col 1"),
            StatusPiece::new(Tone::Weak, "Rust"),
            StatusPiece::new(Tone::Error, "2 errors"),
        ];
        let shapes = panel_shapes(&pieces);
        let all: Vec<_> = shapes.iter().map(texts).collect();
        assert_eq!(all[0], ["Ln 1, Col 1", "·", "Rust", "·", "2 errors"]);
        assert_eq!(all[1], ["Ln 1, Col 1", "·", "Rust", "·", "…"]);
        assert_eq!(all[2], ["Ln 1, Col 1", "·", "…"]);
        assert_eq!(all[3], ["Ln 1, Col 1"]);
        assert_eq!(shapes[3].cut, Some(0));
        assert_eq!(all[4], ["Info"]);
        assert!(
            matches!(shapes[4].bits[0], Bit::Icon(_, Tone::Error)),
            "coloured by the worst"
        );
        assert_eq!(shapes[4].tooltip.len(), 3, "everything on hover");
        let Bit::Text(more) = &shapes[2].bits[2] else {
            panic!("the more marker")
        };
        assert_eq!(more.tooltip.as_deref(), Some("Rust\n2 errors"));
        assert_eq!(panel_shapes(&[]).len(), 1, "nothing: one empty form");
    }

    fn activity(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(source, doing)| (source.to_string(), doing.to_string()))
            .collect()
    }

    #[test]
    fn updates_condense_activity_then_cut_the_notification_then_are_icons() {
        let doing = activity(&[("git", "fetching"), ("rust-analyzer", "indexing")]);
        let saved = Shown {
            piece: StatusPiece::new(Tone::Normal, "Saved"),
            urgent: false,
        };
        let (shapes, floor) = update_shapes(&doing, Some(&saved), &[]);
        let all: Vec<_> = shapes.iter().map(texts).collect();
        assert_eq!(
            all[0],
            [
                "spinner",
                "git: fetching",
                "·",
                "spinner",
                "rust-analyzer: indexing",
                "·",
                "CircleAlert",
                "Saved"
            ]
        );
        assert_eq!(
            all[1],
            [
                "spinner",
                "git: fetching",
                "+1",
                "·",
                "CircleAlert",
                "Saved"
            ]
        );
        assert_eq!(all[2], all[1]);
        assert_eq!(shapes[2].cut, Some(5), "the notification is cut");
        assert_eq!(all[3], ["spinner", "CircleAlert"]);
        assert_eq!(floor, 3, "not urgent: all the way to icons first");

        let failed = Shown {
            piece: StatusPiece::new(Tone::Error, "Couldn't save"),
            urgent: true,
        };
        let (shapes, floor) = update_shapes(&[], Some(&failed), &[]);
        assert_eq!(shapes.iter().map(texts).collect::<Vec<_>>().len(), 3);
        assert_eq!(floor, 1, "urgent: keeps its cut text");
        assert_eq!(update_shapes(&[], None, &[]).0.len(), 1);
        let (only_activity, _) = update_shapes(&doing[..1], None, &[]);
        assert_eq!(
            only_activity.iter().map(texts).collect::<Vec<_>>(),
            [vec!["spinner", "git: fetching"], vec!["spinner"],]
        );
    }
}
