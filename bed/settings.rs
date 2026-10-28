//! bEd's settings: what they are, how they reach the window, and the panel.
//!
//! Each setting writes into `Appearance` when its value changes; one system
//! applies `Appearance` to the theme and workbench surface.
use crate::{panels::PanelPolicy, workspace::Workspace};
use bedtime_ui::{
    BuiltinIcon, Metrics, UiContext, egui,
    fonts::{DEFAULT_FAMILY, InstalledFonts},
    theme::{FONT_SIZES, ThemeFolder},
};
use bedtime_workbench::{
    CommandAppExt, ContextCommand, ContextCommandInvocation, ContextCommandResult, Handler,
    KeyChord, Panel, PanelFactory, PanelInstance, WindowBlur, WorkbenchSurface,
    persistence::{ReflectPersist, Store, StoreId, Stores, WorkbenchStore},
    setting,
    settings::{Setting, Settings, SettingsAppExt, clear_setting, set_setting},
};
use bevy::prelude::*;
use std::{ops::RangeInclusive, path::PathBuf};

const OPACITY: RangeInclusive<f32> = 0.0..=1.0;
const PANEL_RADIUS: RangeInclusive<f32> = 0.0..=32.0;
const PANEL_GAP: RangeInclusive<f32> = 0.0..=32.0;

/// Settings and data shared by every workspace.
pub struct GlobalStore;
impl Store for GlobalStore {}

/// Where the global store lives, if anywhere; shown in the Settings panel.
#[derive(Resource)]
pub struct GlobalLocation(pub Option<PathBuf>);

setting! {
    /// A theme's id: the name of a file in the themes folder (`tokyo`).
    pub ColorTheme: String = crate::themes::DEFAULT.into(), key = "appearance.theme",
    apply = |world, theme| world.resource_mut::<Appearance>().theme = theme.clone(),
}
setting! {
    pub FontSize: f32 = 15.5, key = "appearance.font_size",
    apply = |world, size| world.resource_mut::<Appearance>().font_size = limit(*size, FONT_SIZES, 15.5),
}
setting! {
    /// An installed font family; `None` is the bundled Bedfont.
    pub FontFamily: Option<String> = None, key = "appearance.font_family",
    apply = |world, family| world.resource_mut::<Appearance>().font_family = family.clone(),
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

setting! {
    /// Started without a folder, reopen the last one rather than the default
    /// workspace.
    pub ReopenLastWorkspace: bool = false, key = "startup.reopen_last", only_in = [GlobalStore],
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
pub struct Appearance {
    pub theme: String,
    pub font_size: f32,
    pub font_family: Option<String>,
    pub window_opacity: f32,
    pub window_blur: bool,
    pub panel_opacity: f32,
    pub panel_radius: f32,
    pub panel_gap: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ColorTheme::default(),
            font_size: FontSize::default(),
            font_family: FontFamily::default(),
            window_opacity: WindowOpacity::default(),
            window_blur: WindowBlurSetting::default(),
            panel_opacity: PanelOpacity::default(),
            panel_radius: PanelRadius::default(),
            panel_gap: PanelGap::default(),
        }
    }
}

mod keys;

pub struct OpenSettings;
impl ContextCommand for OpenSettings {
    const KEY: &'static str = "app.open_settings";
    const LABEL: &'static str = "Open Settings";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Comma)];
}

fn open_settings(world: &mut World, _: ContextCommandInvocation) -> ContextCommandResult {
    let factory = PanelFactory::of::<SettingsPanel>();
    crate::panels::open_panel::<SettingsPanel>(world, (), factory).map(|_| ())
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SettingsPanel>()
            .init_resource::<Appearance>()
            .add_settings_layer::<GlobalStore>()
            .register_setting::<ColorTheme>()
            .register_setting::<FontSize>()
            .register_setting::<FontFamily>()
            .register_setting::<WindowOpacity>()
            .register_setting::<WindowBlurSetting>()
            .register_setting::<PanelOpacity>()
            .register_setting::<PanelRadius>()
            .register_setting::<PanelGap>()
            .register_setting::<ReopenLastWorkspace>()
            .add_command_handler::<OpenSettings>(Handler::new(open_settings))
            .add_systems(Update, apply_appearance);
    }
}

