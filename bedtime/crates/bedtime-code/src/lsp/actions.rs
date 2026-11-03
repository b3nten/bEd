//! Code actions, rename, find references and formatting, as actions on the
//! focused editor. Each request goes to the server at once or fails at once;
//! only each kind's latest answer is used. Edits apply to open documents as
//! one undoable change each (left unsaved), and to other files on disk if
//! they haven't changed since they were read.
use super::{
    Answer, Ask, AskKind, CodeAction, DocumentServer, Encoding, LanguageServer, Location,
    RequestOrigin, WorkspaceEdit, documents::answering_server, language_activity, server_of,
};
use crate::editor::{EditorOutput, EditorPanel, Indentation, editor_id, word_at};
use bedtime_document::{BufferEdit, TextBuffer};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::{
    Action, ActionAppExt, ActionInvocation, ActionResult, DocumentId, DocumentReference,
    EditOptions, Handler, InArea, KeyChord, LayoutsWorldExt, Modifiers, Notification, Panel,
    PanelInstance, PathOptions, Root, ViewerFile, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

pub struct CodeActions;
impl Action for CodeActions {
    const KEY: &'static str = "code.actions";
    const LABEL: &'static str = "Code Actions…";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Period)];
}

pub struct RenameSymbol;
impl Action for RenameSymbol {
    const KEY: &'static str = "code.rename";
    const LABEL: &'static str = "Rename Symbol…";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(Modifiers::NONE, KeyCode::F2)];
}

pub struct FindReferences;
impl Action for FindReferences {
    const KEY: &'static str = "code.find_references";
    const LABEL: &'static str = "Find References";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(Modifiers::SHIFT, KeyCode::F12)];
}

pub struct FormatDocument;
impl Action for FormatDocument {
    const KEY: &'static str = "code.format";
    const LABEL: &'static str = "Format Document";
    // Not Alt-based: Alt with a letter types a character while editing.
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::new(
        Modifiers::PRIMARY.union(Modifiers::SHIFT),
        KeyCode::KeyI,
    )];
}

/// Each kind's latest request, and what's open: the code action menu, the
/// rename box, references to open.
#[derive(Resource, Default)]
struct Asks {
    latest: HashMap<AskKind, RequestOrigin>,
    next_ticket: u64,
    menu: Option<Menu>,
    rename: Option<RenameBox>,
    /// What references were asked for (the panel's title).
    references_of: String,
    /// References clicked, opened in `Update` (opening can close panels).
    open: Vec<(PathBuf, u32, u32)>,
}

struct Menu {
    panel: Entity,
    actions: Vec<CodeAction>,
    selected: usize,
}

struct RenameBox {
    panel: Entity,
    byte: usize,
    name: String,
    focused: bool,
}

pub(crate) fn install(app: &mut App) {
    let editing =
        |world: &World, panel: Option<Entity>| panel.is_some_and(|p| server_of(world, p).is_some());
    app.init_resource::<Asks>()
        .add_action_handler::<CodeActions>(Handler::new(code_actions).available_when(editing))
        .add_action_handler::<RenameSymbol>(Handler::new(rename).available_when(editing))
        .add_action_handler::<FindReferences>(Handler::new(find_references).available_when(editing))
        .add_action_handler::<FormatDocument>(Handler::new(format).available_when(editing))
        .add_systems(Update, open_references);
}

/// Send `ask` about `panel`'s document now, or say why it can't be.
fn ask(world: &mut World, panel: Entity, ask: Ask) -> Result<(), String> {
    let (server, mut origin) = answering_server(world, panel)?;
    let mut asks = world.resource_mut::<Asks>();
    asks.next_ticket += 1;
    origin.ticket = asks.next_ticket;
    asks.latest.insert(ask.kind(), origin);
    world
        .get::<LanguageServer>(server)
        .ok_or("The language server has stopped")?
        .handle
        .ask(origin, ask)
}

