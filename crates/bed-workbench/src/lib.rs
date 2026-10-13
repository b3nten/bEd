//! Content-independent desktop panels, layout, persistence and command dispatch.
mod compositor;
mod docs;
pub use docs::{DOCS_PANEL_ID, DocsPlugin, SHOW_DOCS_COMMAND};
mod shortcuts;

#[derive(Resource, Default)]
struct PanelFocusHistory(Vec<Entity>);
pub use shortcuts::{
    ResolvedKeymap, WorkbenchShortcuts, global_shortcuts, prepare_shortcuts, resolve_keymap,
};
mod status_bar;
pub use status_bar::{StatusBarHost, StatusIconResolver, render_status_bar, status_bar_height};
mod tiling_ui;
pub mod workspace;
pub use compositor::{
    PanelDirty, WorkbenchAppearance, area_at, area_bounds, render_ui, tab_bounds,
};
unsafe extern "C" {
    fn bed_imgui_dock_node_clear_background(node: *mut dear_imgui_rs::sys::ImGuiDockNode);

    fn bed_imgui_dock_node_select_window(
        node: *mut dear_imgui_rs::sys::ImGuiDockNode,
        window: *mut dear_imgui_rs::sys::ImGuiWindow,
    );
}
pub use api::{
    CommandInvocation, Panel, PanelPresentation, WorkbenchAction, WorkbenchRegistry, register,
};
use api::{PanelCreation, PanelResult};
pub use bed_api::workbench as api;
use bevy::prelude::*;
use dear_imgui_rs::{Condition, Ui, WindowFlags};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
};
use workspace::tiling_state::TilingState;

/// Logical inset reserved for native application chrome outside the panels.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct WorkbenchViewport {
    pub top_inset: f32,
    pub bottom_inset: f32,
    pub bounds: Option<([f32; 2], [f32; 2])>,
}

/// Identity of the current live layout activation. It changes when a host
/// switches layouts, even when it later returns to the same numbered layout.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkbenchLayoutEpoch(pub u64);

