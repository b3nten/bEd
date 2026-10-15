//! Document requests resolve between UI scopes, with retained operation receipts.
use crate::models::{self, ModelRegistry};
pub use bedtime_api::features::HostRequest;
use bedtime_api::{
    documents::DocumentFrame,
    documents::{PanelBinding, PanelInput, Revision},
    features::{HostAction, HostEffects},
};
use bedtime_documents::{ClosePolicy, DocumentId, DocumentSession};
use bedtime_workbench::{WorkbenchLayout, WorkbenchRequestMode, WorkbenchRequests};
use bevy::{ecs::message::MessageCursor, prelude::*};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
};

/// An application may retain a document selection while a tool panel has focus.
/// Without this input the frame keeps the shared workbench selection when a tool
/// is focused. An explicit empty selection clears that remembered document.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct DocumentFrameSelection {
    pub document: Option<DocumentId>,
    pub view: Option<bedtime_documents::ViewId>,
}

/// The shared editing selection survives focusing a tool panel. Layout
/// integrations can capture this small value alongside retained panels.
#[derive(Resource, Default, Clone, Debug)]
pub struct DocumentNavigation {
    pub active_view: Option<bedtime_documents::ViewId>,
    pub active_document: Option<DocumentId>,
    pub last_document_panel: Option<Entity>,
}

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum DocumentCloseDecisionPolicy {
    #[default]
    Automatic,
    Ask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PromptOwner {
    Context(crate::OperationBarrierToken),
    Panels(Vec<Entity>),
    Quit,
}

#[derive(Resource)]
pub struct CloseDecisionPrompt {
    pub token: u64,
    pub documents: Vec<DocumentId>,
    owner: PromptOwner,
    decision: Option<CloseDecision>,
}

/// A response belongs to the prompt that produced it, even if its UI outlives
/// a cancelled operation.
pub fn answer_close_prompt(world: &mut World, token: u64, decision: CloseDecision) -> bool {
    let Some(mut prompt) = world.get_resource_mut::<CloseDecisionPrompt>() else {
        return false;
    };
    if prompt.token != token {
        return false;
    }
    prompt.decision = Some(decision);
    true
}

pub fn render_close_prompt(world: &mut World, ui: &dear_imgui_rs::Ui) {
    let Some(prompt) = world.get_resource::<CloseDecisionPrompt>() else {
        return;
    };
    let token = prompt.token;
    let documents = prompt.documents.clone();
    ui.open_popup("Save changes?##bedtime-documents");
    if let Some(_popup) = ui.begin_modal_popup("Save changes?##bedtime-documents") {
        ui.text("Save your changes before continuing?");
        for document in documents {
            let label = models::model_document(world, document)
                .map(|model| model.label.clone())
                .or_else(|| {
                    world
                        .non_send::<DocumentSession>()
                        .with_document(document, |state| {
                            if state.path.is_empty() {
                                "Untitled document".into()
                            } else {
                                state.path.clone()
                            }
                        })
                        .ok()
                })
                .unwrap_or_else(|| "Document".into());
            ui.bullet_text(label);
        }
        for (label, decision) in [
            ("Save", CloseDecision::Save),
            ("Discard", CloseDecision::Discard),
            ("Cancel", CloseDecision::Cancel),
        ] {
            if ui.button(label) {
                answer_close_prompt(world, token, decision);
                ui.close_current_popup();
            }
            ui.same_line();
        }
    }
}

fn lifecycle_policy(
    world: &mut World,
    owner: PromptOwner,
    dirty: &[DocumentId],
) -> Result<Option<ClosePolicy>, String> {
    if dirty.is_empty()
        || *world.resource::<DocumentCloseDecisionPolicy>()
            == DocumentCloseDecisionPolicy::Automatic
    {
        return Ok(Some(world.resource::<DocumentClosePolicy>().0));
    }
    if let Some(prompt) = world.get_resource::<CloseDecisionPrompt>() {
        if prompt.owner != owner {
            return Ok(None);
        }
        let Some(decision) = prompt.decision else {
            return Ok(None);
        };
        world.remove_resource::<CloseDecisionPrompt>();
        return match decision {
            CloseDecision::Save => Ok(Some(ClosePolicy::Save)),
            CloseDecision::Discard => Ok(Some(ClosePolicy::Discard)),
            CloseDecision::Cancel => Err("Operation cancelled".into()),
        };
    }
    world.insert_resource(CloseDecisionPrompt {
        token: bedtime_api::features::next_request_sequence(),
        documents: dirty.to_vec(),
        owner,
        decision: None,
    });
    Ok(None)
}

pub(crate) fn lifecycle_save_targets(
    world: &mut World,
    documents: &[DocumentId],
) -> Result<Vec<crate::DocumentSaveTarget>, String> {
    let mut targets = Vec::new();
    for &document in documents {
        let needs_path = !models::model_document_has_writer(world, document)
            && document_path(world, document)?.is_none_or(|path| path.as_os_str().is_empty());
        let destination = if needs_path {
            Some(
                rfd::FileDialog::new()
                    .set_directory(&world.resource::<bedtime_files::FileContext>().directory)
                    .save_file()
                    .ok_or("Operation cancelled")?,
            )
        } else {
            None
        };
        targets.push(crate::DocumentSaveTarget {
            document,
            destination,
        });
    }
    Ok(targets)
}

pub(crate) fn prepare_lifecycle_documents(
    world: &mut World,
    token: crate::OperationBarrierToken,
) -> Result<crate::OperationBarrierStatus, String> {
    match crate::barrier_status(world, token) {
        Some(crate::OperationBarrierStatus::Pending | crate::OperationBarrierStatus::Prepared) => {
            return crate::poll_document_transition(world, token);
        }
        Some(crate::OperationBarrierStatus::Waiting) => {
            return Ok(crate::OperationBarrierStatus::Waiting);
        }
        Some(crate::OperationBarrierStatus::Failed(error)) => return Err(error),
        _ => {}
    }
    let documents = crate::transitions::transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let dirty = documents
        .into_iter()
        .filter(|document| document_dirty(world, *document).unwrap_or(true))
        .collect::<Vec<_>>();
    let Some(policy) = lifecycle_policy(world, PromptOwner::Context(token), &dirty)? else {
        return Ok(crate::OperationBarrierStatus::NeedsDecision(dirty));
    };
    crate::prepare_document_transition(world, token, policy)
}
#[derive(Resource, Default)]
pub(crate) struct CoreExiting(pub bool);
#[derive(Resource, Default)]
struct LifecycleSaves {
    close: Option<crate::DocumentSaveId>,
    quit: Option<crate::DocumentSaveId>,
}
#[derive(Resource)]
struct LiveDocumentOrigin(bevy::asset::uuid::Uuid);
impl Default for LiveDocumentOrigin {
    fn default() -> Self {
        Self(bevy::asset::uuid::Uuid::new_v4())
    }
}
#[derive(Resource, Clone, Copy)]
pub struct DocumentClosePolicy(pub ClosePolicy);
impl Default for DocumentClosePolicy {
    fn default() -> Self {
        Self(ClosePolicy::Cancel)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreSystems {
    Services,
    Frame,
    Requests,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostRequestId(u64);
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostOutcome {
    Applied,
    Ignored,
    Opened(Entity),
    Pending(crate::DocumentSaveId),
    Transition(crate::OperationBarrierToken),
    Saved {
        documents: Vec<DocumentId>,
        revisions: Vec<Revision>,
    },
}

pub fn capture_host_request(
    world: &World,
    source: Option<Entity>,
    request: HostAction,
) -> HostRequest {
    capture_host_request_area(world, source, request, None)
}
pub fn capture_host_request_area(
    world: &World,
    source: Option<Entity>,
    request: HostAction,
    area: Option<u32>,
) -> HostRequest {
    let explicit_area = area.is_some();
    let layout = world.resource::<WorkbenchLayout>();
    let area = area.unwrap_or_else(|| {
        let source_area = matches!(
            &request,
            HostAction::OpenFileIn {
                target: bedtime_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::PreviewFileIn {
                target: bedtime_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::ShowPanelIn {
                target: bedtime_api::workbench::PanelTarget::LastFocused,
                ..
            } | HostAction::DropFile { .. }
        );
        #[cfg(feature = "language-services")]
        let source_area = source_area || matches!(&request, HostAction::NavigateLocation { .. });
        if source_area {
            source
                .and_then(|entity| bedtime_api::workbench::panel_owner(world, entity))
                .and_then(|owner| world.get::<bedtime_api::workbench::PanelPresentation>(owner))
                .and_then(|panel| layout.tiling.area_for(panel.id))
                .or(layout.focused_area)
                .unwrap_or_else(|| layout.tiling.layout.largest_area())
        } else {
            layout.tiling.layout.largest_area()
        }
    });
    HostRequest::capture(
        world.non_send::<DocumentSession>().session_id(),
        source,
        request,
        area,
        world
            .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
            .0,
    )
    .with_explicit_area(explicit_area)
}
#[derive(Message, Clone, Debug)]
pub struct CoreNotice {
    pub source: Option<Entity>,
    pub message: String,
}

struct Accepted {
    id: Option<HostRequestId>,
    request: HostRequest,
}
struct PendingCommand {
    id: Option<bedtime_workbench::WorkbenchRequestId>,
    outcome: Option<Result<bedtime_workbench::WorkbenchOutcome, String>>,
    effects: VecDeque<HostRequest>,
    saves: Vec<crate::DocumentSaveId>,
    barrier: Option<crate::OperationBarrierToken>,
    error: Option<String>,
    tab_index: Option<usize>,
    host_id: Option<HostRequestId>,
    last_outcome: Option<HostOutcome>,
    workbench: Option<bedtime_workbench::WorkbenchRequestId>,
    nested_outcome: Option<bedtime_workbench::WorkbenchOutcome>,
}
#[derive(Resource, Default)]
pub struct HostRequests {
    next: u64,
    pending: VecDeque<Accepted>,
    results: HashMap<HostRequestId, Result<HostOutcome, String>>,
    cursor: MessageCursor<HostRequest>,
    settings_cursor: MessageCursor<bedtime_api::settings::SettingsRequest>,
    settings: VecDeque<bedtime_api::settings::SettingsRequest>,
    saves: Vec<(Option<HostRequestId>, crate::DocumentSaveId)>,
    commands: Vec<PendingCommand>,
    transitions: Vec<(Option<HostRequestId>, crate::OperationBarrierToken)>,
    quit: Option<(HostRequestId, u64)>,
    batches: VecDeque<(HostRequestId, u64, Vec<HostRequest>)>,
}

pub fn submit_quit(world: &mut World) -> HostRequestId {
    if let Some((id, _)) = world.resource::<HostRequests>().quit {
        return id;
    }
    let mut requests = world.resource_mut::<HostRequests>();
    requests.next = requests
        .next
        .checked_add(1)
        .expect("document request ID overflow");
    let id = HostRequestId(requests.next);
    requests.quit = Some((id, bedtime_api::features::next_request_sequence()));
    id
}

pub fn submit_captured_host_request(world: &mut World, request: HostRequest) -> HostRequestId {
    let mut requests = world.resource_mut::<HostRequests>();
    requests.next = requests
        .next
        .checked_add(1)
        .expect("document request ID overflow");
    let id = HostRequestId(requests.next);
    requests.pending.push_back(Accepted {
        id: Some(id),
        request,
    });
    id
}

pub fn submit_host_batch(world: &mut World, effects: Vec<HostRequest>) -> HostRequestId {
    let mut requests = world.resource_mut::<HostRequests>();
    requests.next = requests
        .next
        .checked_add(1)
        .expect("document request ID overflow");
    let id = HostRequestId(requests.next);
    requests
        .batches
        .push_back((id, bedtime_api::features::next_request_sequence(), effects));
    id
}
pub fn submit_host_request(
    world: &mut World,
    source: Option<Entity>,
    request: HostAction,
) -> HostRequestId {
    let request = capture_host_request(world, source, request);
    let mut queue = world.resource_mut::<HostRequests>();
    queue.next = queue
        .next
        .checked_add(1)
        .expect("document request ID overflow");
    let id = HostRequestId(queue.next);
    queue.pending.push_back(Accepted {
        id: Some(id),
        request,
    });
    id
}
pub fn take_host_result(
    world: &mut World,
    id: HostRequestId,
) -> Option<Result<HostOutcome, String>> {
    world.resource_mut::<HostRequests>().results.remove(&id)
}

pub struct DocumentHostPlugin;
impl Plugin for DocumentHostPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_workbench::WorkbenchPlugin>() {
            app.add_plugins(bedtime_workbench::WorkbenchPlugin);
        }
        crate::settings::install_settings(app.world_mut());
        app.init_non_send::<DocumentSession>()
            .init_resource::<bedtime_files::FileContext>()
            .init_resource::<bedtime_files::FileServices>()
            .init_resource::<bedtime_storage::Storage>()
            .init_resource::<crate::saves::DocumentSaves>()
            .init_resource::<crate::transitions::OperationBarriers>()
            .init_non_send::<DocumentServiceReport>()
            .init_resource::<crate::files::FileOperations>()
            .init_resource::<ModelRegistry>()
            .init_resource::<DocumentFrame>()
            .init_resource::<HostEffects>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .init_resource::<bedtime_api::workbench::CommandPresentations>()
            .init_resource::<HostRequests>()
            .init_resource::<bedtime_api::settings::SettingsSnapshot>()
            .init_resource::<bedtime_api::presentation::PresentationFrame>()
            .add_message::<bedtime_api::settings::SettingsRequest>()
            .init_resource::<CoreExiting>()
            .init_resource::<LifecycleSaves>()
            .init_resource::<LiveDocumentOrigin>()
            .init_resource::<DocumentClosePolicy>()
            .init_resource::<DocumentCloseDecisionPolicy>()
            .init_resource::<DocumentNavigation>()
            .init_resource::<crate::LifecyclePreCommit>()
            .insert_resource(WorkbenchRequestMode::External)
            .add_message::<HostRequest>()
            .add_message::<CoreNotice>()
            .add_message::<bedtime_api::documents::DocumentReply>()
            .add_message::<bedtime_api::documents::DocumentEvent>()
            .add_message::<bedtime_api::storage::FeatureStateChanged>()
            .add_systems(
                PreUpdate,
                (
                    poll_documents.in_set(CoreSystems::Services),
                    publish_documents
                        .in_set(CoreSystems::Frame)
                        .after(CoreSystems::Services)
                        .before(bedtime_api::features::FeatureSystems::Prepare),
                    resolve_scheduled_requests
                        .in_set(CoreSystems::Requests)
                        .after(bedtime_api::features::FeatureSystems::Prepare),
                ),
            );
        if !app
            .world()
            .contains_resource::<bedtime_workbench::WorkbenchClosePolicy>()
        {
            app.insert_resource(bedtime_workbench::WorkbenchClosePolicy(close_panel));
        }
        if !app
            .world()
            .contains_resource::<bedtime_workbench::WorkbenchPanelRetirement>()
        {
            app.insert_resource(bedtime_workbench::WorkbenchPanelRetirement(retire_panel));
        }
        app.insert_resource(bedtime_workbench::WorkbenchSplitPolicy(split_panel));
        if !app
            .world()
            .contains_resource::<bedtime_workbench::PanelPreparation>()
        {
            app.insert_resource(bedtime_workbench::PanelPreparation(prepare_panel));
        }
        if !app.is_plugin_added::<crate::commands::DocumentCommandsPlugin>() {
            app.add_plugins(crate::commands::DocumentCommandsPlugin);
        }
        #[cfg(feature = "language-services")]
        app.init_resource::<crate::format_saves::ExplicitDocumentSaves>()
            .init_resource::<crate::SyntaxAppearance>();
    }
}
fn resolve_scheduled_requests(world: &mut World) {
    resolve_documents(world);
}
#[derive(Default)]
struct DocumentServiceReport(bedtime_documents::session::TickReport);

pub fn take_document_service_report(world: &mut World) -> bedtime_documents::session::TickReport {
    std::mem::take(&mut world.non_send_mut::<DocumentServiceReport>().0)
}
pub fn poll_document_services(world: &mut World) {
    let _timing =
        bedtime_desktop::measure_frame_stage(world, bedtime_desktop::FrameStage::Documents);
    admit(world);
    supersede_file_intentions(world);
    crate::settings::refresh_settings(world);
    #[cfg(feature = "language-services")]
    crate::configuration::configure_syntax(world);
    let report = world.non_send_mut::<DocumentSession>().tick();
    crate::saves::poll_document_saves(world, &report);
    #[cfg(feature = "language-services")]
    crate::format_saves::poll_explicit_document_saves(world);
    crate::transitions::poll_barrier_admission(world);
    supersede_file_intentions(world);
    crate::files::poll_file_operations(world);
    for event in &report.events {
        if let bedtime_documents::SessionEvent::Removed { document, .. } = event {
            let panels = panel_entities(world)
                .into_iter()
                .filter(|panel| {
                    world
                        .get::<PanelBinding>(*panel)
                        .is_some_and(|binding| binding.document == Some(*document))
                })
                .collect::<Vec<_>>();
            let committed = commit_document_drafts(world, &[*document]);
            let dirty = document_dirty(world, *document).unwrap_or(true);
            if committed.is_err() || dirty {
                if let Err(error) = world
                    .non_send_mut::<DocumentSession>()
                    .detach_removed_document(*document)
                {
                    notice(world, None, error.to_string());
                }
                if let Err(error) = committed {
                    notice(
                        world,
                        None,
                        format!("Deleted file buffer preserved: {error}"),
                    );
                }
            } else if let Err(error) = retire_panels(world, &panels) {
                notice(world, None, error);
            }
        }
        world.write_message(bedtime_api::documents::DocumentEvent(event.clone()));
    }
    for error in &report.errors {
        notice(world, None, error.message.clone());
    }
    world.non_send_mut::<DocumentServiceReport>().0 = report;
}
fn poll_documents(world: &mut World) {
    poll_document_services(world);
}
/// Explicit bindings on the requested entity win; an owning tab can also
/// associate its document independently of its child frontend.
pub(crate) fn panel_document_selection(
    world: &World,
    source: Entity,
) -> Option<(DocumentId, Option<bedtime_documents::ViewId>)> {
    let owner = bedtime_api::workbench::panel_owner(world, source).unwrap_or(source);
    [
        Some(source),
        (owner != source).then_some(owner),
        world
            .get::<bedtime_api::workbench::PanelFrontend>(owner)
            .map(|frontend| frontend.0),
    ]
    .into_iter()
    .flatten()
    .find_map(|entity| {
        world
            .get::<models::DocumentBinding>(entity)
            .map(|binding| (binding.0, None))
            .or_else(|| {
                world
                    .get::<PanelBinding>(entity)
                    .and_then(|binding| binding.document.map(|document| (document, binding.view)))
            })
    })
}

pub fn publish_documents(world: &mut World) {
    let bindings = world
        .query::<(Entity, &bedtime_api::workbench::PanelFrontend)>()
        .iter(world)
        .filter_map(|(panel, frontend)| {
            world
                .get::<PanelBinding>(frontend.0)
                .filter(|binding| {
                    binding.document.is_some()
                        || binding.view.is_some()
                        || binding.terminal.is_some()
                        || binding.input.local_file().is_some()
                        || world.get::<PanelBinding>(panel).is_none()
                })
                .cloned()
                .map(|binding| (panel, binding))
        })
        .collect::<Vec<_>>();
    for (panel, binding) in bindings {
        world.entity_mut(panel).insert(binding);
    }
    let result = world.resource_scope(|world, mut frame: Mut<DocumentFrame>| {
        frame.session_id = world.non_send::<DocumentSession>().session_id().0;
        frame.refresh_documents(world.non_send::<DocumentSession>())
    });
    if let Err(error) = result {
        notice(world, None, error.to_string());
    }
    let records = world
        .query::<(Entity, &PanelBinding)>()
        .iter(world)
        .filter_map(|(entity, binding)| binding.document.map(|document| (entity, document)))
        .collect::<Vec<_>>();
    for (entity, document) in records {
        if let Ok((path, kind, dirty)) = world
            .non_send::<DocumentSession>()
            .with_document(document, |state| {
                (state.path.clone(), state.kind, state.dirty)
            })
        {
            let owner = bedtime_api::workbench::panel_owner(world, entity).unwrap_or(entity);
            if !path.is_empty()
                && let Some(mut record) = world
                    .get_mut::<bedtime_workbench::PanelRecord>(owner)
                    .filter(|record| record.unavailable.is_none())
            {
                record.serialized["path"] = json!(path);
                record.serialized["document_kind"] =
                    json!(if kind == bedtime_documents::DocumentKind::Text {
                        "text"
                    } else {
                        "bytes"
                    });
            }
            if world
                .get::<bedtime_api::workbench::PanelPresentation>(owner)
                .is_some()
            {
                world
                    .entity_mut(owner)
                    .insert(bedtime_workbench::PanelDirty(dirty));
            }
        }
    }
    let models = world
        .query::<(Entity, &models::DocumentBinding)>()
        .iter(world)
        .map(|(entity, binding)| (entity, binding.0))
        .collect::<Vec<_>>();
    for (entity, document) in models {
        if let Some(dirty) =
            models::model_document(world, document).map(|document| document.dirty())
            && let Some(owner) = bedtime_api::workbench::panel_owner(world, entity)
        {
            world
                .entity_mut(owner)
                .insert(bedtime_workbench::PanelDirty(dirty));
        }
    }
    let focused = world.resource::<WorkbenchLayout>().focused;
    let frontend = focused.and_then(|entity| {
        world
            .get::<bedtime_api::workbench::PanelFrontend>(entity)
            .map_or(Some(entity), |frontend| Some(frontend.0))
    });
    let focused_selection = focused.and_then(|panel| panel_document_selection(world, panel));
    if let Some((document, view)) = focused_selection {
        let mut navigation = world.resource_mut::<DocumentNavigation>();
        navigation.active_document = Some(document);
        navigation.active_view = view;
        navigation.last_document_panel = focused;
    } else if frontend.is_some_and(|entity| {
        world
            .get::<PanelBinding>(entity)
            .is_some_and(|binding| binding.input.local_file().is_some())
    }) {
        let mut navigation = world.resource_mut::<DocumentNavigation>();
        navigation.active_document = None;
        navigation.active_view = None;
    } else {
        let selection = world
            .resource::<DocumentNavigation>()
            .last_document_panel
            .and_then(|panel| panel_document_selection(world, panel));
        let mut navigation = world.resource_mut::<DocumentNavigation>();
        navigation.active_document = selection.map(|(document, _)| document);
        navigation.active_view = selection.and_then(|(_, view)| view);
    }
    if world
        .get_resource::<DocumentFrameSelection>()
        .is_some_and(|selection| selection.document.is_none())
    {
        *world.resource_mut::<DocumentNavigation>() = DocumentNavigation::default();
    }
    let navigation = world.resource::<DocumentNavigation>();
    let retained_selection = navigation
        .active_document
        .map(|document| (document, navigation.active_view));
    let active =
        world
            .get_resource::<DocumentFrameSelection>()
            .map_or(retained_selection, |selection| {
                selection
                    .document
                    .map(|document| (document, selection.view))
            });
    {
        let mut frame = world.resource_mut::<DocumentFrame>();
        frame.active_document = active.map(|(document, _)| document);
        frame.active_view = active.and_then(|(_, view)| view);
    }
    let previews = world
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .filter_map(|area| area.preview)
        .collect::<HashSet<_>>();
    let edited = world
        .query::<(&bedtime_workbench::PanelPresentation, &PanelBinding)>()
        .iter(world)
        .filter_map(|(panel, binding)| {
            (previews.contains(&panel.id)
                && binding
                    .document
                    .is_some_and(|document| document_dirty(world, document).unwrap_or(false)))
            .then_some(panel.id)
        })
        .collect::<Vec<_>>();
    for id in edited {
        world.resource_mut::<WorkbenchLayout>().tiling.keep_tab(id);
    }
    crate::commands::publish_document_commands(world);
}

/// Validate every document before quitting. Application-specific prompts and
/// persistence can choose a close policy; a failed save leaves all panels live.
pub fn quit(world: &mut World) -> Result<(), String> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "quit resolves after drawing"
    );
    admit(world);
    if crate::operation_barrier_active(world)
        || !world.resource::<HostRequests>().saves.is_empty()
        || !world.resource::<HostRequests>().commands.is_empty()
        || !world.resource::<HostRequests>().settings.is_empty()
        || !world.resource::<HostRequests>().pending.is_empty()
        || world
            .resource::<WorkbenchRequests>()
            .next_sequence()
            .is_some()
        || bedtime_workbench::waiting_for_layout(world)
    {
        return Err("Wait for pending document and layout operations before quitting".into());
    }
    finish_quit(world)
}

fn finish_quit(world: &mut World) -> Result<(), String> {
    if let Some(save) = world.resource::<LifecycleSaves>().quit {
        let Some(result) = crate::poll_document_save(world, save) else {
            return Err("Waiting for documents to finish saving".into());
        };
        world.resource_mut::<LifecycleSaves>().quit = None;
        let saved = result?;
        let documents = saved.iter().map(|saved| saved.document).collect::<Vec<_>>();
        commit_document_drafts(world, &documents)?;
        for saved in saved {
            if document_revision(world, saved.document)? != saved.revision {
                return Err("Document changed while saving; prepare the operation again".into());
            }
        }
    }
    let documents = crate::transitions::transition_documents(world);
    commit_document_drafts(world, &documents)?;
    let mut dirty = documents
        .into_iter()
        .filter(|document| {
            world
                .non_send::<DocumentSession>()
                .with_document(*document, |state| state.dirty)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    dirty.extend(
        world
            .query::<&models::ModelDocument>()
            .iter(world)
            .filter(|document| document.dirty())
            .map(models::ModelDocument::id),
    );
    let Some(policy) = lifecycle_policy(world, PromptOwner::Quit, &dirty)? else {
        return Err("Waiting for a document decision".into());
    };
    if !dirty.is_empty() {
        match policy {
            ClosePolicy::Cancel => {
                return Err("Save or discard changed documents before quitting".into());
            }
            ClosePolicy::Save => {
                let targets = lifecycle_save_targets(world, &dirty)?;
                let save = crate::begin_document_save(world, &targets)?;
                if let Some(result) = crate::poll_document_save(world, save) {
                    result?;
                } else {
                    world.resource_mut::<LifecycleSaves>().quit = Some(save);
                    return Err("Waiting for documents to finish saving".into());
                }
            }
            ClosePolicy::Discard => {}
        }
    }
    crate::transitions::run_lifecycle_pre_commit(world, crate::LifecycleReason::Quit)?;
    world
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .map_err(|error| error.to_string())?;
    let panels = panel_entities(world);
    retire_panels(world, &panels)?;
    world.resource_mut::<CoreExiting>().0 = true;
    Ok(())
}

fn prepare_panel(world: &mut World, entity: Entity) -> Result<(), String> {
    let record = world
        .get::<bedtime_workbench::PanelRecord>(entity)
        .ok_or("Panel record is unavailable")?
        .serialized
        .clone();
    let Some(path) = record.get("path").and_then(Value::as_str) else {
        if let Some(id) = record["document"].as_u64() {
            let session = world.non_send::<DocumentSession>().session_id().0;
            if record["document_session"].as_u64() != Some(session)
                || record["document_origin"].as_str()
                    != Some(
                        world
                            .resource::<LiveDocumentOrigin>()
                            .0
                            .to_string()
                            .as_str(),
                    )
            {
                return Err(
                    "Untitled documents require application restoration after a session change"
                        .into(),
                );
            }
            let document = DocumentId(id);
            world
                .non_send::<DocumentSession>()
                .with_document(document, |_| ())
                .map_err(|error| error.to_string())?;
            world.entity_mut(entity).insert(PanelBinding {
                input: PanelInput::Document(document),
                document: Some(document),
                session_id: session,
                ..Default::default()
            });
        }
        prepare_frontend_shell(world, entity);
        return Ok(());
    };
    let document = match record.get("backing").and_then(Value::as_str) {
        Some("local_file") => None,
        _ => {
            let kind = if record.get("document_kind").and_then(Value::as_str) == Some("bytes") {
                bedtime_documents::DocumentKind::Bytes
            } else {
                bedtime_documents::DocumentKind::Text
            };
            Some(
                world
                    .non_send_mut::<DocumentSession>()
                    .open_file_with_kind(Path::new(path), kind)
                    .map_err(|error| error.to_string())?,
            )
        }
    };
    let session = world.non_send::<DocumentSession>().session_id().0;
    world.entity_mut(entity).insert(PanelBinding {
        input: document.map_or_else(|| PanelInput::LocalFile(path.into()), PanelInput::Document),
        document,
        viewer: record["viewer"].as_str().map(bedtime_api::ViewerId::from),
        session_id: session,
        ..Default::default()
    });
    prepare_frontend_shell(world, entity);
    Ok(())
}

fn prepare_frontend_shell(world: &mut World, panel: Entity) {
    if world
        .get::<bedtime_api::workbench::PanelFrontend>(panel)
        .is_some()
    {
        return;
    }
    let record = world
        .get::<bedtime_workbench::PanelRecord>(panel)
        .unwrap()
        .clone();
    let binding = world
        .get::<PanelBinding>(panel)
        .cloned()
        .unwrap_or_else(|| PanelBinding {
            session_id: world.non_send::<DocumentSession>().session_id().0,
            ..Default::default()
        });
    let frontend = world
        .spawn((bedtime_api::workbench::PanelOwner(panel), record, binding))
        .id();
    world
        .entity_mut(panel)
        .insert(bedtime_api::workbench::PanelFrontend(frontend));
}

fn notice(world: &mut World, source: Option<Entity>, message: impl Into<String>) {
    world.write_message(CoreNotice {
        source,
        message: message.into(),
    });
}
fn admit(world: &mut World) {
    let requests = world.resource_scope(|world, mut queue: Mut<HostRequests>| {
        queue
            .cursor
            .read(world.resource::<Messages<HostRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world.resource_mut::<HostRequests>().pending.extend(
        requests
            .into_iter()
            .map(|request| Accepted { id: None, request }),
    );
    let settings = world.resource_scope(|world, mut queue: Mut<HostRequests>| {
        queue
            .settings_cursor
            .read(world.resource::<Messages<bedtime_api::settings::SettingsRequest>>())
            .cloned()
            .collect::<Vec<_>>()
    });
    world
        .resource_mut::<HostRequests>()
        .settings
        .extend(settings);
    world
        .resource_mut::<HostRequests>()
        .settings
        .make_contiguous()
        .sort_by_key(|setting| setting.sequence);
    for intent in world.resource_mut::<HostEffects>().take() {
        let request = capture_host_request_area(world, intent.source, intent.action, intent.area)
            .with_tab_index(intent.tab_index);
        world
            .resource_mut::<HostRequests>()
            .pending
            .push_back(Accepted { id: None, request });
    }
    world
        .resource_mut::<HostRequests>()
        .pending
        .make_contiguous()
        .sort_by_key(|accepted| accepted.request.sequence());
}

/// Preview selection describes the latest requested contents of an area.
/// Durable saves/context changes remain ordered; an obsolete read is the one
/// operation a newer selection or explicit layout replacement can retire.
fn supersede_file_intentions(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<HostRequests>().pending);
    let keep = pending
        .iter()
        .enumerate()
        .map(|(index, accepted)| {
            !crate::files::is_preview(&accepted.request)
                || validate_request_origin(world, &accepted.request).is_err()
                || !pending.iter().skip(index + 1).any(|next| {
                    crate::files::supersedes_preview(&accepted.request, &next.request)
                        && validate_request_origin(world, &next.request).is_ok()
                })
        })
        .collect::<Vec<_>>();
    for (accepted, keep) in pending.into_iter().zip(keep) {
        if keep {
            world
                .resource_mut::<HostRequests>()
                .pending
                .push_back(accepted);
        } else if let Some(id) = accepted.id {
            world
                .resource_mut::<HostRequests>()
                .results
                .insert(id, Ok(HostOutcome::Ignored));
        }
    }
    let Some(previous) = crate::files::pending_open(world).cloned() else {
        return;
    };
    let Some(token) = crate::active_operation_barrier(world) else {
        return;
    };
    if crate::barrier_status(world, token) != Some(crate::OperationBarrierStatus::Ready) {
        return;
    }
    let replacement = world
        .resource::<HostRequests>()
        .pending
        .iter()
        .find_map(|accepted| {
            (crate::files::supersedes_preview(&previous, &accepted.request)
                && validate_request_origin(world, &accepted.request).is_ok())
            .then(|| accepted.request.clone())
        });
    let replace_layout = world
        .resource::<WorkbenchRequests>()
        .next_request()
        .is_some_and(|queued| {
            matches!(
                queued.action,
                bedtime_api::workbench::WorkbenchAction::SetLayout { .. }
            ) && bedtime_workbench::request_is_current(world, queued)
        });
    if replacement.is_some() || replace_layout {
        crate::files::cancel_pending_open(world, replacement.as_ref())
            .expect("the active read can be retired");
    }
}

fn command_opened_panel(world: &mut World, command: &mut PendingCommand, panel: Entity) {
    command.last_outcome = Some(HostOutcome::Opened(panel));
    if let Some(index) = command.tab_index
        && let Some(presentation) = world.get::<bedtime_workbench::PanelPresentation>(panel)
    {
        let id = presentation.id;
        let area = world
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(id)
            .expect("opened panel belongs to an area");
        world
            .resource_mut::<WorkbenchLayout>()
            .tiling
            .move_tab(id, area, index);
    }
}

fn poll_command_workbench(world: &mut World, command: &mut PendingCommand) {
    let Some(id) = command.workbench else {
        return;
    };
    let Some(result) = world.resource_mut::<WorkbenchRequests>().take_result(id) else {
        return;
    };
    command.workbench = None;
    match result {
        Err(error) => {
            command.error.get_or_insert(error);
        }
        Ok(bedtime_workbench::WorkbenchOutcome::Opened(panel)) => {
            command_opened_panel(world, command, panel);
            command.nested_outcome = Some(bedtime_workbench::WorkbenchOutcome::Opened(panel));
        }
        Ok(outcome @ bedtime_workbench::WorkbenchOutcome::Closed(_)) => {
            command.nested_outcome = Some(outcome)
        }
        Ok(_) => {}
    }
}

fn continue_command(world: &mut World, mut command: PendingCommand) {
    if command.outcome.is_none()
        && let Some(id) = command.id
        && let Some(result) = world.resource_mut::<WorkbenchRequests>().take_result(id)
    {
        command.outcome = Some(result);
    }
    poll_command_workbench(world, &mut command);
    for save in std::mem::take(&mut command.saves) {
        match crate::poll_document_save(world, save) {
            Some(Ok(saved)) => {
                if command
                    .last_outcome
                    .as_ref()
                    .is_none_or(|outcome| !matches!(outcome, HostOutcome::Opened(_)))
                {
                    command.last_outcome = Some(HostOutcome::Saved {
                        documents: saved.iter().map(|saved| saved.document).collect(),
                        revisions: saved.iter().map(|saved| saved.revision).collect(),
                    });
                }
            }
            Some(Err(error)) => {
                command.error.get_or_insert(error);
            }
            None => command.saves.push(save),
        }
    }
    if let Some(token) = command.barrier {
        match crate::barrier_status(world, token).expect("Admitted command barrier exists") {
            crate::OperationBarrierStatus::Complete => {
                if let Some(outcome) = crate::files::take_outcome(world, token) {
                    match outcome {
                        HostOutcome::Opened(panel) => {
                            command_opened_panel(world, &mut command, panel)
                        }
                        other => command.last_outcome = Some(other),
                    }
                }
                command.barrier = None;
            }
            crate::OperationBarrierStatus::Failed(error) => {
                command.error.get_or_insert(error);
                command.barrier = None;
            }
            _ => {}
        }
    }
    if command.error.is_some() || command.outcome.as_ref().is_some_and(Result::is_err) {
        command.effects.clear();
    }
    if command.barrier.is_none()
        && command.saves.is_empty()
        && command.workbench.is_none()
        && (command.id.is_none() || command.outcome.is_some())
    {
        while let Some(request) = command.effects.pop_front() {
            command.tab_index = request.tab_index().or(command.tab_index);
            let result = apply_host_step(world, request).map(|step| {
                for effect in step.effects.into_iter().rev() {
                    command.effects.push_front(effect);
                }
                command.workbench = step.workbench;
                step.outcome
            });
            match result {
                Ok(HostOutcome::Opened(panel)) => command_opened_panel(world, &mut command, panel),
                Ok(saved @ HostOutcome::Saved { .. }) => {
                    if command
                        .last_outcome
                        .as_ref()
                        .is_none_or(|outcome| !matches!(outcome, HostOutcome::Opened(_)))
                    {
                        command.last_outcome = Some(saved);
                    }
                }
                Ok(HostOutcome::Pending(save)) => {
                    command.saves.push(save);
                    break;
                }
                Ok(HostOutcome::Transition(token)) => {
                    command.barrier = Some(token);
                    break;
                }
                Err(error) => {
                    command.error.get_or_insert(error);
                    command.effects.clear();
                    break;
                }
                _ => {}
            }
            poll_command_workbench(world, &mut command);
            if command.error.is_some() {
                command.effects.clear();
                break;
            }
            if command.workbench.is_some() {
                break;
            }
            if let Some(token) = crate::active_operation_barrier(world) {
                command.barrier = Some(token);
                break;
            }
        }
    }
    if command.effects.is_empty()
        && command.saves.is_empty()
        && command.barrier.is_none()
        && command.workbench.is_none()
        && (command.id.is_none() || command.outcome.is_some())
    {
        if let Some(id) = command.host_id {
            let result = command.error.map_or_else(
                || Ok(command.last_outcome.unwrap_or(HostOutcome::Applied)),
                Err,
            );
            world
                .resource_mut::<HostRequests>()
                .results
                .insert(id, result);
            return;
        }
        if let Some(id) = command.id {
            let outcome = match command.error {
                Some(error) => Err(error),
                None => match command.last_outcome {
                    Some(HostOutcome::Opened(panel)) => {
                        Ok(bedtime_workbench::WorkbenchOutcome::Opened(panel))
                    }
                    _ => command.nested_outcome.map_or_else(
                        || command.outcome.expect("Completed command has a result"),
                        Ok,
                    ),
                },
            };
            world
                .resource_mut::<WorkbenchRequests>()
                .complete(id, outcome);
        } else if let Some(error) = command.error {
            notice(world, None, error);
        }
    } else {
        world.resource_mut::<HostRequests>().commands.push(command);
    }
}

pub fn resolve_documents(world: &mut World) {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "document requests run after drawing"
    );
    admit(world);
    crate::transitions::poll_barrier_admission(world);
    supersede_file_intentions(world);
    let transitions = std::mem::take(&mut world.resource_mut::<HostRequests>().transitions);
    for (receipt, token) in transitions {
        let result = match crate::barrier_status(world, token) {
            Some(crate::OperationBarrierStatus::Complete) => Some(Ok(crate::files::take_outcome(
                world, token,
            )
            .unwrap_or(HostOutcome::Applied))),
            Some(crate::OperationBarrierStatus::Failed(error)) => Some(Err(error)),
            _ => None,
        };
        if let Some(result) = result {
            if let Some(receipt) = receipt {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(receipt, result);
            } else if let Err(error) = result {
                notice(world, None, error);
            }
        } else {
            world
                .resource_mut::<HostRequests>()
                .transitions
                .push((receipt, token));
        }
    }
    bedtime_workbench::poll_request_results(world);
    let commands = std::mem::take(&mut world.resource_mut::<HostRequests>().commands);
    for command in commands {
        continue_command(world, command);
    }
    let pending = std::mem::take(&mut world.resource_mut::<HostRequests>().saves);
    for (receipt, save) in pending {
        if let Some(result) = crate::poll_document_save(world, save) {
            let result = result.map(|saved| HostOutcome::Saved {
                documents: saved.iter().map(|saved| saved.document).collect(),
                revisions: saved.iter().map(|saved| saved.revision).collect(),
            });
            if let Some(receipt) = receipt {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(receipt, result);
            } else if let Err(error) = result {
                notice(world, None, error);
            }
        } else {
            world
                .resource_mut::<HostRequests>()
                .saves
                .push((receipt, save));
        }
    }
    while !bedtime_workbench::waiting_for_layout(world)
        && !world.resource::<CoreExiting>().0
        && !crate::operation_barrier_active(world)
        && world.resource::<HostRequests>().commands.is_empty()
    {
        let document_sequence = world
            .resource::<HostRequests>()
            .pending
            .front()
            .map(|entry| entry.request.sequence());
        let panel_sequence = world.resource::<WorkbenchRequests>().next_sequence();
        let setting_sequence = world
            .resource::<HostRequests>()
            .settings
            .front()
            .map(|setting| setting.sequence);
        let quit_request = world.resource::<HostRequests>().quit;
        let batch_sequence = world
            .resource::<HostRequests>()
            .batches
            .front()
            .map(|(_, sequence, _)| *sequence);
        if batch_sequence.is_some_and(|sequence| {
            document_sequence.is_none_or(|next| sequence < next)
                && panel_sequence.is_none_or(|next| sequence < next)
                && setting_sequence.is_none_or(|next| sequence < next)
                && quit_request.is_none_or(|(_, next)| sequence < next)
        }) {
            let (id, _, effects) = world
                .resource_mut::<HostRequests>()
                .batches
                .pop_front()
                .unwrap();
            continue_command(
                world,
                PendingCommand {
                    id: None,
                    outcome: None,
                    effects: effects.into(),
                    saves: Vec::new(),
                    barrier: None,
                    error: None,
                    tab_index: None,
                    host_id: Some(id),
                    last_outcome: None,
                    workbench: None,
                    nested_outcome: None,
                },
            );
            continue;
        }
        if quit_request.is_some_and(|(_, sequence)| {
            document_sequence.is_none_or(|next| sequence < next)
                && panel_sequence.is_none_or(|next| sequence < next)
                && setting_sequence.is_none_or(|next| sequence < next)
                && batch_sequence.is_none_or(|next| sequence < next)
        }) {
            let (id, _) = quit_request.unwrap();
            if !world.resource::<HostRequests>().saves.is_empty()
                || (crate::saves::saves_pending(world)
                    && world.resource::<LifecycleSaves>().quit.is_none())
            {
                break;
            }
            let result = finish_quit(world);
            if result.is_err()
                && (world.resource::<LifecycleSaves>().quit.is_some()
                    || world
                        .get_resource::<CloseDecisionPrompt>()
                        .is_some_and(|prompt| prompt.owner == PromptOwner::Quit))
            {
                break;
            }
            world.resource_mut::<HostRequests>().quit = None;
            match result {
                Ok(()) => {
                    world
                        .resource_mut::<HostRequests>()
                        .results
                        .insert(id, Ok(HostOutcome::Applied));
                    world.write_message(AppExit::Success);
                }
                Err(error) => {
                    notice(world, None, error.clone());
                    world
                        .resource_mut::<HostRequests>()
                        .results
                        .insert(id, Err(error));
                }
            }
            break;
        }
        if setting_sequence.is_some_and(|sequence| {
            document_sequence.is_none_or(|next| sequence < next)
                && panel_sequence.is_none_or(|next| sequence < next)
        }) {
            let setting = world
                .resource_mut::<HostRequests>()
                .settings
                .pop_front()
                .unwrap();
            if let Err(error) = crate::settings::apply_settings_request(world, &setting) {
                notice(world, None, error);
            }
            continue;
        }
        if panel_sequence.is_some()
            && (document_sequence.is_none() || panel_sequence < document_sequence)
        {
            let queued = world
                .resource_mut::<WorkbenchRequests>()
                .take_next()
                .unwrap();
            let receipt = queued.id;
            let tab_index = queued.tab_index;
            // Every admitted operation retains a receipt while its close/save
            // continuation runs, including ordinary UI fire-and-forget actions.
            let workbench = if receipt.is_none() {
                Some(bedtime_workbench::resolve_tracked_request(world, queued))
            } else {
                bedtime_workbench::resolve_request(world, queued);
                None
            };
            // The outbox is empty at command entry. Its newly produced effects
            // finish before the next independent command.
            let effects = world
                .resource_mut::<HostEffects>()
                .take()
                .into_iter()
                .map(|intent| {
                    capture_host_request_area(world, intent.source, intent.action, intent.area)
                        .with_tab_index(intent.tab_index)
                })
                .collect();
            let outcome =
                receipt.and_then(|id| world.resource_mut::<WorkbenchRequests>().take_result(id));
            continue_command(
                world,
                PendingCommand {
                    id: receipt,
                    outcome,
                    effects,
                    saves: Vec::new(),
                    barrier: None,
                    error: None,
                    tab_index,
                    host_id: None,
                    last_outcome: None,
                    workbench,
                    nested_outcome: None,
                },
            );
        } else if let Some(accepted) = world.resource_mut::<HostRequests>().pending.pop_front() {
            let source = accepted.request.source();
            let tab_index = accepted.request.tab_index();
            let result = apply_host_step(world, accepted.request);
            let result = match result {
                Ok(step)
                    if !step.effects.is_empty()
                        || step.workbench.is_some()
                        || tab_index.is_some() =>
                {
                    let mut command = PendingCommand {
                        id: None,
                        outcome: None,
                        effects: step.effects,
                        saves: Vec::new(),
                        barrier: None,
                        error: None,
                        tab_index,
                        host_id: accepted.id,
                        last_outcome: Some(step.outcome.clone()),
                        workbench: step.workbench,
                        nested_outcome: None,
                    };
                    match step.outcome {
                        HostOutcome::Opened(panel) => {
                            command_opened_panel(world, &mut command, panel)
                        }
                        HostOutcome::Pending(save) => command.saves.push(save),
                        HostOutcome::Transition(token) => command.barrier = Some(token),
                        _ => {}
                    }
                    continue_command(world, command);
                    continue;
                }
                Ok(step) => Ok(step.outcome),
                Err(error) => Err(error),
            };
            if let Ok(HostOutcome::Transition(token)) = &result {
                world
                    .resource_mut::<HostRequests>()
                    .transitions
                    .push((accepted.id, *token));
            } else if let Ok(HostOutcome::Pending(save)) = &result {
                world
                    .resource_mut::<HostRequests>()
                    .saves
                    .push((accepted.id, *save));
            } else if let Some(id) = accepted.id {
                world
                    .resource_mut::<HostRequests>()
                    .results
                    .insert(id, result);
            } else if let Err(error) = result {
                notice(world, source, error);
            }
        } else {
            break;
        }
    }
    publish_documents(world);
}

pub fn open_document_panel(
    world: &mut World,
    panel_type: &bedtime_api::PanelTypeId,
    document: DocumentId,
    state: Value,
    area: Option<u32>,
) -> Result<Entity, String> {
    open_document_panel_with_viewer(world, panel_type, document, state, area, None)
}

fn open_document_panel_with_viewer(
    world: &mut World,
    panel_type: &bedtime_api::PanelTypeId,
    document: DocumentId,
    state: Value,
    area: Option<u32>,
    viewer: Option<&bedtime_api::ViewerId>,
) -> Result<Entity, String> {
    let (path, kind) = world
        .non_send::<DocumentSession>()
        .with_document(document, |state| (state.path.clone(), state.kind))
        .map_err(|error| error.to_string())?;
    let mut serialized = json!({"panel_type":panel_type,"state":state,"document":document.0,"backing":"document","document_kind":if kind == bedtime_documents::DocumentKind::Text { "text" } else { "bytes" },"viewer":viewer});
    if !path.is_empty() {
        serialized["path"] = json!(path);
    }
    serialized["document_session"] = json!(world.non_send::<DocumentSession>().session_id().0);
    serialized["document_origin"] = json!(world.resource::<LiveDocumentOrigin>().0.to_string());
    let panel = bedtime_workbench::spawn_panel(world, serialized, area, None)?;
    let binding = PanelBinding {
        input: PanelInput::Document(document),
        document: Some(document),
        viewer: viewer.cloned(),
        session_id: world.non_send::<DocumentSession>().session_id().0,
        ..Default::default()
    };
    world.entity_mut(panel).insert(binding.clone());
    if let Err(error) = mount_panel_frontend(world, panel, panel_type, binding, state) {
        bedtime_workbench::commit_close(world, panel);
        return Err(error);
    }
    bedtime_workbench::focus_panel(world, panel);
    publish_documents(world);
    Ok(panel)
}

/// Tool and document panels share one stable tab shell and one owned frontend.
pub fn open_panel(
    world: &mut World,
    kind: &bedtime_api::PanelTypeId,
    input: PanelInput,
    state: Value,
    viewer: Option<bedtime_api::ViewerId>,
    area: Option<u32>,
    show_existing: bool,
) -> Result<Entity, String> {
    if show_existing {
        let existing = world
            .query::<(Entity, &bedtime_workbench::PanelRecord, &PanelBinding)>()
            .iter(world)
            .find_map(|(panel, record, binding)| {
                (record.panel_type == *kind
                    && binding.input == input
                    && bedtime_api::workbench::panel_active(world, panel)
                    && world
                        .get::<bedtime_workbench::PanelPresentation>(panel)
                        .is_some())
                .then_some(panel)
            });
        if let Some(panel) = existing {
            bedtime_workbench::focus_panel(world, panel);
            publish_documents(world);
            return Ok(panel);
        }
    }
    if world
        .resource::<bedtime_api::workbench::WorkbenchRegistry>()
        .panel(kind)
        .is_none()
    {
        return Err(format!("Panel feature unavailable: {kind}"));
    }
    let serialized = json!({"panel_type":kind,"viewer":viewer,"state":state,"document":input.document().map(|document|document.0),"path":input.local_file().map(|path|path.to_string_lossy().into_owned())});
    let panel = bedtime_workbench::spawn_panel(world, serialized, area, None)?;
    let binding = PanelBinding {
        document: input.document(),
        input,
        viewer,
        session_id: world.non_send::<DocumentSession>().session_id().0,
        ..Default::default()
    };
    world.entity_mut(panel).insert(binding.clone());
    if let Err(error) = mount_panel_frontend(world, panel, kind, binding, state) {
        bedtime_workbench::commit_close(world, panel);
        return Err(error);
    }
    bedtime_workbench::focus_panel(world, panel);
    publish_documents(world);
    Ok(panel)
}

pub(crate) fn mount_panel_frontend(
    world: &mut World,
    panel: Entity,
    kind: &bedtime_api::PanelTypeId,
    binding: PanelBinding,
    state: Value,
) -> Result<Entity, String> {
    let factory = world
        .resource::<bedtime_api::workbench::WorkbenchRegistry>()
        .panel(kind)
        .map(|definition| definition.factory);
    let unavailable = world
        .get::<bedtime_workbench::PanelRecord>(panel)
        .and_then(|record| record.unavailable.clone())
        .or_else(|| {
            factory
                .is_none()
                .then(|| format!("Panel feature unavailable: {kind}"))
        });
    let serialized = world
        .get::<bedtime_workbench::PanelRecord>(panel)
        .map(|record| record.serialized.clone())
        .unwrap_or_else(|| json!({"panel_type":kind,"state":state}));
    let entity = world
        .spawn((
            bedtime_api::workbench::PanelOwner(panel),
            binding,
            bedtime_workbench::PanelRecord {
                panel_type: kind.into(),
                serialized,
                unavailable: unavailable.clone(),
            },
        ))
        .id();
    world
        .entity_mut(panel)
        .insert(bedtime_api::workbench::PanelFrontend(entity));
    publish_documents(world);
    if let Some(error) = unavailable {
        world
            .get_mut::<bedtime_workbench::PanelRecord>(panel)
            .unwrap()
            .unavailable = Some(error.clone());
        return Err(error);
    }
    let result = world
        .run_system_with(
            factory.expect("available panel has a factory"),
            bedtime_api::workbench::PanelCreation { entity, state },
        )
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    match result {
        Ok(frontend) => {
            bedtime_workbench::install_panel_frontend(world, entity, frontend)?;
        }
        Err(error) => {
            world
                .get_mut::<bedtime_workbench::PanelRecord>(entity)
                .unwrap()
                .unavailable = Some(error.clone());
            world
                .get_mut::<bedtime_workbench::PanelRecord>(panel)
                .unwrap()
                .unavailable = Some(error.clone());
            return Err(error);
        }
    }
    let binding = world.get::<PanelBinding>(entity).cloned();
    if let Some(binding) = binding {
        world.entity_mut(panel).insert(binding);
    }
    Ok(entity)
}

pub fn duplicate_panel(
    world: &mut World,
    source: Entity,
    area: Option<u32>,
) -> Result<Entity, String> {
    let panel = bedtime_api::workbench::panel_owner(world, source).ok_or("Panel is unavailable")?;
    let mut record = world
        .get::<bedtime_workbench::PanelRecord>(panel)
        .ok_or("Panel is unavailable")?
        .serialized
        .clone();
    record["state"] = bedtime_workbench::panel_state(world, panel)?;
    if world
        .get::<PanelBinding>(panel)
        .is_some_and(|binding| binding.terminal.is_some())
        && let Some(state) = record["state"].as_object_mut()
    {
        state.remove("session");
    }
    let copied = bedtime_workbench::open_panel(world, record, area, None)?;
    publish_documents(world);
    Ok(copied)
}

pub fn split_panel(
    world: &mut World,
    source: Entity,
    axis: usize,
    high: bool,
) -> Result<Entity, String> {
    if axis > 1 {
        return Err("Split axis must be horizontal or vertical".into());
    }
    let panel = bedtime_api::workbench::panel_owner(world, source).ok_or("Panel is unavailable")?;
    let id = world
        .get::<bedtime_workbench::PanelPresentation>(panel)
        .ok_or("Panel is unavailable")?
        .id;
    let area = world
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(id)
        .ok_or("Panel area is unavailable")?;
    let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
    let rect = tiling.layout.area(area).rect;
    let new_area = tiling
        .layout
        .split(area, axis, (rect.min[axis] + rect.max[axis]) / 2, 100)
        .ok_or("This area is too small to split")?;
    if !high {
        let low = tiling.layout.area(area).rect;
        let upper = tiling.layout.area(new_area).rect;
        for region in &mut tiling.layout.areas {
            if region.id == area {
                region.rect = upper;
            } else if region.id == new_area {
                region.rect = low;
            }
        }
    }
    tiling.add_area(new_area);
    let copied = duplicate_panel(world, panel, Some(area))?;
    let copied_id = world
        .get::<bedtime_workbench::PanelPresentation>(copied)
        .unwrap()
        .id;
    tiling.assign(copied_id, new_area);
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    bedtime_workbench::focus_panel(world, copied);
    Ok(copied)
}

/// Open a compatible viewer in its own tab, sharing canonical data and undo history.
/// Repeated requests focus the matching viewer in the requested tab group.
pub fn open_document_viewer(
    world: &mut World,
    document: DocumentId,
    viewer: &bedtime_api::ViewerId,
    area: Option<u32>,
) -> Result<Entity, String> {
    let descriptor = world
        .resource::<bedtime_api::features::FeatureRegistry>()
        .viewer(viewer)
        .cloned()
        .ok_or_else(|| format!("Viewer unavailable: {viewer}"))?;
    let kind = world
        .non_send::<DocumentSession>()
        .document_kind(document)
        .map_err(|error| error.to_string())?;
    if !descriptor.supports_document_kind(kind) {
        return Err("This viewer does not support the document's content kind".into());
    }
    commit_document_drafts(world, &[document])?;
    let existing = document_viewer_tab(world, document, viewer, area);
    if let Some(panel) = existing {
        bedtime_workbench::keep_panel(world, panel);
        bedtime_workbench::focus_panel(world, panel);
        return Ok(panel);
    }
    let layout = world.resource::<WorkbenchLayout>();
    let previous_focus = layout.focused;
    let previous_area = layout.focused_area;
    match open_document_panel_with_viewer(
        world,
        &descriptor.panel_type,
        document,
        Value::Null,
        area,
        Some(viewer),
    ) {
        Ok(panel) => Ok(panel),
        Err(error) => {
            if let Some(panel) = previous_focus {
                bedtime_workbench::focus_panel(world, panel);
            } else if let Some(area) = previous_area {
                bedtime_workbench::focus_area(world, area);
            }
            Err(error)
        }
    }
}

fn document_viewer_tab(
    world: &mut World,
    document: DocumentId,
    viewer: &bedtime_api::ViewerId,
    area: Option<u32>,
) -> Option<Entity> {
    bedtime_workbench::recent_panel_matching(world, |world, panel| {
        world.get::<PanelBinding>(panel).is_some_and(|binding| {
            binding.document == Some(document)
                && binding.viewer.as_ref() == Some(viewer)
                && area.is_none_or(|area| {
                    world.resource::<WorkbenchLayout>().tiling.area_for(
                        world
                            .get::<bedtime_workbench::PanelPresentation>(panel)
                            .unwrap()
                            .id,
                    ) == Some(area)
                })
        })
    })
}

pub fn open_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    area: Option<u32>,
) -> Result<Entity, String> {
    open_file_instance(world, path, viewer, false, area)
}

pub fn open_file_instance(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    additional: bool,
    area: Option<u32>,
) -> Result<Entity, String> {
    open_file_instance_intent(world, path, viewer, additional, area, FileOpenIntent::Keep)
}

#[derive(Clone, Copy)]
enum FileOpenIntent {
    Keep,
    Preview,
    Drop,
}

fn open_file_instance_intent(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    additional: bool,
    area: Option<u32>,
    intent: FileOpenIntent,
) -> Result<Entity, String> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "files open after drawing"
    );
    let explicit = viewer
        .map(|id| {
            world
                .resource::<bedtime_api::features::FeatureRegistry>()
                .viewer(&bedtime_api::ViewerId::from(id))
                .cloned()
                .ok_or_else(|| format!("File viewer unavailable: {id}"))
        })
        .transpose()?;
    if let Some(descriptor) = explicit
        .as_ref()
        .filter(|descriptor| descriptor.is_local_file())
    {
        return open_local_file_panel(world, path, descriptor, area, additional, intent);
    }
    let remote = world.non_send::<DocumentSession>().is_remote();
    let kind = if remote {
        let document = world
            .non_send_mut::<DocumentSession>()
            .open_file_auto(path)
            .map_err(|error| error.to_string())?;
        world
            .non_send::<DocumentSession>()
            .document_kind(document)
            .map_err(|error| error.to_string())?
    } else {
        bedtime_files::files::classify_file(path).map_err(|error| error.to_string())?
    };
    let registry = world.resource::<bedtime_api::features::FeatureRegistry>();
    let descriptor = explicit.or_else(|| {
        registry
            .preferred_viewer_for_file(
                &path.to_string_lossy(),
                kind,
                &world
                    .resource::<bedtime_api::settings::SettingsSnapshot>()
                    .default_viewers,
            )
            .cloned()
    });
    let descriptor = descriptor.ok_or_else(|| format!("No file viewer for {}", path.display()))?;
    if descriptor.is_local_file() {
        return open_local_file_panel(world, path, &descriptor, area, additional, intent);
    }
    if !descriptor.supports_document_kind(kind) {
        return Err("This viewer does not support the document's content kind".into());
    }
    let document = world
        .non_send_mut::<DocumentSession>()
        .open_file_auto(path)
        .map_err(|error| error.to_string())?;
    let area = area.or_else(|| {
        (kind == bedtime_documents::DocumentKind::Text)
            .then(|| preferred_text_area(world))
            .flatten()
    });
    // File opening reuses an existing matching tab anywhere in the active layout.
    // Drop requests can then move that tab to their captured destination.
    // Opening another viewer explicitly remains scoped to its source tab group.
    if additional {
        return open_document_panel_with_viewer(
            world,
            &descriptor.panel_type,
            document,
            Value::Null,
            area,
            Some(&descriptor.id),
        );
    }
    let existing = if matches!(intent, FileOpenIntent::Drop) {
        document_viewer_tab(world, document, &descriptor.id, area)
    } else {
        None
    }
    .or_else(|| document_viewer_tab(world, document, &descriptor.id, None));
    if let Some(panel) = existing {
        commit_document_drafts(world, &[document])?;
        if matches!(intent, FileOpenIntent::Keep | FileOpenIntent::Drop) {
            bedtime_workbench::keep_panel(world, panel);
        }
        focus_panel(world, panel, true)?;
        return Ok(panel);
    }
    let placement = area;
    if let Some(panel) = reusable_document_panel(world, placement)? {
        return replace_pristine_document_panel(
            world,
            panel,
            &descriptor.panel_type,
            document,
            &descriptor.id,
        );
    }
    let active = active_panel_entities(world);
    let mut split = None;
    if active.len() == 1
        && world
            .get::<PanelBinding>(active[0])
            .is_some_and(|binding| binding.terminal.is_some())
        && world.resource::<WorkbenchLayout>().tiling.areas.len() == 1
    {
        let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
        let area = placement.unwrap_or_else(|| tiling.layout.largest_area());
        let rect = tiling.layout.area(area).rect;
        let destination = tiling
            .layout
            .split(area, 0, (rect.min[0] + rect.max[0]) / 2, 100)
            .ok_or("This area is too small to split")?;
        tiling.add_area(destination);
        split = Some((tiling, destination));
    }
    let panel = open_document_viewer(world, document, &descriptor.id, placement)?;
    if let Some((mut tiling, destination)) = split {
        tiling.assign(
            world
                .get::<bedtime_workbench::PanelPresentation>(panel)
                .unwrap()
                .id,
            destination,
        );
        world.resource_mut::<WorkbenchLayout>().tiling = tiling;
        focus_panel(world, panel, true)?;
    }
    Ok(panel)
}
fn open_local_file_panel(
    world: &mut World,
    path: &Path,
    viewer: &bedtime_api::documents::FileViewer,
    area: Option<u32>,
    additional: bool,
    intent: FileOpenIntent,
) -> Result<Entity, String> {
    if world.resource::<bedtime_files::FileServices>().is_remote() {
        return Err("This file viewer requires a local filesystem".into());
    }
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !path.is_file() {
        return Err("File viewer requires a regular file".into());
    }
    let existing = (!additional)
        .then(|| {
            let matches = |world: &World, panel: Entity| {
                world.get::<PanelBinding>(panel).is_some_and(|binding| {
                    binding.input.local_file() == Some(path.as_path())
                        && binding.viewer.as_ref() == Some(&viewer.id)
                })
            };
            let target = if matches!(intent, FileOpenIntent::Drop) {
                bedtime_workbench::recent_panel_matching(world, |world, panel| {
                    matches(world, panel)
                        && area.is_none_or(|area| {
                            world.resource::<WorkbenchLayout>().tiling.area_for(
                                world
                                    .get::<bedtime_workbench::PanelPresentation>(panel)
                                    .unwrap()
                                    .id,
                            ) == Some(area)
                        })
                })
            } else {
                None
            };
            target.or_else(|| bedtime_workbench::recent_panel_matching(world, matches))
        })
        .flatten();
    if let Some(panel) = existing {
        if matches!(intent, FileOpenIntent::Keep | FileOpenIntent::Drop) {
            bedtime_workbench::keep_panel(world, panel);
        }
        focus_panel(world, panel, true)?;
        return Ok(panel);
    }
    let entity = bedtime_workbench::open_panel(
        world,
        json!({"panel_type":viewer.panel_type,"state":null,"path":path.to_string_lossy(),"backing":"local_file","viewer":viewer.id}),
        area,
        None,
    )?;
    if let Some(error) = world
        .get::<bedtime_workbench::PanelRecord>(entity)
        .and_then(|record| record.unavailable.clone())
    {
        bedtime_workbench::commit_close(world, entity);
        return Err(error);
    }
    if let Some(mut binding) = world.get_mut::<PanelBinding>(entity) {
        binding.viewer = Some(viewer.id.clone());
    }
    if let Some(frontend) = panel_presentation(world, entity)
        && let Some(mut binding) = world.get_mut::<PanelBinding>(frontend)
    {
        binding.viewer = Some(viewer.id.clone());
    }
    Ok(entity)
}

fn preview_file(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    area: u32,
) -> Result<Entity, String> {
    let previous = world
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .and_then(|group| group.preview);
    if let Some(document) = previous.and_then(|id| {
        world
            .query::<(&bedtime_workbench::PanelPresentation, &PanelBinding)>()
            .iter(world)
            .find_map(|(panel, binding)| (panel.id == id).then_some(binding.document).flatten())
    }) && let Err(error) = commit_document_drafts(world, &[document])
    {
        if let Some(panel) = panel_entities(world).into_iter().find(|panel| {
            world
                .get::<bedtime_workbench::PanelPresentation>(*panel)
                .unwrap()
                .id
                == previous.unwrap()
        }) {
            bedtime_workbench::keep_panel(world, panel);
        }
        return Err(error);
    }
    let existing = world
        .query::<(Entity, &bedtime_api::workbench::PanelPresentation)>()
        .iter(world)
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let panel = open_file_instance_intent(
        world,
        path,
        viewer,
        false,
        Some(area),
        FileOpenIntent::Preview,
    )?;
    if existing.contains(&panel) {
        return Ok(panel);
    }
    if let Some(previous) = previous.and_then(|id| {
        world
            .query::<(Entity, &bedtime_api::workbench::PanelPresentation)>()
            .iter(world)
            .find_map(|(entity, panel)| (panel.id == id).then_some(entity))
    }) {
        let document = world
            .get::<PanelBinding>(previous)
            .and_then(|binding| binding.document);
        let dirty = document
            .map(|document| document_dirty(world, document))
            .transpose()?
            .unwrap_or_else(|| {
                world
                    .get::<bedtime_workbench::PanelDirty>(previous)
                    .is_some_and(|dirty| dirty.0)
            });
        if dirty {
            bedtime_workbench::keep_panel(world, previous);
        } else {
            bedtime_workbench::request_close(world, previous)?;
        }
    }
    bedtime_workbench::mark_panel_preview(world, panel)?;
    Ok(panel)
}

pub fn commit_document_drafts(world: &mut World, documents: &[DocumentId]) -> Result<(), String> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "drafts commit after drawing"
    );
    let targets = world
        .query::<(
            Entity,
            Option<&PanelBinding>,
            Option<&models::DocumentBinding>,
            &bedtime_api::workbench::PanelActions,
        )>()
        .iter(world)
        .filter(|(_, text, model, _)| {
            text.and_then(|binding| binding.document)
                .or(model.map(|binding| binding.0))
                .is_some_and(|document| documents.contains(&document))
        })
        .map(|(entity, _, _, handler)| (entity, handler.0))
        .collect::<Vec<_>>();
    for (entity, handler) in targets {
        let previous = world.resource_mut::<HostEffects>().take();
        let result = world
            .run_system_with(
                handler,
                bedtime_api::workbench::PanelActionInvocation {
                    entity,
                    action: bedtime_api::workbench::PanelAction::CommitEdit,
                },
            )
            .map_err(|error| error.to_string())
            .and_then(|result| result.map(|_| ()));
        let effects = world.resource_mut::<HostEffects>().take();
        for intent in previous {
            world.resource_mut::<HostEffects>().push_effect(intent);
        }
        result?;
        for intent in effects {
            let target = match &intent.action {
                HostAction::ApplyEdits { document, .. }
                | HostAction::ApplyEditsWithResult { document, .. } => Some(*document),
                _ => None,
            };
            if intent.source != Some(entity)
                || target
                    != world
                        .get::<PanelBinding>(entity)
                        .and_then(|binding| binding.document)
                        .or_else(|| {
                            world
                                .get::<models::DocumentBinding>(entity)
                                .map(|binding| binding.0)
                        })
            {
                return Err("Draft commit may only produce its own document edits".into());
            }
            let request = capture_host_request(world, Some(entity), intent.action);
            apply_document_request(world, request)?;
        }
    }
    Ok(())
}

