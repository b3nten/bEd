use super::*;
use crate::{BuiltinIcon, Theme, UiError, UiSurface, icon_cache, input, themes};
use std::{collections::HashMap, sync::Mutex};

#[derive(Component, Default)]
struct Observed {
    inline: u32,
    expanded: u32,
    clicks: u32,
    button: Option<egui::Rect>,
}

fn inline(frame: UiFrame, mut observations: Query<&mut Observed>) {
    let mut observed = observations.get_mut(frame.view).unwrap();
    observed.inline += 1;
    let response = frame.ui.button("Activate");
    observed.button = Some(response.rect);
    observed.clicks += u32::from(response.clicked());
}

fn expanded(frame: UiFrame, mut observations: Query<&mut Observed>) {
    observations.get_mut(frame.view).unwrap().expanded += 1;
    frame.ui.label("Plugin-owned expanded content");
}

struct Fixture {
    world: World,
    surface: Entity,
    bar: Entity,
    ctx: egui::Context,
    theme: Theme,
    icons: Mutex<icon_cache::IconCache>,
    input: Mutex<input::UiInputFrame>,
    bounds: Option<egui::Rect>,
}

impl Fixture {
    fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<Messages<UiError>>();
        let surface = world.spawn(UiSurface).id();
        let bar = create_status_bar(&mut world, surface);
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let theme = themes::dark();
        theme.apply(&ctx);
        ctx.global_style_mut(|s| {
            s.interaction.tooltip_delay = 0.0;
            s.interaction.show_tooltips_only_when_still = false;
        });
        Self {
            world,
            surface,
            bar,
            ctx,
            theme,
            icons: default(),
            input: default(),
            bounds: None,
        }
    }

    fn item(&mut self, side: StatusSide, priority: i32) -> (Entity, Entity) {
        let mut descriptor = StatusItem::new(side, BuiltinIcon::Folder, "Example status", 120.0);
        descriptor.priority = priority;
        let item = register_status_item(&mut self.world, self.bar, descriptor, inline);
        self.world.entity_mut(item).insert(Observed::default());
        let tooltip = register_status_tooltip(&mut self.world, item, expanded);
        (item, tooltip)
    }

    fn frame(&mut self, width: f32, time: f64, events: Vec<egui::Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 200.0),
            )),
            time: Some(time),
            events,
            ..default()
        };
        let textures = HashMap::new();
        let mut output = self.ctx.run_ui(raw, |ui| {
            let mut allocation =
                ui.new_child(egui::UiBuilder::new().max_rect(self.bounds.unwrap_or(ui.max_rect())));
            let ui = &mut allocation;
            egui::Panel::bottom("status")
                .exact_size(status_bar_height(ui))
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    draw_status_bar(
                        &mut UiFrame {
                            view: self.bar,
                            surface: self.surface,
                            ui,
                            theme: &self.theme,
                            motion: default(),
                            panel_settings: default(),
                            icons: &self.icons,
                            textures: &textures,
                            input: &self.input,
                        },
                        &mut self.world,
                        self.bar,
                    );
                });
        });
        output.textures_delta.clear();
        assert!(self.world.resource::<Messages<UiError>>().is_empty());
    }

    fn presentation(&self, item: Entity) -> StatusItemPresentation {
        *self.world.get::<StatusItemPresentation>(item).unwrap()
    }
}

#[test]
fn status_bar_uses_the_hosts_allocation_instead_of_the_window() {
    let mut f = Fixture::new();
    f.bounds = Some(egui::Rect::from_min_max(
        egui::pos2(40.0, 60.0),
        egui::pos2(440.0, 160.0),
    ));
    let (left, _) = f.item(StatusSide::Left, 0);
    let (right, _) = f.item(StatusSide::Right, 0);
    f.frame(1100.0, 0.0, vec![]);
    let bar = f.world.get::<StatusBar>(f.bar).unwrap().rect;
    assert_eq!(bar.min.x, 40.0);
    assert_eq!(bar.max, Vec2::new(440.0, 160.0));
    assert_eq!(f.presentation(left).rect.min.x, 40.0 + PADDING[0]);
    assert_eq!(f.presentation(right).rect.max.x, 440.0 - PADDING[0]);
}