pub fn advance_layout_epoch(world: &mut World) -> WorkbenchLayoutEpoch {
    world.init_resource::<WorkbenchLayoutEpoch>();
    let epoch = {
        let mut epoch = world.resource_mut::<WorkbenchLayoutEpoch>();
        epoch.0 = epoch
            .0
            .checked_add(1)
            .expect("layout activation epoch overflow");
        *epoch
    };
    if let Some(mut requests) = world.get_resource_mut::<WorkbenchRequests>() {
        requests.layout_epoch = epoch;
    }
    epoch
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct WorkbenchLayout {
    pub tiling: TilingState,
    pub focused: Option<Entity>,
    pub focused_area: Option<u32>,
    next_id: u64,
}

impl Default for WorkbenchLayout {
    fn default() -> Self {
        Self {
            tiling: TilingState::default(),
            focused: None,
            focused_area: Some(1),
            next_id: 1,
        }
    }
}

/// Keeps the complete saved record, including fields from unavailable features.
#[derive(Component, Clone, Debug)]
pub struct PanelRecord {
    pub panel_type: String,
    pub serialized: Value,
    pub unavailable: Option<String>,
}

#[derive(Component)]
struct WorkbenchPanel;

pub fn owns_panel(world: &World, entity: Entity) -> bool {
    world.get::<WorkbenchPanel>(entity).is_some()
}

#[derive(Default)]
struct PanelUiStore(HashMap<Entity, Box<dyn Panel>>);

#[derive(Resource)]
struct UiLoan;

pub fn frontend_active(world: &World) -> bool {
    world.contains_resource::<UiLoan>()
}

/// Optional close preflight. Return true to remove the panel now, false to keep
/// it while an application finishes validation or asynchronous saves. The
/// policy must leave panel membership and layout intact; call `commit_close`
/// later when a deferred close is ready. Policies run between frontend scopes.
#[derive(Resource, Clone, Copy)]
pub struct WorkbenchClosePolicy(pub fn(&mut World, Entity) -> Result<bool, String>);

/// Release associated feature state only when an approved close is committed.
/// Approval may be requested for several panels before any are retired.
#[derive(Resource, Clone, Copy)]
pub struct WorkbenchPanelRetirement(pub fn(&mut World, Entity));

#[derive(Resource)]
struct ClosingPanels(Vec<Entity>);
/// Complete set being considered by the current close preflight. A document
/// owner can distinguish another surviving view from another closing view.
pub fn closing_panels(world: &World) -> Option<&[Entity]> {
    world
        .get_resource::<ClosingPanels>()
        .map(|panels| panels.0.as_slice())
}

fn preflight_closes(world: &mut World, panels: &[Entity]) -> Result<bool, String> {
    let Some(policy) = world.get_resource::<WorkbenchClosePolicy>().copied() else {
        return Ok(true);
    };
    assert!(
        !world.contains_resource::<ClosingPanels>(),
        "close preflights do not nest"
    );
    world.insert_resource(ClosingPanels(panels.to_vec()));
    let result = (|| {
        let mut approved = true;
        for &entity in panels {
            approved &= (policy.0)(world, entity)?;
        }
        Ok(approved)
    })();
    world.remove_resource::<ClosingPanels>().unwrap();
    result
}

/// Optional host preparation of a newly spawned panel, before its feature
/// factory runs. Document hosts establish canonical associations here while
/// ordinary workbenches require no extra preparation. A failure retains the
/// saved panel as unavailable instead of constructing a partial frontend.
#[derive(Resource, Clone, Copy)]
pub struct PanelPreparation(pub fn(&mut World, Entity) -> Result<(), String>);

/// Retain the damaged object without calling it again. Moving this presenter
/// out of and back into the store preserves its quarantine and saved state.
struct FailedPanel {
    _damaged: Box<dyn Panel>,
    title: String,
    state: Value,
    error: String,
}

impl Panel for FailedPanel {
    fn title(&self, _world: &World) -> String {
        self.title.clone()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        _world: &mut World,
        _requests: &mut Vec<WorkbenchAction>,
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        ui.text_wrapped(&self.error);
        ui.text_wrapped("Close and reopen this panel to try again.");
    }

    fn save(&self, _world: &World) -> Value {
        self.state.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkbenchRequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkbenchOutcome {
    Applied,
    Ignored,
    Opened(Entity),
    Closed(Entity),
}

/// A host supplies its own transition barrier while sharing this queue.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkbenchRequestMode {
    #[default]
    Automatic,
    External,
}

pub use bed_api::features::next_request_sequence;

pub struct QueuedWorkbenchRequest {
    pub sequence: u64,
    pub id: Option<WorkbenchRequestId>,
    pub action: WorkbenchAction,
    pub layout_epoch: Option<WorkbenchLayoutEpoch>,
}

#[derive(Resource, Default)]
pub struct WorkbenchRequests {
    next_id: u64,
    layout_epoch: WorkbenchLayoutEpoch,
    pending: VecDeque<QueuedWorkbenchRequest>,
    results: HashMap<WorkbenchRequestId, Result<WorkbenchOutcome, String>>,
    waiting: HashMap<WorkbenchRequestId, (WorkbenchOutcome, Vec<Entity>)>,
    layouts: Vec<(Option<WorkbenchRequestId>, Value, Vec<Entity>)>,
    errors: Vec<String>,
}

impl WorkbenchRequests {
    pub fn push(&mut self, action: WorkbenchAction) {
        self.enqueue(action, None, Some(self.layout_epoch));
    }

    /// Application-wide actions, such as changing layouts or quitting, remain
    /// valid across layout activations. Ordinary UI requests use `push`.
    pub fn push_unscoped(&mut self, action: WorkbenchAction) {
        self.enqueue(action, None, None);
    }

    fn enqueue(
        &mut self,
        action: WorkbenchAction,
        id: Option<WorkbenchRequestId>,
        layout_epoch: Option<WorkbenchLayoutEpoch>,
    ) {
        self.pending.push_back(QueuedWorkbenchRequest {
            sequence: next_request_sequence(),
            id,
            action,
            layout_epoch,
        });
    }

    pub fn submit(&mut self, action: WorkbenchAction) -> WorkbenchRequestId {
        let id = self.allocate_id();
        self.enqueue(action, Some(id), Some(self.layout_epoch));
        id
    }

    pub fn submit_unscoped(&mut self, action: WorkbenchAction) -> WorkbenchRequestId {
        let id = self.allocate_id();
        self.enqueue(action, Some(id), None);
        id
    }

    fn allocate_id(&mut self) -> WorkbenchRequestId {
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("workbench request ID overflow");
        WorkbenchRequestId(self.next_id)
    }

    pub fn take_result(
        &mut self,
        id: WorkbenchRequestId,
    ) -> Option<Result<WorkbenchOutcome, String>> {
        self.results.remove(&id)
    }

    pub fn next_sequence(&self) -> Option<u64> {
        self.pending.front().map(|request| request.sequence)
    }

    pub fn take_next(&mut self) -> Option<QueuedWorkbenchRequest> {
        self.pending.pop_front()
    }

    pub fn complete(&mut self, id: WorkbenchRequestId, result: Result<WorkbenchOutcome, String>) {
        self.waiting.remove(&id);
        self.results.insert(id, result);
    }

    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkbenchSystems {
    Keymap,
    InputProjection,
    Requests,
}

pub struct WorkbenchPlugin;

impl Plugin for WorkbenchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchRegistry>()
            .init_resource::<api::CommandPresentations>()
            .init_resource::<api::PinchEvents>()
            .init_resource::<WorkbenchLayout>()
            .init_resource::<PanelFocusHistory>()
            .init_resource::<WorkbenchViewport>()
            .init_resource::<WorkbenchLayoutEpoch>()
            .init_resource::<WorkbenchRequests>()
            .init_resource::<WorkbenchAppearance>()
            .init_resource::<WorkbenchRequestMode>()
            .init_non_send::<PanelUiStore>()
            .init_non_send::<tiling_ui::TilingFrontend>()
            .add_message::<api::PanelDrop>()
            .add_message::<bed_api::storage::FeatureStateChanged>()
            .add_systems(
                PreUpdate,
                (
                    shortcuts::resolve_keymap.in_set(WorkbenchSystems::Keymap),
                    process_requests.in_set(WorkbenchSystems::Requests),
                ),
            );
    }
}

pub fn capture_command(
    world: &World,
    command: impl Into<String>,
    arguments: Value,
) -> CommandInvocation {
    let layout = world.resource::<WorkbenchLayout>();
    let area = layout.tiling.layout.largest_area();
    CommandInvocation {
        command: command.into(),
        source: layout.focused,
        area,
        arguments,
    }
}

/// One controlled FIFO transition point. Returned effects finish before the
/// next command, preserving edit/save and open/focus ordering.
pub fn process_requests(world: &mut World) {
    if *world.resource::<WorkbenchRequestMode>() == WorkbenchRequestMode::External {
        return;
    }
    poll_request_results(world);
    while let Some(queued) = world.resource_mut::<WorkbenchRequests>().take_next() {
        resolve_request(world, queued);
    }
}

/// Finish deferred close and layout results without consuming another request.
/// External hosts call this at their own controlled transition barrier.
pub fn poll_request_results(world: &mut World) {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "commands run between frontend scopes"
    );
    let layouts = std::mem::take(&mut world.resource_mut::<WorkbenchRequests>().layouts);
    for (id, tiling, close) in layouts {
        if close
            .iter()
            .all(|entity| !world.entities().contains(*entity))
        {
            let result = apply_layout(world, &tiling).map(|_| WorkbenchOutcome::Applied);
            if let Some(id) = id {
                world
                    .resource_mut::<WorkbenchRequests>()
                    .complete(id, result);
            } else if let Err(error) = result {
                world.resource_mut::<WorkbenchRequests>().errors.push(error);
            }
        } else {
            world
                .resource_mut::<WorkbenchRequests>()
                .layouts
                .push((id, tiling, close));
        }
    }
    let completed = world
        .resource::<WorkbenchRequests>()
        .waiting
        .iter()
        .filter(|(_, (_, entities))| {
            entities
                .iter()
                .all(|entity| !world.entities().contains(*entity))
        })
        .map(|(id, (outcome, _))| (*id, outcome.clone()))
        .collect::<Vec<_>>();
    for (id, outcome) in completed {
        world
            .resource_mut::<WorkbenchRequests>()
            .complete(id, Ok(outcome));
    }
}