/// Begin an unformatted lifecycle save. Poll the returned receipt for the
/// revision that reached storage; a later edit may keep the document dirty.
pub fn save_document(
    world: &mut World,
    document: DocumentId,
) -> Result<crate::DocumentSaveId, String> {
    crate::begin_document_save(
        world,
        &[crate::DocumentSaveTarget {
            document,
            destination: None,
        }],
    )
}
pub fn save_document_as(
    world: &mut World,
    document: DocumentId,
    path: &Path,
) -> Result<crate::DocumentSaveId, String> {
    crate::begin_document_save(
        world,
        &[crate::DocumentSaveTarget {
            document,
            destination: Some(path.to_owned()),
        }],
    )
}

/// One revision representation for default documents and typed model documents.
pub fn document_revision(world: &World, document: DocumentId) -> Result<Revision, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok((0, model.revision()));
    }
    world
        .non_send::<DocumentSession>()
        .document_revision(document)
        .map_err(|error| error.to_string())
}
pub fn document_dirty(world: &World, document: DocumentId) -> Result<bool, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok(model.dirty());
    }
    world
        .non_send::<DocumentSession>()
        .with_document(document, |state| state.dirty)
        .map_err(|error| error.to_string())
}
pub fn document_path(
    world: &World,
    document: DocumentId,
) -> Result<Option<std::path::PathBuf>, String> {
    if let Some(model) = models::model_document(world, document) {
        if model.session_id() != world.non_send::<DocumentSession>().session_id() {
            return Err("Document session was replaced".into());
        }
        return Ok(model.path.clone());
    }
    let path = world
        .non_send::<DocumentSession>()
        .with_document(document, |state| state.path.clone())
        .map_err(|error| error.to_string())?;
    Ok((!path.is_empty()).then(|| path.into()))
}

