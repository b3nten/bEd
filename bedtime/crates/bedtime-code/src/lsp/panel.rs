//! The Language Servers panel: each server, built in or added, what it
//! runs and how, per settings layer, with its status. The changes are the
//! `LanguageServerConfig` setting, which builds the `ServerCatalog`.
use super::{
    InstalledServers, ServerCatalog, ServerEntry, ServerSettings, ServerStatus, config::resolve,
    restart, server_states,
};
use bedtime_ui::{BuiltinIcon, UiContext, UiFrame, egui, theme};
use bedtime_workbench::{
    Action, ActionAppExt, Handler, Panel,
    persistence::{ReflectPersist, StoreId},
    settings::{Settings, SettingsAppExt, SettingsWorldExt, settings, ui::layer_tabs},
    show_panel,
};
use bevy::{platform::collections::HashMap, prelude::*};
use std::collections::BTreeMap;

settings! {
    /// Changes to language servers, by name: built-in ones changed, others added.
    pub LanguageServerConfig: BTreeMap<String, ServerSettings> {
        default: BTreeMap::new(),
        key: "language_servers",
    }
}

pub struct OpenLanguageServers;
impl Action for OpenLanguageServers {
    const KEY: &'static str = "code.show_language_servers";
    const LABEL: &'static str = "Language Servers";
}

pub struct LanguageServersPanelPlugin;

impl Plugin for LanguageServersPanelPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LanguageServersPanel>()
            .register_setting::<LanguageServerConfig>()
            .add_action_handler::<OpenLanguageServers>(Handler::new(
                show_panel::<LanguageServersPanel>,
            ))
            .add_systems(Update, refresh_catalog.run_if(resource_changed::<Settings>));
    }
}

/// Each layer's changes, lowest first, up to `top` (every layer without one).
fn layers(world: &World, top: Option<StoreId>) -> Vec<BTreeMap<String, ServerSettings>> {
    let settings = world.settings();
    let mut layers = Vec::new();
    for layer in settings.layers() {
        if let Some(map) = settings.get_in::<LanguageServerConfig>(layer) {
            layers.push(map);
        }
        if Some(layer) == top {
            break;
        }
    }
    layers
}

/// The servers, as every layer has them.
fn refresh_catalog(world: &mut World) {
    let catalog = ServerCatalog(resolve(layers(world, None).iter()));
    if world.get_resource::<ServerCatalog>() != Some(&catalog) {
        world.insert_resource(catalog);
    }
}

