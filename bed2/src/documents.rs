//! Application-owned document viewers and a deliberately memory-only save source.
//! Workbench owns documents/history; panels own only presentation and edit-group tokens.
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct BasicModel {
    title: String,
    count: i32,
}

/// A versioned source, not a filesystem or persistence implementation. Its compare-
/// and-write happens under one lock so a stale save never overwrites an external edit.
#[derive(Clone)]
struct MemorySource<T>(Arc<Mutex<MemoryValue<T>>>);

struct MemoryValue<T> {
    value: T,
    version: u64,
    reject_saves: bool,
}

#[derive(Debug)]
enum MemoryWriteError {
    Conflict { version: u64 },
    Failed(String),
}

impl<T: Clone> MemorySource<T> {
    fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(MemoryValue {
            value,
            version: 1,
            reject_saves: false,
        })))
    }

    fn snapshot(&self) -> Result<(T, u64), String> {
        let source = self.0.lock().map_err(|_| "Memory source lock poisoned")?;
        Ok((source.value.clone(), source.version))
    }

    fn write(&self, expected: u64, value: T) -> Result<u64, MemoryWriteError> {
        let mut source = self
            .0
            .lock()
            .map_err(|_| MemoryWriteError::Failed("Memory source lock poisoned".into()))?;
        if source.reject_saves {
            return Err(MemoryWriteError::Failed(
                "The demo memory source is rejecting saves. Disable ‘Reject saves’ to retry."
                    .into(),
            ));
        }
        if source.version != expected {
            return Err(MemoryWriteError::Conflict {
                version: source.version,
            });
        }
        source.version = source
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryWriteError::Failed("Memory source version exhausted".into()))?;
        source.value = value;
        Ok(source.version)
    }

    fn external_update(&self, update: impl FnOnce(&mut T)) -> Result<(), String> {
        let mut source = self.0.lock().map_err(|_| "Memory source lock poisoned")?;
        let next = source
            .version
            .checked_add(1)
            .ok_or("Memory source version exhausted")?;
        update(&mut source.value);
        source.version = next;
        Ok(())
    }

    fn reject_saves(&self) -> Result<bool, String> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "Memory source lock poisoned")?
            .reject_saves)
    }

    fn set_reject_saves(&self, reject: bool) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "Memory source lock poisoned")?
            .reject_saves = reject;
        Ok(())
    }
}

impl DocumentModel for BasicModel {
    type Change = Self;

    fn apply(&mut self, change: &Self) -> Result<Self, String> {
        Ok(std::mem::replace(self, change.clone()))
    }
}

#[derive(Component, Clone)]
enum MemoryBinding {
    Text(MemorySource<String>),
    Model(MemorySource<BasicModel>),
}

fn memory_save_binding<M: DocumentModel>(source: MemorySource<M>) -> DocumentSaveBinding<M> {
    // Each source is a separate allocation owned by one workspace document. There
    // is no shared global destination key; the source itself enforces version CAS.
    DocumentSaveBinding::new(None, move |snapshot| {
        let expected = snapshot
            .expected_source_version
            .as_ref()
            .and_then(|version| version.0.parse::<u64>().ok())
            .ok_or_else(|| DocumentSaveError::Failed("Missing memory-source version".into()))?;
        // The callback runs on WorkspaceWork, never in the draw function.
        std::thread::sleep(Duration::from_millis(120));
        match source.write(expected, snapshot.model) {
            Ok(version) => Ok(Some(SourceVersion(version.to_string()))),
            Err(MemoryWriteError::Conflict { version, .. }) => Err(DocumentSaveError::Conflict(
                format!("Memory source is at version {version}; observe it before saving again"),
            )),
            Err(MemoryWriteError::Failed(error)) => Err(DocumentSaveError::Failed(error)),
        }
    })
}

fn open_memory_document<M: DocumentModel>(
    world: &mut World,
    workspace: Entity,
    key: &str,
    title: &str,
    initial: M,
    binding: fn(MemorySource<M>) -> MemoryBinding,
) -> Result<DocumentHandle<M>, String> {
    // Reopen is deliberately workspace-local. Do not replace either a live model
    // or its source/save binding when this button is used in another layout.
    for id in document_ids(world, workspace) {
        if inspect_document(world, id)
            .map_err(|e| e.to_string())?
            .source_key
            .as_deref()
            == Some(key)
        {
            return cast_document(world, id).map_err(|e| e.to_string());
        }
    }
    let source = MemorySource::new(initial.clone());
    let mut options = DocumentOptions::loaded(title);
    options.source_key = Some(key.into());
    options.initial_state = DocumentInitialState::Loaded(Some(SourceVersion("1".into())));
    let handle = open_document(world, workspace, initial, options).map_err(|e| e.to_string())?;
    bind_document_save(world, handle, memory_save_binding(source.clone()))
        .map_err(|e| e.to_string())?;
    world
        .entity_mut(handle.id().entity())
        .insert(binding(source));
    Ok(handle)
}

#[derive(Clone, Copy)]
enum DemoKind {
    Text,
    Model,
}

impl DemoKind {
    fn open(self, world: &mut World, workspace: Entity) -> Result<DocumentId, String> {
        match self {
            Self::Text => open_memory_document(
                world,
                workspace,
                "bed.demo.memory.text",
                "Memory text",
                String::from(
                    "Shared text document\n\nEdit this text, then duplicate its viewer.\n",
                ),
                MemoryBinding::Text,
            )
            .map(|handle| handle.id()),
            Self::Model => open_memory_document(
                world,
                workspace,
                "bed.demo.memory.model",
                "Memory model",
                BasicModel {
                    title: "Shared model".into(),
                    count: 1,
                },
                MemoryBinding::Model,
            )
            .map(|handle| handle.id()),
        }
    }

    fn of(world: &World, id: DocumentId) -> Option<Self> {
        if cast_document::<String>(world, id).is_ok() {
            Some(Self::Text)
        } else if cast_document::<BasicModel>(world, id).is_ok() {
            Some(Self::Model)
        } else {
            None
        }
    }

    fn bind(self, world: &mut World, panel: Entity, id: DocumentId) -> Result<(), String> {
        match self {
            Self::Text => {
                let handle = cast_document::<String>(world, id).map_err(|e| e.to_string())?;
                bind_document_view(world, panel, handle)
            }
            Self::Model => {
                let handle = cast_document::<BasicModel>(world, id).map_err(|e| e.to_string())?;
                bind_document_view(world, panel, handle)
            }
        }
        .map_err(|e| e.to_string())
    }
}

