//! One status row with plugin-owned Bevy drawing systems and explicit sizing.
use crate::{IconSource, InView, UiFrame, egui, theme};
use bevy::{ecs::system::SystemId, prelude::*};

const PADDING: [f32; 2] = [10.0, 4.0];
const GAP: f32 = 20.0;
const ICON_GAP: f32 = 6.0;

#[derive(Component, Default)]
pub struct StatusBar {
    /// The most recent screen rectangle, for application layout/observations.
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusSide {
    Left,
    Right,
}

#[derive(Component, Clone)]
pub struct StatusItem {
    pub side: StatusSide,
    pub icon: IconSource,
    /// Accessible name and the tooltip when no expanded renderer is registered.
    pub label: String,
    /// Inline content width in logical points, excluding the icon and its gap.
    /// Producers may update it alongside their displayed data.
    pub width: f32,
    /// Larger priorities retain their inline content longer. Ties collapse the
    /// most recently registered item first; registration also orders each side.
    pub priority: i32,
    pub visible: bool,
}

impl StatusItem {
    pub fn new(
        side: StatusSide,
        icon: impl Into<IconSource>,
        label: impl Into<String>,
        width: f32,
    ) -> Self {
        Self {
            side,
            icon: icon.into(),
            label: label.into(),
            width,
            priority: 0,
            visible: true,
        }
    }
}

#[derive(Component, Default, Clone, Copy, Debug)]
pub struct StatusItemPresentation {
    pub visible: bool,
    pub collapsed: bool,
    pub rect: Rect,
    pub icon_rect: Rect,
    pub content_rect: Rect,
    pub tooltip_visible: bool,
}

#[derive(Component)]
#[relationship(relationship_target = StatusBarItems)]
pub struct InStatusBar(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InStatusBar, linked_spawn)]
pub struct StatusBarItems(Vec<Entity>);

impl StatusBarItems {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Component, Clone, Copy)]
struct Renderer {
    expanded: Option<SystemId<UiFrame<'static>>>,
}

/// Create a bar owned by a view or surface. The host allocates its rectangle and
/// calls `draw_status_bar`; bars never position themselves against the window.
pub fn create_status_bar(world: &mut World, owner: Entity) -> Entity {
    world.entity(owner);
    world.spawn((StatusBar::default(), InView(owner))).id()
}

pub fn status_bar_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body) + PADDING[1] * 2.0
}