/// The focused editor and its document's text.
fn editor(world: &World, invocation: &ActionInvocation) -> Result<(Entity, TextBuffer), String> {
    let panel = invocation.entity.ok_or("No editor is focused")?;
    world
        .get::<EditorPanel>(panel)
        .ok_or("No editor is focused")?;
    let document = world
        .get::<DocumentReference>(panel)
        .ok_or("No document is shown")?
        .document();
    let workbench = world.workbench();
    let text = workbench
        .cast_document::<TextBuffer>(document)
        .and_then(|handle| workbench.read_document(handle).cloned())
        .map_err(|e| e.to_string())?;
    Ok((panel, text))
}

fn code_actions(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let (panel, _) = editor(world, &invocation)?;
    let range = world
        .get::<EditorPanel>(panel)
        .unwrap()
        .view
        .selection()
        .range();
    request_code_actions(world, panel, range)
}

/// Ask what can be done at `range` in `panel`'s document; the answer opens
/// the menu (`offered_code_actions`).
pub fn request_code_actions(
    world: &mut World,
    panel: Entity,
    range: std::ops::Range<usize>,
) -> Result<(), String> {
    ask(world, panel, Ask::CodeActions { range })
}

/// The code actions offered in `panel`'s menu, while it's open.
pub fn offered_code_actions(world: &World, panel: Entity) -> Option<&[CodeAction]> {
    let menu = world.resource::<Asks>().menu.as_ref()?;
    (menu.panel == panel).then_some(menu.actions.as_slice())
}

/// Do the menu's code action `index` (and close the menu).
pub fn choose_code_action(world: &mut World, panel: Entity, index: usize) {
    let mut asks = world.resource_mut::<Asks>();
    if asks.menu.as_ref().is_some_and(|menu| menu.panel == panel)
        && let Some(action) = asks.menu.take().unwrap().actions.get(index).cloned()
    {
        perform(world, panel, action);
    }
}

/// Rename the symbol at `byte` in `panel`'s document to `name`, everywhere.
pub fn rename_symbol(
    world: &mut World,
    panel: Entity,
    byte: usize,
    name: String,
) -> Result<(), String> {
    ask(world, panel, Ask::Rename { byte, name })
}

/// Find where the symbol at `byte` is used; the answer opens in References.
pub fn find_references_at(world: &mut World, panel: Entity, byte: usize) -> Result<(), String> {
    ask(world, panel, Ask::References { byte })
}

/// Format `panel`'s document with its editor's indentation.
pub fn format_document(world: &mut World, panel: Entity) -> Result<(), String> {
    let indentation = world
        .get::<EditorPanel>(panel)
        .ok_or("No editor")?
        .view
        .settings
        .indentation;
    ask(
        world,
        panel,
        Ask::Format {
            tab_size: indentation.width() as u32,
            spaces: indentation != Indentation::Tabs,
        },
    )
}

/// Open the rename box at the word at the caret.
fn rename(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let (panel, text) = editor(world, &invocation)?;
    let head = world
        .get::<EditorPanel>(panel)
        .unwrap()
        .view
        .selection()
        .head();
    let word = symbol_at(&text, head).ok_or("There's no name here to rename")?;
    world.resource_mut::<Asks>().rename = Some(RenameBox {
        panel,
        byte: word.start,
        name: text.range_text(word),
        focused: false,
    });
    Ok(())
}

fn find_references(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let (panel, text) = editor(world, &invocation)?;
    let head = world
        .get::<EditorPanel>(panel)
        .unwrap()
        .view
        .selection()
        .head();
    let word = symbol_at(&text, head).ok_or("There's no name here")?;
    world.resource_mut::<Asks>().references_of = text.range_text(word.clone());
    find_references_at(world, panel, word.start)
}

fn format(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let (panel, _) = editor(world, &invocation)?;
    format_document(world, panel)
}