/// Puts the chosen theme and sizes on each surface: when the appearance or
/// the themes folder changes (a theme file was edited), and on a new surface.
fn apply_appearance(
    appearance: Res<Appearance>,
    folder: Option<Res<ThemeFolder>>,
    mut surfaces: Query<(Entity, &mut WorkbenchSurface)>,
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
            let layer = choose_layer(ui, world);
            let location = location(world, layer);
            ui.add(egui::Label::new(egui::RichText::new(&location).small().weak()).truncate());
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
            ui.add_space(4.0);
            ui.separator();

            // Both ways: rows have fixed columns, so a narrow panel scrolls
            // sideways rather than clipping controls.
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Leaves room for the value box beside each slider.
                    ui.spacing_mut().slider_width = CONTROL_WIDTH - 76.0;
                    section(ui, "Appearance");
                    crate::themes::row(ui, world, context, layer);
                    font_family_row(ui, world, context, layer);
                    row::<FontSize>(ui, world, context, layer, "Font size", |ui, size| {
                        slider(ui, size, FONT_SIZES, " pt", 1)
                    });

                    section(ui, "Window");
                    row::<WindowOpacity>(ui, world, context, layer, "Opacity", |ui, opacity| {
                        slider(ui, opacity, OPACITY, "", 2)
                    });
                    if cfg!(target_os = "macos") {
                        row::<WindowBlurSetting>(ui, world, context, layer, "Blur", |ui, blur| {
                            context.frame(ui).toggle(blur, "").changed()
                        });
                    }

                    section(ui, "Panels");
                    row::<PanelOpacity>(ui, world, context, layer, "Opacity", |ui, opacity| {
                        slider(ui, opacity, OPACITY, "", 2)
                    });
                    row::<PanelRadius>(ui, world, context, layer, "Corner radius", |ui, radius| {
                        slider(ui, radius, PANEL_RADIUS, "", 1)
                    });
                    row::<PanelGap>(ui, world, context, layer, "Gap", |ui, gap| {
                        slider(ui, gap, PANEL_GAP, "", 1)
                    });

                    if layer == StoreId::of::<GlobalStore>() {
                        section(ui, "Startup");
                        row::<ReopenLastWorkspace>(
                            ui,
                            world,
                            context,
                            layer,
                            "Reopen last folder",
                            |ui, reopen| {
                                let response = context.frame(ui).toggle(reopen, "");
                                response
                                    .on_hover_text("When bEd is started without a folder")
                                    .changed()
                            },
                        );
                    }

                    crate::effects::section(ui, world, context, layer);

                    section(ui, "Keyboard Shortcuts");
                    keys::section(ui, world, context, layer);
                });
        });
    }
}

/// The width of the label column, so every section's controls line up.
pub(crate) const LABEL_WIDTH: f32 = 210.0;
/// The width of combo boxes, sliders and key buttons.
pub(crate) const CONTROL_WIDTH: f32 = 240.0;

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

pub(crate) fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.heading(title);
    ui.add_space(2.0);
}

/// One row of the panel: a label (then `marker`, such as an override warning),
/// a control and then `state`, each in its own column at a fixed height so
/// rows line up across sections.
pub(crate) fn columns<R>(
    ui: &mut egui::Ui,
    label: &str,
    marker: impl FnOnce(&mut egui::Ui),
    control: impl FnOnce(&mut egui::Ui) -> R,
    state: impl FnOnce(&mut egui::Ui),
) -> R {
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let row_height = ui.spacing().interact_size.y;
    let row = egui::vec2(ui.available_width(), row_height);
    ui.allocate_ui_with_layout(row, layout, |ui| {
        ui.set_min_height(row_height);
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, row_height), layout, |ui| {
            ui.set_min_size(egui::vec2(LABEL_WIDTH, row_height));
            ui.spacing_mut().item_spacing.x = 6.0;
            // Leave room for the marker after a long label.
            ui.scope(|ui| {
                ui.set_max_width(LABEL_WIDTH - 28.0);
                ui.add(egui::Label::new(label).truncate());
            });
            marker(ui);
        });
        let inner = ui
            .allocate_ui_with_layout(egui::vec2(CONTROL_WIDTH, row_height), layout, |ui| {
                ui.set_min_size(egui::vec2(CONTROL_WIDTH, row_height));
                control(ui)
            })
            .inner;
        // As wide as it needs: past the panel's edge it scrolls sideways.
        state(ui);
        inner
    })
    .inner
}

/// A slider with its value shown to `decimals` places. Only the text is
/// rounded: egui writes a slider's value back every frame, and rounding it
/// there (`fixed_decimals`) would change a setting just by showing it.
pub(crate) fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
) -> bool {
    let slider = egui::Slider::new(value, range)
        .suffix(suffix)
        .custom_formatter(move |value, _| format!("{value:.decimals$}"));
    ui.add(slider).changed()
}

/// A combo box filling the control column.
fn combo(ui: &mut egui::Ui, id: &str, selected: String, options: impl FnOnce(&mut egui::Ui)) {
    egui::ComboBox::from_id_salt(id)
        .width(CONTROL_WIDTH)
        .selected_text(selected)
        .show_ui(ui, options);
}

/// Bedfont and the installed monospaced families: the editor and terminal
/// need fixed-width text.
fn font_family_row(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let chosen = world.resource::<Settings>().get_at::<FontFamily>(layer);
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
    row_with_note::<FontFamily>(ui, world, context, layer, "Font", note, |ui, family| {
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

/// Which layer the panel edits: global, or (with a folder open) the workspace.
fn choose_layer(ui: &mut egui::Ui, world: &World) -> StoreId {
    let Some(workspace) = world
        .get_resource::<Workspace>()
        .filter(|workspace| !workspace.default)
    else {
        return StoreId::of::<GlobalStore>();
    };
    let id = ui.id().with("settings-layer");
    let mut editing_workspace = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    // Wrap rather than overflow: an overflowing row would widen everything
    // after it, and the panel would stop scrolling sideways.
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut editing_workspace, false, "Global");
        let label = format!("Workspace: {}", workspace.name());
        ui.selectable_value(&mut editing_workspace, true, label);
    });
    ui.data_mut(|data| data.insert_temp(id, editing_workspace));
    if editing_workspace {
        StoreId::of::<WorkbenchStore>()
    } else {
        StoreId::of::<GlobalStore>()
    }
}