/// Whether a requested geometry replacement is waiting for its panels to close.
pub fn waiting_for_layout(world: &World) -> bool {
    !world.resource::<WorkbenchRequests>().layouts.is_empty()
}

/// Resolve one request through the same authoritative implementation as the
/// automatic workbench. Command effects finish synchronously before returning.
pub fn resolve_request(world: &mut World, queued: QueuedWorkbenchRequest) {
    assert!(
        !frontend_active(world),
        "commands run between frontend scopes"
    );
    if queued
        .layout_epoch
        .is_some_and(|epoch| epoch != *world.resource::<WorkbenchLayoutEpoch>())
    {
        if let Some(id) = queued.id {
            world
                .resource_mut::<WorkbenchRequests>()
                .complete(id, Ok(WorkbenchOutcome::Ignored));
        }
        return;
    }
    if let WorkbenchAction::SetLayout { tiling, close } = queued.action {
        match request_layout(world, &tiling, &close) {
            Ok(true) => {
                if let Some(id) = queued.id {
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .complete(id, Ok(WorkbenchOutcome::Applied));
                }
            }
            Ok(false) => world
                .resource_mut::<WorkbenchRequests>()
                .layouts
                .push((queued.id, tiling, close)),
            Err(error) => {
                if let Some(id) = queued.id {
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .complete(id, Err(error));
                } else {
                    world.resource_mut::<WorkbenchRequests>().errors.push(error);
                }
            }
        }
        return;
    }
    match execute_action(world, queued.action) {
        Ok((outcome, waiting)) => {
            if let Some(id) = queued.id {
                if waiting.is_empty() {
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .complete(id, Ok(outcome));
                } else {
                    world
                        .resource_mut::<WorkbenchRequests>()
                        .waiting
                        .insert(id, (outcome, waiting));
                }
            }
        }
        Err(error) => {
            if let Some(id) = queued.id {
                world
                    .resource_mut::<WorkbenchRequests>()
                    .complete(id, Err(error));
            } else {
                world.resource_mut::<WorkbenchRequests>().errors.push(error);
            }
        }
    }
}

