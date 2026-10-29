//! Keybindings as settings (`settings` feature): one setting per action.
use crate::{Action, ActionId, ActionRegistry, Actions, ActionsRef, KeyChord, Keybindings};
use bedtime_persistence::StoreId;
use bedtime_settings::{Setting, SettingsWorldExt};
use bevy::prelude::*;
use std::borrow::Cow;

/// The setting holding action `A`'s keys: `"keys.document.save": ["Primary+S"]`.
/// A layer's list replaces lower layers' lists; an empty list unbinds.
pub struct Keybinding<A>(std::marker::PhantomData<A>);

impl<A: Action> Setting for Keybinding<A> {
    type Value = Vec<KeyChord>;
    fn key() -> Cow<'static, str> {
        format!("keys.{}", A::KEY).into()
    }
    fn default() -> Vec<KeyChord> {
        A::DEFAULT_KEYS.to_vec()
    }
    fn apply(world: &mut World, chords: &Vec<KeyChord>) {
        world
            .resource_mut::<Keybindings>()
            .set(ActionId::of::<A>(), chords.clone());
    }
}

type SetKeysIn = fn(&mut World, StoreId, Option<Vec<KeyChord>>) -> Result<(), String>;

/// `Keybinding<A>` for code that doesn't know `A` (a settings UI).
pub(crate) struct KeySettings {
    keys_in: fn(&World, StoreId) -> Option<Vec<KeyChord>>,
    keys_at: fn(&World, StoreId) -> Vec<KeyChord>,
    overridden_above: fn(&World, StoreId) -> Option<StoreId>,
    set_keys_in: SetKeysIn,
}

impl KeySettings {
    pub(crate) fn of<A: Action>() -> Self {
        Self {
            keys_in: |world, store| world.settings().get_in::<Keybinding<A>>(store),
            keys_at: |world, store| world.settings().get_at::<Keybinding<A>>(store),
            overridden_above: |world, store| {
                world.settings().overridden_above::<Keybinding<A>>(store)
            },
            set_keys_in: |world, store, chords| match chords {
                Some(chords) => world.settings_mut().set::<Keybinding<A>>(store, chords),
                None => world.settings_mut().clear::<Keybinding<A>>(store),
            },
        }
    }
}

impl ActionsRef {
    /// The keys `action` has in one settings layer, if that layer sets them.
    pub fn keys_in(&self, action: &ActionId, store: StoreId) -> Option<Vec<KeyChord>> {
        let registered = self.world.resource::<ActionRegistry>().get(action)?;
        (registered.keys.keys_in)(&self.world, store)
    }

    /// `action`'s keys as one settings layer and those below it give them.
    pub fn keys_at(&self, action: &ActionId, store: StoreId) -> Vec<KeyChord> {
        self.world
            .resource::<ActionRegistry>()
            .get(action)
            .map(|registered| (registered.keys.keys_at)(&self.world, store))
            .unwrap_or_default()
    }

    /// The layer above `store` whose different keys for `action` are used.
    pub fn keys_overridden_above(&self, action: &ActionId, store: StoreId) -> Option<StoreId> {
        let registered = self.world.resource::<ActionRegistry>().get(action)?;
        (registered.keys.overridden_above)(&self.world, store)
    }
}

impl Actions {
    /// Set `action`'s keys in one settings layer, or with `None` clear them there.
    pub fn set_keys_in(
        &mut self,
        action: &ActionId,
        store: StoreId,
        chords: Option<Vec<KeyChord>>,
    ) -> Result<(), String> {
        let set = self
            .world
            .resource::<ActionRegistry>()
            .get(action)
            .ok_or("Unknown action")?
            .keys
            .set_keys_in;
        set(&mut self.world, store, chords)
    }
}
