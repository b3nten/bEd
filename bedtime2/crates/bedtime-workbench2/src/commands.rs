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
pub struct Handler {
    command: ContextCommandId,
}

#[derive(Component, Debug)]
#[relationship(relationship_target = PanelCommandParts)]
pub struct InPanel(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InPanel, linked_spawn)]
pub struct PanelCommandParts(Vec<Entity>);

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
    pub workspace: WorkspaceId,
    pub command: ContextCommandId,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
}

pub type ContextCommandResult = Result<(), String>;

#[derive(Component, Default)]
pub struct ContextCommandRequests(VecDeque<ContextCommandRequest>);

impl ContextCommandRequests {
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

fn register_context_command<C: ContextCommand>(
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

/// Plugin-owned application-wide handlers can be installed before a workspace exists.
pub(crate) fn install_global_handler<C: ContextCommand, M>(
    world: &mut World,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    let command = register_context_command::<C>(world)?;
    if world
        .query_filtered::<&Handler, (Without<InPanel>, Without<crate::InWorkspace>)>()
        .iter(world)
        .any(|handler| handler.command == command)
    {
        return Err("Command already has a handler in this scope".into());
    }
    let entity = world.register_system(handler).entity();
    world.entity_mut(entity).insert(Handler { command });
    Ok(entity)
}

impl Workbench<'_> {
    pub fn register_context_command<C: ContextCommand>(
        &mut self,
    ) -> Result<ContextCommandId, String> {
        register_context_command::<C>(self.world)
    }

    pub fn register_handler<C: ContextCommand, M>(
        &mut self,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<ContextCommandId, String> {
        self.install_handler::<C, M>(None, handler)?;
        Ok(ContextCommandId::of::<C>())
    }

    pub fn register_panel_handler<C: ContextCommand, M>(
        &mut self,
        panel: Entity,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        self.require_owned(panel)?;
        self.world
            .get::<PanelInstance>(panel)
            .ok_or("Panel is unavailable")?;
        self.install_handler::<C, M>(Some(panel), handler)
    }

    fn install_handler<C: ContextCommand, M>(
        &mut self,
        panel: Option<Entity>,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        if panel.is_none() {
            return install_global_handler::<C, M>(self.world, handler);
        }
        let command = match self.command::<C>() {
            Some(command) => command,
            None => self.register_context_command::<C>()?,
        };
        if self.handler(command.clone(), panel).is_some() {
            return Err("Command already has a handler in this scope".into());
        }
        let entity = self.world.register_system(handler).entity();
        self.world.entity_mut(entity).insert(Handler { command });
        if let Some(panel) = panel {
            self.world
                .entity_mut(entity)
                .insert((InPanel(panel), InWorkspace(self.workspace.0)));
        }
        Ok(entity)
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
        find_handler(self.world, self.workspace, command, panel)
    }
}

fn find_handler(
    world: &mut World,
    workspace: WorkspaceId,
    command: ContextCommandId,
    panel: Option<Entity>,
) -> Option<Entity> {
    let mut query = world.query::<(Entity, &Handler, Option<&InPanel>, Option<&InWorkspace>)>();
    let mut handlers = query.iter(world).filter_map(|(entity, h, p, owner)| {
        (owner.is_none_or(|o| o.0 == workspace.0)
            && h.command == command
            && p.map(|p| p.0) == panel)
            .then_some(entity)
    });
    let handler = handlers.next();
    assert!(
        handlers.next().is_none(),
        "Multiple handlers for an action in one scope; use Workbench registration"
    );
    handler
}

pub(crate) fn focused_panel(world: &World) -> Option<Entity> {
    focused_panel_in(world, crate::workspaces::selected(world)?)
}

pub(crate) fn focused_panel_in(world: &World, workspace: WorkspaceId) -> Option<Entity> {
    let layout = crate::active_layout(world, workspace.0)?;
    let area = world.get::<Layout>(layout)?.focused_area()?;
    world.get::<Area>(area)?.selected_panel()
}

pub(crate) fn resolve(
    world: &mut World,
    workspace: WorkspaceId,
    request: ContextCommandRequest,
) -> (ContextCommandInvocation, Result<Entity, String>) {
    let panel = match request.context {
        InvocationContext::Focused => focused_panel_in(world, workspace),
        InvocationContext::Global => None,
        InvocationContext::Panel(p) => Some(p),
    };
    let invocation = ContextCommandInvocation {
        workspace,
        command: request.command.clone(),
        source: request.source,
        panel,
    };
    if crate::workspaces::validate(world, workspace).is_err()
        || panel.is_some_and(|p| {
            world.get::<PanelInstance>(p).is_none()
                || crate::workspaces::owner(world, p) != Some(workspace)
        })
    {
        return (invocation, Err("Target panel is unavailable".into()));
    }
    let handler = panel
        .and_then(|p| find_handler(world, workspace, request.command.clone(), Some(p)))
        .or_else(|| find_handler(world, workspace, request.command, None));
    let result = handler
        .ok_or_else(|| "No handler for this action in the requested context".into())
        .and_then(|h| {
            if world
                .get::<ActionEnabled>(h)
                .expect("Handler availability")
                .0
            {
                Ok(h)
            } else {
                Err("Command is disabled".into())
            }
        });
    (invocation, result)
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
    let Some(workspace) = workspace else {
        return false;
    };
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
    panel: Option<Entity>,
}

fn resolved_bindings(
    world: &mut World,
    workspace: WorkspaceId,
    context: InvocationContext,
) -> Vec<ResolvedBinding> {
    let panel = match context {
        InvocationContext::Focused => focused_panel_in(world, workspace),
        InvocationContext::Panel(p) => Some(p),
        InvocationContext::Global => None,
    };
    let mut groups: HashMap<KeyChord, Vec<ResolvedBinding>> = HashMap::new();
    for (source, b, scope) in world
        .query::<(Entity, &Binding, Option<&InPanel>)>()
        .iter(world)
    {
        if crate::workspaces::owner(world, source).is_some_and(|w| w != workspace) {
            continue;
        }
        let scope = scope.map(|p| p.0);
        if scope.is_some() && scope != panel {
            continue;
        }
        let chord = KeyChord::new(b.chord.modifiers.resolved(), b.chord.key);
        groups.entry(chord).or_default().push(ResolvedBinding {
            source,
            command: b.command.clone(),
            chord,
            panel: scope,
        });
    }
    let mut resolved = Vec::new();
    for mut group in groups.into_values() {
        if group.iter().any(|b| b.panel.is_some()) {
            group.retain(|b| b.panel.is_some());
        }
        if group.len() == 1 {
            resolved.push(group[0].clone());
        }
        // Same-scope conflicts deliberately produce no executable binding.
    }
    resolved.sort_by_key(|b| b.source.to_bits());
    resolved
}

pub(crate) fn shortcut_for(
    world: &mut World,
    workspace: WorkspaceId,
    command: ContextCommandId,
    context: InvocationContext,
) -> Option<KeyChord> {
    resolved_bindings(world, workspace, context)
        .into_iter()
        .find_map(|b| (b.command == command).then_some(b.chord))
}

fn shortcuts(world: &mut World) {
    let presses = std::mem::take(&mut world.resource_mut::<BindingInput>().0);
    if crate::workspaces::selected(world).is_none() {
        return;
    }
    for press in presses {
        let panel = match press.context {
            InvocationContext::Focused => focused_panel(world),
            InvocationContext::Panel(p) => Some(p),
            InvocationContext::Global => None,
        };
        let mut candidates: Vec<_> = world
            .query::<(Entity, &Binding, Option<&InPanel>)>()
            .iter(world)
            .filter(|(entity, b, scope)| {
                if crate::workspaces::owner(world, *entity)
                    .is_some_and(|w| w != crate::workspaces::active(world))
                {
                    return false;
                }
                KeyChord::new(b.chord.modifiers.resolved(), b.chord.key) == press.chord
                    && scope.is_none_or(|s| Some(s.0) == panel)
            })
            .map(|(e, b, scope)| (e, b.command.clone(), b.repeat, scope.is_some()))
            .collect();
        if candidates.iter().any(|c| c.3) {
            candidates.retain(|c| c.3);
        }
        if candidates.is_empty() {
            continue;
        }
        if candidates.len() > 1 {
            world.write_message(BindingConflict {
                chord: press.chord,
                sources: candidates.iter().map(|c| c.0).collect(),
            });
            continue;
        }
        let (source, command, repeat, _) = candidates.pop().unwrap();
        if !press.repeat || repeat {
            Workbench::new(world)
                .commands()
                .submit(ContextCommandRequest {
                    command,
                    source: Some(source),
                    context: panel.map_or(InvocationContext::Global, InvocationContext::Panel),
                });
        }
    }
}

fn dispatch(world: &mut World) {
    let pending: Vec<_> = world
        .query::<(Entity, &mut ContextCommandRequests)>()
        .iter_mut(world)
        .flat_map(|(entity, mut requests)| {
            std::mem::take(&mut requests.0)
                .into_iter()
                .map(move |r| (WorkspaceId(entity), r))
        })
        .collect();
    // Capture origin and targets before any handler changes focus or activation.
    let admitted: Vec<_> = pending
        .into_iter()
        .map(|(workspace, r)| resolve(world, workspace, r))
        .collect();
    for (invocation, handler) in admitted {
        let result = handler.and_then(|entity| {
            if world.get::<Handler>(entity).is_none() {
                return Err("Command handler is unavailable".into());
            }
            if !world.get::<ActionEnabled>(entity).unwrap().0 {
                return Err("Command is disabled".into());
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