/// Change server `name` in `layer`; an entry left with no changes goes.
fn change(world: &mut World, layer: StoreId, name: &str, f: impl FnOnce(&mut ServerSettings)) {
    let mut map = world
        .settings()
        .get_in::<LanguageServerConfig>(layer)
        .unwrap_or_default();
    let entry = map.entry(name.to_owned()).or_default();
    f(entry);
    if entry.is_empty() {
        map.remove(name);
    }
    let result = if map.is_empty() {
        world.settings_mut().clear::<LanguageServerConfig>(layer)
    } else {
        world.settings_mut().set::<LanguageServerConfig>(layer, map)
    };
    if let Err(error) = result {
        warn!("Couldn't change {name}: {error}");
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
pub struct LanguageServersPanel {
    #[reflect(ignore)]
    state: PanelState,
}

#[derive(Default)]
struct PanelState {
    expanded: Option<String>,
    /// Text being typed into a field, by server and field.
    drafts: HashMap<(String, Field), String>,
    errors: HashMap<(String, Field), String>,
    adding: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Field {
    Program,
    Arguments,
    Languages,
    Environment,
    Settings,
    InitializationOptions,
}

/// What the user asked for while the panel drew, done after.
enum Asked {
    Enable(String, bool),
    Restart(Entity),
    Reset(String),
    Set(String, Field, String),
    Add(String),
    LookAgain,
}

impl Panel for LanguageServersPanel {
    const LABEL: &'static str = "Language Servers";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let mut asked = Vec::new();
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            let Some(layer) = layer_tabs(ui, world, |_, label| label.to_owned()) else {
                return;
            };
            // The first layer is everywhere's (global); later ones override it.
            let note = if world.settings().layers().next() == Some(layer) {
                "Changes here apply to every workspace."
            } else {
                "Changes here apply to this workspace, over the global ones."
            };
            ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
            ui.add_space(6.0);
            let entries = resolve(layers(world, Some(layer)).iter());
            let states = server_states(world);
            let Some(mut panel_state) = world
                .get_mut::<LanguageServersPanel>(panel)
                .map(|mut panel| std::mem::take(&mut panel.state))
            else {
                return;
            };
            let installed = world.resource::<InstalledServers>();
            let (missing, shown): (Vec<&ServerEntry>, Vec<&ServerEntry>) =
                entries.iter().partition(|entry| {
                    entry.known.is_some() && matches!(installed.program(&entry.program), Some(None))
                });
            let frame = context.frame(ui);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(frame.ui, |ui| {
                    let mut frame = context.frame(ui);
                    for entry in &shown {
                        let found = installed.program(&entry.program);
                        server_card(
                            &mut frame,
                            entry,
                            found,
                            &states,
                            &mut panel_state,
                            &mut asked,
                        );
                    }
                    add_server(frame.ui, &mut panel_state, &mut asked);
                    frame.ui.add_space(10.0);
                    egui::CollapsingHeader::new(format!("Not installed ({})", missing.len()))
                        .id_salt("not-installed")
                        .default_open(false)
                        .show(frame.ui, |ui| {
                            for entry in &missing {
                                not_installed(ui, entry);
                            }
                            if ui.button("Look Again").clicked() {
                                asked.push(Asked::LookAgain);
                            }
                        });
                });
            if let Some(mut panel) = world.get_mut::<LanguageServersPanel>(panel) {
                panel.state = panel_state;
            }
            apply(world, layer, panel, std::mem::take(&mut asked));
        });
    }
}

fn apply(world: &mut World, layer: StoreId, panel: Entity, asked: Vec<Asked>) {
    for asked in asked {
        match asked {
            Asked::Enable(name, enabled) => {
                change(world, layer, &name, |s| s.enabled = Some(enabled))
            }
            Asked::Restart(server) => {
                if let Err(error) = restart(world, server) {
                    warn!("{error}");
                }
            }
            Asked::Reset(name) => change(world, layer, &name, |s| *s = ServerSettings::default()),
            Asked::Add(name) => change(world, layer, &name, |s| {
                s.program = Some(name.clone());
                s.enabled = Some(true);
            }),
            Asked::LookAgain => world.resource_mut::<InstalledServers>().forget(),
            Asked::Set(name, field, text) => {
                let result = parse(field, &text);
                let key = (name.clone(), field);
                let mut panel_state = world.get_mut::<LanguageServersPanel>(panel);
                match result {
                    Ok(value) => {
                        if let Some(panel) = panel_state.as_mut() {
                            panel.state.errors.remove(&key);
                            panel.state.drafts.remove(&key);
                        }
                        change(world, layer, &name, |s| value.store(s));
                    }
                    Err(error) => {
                        if let Some(panel) = panel_state.as_mut() {
                            panel.state.errors.insert(key, error);
                        }
                    }
                }
            }
        }
    }
}

/// A field's typed text, as the value to keep (empty: inherit).
enum Value {
    Program(Option<String>),
    Strings(Field, Option<Vec<String>>),
    Environment(Option<BTreeMap<String, String>>),
    Json(Field, Option<serde_json::Value>),
}