#[test]
fn left_and_right_entries_collapse_by_priority_and_expand_again_without_replaying_systems() {
    let mut f = Fixture::new();
    let (important, _) = f.item(StatusSide::Left, 10);
    let (left, _) = f.item(StatusSide::Left, 0);
    let (right, _) = f.item(StatusSide::Right, 0);
    f.frame(1100.0, 0.0, vec![]);
    for item in [important, left, right] {
        let p = f.presentation(item);
        assert!(p.visible && !p.collapsed);
        assert!(
            p.icon_rect.max.x < p.content_rect.min.x,
            "Icon precedes the inline content"
        );
        assert_eq!(f.world.get::<Observed>(item).unwrap().inline, 1);
    }
    assert_eq!(f.presentation(important).rect.min.x, PADDING[0]);
    assert_eq!(f.presentation(right).rect.max.x, 1100.0 - PADDING[0]);

    f.frame(300.0, 0.1, vec![]);
    assert!(!f.presentation(important).collapsed);
    assert!(f.presentation(left).collapsed && f.presentation(right).collapsed);
    assert!(f.presentation(important).rect.max.x <= f.presentation(left).rect.min.x);
    assert!(f.presentation(left).rect.max.x <= f.presentation(right).rect.min.x);
    assert_eq!(f.world.get::<Observed>(important).unwrap().inline, 2);
    assert_eq!(f.world.get::<Observed>(left).unwrap().inline, 1);
    assert_eq!(f.world.get::<Observed>(right).unwrap().inline, 1);

    f.frame(1100.0, 0.2, vec![]);
    for item in [left, right] {
        assert!(!f.presentation(item).collapsed);
        assert_eq!(f.world.get::<Observed>(item).unwrap().inline, 2);
    }
    f.world.get_mut::<StatusItem>(left).unwrap().visible = false;
    f.frame(1100.0, 0.3, vec![]);
    assert!(!f.presentation(left).visible);
    assert_eq!(f.world.get::<Observed>(left).unwrap().inline, 2);
}

#[test]
fn collapsed_icons_keep_their_tooltips_and_inline_widgets_keep_clicks() {
    let mut f = Fixture::new();
    let (item, _) = f.item(StatusSide::Left, 0);
    f.frame(80.0, 0.0, vec![]);
    assert!(f.presentation(item).collapsed);
    let p = f.presentation(item).icon_rect.center();
    for time in [0.01, 0.2] {
        f.frame(
            80.0,
            time,
            vec![egui::Event::PointerMoved(egui::pos2(p.x, p.y))],
        );
    }
    assert!(f.presentation(item).tooltip_visible);
    assert!(f.world.get::<Observed>(item).unwrap().expanded > 0);
    assert_eq!(f.world.get::<Observed>(item).unwrap().inline, 0);

    f.frame(
        600.0,
        0.4,
        vec![egui::Event::PointerMoved(egui::pos2(-20.0, -20.0))],
    );
    let p = f
        .world
        .get::<Observed>(item)
        .unwrap()
        .button
        .unwrap()
        .center();
    for (time, pressed) in [(0.5, true), (0.6, false)] {
        f.frame(
            600.0,
            time,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(f.world.get::<Observed>(item).unwrap().clicks, 1);
    assert_eq!(f.world.get::<Observed>(item).unwrap().inline, 3);
}

#[test]
fn icon_overflow_scrolls_and_item_bar_and_surface_ownership_retire_registered_systems() {
    let mut f = Fixture::new();
    let (first, tooltip) = f.item(StatusSide::Left, 0);
    let (second, _) = f.item(StatusSide::Left, 0);
    let (third, _) = f.item(StatusSide::Right, 0);
    f.frame(60.0, 0.0, vec![]);
    for item in [first, second, third] {
        assert!(f.presentation(item).collapsed);
    }
    let before = f.presentation(first).rect.min.x;
    let bar = f.world.get::<StatusBar>(f.bar).unwrap().rect;
    f.frame(
        60.0,
        0.1,
        vec![
            egui::Event::PointerMoved(egui::pos2(bar.center().x, bar.center().y)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-80.0, 0.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            },
        ],
    );
    f.frame(60.0, 0.2, vec![]);
    assert!(
        f.presentation(first).rect.min.x < before,
        "Even icon overflow stays reachable by scrolling"
    );
    f.world.despawn(first);
    assert!(f.world.get_entity(tooltip).is_err());
    f.world.despawn(f.bar);
    assert!(f.world.get_entity(second).is_err() && f.world.get_entity(third).is_err());
    let bar = create_status_bar(&mut f.world, f.surface);
    let item = register_status_item(
        &mut f.world,
        bar,
        StatusItem::new(StatusSide::Left, BuiltinIcon::Folder, "Owned", 20.0),
        |_: UiFrame| {},
    );
    f.world.despawn(f.surface);
    assert!(f.world.get_entity(bar).is_err() && f.world.get_entity(item).is_err());
}
