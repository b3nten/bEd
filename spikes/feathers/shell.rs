//! The workbench shell: areas with gaps and rounded corners, tab bars and a
//! status bar. Built from the workbench model every frame, so there is no
//! frontend state to keep in step with it; tab clicks go back to the model
//! as workbench operations.
use crate::look::{FONT, Look};
use crate::panels::{self, Content};
use bedtime_workbench::{
    ActiveLayout, Area, AreaPanels, LAYOUT_EXTENT, LayoutAreas, PanelInstance, WorkbenchWorldExt,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::text::FontSize;
use bevy_immediate::{
    Imm,
    attach::{BevyImmediateAttachPlugin, ImmediateAttach},
    ui::{
        CapsUiFeathers, clicked::ImmUiClicked, interaction::ImmUiInteraction, look::ImmUiLook,
        text::ImmUiText,
    },
};

const TAB_BAR_HEIGHT: f32 = 34.0;
const STATUS_BAR_HEIGHT: f32 = 28.0;

/// The window's root node; the shell is built under it.
#[derive(Component)]
#[require(Node = root_node())]
pub struct ShellRoot;

fn root_node() -> Node {
    Node {
        width: percent(100),
        height: percent(100),
        flex_direction: FlexDirection::Column,
        ..default()
    }
}

pub fn plugin(app: &mut App) {
    app.add_plugins(BevyImmediateAttachPlugin::<CapsUiFeathers, ShellRoot>::new());
}

#[derive(SystemParam)]
pub struct Params<'w, 's> {
    layout: Query<'w, 's, &'static LayoutAreas, With<ActiveLayout>>,
    areas: Query<'w, 's, (&'static Area, Option<&'static AreaPanels>)>,
    panels: Query<'w, 's, (&'static PanelInstance, Content)>,
    look: Res<'w, Look>,
    assets: Res<'w, AssetServer>,
}

impl ImmediateAttach<CapsUiFeathers> for ShellRoot {
    type Params = Params<'static, 'static>;

    fn construct(ui: &mut Imm<CapsUiFeathers>, params: &mut Params) {
        let look = &*params.look;
        let p = &look.theme.palette;
        let font = TextFont {
            font: params.assets.load(FONT).into(),
            font_size: FontSize::Px(look.font_size),
            ..default()
        };
        let restyle = params.look.is_changed();

        ui.ch()
            .on_spawn_insert(|| Node {
                flex_grow: 1.0,
                ..default()
            })
            .add(|ui| {
                let Ok(areas) = params.layout.single() else {
                    return;
                };
                for &area in areas.entities() {
                    let Ok((info, tabs)) = params.areas.get(area) else {
                        continue;
                    };
                    let tabs = tabs.map_or(&[][..], |t| t.entities());
                    let r = info.rect();
                    let pct = |v: i32| percent(v as f32 * 100.0 / LAYOUT_EXTENT as f32);
                    // Gaps are full at the window edge and shared between neighbours.
                    let gap = |edge: bool| px(if edge { look.gap } else { look.gap * 0.5 });
                    // The cell is the area's share of the window; the surface is inset by the gap.
                    ui.ch_id(area)
                        .on_spawn_insert(|| Node {
                            position_type: PositionType::Absolute,
                            ..default()
                        })
                        .node_mut(|node| {
                            node.left = pct(r.min[0]);
                            node.top = pct(r.min[1]);
                            node.width = pct(r.max[0] - r.min[0]);
                            node.height = pct(r.max[1] - r.min[1]);
                            node.padding = UiRect {
                                left: gap(r.min[0] == 0),
                                right: gap(r.max[0] == LAYOUT_EXTENT),
                                top: gap(r.min[1] == 0),
                                bottom: gap(r.max[1] == LAYOUT_EXTENT),
                            };
                        })
                        .add(|ui| {
                            ui.ch()
                                .on_spawn_insert(|| Node {
                                    flex_grow: 1.0,
                                    flex_direction: FlexDirection::Column,
                                    overflow: Overflow::clip(),
                                    ..default()
                                })
                                .node_mut(|node| {
                                    node.border_radius = BorderRadius::all(px(look.rounding));
                                })
                                .background_color(look.color(p.panel_background))
                                .add(|ui| {
                                    if look.tab_bar {
                                        tab_bar(
                                            ui,
                                            params,
                                            info.selected_panel(),
                                            tabs,
                                            &font,
                                            restyle,
                                        );
                                    }
                                    let Some(panel) = info.selected_panel() else {
                                        return;
                                    };
                                    let Ok((_, content)) = params.panels.get(panel) else {
                                        return;
                                    };
                                    panels::host(ui, panel, content);
                                });
                        });
                }
            });

        // Status bar: the workspace's name, right-aligned, on the window background.
        ui.ch()
            .on_spawn_insert(|| Node {
                height: px(STATUS_BAR_HEIGHT),
                flex_shrink: 0.0,
                justify_content: JustifyContent::End,
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(px(12)),
                ..default()
            })
            .background_color(look.color(p.background))
            .add(|ui| {
                ui.ch()
                    .on_change_insert(restyle, || font.clone())
                    .text("Default workspace")
                    .text_color(look.color(p.muted));
            });
    }
}

fn tab_bar(
    ui: &mut Imm<CapsUiFeathers>,
    params: &Params,
    selected: Option<Entity>,
    tabs: &[Entity],
    font: &TextFont,
    restyle: bool,
) {
    let look = &*params.look;
    let p = &look.theme.palette;
    ui.ch()
        .on_spawn_insert(|| Node {
            height: px(TAB_BAR_HEIGHT),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            border: UiRect::bottom(px(1)),
            overflow: Overflow::clip(),
            ..default()
        })
        .border_color(&BorderColor::all(look.color(p.border)))
        .add(|ui| {
            for &panel in tabs {
                let Ok((instance, _)) = params.panels.get(panel) else {
                    continue;
                };
                let is_selected = selected == Some(panel);
                let mut tab = ui.ch_id(panel).on_spawn_insert(|| Node {
                    min_width: px(70),
                    max_width: px(210),
                    padding: UiRect::new(px(10), px(4), px(0), px(0)),
                    column_gap: px(8),
                    align_items: AlignItems::Center,
                    ..default()
                });
                let hovered = tab.hovered();
                if tab.clicked() {
                    tab.commands().queue(move |world: &mut World| {
                        let _ = world.workbench_mut().focus_panel(panel);
                    });
                }
                let mut tint = p.text;
                tint[3] = if hovered { 0.12 } else { 0.0 };
                let emphasis = if is_selected || hovered { 1.0 } else { 0.0 };
                tab.node_mut(|node| {
                    node.border_radius = BorderRadius::top(px(look.rounding));
                })
                .background_color(look.color(tint))
                .add(|ui| {
                    ui.ch()
                        .on_change_insert(restyle, || font.clone())
                        .text(instance.title.as_str())
                        .text_color(look.mix(p.muted, p.text, emphasis));
                    // The close button shows on the selected or hovered tab.
                    let mut close = ui.ch().on_spawn_insert(|| Node {
                        width: px(18),
                        height: px(18),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(9)),
                        ..default()
                    });
                    let close_hovered = close.hovered();
                    if close.clicked() {
                        close.commands().queue(move |world: &mut World| {
                            let _ = world.workbench_mut().close_panel(panel);
                        });
                    }
                    let mut fill = p.elevated;
                    fill[3] *= if close_hovered { 1.0 } else { 0.0 };
                    let mut ink = p.text;
                    ink[3] *= emphasis;
                    close.background_color(look.color(fill)).add(|ui| {
                        ui.ch()
                            .on_change_insert(restyle, || font.clone())
                            .text("×")
                            .text_color(look.color(ink));
                    });
                });
            }
        });
}