pub struct DocumentsPlugin;

impl Plugin for DocumentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppClose>()
            .register_panel_type::<DocumentsPanel>()
            .register_panel_type::<TextViewer>()
            .register_panel_type::<ModelViewer>()
            .add_systems(
                Update,
                (request_close, finish_hidden_groups, advance_close)
                    .chain()
                    .after(crate::files::FilesSystems::Update),
            );
        register_document_shortcuts(app.world_mut());
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_documents", inspect);
        }
    }
}

#[derive(Component, Default)]
struct DocumentsPanel {
    error: Option<String>,
}

impl Panel for DocumentsPanel {
    const KEY: &'static str = "bed.documents";
    const LABEL: &'static str = "Documents";

    fn initialize(entity: &mut EntityWorldMut<'_>) {
        entity.entry::<Self>().or_default();
    }

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
        let owner = world.get::<InWorkspace>(panel).unwrap().0;
        let result = Workbench::for_entity(world, panel).and_then(|wb| wb.panel_layout(panel));
        let layout = match result {
            Ok(layout) => layout,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        let ids = document_ids(world, owner);
        let mut action = None;
        context.frame(ui).scroll("documents", surface.rounding, |mut frame| {
            frame.padded(surface.padding, |frame| {
                let ui = frame.ui;
                ui.heading(Self::LABEL);
                ui.weak("Memory-only demonstration. Save writes to a versioned in-memory source, not a file. All data is lost when this application exits.");
                ui.label("Documents belong to this workspace and are shared across its layouts. Closing the last viewer keeps its document here.");
                ui.add_enabled_ui(!is_closing(world), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Create / reopen text").clicked() { action = Some((Some(DemoKind::Text), None, 1)); }
                        if ui.button("Create / reopen model").clicked() { action = Some((Some(DemoKind::Model), None, 1)); }
                    });
                    ui.weak("Reopen uses the same source key only within this workspace. Duplicate viewers have independent selection and scrolling; drag tabs to split the layout.");
                    ui.separator();
                    if ids.is_empty() { ui.weak("No live documents. Create one above."); }
                    for id in &ids {
                        let Ok(info) = inspect_document(world, *id) else { continue };
                        let viewers = world.query::<&DocumentView>().iter(world)
                            .filter(|view| view.document() == *id).count();
                        ui.push_id(id.entity().to_bits(), |ui| {
                            ui.strong(&info.title);
                            ui.label(format!("revision {} · {} · {viewers} viewer(s)", info.revision, document_status(&info)));
                            ui.horizontal_wrapped(|ui| {
                                let supported = DemoKind::of(world, *id).is_some();
                                if ui.add_enabled(supported, egui::Button::new("Open viewer")).clicked() {
                                    action = Some((None, Some(*id), 1));
                                }
                                if ui.add_enabled(supported, egui::Button::new("Open 2 viewers")).clicked() {
                                    action = Some((None, Some(*id), 2));
                                }
                            });
                            ui.separator();
                        });
                    }
                });
                if let Some(error) = &world.get::<Self>(panel).unwrap().error {
                    ui.colored_label(bedtime_ui2::theme::color(context.theme.palette.error), error);
                }
            });
        });
        if let Some((kind, id, count)) = action {
            let result = (|| {
                let id = match id {
                    Some(id) => id,
                    None => kind.unwrap().open(world, owner)?,
                };
                for _ in 0..count {
                    open_viewer(world, layout, id)?;
                }
                Ok::<_, String>(())
            })();
            world.get_mut::<Self>(panel).unwrap().error = result.err();
        }
    }
}

