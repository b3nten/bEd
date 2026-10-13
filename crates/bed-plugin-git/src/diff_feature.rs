//! A comparison is a normal editor presentation in the shared document world.
use crate::{
    GitState,
    conflicts::has_conflict_markers,
    controller::DiffKey,
    panels::{apply_diff_action, confirmation_text, show_error},
};
use bed_api::workbench::{Panel, PanelCreation, PanelResult, WorkbenchAction};
use bed_api::{
    documents::PanelBinding,
    features::{FeatureSystems, HostAction, HostEffects},
    workbench::{PanelClose, PanelTarget},
};
use bed_documents::{DocumentId, DocumentSession, session::ClosePolicy};
use bed_editing::text_commands::CursorReveal;
use bed_editor_widgets::{DiffActionKind, DiffHunkRange, DiffPresentation, DiffSnapshot};
use bed_editor_widgets::{
    EditorView, EditorViewOptions, extensions::EditorExtensions, source_intents::SourceRequests,
};
use bed_git::{ConflictChoice, Diff, DiffSide, Operation};
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io,
    path::{Component, Path},
    sync::Arc,
};

#[derive(Default)]
pub struct DiffStates(HashMap<Entity, DiffState>);

struct DiffState {
    key: DiffKey,
    session_id: u64,
    document: Option<DocumentId>,
    document_state: Value,
    editor: Option<EditorView>,
    snapshot: Option<String>,
    owns_snapshot: bool,
    baseline: Arc<[u8]>,
    baseline_comparison: Option<u64>,
    saved: Option<DiffSnapshot>,
    selected_hunk: usize,
    confirm: Option<Operation>,
    error: Option<String>,
}

#[derive(Resource)]
struct Close(bed_api::workbench::PanelLifecycleSystem);

#[derive(Resource)]
struct Actions(bed_api::workbench::PanelActionSystem);

pub(crate) fn install(app: &mut App) {
    app.world_mut().init_non_send::<DiffStates>();
    let close = app.world_mut().register_system(close_panel);
    let actions = app.world_mut().register_system(panel_action);
    app.insert_resource(Actions(actions));
    app.insert_resource(Close(close)).add_systems(
        PreUpdate,
        (prepare, publish_readiness)
            .chain()
            .after(crate::feature::prepare)
            .in_set(FeatureSystems::Prepare),
    );
}

pub(crate) fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Compare", "Inspect the selected repository file’s changes. Open a comparison from the Git panel’s staged or unstaged file actions."),
        ("Edit", "The working document shares edits with other views. Use diff controls to navigate changes and handle conflicts."),
    ]).shortcut_scope("bed.editor.text-panel").shortcut_actions(&["bed.navigation.move_left", "bed.navigation.move_right", "bed.navigation.move_up", "bed.navigation.move_down", "bed.navigation.select_left", "bed.navigation.select_right", "bed.navigation.select_up", "bed.navigation.select_down", "bed.view.zoom_in", "bed.view.zoom_out", "bed.view.reset_zoom", "bed.edit.find", "bed.edit.copy", "bed.edit.cut", "bed.edit.paste", "bed.edit.select_all", "bed.edit.undo", "bed.edit.redo"]));
    if world
        .get::<PanelBinding>(creation.entity)
        .is_some_and(|binding| binding.input.local_file().is_some())
    {
        return Err("Git comparisons require a repository path, not a local-file panel".into());
    }
    let Some(path) = creation.state["path"]
        .as_str()
        .filter(|path| !path.is_empty())
    else {
        if creation.state.is_null()
            || creation
                .state
                .as_object()
                .is_some_and(|state| state.is_empty())
        {
            return Ok(Box::new(Frontend(creation.entity)));
        }
        return Err("Git diff requires a repository-relative path".into());
    };
    if Path::new(path)
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("Git diff path must stay within the repository".into());
    }
    let key = (
        path.to_owned(),
        if creation.state["side"].as_str() == Some("staged") {
            DiffSide::Staged
        } else {
            DiffSide::Unstaged
        },
    );
    let session_id = world
        .get_non_send::<DocumentSession>()
        .ok_or("Git comparisons require an editor document session")?
        .session_id()
        .0;
    let mut controller = world.non_send_mut::<GitState>();
    controller.0.claim_session(session_id)?;
    controller.0.visible += 1;
    *controller.0.diff_users.entry(key.clone()).or_default() += 1;
    controller.0.request_diff(key.clone());
    world.non_send_mut::<DiffStates>().0.insert(
        creation.entity,
        DiffState {
            key,
            session_id,
            document: None,
            document_state: creation.state["editor"].clone(),
            editor: None,
            snapshot: None,
            owns_snapshot: false,
            baseline: Arc::from([]),
            baseline_comparison: None,
            saved: None,
            selected_hunk: 0,
            confirm: None,
            error: None,
        },
    );
    let close = world.resource::<Close>().0;
    let actions = world.resource::<Actions>().0;
    world
        .entity_mut(creation.entity)
        .insert((PanelClose(close), bed_api::workbench::PanelActions(actions)));
    Ok(Box::new(Frontend(creation.entity)))
}

