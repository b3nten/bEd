//! Action identity is a Rust type; executable handlers are scoped ECS instances.
use crate::{ActiveWorkspace, Area, Layout, Panel, Workbench, Workspace};
use bevy::{ecs::system::SystemId, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    any::TypeId,
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    ops::BitOr,
};

pub trait CommandDefinition: Send + Sync + 'static {
    const KEY: &'static str;
    const LABEL: &'static str;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandKey(Cow<'static, str>);
impl CommandKey {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// Erased typed identity for heterogeneous menus and requests. Never persisted;
/// CommandKey is the stable settings representation. This is not a handler ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommandType {
    kind: TypeId,
    key: &'static str,
    label: &'static str,
}

impl CommandType {
    pub fn of<C: CommandDefinition>() -> Self {
        Self {
            kind: TypeId::of::<C>(),
            key: C::KEY,
            label: C::LABEL,
        }
    }
    pub fn key(self) -> CommandKey {
        CommandKey(Cow::Borrowed(self.key))
    }
    pub fn label(self) -> &'static str {
        self.label
    }
}

#[derive(Component)]
pub struct CommandInfo {
    command: CommandType,
    pub label: String,
}

impl CommandInfo {
    pub fn command(&self) -> CommandType {
        self.command
    }
    pub fn key(&self) -> CommandKey {
        self.command.key()
    }
}

/// A placement names an action type. Handlers may be replaced or scoped without
/// rewriting its menus, buttons or keybindings.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invokes(pub CommandType);

