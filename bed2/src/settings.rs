//! Application settings: explicit global values, per-value workspace overrides.
use bedtime_ui2::{UiContext, UsesTheme, egui, themes};
use bedtime_workbench2::{Panel, PanelInstance, WindowBlur, WorkbenchSurface};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Autosave {
    Never,
    ChangeFocus,
    #[default]
    Timer,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
}

// One field list defines defaults, inheritance and late-read merging. All three
// must agree about the unit of an override: a value, never an entire section.
macro_rules! settings {
    ($($field:ident: $ty:ty = $value:expr),* $(,)?) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct Settings { $(pub $field: $ty,)* }
        impl Default for Settings { fn default() -> Self { Self { $($field: $value,)* } } }
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct SettingsOverrides { $(pub $field: Option<$ty>,)* }
        impl SettingsOverrides {
            pub fn resolve(&self, global: &Settings) -> Settings {
                Settings { $($field: self.$field.clone().unwrap_or_else(|| global.$field.clone()),)* }
            }
            pub fn merge_loaded(baseline: &Self, live: &Self, disk: &Self) -> Self {
                Self { $($field: if live.$field == baseline.$field { disk.$field.clone() } else { live.$field.clone() },)* }
            }
        }
        impl Settings {
            pub fn merge_loaded(baseline: &Self, live: &Self, disk: &Self) -> Self {
                Self { $($field: if live.$field == baseline.$field { disk.$field.clone() } else { live.$field.clone() },)* }
            }
        }
    }
}
settings! {
    autosave: Autosave = Autosave::Timer,
    autosave_timeout: f64 = 1.0,
    theme: ThemeChoice = ThemeChoice::Dark,
    font_family: String = bedtime_ui2::fonts::DEFAULT_FAMILY.into(),
    font_size: f32 = 15.5,
    window_opacity: f32 = 1.0,
    window_blur: bool = false,
    panel_opacity: f32 = 1.0,
    panel_radius: f32 = 7.0,
    panel_gap: f32 = 5.0,
    file_tree_indentation: f32 = 16.0,
    effects_enabled: bool = false,
    scanline_intensity: f32 = 0.06,
    vignette_intensity: f32 = 0.04,
    bloom_intensity: f32 = 0.025,
    noise_intensity: f32 = 0.0,
    curvature_intensity: f32 = 0.0,
    pixel_grid_intensity: f32 = 0.0,
    burn_in_intensity: f32 = 0.0,
    pulse_intensity: f32 = 0.0,
    line_numbers: bool = true,
    smart_wrapping: bool = true,
    smart_indentation: bool = true,
    language_servers_enabled: bool = true,
    language_servers: std::collections::BTreeMap<String, bedtime_lsp2::ServerConfig> = [("rust".into(), bedtime_lsp2::ServerConfig::default())].into(),
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.language_servers.len() > 32 {
            return Err("At most 32 language server definitions are supported".into());
        }
        for (language, server) in &self.language_servers {
            if language.is_empty()
                || language.len() > 64
                || !language
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                return Err("Invalid language server language ID".into());
            }
            server.validate()?;
        }
        if self.font_family.trim().is_empty()
            || self.font_family.len() > 256
            || self.font_family.chars().any(char::is_control)
        {
            return Err("Font family must be a nonempty name of at most 256 bytes".into());
        }
        if !self.font_size.is_finite() || !(8.0..=72.0).contains(&self.font_size) {
            return Err("Font size must be between 8 and 72 points".into());
        }
        if !self.autosave_timeout.is_finite() || !(0.01..=86400.0).contains(&self.autosave_timeout)
        {
            return Err("Autosave timeout must be between 0.01 and 86400 seconds".into());
        }
        for (value, max) in [
            (self.window_opacity, 1.0),
            (self.panel_opacity, 1.0),
            (self.panel_radius, 32.0),
            (self.panel_gap, 32.0),
            (self.file_tree_indentation, 64.0),
            (self.scanline_intensity, 1.0),
            (self.vignette_intensity, 1.0),
            (self.bloom_intensity, 1.0),
            (self.noise_intensity, 1.0),
            (self.curvature_intensity, 1.0),
            (self.pixel_grid_intensity, 1.0),
            (self.burn_in_intensity, 1.0),
            (self.pulse_intensity, 1.0),
        ] {
            if !value.is_finite() || !(0.0..=max).contains(&value) {
                return Err("Invalid appearance setting".into());
            }
        }
        Ok(())
    }
}
impl SettingsOverrides {
    pub fn validate(&self) -> Result<(), String> {
        self.resolve(&Settings::default()).validate()
    }
}
/// Startup policy is global: no workspace exists yet when it is resolved.
#[derive(Resource, Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StartupPreferences {
    pub open_last_workspace: bool,
    pub last_workspace: Option<PathBuf>,
}

