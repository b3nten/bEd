//! Plugin state snapshots and application-chosen persistence scopes.
use bevy::prelude::*;
use serde_json::{Map, Value};
use std::{collections::HashMap, io};

pub struct FeatureStateRestore {
    pub state: Option<Value>,
}

#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureStateChanged {
    pub owner: &'static str,
}

#[derive(Component, Clone, Copy)]
pub struct FeatureStateContribution {
    pub owner: &'static str,
    pub save: bevy::ecs::system::SystemId<(), Value>,
    pub restore: bevy::ecs::system::SystemId<In<FeatureStateRestore>, Result<(), String>>,
}

/// Persistence identity is opaque to contributors. The application defines its meaning.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateScope(pub String);

/// The application selects where state lives; project files use separate file services.
pub trait StateStore: Send + Sync + 'static {
    fn load(&self, scope: &StateScope) -> io::Result<Option<Value>>;
    fn save(&mut self, scope: &StateScope, snapshot: &Value) -> io::Result<()>;
}

#[derive(Default)]
pub struct MemoryStateStore {
    snapshots: HashMap<StateScope, Value>,
}

impl StateStore for MemoryStateStore {
    fn load(&self, scope: &StateScope) -> io::Result<Option<Value>> {
        Ok(self.snapshots.get(scope).cloned())
    }
    fn save(&mut self, scope: &StateScope, snapshot: &Value) -> io::Result<()> {
        validate_snapshot(snapshot)?;
        self.snapshots.insert(scope.clone(), snapshot.clone());
        Ok(())
    }
}

/// Core installs memory storage. Merely collecting contributions never writes a store.
#[derive(Resource)]
pub struct Storage(pub Box<dyn StateStore>);
impl Default for Storage {
    fn default() -> Self {
        Self(Box::<MemoryStateStore>::default())
    }
}

pub fn validate_snapshot(snapshot: &Value) -> io::Result<()> {
    if !snapshot.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Plugin state must be an object",
        ));
    }
    Ok(())
}

/// Collect installed contributors over the saved snapshot, preserving absent plugins.
pub fn collect_snapshot(world: &mut World, previous: Option<&Value>) -> io::Result<Value> {
    if let Some(previous) = previous {
        validate_snapshot(previous)?;
    }
    let mut state: Map<String, Value> = previous
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let hooks = world
        .query::<&FeatureStateContribution>()
        .iter(world)
        .copied()
        .collect::<Vec<_>>();
    let mut owners = std::collections::HashSet::new();
    for hook in hooks {
        if !owners.insert(hook.owner) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Duplicate plugin state owner: {}", hook.owner),
            ));
        }
        let value = world.run_system(hook.save).map_err(io::Error::other)?;
        state.insert(hook.owner.to_owned(), value);
    }
    Ok(Value::Object(state))
}

/// Restore before constructing panels; absent contributions explicitly reset installed plugins.
pub fn restore_snapshot(world: &mut World, snapshot: Option<&Value>) -> io::Result<()> {
    if let Some(snapshot) = snapshot {
        validate_snapshot(snapshot)?;
    }
    let hooks = world
        .query::<&FeatureStateContribution>()
        .iter(world)
        .copied()
        .collect::<Vec<_>>();
    for hook in hooks {
        let state = snapshot
            .and_then(|snapshot| snapshot.get(hook.owner))
            .cloned();
        world
            .run_system_with(hook.restore, FeatureStateRestore { state })
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn collection_preserves_absent_plugins_and_updates_installed_ones() {
        let mut world = World::new();
        let save = world.register_system(|| json!({"draft":"current"}));
        let restore = world.register_system(|In(_): In<FeatureStateRestore>| Ok(()));
        world.spawn(FeatureStateContribution {
            owner: "installed",
            save,
            restore,
        });
        let result = collect_snapshot(
            &mut world,
            Some(&json!({"absent":{"future":1},"installed":false})),
        )
        .unwrap();
        assert_eq!(
            result,
            json!({"absent":{"future":1},"installed":{"draft":"current"}})
        );
    }
    #[test]
    fn scopes_are_independent_and_invalid_saves_retain_previous_state() {
        let mut store = MemoryStateStore::default();
        let a = StateScope("a".into());
        let b = StateScope("b".into());
        store.save(&a, &json!({"plugin":1})).unwrap();
        assert!(store.save(&a, &Value::Null).is_err());
        assert_eq!(store.load(&a).unwrap(), Some(json!({"plugin":1})));
        assert_eq!(store.load(&b).unwrap(), None);
    }
}
