//! bEd's settings: what they are, how they reach the window, and the panel.
//!
//! Each setting writes into `Appearance` when its value changes; one system
//! applies `Appearance` to the theme and workbench surface.
use crate::{panels::PanelPolicy, workspace::Workspace};
use bedtime_ui::{UiContext, UsesTheme, egui, themes};
use bedtime_workbench::{
    Panel, PanelInstance, WindowBlur, WorkbenchSurface,
    persistence::{ReflectPersist, Store, StoreId, Stores, WorkbenchStore},
    setting,
    settings::{Setting, Settings, SettingsAppExt, clear_setting, set_setting},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, path::PathBuf};

const FONT_SIZE: RangeInclusive<f32> = 8.0..=72.0;
const OPACITY: RangeInclusive<f32> = 0.0..=1.0;
const PANEL_RADIUS: RangeInclusive<f32> = 0.0..=32.0;
const PANEL_GAP: RangeInclusive<f32> = 0.0..=32.0;

/// Settings and data shared by every workspace.
pub struct GlobalStore;
impl Store for GlobalStore {}

/// Where the global store lives, if anywhere; shown in the Settings panel.
#[derive(Resource)]
pub struct GlobalLocation(pub Option<PathBuf>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

setting! {
    pub Theme: ThemeChoice = ThemeChoice::Dark, key = "appearance.theme",
    apply = |world, theme| world.resource_mut::<Appearance>().theme = *theme,
}
setting! {
    pub FontSize: f32 = 15.5, key = "appearance.font_size",
    apply = |world, size| world.resource_mut::<Appearance>().font_size = limit(*size, FONT_SIZE, 15.5),
}
setting! {
    pub WindowOpacity: f32 = 1.0, key = "appearance.window_opacity",
    apply = |world, opacity| world.resource_mut::<Appearance>().window_opacity = limit(*opacity, OPACITY, 1.0),
}
setting! {
    pub WindowBlurSetting: bool = false, key = "appearance.window_blur",
    apply = |world, blur| world.resource_mut::<Appearance>().window_blur = *blur,
}
setting! {
    pub PanelOpacity: f32 = 1.0, key = "appearance.panel_opacity",
    apply = |world, opacity| world.resource_mut::<Appearance>().panel_opacity = limit(*opacity, OPACITY, 1.0),
}
setting! {
    pub PanelRadius: f32 = 7.0, key = "appearance.panel_radius",
    apply = |world, radius| world.resource_mut::<Appearance>().panel_radius = limit(*radius, PANEL_RADIUS, 7.0),
}
setting! {
    pub PanelGap: f32 = 5.0, key = "appearance.panel_gap",
    apply = |world, gap| world.resource_mut::<Appearance>().panel_gap = limit(*gap, PANEL_GAP, 5.0),
}

/// Hand-edited files can hold anything; bring numbers into range before use.
fn limit(value: f32, range: RangeInclusive<f32>, default: f32) -> f32 {
    if value.is_nan() {
        default
    } else {
        value.clamp(*range.start(), *range.end())
    }
}

/// How the window and workbench look, fed by the appearance settings.
#[derive(Resource, Clone, PartialEq)]
pub struct Appearance {
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub window_opacity: f32,
    pub window_blur: bool,
    pub panel_opacity: f32,
    pub panel_radius: f32,
    pub panel_gap: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            font_size: FontSize::default(),
            window_opacity: WindowOpacity::default(),
            window_blur: WindowBlurSetting::default(),
            panel_opacity: PanelOpacity::default(),
            panel_radius: PanelRadius::default(),
            panel_gap: PanelGap::default(),
        }
    }
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SettingsPanel>()
            .init_resource::<Appearance>()
            .add_settings_layer::<GlobalStore>()
            .register_setting::<Theme>()
            .register_setting::<FontSize>()
            .register_setting::<WindowOpacity>()
            .register_setting::<WindowBlurSetting>()
            .register_setting::<PanelOpacity>()
            .register_setting::<PanelRadius>()
            .register_setting::<PanelGap>()
            .add_systems(Update, apply_appearance);
    }
}

/// Runs when the appearance changes, and when a surface first gets its theme.
fn apply_appearance(
    appearance: Res<Appearance>,
    mut surfaces: Query<(Entity, &mut WorkbenchSurface, Ref<UsesTheme>)>,
    mut commands: Commands,
) {
    for (surface, mut style, theme) in &mut surfaces {
        if !appearance.is_changed() && !theme.is_added() {
            continue;
        }
        let mut palette = match appearance.theme {
            ThemeChoice::Dark => themes::dark(),
            ThemeChoice::Light => themes::light(),
        };
        palette.palette.background[3] = appearance.window_opacity;
        palette.palette.panel_background[3] = appearance.panel_opacity;
        palette.metrics.font_size = appearance.font_size;
        commands.entity(theme.0).insert(palette);
        commands
            .entity(surface)
            .insert(WindowBlur(appearance.window_blur));
        style.rounding = appearance.panel_radius;
        style.panel_gap = appearance.panel_gap;
    }
}

