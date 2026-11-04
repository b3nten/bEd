//! The Theme Editor panel: pick a theme from the themes folder, duplicate it
//! under a new name, and change its colors. Changes are saved to the theme's
//! file shortly after you stop editing; a shipped theme can be reset.
use crate::{
    settings::{AppearanceSettings, ColorTheme},
    themes,
};
use bedtime_ui::{
    BuiltinIcon, UiContext, egui,
    theme::{Appearance, Palette, Rgba, Theme, ThemeFolder},
};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, Handler, Panel, PanelsWorldExt,
    persistence::ReflectPersist, settings::SettingsWorldExt,
};
use bevy::prelude::*;
use std::time::{Duration, Instant};

/// How long after the last change a theme is saved.
const SAVE_AFTER: Duration = Duration::from_millis(400);

pub struct OpenThemeEditor;
impl Action for OpenThemeEditor {
    const KEY: &'static str = "app.open_theme_editor";
    const LABEL: &'static str = "Theme Editor";
}

pub struct ThemeEditorPlugin;

impl Plugin for ThemeEditorPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ThemeEditorPanel>()
            .add_action_handler::<OpenThemeEditor>(Handler::new(|world, _: ActionInvocation| {
                open(world, None)
            }))
            .add_systems(Update, autosave);
    }
}

/// Open the editor (or focus the open one), on `theme` if given.
pub fn open(world: &mut World, theme: Option<String>) -> ActionResult {
    let panel = world.focus_or_open::<ThemeEditorPanel>()?;
    if let Some(theme) = theme
        && let Some(mut editor) = world.get_mut::<ThemeEditorPanel>(panel)
    {
        editor.theme = theme;
    }
    Ok(())
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
pub struct ThemeEditorPanel {
    /// The theme being edited, by id; empty for the one in use.
    theme: String,
    #[reflect(ignore)]
    state: EditorState,
}

#[derive(Default)]
struct EditorState {
    /// The theme being edited, as last loaded or changed here.
    draft: Option<Draft>,
    /// The name as typed, until it's committed.
    name: String,
    /// The name for a copy, while asking for one.
    copy_name: Option<String>,
    /// Asking whether to delete the theme.
    confirming_delete: bool,
}

struct Draft {
    id: String,
    theme: Theme,
    /// When it was changed, if it hasn't been saved since.
    unsaved_since: Option<Instant>,
}

/// What the panel's buttons asked for, done after drawing.
enum Request {
    Edit(String),
    Copy(String),
    Rename(String),
    Reset,
    Delete,
}

impl Panel for ThemeEditorPanel {
    const LABEL: &'static str = "Theme Editor";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        if !world.contains_resource::<ThemeFolder>() {
            ui.weak("There's no themes folder.");
            return;
        }
        let active = world.resource::<AppearanceSettings>().theme.clone();
        let Some(mut editor) = world.get_mut::<ThemeEditorPanel>(panel) else {
            return;
        };
        if editor.theme.is_empty() {
            editor.theme = active;
        }
        let id = editor.theme.clone();
        let mut state = std::mem::take(&mut editor.state);
        let folder = world.resource::<ThemeFolder>();
        state.load(&id, folder);
        let choices: Vec<(String, String)> = sorted_themes(folder);
        let shipped = themes::built_in(&id);
        let differs = shipped.is_some_and(|text| {
            state
                .draft
                .as_ref()
                .is_some_and(|draft| Theme::from_ron(text).ok().as_ref() != Some(&draft.theme))
        });

        let mut request = None;
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                request = state.header(ui, context, &id, &choices, shipped.is_some(), differs);
                ui.add_space(4.0);
                ui.separator();
                let Some(draft) = &mut state.draft else {
                    ui.weak(format!("There's no readable theme named \"{id}\"."));
                    return;
                };
                let changed = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| colors(ui, context, &mut draft.theme))
                    .inner;
                if changed {
                    draft.unsaved_since = Some(Instant::now());
                }
                if draft.unsaved_since.is_some() {
                    ui.ctx().request_repaint_after(SAVE_AFTER);
                }
            });

        if let Some(mut editor) = world.get_mut::<ThemeEditorPanel>(panel) {
            editor.state = state;
        }
        if let Some(request) = request {
            let result = act(world, panel, request);
            crate::status::notify_failure(world, "Theme Editor", result);
        }
    }
}

