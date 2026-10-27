//! Global settings: the resource, how it's applied to the window, and its panel.
use crate::{
    config::{self, ConfigFile, ConfigPlugin},
    panels::PanelPolicy,
};
use bedtime_ui::{UiContext, UsesTheme, egui, themes};
use bedtime_workbench::{
    ErrorDialog, Panel, PanelInstance, WindowBlur, WorkbenchSurface, request_error_dialog,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, path::PathBuf};

const FONT_SIZE: RangeInclusive<f32> = 8.0..=72.0;
const OPACITY: RangeInclusive<f32> = 0.0..=1.0;
const PANEL_RADIUS: RangeInclusive<f32> = 0.0..=32.0;
const PANEL_GAP: RangeInclusive<f32> = 0.0..=32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

/// Fields missing from the file take their default, so adding a setting
/// doesn't invalidate existing files.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub font_size: f32,
    pub window_opacity: f32,
    pub window_blur: bool,
    pub panel_opacity: f32,
    pub panel_radius: f32,
    pub panel_gap: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            font_size: 15.5,
            window_opacity: 1.0,
            window_blur: false,
            panel_opacity: 1.0,
            panel_radius: 7.0,
            panel_gap: 5.0,
        }
    }
}

impl Settings {
    /// Hand-edited files can hold anything; bring numbers into range before use.
    /// The resource itself is left alone so the file isn't rewritten behind the user's back.
    fn limited(&self) -> Self {
        fn limit(value: f32, range: RangeInclusive<f32>, default: f32) -> f32 {
            if value.is_nan() {
                default
            } else {
                value.clamp(*range.start(), *range.end())
            }
        }
        let default = Self::default();
        Self {
            theme: self.theme,
            font_size: limit(self.font_size, FONT_SIZE, default.font_size),
            window_opacity: limit(self.window_opacity, OPACITY, default.window_opacity),
            window_blur: self.window_blur,
            panel_opacity: limit(self.panel_opacity, OPACITY, default.panel_opacity),
            panel_radius: limit(self.panel_radius, PANEL_RADIUS, default.panel_radius),
            panel_gap: limit(self.panel_gap, PANEL_GAP, default.panel_gap),
        }
    }
}

pub struct SettingsPlugin {
    pub path: Option<PathBuf>,
}

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ConfigPlugin::<Settings>::new(self.path.clone()))
            .add_systems(Update, (apply, report_errors));
    }
}

/// Runs when settings change, and when a surface first gets its theme.
fn apply(
    settings: Res<Settings>,
    mut surfaces: Query<(Entity, &mut WorkbenchSurface, Ref<UsesTheme>)>,
    mut commands: Commands,
) {
    let s = settings.limited();
    for (surface, mut style, theme) in &mut surfaces {
        if !settings.is_changed() && !theme.is_added() {
            continue;
        }
        let mut palette = match s.theme {
            ThemeChoice::Dark => themes::dark(),
            ThemeChoice::Light => themes::light(),
        };
        palette.palette.background[3] = s.window_opacity;
        palette.palette.panel_background[3] = s.panel_opacity;
        palette.metrics.font_size = s.font_size;
        commands.entity(theme.0).insert(palette);
        commands.entity(surface).insert(WindowBlur(s.window_blur));
        style.rounding = s.panel_radius;
        style.panel_gap = s.panel_gap;
    }
}

fn report_errors(world: &mut World, mut reported: Local<Option<String>>) {
    let error = world
        .resource::<ConfigFile<Settings>>()
        .error()
        .map(str::to_owned);
    if error.is_some() && error != *reported {
        request_error_dialog(
            world,
            ErrorDialog {
                title: "Settings".into(),
                message: "Settings couldn't be read or saved. Details are in the Settings panel."
                    .into(),
                details: error.clone(),
            },
        );
    }
    *reported = error;
}

impl PanelPolicy for SettingsPanel {
    /// There's one set of settings, so any settings panel is equivalent.
    type Key = ();
    fn key(_: &World, _: Entity) {}
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct SettingsPanel;

impl Panel for SettingsPanel {
    const LABEL: &'static str = "Settings";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            let file = world.resource::<ConfigFile<Settings>>();
            let path = file.path().map(|path| path.display().to_string());
            let error = file.error().map(str::to_owned);

            ui.label(
                egui::RichText::new(path.unwrap_or("Not saved: no configuration directory".into()))
                    .small()
                    .weak(),
            );
            if let Some(error) = error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.button("Reload from disk").clicked() {
                    config::reload::<Settings>(world);
                }
            }
            ui.separator();

            let mut settings = world.resource::<Settings>().clone();
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Appearance");
                egui::ComboBox::from_label("Theme")
                    .selected_text(format!("{:?}", settings.theme))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.theme, ThemeChoice::Dark, "Dark");
                        ui.selectable_value(&mut settings.theme, ThemeChoice::Light, "Light");
                    });
                ui.add(
                    egui::Slider::new(&mut settings.font_size, FONT_SIZE)
                        .text("Font size")
                        .suffix(" pt"),
                );
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut settings.window_opacity, OPACITY).text("Window opacity"),
                );
                if cfg!(target_os = "macos") {
                    context
                        .frame(ui)
                        .toggle(&mut settings.window_blur, "Window blur");
                }
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut settings.panel_opacity, OPACITY).text("Panel opacity"),
                );
                ui.add(
                    egui::Slider::new(&mut settings.panel_radius, PANEL_RADIUS)
                        .text("Panel radius"),
                );
                ui.add(egui::Slider::new(&mut settings.panel_gap, PANEL_GAP).text("Panel gap"));
            });

            // Assign only on a real edit, so change detection means something.
            if settings != *world.resource::<Settings>() {
                world.insert_resource(settings);
            }
        });
    }
}