#[derive(Resource, Default)]
pub struct GlobalPreferences(pub Settings);
#[derive(Component, Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspacePreferences(pub SettingsOverrides);

pub fn effective_settings(world: &World, workspace: Entity) -> Settings {
    let global = world
        .get_resource::<GlobalPreferences>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    world
        .get::<WorkspacePreferences>(workspace)
        .map(|p| p.0.resolve(&global))
        .unwrap_or(global)
}

// Concrete library-facing policy, derived from settings rather than persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorPreferences {
    pub line_numbers: bool,
    pub wrap: bool,
    pub smart_indent: bool,
    pub read_only: bool,
}
impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            line_numbers: true,
            wrap: true,
            smart_indent: true,
            read_only: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct FileTreePreferences {
    pub read_only: bool,
    pub indent_size: f32,
}
impl Default for FileTreePreferences {
    fn default() -> Self {
        Self {
            read_only: false,
            indent_size: 16.0,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPreferences {
    pub project_directory: bool,
}
impl Default for TerminalPreferences {
    fn default() -> Self {
        Self {
            project_directory: true,
        }
    }
}
pub struct Preferences {
    pub editor: Option<EditorPreferences>,
    pub filetree: Option<FileTreePreferences>,
    pub terminal: Option<TerminalPreferences>,
}
pub fn effective_preferences(world: &World, workspace: Entity) -> Preferences {
    let s = effective_settings(world, workspace);
    Preferences {
        editor: Some(EditorPreferences {
            line_numbers: s.line_numbers,
            wrap: s.smart_wrapping,
            smart_indent: s.smart_indentation,
            read_only: false,
        }),
        filetree: Some(FileTreePreferences {
            read_only: false,
            indent_size: s.file_tree_indentation,
        }),
        terminal: Some(TerminalPreferences::default()),
    }
}
#[derive(Component)]
struct AppliedEditorPreferences(EditorPreferences, Option<bedtime_workbench2::DocumentId>);
#[derive(Component)]
struct AppliedFileTreePreferences(FileTreePreferences);
#[derive(Component)]
struct AppliedTerminalPreferences(TerminalPreferences, Option<PathBuf>);
#[derive(Resource, Default)]
struct AppliedAppearance(Option<Settings>);
pub struct SettingsPlugin;
impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GlobalPreferences>()
            .init_resource::<AppliedAppearance>()
            .add_systems(
                Update,
                update_preferences.before(bedtime_filetree2::FileTreeSystems::Update),
            )
            .add_systems(
                PreUpdate,
                crate::fonts::apply
                    .after(bedtime_ui2::UiSystems::Configure)
                    .before(bedtime_ui2::UiSystems::Draw),
            );
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_settings", inspect)
                .register_automation_command("app.set_settings", set);
        }
        crate::storage::install(app);
        crate::keybindings::install(app);
    }
}
pub fn configure(app: &mut App) {
    crate::storage::configure(app);
}
pub fn flush(world: &mut World) -> Result<bool, String> {
    crate::storage::flush(world)
}
fn apply_appearance(world: &mut World) {
    let active = world
        .query_filtered::<Entity, With<bedtime_workbench2::ActiveWorkspace>>()
        .iter(world)
        .next();
    let desired = active
        .map(|w| effective_settings(world, w))
        .unwrap_or_else(|| world.resource::<GlobalPreferences>().0.clone());
    if world.resource::<AppliedAppearance>().0.as_ref() == Some(&desired) {
        return;
    }
    let surfaces: Vec<_> = world
        .query_filtered::<(Entity, &UsesTheme), With<WorkbenchSurface>>()
        .iter(world)
        .map(|(e, t)| (e, t.0))
        .collect();
    if surfaces.is_empty() {
        return;
    }
    for (surface, theme) in surfaces {
        let mut palette = match desired.theme {
            ThemeChoice::Dark => themes::dark(),
            ThemeChoice::Light => themes::light(),
        };
        palette.palette.background[3] = desired.window_opacity;
        palette.palette.panel_background[3] = desired.panel_opacity;
        palette.metrics.font_size = desired.font_size;
        world.entity_mut(theme).insert(palette);
        let mut style = world.get_mut::<WorkbenchSurface>(surface).unwrap();
        style.rounding = desired.panel_radius;
        style.panel_gap = desired.panel_gap;
        world
            .entity_mut(surface)
            .insert(WindowBlur(desired.window_blur));
    }
    world.resource_mut::<AppliedAppearance>().0 = Some(desired);
}
pub fn configure_editor(world: &mut World, panel: Entity) {
    let Some(workspace) = world
        .get::<bedtime_workbench2::InWorkspace>(panel)
        .map(|owner| owner.0)
    else {
        return;
    };
    let mut desired = effective_preferences(world, workspace)
        .editor
        .unwrap_or_default();
    desired.read_only |= !crate::storage::ready(world, workspace);
    let document = world
        .get::<bedtime_workbench2::DocumentView>(panel)
        .map(|view| view.document());
    if world
        .get::<AppliedEditorPreferences>(panel)
        .is_none_or(|applied| applied.0 != desired || applied.1 != document)
        && let Some(mut editor) =
            world.get_mut::<bedtime_editor2::workbench::WorkbenchEditor>(panel)
    {
        editor.view.settings.line_numbers = desired.line_numbers;
        editor.view.settings.wrap = desired.wrap;
        editor.view.settings.smart_indent = desired.smart_indent;
        editor.view.settings.read_only = desired.read_only;
        world
            .entity_mut(panel)
            .insert(AppliedEditorPreferences(desired, document));
    }
}

