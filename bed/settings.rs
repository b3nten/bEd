//! bEd's settings: what they are, how they reach the window, and the panel.
//!
//! Each setting writes into `AppearanceSettings` when its value changes; one system
//! applies `AppearanceSettings` to the theme and workbench surface.
use crate::{
    panels::PanelPolicy,
    workspace::{GlobalStore, Workspace},
};
use bedtime_ui::{
    Metrics, UiContext, egui,
    fonts::{DEFAULT_FAMILY, InstalledFonts},
    theme::{FONT_SIZES, ThemeFolder},
};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, KeyChord, LayoutSurface, Panel,
    PanelFactory, PanelInstance, WindowBlur,
    persistence::{ReflectPersist, StoreId, Stores, WorkbenchStore},
    settings::{
        Setting, SettingsAppExt, SettingsWorldExt, settings,
        ui::{SettingsUiAppExt, combo, heading, layer_tabs, row, sections},
    },
    ui::keybindings_section,
};
use bevy::prelude::*;
use std::{ops::RangeInclusive, path::PathBuf};

const OPACITY: RangeInclusive<f32> = 0.0..=1.0;
const PANEL_RADIUS: RangeInclusive<f32> = 0.0..=32.0;
const PANEL_GAP: RangeInclusive<f32> = 0.0..=32.0;

/// Where the global store lives, if anywhere; shown in the Settings panel.
#[derive(Resource)]
pub struct GlobalLocation(pub Option<PathBuf>);

settings! {
    "Appearance" {
        /// A theme's id: the name of a file in the themes folder (`tokyo`).
        pub ColorTheme: String {
            default: crate::themes::DEFAULT.into(),
            key: "appearance.theme",
            label: "Theme",
            ui: custom(crate::themes::row),
            apply: |world, theme| world.resource_mut::<AppearanceSettings>().theme = theme.clone(),
        }
        /// An installed font family; `None` is the bundled Bedfont.
        pub FontFamily: Option<String> {
            default: None,
            key: "appearance.font_family",
            label: "Font",
            ui: custom(font_family_row),
            apply: |world, family| world.resource_mut::<AppearanceSettings>().font_family = family.clone(),
        }
        pub FontSize: f32 {
            default: 15.5,
            key: "appearance.font_size",
            ui: slider(FONT_SIZES).suffix(" pt").decimals(1),
            apply: |world, size| world.resource_mut::<AppearanceSettings>().font_size = limit(*size, FONT_SIZES, 15.5),
        }
    }
    "Window" {
        pub WindowOpacity: f32 {
            default: 1.0,
            key: "appearance.window_opacity",
            label: "Opacity",
            ui: slider(OPACITY),
            apply: |world, opacity| world.resource_mut::<AppearanceSettings>().window_opacity = limit(*opacity, OPACITY, 1.0),
        }
        pub WindowBlurEnabled: bool {
            default: false,
            key: "appearance.window_blur",
            label: "Blur",
            // Only macOS blurs behind windows.
            visible: |_, _| cfg!(target_os = "macos"),
            apply: |world, blur| world.resource_mut::<AppearanceSettings>().window_blur = *blur,
        }
    }
    "Panels" {
        pub PanelOpacity: f32 {
            default: 1.0,
            key: "appearance.panel_opacity",
            label: "Opacity",
            ui: slider(OPACITY),
            apply: |world, opacity| world.resource_mut::<AppearanceSettings>().panel_opacity = limit(*opacity, OPACITY, 1.0),
        }
        pub PanelRadius: f32 {
            default: 7.0,
            key: "appearance.panel_radius",
            label: "Corner radius",
            ui: slider(PANEL_RADIUS).decimals(1),
            apply: |world, radius| world.resource_mut::<AppearanceSettings>().panel_radius = limit(*radius, PANEL_RADIUS, 7.0),
        }
        pub PanelGap: f32 {
            default: 5.0,
            key: "appearance.panel_gap",
            label: "Gap",
            ui: slider(PANEL_GAP).decimals(1),
            apply: |world, gap| world.resource_mut::<AppearanceSettings>().panel_gap = limit(*gap, PANEL_GAP, 5.0),
        }
    }
    "Editor" {
        /// Wrap long lines at the editor's width (View ▸ Word Wrap changes one editor).
        pub WrapLines: bool {
            default: false,
            key: "editor.wrap",
            label: "Wrap lines",
            apply: |world, wrap| world.resource_mut::<bedtime_code::EditorDefaults>().wrap = *wrap,
        }
        pub LineNumbers: bool {
            default: true,
            key: "editor.line_numbers",
            label: "Line numbers",
            apply: |world, shown| world.resource_mut::<bedtime_code::EditorDefaults>().line_numbers = *shown,
        }
    }
    "Files" {
        /// Dot files and the like (a tree's menu changes that tree).
        pub ShowHiddenFiles: bool {
            default: true,
            key: "files.show_hidden",
            label: "Show hidden files",
            apply: |world, shown| world.resource_mut::<bedtime_filetree::FileTreeDefaults>().show_hidden = *shown,
        }
        /// Files Git ignores.
        pub ShowIgnoredFiles: bool {
            default: true,
            key: "files.show_ignored",
            label: "Show ignored files",
            apply: |world, shown| world.resource_mut::<bedtime_filetree::FileTreeDefaults>().show_ignored = *shown,
        }
    }
}