/// Retire contents only after the final panel binding has been released.
/// Save/discard approval belongs to the operation that retired those panels.
pub fn retire_document(world: &mut World, document: DocumentId) -> Result<bool, String> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "documents retire after drawing"
    );
    let bound = world
        .query::<(Option<&PanelBinding>, Option<&models::DocumentBinding>)>()
        .iter(world)
        .any(|(binding, model)| {
            binding.is_some_and(|binding| binding.document == Some(document))
                || model.is_some_and(|binding| binding.0 == document)
        });
    if bound {
        return Ok(false);
    }
    if models::model_entity(world, document).is_some() {
        return Ok(models::retire_model_document(world, document));
    }
    world
        .non_send_mut::<DocumentSession>()
        .close_document(document, ClosePolicy::Discard)
        .map_err(|error| error.to_string())
}

fn close_panel(world: &mut World, panel: Entity) -> Result<bool, String> {
    let closing =
        bedtime_workbench::closing_panels(world).map_or_else(|| vec![panel], <[Entity]>::to_vec);
    // This policy checks the complete group. Workbench calls it once per
    // member, so only its first member owns the document decision and save.
    if closing.first().is_some_and(|first| *first != panel) {
        return Ok(true);
    }
    let closing_frontends = world
        .query::<(Entity, Option<&bedtime_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| {
            closing.contains(entity) || owner.is_some_and(|owner| closing.contains(&owner.0))
        })
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let mut documents = closing_frontends
        .iter()
        .filter_map(|entity| {
            world
                .get::<PanelBinding>(*entity)
                .and_then(|binding| binding.document)
                .or_else(|| {
                    world
                        .get::<models::DocumentBinding>(*entity)
                        .map(|binding| binding.0)
                })
        })
        .collect::<Vec<_>>();
    documents.sort_by_key(|document| document.0);
    documents.dedup();
    commit_document_drafts(world, &documents)?;
    if let Some(save) = world.resource::<LifecycleSaves>().close {
        let Some(result) = crate::poll_document_save(world, save) else {
            return Ok(false);
        };
        world.resource_mut::<LifecycleSaves>().close = None;
        for saved in result? {
            if document_revision(world, saved.document)? != saved.revision {
                return Err("Document changed while saving; prepare the operation again".into());
            }
        }
    }
    let mut dirty_documents = Vec::new();
    for document in documents {
        let remaining = world
            .query::<(
                Entity,
                Option<&PanelBinding>,
                Option<&models::DocumentBinding>,
            )>()
            .iter(world)
            .any(|(entity, binding, model)| {
                !closing_frontends.contains(&entity)
                    && (binding.is_some_and(|binding| binding.document == Some(document))
                        || model.is_some_and(|binding| binding.0 == document))
            });
        if remaining {
            continue;
        }
        let dirty = document_dirty(world, document)?;
        if dirty {
            dirty_documents.push(document);
        }
    }
    if !dirty_documents.is_empty() {
        let Some(policy) = lifecycle_policy(world, PromptOwner::Panels(closing), &dirty_documents)?
        else {
            return Ok(false);
        };
        match policy {
            ClosePolicy::Cancel => return Err("Save or discard the document before closing".into()),
            ClosePolicy::Discard => return Ok(true),
            ClosePolicy::Save => {}
        }
        let targets = lifecycle_save_targets(world, &dirty_documents)?;
        let save = crate::begin_document_save(world, &targets)?;
        if let Some(result) = crate::poll_document_save(world, save) {
            result?;
        } else {
            world.resource_mut::<LifecycleSaves>().close = Some(save);
            return Ok(false);
        }
    }
    Ok(true)
}