fn execute_action(
    world: &mut World,
    action: WorkbenchAction,
) -> Result<(WorkbenchOutcome, Vec<Entity>), String> {
    match action {
        WorkbenchAction::Invoke(invocation) => {
            if invocation
                .source
                .is_some_and(|entity| api::panel_owner(world, entity).is_none())
            {
                return Ok((WorkbenchOutcome::Ignored, Vec::new()));
            }
            if world
                .resource::<api::CommandPresentations>()
                .0
                .get(&invocation.command)
                .is_some_and(|presentation| !presentation.enabled || !presentation.visible)
            {
                return Ok((WorkbenchOutcome::Ignored, Vec::new()));
            }
            let handler = world
                .resource::<WorkbenchRegistry>()
                .command(&invocation.command)
                .map(|entry| entry.handler)
                .ok_or_else(|| format!("Command unavailable: {}", invocation.command))?;
            let effects = world
                .run_system_with(handler, invocation)
                .map_err(|error| error.to_string())??;
            let mut waiting = Vec::new();
            for effect in effects {
                waiting.extend(execute_action(world, effect)?.1);
            }
            Ok((WorkbenchOutcome::Applied, waiting))
        }
        WorkbenchAction::ShowPanel {
            panel_type,
            state,
            area,
        } => {
            let existing = recent_panel(world, &panel_type);
            if let Some(entity) = existing {
                focus_panel(world, entity);
                Ok((WorkbenchOutcome::Applied, Vec::new()))
            } else {
                open_panel(
                    world,
                    json!({"panel_type":panel_type,"state":state}),
                    area,
                    None,
                )
                .map(|entity| (WorkbenchOutcome::Opened(entity), Vec::new()))
            }
        }
        WorkbenchAction::OpenPanel {
            panel_type,
            state,
            area,
        } => {
            let record = json!({"panel_type":panel_type,"state":state});
            open_panel(world, record, area, None)
                .map(|entity| (WorkbenchOutcome::Opened(entity), Vec::new()))
        }
        WorkbenchAction::ClosePanel(source) => {
            let Some(entity) =
                api::panel_owner(world, source).filter(|entity| owns_panel(world, *entity))
            else {
                return Ok((WorkbenchOutcome::Ignored, Vec::new()));
            };
            if request_close(world, entity)? {
                Ok((WorkbenchOutcome::Closed(entity), Vec::new()))
            } else {
                Ok((WorkbenchOutcome::Closed(entity), vec![entity]))
            }
        }
        WorkbenchAction::KeepPanel(source) => {
            let kept = keep_panel(world, source);
            Ok((
                if kept {
                    WorkbenchOutcome::Applied
                } else {
                    WorkbenchOutcome::Ignored
                },
                Vec::new(),
            ))
        }
        WorkbenchAction::ObserveFocus(source) => {
            let Some(panel) =
                api::panel_owner(world, source).filter(|entity| owns_panel(world, *entity))
            else {
                return Ok((WorkbenchOutcome::Ignored, Vec::new()));
            };
            observe_focus(world, panel);
            Ok((WorkbenchOutcome::Applied, Vec::new()))
        }
        WorkbenchAction::FocusArea(area) => {
            if world
                .resource::<WorkbenchLayout>()
                .tiling
                .layout
                .areas
                .iter()
                .any(|candidate| candidate.id == area)
            {
                focus_area(world, area);
                Ok((WorkbenchOutcome::Applied, Vec::new()))
            } else {
                Ok((WorkbenchOutcome::Ignored, Vec::new()))
            }
        }
        WorkbenchAction::SetLayout { tiling, close } => {
            if request_layout(world, &tiling, &close)? {
                Ok((WorkbenchOutcome::Applied, Vec::new()))
            } else {
                world.resource_mut::<WorkbenchRequests>().layouts.push((
                    None,
                    tiling,
                    close.clone(),
                ));
                Ok((WorkbenchOutcome::Applied, close))
            }
        }
        WorkbenchAction::FocusPanel(entity) => {
            let exists =
                api::panel_owner(world, entity).is_some_and(|entity| owns_panel(world, entity));
            focus_panel(world, entity);
            Ok((
                if exists {
                    WorkbenchOutcome::Applied
                } else {
                    WorkbenchOutcome::Ignored
                },
                Vec::new(),
            ))
        }
        WorkbenchAction::SplitPanel { panel, axis, high } => {
            if axis > 1 {
                return Err("Split axis must be horizontal or vertical".into());
            }
            let target = api::panel_owner(world, panel)
                .filter(|entity| owns_panel(world, *entity))
                .and_then(|entity| world.get::<PanelPresentation>(entity).map(|panel| panel.id));
            if let Some(id) = target {
                let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
                if let Some(area) = tiling.area_for(id) {
                    tiling.split_tab(id, area, axis, high, [100, 100]);
                    install_tiling(world, tiling);
                    return Ok((WorkbenchOutcome::Applied, Vec::new()));
                }
            }
            Ok((WorkbenchOutcome::Ignored, Vec::new()))
        }
    }
}

/// Resolve a frontend to its owning tab; only the workbench changes replaceability.
pub fn keep_panel(world: &mut World, source: Entity) -> bool {
    let Some(panel) = api::panel_owner(world, source).filter(|panel| owns_panel(world, *panel))
    else {
        return false;
    };
    let id = world.get::<PanelPresentation>(panel).unwrap().id;
    world.resource_mut::<WorkbenchLayout>().tiling.keep_tab(id)
}

/// Move an explicit file/drop destination and retain it as an ordinary tab.
pub fn move_panel_to_area(world: &mut World, source: Entity, area: u32) -> Result<(), String> {
    assert!(
        !frontend_active(world),
        "panels move between frontend scopes"
    );
    let panel = api::panel_owner(world, source)
        .filter(|panel| owns_panel(world, *panel) && api::panel_active(world, *panel))
        .ok_or("Panel is unavailable")?;
    let id = world.get::<PanelPresentation>(panel).unwrap().id;
    let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
    let index = tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .ok_or("Target area is unavailable")?
        .tabs
        .len();
    tiling.move_tab(id, area, index);
    install_tiling(world, tiling);
    focus_panel(world, panel);
    Ok(())
}

/// The layout owner records which tab a later preview may replace.
pub fn mark_panel_preview(world: &mut World, source: Entity) -> Result<(), String> {
    assert!(
        !frontend_active(world),
        "previews change between frontend scopes"
    );
    let panel = api::panel_owner(world, source)
        .filter(|panel| owns_panel(world, *panel) && api::panel_active(world, *panel))
        .ok_or("Panel is unavailable")?;
    let id = world.get::<PanelPresentation>(panel).unwrap().id;
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    let group = layout
        .tiling
        .areas
        .iter_mut()
        .find(|group| group.tabs.contains(&id))
        .expect("active panel belongs to an area");
    group.preview = Some(id);
    Ok(())
}

pub fn focus_panel(world: &mut World, entity: Entity) {
    let Some(entity) = api::panel_owner(world, entity) else {
        return;
    };
    if !owns_panel(world, entity) || !api::panel_active(world, entity) {
        return;
    }
    let Some(id) = world.get::<PanelPresentation>(entity).map(|value| value.id) else {
        return;
    };
    let Some(area) = world.resource::<WorkbenchLayout>().tiling.area_for(id) else {
        return;
    };
    {
        let mut history = world.resource_mut::<PanelFocusHistory>();
        history.0.retain(|e| *e != entity);
        history.0.push(entity);
    }
    let siblings = world
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|group| group.area == area)
        .unwrap()
        .tabs
        .clone();
    let presentations = world
        .query::<(Entity, &PanelPresentation, &PanelRecord)>()
        .iter(world)
        .filter_map(|(entity, panel, _)| siblings.contains(&panel.id).then_some(entity))
        .collect::<Vec<_>>();
    for entity in presentations {
        world.get_mut::<PanelPresentation>(entity).unwrap().focus = false;
    }
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    layout.focused = Some(entity);
    layout.focused_area = Some(area);
    layout
        .tiling
        .areas
        .iter_mut()
        .find(|group| group.area == area)
        .unwrap()
        .selected = Some(id);
    world.get_mut::<PanelPresentation>(entity).unwrap().focus = true;
}

