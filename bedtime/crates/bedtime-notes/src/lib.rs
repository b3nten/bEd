//! Notes: personal plain-text notes for a workspace, in a panel with the list
//! down the side and the selected note's editor beside it.
//!
//! The notes (`Notes`) are kept in the workspace's store (`notes.ron`), not as
//! files in its folder, so they never end up in the project or its
//! repository. Each panel remembers which note it shows.
//!
//! Drawing reads the notes and returns what was asked (`Intent`s); `apply`
//! does it, right after drawing.
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
use bedtime_workbench::{
    Panel,
    persistence::{Persisted, PersistenceAppExt, ReflectPersist, WorkbenchStore},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Open a Notes panel: a new one, in the focused area.
pub struct ShowNotes;
impl bedtime_workbench::Action for ShowNotes {
    const KEY: &'static str = "notes.show";
    const LABEL: &'static str = "Notes";
}

pub struct NotesPlugin;

impl Plugin for NotesPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{ActionAppExt, MenusAppExt};
        app.register_type::<NotesPanel>()
            .add_action_handler::<ShowNotes>(bedtime_workbench::Handler::new(
                bedtime_workbench::open_new::<NotesPanel>,
            ))
            .add_menu_item::<ShowNotes>(bedtime_workbench::StandardMenu::View, "panels")
            .init_resource::<Notes>()
            .persist::<Notes, WorkbenchStore>()
            // The notes are the workspace's; the next workspace's are loaded
            // after this.
            .add_systems(
                bedtime_workbench::OnWorkspaceClose,
                |mut commands: Commands| commands.insert_resource(Notes::default()),
            );
    }
}

/// One note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    /// May be empty: the list then shows the first line of the text.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
}

impl Note {
    /// What the list shows for it.
    pub fn label(&self) -> String {
        let title = self.title.trim();
        if !title.is_empty() {
            return title.to_owned();
        }
        self.text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| "Untitled".to_owned(), str::to_owned)
    }
}

/// The workspace's notes, newest first unless moved. Saved in its store.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Persisted)]
#[persisted(key = "notes")]
pub struct Notes {
    #[serde(default)]
    pub notes: Vec<Note>,
    /// The id the next note gets.
    #[serde(default)]
    pub next_id: u64,
}

impl Notes {
    pub fn get(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.notes.iter_mut().find(|note| note.id == id)
    }

    fn fresh_id(&mut self) -> u64 {
        // Never reuse an id, even if `next_id` was lost or edited by hand.
        let id = self
            .next_id
            .max(self.notes.iter().map(|note| note.id + 1).max().unwrap_or(0));
        self.next_id = id + 1;
        id
    }

    /// A new note at the top of the list. Returns its id.
    pub fn add(&mut self, title: impl Into<String>, text: impl Into<String>) -> u64 {
        let id = self.fresh_id();
        self.notes.insert(
            0,
            Note {
                id,
                title: title.into(),
                text: text.into(),
            },
        );
        id
    }

    /// A copy of note `id`, just below it. Returns the copy's id.
    pub fn duplicate(&mut self, id: u64) -> Option<u64> {
        let at = self.notes.iter().position(|note| note.id == id)?;
        let mut copy = self.notes[at].clone();
        copy.id = self.fresh_id();
        copy.title = format!("{} copy", self.notes[at].label());
        let copied = copy.id;
        self.notes.insert(at + 1, copy);
        Some(copied)
    }

    /// Remove note `id`. Returns the note to show instead: the one that took
    /// its place in the list, else the one before.
    pub fn remove(&mut self, id: u64) -> Option<u64> {
        let at = self.notes.iter().position(|note| note.id == id)?;
        self.notes.remove(at);
        self.notes
            .get(at)
            .or_else(|| self.notes.get(at.wrapping_sub(1)))
            .map(|note| note.id)
    }
}

/// The Notes panel. Which note it shows is saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
#[require(NotesView)]
pub struct NotesPanel {
    pub selected: Option<u64>,
}

/// A panel's prompts and pending focus. Not saved.
#[derive(Component, Default)]
pub struct NotesView {
    /// The note being renamed in the list, and its name so far.
    renaming: Option<(u64, String)>,
    focus_rename: bool,
    /// Asking whether to delete this note.
    confirm_delete: Option<u64>,
    /// Give the keyboard to the title (a new note) or the text.
    focus: Option<Field>,
    /// Only notes whose label or text has this are listed.
    filter: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Title,
    Text,
}