/// The name at `byte`, or just before it (the caret after a name).
fn symbol_at(text: &TextBuffer, byte: usize) -> Option<std::ops::Range<usize>> {
    let is_name = |range: &std::ops::Range<usize>| {
        !range.is_empty()
            && text
                .range_text(range.clone())
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
    };
    let here = word_at(text, byte);
    if is_name(&here) {
        return Some(here);
    }
    (byte > 0)
        .then(|| word_at(text, text.previous_grapheme(byte)))
        .filter(is_name)
}

fn label(kind: AskKind) -> &'static str {
    match kind {
        AskKind::CodeActions | AskKind::ResolveCodeAction | AskKind::Command => "Code action",
        AskKind::Rename => "Rename",
        AskKind::References => "Find References",
        AskKind::Format => "Format",
    }
}

fn tell(world: &mut World, notification: Notification) {
    world.workbench_mut().notify(notification);
}

/// A server's answer to one of these requests.
pub(crate) fn answered(
    world: &mut World,
    origin: RequestOrigin,
    kind: AskKind,
    result: Result<Answer, String>,
) {
    let mut asks = world.resource_mut::<Asks>();
    if asks.latest.get(&kind) != Some(&origin) {
        return;
    }
    asks.latest.remove(&kind);
    let Some(panel) = Entity::try_from_bits(origin.panel) else {
        return;
    };
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            tell(
                world,
                Notification::warning(format!("{}: {error}", label(kind))),
            );
            return;
        }
    };
    match answer {
        Answer::CodeActions(actions) if actions.is_empty() => {
            tell(world, Notification::info("No code actions here"));
        }
        Answer::CodeActions(actions) => {
            world.resource_mut::<Asks>().menu = Some(Menu {
                panel,
                actions,
                selected: 0,
            });
        }
        Answer::CodeAction(action) => perform(world, panel, action),
        Answer::Edit(edit) => match apply_workspace_edit(world, &edit, label(kind)) {
            Ok(files) if kind == AskKind::Rename => tell(
                world,
                Notification::info(match files {
                    0 => "Nothing was renamed".to_owned(),
                    1 => "Renamed in 1 file".to_owned(),
                    n => format!("Renamed in {n} files"),
                }),
            ),
            Ok(_) => {}
            Err(error) => tell(
                world,
                Notification::warning(format!("{}: {error}", label(kind))),
            ),
        },
        Answer::References(locations) => show_references(world, locations),
        Answer::Done => {}
    }
}

/// Do a chosen code action: resolve it first if the server sent it bare,
/// then apply its edit and run its command.
fn perform(world: &mut World, panel: Entity, action: CodeAction) {
    if let Some(reason) = &action.disabled {
        tell(
            world,
            Notification::info(format!("{}: {reason}", action.title)),
        );
        return;
    }
    let result = if action.unresolved() {
        ask(world, panel, Ask::ResolveCodeAction(action.raw.clone()))
    } else {
        let encoding = language_activity(world, panel)
            .and_then(|activity| activity.encoding)
            .unwrap_or_default();
        let edited = match action.edit(encoding) {
            Some(edit) => edit.and_then(|edit| apply_workspace_edit(world, &edit, &action.title)),
            None => Ok(0),
        };
        edited.and_then(|_| match action.command() {
            Some(command) => ask(world, panel, Ask::Command(command)),
            None => Ok(()),
        })
    };
    if let Err(error) = result {
        tell(
            world,
            Notification::warning(format!("{}: {error}", action.title)),
        );
    }
}

/// Apply `edit`: to an open document as one undoable change (left unsaved),
/// else to the file on disk if it hasn't changed since it was read. Returns
/// how many files changed; every file is tried, and the problems reported.
pub(crate) fn apply_workspace_edit(
    world: &mut World,
    edit: &WorkspaceEdit,
    label: &str,
) -> Result<usize, String> {
    let mut changed = 0;
    let mut problems = Vec::new();
    for file in &edit.files {
        if file.edits.is_empty() {
            continue;
        }
        let path = file.uri.path();
        let result = match document_for(world, &path) {
            Some(document) => edit_document(world, document, &file.edits, edit.encoding, label),
            None => edit_file(&path, &file.edits, edit.encoding),
        };
        match result {
            Ok(()) => changed += 1,
            Err(error) => problems.push(format!(
                "{}: {error}",
                path.file_name().unwrap_or(path.as_os_str()).display()
            )),
        }
    }
    if problems.is_empty() {
        Ok(changed)
    } else {
        Err(problems.join("; "))
    }
}