fn retire_panel(world: &mut World, panel: Entity) {
    let frontends = world
        .query::<(Entity, Option<&bedtime_api::workbench::PanelOwner>)>()
        .iter(world)
        .filter(|(entity, owner)| *entity == panel || owner.is_some_and(|owner| owner.0 == panel))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();
    let documents = frontends
        .iter()
        .filter_map(|entity| {
            world
                .get::<PanelBinding>(*entity)
                .and_then(|binding| binding.document)
                .or_else(|| {
                    world
                        .get::<models::DocumentBinding>(*entity)
                        .map(|binding| binding.0)
                })
        })
        .collect::<HashSet<_>>();
    for entity in frontends {
        if let Some(handler) = world
            .get::<bedtime_api::workbench::PanelClose>(entity)
            .copied()
            && let Err(error) = world
                .run_system_with(handler.0, entity)
                .map_err(|error| error.to_string())
                .and_then(|result| result)
        {
            notice(world, Some(entity), error);
        }
        if let Some(binding) = world.entity_mut(entity).take::<PanelBinding>()
            && let Some(view) = binding.view
            && world
                .non_send::<DocumentSession>()
                .document_for_view(view)
                .is_some()
        {
            world.non_send_mut::<DocumentSession>().detach_view(view);
        }
        world.entity_mut(entity).remove::<models::DocumentBinding>();
    }
    // An approved final-view discard must remove its unsaved canonical state.
    // Surviving views keep it alive; saved unbound documents retain their history.
    for document in documents {
        if document_dirty(world, document).unwrap_or(false)
            && let Err(error) = retire_document(world, document)
        {
            notice(world, Some(panel), error);
        }
    }
}