fn update_preferences(world: &mut World) {
    crate::storage::update(world);
    apply_appearance(world);
    let panels: Vec<_> = world
        .query::<(Entity, &bedtime_workbench2::InWorkspace)>()
        .iter(world)
        .map(|(entity, owner)| (entity, owner.0))
        .collect();
    for (panel, workspace) in panels {
        let preferences = effective_preferences(world, workspace);
        let ready = crate::storage::ready(world, workspace);
        if world
            .get::<bedtime_editor2::workbench::WorkbenchEditor>(panel)
            .is_some()
        {
            configure_editor(world, panel);
        }
        if world
            .get::<bedtime_filetree2::FileTreePanel>(panel)
            .is_some()
        {
            let mut desired = preferences.filetree.unwrap_or_default();
            desired.read_only |= !ready;
            if world
                .get::<AppliedFileTreePreferences>(panel)
                .is_none_or(|applied| applied.0 != desired)
            {
                world.entity_mut(panel).insert((
                    bedtime_filetree2::FileTreeSettings {
                        read_only: desired.read_only,
                        indent_size: desired.indent_size,
                    },
                    AppliedFileTreePreferences(desired),
                ));
            }
        }
        if world
            .get::<bedtime_terminal2::TerminalPanel>(panel)
            .is_some()
        {
            let desired = preferences.terminal.unwrap_or_default();
            let directory = if desired.project_directory {
                crate::files::workspace_directory(world, workspace)
            } else {
                None
            };
            if world
                .get::<AppliedTerminalPreferences>(panel)
                .is_none_or(|applied| applied.0 != desired || applied.1 != directory)
            {
                world
                    .get_mut::<bedtime_terminal2::TerminalPanel>(panel)
                    .unwrap()
                    .options
                    .working_directory = directory.clone();
                world
                    .entity_mut(panel)
                    .insert(AppliedTerminalPreferences(desired, directory));
            }
        }
    }
}

