//! Notes: personal plain-text notes for a workspace, in a panel with the list
//! down the side and the selected note's editor beside it.
//!
//! The notes (`Notes`) are a document kept in the workspace's store
//! (`notes.ron`), not as files in its folder, so they never end up in the
//! project or its repository. Every change is saved as it's made, can be
//! undone, and the notes are read again if `notes.ron` is changed elsewhere.
//! Each panel remembers which note it shows.
//!
//! Drawing reads the notes and returns what was asked (`Intent`s); `apply`
//! makes them changes to the document, right after drawing.
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
use bedtime_workbench::PanelsAppExt as _;
use bedtime_workbench::{
    BedtimeWorldExt, ByteModel, DocumentHandle, DocumentModel, DocumentReference, DocumentStatus,
    DocumentUndoGroup, EditOptions, Panel,
    persistence::{ReflectPersist, WorkbenchStore},
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    ops::Range,
    time::{Duration, Instant},
};

impl bedtime_workbench::ShowPanel for NotesPanel {
    const SHOW: &'static str = "notes.show";
}

/// The store entry the notes are kept in.
const ENTRY: &str = "notes";
/// Typing after a pause this long is a new undo step.
const TYPING_PAUSE: Duration = Duration::from_secs(1);

pub struct NotesPlugin;

impl Plugin for NotesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<NotesPanel>()
            .add_panel::<NotesPanel>()
            .add_systems(Update, attach_documents);
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

/// The workspace's notes, newest first unless moved: a document, kept in
/// the workspace's store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    #[serde(default)]
    pub notes: Vec<Note>,
    /// The id the next note gets.
    #[serde(default)]
    pub next_id: u64,
}

/// A change to the notes. Each returns its inverse, so it can be undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteChange {
    /// Put `note` at `at` in the list (or last, if that's past the end).
    Insert {
        at: usize,
        note: Note,
    },
    Remove {
        id: u64,
    },
    SetTitle {
        id: u64,
        title: String,
    },
    /// Replace `range` (in bytes) of a note's text with `text`.
    EditText {
        id: u64,
        range: Range<usize>,
        text: String,
    },
}

impl DocumentModel for Notes {
    type Change = NoteChange;
    type OpenState = ();

    fn apply(&mut self, change: &NoteChange) -> Result<NoteChange, String> {
        match change {
            NoteChange::Insert { at, note } => {
                if self.get(note.id).is_some() {
                    return Err(format!("There's already a note {}", note.id));
                }
                self.notes.insert((*at).min(self.notes.len()), note.clone());
                self.next_id = self.next_id.max(note.id + 1);
                Ok(NoteChange::Remove { id: note.id })
            }
            NoteChange::Remove { id } => {
                let at = self.index(*id)?;
                let note = self.notes.remove(at);
                Ok(NoteChange::Insert { at, note })
            }
            NoteChange::SetTitle { id, title } => {
                let at = self.index(*id)?;
                let old = std::mem::replace(&mut self.notes[at].title, title.clone());
                Ok(NoteChange::SetTitle {
                    id: *id,
                    title: old,
                })
            }
            NoteChange::EditText { id, range, text } => {
                let at = self.index(*id)?;
                let note = &mut self.notes[at].text;
                if range.start > range.end
                    || range.end > note.len()
                    || !note.is_char_boundary(range.start)
                    || !note.is_char_boundary(range.end)
                {
                    return Err("That range isn't in the note".into());
                }
                let old = note[range.clone()].to_owned();
                note.replace_range(range.clone(), text);
                Ok(NoteChange::EditText {
                    id: *id,
                    range: range.start..range.start + text.len(),
                    text: old,
                })
            }
        }
    }
}

/// Kept as RON (`notes.ron`). Nothing there yet is no notes.
impl ByteModel for Notes {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        ron::de::from_bytes(bytes).map_err(|e| e.to_string())
    }

    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        out.write_all(text.as_bytes())
    }
}