pub fn open_picker(world: &mut World, layout: Entity) {
    if is_closing(world) {
        return;
    }
    let result = (|| {
        let mut wb = Workbench::for_entity(world, layout)?;
        let existing = wb.panels(layout).into_iter().find(|panel| {
            wb.world()
                .get::<PanelInstance>(*panel)
                .is_some_and(|instance| instance.key == DocumentsPanel::KEY)
        });
        if let Some(panel) = existing {
            wb.focus_panel(panel)?;
        } else {
            wb.register_panel::<DocumentsPanel>(layout)?;
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = result {
        warn!("Cannot open Documents: {error}");
    }
}

fn open_viewer(world: &mut World, layout: Entity, id: DocumentId) -> Result<Entity, String> {
    let info = inspect_document(world, id).map_err(|e| e.to_string())?;
    let kind = DemoKind::of(world, id).ok_or("No demo viewer for this document model")?;
    let mut wb = Workbench::for_entity(world, layout)?;
    if wb.workspace_id().entity() != info.workspace {
        return Err("Viewer destination is in another workspace".into());
    }
    let panel = match kind {
        DemoKind::Text => wb.register_panel::<TextViewer>(layout)?,
        DemoKind::Model => wb.register_panel::<ModelViewer>(layout)?,
    };
    kind.bind(wb.world_mut(), panel, id)?;
    wb.world_mut()
        .get_mut::<PanelInstance>(panel)
        .unwrap()
        .title = info.title;
    Ok(panel)
}

#[derive(Component, Default)]
struct ViewerState {
    group: Option<DocumentUndoGroup>,
    editor: Option<egui::Id>,
    last_edit_at: f64,
    error: Option<String>,
}

struct TextViewer;
impl Panel for TextViewer {
    const KEY: &'static str = "bed.document-text";
    const LABEL: &'static str = "Text document";
    fn initialize(entity: &mut EntityWorldMut<'_>) {
        entity.entry::<ViewerState>().or_default();
    }
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw_viewer(ui, world, context, panel, DemoKind::Text);
    }
}

struct ModelViewer;
impl Panel for ModelViewer {
    const KEY: &'static str = "bed.document-model";
    const LABEL: &'static str = "Model document";
    fn initialize(entity: &mut EntityWorldMut<'_>) {
        entity.entry::<ViewerState>().or_default();
    }
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        draw_viewer(ui, world, context, panel, DemoKind::Model);
    }
}

fn document_status(info: &DocumentInfo) -> String {
    let mut status = vec![if info.dirty { "modified" } else { "saved" }];
    if info.saving {
        status.push("saving");
    }
    if info.conflict {
        status.push("conflict");
    }
    if info.requires_source_refresh {
        status.push("observe source required");
    }
    if info.has_drafts {
        status.push("uncommitted draft");
    }
    if info.last_save_error.is_some() {
        status.push("save failed");
    }
    status.join(" · ")
}

fn finish_group(world: &mut World, id: DocumentId, state: &mut ViewerState) {
    state.editor = None;
    if let Some(group) = state.group.take()
        && let Err(error) = end_document_undo_group(world, id, group)
        && error != DocumentError::StaleUndoGroup
    {
        state.error = Some(error.to_string());
    }
}

fn finish_hidden_groups(world: &mut World) {
    let hidden: Vec<_> = world
        .query::<(Entity, &DocumentView, &ViewerState, &PanelPresentation)>()
        .iter(world)
        .filter_map(|(panel, view, state, presentation)| {
            (state.group.is_some() && !presentation.visible).then_some((panel, view.document()))
        })
        .collect();
    for (panel, id) in hidden {
        let mut state = std::mem::take(&mut *world.get_mut::<ViewerState>(panel).unwrap());
        finish_group(world, id, &mut state);
        world.entity_mut(panel).insert(state);
    }
}

fn draw_viewer(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    kind: DemoKind,
) {
    let surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
    let mut state = std::mem::take(&mut *world.get_mut::<ViewerState>(panel).unwrap());
    context.frame(ui).scroll("document-viewer", surface.rounding, |mut frame| {
        frame.padded(surface.padding, |frame| {
            let ui = frame.ui;
            ui.add_enabled_ui(!is_closing(world), |ui| {
                let Some(id) = world.get::<DocumentView>(panel).map(DocumentView::document) else {
                    ui.heading("Unbound document viewer");
                    ui.weak("Layout restoration creates a fresh viewer, not a new document or a persisted binding.");
                    let owner = world.get::<InWorkspace>(panel).unwrap().0;
                    for id in document_ids(world, owner) {
                        let compatible = match kind {
                            DemoKind::Text => cast_document::<String>(world, id).is_ok(),
                            DemoKind::Model => cast_document::<BasicModel>(world, id).is_ok(),
                        };
                        if compatible && ui.button(inspect_document(world, id).unwrap().title).clicked() {
                            state.error = kind.bind(world, panel, id).err();
                        }
                    }
                    if ui.button("Create / reopen demo document").clicked() {
                        state.error = kind.open(world, owner).and_then(|id| kind.bind(world, panel, id)).err();
                    }
                    return;
                };
                if ui.input(|input| input.time) - state.last_edit_at > 0.8 { finish_group(world, id, &mut state); }
                let editor = ui.make_persistent_id(("canonical-editor", panel.to_bits()));
                // This runs before TextEdit sees any events. Its undoer is cleared
                // on both sides of drawing, while cursor/selection state is kept.
                for action in take_history_shortcuts(ui, &[editor]) {
                    finish_group(world, id, &mut state);
                    let result = match action {
                        HistoryAction::Undo => undo_document(world, id),
                        HistoryAction::Redo => redo_document(world, id),
                    };
                    state.error = result.err().map(|e| e.to_string());
                }
                let result = draw_document_toolbar(ui, world, panel, id, &mut state).and_then(|_| {
                    match kind {
                        DemoKind::Text => draw_text(ui, world, panel, id, editor, &mut state),
                        DemoKind::Model => draw_model(ui, world, panel, id, editor, &mut state),
                    }?;
                    draw_memory_source(ui, world, id, &mut state)?;
                    match kind {
                        DemoKind::Text => draw_conflict::<String>(ui, world, id, &mut state),
                        DemoKind::Model => draw_conflict::<BasicModel>(ui, world, id, &mut state),
                    }
                });
                if let Err(error) = result { state.error = Some(error); }
            });
            if let Some(error) = &state.error {
                ui.colored_label(bedtime_ui2::theme::color(context.theme.palette.error), error);
            }
        });
    });
    world.entity_mut(panel).insert(state);
}

fn draw_document_toolbar(
    ui: &mut egui::Ui,
    world: &mut World,
    panel: Entity,
    id: DocumentId,
    state: &mut ViewerState,
) -> Result<(), String> {
    let info = inspect_document(world, id).map_err(|e| e.to_string())?;
    ui.heading(&info.title);
    ui.label(format!(
        "revision {} · {}",
        info.revision,
        document_status(&info)
    ));
    if let Some(error) = &info.last_save_error {
        ui.label(format!("Save error: {error}"));
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                info.has_save_binding && !info.saving && !info.conflict && !info.has_drafts,
                egui::Button::new("Save"),
            )
            .clicked()
        {
            finish_group(world, id, state);
            state.error = save_document(world, id).err().map(|e| e.to_string());
        }
        if ui
            .add_enabled(info.can_undo, egui::Button::new("Undo"))
            .clicked()
        {
            finish_group(world, id, state);
            state.error = undo_document(world, id).err().map(|e| e.to_string());
        }
        if ui
            .add_enabled(info.can_redo, egui::Button::new("Redo"))
            .clicked()
        {
            finish_group(world, id, state);
            state.error = redo_document(world, id).err().map(|e| e.to_string());
        }
        if ui.button("Duplicate viewer").clicked() {
            finish_group(world, id, state);
            state.error = Workbench::for_entity(world, panel)
                .and_then(|wb| wb.panel_layout(panel))
                .and_then(|layout| open_viewer(world, layout, id))
                .err();
        }
    });
    ui.weak("Live shared edits · Ctrl/Cmd+Z, Shift+Z or Y · one document history");
    Ok(())
}