impl Value {
    fn store(self, settings: &mut ServerSettings) {
        match self {
            Value::Program(program) => settings.program = program,
            Value::Strings(Field::Arguments, value) => settings.arguments = value,
            Value::Strings(_, value) => settings.languages = value,
            Value::Environment(value) => settings.environment = value,
            Value::Json(Field::Settings, value) => settings.settings = value,
            Value::Json(_, value) => settings.initialization_options = value,
        }
    }
}

fn parse(field: Field, text: &str) -> Result<Value, String> {
    let text = text.trim();
    let empty = text.is_empty();
    Ok(match field {
        Field::Program => Value::Program((!empty).then(|| text.to_owned())),
        Field::Arguments => Value::Strings(
            field,
            (!empty).then(|| text.split_whitespace().map(str::to_owned).collect()),
        ),
        Field::Languages => Value::Strings(
            field,
            (!empty).then(|| {
                text.split(',')
                    .map(|language| language.trim().to_owned())
                    .filter(|language| !language.is_empty())
                    .collect()
            }),
        ),
        Field::Environment => {
            let mut environment = BTreeMap::new();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                let (name, value) = line
                    .split_once('=')
                    .ok_or_else(|| format!("“{line}” isn't NAME=value"))?;
                environment.insert(name.trim().to_owned(), value.to_owned());
            }
            Value::Environment((!empty).then_some(environment))
        }
        Field::Settings | Field::InitializationOptions => {
            if empty {
                Value::Json(field, None)
            } else {
                let value =
                    serde_json::from_str(text).map_err(|error| format!("Not JSON: {error}"))?;
                Value::Json(field, Some(value))
            }
        }
    })
}

/// How a field shows a server's value.
fn shown(entry: &ServerEntry, field: Field) -> String {
    match field {
        Field::Program => entry.program.clone(),
        Field::Arguments => entry.arguments.join(" "),
        Field::Languages => entry.languages.join(", "),
        Field::Environment => entry
            .environment
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Field::Settings => pretty(&entry.settings),
        Field::InitializationOptions => pretty(&entry.initialization_options),
    }
}