#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct SettingsPanel;
#[derive(Clone, Copy, Default, PartialEq)]
enum Section {
    #[default]
    General,
    Appearance,
    Editor,
}
impl Panel for SettingsPanel {
    const KEY: &'static str = "bed.settings";
    const LABEL: &'static str = "Settings";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        let workspace = world
            .get::<bedtime_workbench2::InWorkspace>(panel)
            .map(|o| o.0);
        let scope_id = ui.id().with((panel, "settings-scope"));
        let section_id = ui.id().with((panel, "settings-section"));
        let mut local = ui.data_mut(|d| d.get_temp::<bool>(scope_id).unwrap_or(false));
        let mut section = ui.data_mut(|d| d.get_temp::<Section>(section_id).unwrap_or_default());
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Workspace");
                ui.add_enabled_ui(workspace.is_some(), |ui| {
                    toggle(ui, &mut local);
                });
                ui.label("Global");
            });
        });
        crate::storage::draw_errors(ui, world);
        if let Some(workspace) = workspace {
            crate::session::draw_status(ui, world, workspace);
        }
        ui.separator();
        let global = world.resource::<GlobalPreferences>().0.clone();
        let mut overrides = workspace
            .and_then(|w| world.get::<WorkspacePreferences>(w))
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let mut values = if local {
            overrides.resolve(&global)
        } else {
            global.clone()
        };
        let before = values.clone();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_min_width(110.0);
                for (name, value) in [
                    ("General", Section::General),
                    ("Appearance", Section::Appearance),
                    ("Editor", Section::Editor),
                ] {
                    ui.selectable_value(&mut section, value, name);
                }
            });
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .id_salt((panel, "settings-content"))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(320.0);
                        macro_rules! row {
                            ($field:ident, $draw:expr) => {{
                                ui.horizontal(|ui| {
                                    if ($draw)(ui, &mut values.$field).changed() && local {
                                        overrides.$field = Some(values.$field.clone());
                                    }
                                    if local
                                        && overrides.$field.is_some()
                                        && ui
                                            .small_button("↶")
                                            .on_hover_text("Use global value")
                                            .clicked()
                                    {
                                        overrides.$field = None;
                                        values.$field = global.$field.clone();
                                    }
                                });
                            }};
                        }
                        match section {
                            Section::General => {
                                if !local {
                                    ui.checkbox(
                                        &mut world
                                            .resource_mut::<StartupPreferences>()
                                            .open_last_workspace,
                                        "Open last workspace on open",
                                    );
                                }
                                row!(autosave, |ui: &mut egui::Ui, value: &mut Autosave| {
                                    ui.label("Autosave");
                                    let before = *value;
                                    let mut response = egui::ComboBox::from_id_salt("autosave")
                                        .selected_text(match value {
                                            Autosave::Never => "Never",
                                            Autosave::ChangeFocus => "Change Focus",
                                            Autosave::Timer => "Timer",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(value, Autosave::Never, "Never");
                                            ui.selectable_value(
                                                value,
                                                Autosave::ChangeFocus,
                                                "Change Focus",
                                            );
                                            ui.selectable_value(value, Autosave::Timer, "Timer");
                                        })
                                        .response;
                                    if before != *value {
                                        response.mark_changed();
                                    }
                                    response
                                });
                                if values.autosave == Autosave::Timer {
                                    row!(autosave_timeout, |ui: &mut egui::Ui, value: &mut f64| ui
                                        .add(
                                            egui::DragValue::new(value)
                                                .range(0.01..=86400.0)
                                                .speed(0.1)
                                                .suffix(" s")
                                                .prefix("Autosave timeout ")
                                        ));
                                }
                            }
                            Section::Appearance => {
                                row!(font_family, |ui: &mut egui::Ui, value: &mut String| {
                                    ui.label("Font family");
                                    let before = value.clone();
                                    let mut response = egui::ComboBox::from_id_salt("font-family")
                                        .selected_text(value.as_str())
                                        .height(300.0)
                                        .show_ui(ui, |ui| {
                                            for family in crate::fonts::available() {
                                                ui.selectable_value(value, family.clone(), family);
                                            }
                                        })
                                        .response;
                                    if !crate::fonts::available().contains(value) {
                                        ui.label("Unavailable; using Bedfont");
                                    }
                                    if before != *value {
                                        response.mark_changed();
                                    }
                                    response
                                });
                                row!(font_size, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 8.0..=72.0)
                                        .text("Font size")
                                        .suffix(" pt")
                                ));
                                row!(theme, |ui: &mut egui::Ui, value: &mut ThemeChoice| {
                                    ui.label("Theme");
                                    let before = *value;
                                    let mut response = egui::ComboBox::from_id_salt("theme")
                                        .selected_text(match value {
                                            ThemeChoice::Dark => "Dark",
                                            ThemeChoice::Light => "Light",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(value, ThemeChoice::Dark, "Dark");
                                            ui.selectable_value(value, ThemeChoice::Light, "Light");
                                        })
                                        .response;
                                    if before != *value {
                                        response.mark_changed();
                                    }
                                    response
                                });
                                row!(window_opacity, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=1.0).text("Window opacity")
                                ));
                                #[cfg(target_os = "macos")]
                                row!(window_blur, |ui: &mut egui::Ui, value: &mut bool| ui
                                    .checkbox(value, "Window blur"));
                                row!(panel_opacity, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=1.0).text("Panel opacity")
                                ));
                                row!(panel_radius, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=32.0)
                                        .text("Panel border radius")
                                ));
                                row!(panel_gap, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=32.0).text("Panel gap")));
                                row!(
                                    file_tree_indentation,
                                    |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                        egui::Slider::new(value, 0.0..=64.0)
                                            .text("File tree indentation")
                                    )
                                );
                                ui.separator();
                                row!(effects_enabled, |ui: &mut egui::Ui, value: &mut bool| ui
                                    .checkbox(value, "Shader effects"));
                                // Keep the controls visible and editable while disabled: values
                                // are independent preferences and survive toggling the effects.
                                row!(scanline_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=1.0).text("Scanlines")));
                                row!(vignette_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=1.0).text("Vignette")));
                                row!(bloom_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=1.0).text("Bloom")));
                                row!(noise_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(
                                        egui::Slider::new(value, 0.0..=1.0).text("Static noise")
                                    ));
                                row!(curvature_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=1.0).text("Curvature")));
                                row!(
                                    pixel_grid_intensity,
                                    |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                        egui::Slider::new(value, 0.0..=1.0).text("Pixel grid")
                                    )
                                );
                                row!(burn_in_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(
                                        egui::Slider::new(value, 0.0..=1.0)
                                            .text("Phosphor persistence")
                                    ));
                                row!(pulse_intensity, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(
                                        egui::Slider::new(value, 0.0..=1.0)
                                            .text("Brightness pulse")
                                    ));
                            }
                            Section::Editor => {
                                row!(line_numbers, |ui: &mut egui::Ui, value: &mut bool| ui
                                    .checkbox(value, "Line numbers"));
                                row!(smart_wrapping, |ui: &mut egui::Ui, value: &mut bool| ui
                                    .checkbox(value, "Smart wrapping"));
                                row!(smart_indentation, |ui: &mut egui::Ui, value: &mut bool| ui
                                    .checkbox(value, "Smart indentation"));
                                ui.separator();
                                row!(
                                    language_servers_enabled,
                                    |ui: &mut egui::Ui, value: &mut bool| ui
                                        .checkbox(value, "Language servers")
                                );
                                row!(
                                    language_servers,
                                    |ui: &mut egui::Ui,
                                     servers: &mut std::collections::BTreeMap<
                                        String,
                                        bedtime_lsp2::ServerConfig,
                                    >| {
                                        let mut changed = false;
                                        let mut response = ui
                                            .vertical(|ui| {
                                                for (language, server) in servers.iter_mut() {
                                                    let id =
                                                        ui.id().with(("server-draft", language));
                                                    let (baseline, mut draft) = ui.data_mut(|d| {
                                                        d.get_temp::<(
                                                            bedtime_lsp2::ServerConfig,
                                                            bedtime_lsp2::ServerConfig,
                                                        )>(
                                                            id
                                                        )
                                                        .unwrap_or_else(|| {
                                                            (server.clone(), server.clone())
                                                        })
                                                    });
                                                    if baseline != *server {
                                                        draft = server.clone();
                                                    }
                                                    ui.label(language);
                                                    ui.text_edit_singleline(&mut draft.program);
                                                    for argument in &mut draft.arguments {
                                                        ui.text_edit_singleline(argument);
                                                    }
                                                    ui.horizontal(|ui| {
                                                        if ui.small_button("Add argument").clicked()
                                                            && draft.arguments.len() < 128
                                                        {
                                                            draft.arguments.push(String::new());
                                                        }
                                                        if !draft.arguments.is_empty()
                                                            && ui
                                                                .small_button(
                                                                    "Remove last argument",
                                                                )
                                                                .clicked()
                                                        {
                                                            draft.arguments.pop();
                                                        }
                                                        if ui
                                                            .add_enabled(
                                                                draft != *server
                                                                    && draft.validate().is_ok(),
                                                                egui::Button::new("Apply"),
                                                            )
                                                            .clicked()
                                                        {
                                                            *server = draft.clone();
                                                            changed = true;
                                                        }
                                                    });
                                                    if let Err(error) = draft.validate() {
                                                        ui.colored_label(
                                                            ui.visuals().error_fg_color,
                                                            error,
                                                        );
                                                    }
                                                    ui.data_mut(|d| {
                                                        d.insert_temp(id, (server.clone(), draft))
                                                    });
                                                }
                                            })
                                            .response;
                                        if changed {
                                            response.mark_changed();
                                        }
                                        response
                                    }
                                );
                            }
                        }
                    });
                });
        });
        if local {
            if let Some(workspace) = workspace {
                world
                    .entity_mut(workspace)
                    .insert(WorkspacePreferences(overrides));
            }
        } else if values != before {
            world.resource_mut::<GlobalPreferences>().0 = values;
        }
        ui.data_mut(|d| {
            d.insert_temp(scope_id, local);
            d.insert_temp(section_id, section);
        });
    }
}
fn toggle(ui: &mut egui::Ui, value: &mut bool) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 18.0), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
    }
    let visuals = ui.style().interact(&response);
    ui.painter().rect_filled(
        rect,
        9.0,
        if *value {
            ui.visuals().selection.bg_fill
        } else {
            visuals.bg_fill
        },
    );
    let center = egui::pos2(
        if *value {
            rect.right() - 9.0
        } else {
            rect.left() + 9.0
        },
        rect.center().y,
    );
    ui.painter()
        .circle_filled(center, 6.0, visuals.fg_stroke.color);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            *value,
            "Workspace settings",
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_overrides_are_per_value_and_reset_resumes_inheritance() {
        let mut world = World::new();
        world.insert_resource(GlobalPreferences::default());
        let first = world
            .spawn(WorkspacePreferences(SettingsOverrides {
                line_numbers: Some(false),
                ..default()
            }))
            .id();
        let second = world.spawn(WorkspacePreferences::default()).id();
        world.resource_mut::<GlobalPreferences>().0.smart_wrapping = false;
        assert!(!effective_settings(&world, first).line_numbers);
        assert!(!effective_settings(&world, first).smart_wrapping);
        assert!(effective_settings(&world, second).line_numbers);
        world
            .get_mut::<WorkspacePreferences>(first)
            .unwrap()
            .0
            .line_numbers = None;
        assert!(effective_settings(&world, first).line_numbers);
        world.resource_mut::<GlobalPreferences>().0.line_numbers = false;
        assert!(!effective_settings(&world, first).line_numbers);
    }
    #[test]
    fn invalid_settings_are_rejected_at_the_boundary() {
        for size in [0.0, 7.9, 72.1, f32::INFINITY, f32::NAN] {
            assert!(
                SettingsOverrides {
                    font_size: Some(size),
                    ..default()
                }
                .validate()
                .is_err()
            );
        }
        for name in ["".into(), "  ".into(), "bad\nfont".into(), "x".repeat(257)] {
            assert!(
                SettingsOverrides {
                    font_family: Some(name),
                    ..default()
                }
                .validate()
                .is_err()
            );
        }
        for field in [
            "scanline_intensity",
            "vignette_intensity",
            "bloom_intensity",
            "noise_intensity",
            "curvature_intensity",
            "pixel_grid_intensity",
            "burn_in_intensity",
            "pulse_intensity",
        ] {
            for value in [-0.01, 1.01] {
                let mut data = serde_json::json!({});
                data[field] = value.into();
                let settings: Settings = serde_json::from_value(data.clone()).unwrap();
                assert!(settings.validate().is_err(), "{field}: {value}");
                let overrides: SettingsOverrides = serde_json::from_value(data).unwrap();
                assert!(overrides.validate().is_err());
            }
        }
        for timeout in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert!(
                Settings {
                    autosave_timeout: timeout,
                    ..default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            SettingsOverrides {
                panel_opacity: Some(2.0),
                ..default()
            }
            .validate()
            .is_err()
        );
        assert!(ron::from_str::<Settings>("(unknown: true)").is_err());
    }
    #[test]
    fn old_preferences_default_effects_off_and_overrides_inherit_individual_values() {
        let old: Settings = ron::from_str("(panel_gap: 9.0)").unwrap();
        assert_eq!(old.font_family, "Bedfont");
        assert_eq!(old.font_size, 15.5);
        assert!(!old.effects_enabled);
        let global = Settings {
            effects_enabled: true,
            curvature_intensity: 0.8,
            bloom_intensity: 0.2,
            ..old
        };
        let mut overrides = SettingsOverrides {
            curvature_intensity: Some(0.4),
            ..default()
        };
        let local = overrides.resolve(&global);
        assert!(local.effects_enabled);
        assert_eq!(local.curvature_intensity, 0.4);
        assert_eq!(local.bloom_intensity, 0.2);
        overrides.curvature_intensity = None;
        assert_eq!(overrides.resolve(&global), global);
    }
    #[test]
    fn font_settings_round_trip_merge_and_inherit_independently() {
        let baseline = Settings::default();
        let global = Settings {
            font_family: "An installed family".into(),
            font_size: 18.0,
            ..baseline.clone()
        };
        let global: Settings = ron::from_str(&ron::to_string(&global).unwrap()).unwrap();
        let local = SettingsOverrides {
            font_family: Some("A workspace family".into()),
            ..default()
        };
        let mut local: SettingsOverrides = ron::from_str(&ron::to_string(&local).unwrap()).unwrap();
        assert_eq!(local.resolve(&global).font_family, "A workspace family");
        assert_eq!(local.resolve(&global).font_size, 18.0);
        local.font_family = None;
        local.font_size = Some(24.0);
        assert_eq!(local.resolve(&global).font_family, "An installed family");
        assert_eq!(local.resolve(&global).font_size, 24.0);
        let live = Settings {
            font_family: "Live family".into(),
            ..baseline.clone()
        };
        let merged = Settings::merge_loaded(&baseline, &live, &global);
        assert_eq!(merged.font_family, "Live family");
        assert_eq!(merged.font_size, 18.0);
        let merged = SettingsOverrides::merge_loaded(
            &SettingsOverrides::default(),
            &local,
            &SettingsOverrides {
                font_family: Some("Loaded family".into()),
                font_size: Some(30.0),
                ..default()
            },
        );
        assert_eq!(merged.font_family.as_deref(), Some("Loaded family"));
        assert_eq!(merged.font_size, Some(24.0));
    }
    #[test]
    fn workspace_switch_applies_appearance_without_changing_global_values() {
        let mut app = App::new();
        app.add_plugins(bedtime_workbench2::WorkbenchCorePlugin)
            .init_resource::<GlobalPreferences>()
            .init_resource::<AppliedAppearance>();
        let first = bedtime_workbench2::Workspaces::new(app.world_mut()).open("First");
        let second = bedtime_workbench2::Workspaces::new(app.world_mut()).open("Second");
        app.world_mut()
            .entity_mut(second.entity())
            .insert(WorkspacePreferences(SettingsOverrides {
                theme: Some(ThemeChoice::Light),
                window_opacity: Some(0.5),
                panel_gap: Some(12.0),
                font_size: Some(22.0),
                ..default()
            }));
        let theme = app.world_mut().spawn(themes::dark()).id();
        let surface = app
            .world_mut()
            .spawn((WorkbenchSurface::default(), UsesTheme(theme)))
            .id();
        bedtime_workbench2::Workspaces::new(app.world_mut())
            .activate(first)
            .unwrap();
        apply_appearance(app.world_mut());
        assert_eq!(
            app.world()
                .get::<bedtime_ui2::Theme>(theme)
                .unwrap()
                .metrics
                .font_size,
            15.5
        );
        assert_eq!(
            app.world()
                .get::<WorkbenchSurface>(surface)
                .unwrap()
                .panel_gap,
            5.0
        );
        bedtime_workbench2::Workspaces::new(app.world_mut())
            .activate(second)
            .unwrap();
        apply_appearance(app.world_mut());
        assert_eq!(
            app.world()
                .get::<bedtime_ui2::Theme>(theme)
                .unwrap()
                .metrics
                .font_size,
            22.0
        );
        assert_eq!(
            app.world()
                .get::<WorkbenchSurface>(surface)
                .unwrap()
                .panel_gap,
            12.0
        );
        assert_eq!(
            app.world()
                .get::<bedtime_ui2::Theme>(theme)
                .unwrap()
                .palette
                .background[3],
            0.5
        );
        assert_eq!(
            app.world().resource::<GlobalPreferences>().0.theme,
            ThemeChoice::Dark
        );
        bedtime_workbench2::Workspaces::new(app.world_mut())
            .activate(first)
            .unwrap();
        apply_appearance(app.world_mut());
        assert_eq!(
            app.world()
                .get::<bedtime_ui2::Theme>(theme)
                .unwrap()
                .palette
                .background[3],
            1.0
        );
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    let fonts = crate::fonts::inspect(world);
    let workspaces: Vec<_> = world
        .query_filtered::<Entity, With<bedtime_workbench2::Workspace>>()
        .iter(world)
        .collect();
    Ok(
        serde_json::json!({ "global": world.resource::<GlobalPreferences>().0,
        "fonts": fonts,
        "startup": world.resource::<StartupPreferences>(),
        "workspaces": workspaces.into_iter().map(|workspace| serde_json::json!({
            "workspace": workspace.to_bits().to_string(), "effective": effective_settings(world, workspace),
            "overrides": world.get::<WorkspacePreferences>(workspace).map(|p| &p.0),
        })).collect::<Vec<_>>() }),
    )
}
#[cfg(feature = "automation")]
fn set(world: &mut World, arguments: serde_json::Value) -> bedtime_automation2::AutomationResult {
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before editing settings".into());
    }
    let values = arguments
        .get("values")
        .and_then(|v| v.as_object())
        .ok_or("values must be a settings object")?;
    if let Some(workspace) = arguments.get("workspace") {
        let bits = workspace
            .as_str()
            .ok_or("workspace must be an entity ID")?
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        let workspace = Entity::try_from_bits(bits).ok_or("Invalid workspace ID")?;
        if world
            .get::<bedtime_workbench2::Workspace>(workspace)
            .is_none()
        {
            return Err("Workspace is unavailable".into());
        }
        let current = world
            .get::<WorkspacePreferences>(workspace)
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let mut merged = serde_json::to_value(current).map_err(|e| e.to_string())?;
        for (key, value) in values {
            merged
                .as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
        let settings: SettingsOverrides =
            serde_json::from_value(merged).map_err(|e| e.to_string())?;
        settings.validate()?;
        world
            .entity_mut(workspace)
            .insert(WorkspacePreferences(settings));
    } else {
        let mut merged = serde_json::to_value(&world.resource::<GlobalPreferences>().0)
            .map_err(|e| e.to_string())?;
        for (key, value) in values {
            merged
                .as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
        let settings: Settings = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        settings.validate()?;
        world.resource_mut::<GlobalPreferences>().0 = settings;
    }
    inspect(world, serde_json::Value::Null)
}