fn commit_widget_edit<M: DocumentModel>(
    ui: &egui::Ui,
    world: &mut World,
    panel: Entity,
    handle: DocumentHandle<M>,
    revision: u64,
    change: M::Change,
    editor: egui::Id,
    state: &mut ViewerState,
) -> Result<(), String> {
    if state.editor != Some(editor) {
        finish_group(world, handle.id(), state);
    }
    let group = match state.group {
        Some(group) => group,
        None => begin_document_undo_group(world, handle.id(), panel).map_err(|e| e.to_string())?,
    };
    state.group = Some(group);
    state.editor = Some(editor);
    let mut result = edit_document(
        world,
        handle,
        revision,
        change.clone(),
        EditOptions::grouped("Typing", group),
    );
    // A save, source observation or another viewer may have ended this group.
    // Only retry the group token; never retry a stale document revision.
    if result == Err(DocumentError::StaleUndoGroup) {
        let group =
            begin_document_undo_group(world, handle.id(), panel).map_err(|e| e.to_string())?;
        state.group = Some(group);
        result = edit_document(
            world,
            handle,
            revision,
            change,
            EditOptions::grouped("Typing", group),
        );
    }
    state.last_edit_at = ui.input(|input| input.time);
    match result {
        Ok(_) => {
            state.error = None;
            Ok(())
        }
        Err(error) => {
            finish_group(world, handle.id(), state);
            clear_widget_history(ui.ctx(), editor);
            ui.ctx().request_repaint();
            Err(format!(
                "Edit rejected; refreshed from the shared document: {error}"
            ))
        }
    }
}

fn draw_text(
    ui: &mut egui::Ui,
    world: &mut World,
    panel: Entity,
    id: DocumentId,
    editor: egui::Id,
    state: &mut ViewerState,
) -> Result<(), String> {
    let handle = cast_document::<String>(world, id).map_err(|e| e.to_string())?;
    let revision = inspect_document(world, id)
        .map_err(|e| e.to_string())?
        .revision;
    // This is a draw-local copy, never a second authoritative editing buffer.
    let previous = read_document(world, handle).map_err(|e| e.to_string())?;
    let mut text = previous.clone();
    clear_widget_history(ui.ctx(), editor);
    let mut output = egui::TextEdit::multiline(&mut text)
        .id(editor)
        .desired_width(f32::INFINITY)
        .desired_rows(9)
        .show(ui);
    output.state.clear_undoer();
    output.state.store(ui.ctx(), editor);
    if output.response.changed() {
        let change = text_change(previous, &text);
        commit_widget_edit(ui, world, panel, handle, revision, change, editor, state)?;
    }
    if !output.response.has_focus() {
        finish_group(world, id, state);
    }
    Ok(())
}

/// Keep ordinary typing transactions small while preserving UTF-8 boundaries.
fn text_change(previous: &str, next: &str) -> TextChange {
    let mut prefix = 0;
    for (before, after) in previous.chars().zip(next.chars()) {
        if before != after {
            break;
        }
        prefix += before.len_utf8();
    }
    let mut suffix = 0;
    for (before, after) in previous[prefix..]
        .chars()
        .rev()
        .zip(next[prefix..].chars().rev())
    {
        if before != after {
            break;
        }
        suffix += before.len_utf8();
    }
    TextChange {
        range: prefix..previous.len() - suffix,
        replacement: next[prefix..next.len() - suffix].into(),
    }
}

fn draw_model(
    ui: &mut egui::Ui,
    world: &mut World,
    panel: Entity,
    id: DocumentId,
    editor: egui::Id,
    state: &mut ViewerState,
) -> Result<(), String> {
    let handle = cast_document::<BasicModel>(world, id).map_err(|e| e.to_string())?;
    let revision = inspect_document(world, id)
        .map_err(|e| e.to_string())?
        .revision;
    let mut model = read_document(world, handle)
        .map_err(|e| e.to_string())?
        .clone();
    ui.label("Typed model: title + integer");
    clear_widget_history(ui.ctx(), editor);
    let mut output = egui::TextEdit::singleline(&mut model.title)
        .id(editor)
        .show(ui);
    output.state.clear_undoer();
    output.state.store(ui.ctx(), editor);
    let mut count_changed = false;
    ui.horizontal(|ui| {
        ui.label("Count");
        if ui.button("−1").clicked() {
            model.count = model.count.saturating_sub(1);
            count_changed = true;
        }
        ui.label(model.count.to_string());
        if ui.button("+1").clicked() {
            model.count = model.count.saturating_add(1);
            count_changed = true;
        }
    });
    if count_changed {
        finish_group(world, id, state);
        let mut options = EditOptions::new("Change count");
        options.viewer = Some(panel);
        edit_document(world, handle, revision, model, options).map_err(|e| e.to_string())?;
        state.error = None;
    } else if output.response.changed() {
        commit_widget_edit(ui, world, panel, handle, revision, model, editor, state)?;
    }
    if !output.response.has_focus() {
        finish_group(world, id, state);
    }
    Ok(())
}