/// The same MRU rule is used by generic and editor-integrated show commands.
pub fn recent_panel(world: &mut World, panel_type: &str) -> Option<Entity> {
    world
        .resource::<PanelFocusHistory>()
        .0
        .iter()
        .rev()
        .copied()
        .find(|entity| {
            api::panel_active(world, *entity)
                && world
                    .get::<PanelRecord>(*entity)
                    .is_some_and(|r| r.panel_type == panel_type)
        })
        .or_else(|| {
            world
                .query::<(Entity, &PanelRecord, &PanelPresentation)>()
                .iter(world)
                .filter(|(entity, r, _)| {
                    api::panel_active(world, *entity) && r.panel_type == panel_type
                })
                .max_by_key(|(_, _, p)| p.id)
                .map(|(entity, _, _)| entity)
        })
}

pub fn observe_focus(world: &mut World, entity: Entity) {
    if !owns_panel(world, entity) || !api::panel_active(world, entity) {
        return;
    }
    {
        let mut history = world.resource_mut::<PanelFocusHistory>();
        history.0.retain(|e| *e != entity);
        history.0.push(entity);
    }
    let id = world.get::<PanelPresentation>(entity).unwrap().id;
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    if let Some(area) = layout.tiling.area_for(id) {
        layout.focused = Some(entity);
        layout.focused_area = Some(area);
        layout
            .tiling
            .areas
            .iter_mut()
            .find(|group| group.area == area)
            .unwrap()
            .selected = Some(id);
    }
}

pub fn focus_area(world: &mut World, area: u32) -> bool {
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    if !layout.tiling.areas.iter().any(|group| group.area == area) {
        return false;
    }
    layout.focused = None;
    layout.focused_area = Some(area);
    true
}

fn request_layout(world: &mut World, value: &Value, close: &[Entity]) -> Result<bool, String> {
    if close.iter().any(|entity| !owns_panel(world, *entity)) {
        return Err("Layout close target is not owned by this workbench".into());
    }
    let ids = world
        .query::<(Entity, &PanelPresentation, &WorkbenchPanel)>()
        .iter(world)
        .filter_map(|(entity, panel, _)| {
            (api::panel_active(world, entity) && !close.contains(&entity)).then_some(panel.id)
        })
        .collect::<Vec<_>>();
    TilingState::from_value(value, ids).map_err(str::to_owned)?;
    if !preflight_closes(world, close)? {
        return Ok(false);
    }
    for &entity in close {
        commit_close(world, entity);
    }
    apply_layout(world, value)?;
    Ok(true)
}

pub fn apply_layout(world: &mut World, value: &Value) -> Result<(), String> {
    let ids = world
        .query_filtered::<(&PanelPresentation, &WorkbenchPanel), Without<api::PanelSuspended>>()
        .iter(world)
        .map(|(panel, _)| panel.id)
        .collect::<Vec<_>>();
    let tiling = TilingState::from_value(value, ids).map_err(str::to_owned)?;
    install_tiling(world, tiling);
    Ok(())
}

/// A host replaced its whole active layout. Drop stale native dock placement
/// and drag state before drawing the new panels; feature state stays retained.
pub fn invalidate_layout_frontend(world: &mut World) {
    world
        .non_send_mut::<tiling_ui::TilingFrontend>()
        .tiling_ui
        .clear_roots = true;
    for mut panel in world.query::<&mut PanelPresentation>().iter_mut(world) {
        panel.dock_next = true;
        panel.focus = false;
    }
    if let Some(panel) = world.resource::<WorkbenchLayout>().focused
        && let Some(mut presentation) = world.get_mut::<PanelPresentation>(panel)
    {
        presentation.focus = true;
    }
}

// Geometry and focused-area ownership change together, including tab moves
// applied by the compositor and programmatic splits.
fn install_tiling(world: &mut World, tiling: TilingState) {
    let focused_id = world
        .resource::<WorkbenchLayout>()
        .focused
        .and_then(|entity| world.get::<PanelPresentation>(entity))
        .map(|panel| panel.id);
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    layout.focused_area = focused_id
        .and_then(|id| tiling.area_for(id))
        .or_else(|| {
            layout
                .focused_area
                .filter(|area| tiling.areas.iter().any(|group| group.area == *area))
        })
        .or_else(|| tiling.layout.areas.first().map(|area| area.id));
    layout.tiling = tiling;
    for mut panel in world
        .query_filtered::<&mut PanelPresentation, With<WorkbenchPanel>>()
        .iter_mut(world)
    {
        panel.dock_next = true;
    }
}

/// Ask the installed close owner to resolve the barrier before retiring a panel.
/// External document coordinators use the same transition as queued actions.
pub fn request_close(world: &mut World, entity: Entity) -> Result<bool, String> {
    let Some(entity) = api::panel_owner(world, entity) else {
        return Ok(false);
    };
    if !owns_panel(world, entity) {
        return Ok(false);
    }
    request_close_many(world, &[entity])
}

/// Approve an entire closing set before retiring any of its panels.
pub fn request_close_many(world: &mut World, panels: &[Entity]) -> Result<bool, String> {
    assert!(
        !frontend_active(world),
        "panels close between frontend scopes"
    );
    if panels.iter().any(|entity| !owns_panel(world, *entity)) {
        return Err("Close target is not owned by this workbench".into());
    }
    if !preflight_closes(world, panels)? {
        return Ok(false);
    }
    for &entity in panels {
        commit_close(world, entity);
    }
    Ok(true)
}