impl EditorState {
    /// Load `id` from the folder when switching to it, or when its file
    /// changed and there's nothing unsaved here.
    fn load(&mut self, id: &str, folder: &ThemeFolder) {
        let current = self.draft.as_ref().filter(|draft| draft.id == id);
        let reload = match current {
            None => true,
            Some(draft) => draft.unsaved_since.is_none() && folder.get(id) != Some(&draft.theme),
        };
        if !reload {
            return;
        }
        if current.is_none() {
            self.copy_name = None;
            self.confirming_delete = false;
        }
        self.draft = folder.get(id).map(|theme| Draft {
            id: id.to_owned(),
            theme: theme.clone(),
            unsaved_since: None,
        });
        self.name = self
            .draft
            .as_ref()
            .map_or_else(String::new, |draft| draft.theme.name.clone());
    }

    /// The theme picker, its actions, then its name and appearance.
    fn header(
        &mut self,
        ui: &mut egui::Ui,
        context: &UiContext<'_>,
        id: &str,
        choices: &[(String, String)],
        shipped: bool,
        differs: bool,
    ) -> Option<Request> {
        let mut request = None;
        let shown = self
            .draft
            .as_ref()
            .map_or_else(|| id.to_owned(), |draft| draft.theme.name.clone());
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("theme-editor-theme")
                .width(220.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (choice, name) in choices {
                        if ui.selectable_label(choice == id, name).clicked() && choice != id {
                            request = Some(Request::Edit(choice.clone()));
                        }
                    }
                });
            let Some(draft) = &self.draft else {
                return;
            };
            let mut frame = context.frame(ui);
            let copy = frame.icon_button_sized(BuiltinIcon::Plus, 14.0);
            copy.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Duplicate theme")
            });
            frame.tooltip(&copy, "Duplicate this theme under a new name");
            if copy.clicked() {
                self.copy_name = Some(format!("{} Copy", draft.theme.name));
                self.confirming_delete = false;
            }
            if shipped {
                ui.add_enabled_ui(differs, |ui| {
                    let mut frame = context.frame(ui);
                    let reset = frame.icon_button_sized(BuiltinIcon::RotateCcw, 14.0);
                    reset.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, differs, "Reset theme")
                    });
                    frame.tooltip(
                        &reset,
                        if differs {
                            "Reset this theme to the colors bEd ships with"
                        } else {
                            "This theme is as bEd ships it"
                        },
                    );
                    if reset.clicked() {
                        request = Some(Request::Reset);
                    }
                });
            } else {
                let mut frame = context.frame(ui);
                let delete = frame.icon_button_sized(BuiltinIcon::Trash, 14.0);
                delete.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Delete theme")
                });
                frame.tooltip(&delete, "Delete this theme");
                if delete.clicked() {
                    self.confirming_delete = true;
                    self.copy_name = None;
                }
            }
            ui.weak(if draft.unsaved_since.is_some() {
                "Saving…"
            } else {
                "Saved"
            });
        });

        if let Some(name) = &mut self.copy_name {
            let mut create = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.label("New theme");
                let field = ui.add(egui::TextEdit::singleline(name).desired_width(200.0));
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                create = ui.button("Create").clicked() || entered;
                cancel = ui.button("Cancel").clicked();
            });
            if create && !name.trim().is_empty() {
                request = Some(Request::Copy(name.trim().to_owned()));
                self.copy_name = None;
            } else if cancel {
                self.copy_name = None;
            }
        }
        if self.confirming_delete {
            ui.horizontal(|ui| {
                ui.weak("Delete this theme's file?");
                let error = bedtime_ui::theme::color(context.theme.palette.error);
                if ui
                    .button(egui::RichText::new("Delete").color(error))
                    .clicked()
                {
                    request = Some(Request::Delete);
                    self.confirming_delete = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirming_delete = false;
                }
            });
        }

        let Some(draft) = &mut self.draft else {
            return request;
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            let field = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
            if field.lost_focus() {
                let name = self.name.trim();
                if name.is_empty() {
                    self.name = draft.theme.name.clone();
                } else if name != draft.theme.name {
                    request = Some(Request::Rename(name.to_owned()));
                }
            }
            ui.add_space(12.0);
            let before = draft.theme.appearance;
            ui.selectable_value(&mut draft.theme.appearance, Appearance::Dark, "Dark");
            ui.selectable_value(&mut draft.theme.appearance, Appearance::Light, "Light");
            if draft.theme.appearance != before {
                draft.unsaved_since = Some(Instant::now());
            }
        });
        let file = format!("{id}.ron");
        ui.weak(if shipped {
            format!("{file}, shipped with bEd")
        } else {
            file
        });
        request
    }
}