/// Apply one revision-checked text or byte edit batch. The shared document
/// owner preserves I/O error kinds for integrations with typed failure policy.
pub fn apply_edits(
    world: &mut World,
    document: DocumentId,
    revision: Revision,
    edits: &[bedtime_documents::ByteEdit],
) -> std::io::Result<()> {
    assert!(
        !bedtime_workbench::frontend_active(world),
        "document edits run after drawing"
    );
    world
        .non_send_mut::<DocumentSession>()
        .apply_edits(document, revision, edits)
}

struct HostStep {
    outcome: HostOutcome,
    effects: VecDeque<HostRequest>,
    workbench: Option<bedtime_workbench::WorkbenchRequestId>,
}

fn apply_host_step(world: &mut World, request: HostRequest) -> Result<HostStep, String> {
    let barrier = match request.action() {
        HostAction::OperationBarrier { token } => Some(*token),
        _ => None,
    };
    let previous = world.resource_mut::<HostEffects>().take();
    let mut workbench = None;
    let result = apply_document_request_inner(world, request.clone(), &mut workbench);
    let produced = world.resource_mut::<HostEffects>().take();
    for effect in previous {
        world.resource_mut::<HostEffects>().push_effect(effect);
    }
    if let (Some(token), Err(error)) = (barrier, &result) {
        crate::transitions::reject_operation_barrier(world, token, error.clone());
    }
    let outcome = result?;
    let effects = produced
        .into_iter()
        .map(|effect| {
            HostRequest::capture(
                request.session_id(),
                effect.source,
                effect.action,
                effect.area.unwrap_or(request.area()),
                request.layout_epoch(),
            )
            .with_explicit_area(effect.area.is_some() || request.has_explicit_area())
            .with_tab_index(effect.tab_index.or(request.tab_index()))
        })
        .collect();
    Ok(HostStep {
        outcome,
        effects,
        workbench,
    })
}

/// Apply an unreceipted request at the shared transition point. Use
/// submit_host_request when the caller needs the complete asynchronous result.
pub fn apply_document_request(
    world: &mut World,
    request: HostRequest,
) -> Result<HostOutcome, String> {
    let step = apply_host_step(world, request)?;
    let outcome = step.outcome;
    if !step.effects.is_empty() || step.workbench.is_some() {
        continue_command(
            world,
            PendingCommand {
                id: None,
                outcome: None,
                effects: step.effects,
                saves: Vec::new(),
                barrier: None,
                error: None,
                tab_index: None,
                host_id: None,
                last_outcome: Some(outcome.clone()),
                workbench: step.workbench,
                nested_outcome: None,
            },
        );
    }
    Ok(outcome)
}

