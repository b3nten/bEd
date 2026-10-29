//! Panels. Each is an ordinary workbench panel component; its content is its
//! own immediate tree, attached to a host node the shell puts in the area.
//! Content reads and writes app state directly: no messages, no observers,
//! no per-widget sync systems.
use crate::look::{FONT, Look, THEMES, theme};
use bedtime_workbench::{Panel, PanelInstance};
use bevy::ecs::system::SystemParam;
use bevy::feathers::constants::icons;
use bevy::feathers::controls::{
    FeathersButton, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    FeathersSlider, FeathersToggleSwitch,
};
use bevy::feathers::display::{caption, icon};
use bevy::prelude::*;
use bevy::scene::bsn;
use bevy::text::FontSize;
use bevy::ui_widgets::{ScrollArea, SliderPrecision, SliderStep};
use bevy_immediate::{
    Imm,
    attach::{BevyImmediateAttachPlugin, ImmediateAttach},
    ui::{
        CapsUiFeathers, activated::ImmUiActivated, checked::ImmUiChecked, clicked::ImmUiClicked,
        interaction::ImmUiInteraction, look::ImmUiLook, slider_value::ImmUiSliderValue,
        text::ImmUiText,
    },
};

const LABEL_WIDTH: f32 = 220.0;
const CONTROL_WIDTH: f32 = 260.0;
const ROW_HEIGHT: f32 = 34.0;

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct SettingsPanel;
impl Panel for SettingsPanel {
    const LABEL: &'static str = "Settings";
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct NotesPanel;
impl Panel for NotesPanel {
    const LABEL: &'static str = "Notes";
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct WorkspacesPanel;
impl Panel for WorkspacesPanel {
    const LABEL: &'static str = "Workspaces";
}

/// Which content a panel shows, read by the shell.
pub type Content = (Has<SettingsPanel>, Has<NotesPanel>, Has<WorkspacesPanel>);

/// Content roots: one immediate tree per host node.
#[derive(Component)]
pub struct SettingsContent;
#[derive(Component)]
pub struct NotesContent;
#[derive(Component)]
pub struct WorkspacesContent;

pub fn plugin(app: &mut App) {
    app.init_resource::<Workspaces>().add_plugins((
        BevyImmediateAttachPlugin::<CapsUiFeathers, SettingsContent>::new(),
        BevyImmediateAttachPlugin::<CapsUiFeathers, NotesContent>::new(),
        BevyImmediateAttachPlugin::<CapsUiFeathers, WorkspacesContent>::new(),
    ));
}

/// The node a panel's content is built in, below the tab bar.
pub fn host(
    ui: &mut Imm<CapsUiFeathers>,
    panel: Entity,
    (settings, notes, workspaces): (bool, bool, bool),
) {
    let id = ("content", panel);
    if settings {
        ui.ch_id(id)
            .on_spawn_insert(|| (content_node(), SettingsContent));
    } else if notes {
        ui.ch_id(id)
            .on_spawn_insert(|| (content_node(), NotesContent));
    } else if workspaces {
        ui.ch_id(id)
            .on_spawn_insert(|| (content_node(), WorkspacesContent));
    }
}

fn content_node() -> (Node, ScrollArea, ScrollPosition) {
    (
        Node {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            padding: UiRect::axes(px(14), px(12)),
            row_gap: px(4),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        ScrollArea,
        ScrollPosition::default(),
    )
}

#[derive(SystemParam)]
pub struct Params<'w> {
    look: ResMut<'w, Look>,
    assets: Res<'w, AssetServer>,
}

impl Params<'_> {
    fn font(&self, scale: f32) -> TextFont {
        TextFont {
            font: self.assets.load(FONT).into(),
            font_size: FontSize::Px(self.look.font_size * scale),
            ..default()
        }
    }
}

/// Text in bed's font and color, kept current when the look changes.
fn text(ui: &mut Imm<CapsUiFeathers>, params: &Params, value: &str, scale: f32, dim: bool) {
    let p = &params.look.theme.palette;
    ui.ch()
        .on_change_insert(params.look.is_changed(), || params.font(scale))
        .text(value)
        .text_color(params.look.color(if dim { p.muted } else { p.text }));
}

fn heading(ui: &mut Imm<CapsUiFeathers>, params: &Params, value: &str) {
    ui.ch_id(("heading", value))
        .on_spawn_insert(|| Node {
            margin: UiRect::new(px(0), px(0), px(18), px(6)),
            ..default()
        })
        .add(|ui| text(ui, params, value, 1.3, false));
}

/// Label column, control column: the same grid as bed's Settings panel.
fn row(
    ui: &mut Imm<CapsUiFeathers>,
    params: &Params,
    label: &str,
    control: impl FnOnce(&mut Imm<CapsUiFeathers>),
) {
    ui.ch_id(("row", label))
        .on_spawn_insert(|| Node {
            height: px(ROW_HEIGHT),
            align_items: AlignItems::Center,
            ..default()
        })
        .add(|ui| {
            ui.ch()
                .on_spawn_insert(|| Node {
                    width: px(LABEL_WIDTH),
                    flex_shrink: 0.0,
                    ..default()
                })
                .add(|ui| text(ui, params, label, 1.0, false));
            ui.ch()
                .on_spawn_insert(|| Node {
                    width: px(CONTROL_WIDTH),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .add(control);
        });
}

fn slider(ui: &mut Imm<CapsUiFeathers>, value: &mut f32, min: f32, max: f32, step: f32) {
    ui.ch()
        .on_spawn_apply_scene(move || {
            bsn! {
                @FeathersSlider { @min: min, @max: max }
                SliderStep(step)
                SliderPrecision(2)
            }
        })
        .slider(value);
}

impl ImmediateAttach<CapsUiFeathers> for SettingsContent {
    type Params = Params<'static>;

    fn construct(ui: &mut Imm<CapsUiFeathers>, params: &mut Params) {
        // Widgets edit a copy; the resource only changes (and only restyles
        // the shell) when a value really changed.
        let mut look = (*params.look).clone();
        settings(ui, params, &mut look);
        params.look.set_if_neq(look);
    }
}

fn settings(ui: &mut Imm<CapsUiFeathers>, params: &Params, look: &mut Look) {
    heading(ui, params, "Appearance");
    row(ui, params, "Theme", |ui| {
        ui.ch()
            .on_spawn_apply_scene(|| bsn! { @FeathersMenu Node { flex_grow: 1.0 } })
            .add(|ui| {
                ui.ch()
                    .on_spawn_apply_scene(|| {
                        bsn! {
                            @FeathersMenuButton { @arrow: false }
                            Node { flex_grow: 1.0 }
                        }
                    })
                    .add(|ui| {
                        text(ui, params, &look.theme.name, 1.0, false);
                        ui.ch().on_spawn_insert(|| Node {
                            flex_grow: 1.0,
                            ..default()
                        });
                        ui.ch().on_spawn_apply_scene(|| icon(icons::CHEVRON_DOWN));
                    });
                ui.ch()
                    .on_spawn_apply_scene(|| bsn! { @FeathersMenuPopup })
                    .add(|ui| {
                        for index in 0..THEMES.len() {
                            let name = theme(index).name;
                            if ui
                                .ch_id(("theme", index))
                                .on_spawn_apply_scene(move || {
                                    bsn! { @FeathersMenuItem { @caption: bsn! { @caption(name) } } }
                                })
                                .activated()
                            {
                                look.theme_index = index;
                                look.theme = theme(index);
                            }
                        }
                    });
            });
    });
    row(ui, params, "Font size", |ui| {
        slider(ui, &mut look.font_size, 8.0, 32.0, 0.5)
    });

    heading(ui, params, "Window");
    row(ui, params, "Opacity", |ui| {
        slider(ui, &mut look.opacity, 0.2, 1.0, 0.05)
    });
    row(ui, params, "Blur", |ui| {
        ui.ch()
            .on_spawn_apply_scene(|| bsn! { @FeathersToggleSwitch })
            .checked(&mut look.blur);
    });

    heading(ui, params, "Panels");
    row(ui, params, "Corner radius", |ui| {
        slider(ui, &mut look.rounding, 0.0, 20.0, 0.5)
    });
    row(ui, params, "Gap", |ui| {
        slider(ui, &mut look.gap, 0.0, 24.0, 0.5)
    });
    row(ui, params, "Tab bar", |ui| {
        ui.ch()
            .on_spawn_apply_scene(|| bsn! { @FeathersToggleSwitch })
            .checked(&mut look.tab_bar);
    });
    row(ui, params, "", |ui| {
        if ui
            .ch()
            .on_spawn_apply_scene(|| {
                bsn! { @FeathersButton { @caption: bsn! { @caption("Reset to defaults") } } }
            })
            .activated()
        {
            *look = Look::default();
        }
    });
}

impl ImmediateAttach<CapsUiFeathers> for NotesContent {
    type Params = Params<'static>;

    fn construct(ui: &mut Imm<CapsUiFeathers>, params: &mut Params) {
        let params = &*params;
        heading(ui, params, "Notes");
        for line in [
            "The shell and every panel here are immediate-mode functions over the \
             workbench model: areas, tabs and the selected panel come from the ECS \
             each frame, and the Settings panel edits one resource directly.",
            "Drag the Gap or Corner radius sliders and the layout follows; pick a \
             theme and feathers' widgets are recolored from bed's palette.",
        ] {
            ui.ch_id(line)
                .on_spawn_insert(|| Node {
                    margin: UiRect::bottom(px(12)),
                    ..default()
                })
                .add(|ui| text(ui, params, line, 1.0, true));
        }
    }
}

/// Stand-in for bed's recent workspaces.
#[derive(Resource)]
pub struct Workspaces {
    names: Vec<&'static str>,
    open: usize,
}
impl Default for Workspaces {
    fn default() -> Self {
        Self {
            names: vec!["Default workspace", "bed", "bevy", "zed", "notes"],
            open: 0,
        }
    }
}

#[derive(SystemParam)]
pub struct WorkspacesParams<'w> {
    base: Params<'w>,
    workspaces: ResMut<'w, Workspaces>,
}

impl ImmediateAttach<CapsUiFeathers> for WorkspacesContent {
    type Params = WorkspacesParams<'static>;

    fn construct(ui: &mut Imm<CapsUiFeathers>, params: &mut WorkspacesParams) {
        let WorkspacesParams { base, workspaces } = params;
        let look = &*base.look;
        let p = &look.theme.palette;
        heading(ui, base, "Recent");
        let mut open = workspaces.open;
        for (index, name) in workspaces.names.iter().enumerate() {
            let mut item = ui.ch_id(("workspace", index)).on_spawn_insert(|| Node {
                height: px(ROW_HEIGHT),
                padding: UiRect::horizontal(px(10)),
                align_items: AlignItems::Center,
                ..default()
            });
            let hovered = item.hovered();
            if item.clicked() {
                open = index;
            }
            let fill = if index == open {
                look.color(p.surface)
            } else if hovered {
                look.mix(p.panel_background, p.text, 0.08)
            } else {
                Color::NONE
            };
            item.node_mut(|node| node.border_radius = BorderRadius::all(px(look.rounding)))
                .background_color(fill)
                .add(|ui| text(ui, base, name, 1.0, index != open));
        }
        if workspaces.open != open {
            workspaces.open = open;
        }
    }
}