/// The open document for the file at `path`.
fn document_for(world: &mut World, path: &Path) -> Option<DocumentId> {
    if let Some(entity) = world
        .query::<(Entity, &DocumentServer)>()
        .iter(world)
        .find(|(_, server)| server.file == path)
        .map(|(entity, _)| entity)
    {
        return Some(DocumentId(entity));
    }
    world
        .query_filtered::<(&ViewerFile, &DocumentReference), With<EditorPanel>>()
        .iter(world)
        .find(|(file, _)| file.0 == path)
        .map(|(_, reference)| reference.document())
}

/// Edits converted to byte ranges in `text`, last first (so each applies to
/// the text the next one was made for). Overlapping edits are refused.
fn byte_edits(
    text: &TextBuffer,
    edits: &[(super::ProtocolRange, String)],
    encoding: Encoding,
) -> Result<Vec<(std::ops::Range<usize>, String)>, String> {
    let mut edits = edits
        .iter()
        .map(|(range, replacement)| Ok((encoding.range(text, *range)?, replacement.clone())))
        .collect::<Result<Vec<_>, String>>()?;
    edits.sort_by_key(|(range, _)| std::cmp::Reverse((range.start, range.end)));
    if edits.windows(2).any(|pair| pair[1].0.end > pair[0].0.start) {
        return Err("The server sent overlapping edits".into());
    }
    Ok(edits)
}