fn prepare(world: &mut World) {
    let session_id = world
        .get_non_send::<DocumentSession>()
        .map(|session| session.session_id().0);
    let dead = world
        .non_send::<DiffStates>()
        .0
        .iter()
        .filter(|(entity, state)| {
            !world.entities().contains(**entity)
                || session_id.is_some_and(|session_id| session_id != state.session_id)
        })
        .map(|(entity, _)| *entity)
        .collect::<Vec<_>>();
    for entity in dead {
        close_panel(In(entity), world).expect("retiring a diff releases its view");
    }
    if !world.contains_non_send::<DocumentSession>() {
        return;
    }
    let mut system = SystemState::<(
        NonSendMut<DiffStates>,
        NonSendMut<GitState>,
        NonSendMut<DocumentSession>,
    )>::new(world);
    let updates = {
        let (mut states, controller, mut session) =
            system.get_mut(world).expect("Git resources are installed");
        let mut updates = Vec::new();
        for (entity, state) in &mut states.0 {
            let Some(diff) = controller.0.diffs.get(&state.key).cloned() else {
                continue;
            };
            match state.sync_editor(&mut session, &controller.0, &diff) {
                Ok(()) => state.error = None,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => state.error = Some(error.to_string()),
            }
            if let Some(editor) = &state.editor {
                updates.push((*entity, state.document, editor.id()));
            }
        }
        updates
    };
    for (entity, document, view) in updates {
        if let Some(mut binding) = world.get_mut::<PanelBinding>(entity) {
            binding.document = document;
            binding.input = document.into();
            binding.view = Some(view);
        }
    }
}