/// Remove an owned panel whose application close barrier has completed.
/// This bypasses the optional policy and releases its frontend and entity.
pub fn commit_close(world: &mut World, entity: Entity) -> bool {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "panels close after the frontend loan returns"
    );
    let Some(entity) = api::panel_owner(world, entity) else {
        return false;
    };
    if !owns_panel(world, entity) {
        return false;
    }
    if let Some(retirement) = world.get_resource::<WorkbenchPanelRetirement>().copied() {
        (retirement.0)(world, entity);
    }
    let id = world
        .get::<PanelPresentation>(entity)
        .expect("live panel has presentation")
        .id;
    world.non_send_mut::<PanelUiStore>().0.remove(&entity);
    let presentations = world
        .query::<(Entity, &api::PanelOwner)>()
        .iter(world)
        .filter_map(|(presentation, owner)| (owner.0 == entity).then_some(presentation))
        .collect::<Vec<_>>();
    for presentation in presentations {
        world.non_send_mut::<PanelUiStore>().0.remove(&presentation);
        world.despawn(presentation);
    }
    world.despawn(entity);
    world
        .resource_mut::<PanelFocusHistory>()
        .0
        .retain(|panel| *panel != entity);
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    layout.tiling.remove(id);
    if layout.focused == Some(entity) {
        layout.focused = None;
    }
    true
}

/// Allocate a layout panel before mounting a presentation against application bindings.
pub fn spawn_panel(
    world: &mut World,
    mut serialized: Value,
    area: Option<u32>,
    saved_id: Option<u64>,
) -> Result<Entity, String> {
    let panel_type = serialized["panel_type"]
        .as_str()
        .ok_or("Saved panel has no type")?
        .to_owned();
    if let Some(id) = saved_id
        && (id == 0
            || world
                .query::<&PanelPresentation>()
                .iter(world)
                .any(|panel| panel.id == id))
    {
        return Err("Invalid or duplicate saved panel ID".into());
    }
    // Retained inactive layouts can have IDs beyond this layout's allocator.
    let next_live_id = world
        .query::<&PanelPresentation>()
        .iter(world)
        .map(|panel| panel.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("Panel IDs exhausted")?;
    let (id, area) = {
        let mut layout = world.resource_mut::<WorkbenchLayout>();
        let id = saved_id.unwrap_or(layout.next_id.max(next_live_id));
        if id > u32::MAX as u64 {
            return Err("Panel IDs exhausted".into());
        }
        layout.next_id = layout
            .next_id
            .max(id.checked_add(1).ok_or("Panel IDs exhausted")?);
        let area = area
            .filter(|id| layout.tiling.areas.iter().any(|group| group.area == *id))
            .unwrap_or_else(|| layout.tiling.layout.largest_area());
        (id, area)
    };
    serialized["id"] = json!(id);
    let entity = world
        .spawn((
            WorkbenchPanel,
            PanelPresentation::new(id),
            PanelRecord {
                panel_type: panel_type.clone(),
                serialized: serialized.clone(),
                unavailable: None,
            },
        ))
        .id();
    world
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .assign(id, area);
    focus_panel(world, entity);
    Ok(entity)
}

pub fn open_panel(
    world: &mut World,
    serialized: Value,
    area: Option<u32>,
    saved_id: Option<u64>,
) -> Result<Entity, String> {
    let panel_type = serialized["panel_type"]
        .as_str()
        .ok_or("Saved panel has no type")?
        .to_owned();
    let factory = world
        .resource::<WorkbenchRegistry>()
        .panel(&panel_type)
        .map(|entry| entry.factory);
    let entity = spawn_panel(world, serialized.clone(), area, saved_id)?;
    if let Some(preparation) = world.get_resource::<PanelPreparation>().copied()
        && let Err(error) = (preparation.0)(world, entity)
    {
        world.get_mut::<PanelRecord>(entity).unwrap().unavailable = Some(error);
        return Ok(entity);
    }
    let result: PanelResult = match factory {
        Some(factory) => world
            .run_system_with(
                factory,
                PanelCreation {
                    entity,
                    state: serialized["state"].clone(),
                },
            )
            .map_err(|error| error.to_string())
            .and_then(|result| result),
        None => Err(format!("Panel feature unavailable: {panel_type}")),
    };
    match result {
        Ok(panel) => {
            install_panel_frontend(world, entity, panel)?;
        }
        Err(error) => {
            world.get_mut::<PanelRecord>(entity).unwrap().unavailable = Some(error);
        }
    }
    Ok(entity)
}

/// Install or replace the sole frontend for an existing workbench panel. The
/// returned previous object belongs to the caller, for example an inactive
/// viewer presentation. Feature components and live identity are unchanged.
pub fn install_panel_frontend(
    world: &mut World,
    entity: Entity,
    panel: Box<dyn Panel>,
) -> Result<Option<Box<dyn Panel>>, String> {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "frontends are replaced between frontend scopes"
    );
    if world.get::<PanelRecord>(entity).is_none() || api::panel_owner(world, entity).is_none() {
        return Err("Panel is not owned by this workbench".into());
    }
    world.get_mut::<PanelRecord>(entity).unwrap().unavailable = None;
    Ok(world.non_send_mut::<PanelUiStore>().0.insert(entity, panel))
}