fn validate_request_origin(world: &World, request: &HostRequest) -> Result<(), String> {
    // Redraw invalidates the application frame, without accessing its source.
    // A transition can replace its session or close its source before admission.
    if matches!(request.action(), HostAction::Invalidate) {
        return Ok(());
    }
    if request.session_id() != world.non_send::<DocumentSession>().session_id() {
        return Err("Document session was replaced".into());
    }
    if let Some(source) = request.source() {
        let owner = bedtime_api::workbench::panel_owner(world, source)
            .ok_or("Originating panel is unavailable")?;
        if [source, owner].into_iter().any(|entity| {
            world
                .get::<bedtime_workbench::PanelRecord>(entity)
                .is_some_and(|record| record.unavailable.is_some())
        }) {
            return Err("Originating panel failed".into());
        }
    }
    if request.is_layout_scoped()
        && request
            .source()
            .is_some_and(|source| !bedtime_api::workbench::panel_active(world, source))
    {
        return Err("Originating panel's layout is inactive".into());
    }
    if request.is_layout_scoped()
        && request.layout_epoch()
            != world
                .resource::<bedtime_workbench::WorkbenchLayoutEpoch>()
                .0
    {
        return Err("Originating layout is no longer active".into());
    }
    Ok(())
}

fn apply_document_request_inner(
    world: &mut World,
    request: HostRequest,
    workbench: &mut Option<bedtime_workbench::WorkbenchRequestId>,
) -> Result<HostOutcome, String> {
    // Layout intentions describe the old presentation, and expire normally
    // when its document session is retired. Explicit edits/saves still fail
    // validation so their callers receive an actionable rejection.
    if request.is_layout_scoped()
        && request.session_id() != world.non_send::<DocumentSession>().session_id()
    {
        return Ok(HostOutcome::Ignored);
    }
    validate_request_origin(world, &request)?;
    #[cfg(feature = "language-services")]
    if let HostAction::NavigateLocation {
        origin: Some(origin),
        ..
    } = request.action()
        && !world
            .non_send::<DocumentSession>()
            .accepts_lsp_origin(origin)
    {
        return Ok(HostOutcome::Applied);
    }
    if let Some(token) = crate::files::defer_remote_open(world, &request)? {
        return Ok(HostOutcome::Transition(token));
    }
    match request.action().clone() {
        HostAction::DuplicatePanel { panel } => {
            if !bedtime_api::workbench::panel_active(world, panel) {
                return Ok(HostOutcome::Ignored);
            }
            return duplicate_panel(world, panel, Some(request.area())).map(HostOutcome::Opened);
        }
        HostAction::NewDocument => {
            let viewer = world
                .resource::<bedtime_api::features::FeatureRegistry>()
                .default_viewer(bedtime_api::documents::DocumentKind::Text)
                .ok_or("No text editor is installed")?
                .clone();
            let document = world
                .non_send_mut::<DocumentSession>()
                .create_document(&[])
                .map_err(|error| error.to_string())?;
            let panel = open_document_panel_with_viewer(
                world,
                &viewer.panel_type,
                document,
                Value::Null,
                Some(request.area()),
                Some(&viewer.id),
            )?;
            let revision = world
                .non_send::<DocumentSession>()
                .document_revision(document)
                .map_err(|error| error.to_string())?;
            if let Some(frontend) = panel_presentation(world, panel) {
                world
                    .entity_mut(frontend)
                    .insert(bedtime_api::documents::PanelDefaultInput { document, revision });
            }
            return Ok(HostOutcome::Opened(panel));
        }
        HostAction::OpenFileInstance {
            path,
            viewer,
            additional,
        } => {
            return open_file_instance(
                world,
                Path::new(&path),
                viewer.as_ref(),
                additional,
                Some(request.area()),
            )
            .map(HostOutcome::Opened);
        }
        HostAction::SaveDocuments { documents } => {
            let targets = documents
                .into_iter()
                .map(|document| crate::DocumentSaveTarget {
                    document,
                    destination: None,
                })
                .collect::<Vec<_>>();
            let save = crate::begin_document_save(world, &targets)?;
            return document_save_outcome(world, save);
        }
        HostAction::SaveAs { document, path } => {
            let save = crate::begin_document_save(
                world,
                &[crate::DocumentSaveTarget {
                    document,
                    destination: Some(path),
                }],
            )?;
            return document_save_outcome(world, save);
        }
        HostAction::Command { command, context } => {
            let arguments = command_arguments(&context);
            let invocation = bedtime_api::workbench::CommandInvocation {
                command,
                source: request.source(),
                area: request.area(),
                arguments,
            };
            *workbench = Some(bedtime_workbench::resolve_command(
                world,
                invocation,
                bedtime_workbench::WorkbenchLayoutEpoch(request.layout_epoch()),
            ));
        }
        HostAction::ShowTerminal { id } => {
            let existing = world
                .query::<(Entity, &PanelBinding, &bedtime_workbench::PanelPresentation)>()
                .iter(world)
                .find_map(|(panel, binding, _)| {
                    (binding.terminal == Some(id)
                        && bedtime_api::workbench::panel_active(world, panel))
                    .then_some(panel)
                });
            let panel = if let Some(panel) = existing {
                focus_panel(world, panel, true)?;
                panel
            } else {
                open_panel(
                    world,
                    &bedtime_api::PanelTypeId::new("bedtime.terminal.panel"),
                    PanelInput::None,
                    json!({"session":id}),
                    None,
                    Some(request.area()),
                    false,
                )?
            };
            return Ok(HostOutcome::Opened(panel));
        }
        HostAction::OperationBarrier { token } => {
            crate::transitions::activate_operation_barrier(world, token)?
        }
        HostAction::OpenDirectory { location } => {
            return crate::files::open_directory(world, location).map(HostOutcome::Transition);
        }
        HostAction::ChooseDirectory => {
            if world.resource::<bedtime_files::FileServices>().is_remote() {
                return Err("Choose a remote directory through its filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(&world.resource::<bedtime_files::FileContext>().directory)
                .pick_folder()
            {
                return crate::files::open_directory(
                    world,
                    bedtime_files::FileLocation {
                        provider: bedtime_files::FileProviderId::Local,
                        path,
                    },
                )
                .map(HostOutcome::Transition);
            }
        }
        HostAction::ApplyEdits {
            document,
            revision,
            edits,
        } => apply_edits(world, document, revision, &edits).map_err(|error| error.to_string())?,
        HostAction::ApplyEditsWithResult {
            token,
            document,
            revision,
            edits,
        } => {
            let result = apply_edits(world, document, revision, &edits)
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    world
                        .non_send::<DocumentSession>()
                        .document_revision(document)
                        .map_err(|error| error.to_string())
                });
            if let Some(source) = request.source().and_then(|source| {
                world
                    .get::<bedtime_api::documents::PanelEdits>(source)
                    .map(|handler| (source, handler.0))
            }) {
                world
                    .run_system_with(
                        source.1,
                        bedtime_api::documents::PanelEditResult {
                            entity: source.0,
                            token,
                            result: result.clone(),
                        },
                    )
                    .map_err(|error| error.to_string())?;
            }
            result?;
        }
        HostAction::ApplyModuleEditsWithResult {
            recipient,
            token,
            document,
            revision,
            edits,
        } => {
            let result = commit_document_drafts(world, &[document])
                .and_then(|()| {
                    apply_edits(world, document, revision, &edits)
                        .map_err(|error| error.to_string())
                })
                .and_then(|_| {
                    world
                        .non_send::<DocumentSession>()
                        .document_revision(document)
                        .map_err(|error| error.to_string())
                });
            world.write_message(bedtime_api::documents::DocumentReply::Edit {
                recipient,
                token,
                result: result.clone(),
            });
            result?;
        }
        HostAction::Undo { document } => models::undo_document(world, document)?,
        HostAction::Redo { document } => models::redo_document(world, document)?,
        HostAction::Save { document } => {
            #[cfg(feature = "language-services")]
            let save = crate::begin_explicit_document_save(world, document)?;
            #[cfg(not(feature = "language-services"))]
            let save = crate::begin_document_save(
                world,
                &[crate::DocumentSaveTarget {
                    document,
                    destination: None,
                }],
            )?;
            return document_save_outcome(world, save);
        }
        HostAction::SaveAsDialog { document } => {
            if world.resource::<bedtime_files::FileServices>().is_remote() {
                return Err("Choose an SSH save destination through its filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new().save_file() {
                let save = crate::begin_document_save(
                    world,
                    &[crate::DocumentSaveTarget {
                        document,
                        destination: Some(path),
                    }],
                )?;
                return document_save_outcome(world, save);
            }
        }
        HostAction::SaveDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let targets = documents
                .into_iter()
                .map(|document| crate::DocumentSaveTarget {
                    document,
                    destination: None,
                })
                .collect::<Vec<_>>();
            match crate::begin_document_save(world, &targets) {
                Ok(save) => {
                    crate::saves::attach_document_save_reply(world, save, recipient, token);
                    return document_save_outcome(world, save);
                }
                Err(error) => {
                    world.write_message(bedtime_api::documents::DocumentReply::Save {
                        recipient,
                        token,
                        result: Err(error.clone()),
                    });
                    return Err(error);
                }
            }
        }
        HostAction::CommitDocumentsWithResult {
            recipient,
            token,
            documents,
        } => {
            let result = (|| {
                commit_document_drafts(world, &documents)?;
                let mut saved = Vec::new();
                for document in documents {
                    if world.non_send::<DocumentSession>().save_pending(document) {
                        return Err("Document has a pending write".into());
                    }
                    saved.push(bedtime_api::documents::SavedDocument {
                        document,
                        revision: document_revision(world, document)?,
                    });
                }
                Ok::<_, String>(saved)
            })();
            world.write_message(bedtime_api::documents::DocumentReply::Save {
                recipient,
                token,
                result: result.clone(),
            });
            result?;
        }
        HostAction::OpenFile { path, viewer } | HostAction::OpenFileIn { path, viewer, .. } => {
            let area = if request.has_explicit_area()
                || matches!(
                    request.action(),
                    HostAction::OpenFileIn {
                        target: bedtime_api::workbench::PanelTarget::LastFocused,
                        ..
                    }
                ) {
                request.area()
            } else {
                default_file_area(world, Path::new(&path), viewer.as_ref(), request.area())?
            };
            return open_file(world, Path::new(&path), viewer.as_ref(), Some(area))
                .map(HostOutcome::Opened);
        }
        HostAction::PreviewFileIn {
            path,
            viewer,
            target,
        } => {
            let area = if request.has_explicit_area()
                || target == bedtime_api::workbench::PanelTarget::LastFocused
            {
                request.area()
            } else {
                default_file_area(world, Path::new(&path), viewer.as_ref(), request.area())?
            };
            return preview_file(world, Path::new(&path), viewer.as_ref(), area)
                .map(HostOutcome::Opened);
        }
        HostAction::OpenTerminalFiles { paths } => {
            for path in paths {
                world.resource_mut::<HostEffects>().push_at(
                    request.source(),
                    request.area(),
                    HostAction::OpenTerminalLink {
                        path,
                        position: None,
                    },
                );
            }
        }
        HostAction::OpenTerminalLink { path, position } => {
            let area = terminal_open_area(world, request.area());
            if let Some((row, column)) = position {
                return reveal_source(
                    world,
                    &path,
                    SourceJump {
                        position: (row, column),
                        center: true,
                        preview: false,
                        area,
                        utf16: false,
                        source: None,
                    },
                )
                .map(HostOutcome::Opened);
            }
            return open_file(world, &path, None, Some(area)).map(HostOutcome::Opened);
        }
        HostAction::RevealSource {
            path,
            row,
            column,
            center,
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                SourceJump {
                    position: (row, column),
                    center,
                    preview: false,
                    area: request.area(),
                    utf16: false,
                    source: None,
                },
            )
            .map(HostOutcome::Opened);
        }
        HostAction::PreviewSource {
            path,
            row,
            column,
            center,
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                SourceJump {
                    position: (row, column),
                    center,
                    preview: true,
                    area: request.area(),
                    utf16: false,
                    source: None,
                },
            )
            .map(HostOutcome::Opened);
        }
        #[cfg(feature = "language-services")]
        HostAction::NavigateLocation {
            path,
            row,
            utf16_column,
            ..
        } => {
            return reveal_source(
                world,
                Path::new(&path),
                SourceJump {
                    position: (row, utf16_column),
                    center: true,
                    preview: true,
                    area: request.area(),
                    utf16: true,
                    source: request.source(),
                },
            )
            .map(HostOutcome::Opened);
        }
        HostAction::OpenFileDialog { viewer } => {
            if world.resource::<bedtime_files::FileServices>().is_remote() {
                return Err("Open SSH files through their filesystem browser".into());
            }
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                let area = if request.has_explicit_area() {
                    request.area()
                } else {
                    default_file_area(world, &path, viewer.as_ref(), request.area())?
                };
                return open_file(world, &path, viewer.as_ref(), Some(area))
                    .map(HostOutcome::Opened);
            }
        }
        HostAction::DropFile { path } => {
            let panel = open_file_instance_intent(
                world,
                Path::new(&path),
                None,
                false,
                Some(request.area()),
                FileOpenIntent::Drop,
            )?;
            bedtime_workbench::move_panel_to_area(world, panel, request.area())?;
            return Ok(HostOutcome::Opened(panel));
        }
        HostAction::OpenPanel {
            panel_type,
            document,
            state,
        } => {
            let entity = match document {
                Some(document) => {
                    open_document_panel(world, &panel_type, document, state, Some(request.area()))?
                }
                None => bedtime_workbench::open_panel(
                    world,
                    json!({"panel_type":panel_type,"state":state}),
                    Some(request.area()),
                    None,
                )?,
            };
            return Ok(HostOutcome::Opened(entity));
        }
        HostAction::ShowPanel {
            panel_type,
            document,
            state,
            action,
        }
        | HostAction::ShowPanelIn {
            panel_type,
            document,
            state,
            action,
            ..
        } => {
            let existing = bedtime_workbench::recent_panel_matching(world, |world, entity| {
                world
                    .get::<bedtime_workbench::PanelRecord>(entity)
                    .is_some_and(|record| record.panel_type == panel_type)
                    && document
                        == world
                            .get::<PanelBinding>(entity)
                            .and_then(|binding| binding.document)
            });
            let entity = if let Some(entity) = existing {
                if matches!(
                    request.action(),
                    HostAction::ShowPanelIn {
                        target: bedtime_api::workbench::PanelTarget::LargestArea,
                        ..
                    }
                ) {
                    bedtime_workbench::move_panel_to_area(world, entity, request.area())?;
                }
                entity
            } else {
                match document {
                    Some(document) => open_document_panel(
                        world,
                        &panel_type,
                        document,
                        state,
                        Some(request.area()),
                    )?,
                    None => bedtime_workbench::open_panel(
                        world,
                        json!({"panel_type":panel_type,"state":state}),
                        Some(request.area()),
                        None,
                    )?,
                }
            };
            bedtime_workbench::focus_panel(world, entity);
            if let Some(action) = action {
                let (accepted, effects) = crate::dispatch_panel_action(world, entity, action)?;
                if !accepted {
                    return Err("Panel does not support this action".into());
                }
                for effect in effects {
                    world.resource_mut::<HostEffects>().push_effect(effect);
                }
            }
            return Ok(HostOutcome::Opened(entity));
        }
        HostAction::ClosePanels { panel_type } => {
            let panels = world
                .query::<(
                    Entity,
                    &bedtime_workbench::PanelRecord,
                    &bedtime_api::workbench::PanelPresentation,
                )>()
                .iter(world)
                .filter_map(|(entity, record, _)| {
                    (record.panel_type == panel_type
                        && bedtime_api::workbench::panel_active(world, entity))
                    .then_some(entity)
                })
                .collect::<Vec<_>>();
            if !bedtime_workbench::request_close_many(world, &panels)? {
                return Err("Panel close is awaiting application approval".into());
            }
        }
        HostAction::FocusView { view } => {
            let entity = world
                .query::<(Entity, &PanelBinding)>()
                .iter(world)
                .find_map(|(entity, binding)| (binding.view == Some(view)).then_some(entity))
                .ok_or("Editor view is unavailable")?;
            bedtime_workbench::focus_panel(world, entity);
        }
        HostAction::Navigate {
            document,
            byte_offset,
            revision,
        } => {
            let active_views = world
                .query::<(Entity, &PanelBinding)>()
                .iter(world)
                .filter_map(|(panel, binding)| {
                    (binding.document == Some(document)
                        && bedtime_api::workbench::panel_active(world, panel))
                    .then_some(binding.view)
                    .flatten()
                })
                .collect::<HashSet<_>>();
            if active_views.is_empty() {
                return Ok(HostOutcome::Ignored);
            }
            let session = world.non_send::<DocumentSession>();
            if session
                .document_revision(document)
                .map_err(|error| error.to_string())?
                != revision
            {
                return Err("Document changed before navigation".into());
            }
            let view = request
                .source()
                .and_then(|source| world.get::<PanelBinding>(source))
                .filter(|binding| binding.document == Some(document))
                .and_then(|binding| binding.view)
                .filter(|view| active_views.contains(view))
                .or_else(|| {
                    world
                        .resource::<DocumentFrame>()
                        .active_view
                        .filter(|view| active_views.contains(view))
                })
                .or_else(|| {
                    session
                        .view_ids(document)
                        .into_iter()
                        .find(|view| active_views.contains(view))
                })
                .ok_or("Document has no text view")?;
            let (row, col) = session
                .with_document(document, |state| state.row_col_from_offset(byte_offset))
                .map_err(|error| error.to_string())?;
            world
                .non_send_mut::<DocumentSession>()
                .with_commands(view, |commands| {
                    commands.set_cursor(
                        row,
                        col,
                        false,
                        bedtime_editing::text_commands::CursorReveal::Center,
                    )
                })
                .map_err(|error| error.to_string())?;
        }
        HostAction::Clipboard { panel, action } => {
            let effects = crate::dispatch_panel_clipboard(world, panel, action)?;
            for effect in effects {
                world.resource_mut::<HostEffects>().push_effect(effect);
            }
        }
        HostAction::PanelAction { panel, action } => {
            let document = world
                .get::<PanelBinding>(panel)
                .and_then(|binding| binding.document)
                .or_else(|| {
                    bedtime_api::workbench::panel_owner(world, panel)
                        .and_then(|panel| world.get::<PanelBinding>(panel))
                        .and_then(|binding| binding.document)
                });
            if matches!(
                action,
                bedtime_api::workbench::PanelAction::Undo
                    | bedtime_api::workbench::PanelAction::Redo
                    | bedtime_api::workbench::PanelAction::CommitEdit
            ) && let Some(document) = document
            {
                commit_document_drafts(world, &[document])?;
            }
            let (_, effects) = crate::dispatch_panel_action(world, panel, action)?;
            for effect in effects {
                world.resource_mut::<HostEffects>().push_effect(effect);
            }
        }
        HostAction::ExternalFiles { panel, event } => {
            let (response, effects) = crate::dispatch_external_files(world, panel, event.clone())?;
            let mut effects = effects;
            if response != bedtime_api::workbench::ExternalFileDropResponse::Accepted
                && event.phase == bedtime_api::workbench::ExternalFileDragPhase::Drop
            {
                let area = bedtime_workbench::area_at(world, event.position, event.viewport)
                    .unwrap_or(request.area());
                effects.extend(event.paths.into_iter().map(|path| {
                    bedtime_api::features::HostEffect {
                        source: request.source(),
                        area: Some(area),
                        tab_index: None,
                        action: if path.is_dir() {
                            HostAction::OpenDirectory {
                                location: bedtime_files::FileLocation {
                                    provider: bedtime_files::FileProviderId::Local,
                                    path,
                                },
                            }
                        } else {
                            HostAction::DropFile {
                                path: path.to_string_lossy().into_owned(),
                            }
                        },
                    }
                }));
            }
            for effect in effects {
                world.resource_mut::<HostEffects>().push_effect(effect);
            }
        }
        HostAction::OpenDocumentViewer {
            tab,
            document,
            viewer,
        } => {
            let source = world
                .query::<(Entity, &PanelBinding, &bedtime_workbench::PanelPresentation)>()
                .iter(world)
                .find_map(|(panel, binding, presentation)| {
                    (presentation.id == tab && binding.document == Some(document)).then_some(panel)
                })
                .ok_or("Originating document tab is unavailable")?;
            if !bedtime_api::workbench::panel_active(world, source) {
                return Ok(HostOutcome::Ignored);
            }
            let area = world
                .resource::<bedtime_workbench::WorkbenchLayout>()
                .tiling
                .area_for(
                    world
                        .get::<bedtime_workbench::PanelPresentation>(source)
                        .unwrap()
                        .id,
                );
            let opened = open_document_viewer(world, document, &viewer, area)?;
            bedtime_workbench::keep_panel(world, source);
            return Ok(HostOutcome::Opened(opened));
        }
        HostAction::Notify { message } => notice(world, request.source(), message),
        HostAction::Invalidate => {}
        // Another dependency can enable optional API variants through Cargo
        // feature unification while this core was built without their owner.
        #[cfg(not(feature = "language-services"))]
        #[allow(unreachable_patterns)]
        _ => return Err("This operation requires language services".into()),
    }
    Ok(HostOutcome::Applied)
}