impl PanelPolicy for SettingsPanel {
    /// There's one set of settings, so any settings panel is equivalent.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Persist)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct SettingsPanel;

impl Panel for SettingsPanel {
    const LABEL: &'static str = "Settings";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            let layer = choose_layer(ui, world);
            let location = if layer == StoreId::of::<GlobalStore>() {
                world
                    .resource::<GlobalLocation>()
                    .0
                    .as_ref()
                    .map(|path| path.join("settings.ron").display().to_string())
                    .unwrap_or("Not saved: no configuration directory".into())
            } else {
                "Saved with this workspace; overrides global settings".into()
            };
            ui.label(egui::RichText::new(location).small().weak());
            let stores = world.resource::<Stores>();
            let store_error = if layer == StoreId::of::<GlobalStore>() {
                stores.error::<GlobalStore>()
            } else {
                stores.error::<WorkbenchStore>()
            };
            let errors = [
                world.resource::<Settings>().error(layer),
                store_error.map(str::to_owned),
            ];
            for error in errors.into_iter().flatten() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Appearance");
                row::<Theme>(ui, world, layer, |ui, theme| {
                    let before = *theme;
                    egui::ComboBox::from_label("Theme")
                        .selected_text(format!("{theme:?}"))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(theme, ThemeChoice::Dark, "Dark");
                            ui.selectable_value(theme, ThemeChoice::Light, "Light");
                        });
                    before != *theme
                });
                row::<FontSize>(ui, world, layer, |ui, size| {
                    ui.add(
                        egui::Slider::new(size, FONT_SIZE)
                            .text("Font size")
                            .suffix(" pt"),
                    )
                    .changed()
                });
                ui.separator();
                row::<WindowOpacity>(ui, world, layer, |ui, opacity| {
                    ui.add(egui::Slider::new(opacity, OPACITY).text("Window opacity"))
                        .changed()
                });
                if cfg!(target_os = "macos") {
                    row::<WindowBlurSetting>(ui, world, layer, |ui, blur| {
                        context.frame(ui).toggle(blur, "Window blur").changed()
                    });
                }
                ui.separator();
                row::<PanelOpacity>(ui, world, layer, |ui, opacity| {
                    ui.add(egui::Slider::new(opacity, OPACITY).text("Panel opacity"))
                        .changed()
                });
                row::<PanelRadius>(ui, world, layer, |ui, radius| {
                    ui.add(egui::Slider::new(radius, PANEL_RADIUS).text("Panel radius"))
                        .changed()
                });
                row::<PanelGap>(ui, world, layer, |ui, gap| {
                    ui.add(egui::Slider::new(gap, PANEL_GAP).text("Panel gap"))
                        .changed()
                });
            });
        });
    }
}

/// Which layer the panel edits: global, or (with a folder open) the workspace.
fn choose_layer(ui: &mut egui::Ui, world: &World) -> StoreId {
    let Some(workspace) = world.get_resource::<Workspace>() else {
        return StoreId::of::<GlobalStore>();
    };
    let id = ui.id().with("settings-layer");
    let mut editing_workspace = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut editing_workspace, false, "Global");
        ui.selectable_value(
            &mut editing_workspace,
            true,
            format!("Workspace: {}", workspace.name()),
        );
    });
    ui.data_mut(|data| data.insert_temp(id, editing_workspace));
    if editing_workspace {
        StoreId::of::<WorkbenchStore>()
    } else {
        StoreId::of::<GlobalStore>()
    }
}

/// One setting's editor, showing the resolved value. Changes go to `layer`.
/// When `layer` holds a value, ↶ clears it so the value is inherited again.
fn row<S: Setting>(
    ui: &mut egui::Ui,
    world: &mut World,
    layer: StoreId,
    edit: impl FnOnce(&mut egui::Ui, &mut S::Value) -> bool,
) {
    let settings = world.resource::<Settings>();
    let mut value = settings.get::<S>();
    let set_here = settings.get_in::<S>(layer).is_some();
    let inherited = !set_here && layer != StoreId::of::<GlobalStore>();
    ui.horizontal(|ui| {
        if edit(ui, &mut value) {
            let _ = set_setting::<S>(world, layer, value);
        }
        if set_here {
            let hint = if layer == StoreId::of::<GlobalStore>() {
                "Reset to default"
            } else {
                "Use the global value"
            };
            if ui.small_button("↶").on_hover_text(hint).clicked() {
                let _ = clear_setting::<S>(world, layer);
            }
        } else if inherited {
            ui.weak("inherited");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::WorkbenchCorePlugin;

    #[test]
    fn appearance_follows_the_settings_and_stays_in_range() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, SettingsPlugin));
        app.update();
        let global = StoreId::of::<GlobalStore>();
        let world = app.world_mut();
        set_setting::<FontSize>(world, global, 20.0).unwrap();
        set_setting::<Theme>(world, global, ThemeChoice::Light).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 20.0);
        assert_eq!(world.resource::<Appearance>().theme, ThemeChoice::Light);
        // As if hand-edited: the value is kept, but what's applied is in range.
        set_setting::<FontSize>(world, global, 500.0).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 72.0);
        clear_setting::<FontSize>(world, global).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 15.5);
    }
}