/// The folder's themes as (id, name), by name.
fn sorted_themes(folder: &ThemeFolder) -> Vec<(String, String)> {
    let mut themes: Vec<_> = folder
        .themes()
        .map(|(id, theme)| (id.to_owned(), theme.name.clone()))
        .collect();
    themes.sort_by(|a, b| a.1.cmp(&b.1));
    themes
}

fn act(world: &mut World, panel: Entity, request: Request) -> Result<(), String> {
    let mut editor = world
        .get_mut::<ThemeEditorPanel>(panel)
        .ok_or("The editor closed")?;
    // Anything still unsaved goes first, so a copy or rename carries it.
    let draft = editor.state.draft.take();
    let id = editor.theme.clone();
    let Some(mut draft) = draft else {
        if let Request::Edit(other) = request {
            editor.theme = other;
        }
        return Ok(());
    };
    if draft.unsaved_since.take().is_some() {
        save(world, &draft.id, &draft.theme)?;
    }
    let folder = world.resource::<ThemeFolder>();
    let editing = match request {
        Request::Edit(other) => other,
        Request::Copy(name) => {
            let copy = unused_id(folder, &slug(&name), None);
            draft.theme.name = name;
            save(world, &copy, &draft.theme)?;
            copy
        }
        Request::Rename(name) => {
            draft.theme.name = name;
            if themes::built_in(&id).is_some() {
                // Shipped themes keep their file; only the name changes.
                save(world, &id, &draft.theme)?;
                id
            } else {
                let renamed = unused_id(folder, &slug(&draft.theme.name), Some(&id));
                save(world, &renamed, &draft.theme)?;
                if renamed != id {
                    remove(world, &id)?;
                    replace_setting(world, &id, Some(&renamed))?;
                }
                renamed
            }
        }
        Request::Reset => {
            let text = themes::built_in(&id).ok_or("Only shipped themes can be reset")?;
            write(world, &id, text)?;
            id
        }
        Request::Delete => {
            remove(world, &id)?;
            replace_setting(world, &id, None)?;
            world.resource::<AppearanceSettings>().theme.clone()
        }
    };
    if let Some(mut editor) = world.get_mut::<ThemeEditorPanel>(panel) {
        editor.theme = editing;
        editor.state.draft = None;
    }
    Ok(())
}

/// Save drafts a moment after their last change.
fn autosave(world: &mut World) {
    let mut due = Vec::new();
    let mut panels = world.query::<&mut ThemeEditorPanel>();
    for mut editor in panels.iter_mut(world) {
        let Some(draft) = &mut editor.state.draft else {
            continue;
        };
        if draft
            .unsaved_since
            .is_some_and(|since| since.elapsed() >= SAVE_AFTER)
        {
            draft.unsaved_since = None;
            due.push((draft.id.clone(), draft.theme.clone()));
        }
    }
    for (id, theme) in due {
        let result = save(world, &id, &theme);
        crate::status::notify_failure(world, &format!("Couldn't save the theme {id}"), result);
    }
}

fn save(world: &mut World, id: &str, theme: &Theme) -> Result<(), String> {
    theme.validate()?;
    write(world, id, &theme.to_ron())
}

/// Write a theme file, then read the folder again so the change shows now
/// rather than at the next look.
fn write(world: &mut World, id: &str, text: &str) -> Result<(), String> {
    let mut folder = world.resource_mut::<ThemeFolder>();
    let path = folder.path().to_owned();
    let result = std::fs::create_dir_all(&path)
        .and_then(|()| std::fs::write(path.join(format!("{id}.ron")), text))
        .map_err(|error| error.to_string());
    *folder = ThemeFolder::new(path);
    result
}