impl Notes {
    pub fn get(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    fn index(&self, id: u64) -> Result<usize, String> {
        self.notes
            .iter()
            .position(|note| note.id == id)
            .ok_or_else(|| format!("There's no note {id}"))
    }

    /// The id a new note gets: never one in use, even if `next_id` was lost
    /// or edited by hand.
    fn fresh_id(&self) -> u64 {
        self.next_id
            .max(self.notes.iter().map(|note| note.id + 1).max().unwrap_or(0))
    }

    /// A new note at the top of the list: its id, and the change.
    pub fn new_note(&self, title: impl Into<String>, text: impl Into<String>) -> (u64, NoteChange) {
        let id = self.fresh_id();
        let note = Note {
            id,
            title: title.into(),
            text: text.into(),
        };
        (id, NoteChange::Insert { at: 0, note })
    }

    /// A copy of note `id`, just below it: the copy's id, and the change.
    pub fn copy_note(&self, id: u64) -> Option<(u64, NoteChange)> {
        let at = self.index(id).ok()?;
        let original = &self.notes[at];
        let copy = self.fresh_id();
        let note = Note {
            id: copy,
            title: format!("{} copy", original.label()),
            text: original.text.clone(),
        };
        Some((copy, NoteChange::Insert { at: at + 1, note }))
    }

    /// The note to show once `id` is removed: the one that takes its place
    /// in the list, else the one before.
    pub fn shown_after_removing(&self, id: u64) -> Option<u64> {
        let at = self.index(id).ok()?;
        self.notes
            .get(at + 1)
            .or_else(|| self.notes.get(at.wrapping_sub(1)))
            .map(|note| note.id)
    }

    /// The edit that turns note `id`'s text into `text`: the part between
    /// what they start and end with.
    pub fn text_change(&self, id: u64, text: &str) -> Option<NoteChange> {
        let old = &self.get(id)?.text;
        if old == text {
            return None;
        }
        let mut start = old
            .bytes()
            .zip(text.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(start) || !text.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = old.len().min(text.len()) - start;
        end = old.as_bytes()[start..]
            .iter()
            .rev()
            .zip(text.as_bytes()[start..].iter().rev())
            .take(end)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - end) || !text.is_char_boundary(text.len() - end) {
            end -= 1;
        }
        Some(NoteChange::EditText {
            id,
            range: start..old.len() - end,
            text: text[start..text.len() - end].to_owned(),
        })
    }
}

/// The Notes panel. Which note it shows, and whether the list is hidden, are
/// saved with it.
#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default, Persist)]
#[require(NotesView)]
pub struct NotesPanel {
    pub selected: Option<u64>,
    /// The list of notes is collapsed, leaving the editor the whole panel.
    #[reflect(default)]
    pub list_hidden: bool,
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
    /// Typing in a note's title or text: one undo step until a pause, or
    /// something else is done.
    typing: Option<Typing>,
}

struct Typing {
    note: u64,
    field: Field,
    group: DocumentUndoGroup,
    last: Instant,
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
    /// Hide the list of notes (true) or show it.
    HideList(bool),
}

/// The notes document `panel` shows, if it has one.
fn document(world: &World, panel: Entity) -> Option<DocumentHandle<Notes>> {
    let id = world.get::<DocumentReference>(panel)?.document();
    world.bedtime().cast_document(id).ok()
}

/// Give Notes panels the workspace's notes: restored, opened, or after
/// switching workspace.
fn attach_documents(world: &mut World) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, (With<NotesPanel>, Without<DocumentReference>)>()
        .iter(world)
        .collect();
    for panel in panels {
        attach(world, panel);
    }
}

fn attach(world: &mut World, panel: Entity) {
    match world
        .bedtime_mut()
        .store_document::<Notes, WorkbenchStore>(ENTRY, "Notes")
    {
        Ok(notes) => {
            world.entity_mut(panel).insert(DocumentReference::of(notes));
        }
        Err(error) => warn!("The notes couldn't be opened: {error}"),
    }
}