fn preferred_text_area(world: &mut World) -> Option<u32> {
    let panels = world
        .query::<(&bedtime_workbench::PanelPresentation, &PanelBinding)>()
        .iter(world)
        .filter_map(|(panel, binding)| binding.document.map(|document| (panel.id, document)))
        .filter(|(_, document)| {
            world
                .non_send::<DocumentSession>()
                .document_kind(*document)
                .ok()
                == Some(bedtime_documents::DocumentKind::Text)
        })
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>();
    layout
        .tiling
        .layout
        .areas
        .iter()
        .filter(|area| {
            panels
                .iter()
                .any(|id| layout.tiling.area_for(*id) == Some(area.id))
        })
        .max_by_key(|area| {
            (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
        })
        .map(|area| area.id)
}

fn default_file_area(
    world: &mut World,
    path: &Path,
    viewer: Option<&bedtime_api::ViewerId>,
    fallback: u32,
) -> Result<u32, String> {
    let kind = if world.non_send::<DocumentSession>().is_remote() {
        let document = world
            .non_send_mut::<DocumentSession>()
            .open_file_auto(path)
            .map_err(|error| error.to_string())?;
        world
            .non_send::<DocumentSession>()
            .document_kind(document)
            .map_err(|error| error.to_string())?
    } else {
        bedtime_files::files::classify_file(path).map_err(|error| error.to_string())?
    };
    let registry = world.resource::<bedtime_api::features::FeatureRegistry>();
    let descriptor = viewer
        .and_then(|id| registry.viewer(&bedtime_api::ViewerId::from(id)))
        .or_else(|| {
            registry.preferred_viewer_for_file(
                &path.to_string_lossy(),
                kind,
                &world
                    .resource::<bedtime_api::settings::SettingsSnapshot>()
                    .default_viewers,
            )
        });
    if kind == bedtime_documents::DocumentKind::Text
        && descriptor.is_some_and(|viewer| !viewer.is_local_file())
    {
        Ok(preferred_text_area(world).unwrap_or(fallback))
    } else {
        Ok(fallback)
    }
}

fn terminal_open_area(world: &mut World, captured: u32) -> u32 {
    let panels = world
        .query::<(&bedtime_workbench::PanelPresentation, &PanelBinding)>()
        .iter(world)
        .map(|(panel, binding)| {
            (
                panel.id,
                binding.terminal.is_some(),
                binding.document.is_some(),
            )
        })
        .collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>();
    let target = panels
        .iter()
        .filter(|(id, _, _)| layout.tiling.area_for(*id) == Some(captured))
        .collect::<Vec<_>>();
    if target.is_empty() || target.iter().any(|(_, terminal, _)| !terminal) {
        return captured;
    }
    layout
        .tiling
        .layout
        .areas
        .iter()
        .filter(|area| {
            panels
                .iter()
                .any(|(id, _, document)| *document && layout.tiling.area_for(*id) == Some(area.id))
        })
        .max_by_key(|area| {
            (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
        })
        .map_or(captured, |area| area.id)
}

struct SourceJump {
    position: (i32, i32),
    center: bool,
    preview: bool,
    area: u32,
    utf16: bool,
    source: Option<Entity>,
}

fn reveal_source(world: &mut World, path: &Path, jump: SourceJump) -> Result<Entity, String> {
    let SourceJump {
        position,
        center,
        preview,
        area,
        utf16,
        source,
    } = jump;
    let (row, column) = position;
    let viewer = world
        .resource::<bedtime_api::features::FeatureRegistry>()
        .default_viewer(bedtime_documents::DocumentKind::Text)
        .ok_or("No text viewer is installed")?
        .id
        .clone();
    let source = source
        .and_then(|source| bedtime_api::workbench::panel_owner(world, source))
        .filter(|source| {
            let selected = panel_document_selection(world, *source);
            bedtime_api::workbench::panel_active(world, *source)
                && selected.is_some_and(|(document, view)| {
                    view.is_some()
                        && world.non_send::<DocumentSession>().document_for_path(path)
                            == Some(document)
                })
        });
    let panel = if let Some(source) = source {
        source
    } else if preview {
        preview_file(world, path, Some(&viewer), area)?
    } else {
        open_file(world, path, Some(&viewer), Some(area))?
    };
    let binding = world
        .get::<PanelBinding>(panel)
        .ok_or("Source panel has no document binding")?;
    let view = binding.view.ok_or("Source panel has no text view")?;
    let column = if utf16 {
        world
            .non_send::<DocumentSession>()
            .with_document(
                binding.document.ok_or("Source panel has no document")?,
                |state| {
                    bedtime_editing::util::utf8::utf16_to_utf8_byte_offset(&state.line(row), column)
                },
            )
            .map_err(|error| error.to_string())?
    } else {
        column
    };
    world
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.set_cursor(
                row,
                column,
                false,
                if center {
                    bedtime_editing::text_commands::CursorReveal::Center
                } else {
                    bedtime_editing::text_commands::CursorReveal::Ensure
                },
            )
        })
        .map_err(|error| error.to_string())?;
    bedtime_workbench::focus_panel(world, panel);
    Ok(panel)
}

fn document_save_outcome(
    world: &mut World,
    save: crate::DocumentSaveId,
) -> Result<HostOutcome, String> {
    match crate::poll_document_save(world, save) {
        Some(result) => result.map(|saved| HostOutcome::Saved {
            documents: saved.iter().map(|saved| saved.document).collect(),
            revisions: saved.iter().map(|saved| saved.revision).collect(),
        }),
        None => Ok(HostOutcome::Pending(save)),
    }
}

pub fn panel_presentation(world: &World, panel: Entity) -> Option<Entity> {
    world
        .get::<bedtime_api::workbench::PanelFrontend>(panel)
        .map(|frontend| frontend.0)
        .or_else(|| bedtime_api::workbench::panel_owner(world, panel).map(|_| panel))
}

pub fn focus_panel(world: &mut World, source: Entity, request_native: bool) -> Result<(), String> {
    if request_native {
        bedtime_workbench::focus_panel(world, source);
    } else {
        bedtime_workbench::observe_focus(world, source);
    }
    publish_documents(world);
    Ok(())
}

pub fn retire_panels(world: &mut World, panels: &[Entity]) -> Result<(), String> {
    let documents = world
        .query::<(
            Entity,
            Option<&PanelBinding>,
            Option<&models::DocumentBinding>,
        )>()
        .iter(world)
        .filter_map(|(entity, text, model)| {
            panels
                .contains(&bedtime_api::workbench::panel_owner(world, entity).unwrap_or(entity))
                .then(|| {
                    text.and_then(|binding| binding.document)
                        .or_else(|| model.map(|binding| binding.0))
                })
                .flatten()
        })
        .collect::<std::collections::HashSet<_>>();
    for &panel in panels {
        bedtime_workbench::commit_close(world, panel);
    }
    for document in documents {
        retire_document(world, document)?;
    }
    publish_documents(world);
    Ok(())
}
pub fn panel_entities(world: &mut World) -> Vec<Entity> {
    let mut panels = world
        .query::<(
            Entity,
            &bedtime_workbench::PanelPresentation,
            &bedtime_workbench::PanelRecord,
        )>()
        .iter(world)
        .map(|(entity, panel, _)| (panel.id, entity))
        .collect::<Vec<_>>();
    panels.sort_by_key(|(id, _)| *id);
    panels.into_iter().map(|(_, entity)| entity).collect()
}

pub fn active_panel_entities(world: &mut World) -> Vec<Entity> {
    panel_entities(world)
        .into_iter()
        .filter(|panel| bedtime_api::workbench::panel_active(world, *panel))
        .collect()
}

pub fn snapshot_document_layout(
    world: &mut World,
    panels: Vec<Entity>,
    layout: &WorkbenchLayout,
    navigation: &DocumentNavigation,
) -> Result<Value, String> {
    let mut records = Vec::new();
    for entity in panels {
        let presentation = world
            .get::<bedtime_workbench::PanelPresentation>(entity)
            .unwrap()
            .clone();
        let binding = world
            .get::<PanelBinding>(entity)
            .cloned()
            .unwrap_or_default();
        let record = world
            .get::<bedtime_workbench::PanelRecord>(entity)
            .unwrap()
            .clone();
        let active = panel_presentation(world, entity);
        if active
            .and_then(|entity| world.get::<bedtime_api::workbench::PanelPersistence>(entity))
            .is_some_and(|policy| !policy.0)
        {
            continue;
        }
        let mut value = record.serialized.clone();
        value["id"] = json!(presentation.id);
        value["panel_type"] = json!(record.panel_type);
        value["viewer"] = json!(binding.viewer);
        if record.unavailable.is_none() {
            let state = active
                .and_then(|entity| bedtime_workbench::panel_state(world, entity).ok())
                .unwrap_or(value["state"].clone());
            value["state"] = state;
            if let Some(path) = binding.input.local_file() {
                value["path"] = json!(path);
                value["backing"] = json!("local_file");
            } else if let Some(document) = binding.document {
                let (path, kind, size, dirty) = world
                    .non_send::<DocumentSession>()
                    .with_document(document, |state| {
                        (
                            state.path.clone(),
                            state.kind,
                            state.byte_size(),
                            state.dirty,
                        )
                    })
                    .map_err(|error| error.to_string())?;
                if world
                    .non_send::<DocumentSession>()
                    .is_snapshot_document(document)
                {
                    continue;
                }
                if path.is_empty() {
                    if size != 0 || dirty {
                        continue;
                    }
                    value["default_input"] = json!(true);
                } else {
                    value["path"] = json!(path);
                    value["document_kind"] =
                        json!(if kind == bedtime_documents::DocumentKind::Bytes {
                            "bytes"
                        } else {
                            "text"
                        });
                }
            } else if binding.terminal.is_some() {
                value["default_input"] = json!(true);
                if let Some(state) = value["state"].as_object_mut() {
                    state.remove("session");
                }
            }
        }
        records.push(value);
    }
    let active_document_panel = navigation
        .last_document_panel
        .and_then(|panel| world.get::<bedtime_workbench::PanelPresentation>(panel))
        .map(|panel| panel.id)
        .filter(|id| {
            records
                .iter()
                .any(|record| record["id"].as_u64() == Some(*id))
        });
    let value = json!({"version":2,"panels":records,"focused_area":layout.focused_area,"tiling":layout.tiling.to_value(records.iter().filter_map(|record|record["id"].as_u64())),
        "ini":"","focused":layout.focused.and_then(|entity|world.get::<bedtime_workbench::PanelPresentation>(entity)).map(|panel|panel.id),
        "active_document_panel":active_document_panel});
    Ok(value)
}