/// Hand-edited files can hold anything; bring numbers into range before use.
pub(crate) fn limit(value: f32, range: RangeInclusive<f32>, default: f32) -> f32 {
    if value.is_nan() {
        default
    } else {
        value.clamp(*range.start(), *range.end())
    }
}

/// How the window and workbench look, fed by the appearance settings.
#[derive(Resource, Clone, PartialEq)]
pub struct AppearanceSettings {
    pub theme: String,
    pub font_size: f32,
    pub font_family: Option<String>,
    pub window_opacity: f32,
    pub window_blur: bool,
    pub panel_opacity: f32,
    pub panel_radius: f32,
    pub panel_gap: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ColorTheme::default(),
            font_size: FontSize::default(),
            font_family: FontFamily::default(),
            window_opacity: WindowOpacity::default(),
            window_blur: WindowBlurEnabled::default(),
            panel_opacity: PanelOpacity::default(),
            panel_radius: PanelRadius::default(),
            panel_gap: PanelGap::default(),
        }
    }
}

pub struct OpenSettings;
impl Action for OpenSettings {
    const KEY: &'static str = "app.open_settings";
    const LABEL: &'static str = "Open Settings";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Comma)];
}

fn open_settings(world: &mut World, _: ActionInvocation) -> ActionResult {
    let factory = PanelFactory::of::<SettingsPanel>();
    crate::panels::open_panel::<SettingsPanel>(world, (), factory).map(|_| ())
}

pub struct BedSettingsPlugin;

impl Plugin for BedSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SettingsPanel>()
            .init_resource::<AppearanceSettings>()
            // Startup is workbench's (reopening the last workspace).
            .settings_sections([
                "Appearance",
                "Window",
                "Panels",
                "Editor",
                "Files",
                "Startup",
                "Effects",
            ])
            .init_resource::<bedtime_code::EditorDefaults>()
            .register_setting::<WrapLines>()
            .register_setting::<LineNumbers>()
            .init_resource::<bedtime_filetree::FileTreeDefaults>()
            .register_setting::<ShowHiddenFiles>()
            .register_setting::<ShowIgnoredFiles>()
            .register_setting::<ColorTheme>()
            .register_setting::<FontFamily>()
            .register_setting::<FontSize>()
            .register_setting::<WindowOpacity>()
            .register_setting::<WindowBlurEnabled>()
            .register_setting::<PanelOpacity>()
            .register_setting::<PanelRadius>()
            .register_setting::<PanelGap>()
            .add_action_handler::<OpenSettings>(Handler::new(open_settings))
            .add_systems(Update, apply_appearance);
    }
}

/// Puts the chosen theme and sizes on each surface: when the appearance or
/// the themes folder changes (a theme file was edited), and on a new surface.
fn apply_appearance(
    appearance: Res<AppearanceSettings>,
    folder: Option<Res<ThemeFolder>>,
    mut surfaces: Query<(Entity, &mut LayoutSurface)>,
    mut commands: Commands,
) {
    let folder_changed = folder.as_ref().is_some_and(|folder| folder.is_changed());
    for (surface, mut style) in &mut surfaces {
        if !appearance.is_changed() && !folder_changed && !style.is_added() {
            continue;
        }
        let mut theme = folder
            .as_ref()
            .and_then(|folder| folder.get(&appearance.theme))
            .cloned()
            .unwrap_or_else(bedtime_ui::Theme::dark);
        theme.palette.background[3] = appearance.window_opacity;
        theme.palette.panel_background[3] = appearance.panel_opacity;
        let metrics = Metrics {
            font_size: appearance.font_size,
            font_family: appearance.font_family.clone(),
            ..default()
        };
        commands
            .entity(surface)
            .insert((theme, metrics, WindowBlur(appearance.window_blur)));
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
            let Some(layer) = layer_tabs(ui, world, |layer, label| {
                match world.get_resource::<Workspace>() {
                    Some(workspace) if layer == StoreId::of::<WorkbenchStore>() => {
                        format!("{label}: {}", workspace.name())
                    }
                    _ => label.to_owned(),
                }
            }) else {
                return;
            };
            let location = location(world, layer);
            ui.add(egui::Label::new(egui::RichText::new(&location).small().weak()).truncate());
            let stores = world.resource::<Stores>();
            let store_error = if layer == StoreId::of::<GlobalStore>() {
                stores.error::<GlobalStore>()
            } else {
                stores.error::<WorkbenchStore>()
            };
            let errors = [
                world.settings().error(layer),
                store_error.map(str::to_owned),
            ];
            for error in errors.into_iter().flatten() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(4.0);
            ui.separator();

            // Both ways: rows have fixed columns, so a narrow panel scrolls
            // sideways rather than clipping controls.
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    sections(ui, world, context, layer);
                    heading(ui, "Keyboard Shortcuts");
                    keybindings_section(ui, world, context, layer);
                });
        });
    }
}

