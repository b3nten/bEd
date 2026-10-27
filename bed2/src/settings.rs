//! Application settings: explicit global values, per-value workspace overrides.
use crate::persistence::ReflectGlobalPersist;
use crate::persistence::ReflectProjectPersist;
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
    language_servers: std::collections::BTreeMap<String, bedtime_lsp2::ServerConfig> = default_language_servers(),
}

// Protocol IDs, including TSX, map to identical launch configurations so the
// workbench shares one TypeScript process within a workspace.
fn default_language_servers() -> std::collections::BTreeMap<String, bedtime_lsp2::ServerConfig> {
    [
        ("rust", "rust-analyzer", vec![]),
        ("go", "gopls", vec![]),
        ("typescript", "typescript-language-server", vec!["--stdio"]),
        (
            "typescriptreact",
            "typescript-language-server",
            vec!["--stdio"],
        ),
        ("python", "pyright-langserver", vec!["--stdio"]),
    ]
    .into_iter()
    .map(|(language, program, arguments)| {
        (
            language.into(),
            bedtime_lsp2::ServerConfig {
                program: program.into(),
                arguments: arguments.into_iter().map(String::from).collect(),
                ..default()
            },
        )
    })
    .collect()
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
#[derive(Resource, Default, Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, GlobalPersist)]
#[serde(default, deny_unknown_fields)]
pub struct StartupPreferences {
    pub open_last_workspace: bool,
    #[serde(with = "bedtime_workbench2::native_path::option")]
    pub last_workspace: Option<PathBuf>,
}

#[derive(Resource, Default, Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, GlobalPersist)]
pub struct GlobalPreferences(pub Settings);
#[derive(Resource, Default, Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, ProjectPersist)]
pub struct ProjectPreferences(pub SettingsOverrides);

pub fn effective_settings(world: &World) -> Settings {
    let global = world
        .get_resource::<GlobalPreferences>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    world
        .get_resource::<ProjectPreferences>()
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

pub fn effective_preferences(world: &World) -> Preferences {
    let s = effective_settings(world);
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
        register_menu(app);
        app.init_resource::<GlobalPreferences>()
            .init_resource::<ProjectPreferences>()
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
        crate::fonts::install(app);
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
    let desired = effective_settings(world);
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
    let mut desired = effective_preferences(world).editor.unwrap_or_default();
    desired.read_only |= !crate::storage::ready(world);
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
        .query_filtered::<Entity, With<PanelInstance>>()
        .iter(world)
        .collect();
    for panel in panels {
        let preferences = effective_preferences(world);
        let ready = crate::storage::ready(world);
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
                crate::files::project_directory(world)
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
    LanguageServers,
}

impl Panel for SettingsPanel {
    const KEY: &'static str = "bed.settings";
    const LABEL: &'static str = "Settings";

    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
        // egui's horizontal widgets position their first control before later
        // controls can enlarge the row. Include text and padding up front so
        // slider rails, numeric editors, switches and names share one center.
        ui.spacing_mut().interact_size.y =
            (ui.text_style_height(&egui::TextStyle::Button)
                .max(ui.text_style_height(&egui::TextStyle::Body))
                + 2.0 * ui.spacing().button_padding.y)
                .max(ui.spacing().interact_size.y);
        let scope_id = ui.id().with((panel, "settings-scope"));
        let section_id = ui.id().with((panel, "settings-section"));
        let mut local = ui.data_mut(|d| d.get_temp::<bool>(scope_id).unwrap_or(false));
        let mut section = ui.data_mut(|d| d.get_temp::<Section>(section_id).unwrap_or_default());
        let wide = ui.available_width() >= 600.0;
        ui.horizontal(|ui| {
            // Reserve the trailing scope controls first. The path gets the rest
            // of this same row, so long filenames cannot push the switch out.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Workspace");
                ui.add_enabled_ui(true, |ui| {
                    toggle(ui, &mut local, "");
                });
                ui.label("Global");
                let path = if local {
                    world.get_resource::<crate::files::ProjectFolder>().and_then(|_| crate::files::project_directory(world))
                        .and_then(|root| crate::storage::workspace_path(world, &root))
                } else {
                    world.get_resource::<crate::storage::ConfigDirectory>()
                        .map(|config| config.0.join("preferences.bed"))
                };
                let text = path.as_ref().map(|path| path.display().to_string())
                    .unwrap_or_else(|| "In-memory settings".into());
                let response = ui.add(egui::Label::new(egui::RichText::new(&text).small().weak())
                    .truncate().sense(egui::Sense::click()))
                    .on_hover_text(if path.is_some() { format!("{text}\nClick to copy path") } else { text.clone() });
                if response.clicked() && path.is_some() {
                    ui.ctx().copy_text(text);
                }
            });
        });
        crate::storage::draw_errors(ui, world);
        crate::session::draw_status(ui, world);
        ui.separator();
        let global = world.resource::<GlobalPreferences>().0.clone();
        let mut overrides = world.get_resource::<ProjectPreferences>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let mut values = if local {
            overrides.resolve(&global)
        } else {
            global.clone()
        };
        let before = values.clone();
        ui.with_layout(if wide {
            egui::Layout::left_to_right(egui::Align::TOP)
        } else {
            egui::Layout::top_down(egui::Align::LEFT)
        }, |ui| {
            ui.vertical(|ui| {
                if wide { ui.set_width(145.0); }
                let navigation = |ui: &mut egui::Ui| {
                for (name, value) in [
                    ("General", Section::General),
                    ("Appearance", Section::Appearance),
                    ("Editor", Section::Editor),
                    ("Language servers", Section::LanguageServers),
                ] {
                    ui.selectable_value(&mut section, value, name);
                }
                };
                if wide { ui.vertical(navigation); } else { ui.horizontal_wrapped(navigation); }
            });
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .id_salt((panel, "settings-content"))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(280.0);
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
                                    ui.horizontal(|ui| {
                                        toggle(ui, &mut world.resource_mut::<StartupPreferences>().open_last_workspace,
                                            "Open last workspace on open");
                                    });
                                    ui.separator();
                                }
                                row!(autosave, |ui: &mut egui::Ui, value: &mut Autosave| {
                                    let before = *value;
                                    let mut response = ui.scope_builder(
                                        egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::LEFT)),
                                        |ui| egui::ComboBox::from_id_salt("autosave")
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
                                        .response).inner;
                                    ui.label("Autosave");
                                    if before != *value {
                                        response.mark_changed();
                                    }
                                    response
                                });
                                if values.autosave == Autosave::Timer {
                                    row!(autosave_timeout, |ui: &mut egui::Ui, value: &mut f64| {
                                        let response = ui.add(egui::DragValue::new(value)
                                            .range(0.01..=86400.0).speed(0.1).suffix(" s"));
                                        ui.label("Autosave timeout");
                                        response
                                    });
                                }
                            }
                            Section::Appearance => {
                                row!(theme, |ui: &mut egui::Ui, value: &mut ThemeChoice| {
                                    let before = *value;
                                    let mut response = ui.scope_builder(
                                        egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::LEFT)),
                                        |ui| egui::ComboBox::from_id_salt("theme")
                                        .selected_text(match value {
                                            ThemeChoice::Dark => "Dark",
                                            ThemeChoice::Light => "Light",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(value, ThemeChoice::Dark, "Dark");
                                            ui.selectable_value(value, ThemeChoice::Light, "Light");
                                        })
                                        .response).inner;
                                    ui.label("Theme");
                                    if before != *value {
                                        response.mark_changed();
                                    }
                                    response
                                });
                                ui.separator();
                                row!(font_family, |ui: &mut egui::Ui, value: &mut String| {
                                    let before = value.clone();
                                    let mut response = ui.scope_builder(
                                        egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::LEFT)),
                                        |ui| egui::ComboBox::from_id_salt("font-family")
                                        .selected_text(value.as_str())
                                        .height(300.0)
                                        .show_ui(ui, |ui| {
                                            for family in crate::fonts::available(world) {
                                                ui.selectable_value(value, family.clone(), family);
                                            }
                                        })
                                        .response).inner;
                                    ui.label("Font family");
                                    if crate::fonts::loading(world) {
                                        ui.label("Loading installed fonts…");
                                    } else if let Some(error) = crate::fonts::error(world) {
                                        ui.label(error);
                                    } else if !crate::fonts::available(world).contains(value) {
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
                                ui.separator();
                                row!(window_opacity, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=1.0).text("Window opacity")
                                ));
                                #[cfg(target_os = "macos")]
                                row!(window_blur, |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Window blur"));
                                ui.separator();
                                row!(panel_opacity, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=1.0).text("Panel opacity")
                                ));
                                row!(panel_radius, |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                    egui::Slider::new(value, 0.0..=32.0)
                                        .text("Panel border radius")
                                ));
                                row!(panel_gap, |ui: &mut egui::Ui, value: &mut f32| ui
                                    .add(egui::Slider::new(value, 0.0..=32.0).text("Panel gap")));
                                ui.separator();
                                row!(
                                    file_tree_indentation,
                                    |ui: &mut egui::Ui, value: &mut f32| ui.add(
                                        egui::Slider::new(value, 0.0..=64.0)
                                            .text("File tree indentation")
                                    )
                                );
                                ui.separator();
                                row!(effects_enabled, |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Shader effects"));
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
                                row!(line_numbers, |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Line numbers"));
                                row!(smart_wrapping, |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Smart wrapping"));
                                row!(smart_indentation, |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Smart indentation"));
                            }
                            Section::LanguageServers => {
                                ui.heading("Language servers");
                                ui.add(egui::Label::new("Install servers separately and make their executables available on PATH. Servers start when a supported file is opened in a folder workspace.").wrap());
                                row!(
                                    language_servers_enabled,
                                    |ui: &mut egui::Ui, value: &mut bool| toggle(ui, value, "Enable language servers")
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
                                                for (language, config) in default_language_servers() {
                                                    if !servers.contains_key(&language)
                                                        && ui.button(format!("Add {language} support")).clicked()
                                                    {
                                                        servers.insert(language, config);
                                                        changed = true;
                                                    }
                                                }
                                                for (language, server) in servers.iter_mut() {
                                                    let id =
                                                        ui.id().with(("server-draft", local, language));
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
                                                    ui.separator();
                                                    let label = match language.as_str() {
                                                        "rust" => "Rust · rust-analyzer",
                                                        "go" => "Go · gopls",
                                                        "typescript" => "TypeScript · TypeScript Language Server",
                                                        "typescriptreact" => "TypeScript React (TSX)",
                                                        "python" => "Python · Pyright",
                                                        other => other,
                                                    };
                                                    egui::CollapsingHeader::new(label).id_salt(id).show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.add(egui::TextEdit::singleline(&mut draft.program)
                                                            .desired_width((ui.available_width() - 100.0).clamp(100.0, 260.0)));
                                                        ui.label("Executable");
                                                    });
                                                    for (index, argument) in draft.arguments.iter_mut().enumerate() {
                                                        ui.horizontal(|ui| {
                                                            ui.add(egui::TextEdit::singleline(argument).id_salt((id, index))
                                                                .desired_width((ui.available_width() - 100.0).clamp(100.0, 260.0)));
                                                            ui.label(format!("Argument {}", index + 1));
                                                        });
                                                    }
                                                    ui.horizontal_wrapped(|ui| {
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
            world.insert_resource(ProjectPreferences(overrides));
        } else if values != before {
            world.resource_mut::<GlobalPreferences>().0 = values;
        }
        ui.data_mut(|d| {
            d.insert_temp(scope_id, local);
            d.insert_temp(section_id, section);
        });
        });
    }
}

fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(32.0, 18.0), egui::Sense::click());
    if !label.is_empty() {
        response = response.union(ui.add(egui::Label::new(label).sense(egui::Sense::click())));
    }
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
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
            if label.is_empty() {
                "Workspace settings"
            } else {
                label
            },
        )
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_servers_cover_supported_languages_and_preserve_saved_definitions() {
        let defaults = Settings::default();
        defaults.validate().unwrap();
        for (language, program, arguments) in [
            ("rust", "rust-analyzer", vec![]),
            ("go", "gopls", vec![]),
            ("typescript", "typescript-language-server", vec!["--stdio"]),
            ("python", "pyright-langserver", vec!["--stdio"]),
        ] {
            let server = &defaults.language_servers[language];
            assert_eq!(server.program, program);
            assert_eq!(server.arguments, arguments);
        }
        assert_eq!(
            defaults.language_servers["typescript"],
            defaults.language_servers["typescriptreact"]
        );
        let old: Settings =
            ron::from_str("(language_servers: {\"rust\": (program: \"custom-rust\")})").unwrap();
        assert_eq!(old.language_servers.len(), 1);
        assert_eq!(old.language_servers["rust"].program, "custom-rust");
        let restored: Settings = ron::from_str(&ron::to_string(&defaults).unwrap()).unwrap();
        assert_eq!(restored.language_servers, defaults.language_servers);
    }

    #[test]
    fn workspace_overrides_are_per_value_and_reset_resumes_inheritance() {
        let mut world = World::new();
        world.insert_resource(GlobalPreferences::default());
        world.insert_resource(ProjectPreferences(SettingsOverrides {
            line_numbers: Some(false),
            ..default()
        }));
        world.resource_mut::<GlobalPreferences>().0.smart_wrapping = false;
        assert!(!effective_settings(&world).line_numbers);
        assert!(!effective_settings(&world).smart_wrapping);
        world
            .get_resource_mut::<ProjectPreferences>()
            .unwrap()
            .0
            .line_numbers = None;
        assert!(effective_settings(&world).line_numbers);
        world.resource_mut::<GlobalPreferences>().0.line_numbers = false;
        assert!(!effective_settings(&world).line_numbers);
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
    fn project_overrides_apply_appearance_without_changing_global_values() {
        let mut app = App::new();
        app.add_plugins(bedtime_workbench2::WorkbenchCorePlugin)
            .init_resource::<GlobalPreferences>()
            .init_resource::<AppliedAppearance>();
        app.insert_resource(ProjectPreferences(SettingsOverrides {
            font_size: Some(22.0),
            panel_gap: Some(12.0),
            ..default()
        }));
        let theme = app.world_mut().spawn(themes::dark()).id();
        let surface = app
            .world_mut()
            .spawn((WorkbenchSurface::default(), UsesTheme(theme)))
            .id();
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
            app.world().resource::<GlobalPreferences>().0.font_size,
            15.5
        );
        app.world_mut()
            .resource_mut::<ProjectPreferences>()
            .0
            .font_size = None;
        apply_appearance(app.world_mut());
        assert_eq!(
            app.world()
                .get::<bedtime_ui2::Theme>(theme)
                .unwrap()
                .metrics
                .font_size,
            15.5
        );
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    let fonts = crate::fonts::inspect(world);
    Ok(
        serde_json::json!({ "global": world.resource::<GlobalPreferences>().0,
        "fonts": fonts,
        "startup": world.resource::<StartupPreferences>(),
        "project": {"effective": effective_settings(world), "overrides": world.get_resource::<ProjectPreferences>().map(|p| &p.0)} }),
    )
}