/// The system receives `UiFrame.view == item`. Attach plugin data to the returned
/// entity. Inline systems execute once per frame while expanded in the row.
pub fn register_status_item<M>(
    world: &mut World,
    bar: Entity,
    item: StatusItem,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Entity {
    assert!(
        world.get::<StatusBar>(bar).is_some(),
        "Status bar is unavailable"
    );
    let system: SystemId<UiFrame<'static>> = world.register_system(draw);
    world.entity_mut(system.entity()).insert((
        item,
        Renderer { expanded: None },
        InStatusBar(bar),
        StatusItemPresentation::default(),
    ));
    system.entity()
}

/// Replace or attach the optional hover expansion. It receives the same item
/// entity as the inline system, and its lifetime is owned by that item.
pub fn register_status_tooltip<M>(
    world: &mut World,
    item: Entity,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Entity {
    let old = world
        .get::<Renderer>(item)
        .expect("Registered status item")
        .expanded;
    if let Some(old) = old {
        world.despawn(old.entity());
    }
    let system: SystemId<UiFrame<'static>> = world.register_system(draw);
    world.entity_mut(system.entity()).insert(InView(item));
    world.get_mut::<Renderer>(item).unwrap().expanded = Some(system);
    system.entity()
}

struct Entry {
    entity: Entity,
    item: StatusItem,
    expanded: Option<SystemId<UiFrame<'static>>>,
}

// Decide representation before executing any plugin systems. Widths belong to
// the producer; no stateful renderer is replayed in a hidden measurement pass.
fn allocate(entries: &[Entry], budget: f32, icon: f32) -> (Vec<bool>, f32) {
    let mut collapsed = vec![false; entries.len()];
    let mut width = entries
        .iter()
        .map(|e| icon + ICON_GAP + e.item.width)
        .sum::<f32>()
        + entries.len().saturating_sub(1) as f32 * GAP;
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by_key(|&i| (entries[i].item.priority, std::cmp::Reverse(i)));
    for i in order {
        if width <= budget {
            break;
        }
        collapsed[i] = true;
        width -= ICON_GAP + entries[i].item.width;
    }
    (collapsed, width)
}

/// Draw inside the supplied allocation. The host owns its background and reserves
/// `status_bar_height` before drawing. Item renderers remain ordinary Bevy systems.
pub fn draw_status_bar(frame: &mut UiFrame, world: &mut World, bar: Entity) {
    let items = world
        .get::<StatusBarItems>(bar)
        .map_or_else(Vec::new, |items| items.0.clone());
    let mut entries = Vec::new();
    for entity in items {
        *world
            .get_mut::<StatusItemPresentation>(entity)
            .expect("Registered status presentation") = default();
        let item = world
            .get::<StatusItem>(entity)
            .expect("Registered status item");
        assert!(
            item.width.is_finite() && item.width >= 0.0 && !item.label.trim().is_empty(),
            "Status items require a nonnegative finite width and a label"
        );
        if item.visible {
            entries.push(Entry {
                entity,
                item: item.clone(),
                expanded: world.get::<Renderer>(entity).unwrap().expanded,
            });
        }
    }
    let bounds = frame.ui.max_rect().intersect(frame.ui.clip_rect());
    world
        .get_mut::<StatusBar>(bar)
        .expect("Registered status bar")
        .rect = Rect::from_corners(
        Vec2::new(bounds.min.x, bounds.min.y),
        Vec2::new(bounds.max.x, bounds.max.y),
    );
    let mut bar = frame.ui.new_child(
        egui::UiBuilder::new()
            .id_salt("status-bar")
            .max_rect(bounds),
    );
    bar.set_clip_rect(bounds);
    bar.painter().line_segment(
        [bounds.left_top(), bounds.right_top()],
        egui::Stroke::new(1.0, theme::color(frame.theme.palette.border)),
    );
    let inner = bounds.shrink2(egui::vec2(PADDING[0], PADDING[1]));
    if !inner.is_positive() {
        return;
    }
    let icon = inner.height();
    let budget = inner.width();
    let (collapsed, width) = allocate(&entries, budget, icon);
    let content_width = width.max(budget);
    let mut row = bar.new_child(egui::UiBuilder::new().id_salt("status-row").max_rect(inner));
    row.set_clip_rect(inner);
    egui::ScrollArea::horizontal()
        .id_salt("status-overflow")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, false])
        .show(&mut row, |ui| {
            let origin = ui.cursor().min;
            ui.set_min_size(egui::vec2(content_width, icon));
            let mut left = origin.x;
            let mut right = origin.x + content_width;
            for (i, entry) in entries.iter().enumerate() {
                if world.get::<StatusItem>(entry.entity).is_none() {
                    continue;
                }
                let width = icon
                    + if collapsed[i] {
                        0.0
                    } else {
                        ICON_GAP + entry.item.width
                    };
                let x = match entry.item.side {
                    StatusSide::Left => {
                        let x = left;
                        left += width + GAP;
                        x
                    }
                    StatusSide::Right => {
                        right -= width;
                        let x = right;
                        right -= GAP;
                        x
                    }
                };
                let rect =
                    egui::Rect::from_min_size(egui::pos2(x, origin.y), egui::vec2(width, icon));
                let icon_rect = egui::Rect::from_min_size(rect.min, egui::vec2(icon, icon));
                let side = frame.theme.metrics.font_size.min(icon);
                let point = icon_rect.center() - egui::vec2(side, side) * 0.5;
                let mut item_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(entry.entity.to_bits())
                        .max_rect(rect),
                );
                item_ui.set_clip_rect(ui.clip_rect().intersect(rect));
                frame.for_view(entry.entity, &mut item_ui).icon_at(
                    &entry.item.icon,
                    [point.x, point.y],
                    side,
                );
                let mut content = Rect::default();
                if !collapsed[i] {
                    let body = egui::Rect::from_min_max(
                        egui::pos2(icon_rect.right() + ICON_GAP, rect.top()),
                        rect.max,
                    );
                    content = Rect::from_corners(
                        Vec2::new(body.min.x, body.min.y),
                        Vec2::new(body.max.x, body.max.y),
                    );
                    let mut body_ui = item_ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt("body")
                            .max_rect(body)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    body_ui.set_clip_rect(item_ui.clip_rect().intersect(body));
                    body_ui.spacing_mut().button_padding.y = 0.0;
                    body_ui.spacing_mut().interact_size.y = icon;
                    if let Err(error) = world.run_system_with(
                        SystemId::<UiFrame<'static>>::from_entity(entry.entity),
                        frame.for_view(entry.entity, &mut body_ui),
                    ) {
                        world.write_message(crate::UiError(error.to_string()));
                    }
                }
                let response =
                    item_ui.interact(rect, item_ui.id().with("hover"), egui::Sense::hover());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &entry.item.label)
                });
                let tooltip_visible = if let Some(expanded) = entry.expanded {
                    frame
                        .for_view(entry.entity, &mut item_ui)
                        .tooltip_ui(&response, |frame| {
                            if let Err(error) = world.run_system_with(expanded, frame) {
                                world.write_message(crate::UiError(error.to_string()));
                            }
                        })
                        .is_some()
                } else {
                    frame
                        .for_view(entry.entity, &mut item_ui)
                        .tooltip(&response, &entry.item.label)
                };
                if let Some(mut presentation) =
                    world.get_mut::<StatusItemPresentation>(entry.entity)
                {
                    *presentation = StatusItemPresentation {
                        visible: rect.intersects(ui.clip_rect()),
                        collapsed: collapsed[i],
                        rect: Rect::from_corners(
                            Vec2::new(rect.min.x, rect.min.y),
                            Vec2::new(rect.max.x, rect.max.y),
                        ),
                        icon_rect: Rect::from_corners(
                            Vec2::new(icon_rect.min.x, icon_rect.min.y),
                            Vec2::new(icon_rect.max.x, icon_rect.max.y),
                        ),
                        content_rect: content,
                        tooltip_visible,
                    };
                }
            }
        });
}

#[cfg(test)]
mod tests;