fn pretty(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        value => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

/// What a server is doing, in words, and its light's color.
fn status(
    entry: &ServerEntry,
    found: Option<Option<&std::path::PathBuf>>,
    running: Option<&(Entity, ServerStatus, usize)>,
    p: &theme::Palette,
) -> (String, egui::Color32) {
    let muted = theme::color(p.muted);
    if !entry.enabled {
        return (
            "Disabled".into(),
            theme::color(theme::mix(p.muted, p.panel_background, 0.5)),
        );
    }
    match (running, found) {
        (Some((_, ServerStatus::Ready, documents)), _) => (
            match documents {
                1 => "Running · 1 file".into(),
                n => format!("Running · {n} files"),
            },
            theme::color(p.success),
        ),
        (Some((_, ServerStatus::Starting, _)), _) => ("Starting…".into(), theme::color(p.warning)),
        (Some((_, ServerStatus::Failed(error), _)), _) => {
            (format!("Failed: {error}"), theme::color(p.error))
        }
        (Some((_, ServerStatus::Stopped, _)), _) => ("Stopped".into(), muted),
        (None, None) => ("Looking for it…".into(), muted),
        (None, Some(None)) => (
            format!("{} isn't installed", entry.program),
            theme::color(p.error),
        ),
        (None, Some(Some(_))) if entry.languages.is_empty() => {
            ("Serves no languages yet".into(), muted)
        }
        (None, Some(Some(_))) => (
            format!("Starts with a {} file", entry.languages.join(", ")),
            muted,
        ),
    }
}

fn server_card(
    frame: &mut UiFrame<'_>,
    entry: &ServerEntry,
    found: Option<Option<&std::path::PathBuf>>,
    states: &std::collections::HashMap<String, (Entity, ServerStatus, usize)>,
    panel: &mut PanelState,
    asked: &mut Vec<Asked>,
) {
    let p = frame.theme.palette.clone();
    let running = states.get(&entry.name);
    let (text, light) = status(entry, found, running, &p);
    let expanded = panel.expanded.as_deref() == Some(entry.name.as_str());
    let services = frame.context();
    let mut menu_targets: Vec<egui::Response> = Vec::new();
    egui::Frame::new()
        .fill(theme::color(theme::mix(p.panel_background, p.text, 0.035)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(frame.ui, |ui| {
            ui.set_width(ui.available_width());
            let mut inner = services.frame(ui);
            // One row high: the right-aligned buttons would take all the height.
            let header = egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    inner.ui.cursor().min,
                    egui::vec2(inner.ui.available_width(), 26.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center));
            inner.scope(header, |mut row| {
                let (dot, _) = row
                    .ui
                    .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                row.ui.painter().circle_filled(dot.center(), 4.0, light);
                let title = row.ui.add(
                    egui::Label::new(egui::RichText::new(&entry.name).strong())
                        .sense(egui::Sense::click()),
                );
                if title.clicked() {
                    panel.expanded = (!expanded).then(|| entry.name.clone());
                }
                menu_targets.push(title);
                if !entry.languages.is_empty() {
                    row.ui.weak(entry.languages.join(", "));
                }
                let right =
                    egui::UiBuilder::new().layout(egui::Layout::right_to_left(egui::Align::Center));
                row.scope(right, |mut buttons| {
                    let chevron = if expanded {
                        BuiltinIcon::ChevronDown
                    } else {
                        BuiltinIcon::ChevronRight
                    };
                    let toggle = buttons.icon_button(chevron);
                    buttons.tooltip(&toggle, "Configure");
                    if toggle.clicked() {
                        panel.expanded = (!expanded).then(|| entry.name.clone());
                    }
                    if let Some((server, ..)) = running {
                        let restart = buttons.icon_button(BuiltinIcon::RotateCcw);
                        buttons.tooltip(&restart, "Restart");
                        if restart.clicked() {
                            asked.push(Asked::Restart(*server));
                        }
                    }
                    let mut enabled = entry.enabled;
                    if buttons.toggle(&mut enabled, "").changed() {
                        asked.push(Asked::Enable(entry.name.clone(), enabled));
                    }
                });
            });
            menu_targets.push(
                inner.ui.add(
                    egui::Label::new(egui::RichText::new(&text).small().color(light))
                        .sense(egui::Sense::click()),
                ),
            );
            if let Some(Some(path)) = found {
                inner.ui.add(
                    egui::Label::new(
                        egui::RichText::new(path.display().to_string())
                            .small()
                            .weak(),
                    )
                    .truncate(),
                );
            }
            if expanded {
                inner.ui.add_space(6.0);
                fields(inner.ui, entry, panel, asked);
            }
        });
    // The menu is on the name and status: a whole-card target would sit on
    // top of the buttons and take their clicks.
    for target in menu_targets {
        target.context_menu(|ui| card_menu(ui, entry, found, running, asked));
    }
    frame.ui.add_space(6.0);
}

fn card_menu(
    ui: &mut egui::Ui,
    entry: &ServerEntry,
    found: Option<Option<&std::path::PathBuf>>,
    running: Option<&(Entity, ServerStatus, usize)>,
    asked: &mut Vec<Asked>,
) {
    {
        if let Some((server, ..)) = running
            && ui.button("Restart").clicked()
        {
            asked.push(Asked::Restart(*server));
        }
        let reset = if entry.known.is_some() {
            "Reset to Defaults"
        } else {
            "Remove"
        };
        if ui.button(reset).clicked() {
            asked.push(Asked::Reset(entry.name.clone()));
        }
        if let Some(Some(path)) = found
            && ui.button("Copy Program Path").clicked()
        {
            ui.ctx().copy_text(path.display().to_string());
        }
    }
}

/// The configuration of an expanded server: each field keeps what's typed
/// when Enter is pressed or it loses focus.
fn fields(ui: &mut egui::Ui, entry: &ServerEntry, panel: &mut PanelState, asked: &mut Vec<Asked>) {
    egui::Grid::new(("fields", &entry.name))
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for (field, label, multiline) in [
                (Field::Program, "Program", false),
                (Field::Arguments, "Arguments", false),
                (Field::Languages, "Languages", false),
                (Field::Environment, "Environment", true),
                (Field::Settings, "Settings", true),
                (Field::InitializationOptions, "Initialization", true),
            ] {
                ui.label(label);
                let key = (entry.name.clone(), field);
                let text = panel
                    .drafts
                    .entry(key.clone())
                    .or_insert_with(|| shown(entry, field));
                let hint = match field {
                    Field::Arguments => "--stdio",
                    Field::Languages => "rust, toml",
                    Field::Environment => "NAME=value, one a line",
                    Field::Settings | Field::InitializationOptions => "{ }  (JSON)",
                    Field::Program => "a name on the PATH, or a path",
                };
                let edit = if multiline {
                    egui::TextEdit::multiline(text)
                        .code_editor()
                        .desired_rows(2)
                } else {
                    egui::TextEdit::singleline(text)
                };
                let response = ui.add(edit.hint_text(hint).desired_width(f32::INFINITY));
                let done = response.lost_focus();
                if done {
                    asked.push(Asked::Set(entry.name.clone(), field, text.clone()));
                } else if !response.has_focus() && *text != shown(entry, field) {
                    // Changed elsewhere (another layer, the file): show that.
                    *text = shown(entry, field);
                }
                ui.end_row();
                if let Some(error) = panel.errors.get(&key) {
                    ui.label("");
                    ui.colored_label(ui.visuals().error_fg_color, error);
                    ui.end_row();
                }
            }
        });
}