/// Transfer the frontend to the caller without closing its entity or layout.
/// Reinstall it or a replacement before that panel is drawn again.
pub fn take_panel_frontend(world: &mut World, entity: Entity) -> Option<Box<dyn Panel>> {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "frontends are taken between frontend scopes"
    );
    world.non_send_mut::<PanelUiStore>().0.remove(&entity)
}

fn with_panel_frontend<R>(
    world: &mut World,
    entity: Entity,
    operation: &str,
    callback: impl FnOnce(&mut dyn Panel, &mut World) -> R,
) -> Result<R, String> {
    let record = world
        .get::<PanelRecord>(entity)
        .ok_or("Panel is not owned by this workbench")?
        .clone();
    let mut panel = world
        .non_send_mut::<PanelUiStore>()
        .0
        .remove(&entity)
        .ok_or_else(|| {
            record
                .unavailable
                .clone()
                .unwrap_or_else(|| "Panel frontend is unavailable".into())
        })?;
    // The default compositor already holds this guard. Single-panel helpers
    // also establish it so registered systems cannot run during a callback.
    let owns_loan = !world.contains_resource::<UiLoan>();
    if owns_loan {
        world.insert_resource(UiLoan);
    }
    let result = catch_unwind(AssertUnwindSafe(|| callback(panel.as_mut(), world)));
    let result = match result {
        Ok(value) => Ok(value),
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-text panic payload");
            let error = format!(
                "Panel {} panicked during {operation}: {detail}",
                record.panel_type
            );
            panel = Box::new(FailedPanel {
                _damaged: panel,
                title: record.panel_type,
                state: record.serialized["state"].clone(),
                error: error.clone(),
            });
            world.get_mut::<PanelRecord>(entity).unwrap().unavailable = Some(error.clone());
            Err(error)
        }
    };
    world.non_send_mut::<PanelUiStore>().0.insert(entity, panel);
    if owns_loan {
        world.remove_resource::<UiLoan>().unwrap();
    }
    result
}

/// Inspect a frontend title without removing its owned live panel. A failed
/// callback is quarantined, allowing the compositor to show its type label.
pub fn panel_title(world: &mut World, entity: Entity) -> Result<String, String> {
    let entity = world
        .get::<api::PanelFrontend>(entity)
        .map(|frontend| frontend.0)
        .unwrap_or(entity);
    if !world.non_send::<PanelUiStore>().0.contains_key(&entity)
        && let Some(record) = world
            .get::<PanelRecord>(entity)
            .filter(|record| record.unavailable.is_some())
    {
        return Ok(record.panel_type.clone());
    }
    with_panel_frontend(world, entity, "title", |panel, world| panel.title(world))
}

/// Capture the latest valid presentation state. Quarantine preserves this
/// snapshot and the rest of the opaque persisted record if a callback panics.
pub fn panel_state(world: &mut World, entity: Entity) -> Result<Value, String> {
    let entity = world
        .get::<api::PanelFrontend>(entity)
        .map(|frontend| frontend.0)
        .unwrap_or(entity);
    if !world.non_send::<PanelUiStore>().0.contains_key(&entity)
        && let Some(record) = world
            .get::<PanelRecord>(entity)
            .filter(|record| record.unavailable.is_some())
    {
        return Ok(record.serialized["state"].clone());
    }
    let state = with_panel_frontend(world, entity, "save", |panel, world| panel.save(world))?;
    world.get_mut::<PanelRecord>(entity).unwrap().serialized["state"] = state.clone();
    Ok(state)
}

/// Draw one owned surface for an application compositor. Returned intentions
/// have not run. A panicking callback loses its partial intentions, restores
/// ImGui's entry stacks and becomes a quarantined error presenter.
pub fn render_panel(
    world: &mut World,
    entity: Entity,
    ui: &Ui,
) -> Result<Vec<WorkbenchAction>, String> {
    // The presenter may hold view state that is newer than the loaded record.
    // Save its entry state before allowing a failing draw to damage it.
    panel_state(world, entity)?;
    prepare_shortcuts(world, ui);
    let scope = world
        .get::<PanelRecord>(entity)
        .map(|record| record.panel_type.clone())
        .unwrap_or_default();
    let input = world
        .non_send::<WorkbenchShortcuts>()
        .input
        .with_scope(api::ShortcutScope::panel(scope));
    if ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::ROOT_AND_CHILD_WINDOWS) {
        let area = api::panel_owner(world, entity)
            .and_then(|owner| world.get::<PanelPresentation>(owner))
            .and_then(|p| world.resource::<WorkbenchLayout>().tiling.area_for(p.id))
            .unwrap_or(0);
        world.non_send_mut::<WorkbenchShortcuts>().focus = Some((entity, area));
    }
    ui.with_bound_context(|| {
        let mut stacks = dear_imgui_rs::sys::ImGuiErrorRecoveryState::default();
        unsafe { dear_imgui_rs::sys::igErrorRecoveryStoreState(&mut stacks) };
        let result = with_panel_frontend(world, entity, "draw", |panel, world| {
            let mut requests = Vec::new();
            panel.draw(ui, world, &mut requests, &input);
            requests
        });
        if result.is_err() {
            // Rust scopes unwind first; recover any raw ImGui scopes left by
            // the feature before returning to its compositor's own window.
            unsafe {
                let io = dear_imgui_rs::sys::igGetIO_Nil();
                let assert = (*io).ConfigErrorRecoveryEnableAssert;
                (*io).ConfigErrorRecoveryEnableAssert = false;
                dear_imgui_rs::sys::igErrorRecoveryTryToRecoverState(&stacks);
                (*io).ConfigErrorRecoveryEnableAssert = assert;
            }
        }
        result
    })
}

