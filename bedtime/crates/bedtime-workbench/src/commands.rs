//! Stable contextual command identities with independently scoped bindings and handlers.
#[cfg(feature = "persistence")]
use crate::persistence::ReflectWorkbenchPersist;
use crate::{Area, Layout, PanelInstance, Workbench};
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

    /// Compose application behavior directly on a panel or world entity.
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

/// A handler attached directly to its panel or an unscoped entity. Ordinary unscoped
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
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, WorkbenchPersist))]
#[component(on_insert = crate::membership::member_changed, on_remove = crate::membership::member_changed)]
#[relationship(relationship_target = PanelCommandParts)]
pub struct InPanel(#[cfg_attr(feature = "persistence", entities)] pub Entity);

#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component))]
#[relationship_target(relationship = InPanel, linked_spawn)]
pub struct PanelCommandParts(#[cfg_attr(feature = "persistence", entities)] Vec<Entity>);

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
    /// The requested origin, retained independently of the resolved panel.
    pub context: InvocationContext,
    pub command: ContextCommandId,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
}

pub type ContextCommandResult = Result<(), String>;

/// One request queue for the editing session, including when no layout is selected.
#[derive(Resource, Default)]
pub struct ContextCommandRequests(VecDeque<ContextCommandRequest>);

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
#[cfg_attr(feature = "persistence", derive(Serialize, Deserialize))]
pub struct Modifiers(u8);
impl Modifiers {
    pub const CONTROL: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const META: Self = Self(8);
    pub const PRIMARY: Self = Self(16);
    pub fn is_valid(self) -> bool {
        self.0 & !31 == 0
    }
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
#[cfg_attr(feature = "persistence", derive(Serialize, Deserialize))]
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
        .init_resource::<ContextCommandRequests>()
        .add_message::<ContextCommandFinished>()
        .add_message::<BindingConflict>()
        .configure_sets(
            PreUpdate,
            (
                ContextCommandSystems::CollectInput.after(crate::SessionSystems::Prepare),
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

/// Install a world handler, independently of command metadata or layout selection.
pub fn install_global_handler<C: ContextCommand, M>(
    world: &mut World,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    install_handler::<C, M>(world, CommandScope::World, handler)
}

/// Install a panel handler. Panel ownership supplies its lifetime.
pub fn install_panel_handler<C: ContextCommand, M>(
    world: &mut World,
    panel: Entity,
    handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
) -> Result<Entity, String> {
    world
        .get::<PanelInstance>(panel)
        .ok_or("Panel is unavailable")?;
    install_handler::<C, M>(world, CommandScope::Panel(panel), handler)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandScope {
    World,
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
    if let CommandScope::Panel(panel) = scope {
        world.entity_mut(entity).insert(InPanel(panel));
    }
    Ok(entity)
}

impl Workbench<'_> {
    pub fn register_context_command<C: ContextCommand>(
        &mut self,
    ) -> Result<ContextCommandId, String> {
        register_context_command::<C>(self.world)
    }

    /// Install a world handler for this session.
    pub fn register_handler<C: ContextCommand, M>(
        &mut self,
        handler: impl IntoSystem<In<ContextCommandInvocation>, ContextCommandResult, M> + 'static,
    ) -> Result<ContextCommandId, String> {
        install_global_handler::<C, M>(self.world, handler)?;
        Ok(ContextCommandId::of::<C>())
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
        let scope = panel.map_or(CommandScope::World, CommandScope::Panel);
        find_handler(self.world, command, scope)
    }
}

fn scope(panel: Option<&InPanel>) -> CommandScope {
    panel.map_or(CommandScope::World, |panel| CommandScope::Panel(panel.0))
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
        .query::<(Entity, &Handler, Option<&InPanel>)>()
        .iter(world)
        .filter_map(|(entity, h, panel)| {
            (h.command == command && scope(panel) == requested_scope).then_some(entity)
        })
        .collect();
    let attached: Vec<_> = world
        .query::<(
            Entity,
            &AttachedHandlers,
            Option<&InPanel>,
            Option<&PanelInstance>,
        )>()
        .iter(world)
        .filter_map(|(entity, handlers, panel, is_panel)| {
            let actual = if is_panel.is_some() {
                CommandScope::Panel(entity)
            } else {
                scope(panel)
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

pub(crate) fn focused_panel(world: &World) -> Option<Entity> {
    let layout = crate::active_layout(world)?;
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
            scope: scope(world.get::<InPanel>(entity)),
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
        } else {
            scope(world.get::<InPanel>(entity))
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
    request: ContextCommandRequest,
) -> (ContextCommandInvocation, Result<AdmittedHandler, String>) {
    let panel = match request.context {
        InvocationContext::Focused => focused_panel(world),
        InvocationContext::Global => None,
        InvocationContext::Panel(p) => Some(p),
    };
    let invocation = ContextCommandInvocation {
        context: request.context,
        command: request.command.clone(),
        source: request.source,
        panel,
    };
    if panel.is_some_and(|p| world.get::<PanelInstance>(p).is_none()) {
        return (invocation, Err("Target panel is unavailable".into()));
    }
    let handler = panel
        .and_then(|p| find_handler(world, request.command.clone(), CommandScope::Panel(p)))
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
    resolve(
        world,
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
    context: InvocationContext,
) -> HashMap<KeyChord, Vec<ResolvedBinding>> {
    let panel = match context {
        InvocationContext::Focused => focused_panel(world),
        InvocationContext::Panel(p) => Some(p),
        InvocationContext::Global => None,
    };
    let mut groups: HashMap<KeyChord, Vec<ResolvedBinding>> = HashMap::new();
    for (source, b, p) in world
        .query::<(Entity, &Binding, Option<&InPanel>)>()
        .iter(world)
    {
        let depth = match scope(p) {
            CommandScope::World => 0,
            CommandScope::Panel(p)
                if Some(p) == panel && world.get::<PanelInstance>(p).is_some() =>
            {
                1
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
    command: ContextCommandId,
    context: InvocationContext,
) -> Option<KeyChord> {
    let mut bindings: Vec<_> = binding_groups(world, context)
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
        let chord = KeyChord::new(press.chord.modifiers.resolved(), press.chord.key);
        let Some(mut candidates) = binding_groups(world, press.context).remove(&chord) else {
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
            world
                .resource_mut::<ContextCommandRequests>()
                .submit(request);
        }
    }
}

fn dispatch(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<ContextCommandRequests>().0);
    // Capture origin and targets before any handler changes focus or activation.
    let admitted: Vec<_> = pending.into_iter().map(|r| resolve(world, r)).collect();
    for (invocation, handler) in admitted {
        let result = handler.and_then(|admitted| {
            let entity = admitted.entity;
            if handler_snapshot(world, entity, &invocation.command) != Some(admitted) {
                return Err("Admitted command handler was removed, replaced, or retargeted".into());
            }
            if invocation
                .panel
                .is_some_and(|p| world.get::<PanelInstance>(p).is_none())
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
    use crate::WorkbenchCorePlugin;
    struct Save;
    impl ContextCommand for Save {
        const KEY: &'static str = "scope.save";
        const LABEL: &'static str = "Save";
    }
    #[derive(Resource, Default)]
    struct Calls(Vec<Option<Entity>>);
    fn record(i: In<ContextCommandInvocation>, mut calls: ResMut<Calls>) -> ContextCommandResult {
        calls.0.push(i.panel);
        Ok(())
    }
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Calls>();
        app
    }
    fn invoke(world: &mut World, context: InvocationContext) {
        world
            .resource_mut::<ContextCommandRequests>()
            .submit(ContextCommandRequest {
                command: ContextCommandId::of::<Save>(),
                source: None,
                context,
            });
        dispatch(world);
    }
    #[test]
    fn world_commands_work_without_layout_or_implicit_definition() {
        let mut app = app();
        let world = app.world_mut();
        install_global_handler::<Save, _>(world, record).unwrap();
        assert_eq!(world.query::<&ContextCommandInfo>().iter(world).count(), 3);
        invoke(world, InvocationContext::Focused);
        assert_eq!(world.resource::<Calls>().0, [None]);
    }
    #[test]
    fn disabled_panel_handler_blocks_world_fallback() {
        let mut app = app();
        let world = app.world_mut();
        let layout = crate::tests::layout(world);
        let panel = Workbench::new(world).create_panel(layout, "Panel").unwrap();
        install_global_handler::<Save, _>(world, record).unwrap();
        let local = install_panel_handler::<Save, _>(world, panel, record).unwrap();
        assert!(install_panel_handler::<Save, _>(world, panel, record).is_err());
        world.get_mut::<ActionEnabled>(local).unwrap().0 = false;
        invoke(world, InvocationContext::Panel(panel));
        assert!(world.resource::<Calls>().0.is_empty());
        world.despawn(local);
        invoke(world, InvocationContext::Panel(panel));
        assert_eq!(world.resource::<Calls>().0, [Some(panel)]);
    }
    #[test]
    fn panel_binding_conflicts_reserve_the_world_chord() {
        let mut app = app();
        let world = app.world_mut();
        let layout = crate::tests::layout(world);
        let panel = Workbench::new(world).create_panel(layout, "Panel").unwrap();
        install_global_handler::<Save, _>(world, record).unwrap();
        let chord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyS);
        world.spawn(Binding::new::<Save>(chord));
        let a = world
            .spawn((Binding::new::<Save>(chord), InPanel(panel)))
            .id();
        world.spawn((Binding::new::<Save>(chord), InPanel(panel)));
        world.resource_mut::<BindingInput>().0.push(BindingPress {
            chord,
            repeat: false,
            context: InvocationContext::Panel(panel),
        });
        shortcuts(world);
        dispatch(world);
        assert!(world.resource::<Calls>().0.is_empty());
        assert_eq!(
            world
                .resource_mut::<Messages<BindingConflict>>()
                .drain()
                .count(),
            1
        );
        world.despawn(a);
        world.resource_mut::<BindingInput>().0.push(BindingPress {
            chord,
            repeat: false,
            context: InvocationContext::Panel(panel),
        });
        shortcuts(world);
        dispatch(world);
        assert_eq!(world.resource::<Calls>().0, [Some(panel)]);
    }
}