fn add_server(ui: &mut egui::Ui, panel: &mut PanelState, asked: &mut Vec<Asked>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut panel.adding)
                .hint_text("Another server's name")
                .desired_width(200.0),
        );
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let name = panel.adding.trim().to_owned();
        if (ui
            .add_enabled(!name.is_empty(), egui::Button::new("Add Server"))
            .clicked()
            || enter)
            && !name.is_empty()
        {
            panel.expanded = Some(name.clone());
            asked.push(Asked::Add(name));
            panel.adding.clear();
        }
    });
}

fn not_installed(ui: &mut egui::Ui, entry: &ServerEntry) {
    ui.horizontal(|ui| {
        ui.label(&entry.name);
        ui.weak(entry.languages.join(", "));
        if let Some(known) = entry.known {
            ui.hyperlink_to("Get it", known.home);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_fields_become_values_or_errors() {
        let mut settings = ServerSettings::default();
        parse(Field::Arguments, " --stdio  --log ")
            .unwrap()
            .store(&mut settings);
        assert_eq!(
            settings.arguments.as_deref(),
            Some(&["--stdio".to_owned(), "--log".to_owned()][..])
        );
        parse(Field::Languages, "rust, toml,")
            .unwrap()
            .store(&mut settings);
        assert_eq!(
            settings.languages.as_deref(),
            Some(&["rust".to_owned(), "toml".to_owned()][..])
        );
        parse(Field::Environment, "A=1\nB=x=y")
            .unwrap()
            .store(&mut settings);
        assert_eq!(settings.environment.as_ref().unwrap()["B"], "x=y");
        assert!(parse(Field::Environment, "nope").is_err());
        assert!(parse(Field::Settings, "{ not json").is_err());
        parse(Field::Settings, r#"{"check": {"command": "clippy"}}"#)
            .unwrap()
            .store(&mut settings);
        assert!(settings.settings.is_some());
        // Emptied: inherit again.
        parse(Field::Arguments, "").unwrap().store(&mut settings);
        assert!(settings.arguments.is_none());
    }
}