/// What drawing asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    New,
    Select(u64),
    Rename(u64, String),
    SetText(u64, String),
    Duplicate(u64),
    Delete(u64),
}

/// Do what panel `panel` asked.
pub fn apply(world: &mut World, panel: Entity, intents: Vec<Intent>) {
    for intent in intents {
        let select = {
            let mut notes = world.resource_mut::<Notes>();
            match intent {
                Intent::New => Some((Some(notes.add("", "")), Some(Field::Title))),
                Intent::Select(id) => Some((Some(id), None)),
                Intent::Rename(id, title) => {
                    if let Some(note) = notes.get_mut(id)
                        && note.title != title
                    {
                        note.title = title;
                    }
                    None
                }
                Intent::SetText(id, text) => {
                    if let Some(note) = notes.get_mut(id)
                        && note.text != text
                    {
                        note.text = text;
                    }
                    None
                }
                Intent::Duplicate(id) => notes.duplicate(id).map(|copy| (Some(copy), None)),
                Intent::Delete(id) => {
                    let next = notes.remove(id);
                    let shown = world.get::<NotesPanel>(panel).and_then(|p| p.selected);
                    (shown == Some(id)).then_some((next, None))
                }
            }
        };
        if let Some((selected, focus)) = select {
            if let Some(mut shown) = world.get_mut::<NotesPanel>(panel)
                && shown.selected != selected
            {
                shown.selected = selected;
            }
            if let Some(mut view) = world.get_mut::<NotesView>(panel) {
                view.focus = focus;
                view.confirm_delete = None;
            }
        }
    }
}

/// Whether `note` is listed while filtering by `filter`.
fn listed(note: &Note, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || note.label().to_lowercase().contains(&filter)
        || note.text.to_lowercase().contains(&filter)
}

impl Panel for NotesPanel {
    const LABEL: &'static str = "Notes";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut shown) = world.entity_mut(panel).take::<NotesPanel>() else {
            return;
        };
        let Some(mut view) = world.entity_mut(panel).take::<NotesView>() else {
            world.entity_mut(panel).insert(shown);
            return;
        };
        let notes = world.resource::<Notes>();
        // A note deleted elsewhere (or none chosen yet): show the first.
        if shown.selected.is_none_or(|id| notes.get(id).is_none()) {
            shown.selected = notes.notes.first().map(|note| note.id);
        }
        let mut intents = Vec::new();
        let margin = egui::Margin::symmetric(8, 6);
        egui::Panel::left(egui::Id::new((panel, "notes-list")))
            .resizable(true)
            .default_size(190.0)
            .size_range(120.0..=420.0)
            .frame(egui::Frame::NONE.inner_margin(margin))
            .show(ui, |ui| {
                sidebar(ui, context, panel, notes, &shown, &mut view, &mut intents)
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| match shown.selected.and_then(|id| notes.get(id)) {
                Some(note) => editor(ui, context, panel, note, &mut view, &mut intents),
                None => {
                    ui.weak("No notes in this workspace yet.");
                    ui.add_space(6.0);
                    if ui.button("New Note").clicked() {
                        intents.push(Intent::New);
                    }
                }
            });
        world.entity_mut(panel).insert((shown, view));
        apply(world, panel, intents);
    }
}

fn sidebar(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    notes: &Notes,
    shown: &NotesPanel,
    view: &mut NotesView,
    intents: &mut Vec<Intent>,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Notes").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut frame = context.frame(ui);
            let new = frame.icon_button(BuiltinIcon::Plus);
            frame.tooltip(&new, "New Note");
            new.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New Note")
            });
            if new.clicked() {
                intents.push(Intent::New);
            }
        });
    });
    ui.add_space(4.0);
    if notes.notes.len() > 1 || !view.filter.is_empty() {
        let field = ui.add(
            egui::TextEdit::singleline(&mut view.filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY),
        );
        field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Filter"));
        ui.add_space(4.0);
    }
    egui::ScrollArea::vertical()
        .id_salt((panel, "notes"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let filter = view.filter.clone();
            let mut any = false;
            for note in notes.notes.iter().filter(|note| listed(note, &filter)) {
                any = true;
                let id = note.id;
                if view
                    .renaming
                    .as_ref()
                    .is_some_and(|(renaming, _)| *renaming == id)
                {
                    rename_field(ui, panel, view, intents);
                    continue;
                }
                let label = note.label();
                let response = row(ui, context, shown.selected == Some(id), &label);
                if response.clicked() {
                    intents.push(Intent::Select(id));
                }
                if response.double_clicked() {
                    view.renaming = Some((id, note.title.clone()));
                    view.focus_rename = true;
                }
                response.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        view.renaming = Some((id, note.title.clone()));
                        view.focus_rename = true;
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        intents.push(Intent::Duplicate(id));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Delete…").clicked() {
                        view.confirm_delete = Some(id);
                        ui.close();
                    }
                });
                if view.confirm_delete == Some(id) {
                    ui.horizontal(|ui| {
                        ui.weak("Delete?");
                        if ui.small_button("Delete").clicked() {
                            intents.push(Intent::Delete(id));
                            view.confirm_delete = None;
                        }
                        if ui.small_button("Cancel").clicked() {
                            view.confirm_delete = None;
                        }
                    });
                }
            }
            if !any && !notes.notes.is_empty() {
                ui.weak(format!("Nothing matches “{}”", filter.trim()));
            }
        });
}