impl Invokes {
    pub fn of<C: CommandDefinition>() -> Self {
        Self(CommandType::of::<C>())
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
pub struct CommandHandler {
    command: CommandType,
}

#[derive(Component, Debug)]
#[relationship(relationship_target = PanelHandlers)]
pub struct InPanel(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InPanel, linked_spawn)]
pub struct PanelHandlers(Vec<Entity>);

#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandContext {
    #[default]
    Focused,
    Global,
    Panel(Entity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandRequest {
    pub command: CommandType,
    pub source: Option<Entity>,
    pub context: CommandContext,
}

impl CommandRequest {
    pub fn new<C: CommandDefinition>() -> Self {
        Self {
            command: CommandType::of::<C>(),
            source: None,
            context: CommandContext::Focused,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInvocation {
    pub command: CommandType,
    pub source: Option<Entity>,
    pub panel: Option<Entity>,
}

pub type CommandResult = Result<(), String>;

#[derive(Resource, Default)]
pub struct CommandRequests(VecDeque<CommandRequest>);

impl CommandRequests {
    pub fn invoke<C: CommandDefinition>(&mut self) {
        self.invoke_type(CommandType::of::<C>());
    }
    pub fn invoke_type(&mut self, command: CommandType) {
        self.submit(CommandRequest {
            command,
            source: None,
            context: CommandContext::Focused,
        });
    }
    pub fn submit(&mut self, request: CommandRequest) {
        self.0.push_back(request);
    }
}

#[derive(Message, Clone, Debug)]
pub struct CommandFinished {
    pub invocation: CommandInvocation,
    pub result: CommandResult,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandSystems {
    Shortcuts,
    Dispatch,
}

/// Portable modifier data; no native menu or ImGui types occur in the model.
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
pub struct KeyBinding {
    pub command: CommandType,
    pub chord: KeyChord,
    pub allow_in_text: bool,
}

impl KeyBinding {
    pub fn new<C: CommandDefinition>(chord: KeyChord) -> Self {
        Self {
            command: CommandType::of::<C>(),
            chord,
            allow_in_text: false,
        }
    }
}

/// UI integration publishes capture before shortcut resolution. No UI dependency.
#[derive(Resource, Default)]
pub struct ShortcutCapture {
    pub keyboard: bool,
    pub text: bool,
}

/// Chords currently owned by installed native menus. The keyboard adapter skips
/// these so one physical shortcut cannot also invoke through a second path.
#[derive(Resource, Default)]
pub(crate) struct NativeShortcuts(pub HashSet<KeyChord>);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<CommandRequests>()
        .init_resource::<ShortcutCapture>()
        .init_resource::<NativeShortcuts>()
        .add_message::<CommandFinished>()
        .configure_sets(
            PostUpdate,
            (CommandSystems::Shortcuts, CommandSystems::Dispatch).chain(),
        )
        .add_systems(PostUpdate, shortcuts.in_set(CommandSystems::Shortcuts))
        .add_systems(PostUpdate, dispatch.in_set(CommandSystems::Dispatch));
}

impl Workbench<'_> {
    fn definition<C: CommandDefinition>(&mut self) -> Result<CommandType, String> {
        if !crate::valid_key(C::KEY) || C::LABEL.trim().is_empty() {
            return Err("Command requires a namespaced key and nonempty label".into());
        }
        let command = CommandType::of::<C>();
        for info in self.world.query::<&CommandInfo>().iter(self.world) {
            if info.command.kind == command.kind {
                return Ok(command);
            }
            if info.command.key == C::KEY {
                return Err(format!("Command key {:?} is already registered", C::KEY));
            }
        }
        self.world.spawn(CommandInfo {
            command,
            label: C::LABEL.into(),
        });
        Ok(command)
    }

    pub fn register_command<C: CommandDefinition, M>(
        &mut self,
        handler: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) -> Result<CommandType, String> {
        self.register_handler::<C, M>(None, handler)?;
        Ok(CommandType::of::<C>())
    }

    pub fn register_panel_command<C: CommandDefinition, M>(
        &mut self,
        panel: Entity,
        handler: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        self.world
            .get::<Panel>(panel)
            .ok_or("Panel is unavailable")?;
        self.register_handler::<C, M>(Some(panel), handler)
    }

    fn register_handler<C: CommandDefinition, M>(
        &mut self,
        panel: Option<Entity>,
        handler: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        let command = self.definition::<C>()?;
        if self.handler(command, panel).is_some() {
            return Err("Action already has a handler in this scope".into());
        }
        let entity = self.world.register_system(handler).entity();
        self.world
            .entity_mut(entity)
            .insert(CommandHandler { command });
        if let Some(panel) = panel {
            self.world.entity_mut(entity).insert(InPanel(panel));
        }
        Ok(entity)
    }

    pub fn command<C: CommandDefinition>(&mut self) -> Option<CommandType> {
        let command = CommandType::of::<C>();
        self.world
            .query::<&CommandInfo>()
            .iter(self.world)
            .any(|i| i.command == command)
            .then_some(command)
    }

    pub fn command_by_key(&mut self, key: &str) -> Option<CommandType> {
        self.world
            .query::<&CommandInfo>()
            .iter(self.world)
            .find_map(|i| (i.command.key == key).then_some(i.command))
    }

    pub fn handler(&mut self, command: CommandType, panel: Option<Entity>) -> Option<Entity> {
        find_handler(self.world, command, panel)
    }
}

fn find_handler(world: &mut World, command: CommandType, panel: Option<Entity>) -> Option<Entity> {
    let mut query = world.query::<(Entity, &CommandHandler, Option<&InPanel>)>();
    let mut handlers = query.iter(world).filter_map(|(entity, h, p)| {
        (h.command == command && p.map(|p| p.0) == panel).then_some(entity)
    });
    let handler = handlers.next();
    assert!(
        handlers.next().is_none(),
        "Multiple handlers for an action in one scope; use Workbench registration"
    );
    handler
}

fn focused_panel(world: &World) -> Option<Entity> {
    let workspace = world.resource::<ActiveWorkspace>().entity();
    let layout = world.get::<Workspace>(workspace).unwrap().active_layout();
    let area = world.get::<Layout>(layout).unwrap().focused_area();
    world.get::<Area>(area).unwrap().selected_panel()
}

pub(crate) fn resolve(
    world: &mut World,
    request: CommandRequest,
) -> (CommandInvocation, Result<Entity, String>) {
    let panel = match request.context {
        CommandContext::Focused => focused_panel(world),
        CommandContext::Global => None,
        CommandContext::Panel(p) => Some(p),
    };
    let invocation = CommandInvocation {
        command: request.command,
        source: request.source,
        panel,
    };
    if panel.is_some_and(|p| world.get::<Panel>(p).is_none()) {
        return (invocation, Err("Target panel is unavailable".into()));
    }
    let handler = panel
        .and_then(|p| find_handler(world, request.command, Some(p)))
        .or_else(|| find_handler(world, request.command, None));
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

pub fn command_available(world: &mut World, command: CommandType, context: CommandContext) -> bool {
    resolve(
        world,
        CommandRequest {
            command,
            source: None,
            context,
        },
    )
    .1
    .is_ok()
}

#[derive(Clone, Copy)]
struct ResolvedBinding {
    source: Entity,
    command: CommandType,
    chord: KeyChord,
    panel: Option<Entity>,
    allow_in_text: bool,
}

fn resolved_bindings(world: &mut World, context: CommandContext) -> Vec<ResolvedBinding> {
    let panel = match context {
        CommandContext::Focused => focused_panel(world),
        CommandContext::Panel(p) => Some(p),
        CommandContext::Global => None,
    };
    let mut groups: HashMap<KeyChord, Vec<ResolvedBinding>> = HashMap::new();
    for (source, b, scope) in world
        .query::<(Entity, &KeyBinding, Option<&InPanel>)>()
        .iter(world)
    {
        let scope = scope.map(|p| p.0);
        if scope.is_some() && scope != panel {
            continue;
        }
        let chord = KeyChord::new(b.chord.modifiers.resolved(), b.chord.key);
        groups.entry(chord).or_default().push(ResolvedBinding {
            source,
            command: b.command,
            chord,
            panel: scope,
            allow_in_text: b.allow_in_text,
        });
    }
    let mut resolved = Vec::new();
    for mut group in groups.into_values() {
        if group.iter().any(|b| b.panel.is_some()) {
            group.retain(|b| b.panel.is_some());
        }
        if group.len() == 1 {
            resolved.push(group[0]);
        }
        // Same-scope conflicts deliberately produce no executable binding.
    }
    resolved.sort_by_key(|b| b.source.to_bits());
    resolved
}

pub(crate) fn shortcut_for(
    world: &mut World,
    command: CommandType,
    context: CommandContext,
) -> Option<KeyChord> {
    resolved_bindings(world, context)
        .into_iter()
        .find_map(|b| (b.command == command).then_some(b.chord))
}

fn shortcuts(world: &mut World) {
    let Some(keys) = world.get_resource::<ButtonInput<KeyCode>>() else {
        return;
    };
    let mut modifiers = Modifiers::default();
    for (left, right, flag) in [
        (
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            Modifiers::CONTROL,
        ),
        (KeyCode::SuperLeft, KeyCode::SuperRight, Modifiers::META),
        (KeyCode::AltLeft, KeyCode::AltRight, Modifiers::ALT),
        (KeyCode::ShiftLeft, KeyCode::ShiftRight, Modifiers::SHIFT),
    ] {
        if keys.pressed(left) || keys.pressed(right) {
            modifiers = modifiers | flag;
        }
    }
    let pressed: HashSet<_> = keys.get_just_pressed().copied().collect();
    let capture = world.resource::<ShortcutCapture>();
    let blocked = capture.keyboard || capture.text;
    let native = world.resource::<NativeShortcuts>().0.clone();
    let requests: Vec<_> = resolved_bindings(world, CommandContext::Focused)
        .into_iter()
        .filter_map(|b| {
            (pressed.contains(&b.chord.key)
                && b.chord.modifiers == modifiers
                && (!blocked || b.allow_in_text)
                && !native.contains(&b.chord))
            .then_some(CommandRequest {
                command: b.command,
                source: Some(b.source),
                context: b
                    .panel
                    .map_or(CommandContext::Focused, CommandContext::Panel),
            })
        })
        .collect();
    for request in requests {
        world.resource_mut::<CommandRequests>().submit(request);
    }
}

fn dispatch(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<CommandRequests>().0);
    // Capture every target before any handler can change focus or close panels.
    let admitted: Vec<_> = pending.into_iter().map(|r| resolve(world, r)).collect();
    for (invocation, handler) in admitted {
        let result = handler.and_then(|entity| {
            if world.get::<CommandHandler>(entity).is_none() {
                return Err("Command handler is unavailable".into());
            }
            if !world.get::<ActionEnabled>(entity).unwrap().0 {
                return Err("Command is disabled".into());
            }
            world
                .run_system_with(
                    SystemId::<In<CommandInvocation>, CommandResult>::from_entity(entity),
                    invocation,
                )
                .map_err(|e| e.to_string())
                .and_then(|r| r)
        });
        world.write_message(CommandFinished { invocation, result });
    }
}