fn observe_memory_source(
    world: &mut World,
    id: DocumentId,
    binding: &MemoryBinding,
) -> Result<(), String> {
    let generation = inspect_document(world, id)
        .map_err(|e| e.to_string())?
        .source_generation;
    match binding {
        MemoryBinding::Text(source) => {
            let (model, version) = source.snapshot()?;
            let handle = cast_document::<String>(world, id).map_err(|e| e.to_string())?;
            offer_document_external_update(
                world,
                handle,
                generation,
                model,
                Some(SourceVersion(version.to_string())),
            )
        }
        MemoryBinding::Model(source) => {
            let (model, version) = source.snapshot()?;
            let handle = cast_document::<BasicModel>(world, id).map_err(|e| e.to_string())?;
            offer_document_external_update(
                world,
                handle,
                generation,
                model,
                Some(SourceVersion(version.to_string())),
            )
        }
    }
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn draw_memory_source(
    ui: &mut egui::Ui,
    world: &mut World,
    id: DocumentId,
    state: &mut ViewerState,
) -> Result<(), String> {
    let Some(binding) = world.get::<MemoryBinding>(id.entity()).cloned() else {
        return Ok(());
    };
    ui.separator();
    let (version, mut reject) = match &binding {
        MemoryBinding::Text(source) => (source.snapshot()?.1, source.reject_saves()?),
        MemoryBinding::Model(source) => (source.snapshot()?.1, source.reject_saves()?),
    };
    ui.strong(format!("Memory-only source · version {version}"));
    ui.weak("No filesystem writes. External changes are observed explicitly, never automatically merged.");
    if ui
        .checkbox(&mut reject, "Reject saves (simulate a backend failure)")
        .changed()
    {
        match &binding {
            MemoryBinding::Text(source) => source.set_reject_saves(reject)?,
            MemoryBinding::Model(source) => source.set_reject_saves(reject)?,
        }
    }
    let mut update = false;
    let mut observe = false;
    ui.horizontal_wrapped(|ui| {
        if ui.button("External update + observe").clicked() {
            update = true;
            observe = true;
        }
        if ui.button("Change source only").clicked() {
            update = true;
        }
        if ui.button("Observe source").clicked() {
            observe = true;
        }
    });
    if update {
        match &binding {
            MemoryBinding::Text(source) => {
                source.external_update(|text| text.push_str("External memory-source update.\n"))?
            }
            MemoryBinding::Model(source) => source.external_update(|model| {
                model.count = model.count.saturating_add(10);
                model.title.push_str(" (external)");
            })?,
        }
    }
    if observe {
        finish_group(world, id, state);
        state.error = observe_memory_source(world, id, &binding).err();
    }
    Ok(())
}

fn draw_conflict<M: DocumentModel + std::fmt::Debug>(
    ui: &mut egui::Ui,
    world: &mut World,
    id: DocumentId,
    state: &mut ViewerState,
) -> Result<(), String> {
    let handle = cast_document::<M>(world, id).map_err(|e| e.to_string())?;
    let Some(conflict) = document_conflict(world, handle).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    ui.separator();
    ui.strong("Source conflict — choose explicitly");
    egui::CollapsingHeader::new("Compare base / local / incoming").show(ui, |ui| {
        ui.label(format!("Base: {:?}", conflict.base));
        ui.label(format!("Local: {:?}", conflict.local));
        ui.label(format!("Incoming: {:?}", conflict.incoming));
    });
    ui.weak("Accept incoming changes the document (undoable). Keep local acknowledges the incoming source and leaves local edits ready to save.");
    let mut resolution = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Accept incoming").clicked() {
            resolution = Some(DocumentConflictResolution::AcceptIncoming);
        }
        if ui.button("Keep local").clicked() {
            resolution = Some(DocumentConflictResolution::KeepLocal);
        }
    });
    if let Some(resolution) = resolution {
        finish_group(world, id, state);
        state.error = resolve_document_conflict(
            world,
            handle,
            conflict.revision,
            conflict.source_generation,
            resolution,
        )
        .err()
        .map(|e| e.to_string());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryAction {
    Undo,
    Redo,
}

/// Called before drawing a focused editor. Consume only document-history keys,
/// leaving copy/paste, selection/navigation and unrelated modified shortcuts alone.
fn take_history_shortcuts(ui: &mut egui::Ui, editors: &[egui::Id]) -> Vec<HistoryAction> {
    if !ui.is_enabled()
        || ui.memory(|memory| {
            memory
                .top_modal_layer()
                .is_some_and(|layer| layer != ui.layer_id())
        })
        || !ui.memory(|memory| editors.iter().any(|id| memory.has_focus(*id)))
    {
        return Vec::new();
    }
    let mut actions = Vec::new();
    ui.input_mut(|input| {
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let command = modifiers.command || modifiers.ctrl;
            if !command || modifiers.alt {
                return true;
            }
            let action = match key {
                egui::Key::Z if modifiers.shift => HistoryAction::Redo,
                egui::Key::Z => HistoryAction::Undo,
                egui::Key::Y if !modifiers.shift => HistoryAction::Redo,
                _ => return true,
            };
            actions.push(action);
            false
        });
    });
    actions
}