fn remove(world: &mut World, id: &str) -> Result<(), String> {
    let mut folder = world.resource_mut::<ThemeFolder>();
    let path = folder.path().to_owned();
    let result =
        std::fs::remove_file(path.join(format!("{id}.ron"))).map_err(|error| error.to_string());
    *folder = ThemeFolder::new(path);
    result
}

/// Where a settings layer chose theme `old`, choose `new` instead, or
/// nothing (the default) without one.
fn replace_setting(world: &mut World, old: &str, new: Option<&str>) -> Result<(), String> {
    let layers: Vec<_> = world.settings().layers().collect();
    for layer in layers {
        if world.settings().get_in::<ColorTheme>(layer).as_deref() != Some(old) {
            continue;
        }
        let settings = world.settings_mut();
        match new {
            Some(new) => settings.set::<ColorTheme>(layer, new.to_owned())?,
            None => settings.clear::<ColorTheme>(layer)?,
        }
    }
    Ok(())
}

/// A file name for a theme called `name`: lowercase letters and digits
/// joined by dashes.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "theme".into()
    } else {
        slug.into()
    }
}

/// `wanted`, or `wanted-2`, `wanted-3`… whichever no file (other than
/// `keeping`'s own) or shipped theme uses.
fn unused_id(folder: &ThemeFolder, wanted: &str, keeping: Option<&str>) -> String {
    let taken = |id: &str| {
        keeping != Some(id)
            && (themes::built_in(id).is_some() || folder.path().join(format!("{id}.ron")).exists())
    };
    if !taken(wanted) {
        return wanted.into();
    }
    (2..)
        .map(|n| format!("{wanted}-{n}"))
        .find(|id| !taken(id))
        .expect("some number is free")
}

type Field = (&'static str, fn(&mut Palette) -> &mut Rgba);

const INTERFACE: &[Field] = &[
    ("Background", |p| &mut p.background),
    ("Panel background", |p| &mut p.panel_background),
    ("Surface", |p| &mut p.surface),
    ("Elevated", |p| &mut p.elevated),
    ("Border", |p| &mut p.border),
    ("Text", |p| &mut p.text),
    ("Muted", |p| &mut p.muted),
    ("Accent", |p| &mut p.accent),
    ("Selection", |p| &mut p.selection),
    ("Success", |p| &mut p.success),
    ("Warning", |p| &mut p.warning),
    ("Error", |p| &mut p.error),
];

const SYNTAX: &[Field] = &[
    ("Text", |p| &mut p.syntax.text),
    ("Comment", |p| &mut p.syntax.comment),
    ("Keyword", |p| &mut p.syntax.keyword),
    ("String", |p| &mut p.syntax.string),
    ("Number", |p| &mut p.syntax.number),
    ("Function", |p| &mut p.syntax.function),
    ("Type", |p| &mut p.syntax.r#type),
    ("Variable", |p| &mut p.syntax.variable),
    ("Parameter", |p| &mut p.syntax.parameter),
    ("Property", |p| &mut p.syntax.property),
    ("Constant", |p| &mut p.syntax.constant),
    ("Operator", |p| &mut p.syntax.operator),
    ("Punctuation", |p| &mut p.syntax.punctuation),
    ("Special", |p| &mut p.syntax.special),
];

const TERMINAL: [&str; 8] = [
    "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
];

/// The preview and every color; whether any changed.
fn colors(ui: &mut egui::Ui, context: &UiContext<'_>, theme: &mut Theme) -> bool {
    let mut changed = false;
    ui.add_space(6.0);
    preview(ui, context, theme);
    let palette = &mut theme.palette;
    for (title, fields) in [("Interface", INTERFACE), ("Syntax", SYNTAX)] {
        section(ui, title);
        egui::Grid::new(title)
            .num_columns(3)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for (label, field) in fields {
                    ui.label(*label);
                    changed |= color_edit(ui, (title, *label), field(palette));
                    ui.end_row();
                }
            });
    }
    section(ui, "Terminal");
    egui::Grid::new("terminal")
        .num_columns(5)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.weak("Normal");
            ui.label("");
            ui.weak("Bright");
            ui.end_row();
            for (i, label) in TERMINAL.iter().enumerate() {
                ui.label(*label);
                changed |= color_edit(ui, ("terminal", i), &mut palette.terminal[i]);
                changed |= color_edit(ui, ("terminal", i + 8), &mut palette.terminal[i + 8]);
                ui.end_row();
            }
        });
    ui.add_space(12.0);
    changed
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.heading(title);
    ui.add_space(2.0);
}