/// Do what panel `panel` asked: changes to the notes, made one at a time.
pub fn apply(world: &mut World, panel: Entity, intents: Vec<Intent>) {
    for intent in intents {
        if let Intent::HideList(hidden) = intent {
            if let Some(mut shown) = world.get_mut::<NotesPanel>(panel)
                && shown.list_hidden != hidden
            {
                shown.list_hidden = hidden;
            }
            continue;
        }
        let Some(handle) = document(world, panel) else {
            return;
        };
        let Ok(notes) = world.bedtime().read_document(handle) else {
            return;
        };
        let shown = world.get::<NotesPanel>(panel).and_then(|p| p.selected);
        // The change, its label, typing in which field, and what to select.
        let (change, label, typing, select) = match intent {
            Intent::HideList(_) => unreachable!(),
            Intent::New => {
                let (id, change) = notes.new_note("", "");
                (
                    Some(change),
                    "New Note",
                    None,
                    Some((Some(id), Some(Field::Title))),
                )
            }
            Intent::Select(id) => (None, "", None, Some((Some(id), None))),
            Intent::Rename(id, title) => {
                let changed = notes.get(id).is_some_and(|note| note.title != title);
                let change = changed.then_some(NoteChange::SetTitle { id, title });
                (change, "Rename Note", Some((id, Field::Title)), None)
            }
            Intent::SetText(id, text) => {
                let change = notes.text_change(id, &text);
                (change, "Typing", Some((id, Field::Text)), None)
            }
            Intent::Duplicate(id) => match notes.copy_note(id) {
                Some((copy, change)) => (
                    Some(change),
                    "Duplicate Note",
                    None,
                    Some((Some(copy), None)),
                ),
                None => (None, "", None, None),
            },
            Intent::Delete(id) => {
                let next = notes.shown_after_removing(id);
                let select = (shown == Some(id)).then_some((next, None));
                (Some(NoteChange::Remove { id }), "Delete Note", None, select)
            }
        };
        if let Some(change) = change
            && let Err(error) = edit(world, panel, handle, change, label, typing)
        {
            warn!("The notes couldn't be changed: {error}");
        }
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

/// Make `change`. Typing in the same field of the same note, without a
/// pause, joins the last undo step; anything else starts a new one.
fn edit(
    world: &mut World,
    panel: Entity,
    handle: DocumentHandle<Notes>,
    change: NoteChange,
    label: &str,
    typing: Option<(u64, Field)>,
) -> Result<(), String> {
    let id = handle.id();
    let revision = world
        .bedtime()
        .inspect_document(id)
        .map_err(|e| e.to_string())?
        .revision;
    let now = Instant::now();
    let joined = world.get::<NotesView>(panel).and_then(|view| {
        let current = view.typing.as_ref()?;
        let (note, field) = typing?;
        (current.note == note && current.field == field && now - current.last < TYPING_PAUSE)
            .then_some(current.group)
    });
    let options = match (joined, typing) {
        (Some(group), _) => EditOptions::grouped(label, group),
        (None, Some((note, field))) => {
            let group = world
                .bedtime_mut()
                .begin_document_undo_group(id, panel)
                .map_err(|e| e.to_string())?;
            if let Some(mut view) = world.get_mut::<NotesView>(panel) {
                view.typing = Some(Typing {
                    note,
                    field,
                    group,
                    last: now,
                });
            }
            EditOptions::grouped(label, group)
        }
        (None, None) => {
            if let Some(mut view) = world.get_mut::<NotesView>(panel) {
                view.typing = None;
            }
            EditOptions::new(label)
        }
    };
    match world
        .bedtime_mut()
        .edit_document(handle, revision, change.clone(), options)
    {
        Ok(_) => {}
        // Something else changed the notes since (an undo): start a new step.
        Err(bedtime_workbench::DocumentError::StaleUndoGroup) if joined.is_some() => {
            if let Some(mut view) = world.get_mut::<NotesView>(panel) {
                view.typing = None;
            }
            return edit(world, panel, handle, change, label, typing);
        }
        Err(error) => return Err(error.to_string()),
    }
    if let Some(typing) = world
        .get_mut::<NotesView>(panel)
        .and_then(|view| view.into_inner().typing.as_mut())
    {
        typing.last = now;
    }
    Ok(())
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
        let loaded = document(world, panel).and_then(|handle| {
            let info = world.bedtime().inspect_document(handle.id()).ok()?;
            match info.status {
                DocumentStatus::Ready => world.bedtime().read_document(handle).ok(),
                DocumentStatus::Loading => {
                    ui.centered_and_justified(|ui| ui.spinner());
                    None
                }
                DocumentStatus::Failed(error) => {
                    ui.weak(format!("The notes can't be read: {error}"));
                    None
                }
            }
        });
        let Some(notes) = loaded else {
            world.entity_mut(panel).insert((shown, view));
            return;
        };
        // A note deleted elsewhere (or none chosen yet): show the first.
        if shown.selected.is_none_or(|id| notes.get(id).is_none()) {
            shown.selected = notes.notes.first().map(|note| note.id);
        }
        let mut intents = Vec::new();
        let margin = egui::Margin::symmetric(8, 6);
        let hidden = shown.list_hidden;
        if !hidden {
            egui::Panel::left(egui::Id::new((panel, "notes-list")))
                .resizable(true)
                .default_size(190.0)
                .size_range(120.0..=420.0)
                .frame(egui::Frame::NONE.inner_margin(margin))
                .show(ui, |ui| {
                    sidebar(ui, context, panel, notes, &shown, &mut view, &mut intents)
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| match shown.selected.and_then(|id| notes.get(id)) {
                Some(note) => editor(ui, context, panel, note, hidden, &mut view, &mut intents),
                None => {
                    if hidden {
                        list_toggle(ui, context, true, &mut intents);
                        ui.add_space(6.0);
                    }
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
        list_toggle(ui, context, false, intents);
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

/// The button that hides the list (in its header) or shows it again (beside
/// the note's title).
fn list_toggle(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    hidden: bool,
    intents: &mut Vec<Intent>,
) {
    let label = if hidden {
        "Show Note List"
    } else {
        "Hide Note List"
    };
    let mut frame = context.frame(ui);
    let button = frame.icon_button(BuiltinIcon::PanelLeft);
    frame.tooltip(&button, label);
    button.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if button.clicked() {
        intents.push(Intent::HideList(!hidden));
    }
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

/// The selected note: its title, then its text, filling the rest. With the
/// list hidden, the buttons to show it and to add a note sit before the
/// title.
fn editor(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    note: &Note,
    list_hidden: bool,
    view: &mut NotesView,
    intents: &mut Vec<Intent>,
) {
    let mut title = note.title.clone();
    let title_id = egui::Id::new((panel, "notes-title", note.id));
    let title_field = ui
        .horizontal(|ui| {
            if list_hidden {
                list_toggle(ui, context, true, intents);
                let mut frame = context.frame(ui);
                let new = frame.icon_button(BuiltinIcon::Plus);
                frame.tooltip(&new, "New Note");
                new.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "New Note")
                });
                if new.clicked() {
                    intents.push(Intent::New);
                }
                ui.add_space(4.0);
            }
            ui.add(
                egui::TextEdit::singleline(&mut title)
                    .id(title_id)
                    .hint_text("Untitled")
                    .font(egui::FontId::proportional(context.metrics.font_size * 1.25))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            )
        })
        .inner;
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