#[cfg(feature = "automation")]
fn set(world: &mut World, arguments: serde_json::Value) -> bedtime_automation2::AutomationResult {
    if !crate::close::accepts_user_actions(world) {
        return Err("Cancel quit before editing settings".into());
    }
    if arguments.get("workspace").is_some()
        || arguments
            .get("scope")
            .is_some_and(|scope| scope != "global" && scope != "project")
    {
        return Err("Settings scope must be global or project".into());
    }
    let values = arguments
        .get("values")
        .and_then(|v| v.as_object())
        .ok_or("values must be a settings object")?;
    if arguments.get("scope").and_then(|s| s.as_str()) == Some("project") {
        let current = world
            .get_resource::<ProjectPreferences>()
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
        world.insert_resource(ProjectPreferences(settings));
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

use bedtime_workbench2::persistence::ReflectWorkbenchPersist;

#[derive(Component, Reflect, Default)]
#[reflect(Component, WorkbenchPersist)]
#[require(SettingsPanel)]
pub(crate) struct PersistedSettings;

pub(crate) fn register_persistence(app: &mut App) {
    app.register_type::<PersistedSettings>();
}
pub(crate) fn project_persistence(
    world: &mut World,
    captured: &mut bedtime_workbench2::persistence::CapturedWorld,
) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<SettingsPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        captured.insert_component(panel, PersistedSettings);
    }
}
pub(crate) fn clear_persistence(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<PersistedSettings>>()
        .iter(world)
        .collect();
    for panel in panels {
        world.entity_mut(panel).remove::<PersistedSettings>();
    }
}

pub(crate) fn register_menu(app: &mut App) {
    crate::menus::register_panel::<SettingsPanel>(
        app,
        Some(90),
        bedtime_workbench2::PanelFactory::of::<SettingsPanel>(),
        |_| true,
    );
}