/// One setting's row, showing the resolved value. Changes go to `layer`.
pub(crate) fn row<S: Setting>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    layer: StoreId,
    label: &str,
    edit: impl FnOnce(&mut egui::Ui, &mut S::Value) -> bool,
) {
    row_with_note::<S>(ui, world, context, layer, label, None, edit);
}

/// A setting's row with a note after it, such as why its value isn't used.
fn row_with_note<S: Setting>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    layer: StoreId,
    label: &str,
    note: Option<String>,
    edit: impl FnOnce(&mut egui::Ui, &mut S::Value) -> bool,
) {
    let settings = world.resource::<Settings>();
    // The layer's own view: what it would be without the layers above it.
    let mut value = settings.get_at::<S>(layer);
    let set_here = settings.get_in::<S>(layer).is_some();
    let overridden = settings.overridden_above::<S>(layer);
    let mut reset = false;
    let changed = columns(
        ui,
        label,
        |ui| overridden_marker(ui, context, overridden),
        |ui| edit(ui, &mut value),
        |ui| {
            reset = reset_button(ui, context, layer, set_here);
            if let Some(note) = note {
                ui.weak(note);
            }
        },
    );
    if changed {
        let _ = set_setting::<S>(world, layer, value);
    }
    if reset {
        let _ = clear_setting::<S>(world, layer);
    }
}

/// After a label: a warning icon when a layer above the one being edited sets
/// this too, so changes here won't show until it's cleared there.
pub(crate) fn overridden_marker(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    above: Option<StoreId>,
) {
    let Some(above) = above else {
        return;
    };
    let hint = if above == StoreId::of::<WorkbenchStore>() {
        "Set differently in this workspace, which uses its own value. \
         Clear it on the Workspace tab to use this one."
    } else {
        "A higher settings layer sets this, so its value is used."
    };
    let tint = bedtime_ui::theme::color(context.theme.palette.warning);
    let mut frame = context.frame(ui);
    let side = context.metrics.font_size;
    let icon = frame.icon(BuiltinIcon::CircleAlert, side, tint);
    icon.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Overridden"));
    frame.tooltip(&icon, hint);
}

/// When `layer` holds a value: a button that clears it (default, or the global
/// value for a workspace). Returns whether it was clicked. Rows without one
/// show the value from below.
pub(crate) fn reset_button(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    layer: StoreId,
    set_here: bool,
) -> bool {
    if !set_here {
        return false;
    }
    let hint = if layer == StoreId::of::<GlobalStore>() {
        "Reset to default"
    } else {
        "Use the global value"
    };
    let mut frame = context.frame(ui);
    let response = frame.icon_button_sized(BuiltinIcon::RotateCcw, 14.0);
    frame.tooltip(&response, hint);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench::WorkbenchCorePlugin;

    #[test]
    fn appearance_follows_the_settings_and_stays_in_range() {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, SettingsPlugin))
            .add_settings_layer::<WorkbenchStore>();
        app.update();
        let global = StoreId::of::<GlobalStore>();
        let world = app.world_mut();
        set_setting::<FontSize>(world, global, 20.0).unwrap();
        set_setting::<ColorTheme>(world, global, "tokyo".into()).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 20.0);
        assert_eq!(world.resource::<Appearance>().theme, "tokyo");
        // As if hand-edited: the value is kept, but what's applied is in range.
        set_setting::<FontSize>(world, global, 500.0).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 72.0);
        clear_setting::<FontSize>(world, global).unwrap();
        assert_eq!(world.resource::<Appearance>().font_size, 15.5);
        // A workspace can choose its own font.
        let workspace = StoreId::of::<WorkbenchStore>();
        set_setting::<FontFamily>(world, global, Some("Menlo".into())).unwrap();
        set_setting::<FontFamily>(world, workspace, Some("PT Mono".into())).unwrap();
        assert_eq!(
            world.resource::<Appearance>().font_family.as_deref(),
            Some("PT Mono")
        );
        clear_setting::<FontFamily>(world, workspace).unwrap();
        assert_eq!(
            world.resource::<Appearance>().font_family.as_deref(),
            Some("Menlo")
        );
    }

    #[test]
    fn the_open_settings_keys_open_one_settings_panel() {
        use bedtime_workbench::{BindingInput, BindingPress, InvocationContext};
        let mut app = App::new();
        app.add_plugins((
            WorkbenchCorePlugin,
            SettingsPlugin,
            crate::layouts::LayoutsPlugin,
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