fn edit_document(
    world: &mut World,
    document: DocumentId,
    edits: &[(super::ProtocolRange, String)],
    encoding: Encoding,
    label: &str,
) -> Result<(), String> {
    let workbench = world.workbench();
    let handle = workbench
        .cast_document::<TextBuffer>(document)
        .map_err(|e| e.to_string())?;
    let text = workbench
        .read_document(handle)
        .map_err(|e| e.to_string())?
        .clone();
    let revision = workbench
        .inspect_document(document)
        .map_err(|e| e.to_string())?
        .revision;
    let edits = byte_edits(&text, edits, encoding)?
        .into_iter()
        .map(|(range, replacement)| BufferEdit::new(range, replacement));
    world
        .workbench_mut()
        .edit_document_transaction(handle, revision, edits, EditOptions::new(label))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn edit_file(
    path: &Path,
    edits: &[(super::ProtocolRange, String)],
    encoding: Encoding,
) -> Result<(), String> {
    let source = bedtime_filesystem::Filesystem::local().source(path);
    let read = source.read(256 * 1024 * 1024).map_err(|e| e.to_string())?;
    let original = std::str::from_utf8(&read.bytes).map_err(|_| "It isn't UTF-8 text")?;
    let text = TextBuffer::from(original);
    let mut edited = original.to_owned();
    for (range, replacement) in byte_edits(&text, edits, encoding)? {
        edited.replace_range(range, &replacement);
    }
    source
        .prepare_write(&mut std::io::Cursor::new(edited.into_bytes()))
        .and_then(|pending| {
            pending.commit(bedtime_filesystem::WriteExpectation::Contents(
                &mut std::io::Cursor::new(read.bytes),
            ))
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The code action menu and the rename box, drawn over `panel`'s editor.
pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, panel: Entity, output: &EditorOutput) {
    let popup = {
        let asks = world.resource::<Asks>();
        asks.menu.as_ref().is_some_and(|m| m.panel == panel)
            || asks.rename.as_ref().is_some_and(|r| r.panel == panel)
    };
    if let Some(mut editor) = world.get_mut::<EditorPanel>(panel) {
        editor.popup = popup;
    }
    if !popup {
        return;
    }
    // The popup has the keys: the editor doesn't.
    if let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document()) {
        let id = editor_id(panel, document);
        if ui.memory(|memory| memory.has_focus(id)) {
            ui.memory_mut(|memory| memory.surrender_focus(id));
        }
    }
    let at = output.caret_rect.left_bottom();
    draw_menu(ui, world, panel, at);
    draw_rename(ui, world, panel, at);
}

fn draw_menu(ui: &mut egui::Ui, world: &mut World, panel: Entity, at: egui::Pos2) {
    let mut asks = world.resource_mut::<Asks>();
    let Some(menu) = asks.menu.as_mut().filter(|m| m.panel == panel) else {
        return;
    };
    let count = menu.actions.len();
    let (down, up, enter, escape) = ui.input_mut(|input| {
        (
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if down {
        menu.selected = (menu.selected + 1) % count;
    }
    if up {
        menu.selected = (menu.selected + count - 1) % count;
    }
    let mut chosen = enter.then_some(menu.selected);
    // Drawn as egui draws menus (style, frame, layout, opening), so it looks
    // like the right-click menu; the keyboard's row looks hovered.
    let popup = egui::Popup::new(
        egui::Id::new((panel, "code-actions")),
        ui.ctx().clone(),
        at,
        ui.layer_id(),
    )
    .kind(egui::PopupKind::Menu)
    .layout(egui::Layout::top_down_justified(egui::Align::Min))
    .style(egui::containers::menu::menu_style)
    .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
    .show(|ui| {
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for (index, action) in menu.actions.iter().enumerate() {
                    let selected = index == menu.selected;
                    let mut button = egui::Button::new(&action.title);
                    if selected {
                        button = button.fill(ui.visuals().widgets.hovered.weak_bg_fill);
                    }
                    let response = ui.add_enabled(action.disabled.is_none(), button);
                    if selected && (down || up) {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        chosen = Some(index);
                    }
                    if let Some(reason) = &action.disabled {
                        response.on_disabled_hover_text(reason);
                    }
                }
            });
    });
    let popup_rect = popup.map(|shown| shown.response.rect);
    let clicked_outside = ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pointer| !popup_rect.is_some_and(|rect| rect.contains(pointer)))
    });
    if escape || clicked_outside || chosen.is_some() {
        let menu = asks.menu.take().unwrap();
        if let Some(index) = chosen {
            perform(world, panel, menu.actions[index].clone());
        }
    }
}

fn draw_rename(ui: &mut egui::Ui, world: &mut World, panel: Entity, at: egui::Pos2) {
    let mut asks = world.resource_mut::<Asks>();
    let Some(rename) = asks.rename.as_mut().filter(|r| r.panel == panel) else {
        return;
    };
    let id = egui::Id::new((panel, "rename"));
    let mut done = None;
    egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .constrain_to(ui.ctx().content_rect().shrink(8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut rename.name)
                        .id(id)
                        .hint_text("New name")
                        .desired_width(240.0),
                );
                if !rename.focused {
                    response.request_focus();
                    rename.focused = true;
                }
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                if escape {
                    done = Some(None);
                } else if response.lost_focus() {
                    done = Some(enter.then(|| (rename.byte, rename.name.trim().to_owned())));
                }
            });
        });
    if let Some(choice) = done {
        asks.rename = None;
        if let Some((byte, name)) = choice.filter(|(_, name)| !name.is_empty())
            && let Err(error) = rename_symbol(world, panel, byte, name)
        {
            tell(world, Notification::warning(format!("Rename: {error}")));
        }
    }
}

/// Where a symbol is used: files, then lines with their text. Clicking one
/// opens it there. Not saved with the layout.
#[derive(Component, Default)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct ReferencesPanel {
    of: String,
    files: Vec<(PathBuf, Vec<Reference>)>,
}

