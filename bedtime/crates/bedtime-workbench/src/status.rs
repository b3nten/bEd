//! The status bar (`ui` feature): a window body (`SurfaceBody(status::body)`)
//! drawing the panels with a bar along the bottom.
//!
//! The left side is the workbench's: what's going on (`StatusActivity`,
//! which any plugin can report to, with a spinner) and the latest
//! notification (for a few seconds; hover for recent ones). The right side is
//! the app's: the items it adds with `add_status_item`, the first added
//! rightmost. Plugins export items for it (workspaces' name, the code
//! plugin's language server) and the app chooses which to show.
//!
//! Left of the app's items are the panel's: what a plugin shows about a
//! panel of its own kind (`add_panel_status::<P>`), for the panel the bar
//! describes (`StatusSource`): the focused one, or, while a panel without
//! items has focus (a sidebar), the last focused that had some.
//!
//! Items are separated by a "·"; an item draws its own pieces with
//! `StatusPieces`, which only puts gaps between them.
use crate::*;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bevy::prelude::*;
use std::{collections::BTreeMap, time::Duration};

/// Room above and below the status bar's text.
const STATUS_PADDING: f32 = 5.0;

/// How long a notification stays in the status bar.
const SHOWN_FOR: Duration = Duration::from_secs(8);

/// How many recent notifications the tooltip lists.
const RECENT: usize = 10;

/// An item on the right of the status bar: it draws itself into the bar's
/// row (right to left), reading the world.
pub type StatusItem = fn(&mut egui::Ui, &World, &UiContext<'_>);

/// The right side's items, the first added rightmost.
#[derive(Resource, Default)]
pub struct StatusItems(Vec<StatusItem>);

/// An item about one panel: drawn into the bar's row (right to left) while
/// `panel` is the one the bar describes.
pub type PanelStatusItem = fn(&mut egui::Ui, &World, &UiContext<'_>, Entity);

/// The panel items, each with the panel kind it's for, in the order added
/// (the first added rightmost).
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
/// on the left of the status bar with a spinner, before the latest
/// notification. A plugin sets its entry while working and clears it after.
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
    /// Show `item` on the right of the status bar, left of those added before.
    fn add_status_item(&mut self, item: StatusItem) -> &mut Self;

    /// Show `item` while a `P` panel is the one the bar describes: left of
    /// the app's items, and of `P`'s items added before. The plugin that
    /// provides `P` adds these.
    fn add_panel_status<P: Component>(&mut self, item: PanelStatusItem) -> &mut Self;
}

impl StatusAppExt for App {
    fn add_status_item(&mut self, item: StatusItem) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<StatusItems>()
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

    let background = bedtime_ui::theme::color(context.theme.palette.background);
    ui.painter().rect_filled(bar, 0.0, background);
    let content = egui::Rect::from_min_max(
        egui::pos2(bar.left() + 10.0, bar.top() + above),
        egui::pos2(bar.right() - 10.0, bar.bottom() - STATUS_PADDING),
    );
    if content.is_positive() {
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("status-content")
                .max_rect(content)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // Rows centre on `interact_size.y`: the content's height, so text
        // sits in its middle.
        row.spacing_mut().interact_size.y = content.height();
        row.spacing_mut().button_padding.y = 0.0;
        row.set_clip_rect(content);
        draw(&mut row, world, context);
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

fn draw(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    // The app's items on the right, then the panel's, a "·" between each
    // that drew something; then what's going on and the notification in
    // what's left.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut row = Separated::default();
        if let Some(items) = world.get_resource::<StatusItems>() {
            for item in &items.0 {
                row.item(ui, context, |ui| item(ui, world, context));
            }
        }
        if let Some(panel) = described(world)
            && let Some(items) = world.get_resource::<PanelStatusItems>()
        {
            for (is_for, item) in &items.0 {
                if is_for(world, panel) {
                    row.item(ui, context, |ui| item(ui, world, context, panel));
                }
            }
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            activity(ui, world, context);
            notification(ui, world, context);
        });
    });
}

/// The panel the bar describes, if it's still around.
fn described(world: &World) -> Option<Entity> {
    world
        .get_resource::<StatusSource>()
        .and_then(|source| source.0)
        .filter(|&panel| world.get_entity(panel).is_ok())
}

/// Items laid out right to left with a "·" midway between neighbours. Each
/// is drawn in a child first, so one that draws nothing gets no separator.
#[derive(Default)]
struct Separated {
    /// The left edge of the last item that drew something.
    last: Option<f32>,
}

