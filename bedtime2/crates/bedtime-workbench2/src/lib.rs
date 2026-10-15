//! Entity-backed workspaces, retained layouts, panels and queued commands.
mod geometry;
mod layout;
pub use layout::*;
#[cfg(feature = "ui")]
mod frontend;
use bedtime_desktop2::{ActionEnabled, DesktopAction};
use bevy::{ecs::system::SystemId, prelude::*};
#[cfg(feature = "ui")]
pub use frontend::{
    PANEL_TYPE_PAYLOAD, WorkbenchHost, attach_surface, panel_drag_source, register_panel,
    register_panel_in_area,
};
use serde::{Deserialize, Serialize};
use std::{any::TypeId, borrow::Cow, collections::VecDeque};

/// A consumer-defined command kind. Keys remain stable across releases and are
/// namespaced, for example `bed2.copy_greeting`. Rust types identify live registrations.
pub trait CommandDefinition: Send + Sync + 'static {
    const KEY: &'static str;
    const LABEL: &'static str;
}

/// Serializable identity for saved bindings. Runtime invocations use entities.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandKey(Cow<'static, str>);

impl CommandKey {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// Metadata on the registered Bevy system entity. Shared availability is the
/// required `ActionEnabled` component, read by both menus and command dispatch.
#[derive(Component)]
#[require(ActionEnabled)]
pub struct CommandInfo {
    key: CommandKey,
    kind: TypeId,
    pub label: String,
}

impl CommandInfo {
    pub fn key(&self) -> &CommandKey {
        &self.key
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInvocation {
    pub command: Entity,
    pub source: Option<Entity>,
}

pub type CommandResult = Result<(), String>;

/// A durable FIFO outbox. Handlers may enqueue work for the next dispatch pass;
/// they never recursively execute another command through this API.
#[derive(Resource, Default)]
pub struct CommandRequests(VecDeque<CommandInvocation>);

impl CommandRequests {
    pub fn invoke(&mut self, command: Entity) {
        self.submit(CommandInvocation {
            command,
            source: None,
        });
    }
    pub fn submit(&mut self, invocation: CommandInvocation) {
        self.0.push_back(invocation);
    }
}

#[derive(Message, Clone, Debug)]
pub struct CommandFinished {
    pub invocation: CommandInvocation,
    pub result: CommandResult,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandSystems {
    Ingress,
    Dispatch,
}

/// Headless lifecycle and command services. The optional UI frontend attaches
/// to an existing surface; adding this plugin never installs a renderer.
pub struct WorkbenchPlugin;

impl Plugin for WorkbenchPlugin {
    fn build(&self, app: &mut App) {
        layout::install(app);
        #[cfg(feature = "ui")]
        frontend::configure(app);
        app.init_resource::<CommandRequests>()
            .add_message::<CommandFinished>()
            .add_message::<DesktopAction>()
            .configure_sets(
                PostUpdate,
                (CommandSystems::Ingress, CommandSystems::Dispatch).chain(),
            )
            .add_systems(PostUpdate, collect_native.in_set(CommandSystems::Ingress))
            .add_systems(PostUpdate, dispatch.in_set(CommandSystems::Dispatch));
    }
}

/// Registration access to the existing world. Definitions, handlers and metadata
/// live on ECS entities; this facade owns no parallel command registry.
pub struct Workbench<'w> {
    world: &'w mut World,
}

impl<'w> Workbench<'w> {
    pub fn new(world: &'w mut World) -> Self {
        assert!(
            world.contains_resource::<CommandRequests>(),
            "Install WorkbenchPlugin before registering commands"
        );
        Self { world }
    }

    pub fn register_command<C: CommandDefinition, M>(
        &mut self,
        handler: impl IntoSystem<In<CommandInvocation>, CommandResult, M> + 'static,
    ) -> Result<Entity, String> {
        if !valid_key(C::KEY) {
            return Err(format!(
                "Command key {:?} must have nonempty namespaced segments",
                C::KEY
            ));
        }
        if C::LABEL.trim().is_empty() {
            return Err("Command label is empty".into());
        }
        for info in self.world.query::<&CommandInfo>().iter(self.world) {
            if info.key.as_str() == C::KEY || info.kind == TypeId::of::<C>() {
                return Err(format!("Command {:?} is already registered", C::KEY));
            }
        }
        let handler = self.world.register_system(handler);
        let entity = handler.entity();
        self.world.entity_mut(entity).insert(CommandInfo {
            key: CommandKey(Cow::Borrowed(C::KEY)),
            kind: TypeId::of::<C>(),
            label: C::LABEL.into(),
        });
        Ok(entity)
    }

    pub fn command<C: CommandDefinition>(&mut self) -> Option<Entity> {
        self.world
            .query::<(Entity, &CommandInfo)>()
            .iter(self.world)
            .find_map(|(entity, info)| (info.kind == TypeId::of::<C>()).then_some(entity))
    }

    pub fn command_by_key(&mut self, key: &str) -> Option<Entity> {
        self.world
            .query::<(Entity, &CommandInfo)>()
            .iter(self.world)
            .find_map(|(entity, info)| (info.key.as_str() == key).then_some(entity))
    }
}

fn collect_native(
    mut actions: MessageReader<DesktopAction>,
    commands: Query<&CommandInfo>,
    mut requests: ResMut<CommandRequests>,
) {
    for action in actions.read() {
        if let DesktopAction::Activated { source, target } = action
            && commands.contains(*target)
        {
            requests.submit(CommandInvocation {
                command: *target,
                source: Some(*source),
            });
        }
    }
}

fn dispatch(world: &mut World) {
    // One explicit pass. Newly enqueued requests remain durable for the next pass.
    let pending = std::mem::take(&mut world.resource_mut::<CommandRequests>().0);
    for invocation in pending {
        let result = if world.get::<CommandInfo>(invocation.command).is_none() {
            Err("Command is no longer registered".into())
        } else if !world
            .get::<ActionEnabled>(invocation.command)
            .expect("registered commands require availability")
            .0
        {
            Err("Command is disabled".into())
        } else {
            let handler =
                SystemId::<In<CommandInvocation>, CommandResult>::from_entity(invocation.command);
            world
                .run_system_with(handler, invocation)
                .map_err(|error| error.to_string())
                .and_then(|result| result)
        };
        world.write_message(CommandFinished { invocation, result });
    }
}

#[cfg(test)]
mod tests;

// Persistent extension identifiers share one namespacing rule.
fn valid_key(key: &str) -> bool {
    let mut count = 0;
    let valid = key.split('.').all(|part| {
        count += 1;
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    valid && count >= 2
}