struct Reference {
    line: u32,
    column: u32,
    text: String,
}

/// Show `locations` in the current layout's References panel, made if needed.
fn show_references(world: &mut World, locations: Vec<Location>) {
    if locations.is_empty() {
        tell(world, Notification::info("No references found"));
        return;
    }
    let of = std::mem::take(&mut world.resource_mut::<Asks>().references_of);
    let mut files: BTreeMap<PathBuf, Vec<Reference>> = BTreeMap::new();
    for location in locations {
        files
            .entry(location.uri.path())
            .or_default()
            .push(Reference {
                line: location.range.start.line,
                column: location.range.start.character,
                text: String::new(),
            });
    }
    // Each line's text, from the open document or the file.
    for (path, references) in &mut files {
        let text = match document_for(world, path) {
            Some(document) => {
                let workbench = world.workbench();
                workbench
                    .cast_document::<TextBuffer>(document)
                    .and_then(|handle| workbench.read_document(handle).map(|t| t.to_string()))
                    .ok()
            }
            None => std::fs::read_to_string(path).ok(),
        };
        let lines: Vec<&str> = text
            .as_deref()
            .map_or(Vec::new(), |text| text.lines().collect());
        references.sort_by_key(|reference| (reference.line, reference.column));
        for reference in references {
            reference.text = lines
                .get(reference.line as usize)
                .map(|line| line.trim().chars().take(200).collect())
                .unwrap_or_default();
        }
    }
    let content = ReferencesPanel {
        of,
        files: files.into_iter().collect(),
    };
    let Some(layout) = world.layouts().active() else {
        return;
    };
    let existing = world
        .layouts()
        .panels(layout)
        .into_iter()
        .find(|&panel| world.get::<ReferencesPanel>(panel).is_some());
    let panel = match existing {
        Some(panel) => {
            world.entity_mut(panel).insert(content);
            panel
        }
        None => {
            let Ok(area) = world.layouts().destination_area(layout) else {
                return;
            };
            let panel = world.spawn((content, InArea(area))).id();
            world.flush();
            panel
        }
    };
    let _ = world.layouts_mut().focus_panel(panel);
}

impl Panel for ReferencesPanel {
    const LABEL: &'static str = "References";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(references) = world.get::<ReferencesPanel>(panel) else {
            return;
        };
        let root = world.get_resource::<Root>().map(|root| root.0.clone());
        let mut open = None;
        let count: usize = references.files.iter().map(|(_, refs)| refs.len()).sum();
        egui::ScrollArea::vertical()
            .id_salt((panel, "references"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::NONE.inner_margin(8).show(ui, |ui| {
                    ui.weak(match count {
                        1 => format!("1 reference to {}", references.of),
                        n => format!("{n} references to {}", references.of),
                    });
                    for (path, lines) in &references.files {
                        ui.add_space(context.metrics.spacing[0]);
                        let shown = root
                            .as_ref()
                            .and_then(|root| path.strip_prefix(root).ok())
                            .unwrap_or(path);
                        ui.strong(shown.display().to_string());
                        for reference in lines {
                            let text = egui::RichText::new(format!(
                                "{:>5}  {}",
                                reference.line + 1,
                                reference.text
                            ))
                            .monospace();
                            if ui.add(egui::Button::selectable(false, text)).clicked() {
                                open = Some((path.clone(), reference.line, reference.column));
                            }
                        }
                    }
                });
            });
        if let Some(open) = open {
            world.resource_mut::<Asks>().open.push(open);
        }
    }
}

/// Open clicked references (out of drawing: opening can close panels).
fn open_references(world: &mut World) {
    let open = std::mem::take(&mut world.resource_mut::<Asks>().open);
    for (path, line, column) in open {
        let options = PathOptions {
            line: Some((line, column)),
            ..default()
        };
        if let Err(error) = world.workbench_mut().open_path(&path, options) {
            tell(world, Notification::warning(error));
        }
    }
}