/// Where the layer being edited is saved, shown under the layer tabs.
fn location(world: &World, layer: StoreId) -> String {
    if layer != StoreId::of::<GlobalStore>() {
        return "Saved with this workspace. Values set here override the global ones.".into();
    }
    let Some(directory) = &world.resource::<GlobalLocation>().0 else {
        return "Not saved: there's no configuration directory".into();
    };
    let path = directory.join("settings.ron");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

/// Bedfont and the installed monospaced families: the editor and terminal
/// need fixed-width text.
fn font_family_row(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let chosen = world.settings().get_at::<FontFamily>(layer);
    let installed = world.get_resource::<InstalledFonts>();
    let families: Vec<String> = installed
        .map(|fonts| fonts.families())
        .unwrap_or_default()
        .iter()
        .filter(|family| family.monospaced)
        .map(|family| family.name.clone())
        .collect();
    let note = match (installed, &chosen) {
        (Some(fonts), _) if fonts.loading() => Some("Loading installed fonts…".to_owned()),
        (Some(fonts), _) if fonts.error().is_some() => fonts.error().map(str::to_owned),
        (_, Some(family)) if !families.contains(family) => {
            Some(format!("Not installed; using {DEFAULT_FAMILY}"))
        }
        _ => None,
    };
    row::<FontFamily>(ui, world, context, layer, note, |ui, family| {
        let before = family.clone();
        let shown = family.clone().unwrap_or(DEFAULT_FAMILY.into());
        combo(ui, "font", shown, |ui| {
            ui.selectable_value(family, None, DEFAULT_FAMILY);
            for name in families {
                let label = name.clone();
                ui.selectable_value(family, Some(name), label);
            }
        });
        before != *family
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::WorkbenchCorePlugins;

    #[test]
    fn appearance_follows_the_settings_and_stays_in_range() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, BedSettingsPlugin))
            .add_settings_layer::<GlobalStore>()
            .add_settings_layer::<WorkbenchStore>();
        app.update();
        let global = StoreId::of::<GlobalStore>();
        let world = app.world_mut();
        world.settings_mut().set::<FontSize>(global, 20.0).unwrap();
        world
            .settings_mut()
            .set::<ColorTheme>(global, "tokyo".into())
            .unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 20.0);
        assert_eq!(world.resource::<AppearanceSettings>().theme, "tokyo");
        // As if hand-edited: the value is kept, but what's applied is in range.
        world.settings_mut().set::<FontSize>(global, 500.0).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 72.0);
        world.settings_mut().clear::<FontSize>(global).unwrap();
        assert_eq!(world.resource::<AppearanceSettings>().font_size, 15.5);
        // A workspace can choose its own font.
        let workspace = StoreId::of::<WorkbenchStore>();
        world
            .settings_mut()
            .set::<FontFamily>(global, Some("Menlo".into()))
            .unwrap();
        world
            .settings_mut()
            .set::<FontFamily>(workspace, Some("PT Mono".into()))
            .unwrap();
        assert_eq!(
            world
                .resource::<AppearanceSettings>()
                .font_family
                .as_deref(),
            Some("PT Mono")
        );
        world.settings_mut().clear::<FontFamily>(workspace).unwrap();
        assert_eq!(
            world
                .resource::<AppearanceSettings>()
                .font_family
                .as_deref(),
            Some("Menlo")
        );
    }

    #[test]
    fn the_open_settings_keys_open_one_settings_panel() {
        use bedtime_workbench::{BindingInput, BindingPress, InvocationContext};
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugins,
            BedSettingsPlugin,
            crate::layouts::LayoutSlotsPlugin,
        ));
        app.update();
        let chord = KeyChord::primary(KeyCode::Comma);
        for _ in 0..2 {
            app.world_mut()
                .resource_mut::<BindingInput>()
                .0
                .push(BindingPress {
                    chord,
                    repeat: false,
                    context: InvocationContext::Focused,
                });
            app.update();
        }
        let world = app.world_mut();
        let panels = world.query::<&SettingsPanel>().iter(world).count();
        assert_eq!(panels, 1);
    }
}