/// A swatch that opens a picker, and the color in hex; two grid cells.
fn color_edit(
    ui: &mut egui::Ui,
    key: impl std::hash::Hash + std::fmt::Debug,
    color: &mut Rgba,
) -> bool {
    let mut changed = false;
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let mut picked = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
    let id = ui.id().with(("color", &key));
    let swatch = ui
        .push_id(id, |ui| {
            egui::widgets::color_picker::color_edit_button_srgba(
                ui,
                &mut picked,
                egui::color_picker::Alpha::OnlyBlend,
            )
        })
        .inner;
    if swatch.changed() {
        *color = picked.to_srgba_unmultiplied().map(|v| v as f32 / 255.0);
        changed = true;
    }
    // Typed text is kept while the field has focus; otherwise it shows the color.
    let text_id = id.with("hex");
    let mut text = ui
        .data(|data| data.get_temp::<String>(text_id))
        .unwrap_or_else(|| to_hex(color));
    let field = ui.add(
        egui::TextEdit::singleline(&mut text)
            .font(egui::TextStyle::Monospace)
            .desired_width(90.0),
    );
    if field.changed()
        && let Some(typed) = from_hex(&text)
        && typed != *color
    {
        *color = typed;
        changed = true;
    }
    let focused = field.has_focus();
    ui.data_mut(|data| {
        if focused {
            data.insert_temp(text_id, text);
        } else {
            data.remove::<String>(text_id);
        }
    });
    changed
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque, as theme files write colors.
fn to_hex(color: &Rgba) -> String {
    let [r, g, b, a] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// A typed color: six or eight hex digits, with or without `#`.
fn from_hex(text: &str) -> Option<Rgba> {
    let text = text.trim();
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |i: usize| {
        digits
            .get(i..i + 2)
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .map(|v| v as f32 / 255.0)
    };
    Some([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if digits.len() == 8 { channel(6)? } else { 1.0 },
    ])
}

/// A small window drawn in the theme: a panel of code with a selected line,
/// the status colors and the terminal's sixteen.
fn preview(ui: &mut egui::Ui, context: &UiContext<'_>, theme: &Theme) {
    use bedtime_ui::theme::color;
    let p = &theme.palette;
    let s = &p.syntax;
    let font = egui::FontId::monospace(context.metrics.font_size * 0.9);
    let line_height = ui
        .painter()
        .layout_no_wrap("X".into(), font.clone(), egui::Color32::WHITE)
        .size()
        .y;
    let code: &[&[(&str, Rgba)]] = &[
        &[("// Greet each visitor", s.comment)],
        &[
            ("fn ", s.keyword),
            ("greet", s.function),
            ("(", s.punctuation),
            ("name", s.parameter),
            (": &", s.operator),
            ("str", s.r#type),
            (") {", s.punctuation),
        ],
        &[
            ("    let ", s.keyword),
            ("count", s.variable),
            (" = ", s.operator),
            ("42", s.number),
            (";", s.punctuation),
        ],
        &[
            ("    println!", s.special),
            ("(", s.punctuation),
            ("\"hi {name}\"", s.string),
            (");", s.punctuation),
        ],
        &[
            ("    self", s.constant),
            (".", s.punctuation),
            ("seen", s.property),
            (" += ", s.operator),
            ("count", s.text),
            (";", s.punctuation),
        ],
        &[("}", s.punctuation)],
    ];
    let width = ui.available_width().clamp(200.0, 520.0);
    let padding = 10.0;
    let swatch = 16.0;
    let code_height = line_height * code.len() as f32;
    let height = padding * 4.0 + code_height + line_height + swatch * 2.0 + 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, color(p.background));

    // The panel, with a line selected and a muted gutter.
    let panel = egui::Rect::from_min_size(
        rect.min + egui::vec2(padding, padding),
        egui::vec2(width - 2.0 * padding, code_height + 2.0 * padding),
    );
    painter.rect(
        panel,
        6.0,
        color(p.panel_background),
        egui::Stroke::new(1.0, color(p.border)),
        egui::StrokeKind::Inside,
    );
    let gutter = 28.0;
    let top = panel.min.y + padding;
    let selected = egui::Rect::from_min_size(
        egui::pos2(panel.min.x + 1.0, top + line_height * 2.0),
        egui::vec2(panel.width() - 2.0, line_height),
    );
    painter.rect_filled(selected, 0.0, color(p.selection));
    for (row, tokens) in code.iter().enumerate() {
        let y = top + line_height * row as f32;
        painter.text(
            egui::pos2(panel.min.x + gutter - 8.0, y),
            egui::Align2::RIGHT_TOP,
            (row + 1).to_string(),
            font.clone(),
            color(p.muted),
        );
        let mut job = egui::text::LayoutJob::default();
        for (text, rgba) in *tokens {
            job.append(
                text,
                0.0,
                egui::TextFormat::simple(font.clone(), color(*rgba)),
            );
        }
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(panel.min.x + gutter + 4.0, y),
            galley,
            color(s.text),
        );
    }

    // A control on a surface, an accent button and the status colors.
    let mut x = rect.min.x + padding;
    let y = panel.max.y + padding;
    let chip = |x: f32, label: &str, fill: Rgba, text: Rgba| {
        let galley = painter.layout_no_wrap(label.into(), font.clone(), color(text));
        let size = galley.size() + egui::vec2(16.0, 6.0);
        let chip = egui::Rect::from_min_size(egui::pos2(x, y), size);
        painter.rect(
            chip,
            4.0,
            color(fill),
            egui::Stroke::new(1.0, color(p.border)),
            egui::StrokeKind::Inside,
        );
        painter.galley(chip.min + egui::vec2(8.0, 3.0), galley, color(text));
        chip.max.x + 8.0
    };
    x = chip(x, "Surface", p.surface, p.text);
    x = chip(x, "Menu", p.elevated, p.text);
    x = chip(x, "Accent", p.accent, p.background);
    for (label, rgba) in [("ok", p.success), ("warn", p.warning), ("error", p.error)] {
        let galley = painter.layout_no_wrap(label.into(), font.clone(), color(rgba));
        let width = galley.size().x;
        painter.galley(egui::pos2(x, y + 3.0), galley, color(rgba));
        x += width + 10.0;
    }

    // The terminal's colors: normal over bright.
    let y = y + line_height + 18.0;
    for (i, rgba) in p.terminal.iter().enumerate() {
        let (column, row) = (i % 8, i / 8);
        let min = egui::pos2(
            rect.min.x + padding + column as f32 * (swatch + 4.0),
            y + row as f32 * swatch,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(min, egui::vec2(swatch + 4.0, swatch)),
            0.0,
            color(*rgba),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_file_names() {
        assert_eq!(slug("Tokyo Night Copy"), "tokyo-night-copy");
        assert_eq!(slug("  Rosé  Pine!! "), "rosé-pine");
        assert_eq!(slug("***"), "theme");
    }

    #[test]
    fn hex_colors_round_trip() {
        let color = from_hex("#7aa2f7").unwrap();
        assert_eq!(to_hex(&color), "#7aa2f7");
        assert_eq!(to_hex(&from_hex("28345780").unwrap()), "#28345780");
        assert!(from_hex("#12345").is_none());
        assert!(from_hex("#zzzzzz").is_none());
    }

    #[test]
    fn ids_avoid_files_and_shipped_themes() {
        let path = std::env::temp_dir().join(format!("bed-theme-ids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("mine.ron"), Theme::dark().to_ron()).unwrap();
        let folder = ThemeFolder::new(&path);
        assert_eq!(unused_id(&folder, "mine", None), "mine-2");
        assert_eq!(unused_id(&folder, "mine", Some("mine")), "mine");
        assert_eq!(unused_id(&folder, "tokyo", None), "tokyo-2");
        assert_eq!(unused_id(&folder, "fresh", None), "fresh");
        std::fs::remove_dir_all(path).unwrap();
    }
}