/// Save all panel records. Unavailable features retain their complete opaque
/// serialized input so rebuilding with that feature restores its original view.
pub fn save(world: &mut World) -> Value {
    let panels = world
        .query_filtered::<Entity, (With<WorkbenchPanel>, Without<api::PanelSuspended>)>()
        .iter(world)
        .collect::<Vec<_>>();
    let layout = world.resource::<WorkbenchLayout>().clone();
    save_layout(world, panels, &layout)
}

/// Serialize one retained layout without activating its suspended panels.
pub fn save_layout(world: &mut World, panels: Vec<Entity>, layout: &WorkbenchLayout) -> Value {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "save runs between frontend scopes"
    );
    let records = panels
        .into_iter()
        .filter_map(|entity| {
            let panel = world.get::<PanelPresentation>(entity)?;
            let record = world.get::<PanelRecord>(entity)?;
            Some((entity, panel.id, record.serialized.clone()))
        })
        .collect::<Vec<_>>();
    let mut panels = Vec::new();
    for (entity, id, mut value) in records {
        let surface = world
            .get::<api::PanelFrontend>(entity)
            .map(|frontend| frontend.0)
            .unwrap_or(entity);
        if world
            .get::<api::PanelPersistence>(surface)
            .is_some_and(|policy| !policy.0)
        {
            continue;
        }
        value["id"] = json!(id);
        // Save from the active presentation, retaining opaque data when a
        // feature is absent or its frontend has been quarantined.
        value["state"] = panel_state(world, entity).unwrap_or_else(|_| value["state"].clone());
        panels.push(value);
    }
    panels.sort_by_key(|value| value["id"].as_u64());
    json!({"version":1,"panels":panels,"focused_area":layout.focused_area,
        "tiling":layout.tiling.to_value(panels.iter().filter_map(|panel| panel["id"].as_u64())),
        "focused":layout.focused.and_then(|entity| world.get::<PanelPresentation>(entity)).map(|panel| panel.id).filter(|id| panels.iter().any(|panel| panel["id"].as_u64() == Some(*id)))})
}

/// Validate stored IDs and layout before replacing any live content.
pub fn restore(world: &mut World, value: &Value) -> Result<(), String> {
    assert!(
        !world.contains_resource::<UiLoan>(),
        "restore runs between frontend scopes"
    );
    if value["version"].as_u64() != Some(1) {
        return Err("Unsupported workbench version".into());
    }
    let panels = value["panels"]
        .as_array()
        .ok_or("Saved panels must be an array")?;
    let mut ids = HashSet::new();
    for panel in panels {
        let id = panel["id"]
            .as_u64()
            .filter(|id| *id > 0 && *id <= u32::MAX as u64)
            .ok_or("Invalid saved panel ID")?;
        if !ids.insert(id) {
            return Err("Duplicate saved panel ID".into());
        }
        if panel["panel_type"]
            .as_str()
            .is_none_or(|value| value.is_empty())
        {
            return Err("Saved panel has no type".into());
        }
    }
    if !value["focused"].is_null()
        && value["focused"]
            .as_u64()
            .is_none_or(|id| !ids.contains(&id))
    {
        return Err("Saved focused panel is unavailable".into());
    }
    let tiling =
        TilingState::from_value(&value["tiling"], ids.iter().copied()).map_err(str::to_owned)?;
    let old = world
        .query_filtered::<Entity, (With<PanelRecord>, With<PanelPresentation>)>()
        .iter(world)
        .collect::<Vec<_>>();
    // Preflight every old panel before removing any. Deferred saves must not
    // leave a half-replaced workspace or destroy a previously approved panel.
    if !preflight_closes(world, &old)? {
        return Err("Workbench restore is waiting for a panel to close".into());
    }
    for entity in old {
        commit_close(world, entity);
    }
    let next_id = ids.iter().max().copied().unwrap_or(0) + 1;
    *world.resource_mut::<WorkbenchLayout>() = WorkbenchLayout {
        tiling: tiling.clone(),
        focused: None,
        focused_area: None,
        next_id,
    };
    for record in panels {
        let id = record["id"].as_u64().unwrap();
        open_panel(world, record.clone(), tiling.area_for(id), Some(id))?;
    }
    // Opening factories must not change the saved tab selections.
    let selections = tiling
        .areas
        .iter()
        .filter_map(|group| group.selected)
        .collect::<HashSet<_>>();
    for mut panel in world
        .query_filtered::<&mut PanelPresentation, With<PanelRecord>>()
        .iter_mut(world)
    {
        panel.focus = selections.contains(&panel.id);
    }
    world.resource_mut::<WorkbenchLayout>().tiling = tiling;
    let focus = value["focused"].as_u64().and_then(|id| {
        world
            .query::<(Entity, &PanelPresentation, &PanelRecord)>()
            .iter(world)
            .find_map(|(entity, panel, _)| (panel.id == id).then_some(entity))
    });
    world.resource_mut::<WorkbenchLayout>().focused = focus;
    let focused_area = focus
        .and_then(|entity| world.get::<PanelPresentation>(entity))
        .and_then(|panel| {
            world
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(panel.id)
        })
        .or_else(|| {
            value["focused_area"]
                .as_u64()
                .and_then(|area| u32::try_from(area).ok())
                .filter(|area| {
                    world
                        .resource::<WorkbenchLayout>()
                        .tiling
                        .areas
                        .iter()
                        .any(|group| group.area == *area)
                })
        });
    world.resource_mut::<WorkbenchLayout>().focused_area = focused_area;
    if let Some(entity) = focus {
        let mut history = world.resource_mut::<PanelFocusHistory>();
        history.0.retain(|panel| *panel != entity);
        history.0.push(entity);
    }
    Ok(())
}

#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod tests;