fn clear_widget_history(ctx: &egui::Context, editor: egui::Id) {
    if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, editor) {
        state.clear_undoer();
        state.store(ctx, editor);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CloseSnapshot {
    workspaces: Vec<Entity>,
    revisions: Vec<(DocumentId, u64)>,
}

#[derive(Clone, Default)]
enum ClosePhase {
    #[default]
    Idle,
    Prompt,
    Saving {
        snapshot: CloseSnapshot,
        awaited: Vec<DocumentId>,
    },
    ShuttingDown,
    ExitSent,
}

#[derive(Resource, Default)]
struct AppClose {
    phase: ClosePhase,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum CloseChoice {
    SaveAll,
    Discard,
    Cancel,
}

fn is_closing(world: &World) -> bool {
    world
        .get_resource::<AppClose>()
        .is_some_and(|close| !matches!(close.phase, ClosePhase::Idle))
}

fn request_close(mut requests: MessageReader<CloseRequested>, mut close: ResMut<AppClose>) {
    if requests.read().next().is_some() && matches!(close.phase, ClosePhase::Idle) {
        close.phase = ClosePhase::Prompt;
        close.error = None;
    }
}

fn close_snapshot(world: &mut World) -> Result<CloseSnapshot, String> {
    let mut workspaces: Vec<_> = world
        .query_filtered::<Entity, With<Workspace>>()
        .iter(world)
        .collect();
    workspaces.sort();
    let mut revisions = Vec::new();
    for workspace in &workspaces {
        for id in document_ids(world, *workspace) {
            revisions.push((
                id,
                inspect_document(world, id)
                    .map_err(|e| e.to_string())?
                    .revision,
            ));
        }
    }
    revisions.sort_by_key(|entry| entry.0);
    Ok(CloseSnapshot {
        workspaces,
        revisions,
    })
}

fn close_failed(world: &mut World, error: String) {
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::Prompt;
    close.error = Some(error);
}

fn begin_save_all(world: &mut World) -> Result<(), String> {
    let snapshot = close_snapshot(world)?;
    let mut infos = Vec::new();
    // Validate the complete application decision before admitting any writes.
    // A failed write admission later may leave earlier saves running, never a
    // partly closed application or cancelled backend writes.
    for (id, _) in &snapshot.revisions {
        let info = inspect_document(world, *id).map_err(|e| e.to_string())?;
        if info.has_drafts {
            return Err(format!(
                "Commit or cancel drafts for {:?} first",
                info.title
            ));
        }
        if info.conflict || info.requires_source_refresh {
            return Err(format!(
                "Cancel quit and observe/resolve the source conflict for {:?} first",
                info.title
            ));
        }
        if info.dirty && !info.has_save_binding {
            return Err(format!(
                "Document {:?} has no save binding; cancel quit or explicitly discard",
                info.title
            ));
        }
        infos.push(info);
    }
    let mut awaited = Vec::new();
    for info in infos {
        if info.saving {
            awaited.push(info.id);
        } else if info.dirty {
            save_document(world, info.id)
                .map_err(|e| format!("Cannot save {:?}: {e}", info.title))?;
            awaited.push(info.id);
        }
    }
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::Saving { snapshot, awaited };
    close.error = None;
    Ok(())
}

fn begin_shutdown(world: &mut World, mode: WorkspaceCloseMode) -> Result<(), String> {
    let snapshot = close_snapshot(world)?;
    // No workspace is retired or worker cancelled until every workspace passes.
    for workspace in snapshot.workspaces {
        preflight_workspace_close(world, workspace, mode)?;
        if bedtime_filetree2::filetree_operation_count(world, workspace.entity()) != 0 {
            return Err("File operations are still running. Finish or cancel them in Files before quitting; completed changes must be reconciled before preferences are flushed.".into());
        }
    }
    // Admit application-owned file preferences before cancellation asks its
    // worker to flush the last coalesced write. This does no filesystem I/O.
    crate::files::admit_preferences(world);
    Workspaces::new(world).shutdown_work()?;
    let mut close = world.resource_mut::<AppClose>();
    close.phase = ClosePhase::ShuttingDown;
    close.error = None;
    Ok(())
}

fn apply_close_choice(world: &mut World, choice: CloseChoice) {
    if matches!(
        world.resource::<AppClose>().phase,
        ClosePhase::ShuttingDown | ClosePhase::ExitSent
    ) {
        return;
    }
    let result = match choice {
        CloseChoice::SaveAll => begin_save_all(world),
        CloseChoice::Discard => begin_shutdown(world, WorkspaceCloseMode::Discard),
        CloseChoice::Cancel => {
            // Admitted saves continue normally. Cancellation only abandons the
            // quit intention; live documents, viewers and workers are retained.
            let mut close = world.resource_mut::<AppClose>();
            close.phase = ClosePhase::Idle;
            close.error = None;
            Ok(())
        }
    };
    if let Err(error) = result {
        close_failed(world, error);
    }
}

fn advance_close(world: &mut World) {
    let phase = world.resource::<AppClose>().phase.clone();
    match phase {
        ClosePhase::Saving { snapshot, awaited } => {
            let result = (|| {
                if close_snapshot(world)? != snapshot {
                    return Err("Documents or workspaces changed while Save All was pending. Nothing was closed. Cancel quit to review, or choose Save All again.".into());
                }
                let infos: Vec<_> = snapshot
                    .revisions
                    .iter()
                    .map(|(id, _)| inspect_document(world, *id).map_err(|e| e.to_string()))
                    .collect::<Result<_, _>>()?;
                if infos.iter().any(|info| info.saving) {
                    return Ok(());
                }
                for info in &infos {
                    if awaited.contains(&info.id)
                        && let Some(error) = &info.last_save_error
                    {
                        return Err(format!(
                            "Save failed for {:?}: {error}. Nothing was closed; cancel quit to fix it or explicitly discard.",
                            info.title
                        ));
                    }
                }
                // The completed snapshots, not merely save admission, must pass
                // the same core gate that protects normal workspace closure.
                begin_shutdown(world, WorkspaceCloseMode::Saved)
            })();
            if let Err(error) = result {
                close_failed(world, error);
            }
        }
        ClosePhase::ShuttingDown => {
            if Workspaces::new(world).work_finished() {
                world.write_message(AppExit::Success);
                world.resource_mut::<AppClose>().phase = ClosePhase::ExitSent;
            }
        }
        _ => {}
    }
}

/// Drawn before the workbench panels from the app-owned titlebar callback. The
/// foreground modal blocks the entire one-window UI, including during final
/// worker shutdown; normal updates continue so processes and saves can finish.
pub fn draw_close_dialog(ui: &mut egui::Ui, world: &mut World) {
    let Some(close) = world.get_resource::<AppClose>() else {
        return;
    };
    if matches!(close.phase, ClosePhase::Idle) {
        return;
    }
    let phase = close.phase.clone();
    let error = close.error.clone();
    let mut choice = None;
    let response = egui::Modal::new(egui::Id::new("bed.application-close")).show(ui.ctx(), |ui| {
        ui.set_max_width(480.0);
        ui.heading("Quit bEd 2?");
        ui.label("This decision covers every workspace, including inactive workspaces and documents with no open viewers.");
        ui.weak("Demo saves are memory-only. No document is written to a filesystem, and all demo data disappears when this application exits.");
        if let Some(error) = &error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        match &phase {
            ClosePhase::Prompt => {
                if let Ok(snapshot) = close_snapshot(world) {
                    let dirty = snapshot.revisions.iter().filter(|(id, _)|
                        inspect_document(world, *id).is_ok_and(|info| info.dirty)).count();
                    ui.label(format!("{} workspace(s) · {} document(s) · {dirty} modified", snapshot.workspaces.len(), snapshot.revisions.len()));
                }
                ui.horizontal(|ui| {
                    if ui.button("Save All and quit").clicked() { choice = Some(CloseChoice::SaveAll); }
                    if ui.button("Discard and quit").clicked() { choice = Some(CloseChoice::Discard); }
                    if ui.button("Cancel").clicked() { choice = Some(CloseChoice::Cancel); }
                });
            }
            ClosePhase::Saving { .. } => {
                ui.spinner();
                ui.label("Waiting for admitted saves… Edits are paused. Save failures or changed documents return to this decision, never an automatic save loop.");
                if ui.button("Cancel quit (saves keep running)").clicked() { choice = Some(CloseChoice::Cancel); }
            }
            ClosePhase::ShuttingDown | ClosePhase::ExitSent => {
                ui.spinner();
                ui.label("Stopping workspace tasks and processes…");
            }
            ClosePhase::Idle => {}
        }
    });
    if !matches!(phase, ClosePhase::ShuttingDown | ClosePhase::ExitSent) && response.should_close()
    {
        choice = Some(CloseChoice::Cancel);
    }
    if let Some(choice) = choice {
        apply_close_choice(world, choice);
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> bedtime_automation2::AutomationResult {
    use serde_json::json;
    let ids: Vec<_> = world
        .query_filtered::<Entity, With<Document>>()
        .iter(world)
        .map(DocumentId)
        .collect();
    let mut documents = Vec::new();
    for id in ids {
        let info = inspect_document(world, id).map_err(|error| error.to_string())?;
        let (kind, content) = match DemoKind::of(world, id) {
            Some(DemoKind::Text) => {
                let handle =
                    cast_document::<String>(world, id).map_err(|error| error.to_string())?;
                (
                    "text",
                    json!(read_document(world, handle).map_err(|error| error.to_string())?),
                )
            }
            Some(DemoKind::Model) => {
                let handle =
                    cast_document::<BasicModel>(world, id).map_err(|error| error.to_string())?;
                let model = read_document(world, handle).map_err(|error| error.to_string())?;
                (
                    "model",
                    json!({ "title": model.title, "count": model.count }),
                )
            }
            None => ("other", serde_json::Value::Null),
        };
        let source = match world.get::<MemoryBinding>(id.entity()) {
            Some(MemoryBinding::Text(source)) => {
                let (content, version) = source.snapshot()?;
                json!({ "content": content, "version": version, "reject_saves": source.reject_saves()? })
            }
            Some(MemoryBinding::Model(source)) => {
                let (model, version) = source.snapshot()?;
                json!({ "content": { "title": model.title, "count": model.count }, "version": version, "reject_saves": source.reject_saves()? })
            }
            None => serde_json::Value::Null,
        };
        documents.push(json!({
            "id": id.entity().to_bits().to_string(), "workspace": info.workspace.to_bits().to_string(),
            "title": info.title, "kind": kind, "model_type": info.model_type_name, "content": content,
            "revision": info.revision, "source_generation": info.source_generation,
            "source_key": info.source_key, "source_version": info.source_version.map(|version| version.0),
            "dirty": info.dirty, "saving": info.saving, "conflict": info.conflict,
            "requires_source_refresh": info.requires_source_refresh, "has_drafts": info.has_drafts,
            "has_save_binding": info.has_save_binding, "can_undo": info.can_undo, "can_redo": info.can_redo,
            "last_save_error": info.last_save_error.map(|error| error.to_string()), "memory_source": source,
        }));
    }
    let viewers: Vec<_> = world.query::<(Entity, &DocumentView, Option<&ViewerState>, &InWorkspace)>().iter(world)
        .map(|(panel, view, state, workspace)| json!({
            "panel": panel.to_bits().to_string(), "document": view.document().entity().to_bits().to_string(),
            "workspace": workspace.0.to_bits().to_string(), "error": state.and_then(|state| state.error.as_deref()),
            "group_active": state.is_some_and(|state| state.group.is_some()),
        })).collect();
    let close = world.resource::<AppClose>();
    let quit_phase = match close.phase {
        ClosePhase::Idle => "idle",
        ClosePhase::Prompt => "prompt",
        ClosePhase::Saving { .. } => "saving",
        ClosePhase::ShuttingDown => "shutting_down",
        ClosePhase::ExitSent => "exit_sent",
    };
    Ok(
        json!({ "documents": documents, "viewers": viewers, "quit_phase": quit_phase, "quit_error": close.error }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn host() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, DocumentsPlugin))
            .add_message::<AppExit>();
        app
    }

    fn workspace(app: &mut App, name: &str) -> (Entity, Entity) {
        let workspace = Workspaces::new(app.world_mut()).open(name).entity();
        let layout = Workbench::for_entity(app.world_mut(), workspace)
            .unwrap()
            .active_layout();
        (workspace, layout)
    }

    fn change_text(world: &mut World, id: DocumentId, text: &str) {
        let handle = cast_document::<String>(world, id).unwrap();
        let revision = inspect_document(world, id).unwrap().revision;
        let len = read_document(world, handle).unwrap().len();
        edit_document(
            world,
            handle,
            revision,
            TextChange {
                range: 0..len,
                replacement: text.into(),
            },
            EditOptions::new("Test edit"),
        )
        .unwrap();
    }

    fn eventually(app: &mut App, condition: impl Fn(&World) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(app.world()) {
            assert!(Instant::now() < deadline, "Document operation timed out");
            app.update();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn plugin_initializes_no_workspace_or_document_and_viewers_reopen_workspace_sessions() {
        let mut app = host();
        assert!(Workspaces::new(app.world_mut()).list().is_empty());
        let (first, layout) = workspace(&mut app, "First");
        assert!(document_ids(app.world(), first).is_empty());
        let text = DemoKind::Text.open(app.world_mut(), first).unwrap();
        assert_eq!(DemoKind::Text.open(app.world_mut(), first).unwrap(), text);
        let left = open_viewer(app.world_mut(), layout, text).unwrap();
        let right = open_viewer(app.world_mut(), layout, text).unwrap();
        assert_ne!(left, right);
        assert_eq!(
            app.world().get::<DocumentView>(left).unwrap().document(),
            text
        );
        assert_eq!(
            app.world().get::<DocumentView>(right).unwrap().document(),
            text
        );
        assert!(
            app.world()
                .get::<ViewerState>(left)
                .unwrap()
                .group
                .is_none()
        );
        let (second, _) = workspace(&mut app, "Second");
        assert_ne!(DemoKind::Text.open(app.world_mut(), second).unwrap(), text);
        Workbench::for_entity(app.world_mut(), left)
            .unwrap()
            .close_panel(left)
            .unwrap();
        Workbench::for_entity(app.world_mut(), right)
            .unwrap()
            .close_panel(right)
            .unwrap();
        assert!(
            inspect_document(app.world(), text).is_ok(),
            "Closing the last viewer must retain its document"
        );
        assert_eq!(DemoKind::Text.open(app.world_mut(), first).unwrap(), text);
    }

    #[test]
    fn close_cancel_preserves_live_documents_and_never_emits_exit() {
        let mut app = host();
        let (owner, layout) = workspace(&mut app, "Project");
        let id = DemoKind::Model.open(app.world_mut(), owner).unwrap();
        let viewer = open_viewer(app.world_mut(), layout, id).unwrap();
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        apply_close_choice(app.world_mut(), CloseChoice::Cancel);
        app.update();
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Idle
        ));
        assert!(app.world().get::<DocumentView>(viewer).is_some());
        assert!(inspect_document(app.world(), id).is_ok());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn save_all_failure_leaves_the_app_editable_and_does_not_exit() {
        let mut app = host();
        let (owner, _) = workspace(&mut app, "Project");
        let id = DemoKind::Text.open(app.world_mut(), owner).unwrap();
        change_text(app.world_mut(), id, "Local edit");
        let MemoryBinding::Text(source) = app
            .world()
            .get::<MemoryBinding>(id.entity())
            .unwrap()
            .clone()
        else {
            unreachable!()
        };
        source.set_reject_saves(true).unwrap();
        apply_close_choice(app.world_mut(), CloseChoice::SaveAll);
        eventually(&mut app, |world| {
            matches!(world.resource::<AppClose>().phase, ClosePhase::Prompt)
        });
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("Save failed")
        );
        assert!(inspect_document(app.world(), id).unwrap().dirty);
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        apply_close_choice(app.world_mut(), CloseChoice::Cancel);
        change_text(app.world_mut(), id, "Still editable after failed quit");
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Idle
        ));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn discard_preflights_inactive_workspace_drafts_before_shutting_down_any_work() {
        let mut app = host();
        let (first, _) = workspace(&mut app, "Active");
        let (inactive, layout) = workspace(&mut app, "Inactive");
        let id = DemoKind::Text.open(app.world_mut(), inactive).unwrap();
        let viewer = open_viewer(app.world_mut(), layout, id).unwrap();
        app.world_mut().entity_mut(viewer).insert(DocumentDraft);
        apply_close_choice(app.world_mut(), CloseChoice::Discard);
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("draft")
        );
        assert!(app.world().get::<DocumentView>(viewer).is_some());
        assert!(inspect_document(app.world(), id).is_ok());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        let _task = app
            .world_mut()
            .get_mut::<WorkspaceWork>(first)
            .unwrap()
            .spawn_task(None, "Still accepts work after rejected close", |_| 1_u8)
            .unwrap();
    }

    #[test]
    fn successful_quit_waits_for_workspace_work_before_emitting_exit() {
        let mut app = host();
        let (owner, _) = workspace(&mut app, "Project");
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let _task = app
            .world_mut()
            .get_mut::<WorkspaceWork>(owner)
            .unwrap()
            .spawn_task(None, "Pending cleanup", move |_| {
                let _ = wait.recv_timeout(Duration::from_secs(3));
            })
            .unwrap();
        apply_close_choice(app.world_mut(), CloseChoice::Discard);
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::ShuttingDown
        ));
        advance_close(app.world_mut());
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        release.send(()).unwrap();
        eventually(&mut app, |world| {
            matches!(world.resource::<AppClose>().phase, ClosePhase::ExitSent)
        });
        assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn save_all_does_not_loop_when_a_document_changes_while_saving() {
        let mut app = host();
        let (owner, _) = workspace(&mut app, "Project");
        let id = DemoKind::Text.open(app.world_mut(), owner).unwrap();
        change_text(app.world_mut(), id, "Save this version");
        apply_close_choice(app.world_mut(), CloseChoice::SaveAll);
        change_text(app.world_mut(), id, "A newer version arrived");
        advance_close(app.world_mut());
        assert!(matches!(
            app.world().resource::<AppClose>().phase,
            ClosePhase::Prompt
        ));
        assert!(
            app.world()
                .resource::<AppClose>()
                .error
                .as_ref()
                .unwrap()
                .contains("changed")
        );
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
        eventually(&mut app, |world| {
            !inspect_document(world, id).unwrap().saving
        });
        assert!(inspect_document(app.world(), id).unwrap().dirty);
        let handle = cast_document::<String>(app.world(), id).unwrap();
        assert_eq!(
            read_document(app.world(), handle).unwrap(),
            "A newer version arrived"
        );
        assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    }

    #[test]
    fn editor_routes_only_history_keys_before_text_edit() {
        let ctx = egui::Context::default();
        let editor = egui::Id::new("test-editor");
        let key = |key, shift, alt| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                ctrl: true,
                shift,
                alt,
                ..Default::default()
            },
        };
        let raw = egui::RawInput {
            events: vec![
                key(egui::Key::Z, false, false),
                key(egui::Key::Z, true, false),
                key(egui::Key::Y, false, false),
                key(egui::Key::C, false, false),
                key(egui::Key::Z, false, true),
                key(egui::Key::Y, true, false),
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            ui.memory_mut(|memory| memory.request_focus(editor));
            assert_eq!(
                take_history_shortcuts(ui, &[editor]),
                vec![
                    HistoryAction::Undo,
                    HistoryAction::Redo,
                    HistoryAction::Redo
                ]
            );
            assert_eq!(
                ui.input(|input| input
                    .events
                    .iter()
                    .filter(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
                    .count()),
                3
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn widget_diff_and_inverse_preserve_utf8_and_insertions() {
        for (before, after) in [
            ("aé🦀z", "a界🦀z"),
            ("text", "text appended"),
            ("prefix text suffix", "prefix suffix"),
            ("", "🦀"),
            ("same", "same"),
        ] {
            let mut model = before.to_owned();
            let inverse = DocumentModel::apply(&mut model, &text_change(before, after)).unwrap();
            assert_eq!(model, after);
            DocumentModel::apply(&mut model, &inverse).unwrap();
            assert_eq!(model, before);
        }
    }

    #[test]
    fn memory_source_compare_and_write_preserves_external_changes() {
        let source = MemorySource::new(String::from("initial"));
        assert_eq!(source.write(1, "saved".into()).unwrap(), 2);
        source
            .external_update(|text| text.push_str(" externally"))
            .unwrap();
        match source.write(2, "stale save".into()) {
            Err(MemoryWriteError::Conflict { version }) => {
                assert_eq!(version, 3);
            }
            other => panic!("Expected a source conflict, got {other:?}"),
        }
        assert_eq!(source.snapshot().unwrap(), ("saved externally".into(), 3));
    }

    #[test]
    fn failed_memory_save_is_retryable_and_does_not_mutate_the_source() {
        let source = MemorySource::new(BasicModel {
            title: "Original".into(),
            count: 1,
        });
        source.set_reject_saves(true).unwrap();
        let changed = BasicModel {
            title: "Changed".into(),
            count: 2,
        };
        assert!(matches!(
            source.write(1, changed.clone()),
            Err(MemoryWriteError::Failed(_))
        ));
        assert_eq!(source.snapshot().unwrap().1, 1);
        assert_eq!(source.snapshot().unwrap().0.title, "Original");
        source.set_reject_saves(false).unwrap();
        assert_eq!(source.write(1, changed.clone()).unwrap(), 2);
        assert_eq!(source.snapshot().unwrap().0, changed);
    }
}
