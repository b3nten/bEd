//! Stable contextual command identities with independently scoped bindings and handlers.
use crate::{Area, InWorkspace, Layout, PanelInstance, Workbench, WorkspaceId};
use bevy::{ecs::system::SystemId, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    ops::BitOr,
};

pub trait ContextCommand: Send + Sync + 'static {
    const KEY: &'static str;
    const LABEL: &'static str;

    /// Compose application behavior directly on a panel/workspace/entity.
    fn handler(
        call: fn(ContextCommandInvocation, &mut World) -> ContextCommandResult,
    ) -> CommandHandler<Self>
    where
        Self: Sized,
    {
        CommandHandler::new(call)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextCommandId(Cow<'static, str>);
impl ContextCommandId {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl ContextCommandId {
    pub fn of<C: ContextCommand>() -> Self {
        Self(Cow::Borrowed(C::KEY))
    }
}

#[derive(Component)]
pub struct ContextCommandInfo {
    command: ContextCommandId,
    pub label: String,
}

impl ContextCommandInfo {
    pub fn command(&self) -> ContextCommandId {
        self.command.clone()
    }
}

/// A placement names a contextual command. Handlers may be replaced or scoped without
/// rewriting its menus, buttons or keybindings.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Invokes(pub ContextCommandId);

impl Invokes {
    pub fn of<C: ContextCommand>() -> Self {
        Self(ContextCommandId::of::<C>())
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct ActionEnabled(pub bool);
impl Default for ActionEnabled {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Component)]
#[require(ActionEnabled)]
#[component(on_insert = handler_inserted)]
pub struct Handler {
    command: ContextCommandId,
    generation: u64,
}
fn next_handler_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |value| value.checked_add(1),
    )
    .expect("Handler generation exhausted")
}
fn handler_inserted(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    world.get_mut::<Handler>(context.entity).unwrap().generation = next_handler_generation();
}
/// Ownership changes invalidate already-admitted invocations, even if a handler
/// is moved away and back before the batch reaches it.
pub(crate) fn handler_scope_changed(world: &mut bevy::ecs::world::DeferredWorld, entity: Entity) {
    if let Some(mut handler) = world.get_mut::<Handler>(entity) {
        handler.generation = next_handler_generation();
    }
    if let Some(mut handlers) = world.get_mut::<AttachedHandlers>(entity) {
        for handler in &mut handlers.0 {
            handler.generation = next_handler_generation();
        }
    }
}

/// A handler attached directly to its panel or workspace. Ordinary unscoped
/// entities provide world handlers. Construction installs no command definition
/// and executes no application code.
#[derive(Component)]
#[require(AttachedHandlers)]
#[component(on_insert = attached_inserted::<C>, on_remove = attached_removed::<C>)]
pub struct CommandHandler<C: ContextCommand> {
    pub enabled: bool,
    call: fn(ContextCommandInvocation, &mut World) -> ContextCommandResult,
    marker: std::marker::PhantomData<fn() -> C>,
}
impl<C: ContextCommand> CommandHandler<C> {
    pub fn new(call: fn(ContextCommandInvocation, &mut World) -> ContextCommandResult) -> Self {
        Self {
            enabled: true,
            call,
            marker: std::marker::PhantomData,
        }
    }
}
#[derive(Component, Default)]
struct AttachedHandlers(Vec<AttachedHandler>);
#[derive(Clone)]
struct AttachedHandler {
    command: ContextCommandId,
    component: std::any::TypeId,
    generation: u64,
    enabled: fn(&World, Entity) -> bool,
    call: fn(&mut World, Entity, ContextCommandInvocation) -> ContextCommandResult,
}
fn attached_inserted<C: ContextCommand>(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    let mut entries = world
        .get_mut::<AttachedHandlers>(context.entity)
        .expect("Required handler index");
    assert!(
        !entries
            .0
            .iter()
            .any(|entry| entry.command == ContextCommandId::of::<C>()
                && entry.component != std::any::TypeId::of::<C>()),
        "Two attached command types share an identity"
    );
    entries
        .0
        .retain(|entry| entry.command != ContextCommandId::of::<C>());
    entries.0.push(AttachedHandler {
        command: ContextCommandId::of::<C>(),
        component: std::any::TypeId::of::<C>(),
        generation: next_handler_generation(),
        enabled: |world, entity| {
            world
                .get::<CommandHandler<C>>(entity)
                .is_some_and(|h| h.enabled)
        },
        call: |world, entity, invocation| {
            let call = world
                .get::<CommandHandler<C>>(entity)
                .ok_or("Attached handler is unavailable")?
                .call;
            call(invocation, world)
        },
    });
}
fn attached_removed<C: ContextCommand>(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    if let Some(mut entries) = world.get_mut::<AttachedHandlers>(context.entity) {
        entries
            .0
            .retain(|entry| entry.command != ContextCommandId::of::<C>());
    }
}

#[derive(Component, Debug)]
#[component(on_insert = crate::membership::member_changed, on_remove = crate::membership::member_changed)]
#[relationship(relationship_target = PanelCommandParts)]
pub struct InPanel(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InPanel, linked_spawn)]
pub struct PanelCommandParts(Vec<Entity>);
impl PanelCommandParts {
    pub(crate) fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InvocationContext {
    #[default]
    Focused,
    Global,
    Panel(Entity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCommandRequest {
    pub command: ContextCommandId,
    pub source: Option<Entity>,
    pub context: InvocationContext,
}

impl ContextCommandRequest {
    pub fn new<C: ContextCommand>() -> Self {
        Self {
            command: ContextCommandId::of::<C>(),
            source: None,
            context: InvocationContext::Focused,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCommandInvocation {
    /// Workspace captured at admission; world-only invocations need none.
    pub workspace: Option<WorkspaceId>,
    /// The requested origin, retained independently of the resolved panel.
    pub context: InvocationContext,
    pub command: ContextCommandId,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
}

pub type ContextCommandResult = Result<(), String>;

/// Requests owned by a workspace.
#[derive(Component, Default)]
pub struct ContextCommandRequests(VecDeque<ContextCommandRequest>);

/// Application-wide requests, including commands with no workspace. This is a
/// distinct resource: Bevy resources carry singleton ownership and cannot also
/// serve as per-workspace components.
#[derive(Resource, Default)]
pub struct GlobalCommandRequests(ContextCommandRequests);
impl std::ops::Deref for GlobalCommandRequests {
    type Target = ContextCommandRequests;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for GlobalCommandRequests {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl ContextCommandRequests {
    /// Whether all submitted requests have been admitted by dispatch. Hosts can
    /// use this fact when placing a shutdown barrier after admitted follow-ups.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn invoke<C: ContextCommand>(&mut self) {
        self.invoke_id(ContextCommandId::of::<C>());
    }
    pub fn invoke_id(&mut self, command: ContextCommandId) {
        self.submit(ContextCommandRequest {
            command,
            source: None,
            context: InvocationContext::Focused,
        });
    }
    pub fn submit(&mut self, request: ContextCommandRequest) {
        self.0.push_back(request);
    }
}

#[derive(Message, Clone, Debug)]
pub struct ContextCommandFinished {
    pub invocation: ContextCommandInvocation,
    pub result: ContextCommandResult,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextCommandSystems {
    CollectInput,
    Shortcuts,
    Dispatch,
}

/// Portable modifier data; no native menu or egui types occur in the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);
impl Modifiers {
    pub const CONTROL: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const META: Self = Self(8);
    pub const PRIMARY: Self = Self(16);
    pub fn contains(self, rhs: Self) -> bool {
        self.0 & rhs.0 == rhs.0
    }
    pub fn resolved(self) -> Self {
        Self(
            (self.0 & !16)
                | if self.contains(Self::PRIMARY) {
                    if cfg!(target_os = "macos") { 8 } else { 1 }
                } else {
                    0
                },
        )
    }
}

impl BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: KeyCode,
}

impl KeyChord {
    pub fn new(modifiers: Modifiers, key: KeyCode) -> Self {
        Self { modifiers, key }
    }
    pub fn label(self) -> String {
        let m = self.modifiers.resolved();
        let mut s = String::new();
        for (flag, label) in [
            (Modifiers::CONTROL, "Ctrl+"),
            (
                Modifiers::META,
                if cfg!(target_os = "macos") {
                    "Cmd+"
                } else {
                    "Super+"
                },
            ),
            (Modifiers::ALT, "Alt+"),
            (Modifiers::SHIFT, "Shift+"),
        ] {
            if m.contains(flag) {
                s.push_str(label);
            }
        }
        let name = format!("{:?}", self.key);
        s.push_str(
            name.strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name),
        );
        s
    }
}

#[derive(Component)]
pub struct Binding {
    pub command: ContextCommandId,
    pub chord: KeyChord,
    pub repeat: bool,
}

impl Binding {
    pub fn new<C: ContextCommand>(chord: KeyChord) -> Self {
        Self {
            command: ContextCommandId::of::<C>(),
            chord,
            repeat: false,
        }
    }
}

/// Normalized chord ingress, independent of any UI backend. UI adapters admit
/// shortcut presses; headless hosts can provide the same input directly.
#[derive(Clone, Copy, Debug)]
pub struct BindingPress {
    pub chord: KeyChord,
    pub repeat: bool,
    pub context: InvocationContext,
}
#[derive(Resource, Default)]
pub struct BindingInput(pub Vec<BindingPress>);
#[derive(Message, Clone, Debug)]
pub struct BindingConflict {
    pub chord: KeyChord,
    pub sources: Vec<Entity>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<BindingInput>()
        .init_resource::<GlobalCommandRequests>()
        .add_message::<ContextCommandFinished>()
        .add_message::<BindingConflict>()
        .configure_sets(
            PreUpdate,
            (
                ContextCommandSystems::CollectInput.after(crate::WorkspaceSystems::Prepare),
                ContextCommandSystems::Shortcuts,
                ContextCommandSystems::Dispatch,
            )
                .chain(),
        )
        .add_systems(
            PreUpdate,
            shortcuts.in_set(ContextCommandSystems::Shortcuts),
        )
        .add_systems(PreUpdate, dispatch.in_set(ContextCommandSystems::Dispatch));
}

/// Register stable metadata only; installing a handler never registers definitions.
pub fn register_context_command<C: ContextCommand>(
    world: &mut World,
) -> Result<ContextCommandId, String> {
    if !crate::valid_key(C::KEY) || C::LABEL.trim().is_empty() {
        return Err("ContextCommand requires a namespaced key and nonempty label".into());
    }
    let command = ContextCommandId::of::<C>();
    for info in world.query::<&ContextCommandInfo>().iter(world) {
        if info.command == command {
            return if info.label == C::LABEL {
                Ok(command)
            } else {
                Err(format!(
                    "Command key {:?} is already registered with different metadata",
                    C::KEY
                ))
            };
        }
    }
    world.spawn(ContextCommandInfo {
        command: command.clone(),
        label: C::LABEL.into(),
    });
    Ok(command)
}

/// Install a world handler, independently of command metadata or workspace selection.
pub fn install_global_handler<C: ContextCommand, M>(
    world: &mut World,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    install_handler::<C, M>(world, CommandScope::World, handler)
}

/// Install a handler belonging to one workspace, including an inactive workspace.
pub fn install_workspace_handler<C: ContextCommand, M>(
    world: &mut World,
    workspace: WorkspaceId,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    crate::workspaces::validate(world, workspace)?;
    install_handler::<C, M>(world, CommandScope::Workspace(workspace), handler)
}

/// Install a panel handler. Panel ownership supplies its workspace and lifetime.
pub fn install_panel_handler<C: ContextCommand, M>(
    world: &mut World,
    panel: Entity,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    world
        .get::<PanelInstance>(panel)
        .ok_or("Panel is unavailable")?;
    let workspace = crate::workspaces::owner(world, panel).ok_or("Panel has no workspace")?;
    crate::workspaces::validate(world, workspace)?;
    install_handler::<C, M>(world, CommandScope::Panel(panel), handler)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandScope {
    World,
    Workspace(WorkspaceId),
    Panel(Entity),
}

fn install_handler<C: ContextCommand, M>(
    world: &mut World,
    scope: CommandScope,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    let command = ContextCommandId::of::<C>();
    if find_handler(world, command.clone(), scope).is_some() {
        return Err("Command already has a handler in this scope".into());
    }
    let entity = world.register_system(handler).entity();
    world.entity_mut(entity).insert(Handler {
        command,
        generation: 0,
    });
    match scope {
        CommandScope::World => {}
        CommandScope::Workspace(workspace) => {
            world.entity_mut(entity).insert(InWorkspace(workspace.0));
        }
        CommandScope::Panel(panel) => {
            let workspace = crate::workspaces::owner(world, panel).expect("Validated panel owner");
            world
                .entity_mut(entity)
                .insert((InPanel(panel), InWorkspace(workspace.0)));
        }
    }
    Ok(entity)
}

impl Workbench<'_> {
    pub fn register_context_command<C: ContextCommand>(
        &mut self,
    ) -> Result<ContextCommandId, String> {
        register_context_command::<C>(self.world)
    }

    /// Register in this workspace, never implicitly in the world scope.
    pub fn register_handler<C: ContextCommand, M>(
        &mut self,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<ContextCommandId, String> {
        self.register_workspace_handler::<C, M>(handler)?;
        Ok(ContextCommandId::of::<C>())
    }

    pub fn register_workspace_handler<C: ContextCommand, M>(
        &mut self,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        install_workspace_handler::<C, M>(self.world, self.workspace, handler)
    }

    pub fn register_panel_handler<C: ContextCommand, M>(
        &mut self,
        panel: Entity,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        self.require_owned(panel)?;
        install_panel_handler::<C, M>(self.world, panel, handler)
    }

    pub fn command<C: ContextCommand>(&mut self) -> Option<ContextCommandId> {
        let command = ContextCommandId::of::<C>();
        self.world
            .query::<&ContextCommandInfo>()
            .iter(self.world)
            .any(|i| i.command == command)
            .then_some(command)
    }

    pub fn command_by_key(&mut self, key: &str) -> Option<ContextCommandId> {
        self.world
            .query::<&ContextCommandInfo>()
            .iter(self.world)
            .find_map(|i| (i.command.as_str() == key).then(|| i.command.clone()))
    }

    pub fn handler(&mut self, command: ContextCommandId, panel: Option<Entity>) -> Option<Entity> {
        let scope = panel.map_or(CommandScope::Workspace(self.workspace), CommandScope::Panel);
        find_handler(self.world, command, scope)
    }
}

fn scope(panel: Option<&InPanel>, workspace: Option<&InWorkspace>) -> CommandScope {
    match (panel, workspace) {
        (Some(panel), _) => CommandScope::Panel(panel.0),
        (None, Some(workspace)) => CommandScope::Workspace(WorkspaceId(workspace.0)),
        (None, None) => CommandScope::World,
    }
}

pub(crate) fn global_handler_exists(world: &mut World, command: ContextCommandId) -> bool {
    find_handler(world, command, CommandScope::World).is_some()
}

fn find_handler(
    world: &mut World,
    command: ContextCommandId,
    requested_scope: CommandScope,
) -> Option<Entity> {
    let registered: Vec<_> = world
        .query::<(Entity, &Handler, Option<&InPanel>, Option<&InWorkspace>)>()
        .iter(world)
        .filter_map(|(entity, h, panel, owner)| {
            (h.command == command && scope(panel, owner) == requested_scope).then_some(entity)
        })
        .collect();
    let attached: Vec<_> = world
        .query::<(
            Entity,
            &AttachedHandlers,
            Option<&InPanel>,
            Option<&InWorkspace>,
            Option<&PanelInstance>,
            Option<&crate::Workspace>,
        )>()
        .iter(world)
        .filter_map(|(entity, handlers, panel, owner, is_panel, is_workspace)| {
            let actual = if is_panel.is_some() {
                CommandScope::Panel(entity)
            } else if is_workspace.is_some() {
                CommandScope::Workspace(WorkspaceId(entity))
            } else {
                scope(panel, owner)
            };
            (actual == requested_scope && handlers.0.iter().any(|h| h.command == command))
                .then_some(entity)
        })
        .collect();
    let mut handlers = registered.into_iter().chain(attached);
    let handler = handlers.next();
    assert!(
        handlers.next().is_none(),
        "Multiple handlers for a command in one scope"
    );
    handler
}

#[cfg(feature = "ui")]
pub(crate) fn focused_panel(world: &World) -> Option<Entity> {
    focused_panel_in(world, crate::workspaces::selected(world)?)
}

pub(crate) fn focused_panel_in(world: &World, workspace: WorkspaceId) -> Option<Entity> {
    let layout = crate::active_layout(world, workspace.0)?;
    let area = world.get::<Layout>(layout)?.focused_area()?;
    world.get::<Area>(area)?.selected_panel()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdmittedHandler {
    entity: Entity,
    scope: CommandScope,
    generation: u64,
    attached: bool,
}
fn handler_snapshot(
    world: &World,
    entity: Entity,
    command: &ContextCommandId,
) -> Option<AdmittedHandler> {
    if let Some(handler) = world
        .get::<Handler>(entity)
        .filter(|handler| &handler.command == command)
    {
        Some(AdmittedHandler {
            entity,
            scope: scope(
                world.get::<InPanel>(entity),
                world.get::<InWorkspace>(entity),
            ),
            generation: handler.generation,
            attached: false,
        })
    } else {
        let handler = world
            .get::<AttachedHandlers>(entity)?
            .0
            .iter()
            .find(|h| &h.command == command)?;
        let actual = if world.get::<PanelInstance>(entity).is_some() {
            CommandScope::Panel(entity)
        } else if world.get::<crate::Workspace>(entity).is_some() {
            CommandScope::Workspace(WorkspaceId(entity))
        } else {
            scope(
                world.get::<InPanel>(entity),
                world.get::<InWorkspace>(entity),
            )
        };
        Some(AdmittedHandler {
            entity,
            scope: actual,
            generation: handler.generation,
            attached: true,
        })
    }
}

pub(crate) fn resolve(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    request: ContextCommandRequest,
) -> (ContextCommandInvocation, Result<AdmittedHandler, String>) {
    let panel = match request.context {
        InvocationContext::Focused => workspace.and_then(|w| focused_panel_in(world, w)),
        InvocationContext::Global => None,
        InvocationContext::Panel(p) => Some(p),
    };
    let invocation = ContextCommandInvocation {
        workspace,
        context: request.context,
        command: request.command.clone(),
        source: request.source,
        panel,
    };
    if workspace.is_some_and(|w| crate::workspaces::validate(world, w).is_err())
        || panel.is_some_and(|p| {
            workspace.is_none()
                || world.get::<PanelInstance>(p).is_none()
                || crate::workspaces::owner(world, p) != workspace
        })
    {
        return (invocation, Err("Target panel is unavailable".into()));
    }
    let handler = panel
        .and_then(|p| find_handler(world, request.command.clone(), CommandScope::Panel(p)))
        .or_else(|| {
            workspace.and_then(|w| {
                find_handler(world, request.command.clone(), CommandScope::Workspace(w))
            })
        })
        .or_else(|| find_handler(world, request.command.clone(), CommandScope::World));
    let result = handler
        .ok_or_else(|| "No handler for this action in the requested context".into())
        .and_then(|h| {
            if handler_enabled(world, h, &request.command) {
                handler_snapshot(world, h, &request.command)
                    .ok_or_else(|| "Command handler is unavailable".into())
            } else {
                Err("Command is disabled".into())
            }
        });
    (invocation, result)
}

fn handler_enabled(world: &World, entity: Entity, command: &ContextCommandId) -> bool {
    if world
        .get::<Handler>(entity)
        .is_some_and(|handler| &handler.command == command)
    {
        world
            .get::<ActionEnabled>(entity)
            .is_some_and(|enabled| enabled.0)
    } else {
        world
            .get::<AttachedHandlers>(entity)
            .and_then(|handlers| handlers.0.iter().find(|h| &h.command == command))
            .is_some_and(|handler| (handler.enabled)(world, entity))
    }
}

pub fn command_available(
    world: &mut World,
    command: ContextCommandId,
    context: InvocationContext,
) -> bool {
    let workspace = match context {
        InvocationContext::Panel(panel) => crate::workspaces::owner(world, panel),
        _ => crate::workspaces::selected(world),
    };
    command_available_in(world, workspace, command, context)
}

/// Query an explicit workspace (or only world scope) without selected-workspace APIs.
pub fn command_available_in(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    command: ContextCommandId,
    context: InvocationContext,
) -> bool {
    resolve(
        world,
        workspace,
        ContextCommandRequest {
            command,
            source: None,
            context,
        },
    )
    .1
    .is_ok()
}

#[derive(Clone)]
struct ResolvedBinding {
    source: Entity,
    command: ContextCommandId,
    chord: KeyChord,
    repeat: bool,
    depth: u8,
}

/// One shared overlay for shortcut execution and menu hints. Conflicts at the
/// nearest scope reserve the chord rather than exposing an outer binding.
fn binding_groups(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    context: InvocationContext,
) -> HashMap<KeyChord, Vec<ResolvedBinding>> {
    let panel = match context {
        InvocationContext::Focused => workspace.and_then(|w| focused_panel_in(world, w)),
        InvocationContext::Panel(p) => Some(p),
        InvocationContext::Global => None,
    };
    let mut groups: HashMap<KeyChord, Vec<ResolvedBinding>> = HashMap::new();
    for (source, b, p, owner) in world
        .query::<(Entity, &Binding, Option<&InPanel>, Option<&InWorkspace>)>()
        .iter(world)
    {
        let depth = match scope(p, owner) {
            CommandScope::World => 0,
            CommandScope::Workspace(w) if Some(w) == workspace => 1,
            CommandScope::Panel(p)
                if Some(p) == panel
                    && workspace.is_some()
                    && world.get::<PanelInstance>(p).is_some()
                    && crate::workspaces::owner(world, p) == workspace =>
            {
                2
            }
            _ => continue,
        };
        let chord = KeyChord::new(b.chord.modifiers.resolved(), b.chord.key);
        groups.entry(chord).or_default().push(ResolvedBinding {
            source,
            command: b.command.clone(),
            chord,
            repeat: b.repeat,
            depth,
        });
    }
    for group in groups.values_mut() {
        let nearest = group.iter().map(|b| b.depth).max().unwrap();
        group.retain(|b| b.depth == nearest);
        group.sort_by_key(|b| b.source.to_bits());
    }
    groups
}

pub(crate) fn shortcut_for(
    world: &mut World,
    workspace: Option<WorkspaceId>,
    command: ContextCommandId,
    context: InvocationContext,
) -> Option<KeyChord> {
    let mut bindings: Vec<_> = binding_groups(world, workspace, context)
        .into_values()
        .filter(|group| group.len() == 1)
        .flatten()
        .collect();
    bindings.sort_by_key(|b| b.source.to_bits());
    bindings
        .into_iter()
        .find_map(|b| (b.command == command).then_some(b.chord))
}

fn shortcuts(world: &mut World) {
    let presses = std::mem::take(&mut world.resource_mut::<BindingInput>().0);
    for press in presses {
        let workspace = match press.context {
            InvocationContext::Panel(panel) => crate::workspaces::owner(world, panel),
            _ => crate::workspaces::selected(world),
        };
        let chord = KeyChord::new(press.chord.modifiers.resolved(), press.chord.key);
        let Some(mut candidates) = binding_groups(world, workspace, press.context).remove(&chord)
        else {
            continue;
        };
        if candidates.len() > 1 {
            world.write_message(BindingConflict {
                chord,
                sources: candidates.iter().map(|c| c.source).collect(),
            });
            continue;
        }
        let binding = candidates.pop().unwrap();
        if !press.repeat || binding.repeat {
            let request = ContextCommandRequest {
                command: binding.command,
                source: Some(binding.source),
                context: press.context,
            };
            if let Some(workspace) = workspace {
                world
                    .get_mut::<ContextCommandRequests>(workspace.0)
                    .expect("Workspace command queue")
                    .submit(request);
            } else {
                world
                    .resource_mut::<GlobalCommandRequests>()
                    .submit(request);
            }
        }
    }
}

fn dispatch(world: &mut World) {
    let mut pending: Vec<_> = world
        .query_filtered::<(Entity, &mut ContextCommandRequests), With<crate::Workspace>>()
        .iter_mut(world)
        .flat_map(|(entity, mut requests)| {
            std::mem::take(&mut requests.0)
                .into_iter()
                .map(move |r| (Some(WorkspaceId(entity)), r))
        })
        .collect();
    let requests = std::mem::take(&mut world.resource_mut::<GlobalCommandRequests>().0.0);
    for request in requests {
        let workspace = match request.context {
            InvocationContext::Panel(panel) => crate::workspaces::owner(world, panel),
            _ => crate::workspaces::selected(world),
        };
        pending.push((workspace, request));
    }
    // Capture origin and targets before any handler changes focus or activation.
    let admitted: Vec<_> = pending
        .into_iter()
        .map(|(workspace, r)| resolve(world, workspace, r))
        .collect();
    for (invocation, handler) in admitted {
        let result = handler.and_then(|admitted| {
            let entity = admitted.entity;
            if handler_snapshot(world, entity, &invocation.command) != Some(admitted) {
                return Err("Admitted command handler was removed, replaced, or retargeted".into());
            }
            if invocation
                .workspace
                .is_some_and(|w| crate::workspaces::validate(world, w).is_err())
                || invocation.panel.is_some_and(|p| {
                    invocation.workspace.is_none()
                        || world.get::<PanelInstance>(p).is_none()
                        || crate::workspaces::owner(world, p) != invocation.workspace
                })
            {
                return Err("Admitted command target is unavailable".into());
            }
            if !handler_enabled(world, entity, &invocation.command) {
                return Err("Command handler is unavailable or disabled".into());
            }
            if let Some(call) = world
                .get::<AttachedHandlers>(entity)
                .and_then(|handlers| handlers.0.iter().find(|h| h.command == invocation.command))
                .map(|h| h.call)
            {
                return call(world, entity, invocation.clone());
            }
            world
                .run_system_with(
                    SystemId::<In<ContextCommandInvocation>, ContextCommandResult>::from_entity(
                        entity,
                    ),
                    invocation.clone(),
                )
                .map_err(|e| e.to_string())
                .and_then(|r| r)
        });
        world.write_message(ContextCommandFinished { invocation, result });
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::{WorkbenchCorePlugin, Workspaces};

    struct Save;
    impl ContextCommand for Save {
        const KEY: &'static str = "scope-tests.save";
        const LABEL: &'static str = "Save";
    }
    struct Other;
    impl ContextCommand for Other {
        const KEY: &'static str = "scope-tests.other";
        const LABEL: &'static str = "Other";
    }
    #[derive(Resource, Default)]
    struct Calls(Vec<ContextCommandInvocation>);
    fn record(
        input: In<ContextCommandInvocation>,
        mut calls: ResMut<Calls>,
    ) -> ContextCommandResult {
        calls.0.push(input.0);
        Ok(())
    }
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Calls>();
        app
    }
    fn request() -> ContextCommandRequest {
        ContextCommandRequest::new::<Save>()
    }
    fn chord() -> KeyChord {
        KeyChord::new(Modifiers::CONTROL, KeyCode::KeyS)
    }
    fn press(world: &mut World, context: InvocationContext) {
        world.resource_mut::<BindingInput>().0.push(BindingPress {
            chord: chord(),
            repeat: false,
            context,
        });
        shortcuts(world);
        dispatch(world);
    }

    #[test]
    fn world_commands_and_shortcuts_need_no_workspace_or_implicit_definition() {
        let mut app = app();
        let world = app.world_mut();
        install_global_handler::<Save, _>(world, record).unwrap();
        assert!(
            !world
                .query::<&ContextCommandInfo>()
                .iter(world)
                .any(|info| info.command() == ContextCommandId::of::<Save>())
        );
        register_context_command::<Save>(world).unwrap();
        assert!(command_available(
            world,
            ContextCommandId::of::<Save>(),
            InvocationContext::Focused
        ));
        world
            .resource_mut::<GlobalCommandRequests>()
            .submit(request());
        dispatch(world);
        world.spawn(Binding::new::<Save>(chord()));
        press(world, InvocationContext::Focused);
        let calls = &world.resource::<Calls>().0;
        assert_eq!(calls.len(), 2);
        assert!(
            calls
                .iter()
                .all(|invocation| invocation.workspace.is_none() && invocation.panel.is_none())
        );
        assert_eq!(calls[0].context, InvocationContext::Focused);
    }

    #[test]
    fn handler_scopes_are_exact_and_disabled_nearest_blocks_fallback() {
        let mut app = app();
        let world = app.world_mut();
        let first = Workspaces::new(world).open("First");
        let second = Workspaces::new(world).open("Second");
        let mut w = Workbench::with_workspace(world, first).unwrap();
        let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
        let global = install_global_handler::<Save, _>(world, record).unwrap();
        let local = install_workspace_handler::<Save, _>(world, first, record).unwrap();
        let other = install_workspace_handler::<Save, _>(world, second, record).unwrap();
        let panel_handler = install_panel_handler::<Save, _>(world, panel, record).unwrap();
        assert!(install_workspace_handler::<Save, _>(world, first, record).is_err());
        assert!(install_panel_handler::<Save, _>(world, panel, record).is_err());
        let mut r = request();
        r.context = InvocationContext::Panel(panel);
        assert_eq!(
            resolve(world, Some(first), r.clone()).1.unwrap().entity,
            panel_handler
        );
        world.get_mut::<ActionEnabled>(panel_handler).unwrap().0 = false;
        assert!(resolve(world, Some(first), r.clone()).1.is_err());
        world.despawn(panel_handler);
        assert_eq!(
            resolve(world, Some(first), r.clone()).1.unwrap().entity,
            local
        );
        world.get_mut::<ActionEnabled>(local).unwrap().0 = false;
        assert!(resolve(world, Some(first), r.clone()).1.is_err());
        world.despawn(local);
        assert_eq!(resolve(world, Some(first), r).1.unwrap().entity, global);
        assert_eq!(
            resolve(world, Some(second), request()).1.unwrap().entity,
            other
        );
        assert_eq!(resolve(world, None, request()).1.unwrap().entity, global);
    }

    #[test]
    fn bindings_overlay_world_workspace_panel_and_nearest_conflicts_reserve_input() {
        let mut app = app();
        let world = app.world_mut();
        let workspace = Workspaces::new(world).open("Project");
        let unrelated = Workspaces::new(world).open("Unrelated");
        let mut w = Workbench::with_workspace(world, workspace).unwrap();
        let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
        install_global_handler::<Save, _>(world, record).unwrap();
        install_global_handler::<Other, _>(world, record).unwrap();
        world.spawn(Binding::new::<Other>(chord()));
        let local = world
            .spawn((Binding::new::<Save>(chord()), InWorkspace(workspace.0)))
            .id();
        world.spawn((Binding::new::<Other>(chord()), InWorkspace(unrelated.0)));
        let context = InvocationContext::Panel(panel);
        press(world, context);
        assert_eq!(
            world.resource::<Calls>().0.last().unwrap().command,
            ContextCommandId::of::<Save>()
        );
        let panel_binding = world
            .spawn((Binding::new::<Other>(chord()), InPanel(panel)))
            .id();
        press(world, context);
        assert_eq!(
            world.resource::<Calls>().0.last().unwrap().command,
            ContextCommandId::of::<Other>()
        );
        let conflict = world
            .spawn((Binding::new::<Save>(chord()), InPanel(panel)))
            .id();
        let before = world.resource::<Calls>().0.len();
        press(world, context);
        assert_eq!(world.resource::<Calls>().0.len(), before);
        assert!(
            shortcut_for(
                world,
                Some(workspace),
                ContextCommandId::of::<Save>(),
                context
            )
            .is_none()
        );
        assert!(
            shortcut_for(
                world,
                Some(workspace),
                ContextCommandId::of::<Other>(),
                context
            )
            .is_none()
        );
        let conflicts: Vec<_> = world
            .resource_mut::<Messages<BindingConflict>>()
            .drain()
            .collect();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].sources.len(), 2);
        world.despawn(conflict);
        world.despawn(panel_binding);
        world.despawn(local);
        press(world, context);
        assert_eq!(
            world.resource::<Calls>().0.last().unwrap().command,
            ContextCommandId::of::<Other>()
        );
        assert_eq!(
            world.resource::<Calls>().0.last().unwrap().workspace,
            Some(workspace)
        );
    }

    #[test]
    fn dispatch_captures_all_origins_before_handlers_change_workspace_selection() {
        let mut app = app();
        let world = app.world_mut();
        let origin = Workspaces::new(world).open("Origin");
        let destination = Workspaces::new(world).open("Destination");
        install_global_handler::<Save, _>(
            world,
            move |input: In<ContextCommandInvocation>, world: &mut World| -> ContextCommandResult {
                Workspaces::new(world).activate(destination)?;
                world.resource_mut::<Calls>().0.push(input.0);
                Ok(())
            },
        )
        .unwrap();
        world
            .resource_mut::<GlobalCommandRequests>()
            .invoke::<Save>();
        world
            .resource_mut::<GlobalCommandRequests>()
            .invoke::<Save>();
        dispatch(world);
        let calls = &world.resource::<Calls>().0;
        assert_eq!(calls.len(), 2);
        assert!(
            calls
                .iter()
                .all(|invocation| invocation.workspace == Some(origin))
        );
        assert_eq!(crate::workspaces::selected(world), Some(destination));
    }

    #[test]
    fn document_definitions_are_standard_but_behavior_is_opt_in() {
        let mut app = app();
        let world = app.world_mut();
        let save = ContextCommandId::of::<crate::SaveDocument>();
        assert!(
            world
                .query::<&ContextCommandInfo>()
                .iter(world)
                .any(|info| info.command() == save)
        );
        assert!(!command_available(
            world,
            save.clone(),
            InvocationContext::Global
        ));
        crate::register_document_handlers(world).unwrap();
        assert!(command_available(world, save, InvocationContext::Global));
        assert!(crate::register_document_handlers(world).is_err());
    }
}

#[cfg(test)]
mod attached_tests {
    use super::*;
    use crate::*;
    struct New;
    impl ContextCommand for New {
        const KEY: &'static str = "attached.new";
        const LABEL: &'static str = "New";
    }
    #[derive(Component, Default)]
    struct Counter(u32);
    fn increment(invocation: ContextCommandInvocation, world: &mut World) -> ContextCommandResult {
        world
            .get_mut::<Counter>(invocation.panel.ok_or("No panel")?)
            .ok_or("No counter")?
            .0 += 1;
        Ok(())
    }
    #[test]
    fn attached_handlers_are_immediately_available_and_removed_without_a_later_pass() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let workspace = Workspaces::new(app.world_mut()).open("Attached");
        let mut workbench = Workbench::with_workspace(app.world_mut(), workspace).unwrap();
        let area = workbench
            .destination_area(workbench.active_layout())
            .unwrap();
        let panel = workbench
            .world_mut()
            .spawn((
                PanelInstance::default(),
                InArea(area),
                Counter::default(),
                CommandHandler::<New>::new(increment),
            ))
            .id();
        assert!(command_available(
            workbench.world_mut(),
            ContextCommandId::of::<New>(),
            InvocationContext::Panel(panel)
        ));
        workbench.commands().invoke::<New>();
        app.update();
        assert_eq!(app.world().get::<Counter>(panel).unwrap().0, 1);
        app.world_mut()
            .get_mut::<CommandHandler<New>>(panel)
            .unwrap()
            .enabled = false;
        assert!(!command_available(
            app.world_mut(),
            ContextCommandId::of::<New>(),
            InvocationContext::Panel(panel)
        ));
        app.world_mut()
            .entity_mut(panel)
            .remove::<CommandHandler<New>>();
        assert!(!command_available(
            app.world_mut(),
            ContextCommandId::of::<New>(),
            InvocationContext::Panel(panel)
        ));
        assert!(app.world().get::<Counter>(panel).is_some());
    }

    #[test]
    fn registered_panel_handlers_follow_the_panels_owner_before_old_workspace_despawn() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin);
        let first = Workspaces::new(app.world_mut()).open("First");
        let second = Workspaces::new(app.world_mut()).open("Second");
        let first_area = {
            let w = Workbench::with_workspace(app.world_mut(), first).unwrap();
            w.destination_area(w.active_layout()).unwrap()
        };
        let second_area = {
            let w = Workbench::with_workspace(app.world_mut(), second).unwrap();
            w.destination_area(w.active_layout()).unwrap()
        };
        let panel = app
            .world_mut()
            .spawn((
                PanelInstance::default(),
                InArea(first_area),
                Counter::default(),
            ))
            .id();
        let handler = install_panel_handler::<New, _>(
            app.world_mut(),
            panel,
            |input: In<ContextCommandInvocation>, world: &mut World| increment(input.0, world),
        )
        .unwrap();
        Workbench::with_workspace(app.world_mut(), first)
            .unwrap()
            .relocate_panel(panel, second_area)
            .unwrap();
        assert_eq!(
            app.world().get::<InWorkspace>(handler).unwrap().0,
            second.entity()
        );
        app.world_mut().despawn(first.entity());
        assert!(app.world().get_entity(handler).is_ok());
        Workbench::with_workspace(app.world_mut(), second)
            .unwrap()
            .commands()
            .submit(ContextCommandRequest {
                command: ContextCommandId::of::<New>(),
                source: None,
                context: InvocationContext::Panel(panel),
            });
        app.update();
        assert_eq!(app.world().get::<Counter>(panel).unwrap().0, 1);
    }
}

#[cfg(test)]
mod admission_lifetime_tests {
    use super::*;
    use crate::*;
    struct Replace;
    impl ContextCommand for Replace {
        const KEY: &'static str = "admission.replace";
        const LABEL: &'static str = "Replace";
    }
    #[derive(Resource, Default)]
    struct Calls(u32);
    fn replacement(_: ContextCommandInvocation, world: &mut World) -> ContextCommandResult {
        world.resource_mut::<Calls>().0 += 100;
        Ok(())
    }
    fn replace_self(
        invocation: ContextCommandInvocation,
        world: &mut World,
    ) -> ContextCommandResult {
        world.resource_mut::<Calls>().0 += 1;
        world
            .entity_mut(invocation.panel.unwrap())
            .insert(CommandHandler::<Replace>::new(replacement));
        Ok(())
    }
    #[test]
    fn a_batch_does_not_execute_an_attached_handler_installed_after_admission() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Calls>();
        let workspace = Workspaces::new(app.world_mut()).open("Origin");
        let area = {
            let w = Workbench::with_workspace(app.world_mut(), workspace).unwrap();
            w.destination_area(w.active_layout()).unwrap()
        };
        let panel = app
            .world_mut()
            .spawn((
                PanelInstance::default(),
                InArea(area),
                CommandHandler::<Replace>::new(replace_self),
            ))
            .id();
        for _ in 0..2 {
            Workbench::with_workspace(app.world_mut(), workspace)
                .unwrap()
                .commands()
                .submit(ContextCommandRequest {
                    command: ContextCommandId::of::<Replace>(),
                    source: None,
                    context: InvocationContext::Panel(panel),
                });
        }
        app.update();
        assert_eq!(app.world().resource::<Calls>().0, 1);
        let finished: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ContextCommandFinished>>()
            .drain()
            .collect();
        assert!(finished[0].result.is_ok());
        assert!(
            finished[1]
                .result
                .as_ref()
                .unwrap_err()
                .contains("replaced")
        );
    }
}