impl Separated {
    fn item(
        &mut self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        draw: impl FnOnce(&mut egui::Ui),
    ) {
        let gap = item_gap(context);
        let color = ui.visuals().weak_text_color();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let dot = ui.painter().layout_no_wrap(SEPARATOR.into(), font, color);
        let available = ui.available_rect_before_wrap();
        let right = match self.last {
            Some(last) => available.right().min(last - gap - dot.size().x - gap),
            None => available.right(),
        };
        if right <= available.left() {
            return;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    available.min,
                    egui::pos2(right, available.bottom()),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        draw(&mut child);
        let drawn = child.min_rect();
        if drawn.width() <= 0.0 {
            return;
        }
        if let Some(last) = self.last {
            let at = egui::pos2((drawn.right() + last) / 2.0, available.center().y);
            ui.painter().galley(at - dot.size() / 2.0, dot, color);
        }
        let taken = egui::Rect::from_min_max(
            egui::pos2(drawn.left(), available.top()),
            egui::pos2(available.right(), available.bottom()),
        );
        ui.allocate_rect(taken, egui::Sense::hover());
        self.last = Some(drawn.left());
    }
}

/// What separates items on the right of the bar.
const SEPARATOR: &str = "·";

/// The room either side of a separator, and between an item's pieces.
fn item_gap(context: &UiContext<'_>) -> f32 {
    context.metrics.spacing[0] * 2.0
}

/// An item's pieces, drawn right to left (the first rightmost) with a gap
/// between each and none at the ends: the bar spaces items and puts a "·"
/// between them.
pub struct StatusPieces<'u> {
    ui: &'u mut egui::Ui,
    gap: f32,
    any: bool,
}

impl<'u> StatusPieces<'u> {
    pub fn new(ui: &'u mut egui::Ui, context: &UiContext<'_>) -> Self {
        Self {
            ui,
            gap: item_gap(context),
            any: false,
        }
    }

    /// Draw a piece left of those before it.
    pub fn add<R>(&mut self, piece: impl FnOnce(&mut egui::Ui) -> R) -> R {
        if self.any {
            self.ui.add_space(self.gap);
        }
        self.any = true;
        piece(self.ui)
    }

    /// A piece in the weak text colour.
    pub fn weak(&mut self, text: impl Into<egui::RichText>) -> egui::Response {
        self.add(|ui| ui.weak(text))
    }

    /// A piece in the normal text colour.
    pub fn label(&mut self, text: impl Into<egui::WidgetText>) -> egui::Response {
        self.add(|ui| ui.label(text))
    }

    /// A piece in `color` (`bedtime_ui::theme::color(palette.warning)`).
    pub fn colored(
        &mut self,
        color: egui::Color32,
        text: impl Into<egui::RichText>,
    ) -> egui::Response {
        self.add(|ui| ui.colored_label(color, text))
    }

    /// The row, for what isn't a piece (asking for a repaint).
    pub fn ui(&mut self) -> &mut egui::Ui {
        self.ui
    }
}

fn activity(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let Some(activity) = world.get_resource::<StatusActivity>() else {
        return;
    };
    for (source, doing) in activity.iter() {
        ui.spinner();
        ui.weak(format!("{source}: {doing}"));
        ui.add_space(context.metrics.spacing[0]);
    }
}

fn notification(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>) {
    let palette = &context.theme.palette;
    let Some(notifications) = world.get_resource::<Notifications>() else {
        return;
    };
    let Some(latest) = notifications.latest() else {
        return;
    };
    let Some(left) = SHOWN_FOR.checked_sub(latest.at.elapsed()) else {
        return;
    };
    let tint = match latest.level {
        NotificationLevel::Info => palette.muted,
        NotificationLevel::Warning => palette.warning,
        NotificationLevel::Error => palette.error,
    };
    let recent: Vec<_> = notifications
        .iter()
        .rev()
        .take(RECENT)
        .map(|notification| notification.text.as_str())
        .collect();
    let mut frame = context.frame(ui);
    let side = context.metrics.font_size;
    let icon = frame.icon(
        BuiltinIcon::CircleAlert,
        side,
        bedtime_ui::theme::color(tint),
    );
    let text = frame.ui.add(egui::Label::new(&latest.text).truncate());
    frame.tooltip(&icon.union(text), recent.join("\n"));
    ui.ctx().request_repaint_after(left);
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

    fn noop(_: &mut egui::Ui, _: &World, _: &UiContext<'_>, _: Entity) {}

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
}