/// The name of the note being renamed, in its row: Enter (or clicking away)
/// keeps it, Escape doesn't.
fn rename_field(ui: &mut egui::Ui, panel: Entity, view: &mut NotesView, intents: &mut Vec<Intent>) {
    let Some((id, name)) = view.renaming.as_mut() else {
        return;
    };
    let id = *id;
    let field_id = egui::Id::new((panel, "notes-rename", id));
    let response = ui.add(
        egui::TextEdit::singleline(name)
            .id(field_id)
            .hint_text("Untitled")
            .desired_width(f32::INFINITY),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Name"));
    if std::mem::take(&mut view.focus_rename) {
        response.request_focus();
        // All of it selected, to type over.
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field_id) {
            let end = egui::text::CCursor::new(name.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    end,
                )));
            state.store(ui.ctx(), field_id);
        }
        return;
    }
    if response.lost_focus() {
        if !ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            intents.push(Intent::Rename(id, name.trim().to_owned()));
        }
        view.renaming = None;
    }
}

/// The selected note: its title, then its text, filling the rest.
fn editor(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    note: &Note,
    view: &mut NotesView,
    intents: &mut Vec<Intent>,
) {
    let mut title = note.title.clone();
    let title_id = egui::Id::new((panel, "notes-title", note.id));
    let title_field = ui.add(
        egui::TextEdit::singleline(&mut title)
            .id(title_id)
            .hint_text("Untitled")
            .font(egui::FontId::proportional(context.metrics.font_size * 1.25))
            .frame(egui::Frame::NONE)
            .desired_width(f32::INFINITY),
    );
    title_field
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Title"));
    if title != note.title {
        intents.push(Intent::Rename(note.id, title));
    }
    // Enter in the title goes on to the text.
    if title_field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
        view.focus = Some(Field::Text);
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    let mut text = note.text.clone();
    let text_id = egui::Id::new((panel, "notes-text", note.id));
    let output = egui::ScrollArea::vertical()
        .id_salt((panel, "notes-text-scroll", note.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::TextEdit::multiline(&mut text)
                .id(text_id)
                .hint_text("Write something…")
                .font(egui::FontId::proportional(context.metrics.font_size))
                .frame(egui::Frame::NONE)
                .lock_focus(true)
                .desired_width(f32::INFINITY)
                .min_size(ui.available_size())
                .show(ui)
        })
        .inner;
    output
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Note"));
    if text != note.text {
        intents.push(Intent::SetText(note.id, text));
    }
    match view.focus.take() {
        Some(Field::Title) => title_field.request_focus(),
        Some(Field::Text) => output.response.request_focus(),
        None => {}
    }
}

/// A selectable row of the list: its label cut to fit.
fn row(ui: &mut egui::Ui, context: &UiContext<'_>, selected: bool, text: &str) -> egui::Response {
    let p = &context.theme.palette;
    let height = context.metrics.font_size + 10.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, text)
    });
    let painter = ui.painter_at(rect);
    let fill = if selected {
        Some(p.selection)
    } else if response.hovered() {
        Some(theme::mix(p.panel_background, p.text, 0.08))
    } else {
        None
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, context.metrics.rounding, theme::color(fill));
    }
    let ink = theme::color(p.text);
    let x = rect.left() + 6.0;
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(context.metrics.font_size), ink),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.right() - 6.0 - x).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response
}

#[cfg(test)]
mod tests;