/// Validate the document-layout boundary before any live context is retired.
/// Panel identities and tiling membership have explicit recovery during restore;
/// malformed panel records cannot be restored and are rejected here.
pub fn validate_document_layout(value: &Value) -> Result<(), String> {
    if value["version"] != 2 {
        return Err("Saved layout version must be 2".into());
    }
    let records = value["panels"]
        .as_array()
        .ok_or("Saved panels must be an array")?;
    for record in records {
        if !record.is_object() {
            return Err("Saved panels must be objects".into());
        }
        if !record["panel_type"]
            .as_str()
            .is_some_and(bedtime_api::workbench::valid_id)
        {
            return Err("Saved panel must name a valid panel_type".into());
        }
        for key in ["path", "viewer", "backing", "document_kind"] {
            if let Some(value) = record.get(key)
                && !value.is_null()
                && !value.is_string()
            {
                return Err(format!("Saved panel {key} must be a string"));
            }
        }
        if let Some(value) = record.get("default_input")
            && !value.is_boolean()
        {
            return Err("Saved panel default_input must be a boolean".into());
        }
    }
    Ok(())
}

pub fn restore_document_layout(
    world: &mut World,
    value: &Value,
    attach: bool,
    terminal: Option<Entity>,
) -> Result<(), String> {
    use bedtime_workbench::workspace::tiling_state::TilingState;
    validate_document_layout(value)?;
    let mut records = value["panels"]
        .as_array()
        .expect("validated document layout")
        .clone();
    let carried = if attach {
        active_panel_entities(world)
    } else {
        Vec::new()
    };
    let carried_focus = world
        .resource::<WorkbenchLayout>()
        .focused
        .filter(|panel| carried.contains(panel));
    let all_panels = panel_entities(world);
    let mut used = all_panels
        .iter()
        .map(|entity| {
            world
                .get::<bedtime_workbench::PanelPresentation>(*entity)
                .unwrap()
                .id
        })
        .collect::<HashSet<_>>();
    let terminal_slot = terminal.and_then(|_| {
        records
            .iter()
            .position(|record| record["panel_type"] == "bedtime.terminal.panel")
    });
    let mut seen = HashSet::new();
    let recovered_ids = records.iter().any(|record| {
        record["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .is_none_or(|id| !seen.insert(id))
    });
    if recovered_ids {
        notice(
            world,
            None,
            "Saved panel identities were invalid; restored the contents in a fresh layout",
        );
    }
    let mut remap = HashMap::new();
    let mut next = records
        .iter()
        .filter_map(|record| record["id"].as_u64().filter(|id| *id <= u32::MAX as u64))
        .chain(used.iter().copied())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut recovered_next = 1;
    for (index, record) in records.iter_mut().enumerate() {
        let saved = record["id"].as_u64().unwrap_or(0);
        let id = if Some(index) == terminal_slot {
            world
                .get::<bedtime_workbench::PanelPresentation>(terminal.unwrap())
                .unwrap()
                .id
        } else if recovered_ids {
            while used.contains(&recovered_next) {
                recovered_next += 1;
            }
            let id = recovered_next;
            recovered_next += 1;
            id
        } else if used.contains(&saved) {
            let id = next;
            next = next
                .checked_add(1)
                .ok_or_else(|| "Panel IDs exhausted".to_owned())?;
            id
        } else {
            saved
        };
        used.insert(id);
        remap.entry(saved).or_insert(id);
        record["id"] = json!(id);
    }
    let saved_ids = remap.keys().copied().collect::<Vec<_>>();
    let parsed = if recovered_ids {
        None
    } else if value["tiling"].is_object() {
        match TilingState::from_value(&value["tiling"], saved_ids.iter().copied()) {
            Ok(tiling) => Some(tiling),
            Err(error) => {
                notice(
                    world,
                    None,
                    format!(
                        "Saved layout was invalid; restored the contents in a fresh layout: {error}"
                    ),
                );
                None
            }
        }
    } else {
        None
    };
    let mut tiling = match parsed {
        Some(mut tiling) => {
            tiling.remap_panel_ids(&remap);
            tiling
        }
        None => {
            let mut tiling = TilingState::default();
            for record in &records {
                tiling.assign(record["id"].as_u64().unwrap(), 1);
            }
            tiling
        }
    };
    if !attach && !active_panel_entities(world).is_empty() {
        return Err("Saved layout requires an empty editor".to_owned());
    }
    let mut restored = HashMap::new();
    for (index, mut record) in records.into_iter().enumerate() {
        let id = record["id"].as_u64().unwrap();
        if Some(index) == terminal_slot {
            restored.insert(id, terminal.unwrap());
            continue;
        }
        let kind = bedtime_api::PanelTypeId::from(
            record["panel_type"]
                .as_str()
                .ok_or_else(|| "Saved panel must name its panel_type".to_owned())?,
        );
        record["panel_type"] = json!(kind);
        let viewer = record["viewer"].as_str().map(bedtime_api::ViewerId::from);
        let descriptor = viewer
            .as_ref()
            .and_then(|viewer| {
                world
                    .resource::<bedtime_api::features::FeatureRegistry>()
                    .viewer(viewer)
            })
            .cloned();
        let mut unavailable = viewer
            .as_deref()
            .filter(|_| descriptor.is_none())
            .map(|viewer| format!("Viewer unavailable: {viewer}"));
        let input = if let Some(path) = record["path"].as_str().filter(|path| !path.is_empty()) {
            if descriptor
                .as_ref()
                .is_some_and(|viewer| viewer.is_local_file())
                || record["backing"] == "local_file"
            {
                PanelInput::LocalFile(PathBuf::from(path))
            } else {
                let kind = if record["document_kind"] == "bytes" {
                    bedtime_documents::DocumentKind::Bytes
                } else {
                    descriptor
                        .as_ref()
                        .and_then(|viewer| viewer.document_kind())
                        .unwrap_or(bedtime_documents::DocumentKind::Text)
                };
                match world
                    .non_send_mut::<DocumentSession>()
                    .open_file_with_kind(Path::new(path), kind)
                {
                    Ok(document) => PanelInput::Document(document),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        let panel = crate::files::queue_document_restore(
                            world,
                            record,
                            tiling.area_for(id),
                        )?;
                        restored.insert(id, panel);
                        continue;
                    }
                    Err(error) => {
                        unavailable = Some(error.to_string());
                        notice(world, None, error.to_string());
                        PanelInput::None
                    }
                }
            }
        } else {
            PanelInput::None
        };
        let panel =
            bedtime_workbench::spawn_panel(world, record.clone(), tiling.area_for(id), Some(id))?;
        world
            .get_mut::<bedtime_workbench::PanelRecord>(panel)
            .unwrap()
            .unavailable = unavailable.clone();
        let binding = PanelBinding {
            document: input.document(),
            input,
            viewer,
            session_id: world.non_send::<DocumentSession>().session_id().0,
            ..Default::default()
        };
        world.entity_mut(panel).insert(binding.clone());
        let state = record
            .get("state")
            .cloned()
            .unwrap_or_else(|| record.clone());
        // A removed plugin is a valid persisted panel state. Its unavailable
        // presenter preserves the record; installed factories report failures.
        let report_factory_failure = unavailable.is_none()
            && world
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .panel(&kind)
                .is_some();
        if let Err(error) = mount_panel_frontend(world, panel, &kind, binding, state)
            && report_factory_failure
        {
            notice(world, Some(panel), error);
        }
        if let Some(error) = unavailable
            && let Some(frontend) = panel_presentation(world, panel)
        {
            world
                .get_mut::<bedtime_workbench::PanelRecord>(panel)
                .unwrap()
                .unavailable = Some(error.clone());
            if let Some(mut record) = world.get_mut::<bedtime_workbench::PanelRecord>(frontend) {
                record.unavailable = Some(error);
            }
        }
        // Preserve the original record for unavailable features.
        world
            .get_mut::<bedtime_workbench::PanelRecord>(panel)
            .unwrap()
            .serialized = record;
        restored.insert(id, panel);
    }
    let destination = tiling
        .layout
        .areas
        .iter()
        .max_by_key(|area| {
            (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
        })
        .unwrap()
        .id;
    for entity in carried {
        if restored.values().any(|restored| *restored == entity) {
            continue;
        }
        let id = world
            .get::<bedtime_workbench::PanelPresentation>(entity)
            .unwrap()
            .id;
        tiling.assign(id, destination);
    }
    let live_ids = active_panel_entities(world)
        .iter()
        .map(|entity| {
            world
                .get::<bedtime_workbench::PanelPresentation>(*entity)
                .unwrap()
                .id
        })
        .collect::<Vec<_>>();
    tiling.retain_panels(live_ids);
    let focused_area = value["focused_area"]
        .as_u64()
        .and_then(|area| u32::try_from(area).ok())
        .filter(|area| tiling.areas.iter().any(|group| group.area == *area))
        .or_else(|| tiling.areas.first().map(|group| group.area));
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    world.resource_mut::<WorkbenchLayout>().focused_area = focused_area;
    let active = value["active_document_panel"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .and_then(|id| restored.get(id))
        .copied();
    if (!attach
        || world
            .resource::<DocumentNavigation>()
            .last_document_panel
            .is_none())
        && active.is_some()
    {
        world
            .resource_mut::<DocumentNavigation>()
            .last_document_panel = active;
    }
    let focus = value["focused"]
        .as_u64()
        .and_then(|id| remap.get(&id))
        .and_then(|id| restored.get(id))
        .copied();
    world.resource_mut::<WorkbenchLayout>().focused = None;
    if let Some(panel) = if attach {
        carried_focus.or(focus)
    } else {
        focus
    } {
        focus_panel(world, panel, true)?;
    }
    publish_documents(world);
    Ok(())
}

pub fn host_requests_pending(world: &World) -> bool {
    let requests = world.resource::<HostRequests>();
    !requests.pending.is_empty()
        || !requests.commands.is_empty()
        || !requests.saves.is_empty()
        || !requests.transitions.is_empty()
        || requests.quit.is_some()
        || !requests.batches.is_empty()
        || crate::operation_barrier_active(world)
        || bedtime_workbench::waiting_for_layout(world)
}

pub fn initialize_file_context(
    world: &mut World,
    context: bedtime_files::FileContext,
    services: bedtime_files::FileServices,
) -> Result<(), String> {
    let mut options = world.non_send::<DocumentSession>().options().clone();
    // Context services watch backing files independently of optional panels.
    options.monitoring = true;
    options.history_path = None;
    world
        .non_send_mut::<DocumentSession>()
        .configure_file_services(options, &context, &services)
        .map_err(|error| error.to_string())?;
    world
        .resource_mut::<bedtime_files::FileContext>()
        .replace(context);
    world.insert_resource(services);
    publish_documents(world);
    Ok(())
}

fn retire_document_frontend(world: &mut World, panel: Entity) -> Result<(), String> {
    retire_panel(world, panel);
    let frontends = world
        .query::<(Entity, &bedtime_api::workbench::PanelOwner)>()
        .iter(world)
        .filter_map(|(entity, owner)| (owner.0 == panel).then_some(entity))
        .collect::<Vec<_>>();
    for frontend in frontends {
        drop(bedtime_workbench::take_panel_frontend(world, frontend));
        world.despawn(frontend);
    }
    world
        .entity_mut(panel)
        .remove::<bedtime_api::workbench::PanelFrontend>();
    Ok(())
}
fn pristine_document_panel(world: &World, panel: Entity) -> bool {
    let Some(surface) = panel_presentation(world, panel) else {
        return false;
    };
    // Reusing the shell requires a child frontend that can be fully retired.
    // A standalone core tab owns its feature components on the shell itself.
    if surface == panel {
        return false;
    }
    if world
        .get::<bedtime_api::documents::ReusableDocumentInput>(surface)
        .is_none()
    {
        return false;
    }
    let Some(stamp) = world.get::<bedtime_api::documents::PanelDefaultInput>(surface) else {
        return false;
    };
    let session = world.non_send::<DocumentSession>();
    session.document_revision(stamp.document).ok() == Some(stamp.revision)
        && session
            .with_document(stamp.document, |state| {
                state.path.is_empty() && state.byte_size() == 0 && !state.dirty
            })
            .unwrap_or(false)
}

fn reusable_document_panel(world: &mut World, area: Option<u32>) -> Result<Option<Entity>, String> {
    let layout = world.resource::<WorkbenchLayout>();
    let focused = layout.focused.filter(|panel| {
        area.is_none_or(|area| {
            world
                .get::<bedtime_workbench::PanelPresentation>(*panel)
                .is_some_and(|panel| layout.tiling.area_for(panel.id) == Some(area))
        })
    });
    let area = area.unwrap_or_else(|| {
        layout
            .tiling
            .layout
            .areas
            .iter()
            .max_by_key(|area| {
                (area.rect.max[0] - area.rect.min[0]) * (area.rect.max[1] - area.rect.min[1])
            })
            .unwrap()
            .id
    });
    let Some(ids) = layout
        .tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .map(|group| group.tabs.clone())
    else {
        return Ok(None);
    };
    let panels = panel_entities(world);
    let candidates = focused
        .into_iter()
        .chain(ids.into_iter().filter_map(|id| {
            panels.iter().copied().find(|panel| {
                world
                    .get::<bedtime_workbench::PanelPresentation>(*panel)
                    .unwrap()
                    .id
                    == id
            })
        }))
        .collect::<Vec<_>>();
    for panel in candidates {
        if !pristine_document_panel(world, panel) {
            continue;
        }
        let document = world
            .get::<PanelBinding>(panel)
            .and_then(|binding| binding.document)
            .expect("pristine input has a document");
        commit_document_drafts(world, &[document])?;
        if pristine_document_panel(world, panel) {
            return Ok(Some(panel));
        }
    }
    Ok(None)
}

fn replace_pristine_document_panel(
    world: &mut World,
    panel: Entity,
    kind: &bedtime_api::PanelTypeId,
    document: DocumentId,
    viewer: &bedtime_api::ViewerId,
) -> Result<Entity, String> {
    let old = world
        .get::<PanelBinding>(panel)
        .and_then(|binding| binding.document);
    retire_document_frontend(world, panel)?;
    let binding = PanelBinding {
        input: PanelInput::Document(document),
        document: Some(document),
        viewer: Some(viewer.into()),
        session_id: world.non_send::<DocumentSession>().session_id().0,
        ..Default::default()
    };
    let id = world
        .get::<bedtime_workbench::PanelPresentation>(panel)
        .unwrap()
        .id;
    world
        .entity_mut(panel)
        .insert(bedtime_workbench::PanelRecord {
            panel_type: kind.into(),
            serialized: json!({"id":id,"panel_type":kind,"viewer":viewer,"state":null}),
            unavailable: None,
        });
    mount_panel_frontend(world, panel, kind, binding, Value::Null)?;
    if let Some(old) = old.filter(|old| *old != document) {
        let retained = world
            .query::<(&bedtime_workbench::PanelPresentation, &PanelBinding)>()
            .iter(world)
            .any(|(_, binding)| binding.document == Some(old));
        if !retained {
            retire_document(world, old)?;
        }
    }
    focus_panel(world, panel, true)?;
    Ok(panel)
}

pub fn command_arguments(context: &bedtime_api::documents::CommandContext) -> Value {
    json!({"path":context.path,"document":context.document.map(|id|id.0),"revision":context.revision,
        "selection":context.selection.as_ref().map(|selection|json!({"document":selection.document.0,"revision":selection.revision,
            "ranges":selection.ranges.iter().map(|range|[range.start,range.end]).collect::<Vec<_>>(),"text":selection.text}))})
}