fn publish_readiness(world: &mut World) {
    let updates = {
        let states = world.non_send::<DiffStates>();
        let git = &world.non_send::<GitState>().0;
        states
            .0
            .iter()
            .map(|(entity, state)| {
                (
                    *entity,
                    match state.error.as_ref().or(git.error.as_ref()) {
                        Some(error) => bed_api::workbench::PanelReadiness::Failed(error.clone()),
                        None if git.diffs.contains_key(&state.key) && !git.busy() => {
                            bed_api::workbench::PanelReadiness::Ready
                        }
                        None => bed_api::workbench::PanelReadiness::Pending,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    for (entity, state) in updates {
        if let Ok(mut entity) = world.get_entity_mut(entity) {
            entity.insert(state);
        }
    }
}

fn panel_action(
    In(invocation): In<bed_api::workbench::PanelActionInvocation>,
    mut states: NonSendMut<DiffStates>,
    mut session: NonSendMut<DocumentSession>,
) -> bed_api::workbench::PanelActionResult {
    let Some(editor) = states
        .0
        .get_mut(&invocation.entity)
        .and_then(|state| state.editor.as_mut())
    else {
        return Ok(false);
    };
    match invocation.action {
        bed_api::workbench::PanelAction::Find => editor
            .open_find(&mut session)
            .map_err(|error| error.to_string())?,
        bed_api::workbench::PanelAction::GoToLine => editor.open_line_jump(),
        bed_api::workbench::PanelAction::SelectAll => session
            .with_commands(editor.id(), |commands| commands.select_all())
            .map_err(|error| error.to_string())?,
        bed_api::workbench::PanelAction::Undo if !editor.read_only() => session
            .with_commands(editor.id(), |commands| commands.undo())
            .map_err(|error| error.to_string())?,
        bed_api::workbench::PanelAction::Redo if !editor.read_only() => session
            .with_commands(editor.id(), |commands| commands.redo())
            .map_err(|error| error.to_string())?,
        _ => {}
    }
    Ok(true)
}

fn close_panel(In(entity): In<Entity>, world: &mut World) -> Result<(), String> {
    let mut system = SystemState::<(
        NonSendMut<DiffStates>,
        NonSendMut<GitState>,
        Option<NonSendMut<DocumentSession>>,
    )>::new(world);
    let (mut states, mut controller, session) =
        system.get_mut(world).expect("Git state is installed");
    let Some(mut state) = states.0.remove(&entity) else {
        return Ok(());
    };
    let Some(mut session) = session.filter(|session| session.session_id().0 == state.session_id)
    else {
        return Ok(());
    };
    if let Some(editor) = state.editor.take() {
        // A comparison owns its view. The editor transition owner decides when
        // a canonical document closes, including after the last view detaches.
        session.detach_view(editor.id());
    }
    if state.owns_snapshot
        && let Some(document) = state.document
    {
        session
            .close_document(document, ClosePolicy::Discard)
            .map_err(|error| error.to_string())?;
    }
    controller.0.visible = controller.0.visible.saturating_sub(1);
    if let Some(users) = controller.0.diff_users.get_mut(&state.key) {
        *users = users.saturating_sub(1);
    }
    if controller.0.diff_users.get(&state.key).copied() == Some(0) {
        controller.0.diff_users.remove(&state.key);
        controller.0.diffs.remove(&state.key);
        controller.0.comparisons.remove(&state.key);
    }
    Ok(())
}

impl DiffState {
    fn sync_editor(
        &mut self,
        session: &mut DocumentSession,
        controller: &crate::controller::GitController,
        diff: &Diff,
    ) -> io::Result<()> {
        if diff.binary {
            return Ok(());
        }
        let path = controller.absolute(&self.key.0);
        if self.document.is_some()
            && self.owns_snapshot
            && self.key.1 == DiffSide::Unstaged
            && self.snapshot.as_ref() != Some(&diff.snapshot)
        {
            match session.open_file(&path) {
                Ok(document) => {
                    self.document_state = self.saved_editor(session);
                    if let Some(editor) = self.editor.take() {
                        session.detach_view(editor.id());
                    }
                    session.close_document(self.document.unwrap(), ClosePolicy::Discard)?;
                    self.document = None;
                    self.owns_snapshot = false;
                    self.attach_editor(session, document)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(document) = self.document {
            if self.owns_snapshot && self.snapshot.as_ref() != Some(&diff.snapshot) {
                session.replace_snapshot_document(document, &diff.new_bytes)?;
            }
            self.snapshot = Some(diff.snapshot.clone());
            return Ok(());
        }
        let document = if self.key.1 == DiffSide::Staged {
            self.owns_snapshot = true;
            let language = session
                .document_for_path(&path)
                .and_then(|id| session.snapshot(id).ok())
                .map(|snapshot| snapshot.language_id)
                .unwrap_or_else(|| {
                    bed_editing::document_state::DocumentState::language_id_from_path(&self.key.0)
                });
            session.create_snapshot_document(&diff.new_bytes, &language)?
        } else {
            match session.open_file(&path) {
                Ok(document) => document,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.owns_snapshot = true;
                    session.create_snapshot_document(&diff.new_bytes, "")?
                }
                Err(error) => return Err(error),
            }
        };
        if let Err(error) = self.attach_editor(session, document) {
            if self.owns_snapshot {
                let _ = session.close_document(document, ClosePolicy::Discard);
            }
            return Err(error);
        }
        self.snapshot = Some(diff.snapshot.clone());
        Ok(())
    }

    fn attach_editor(
        &mut self,
        session: &mut DocumentSession,
        document: DocumentId,
    ) -> io::Result<()> {
        let editor = EditorView::new(session, document)?;
        restore_editor(session, editor.id(), &self.document_state)?;
        self.editor = Some(editor);
        self.document = Some(document);
        Ok(())
    }

    fn saved_editor(&self, session: &DocumentSession) -> Value {
        self.editor.as_ref().and_then(|editor| session.view_snapshot(editor.id()).ok()).map_or_else(|| self.document_state.clone(), |state| json!({"selections":state.selections.iter().map(|selection| [selection.head_row,selection.head_column,selection.anchor_row,selection.anchor_column]).collect::<Vec<_>>(),"primary":state.primary_index,"scroll":state.requested_scroll.unwrap_or(state.scroll_position)}))
    }

    fn navigate_hunk(
        &mut self,
        documents: &mut DocumentSession,
        diff: &Diff,
        next: bool,
    ) -> io::Result<()> {
        if diff.hunks.is_empty() {
            return Ok(());
        }
        let Some(editor) = self.editor.as_ref() else {
            return Ok(());
        };
        let view = editor.id();
        let caret_row = documents.view_snapshot(view)?.primary().head_row.max(0) as usize;
        self.selected_hunk = if next {
            diff.hunks
                .iter()
                .position(|hunk| hunk.new_start > caret_row)
                .unwrap_or(0)
        } else {
            diff.hunks
                .iter()
                .rposition(|hunk| hunk.new_start < caret_row)
                .unwrap_or(diff.hunks.len() - 1)
        };
        let row = diff.hunks[self.selected_hunk].new_start as i32;
        documents.with_commands(view, |commands| {
            commands.set_cursor(row, 0, false, CursorReveal::Center)
        })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "One diff frontend scope names its canonical state and owned intention outputs"
    )]
    fn draw(
        &mut self,
        entity: Entity,
        controller: &mut crate::controller::GitController,
        documents: &mut DocumentSession,
        source: &EditorExtensions,
        ui: &Ui,
        actions: &mut Vec<WorkbenchAction>,
        hover: &mut SourceRequests,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let diff = controller.diffs.get(&self.key).cloned();
        let Some(diff) = diff else {
            ui.dummy([0.0, ui.text_line_height()]);
            controller.request_diff(self.key.clone());
            show_error(ui, controller, requests);
            return Ok(());
        };
        {
            ui.text(if self.key.1 == DiffSide::Staged {
                "HEAD → Index"
            } else {
                "Index → Working file"
            });
            ui.same_line();
            if ui.small_button("Open file") {
                requests.push(HostAction::OpenFileIn {
                    path: controller.absolute(&self.key.0).to_string_lossy().into(),
                    viewer: None,
                    target: PanelTarget::LastFocused,
                });
            }
            ui.same_line();
            if ui.small_button("Refresh") {
                controller.request_diff(self.key.clone());
            }
            let conflicted = controller.status.as_ref().is_some_and(|status| {
                status
                    .entries
                    .iter()
                    .any(|entry| entry.path == self.key.0 && entry.conflicted)
            });
            let enabled = controller.writable() && !controller.mutation_pending();
            {
                let _disabled = ui.begin_disabled_with_cond(!enabled);
                if ui.button(if self.key.1 == DiffSide::Staged {
                    "Unstage file"
                } else if conflicted {
                    "Save and stage resolution"
                } else {
                    "Stage file"
                }) {
                    let markers = self
                        .document
                        .and_then(|document| documents.snapshot(document).ok())
                        .is_some_and(|snapshot| has_conflict_markers(&snapshot.bytes));
                    if conflicted && markers {
                        controller.error = Some(
                            "Remove all conflict markers before staging the resolution.".into(),
                        );
                    } else {
                        controller.mutate(
                            if self.key.1 == DiffSide::Staged {
                                Operation::Unstage {
                                    paths: vec![self.key.0.clone()],
                                }
                            } else {
                                Operation::Stage {
                                    paths: vec![self.key.0.clone()],
                                }
                            },
                            documents,
                            requests,
                        );
                    }
                }
                if self.key.1 == DiffSide::Unstaged && !conflicted && !diff.old_bytes.is_empty() {
                    ui.same_line();
                    if ui.button("Discard file…") {
                        self.confirm = Some(Operation::Discard {
                            path: self.key.0.clone(),
                            snapshot: diff.snapshot.clone(),
                        });
                    }
                }
                if conflicted && (diff.binary || !diff.can_apply_hunks) {
                    ui.text_wrapped("Choose a whole-file version, then stage the resolution. Text conflicts can also be edited manually.");
                    for (index, (label, choice)) in [
                        ("Use current file", ConflictChoice::Current),
                        ("Use incoming file", ConflictChoice::Incoming),
                        ("Delete file", ConflictChoice::Delete),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if index > 0 {
                            ui.same_line();
                        }
                        if ui.button(label) {
                            if self
                                .document
                                .and_then(|document| documents.snapshot(document).ok())
                                .is_some_and(|snapshot| snapshot.dirty)
                            {
                                controller.error = Some("Save or discard editor changes before choosing a whole-file version.".into());
                            } else if choice == ConflictChoice::Delete {
                                self.confirm = Some(Operation::Resolve {
                                    path: self.key.0.clone(),
                                    choice,
                                });
                            } else {
                                controller.mutate(
                                    Operation::Resolve {
                                        path: self.key.0.clone(),
                                        choice,
                                    },
                                    documents,
                                    requests,
                                );
                            }
                        }
                    }
                }
            }
            show_error(ui, controller, requests);
            if let Some(operation) = self.confirm.clone() {
                let (message, button) = confirmation_text(&operation);
                ui.text_wrapped(message);
                if ui.button(button) {
                    controller.mutate(operation, documents, requests);
                    self.confirm = None;
                }
                ui.same_line();
                if ui.button("Keep changes") {
                    self.confirm = None;
                }
            }
        }
        if diff.binary {
            ui.text_wrapped("Binary or non-text change. Use the whole-file actions above.");
            return Ok(());
        }
        if diff.hunks.is_empty() && diff.old_bytes == diff.new_bytes {
            ui.text_disabled("No textual changes. File mode or other metadata may have changed.");
        }
        if let Some(error) = &self.error {
            ui.text_wrapped(format!("Cannot open diff editor: {error}"));
            return Ok(());
        }
        if self.document.is_none() {
            ui.dummy([0.0, ui.text_line_height()]);
            return Ok(());
        }
        if ui.small_button("Previous change") {
            self.navigate_hunk(documents, &diff, false)?;
        }
        ui.same_line();
        if ui.small_button("Next change") {
            self.navigate_hunk(documents, &diff, true)?;
        }
        ui.same_line();
        ui.text_disabled(format!(
            "{} {}",
            diff.hunks.len(),
            if diff.hunks.len() == 1 {
                "change"
            } else {
                "changes"
            }
        ));
        ui.separator();
        let mut options = EditorViewOptions {
            extensions: source.clone(),
            read_only: self.owns_snapshot,
            ..Default::default()
        };
        let controller_state = &*controller;
        let diff_actions = if diff.can_apply_hunks && controller_state.writable() {
            if self.key.1 == DiffSide::Staged {
                vec![DiffActionKind::Unstage]
            } else {
                vec![DiffActionKind::Stage, DiffActionKind::Discard]
            }
        } else {
            vec![]
        };
        let comparison = controller_state
            .comparisons
            .get(&self.key)
            .copied()
            .unwrap_or_default();
        if self.baseline_comparison != Some(comparison) {
            self.baseline = Arc::from(diff.old_bytes.clone());
            self.baseline_comparison = Some(comparison);
            self.saved = Some(DiffSnapshot {
                current: Arc::from(diff.new_bytes.clone()),
                hunks: diff
                    .hunks
                    .iter()
                    .map(|hunk| DiffHunkRange {
                        old_range: hunk.old_start..hunk.old_start + hunk.old_count,
                        new_range: hunk.new_start..hunk.new_start + hunk.new_count,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            });
        }
        options.diff = Some(DiffPresentation {
            baseline: self.baseline.clone(),
            comparison,
            actions: diff_actions,
            actions_enabled: !controller_state.mutation_pending(),
            saved: self.saved.clone(),
        });
        let response = self
            .editor
            .as_mut()
            .expect("attached diff has a portable editor")
            .draw(ui, documents, &options)?;
        hover.observe(&response);
        actions.extend(
            bed_editor_widgets::source_intents::source_commands(&response)
                .into_iter()
                .map(|(command, arguments)| {
                    WorkbenchAction::Invoke(bed_api::workbench::CommandInvocation {
                        command: command.into(),
                        source: Some(entity),
                        area: 0,
                        arguments,
                    })
                }),
        );
        for action in response.diff_actions {
            if let Some(operation) = apply_diff_action(
                controller,
                &self.key,
                action,
                self.document,
                documents,
                requests,
            ) {
                self.confirm = Some(operation);
            }
        }
        Ok(())
    }
}

struct Frontend(Entity);

impl Panel for Frontend {
    fn title(&self, world: &World) -> String {
        world.non_send::<DiffStates>().0.get(&self.0).map_or_else(
            || "Git diff".into(),
            |state| {
                format!(
                    "{} · {}",
                    state.key.0,
                    if state.key.1 == DiffSide::Staged {
                        "Staged"
                    } else {
                        "Unstaged"
                    }
                )
            },
        )
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        actions: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        let mut requests = Vec::new();
        let mut system = SystemState::<(
            NonSendMut<DiffStates>,
            NonSendMut<GitState>,
            NonSendMut<DocumentSession>,
            NonSend<EditorExtensions>,
            NonSendMut<SourceRequests>,
        )>::new(world);
        {
            let (mut states, mut controller, mut session, source, mut hover) = system
                .get_mut(world)
                .expect("Git source resources are installed");
            if let Some(state) = states.0.get_mut(&self.0) {
                if let Err(error) = state.draw(
                    self.0,
                    &mut controller.0,
                    &mut session,
                    &source,
                    ui,
                    actions,
                    &mut hover,
                    &mut requests,
                ) {
                    requests.push(HostAction::Notify {
                        message: error.to_string(),
                    });
                }
            } else {
                ui.text_wrapped("Select a changed file in Git Changes to compare it.");
                if ui.button("Choose comparison…") {
                    requests.push(HostAction::ShowPanel {
                        panel_type: crate::PANEL_ID.into(),
                        document: None,
                        state: Value::Null,
                        action: None,
                    });
                }
            }
        }
        for request in requests {
            world
                .resource_mut::<HostEffects>()
                .push(Some(self.0), request);
        }
    }

    fn save(&self, world: &World) -> Value {
        let Some(state) = world.non_send::<DiffStates>().0.get(&self.0) else {
            return Value::Null;
        };
        let editor = if state.document.is_some() {
            state.saved_editor(world.non_send::<DocumentSession>())
        } else {
            state.document_state.clone()
        };
        json!({"path":state.key.0,"side":if state.key.1 == DiffSide::Staged { "staged" } else { "unstaged" },"editor":editor})
    }
}

fn restore_editor(
    session: &mut DocumentSession,
    view: bed_documents::ViewId,
    saved: &Value,
) -> io::Result<()> {
    if let Some(values) = saved["selections"].as_array() {
        let selections = values
            .iter()
            .filter_map(|value| {
                let value = value.as_array()?;
                Some(bed_editing::text_view_state::Selection {
                    head_row: value.first()?.as_i64()? as i32,
                    head_column: value.get(1)?.as_i64()? as i32,
                    anchor_row: value.get(2)?.as_i64()? as i32,
                    anchor_column: value.get(3)?.as_i64()? as i32,
                    preferred_column: 0,
                })
            })
            .collect();
        session.with_commands(view, |commands| {
            commands.set_selections(
                selections,
                saved["primary"].as_u64().unwrap_or(0) as usize,
                CursorReveal::Ensure,
            )
        })?;
    }
    if let Some(scroll) = saved["scroll"]
        .as_array()
        .filter(|values| values.len() == 2)
    {
        session.set_scroll(
            view,
            scroll[0].as_f64().unwrap_or(0.0) as f32,
            scroll[1].as_f64().unwrap_or(0.0) as f32,
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::workbench::{PanelAction, PanelActionInvocation, PanelActions};
    use bed_documents::ByteEdit;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-git-ecs-diff-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn app(directory: &Directory) -> App {
        let mut app = App::new();
        app.world_mut().insert_non_send(DocumentSession::new());
        app.add_plugins(crate::GitPlugin);
        let session = app.world().non_send::<DocumentSession>().session_id();
        app.world_mut().insert_resource(crate::GitTarget {
            root: directory.0.to_string_lossy().into(),
            working_directory: directory.0.to_string_lossy().into(),
            session_id: Some(session),
        });
        app.world_mut().non_send_mut::<GitState>().0.root = directory.0.to_string_lossy().into();
        app
    }

    fn create(app: &mut App, state: Value) -> (Entity, Box<dyn Panel>) {
        let entity = app.world_mut().spawn(PanelBinding::default()).id();
        let factory = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .panel(crate::DIFF_PANEL_ID)
            .unwrap()
            .factory;
        let panel = app
            .world_mut()
            .run_system_with(factory, PanelCreation { entity, state })
            .unwrap()
            .unwrap();
        (entity, panel)
    }

    fn diff(app: &mut App, entity: Entity, snapshot: &str, bytes: &[u8]) {
        let key = app.world().non_send::<DiffStates>().0[&entity].key.clone();
        let diff = Diff {
            path: key.0.clone(),
            old_path: None,
            side: key.1,
            snapshot: snapshot.into(),
            old_bytes: b"baseline\n".to_vec(),
            new_bytes: bytes.to_vec(),
            hunks: vec![],
            binary: false,
            can_apply_hunks: true,
        };
        let mut state = app.world_mut().non_send_mut::<GitState>();
        let controller = &mut state.0;
        let comparison = controller.comparisons.get(&key).copied().unwrap_or(0) + 1;
        controller.comparisons.insert(key.clone(), comparison);
        controller.diffs.insert(key, Arc::new(diff));
        prepare(app.world_mut());
    }

    fn close(app: &mut App, entity: Entity) {
        let system = app.world().get::<PanelClose>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(system, entity)
            .unwrap()
            .unwrap();
    }

    #[test]
    fn registered_factory_validates_paths_before_registering_a_comparison_owner() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let factory = app
            .world()
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .panel(crate::DIFF_PANEL_ID)
            .unwrap()
            .factory;
        for path in ["../outside.rs", "/absolute.rs"] {
            let entity = app.world_mut().spawn_empty().id();
            assert!(
                app.world_mut()
                    .run_system_with(
                        factory,
                        PanelCreation {
                            entity,
                            state: json!({"path":path})
                        }
                    )
                    .unwrap()
                    .is_err()
            );
        }
        let (empty, panel) = create(&mut app, Value::Null);
        assert_eq!(panel.title(app.world()), "Git diff");
        assert!(panel.save(app.world()).is_null());
        assert!(!app.world().non_send::<DiffStates>().0.contains_key(&empty));
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.visible, 0);
        assert!(controller.diff_users.is_empty());
    }

    #[test]
    fn staged_snapshots_are_read_only_before_ui_and_refresh_keeps_the_same_managed_view() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (entity, panel) = create(
            &mut app,
            json!({"path":"a.rs","side":"staged","editor":{"selections":[[1,1,1,1]],"primary":0,"scroll":[0.0,9.0]}}),
        );
        diff(&mut app, entity, "index-1", b"one\ntwo\n");
        let binding = app.world().get::<PanelBinding>(entity).unwrap().clone();
        let document = binding.document.unwrap();
        let view = binding.view.unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .is_snapshot_document(document)
        );
        assert!(
            app.world().non_send::<DiffStates>().0[&entity]
                .editor
                .as_ref()
                .unwrap()
                .read_only()
        );
        let action = app.world().get::<PanelActions>(entity).unwrap().0;
        for kind in [PanelAction::Undo, PanelAction::Redo] {
            assert!(
                app.world_mut()
                    .run_system_with(
                        action,
                        PanelActionInvocation {
                            entity,
                            action: kind
                        }
                    )
                    .unwrap()
                    .unwrap()
            );
        }
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        assert!(
            app.world_mut()
                .non_send_mut::<DocumentSession>()
                .apply_edits(
                    document,
                    revision,
                    &[ByteEdit {
                        range: 0..0,
                        bytes: b"forbidden".to_vec()
                    }]
                )
                .is_err()
        );
        diff(&mut app, entity, "index-2", b"three\nfour\n");
        assert_eq!(
            app.world().get::<PanelBinding>(entity).unwrap().view,
            Some(view)
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            b"three\nfour\n"
        );
        assert_eq!(
            panel.save(app.world())["editor"]["selections"],
            json!([[1, 1, 1, 1]])
        );
        close(&mut app, entity);
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .is_err()
        );
        assert!(!app.world().non_send::<DiffStates>().0.contains_key(&entity));
        assert_eq!(app.world().non_send::<GitState>().0.visible, 0);
    }

    #[test]
    fn deleted_file_reconnects_to_the_canonical_document_without_losing_selection_or_scroll() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (entity, panel) = create(
            &mut app,
            json!({"path":"a.rs","editor":{"selections":[[1,2,1,2]],"primary":0,"scroll":[0.0,12.0]}}),
        );
        diff(&mut app, entity, "deleted", b"one\ntwo\n");
        let old = app.world().get::<PanelBinding>(entity).unwrap().clone();
        let saved = panel.save(app.world())["editor"].clone();
        fs::write(directory.0.join("a.rs"), b"one\ntwo\n").unwrap();
        let shared = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .open_file(&directory.0.join("a.rs"))
            .unwrap();
        diff(&mut app, entity, "restored", b"one\ntwo\n");
        let current = app.world().get::<PanelBinding>(entity).unwrap();
        assert_eq!(current.document, Some(shared));
        assert_ne!(current.view, old.view);
        assert_eq!(panel.save(app.world())["editor"], saved);
        assert!(
            !app.world().non_send::<DiffStates>().0[&entity]
                .editor
                .as_ref()
                .unwrap()
                .read_only()
        );
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(old.document.unwrap())
                .is_err()
        );
        let view = current.view.unwrap();
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .with_commands(view, |commands| commands.type_text(b" dirty"))
            .unwrap();
        let edited = app
            .world()
            .non_send::<DocumentSession>()
            .snapshot(shared)
            .unwrap();
        assert!(edited.dirty);
        close(&mut app, entity);
        assert_eq!(
            app.world().non_send::<DocumentSession>().view_count(shared),
            0
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(shared)
                .unwrap()
                .bytes,
            edited.bytes
        );
        let view = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .create_view(shared)
            .unwrap();
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(shared)
                .unwrap()
                .bytes,
            b"one\ntwo\n"
        );
    }

    #[test]
    fn closing_or_despawning_one_diff_preserves_other_owners_and_retires_the_last_snapshot() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (first, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        let (second, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, first, "saved", b"staged\n");
        let first_doc = app
            .world()
            .get::<PanelBinding>(first)
            .unwrap()
            .document
            .unwrap();
        let second_doc = app
            .world()
            .get::<PanelBinding>(second)
            .unwrap()
            .document
            .unwrap();
        let key = ("a.rs".into(), DiffSide::Staged);
        close(&mut app, first);
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(first_doc)
                .is_err()
        );
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(second_doc)
                .is_ok()
        );
        assert_eq!(app.world().non_send::<GitState>().0.diff_users[&key], 1);
        assert!(
            app.world()
                .non_send::<GitState>()
                .0
                .comparisons
                .contains_key(&key)
        );
        app.world_mut().despawn(second);
        prepare(app.world_mut());
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(second_doc)
                .is_err()
        );
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.visible, 0);
        assert!(!controller.diff_users.contains_key(&key));
        assert!(!controller.diffs.contains_key(&key));
        assert!(!controller.comparisons.contains_key(&key));
    }

    #[test]
    fn workspace_restoration_preserves_owned_comparisons_when_existing_panels_are_retained() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (entity, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, entity, "saved", b"staged content\n");
        let restore = app
            .world_mut()
            .query::<&bed_api::features::FeatureStateContribution>()
            .iter(app.world())
            .find(|contribution| contribution.owner == crate::MODULE_ID)
            .unwrap()
            .restore;
        app.world_mut()
            .run_system_with(
                restore,
                bed_api::features::FeatureStateRestore {
                    state: Some(json!({"commit_message":"restored draft"})),
                },
            )
            .unwrap()
            .unwrap();
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.draft, "restored draft");
        assert_eq!(
            controller.visible, 1,
            "restoring preferences cannot remove a live presentation owner"
        );
        assert_eq!(
            controller
                .diff_users
                .get(&("a.rs".into(), DiffSide::Staged)),
            Some(&1)
        );
        close(&mut app, entity);
        assert_eq!(app.world().non_send::<GitState>().0.visible, 0);
    }

    #[test]
    fn restoring_a_replaced_workspace_clears_old_owner_facts_before_registering_new_comparisons() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (old, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, old, "old", b"old workspace\n");
        app.world_mut().insert_non_send(DocumentSession::new());
        close(&mut app, old);
        let session = app.world().non_send::<DocumentSession>().session_id();
        app.world_mut()
            .resource_mut::<crate::GitTarget>()
            .session_id = Some(session);
        let restore = app
            .world_mut()
            .query::<&bed_api::features::FeatureStateContribution>()
            .iter(app.world())
            .find(|contribution| contribution.owner == crate::MODULE_ID)
            .unwrap()
            .restore;
        app.world_mut()
            .run_system_with(
                restore,
                bed_api::features::FeatureStateRestore { state: None },
            )
            .unwrap()
            .unwrap();
        let (current, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, current, "current", b"current workspace\n");
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.visible, 1);
        assert_eq!(controller.diff_users[&("a.rs".into(), DiffSide::Staged)], 1);
        close(&mut app, current);
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.visible, 0);
        assert!(controller.diff_users.is_empty());
    }

    #[test]
    fn retiring_an_old_workspace_does_not_release_the_new_workspace_comparison_or_view() {
        let directory = Directory::new();
        let mut app = app(&directory);
        let (old, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, old, "old", b"old workspace\n");
        app.world_mut().insert_non_send(DocumentSession::new());
        app.world_mut().non_send_mut::<GitState>().0 = crate::controller::GitController::default();
        app.world_mut().non_send_mut::<GitState>().0.root = directory.0.to_string_lossy().into();
        let (current, _) = create(&mut app, json!({"path":"a.rs","side":"staged"}));
        diff(&mut app, current, "current", b"current workspace\n");
        let binding = app.world().get::<PanelBinding>(current).unwrap().clone();
        // Preparation retires the old owner even if its presentation entity has
        // not yet been despawned by the workspace publisher.
        assert!(!app.world().non_send::<DiffStates>().0.contains_key(&old));
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .view_count(binding.document.unwrap()),
            1
        );
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(binding.document.unwrap())
                .unwrap()
                .bytes,
            b"current workspace\n"
        );
        let controller = &app.world().non_send::<GitState>().0;
        assert_eq!(controller.visible, 1);
        assert_eq!(controller.diff_users[&("a.rs".into(), DiffSide::Staged)], 1);
        assert!(
            controller
                .diffs
                .contains_key(&("a.rs".into(), DiffSide::Staged))
        );
    }

    #[test]
    fn discard_confirmation_requires_the_rendered_comparison_and_a_clean_canonical_document() {
        let directory = Directory::new();
        fs::write(directory.0.join("a.rs"), b"working\n").unwrap();
        let mut app = app(&directory);
        let (entity, _) = create(&mut app, json!({"path":"a.rs"}));
        diff(&mut app, entity, "saved-working", b"working\n");
        let document = app
            .world()
            .get::<PanelBinding>(entity)
            .unwrap()
            .document
            .unwrap();
        let key = ("a.rs".into(), DiffSide::Unstaged);
        let action = bed_editor_widgets::DiffAction {
            kind: DiffActionKind::Discard,
            comparison: 1,
            old_range: 0..1,
            new_range: 0..1,
            revision: app
                .world()
                .non_send::<DocumentSession>()
                .document_revision(document)
                .unwrap(),
        };

        fn apply(
            world: &mut World,
            key: &DiffKey,
            action: bed_editor_widgets::DiffAction,
            document: DocumentId,
            requests: &mut Vec<HostAction>,
        ) -> Option<Operation> {
            let mut state =
                SystemState::<(NonSendMut<GitState>, NonSend<DocumentSession>)>::new(world);
            let (mut git, session) = state.get_mut(world).unwrap();
            apply_diff_action(&mut git.0, key, action, Some(document), &session, requests)
        }
        let mut requests = Vec::new();
        let mut stale = action.clone();
        stale.comparison = 0;
        assert!(apply(app.world_mut(), &key, stale, document, &mut requests).is_none());
        assert!(
            app.world()
                .non_send::<GitState>()
                .0
                .error
                .as_ref()
                .unwrap()
                .contains("diff changed")
        );
        assert!(
            matches!(apply(app.world_mut(),&key,action.clone(),document,&mut requests),Some(Operation::DiscardRange {snapshot,..}) if snapshot=="saved-working")
        );
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .apply_edits(
                document,
                action.revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"unsaved\n".to_vec(),
                }],
            )
            .unwrap();
        assert!(apply(app.world_mut(), &key, action, document, &mut requests).is_none());
        assert!(
            app.world()
                .non_send::<GitState>()
                .0
                .error
                .as_ref()
                .unwrap()
                .contains("Save and refresh")
        );
        assert!(requests.is_empty());
        assert_eq!(fs::read(directory.0.join("a.rs")).unwrap(), b"working\n");
    }
}
