//! Actions: named things a user can do, the handlers that perform them, and
//! their keys.
//!
//! An action is a marker type with a stable key, a label and default keys.
//! Handlers perform it: one global handler in `ActionRegistry`, and any number
//! of `ActionHandler<A>` components on entities. An action runs for an entity
//! (the focused one, by default); that entity's handler wins over the global
//! one. What's focused is the app's business: `ActionFocus` says
//! (workbench makes it the focused panel). Without it, only global
//! handlers run.
//!
//! Which keys trigger which action is a plain table, `Keybindings`. Actions may
//! share a key: pressing it runs the one handled in the most specific scope
//! (the focused entity, then global). Two actions sharing a key and handled in
//! the same scope are a conflict, reported rather than guessed.
//!
//! With the `persistence` feature, each action's keys are a setting
//! (`Keybinding<A>`) that fills the table; without, the table holds each
//! action's defaults. With `ui`, shortcuts are collected from egui, and with
//! both, `ui::keybindings_section` edits them in a settings panel.
mod chord;
#[cfg(feature = "persistence")]
mod keys;
#[cfg(test)]
mod tests;
#[cfg(feature = "ui")]
pub mod ui;

pub use chord::{KeyChord, Modifiers};
#[cfg(feature = "persistence")]
pub use keys::Keybinding;

use bevy::prelude::*;
use std::{any::TypeId, borrow::Cow, collections::VecDeque};

pub trait Action: Send + Sync + 'static {
    /// "document.save": stable, used in keybinding settings.
    const KEY: &'static str;
    const LABEL: &'static str;
    const DEFAULT_KEYS: &'static [KeyChord] = &[];
    /// Whether holding the keys runs the action repeatedly.
    const REPEATS: bool = false;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ActionId(Cow<'static, str>);

impl ActionId {
    pub fn of<A: Action>() -> Self {
        Self(Cow::Borrowed(A::KEY))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// On a menu item or button: the action it runs.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Invokes(pub ActionId);

impl Invokes {
    pub fn of<A: Action>() -> Self {
        Self(ActionId::of::<A>())
    }
}

/// What a handler is told: which action, and the entity it runs for, if any.
/// A global handler gets the entity too, e.g. to save the focused document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionInvocation {
    pub action: ActionId,
    pub entity: Option<Entity>,
    /// What triggered it, e.g. a menu item; handlers can read details from it.
    pub source: Option<Entity>,
}

pub type ActionResult = Result<(), String>;
pub type ActionFn = fn(&mut World, ActionInvocation) -> ActionResult;

/// Performs an action, globally or for an entity.
#[derive(Clone, Copy)]
pub struct Handler {
    run: ActionFn,
    /// Whether it can run now, for the given entity. Menus grey out actions
    /// that can't. An unavailable entity handler still claims the action for
    /// its entity: it doesn't fall back to the global handler.
    available: fn(&World, Option<Entity>) -> bool,
}

impl Handler {
    pub fn new(run: ActionFn) -> Self {
        Self {
            run,
            available: |_, _| true,
        }
    }

    pub fn available_when(self, available: fn(&World, Option<Entity>) -> bool) -> Self {
        Self { available, ..self }
    }
}

/// An entity's handler for action `A`.
#[derive(Component)]
pub struct ActionHandler<A: Action> {
    pub handler: Handler,
    marker: std::marker::PhantomData<fn() -> A>,
}

impl<A: Action> ActionHandler<A> {
    pub fn new(handler: Handler) -> Self {
        Self {
            handler,
            marker: std::marker::PhantomData,
        }
    }
}

/// The entity actions run for when nothing else is said: the focused one.
/// A function, so it's never stale; it returns `None` until the app sets one.
#[derive(Resource, Clone, Copy)]
pub struct ActionFocus(pub fn(&World) -> Option<Entity>);

impl Default for ActionFocus {
    fn default() -> Self {
        Self(|_| None)
    }
}

/// The registered actions and their global handlers.
#[derive(Resource, Default)]
pub struct ActionRegistry(Vec<Registered>);

struct Registered {
    id: ActionId,
    label: &'static str,
    type_id: TypeId,
    repeats: bool,
    global: Option<Handler>,
    /// The entity's `ActionHandler<A>`.
    on_entity: fn(&World, Entity) -> Option<Handler>,
    #[cfg(feature = "persistence")]
    keys: keys::KeySettings,
}

impl ActionRegistry {
    fn get(&self, action: &ActionId) -> Option<&Registered> {
        self.0.iter().find(|registered| &registered.id == action)
    }

    /// Every registered action and its label, in registration order.
    pub fn actions(&self) -> impl Iterator<Item = (&ActionId, &'static str)> {
        self.0
            .iter()
            .map(|registered| (&registered.id, registered.label))
    }

    pub fn label(&self, action: &ActionId) -> Option<&'static str> {
        self.get(action).map(|registered| registered.label)
    }

    /// Set or replace (or with `None`, remove) `A`'s global handler.
    pub fn set_global_handler<A: Action>(&mut self, handler: Option<Handler>) {
        let registered = self
            .0
            .iter_mut()
            .find(|registered| registered.type_id == TypeId::of::<A>())
            .unwrap_or_else(|| panic!("Action {} isn't registered", A::KEY));
        registered.global = handler;
    }

    pub fn has_global_handler(&self, action: &ActionId) -> bool {
        self.get(action).is_some_and(|r| r.global.is_some())
    }

    fn entity_handler(&self, world: &World, action: &ActionId, entity: Entity) -> Option<Handler> {
        self.get(action)
            .and_then(|registered| (registered.on_entity)(world, entity))
    }
}

/// Each action's keys. Dispatch, menus and the settings UI read this.
#[derive(Resource, Default)]
pub struct Keybindings(Vec<(ActionId, Vec<KeyChord>)>);

impl Keybindings {
    pub fn keys(&self, action: &ActionId) -> &[KeyChord] {
        self.0
            .iter()
            .find(|(id, _)| id == action)
            .map_or(&[], |(_, chords)| chords)
    }

    pub fn set(&mut self, action: ActionId, chords: Vec<KeyChord>) {
        match self.0.iter_mut().find(|(id, _)| *id == action) {
            Some((_, existing)) => *existing = chords,
            None => self.0.push((action, chords)),
        }
    }

    /// The actions bound to `chord`.
    pub fn actions_for(&self, chord: KeyChord) -> impl Iterator<Item = &ActionId> {
        let chord = chord.resolved();
        self.0
            .iter()
            .filter(move |(_, chords)| chords.iter().any(|c| c.resolved() == chord))
            .map(|(id, _)| id)
    }

    /// Every bound chord, resolved for this platform.
    pub fn chords(&self) -> impl Iterator<Item = KeyChord> + '_ {
        self.0
            .iter()
            .flat_map(|(_, chords)| chords.iter().map(|chord| chord.resolved()))
    }
}

/// Where an action runs: for the focused entity, a given entity, or none.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InvocationContext {
    #[default]
    Focused,
    Global,
    Entity(Entity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRequest {
    pub action: ActionId,
    pub context: InvocationContext,
    pub source: Option<Entity>,
}

impl ActionRequest {
    pub fn new<A: Action>() -> Self {
        Self {
            action: ActionId::of::<A>(),
            context: InvocationContext::Focused,
            source: None,
        }
    }
}

/// Actions waiting to run. They run in order in `PreUpdate`; requests made by
/// a handler run in the next frame.
#[derive(Resource, Default)]
pub struct ActionRequests(VecDeque<ActionRequest>);

impl ActionRequests {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn invoke<A: Action>(&mut self) {
        self.submit(ActionRequest::new::<A>());
    }
    pub fn submit(&mut self, request: ActionRequest) {
        self.0.push_back(request);
    }
}

/// An action was requested. `ran` says whether a handler ran (it didn't when
/// the action isn't available, or nothing handles it here); `result` is the
/// handler's, or why none ran.
#[derive(Message, Clone, Debug)]
pub struct ActionFinished {
    pub invocation: ActionInvocation,
    pub ran: bool,
    pub result: ActionResult,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionSystems {
    CollectInput,
    Shortcuts,
    Dispatch,
}

/// A chord pressed, from any input source. The egui collector fills this; a
/// headless host or test can too.
#[derive(Clone, Copy, Debug)]
pub struct BindingPress {
    pub chord: KeyChord,
    pub repeat: bool,
    pub context: InvocationContext,
}

#[derive(Resource, Default)]
pub struct BindingInput(pub Vec<BindingPress>);

/// A chord pressed where several actions bound to it are handled in the same
/// scope. None of them runs.
#[derive(Message, Clone, Debug)]
pub struct BindingConflict {
    pub chord: KeyChord,
    pub actions: Vec<ActionId>,
}

/// Runs requested actions and the shortcuts pressed, in `PreUpdate`.
pub struct ActionsPlugin;

impl Plugin for ActionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActionRegistry>()
            .init_resource::<Keybindings>()
            .init_resource::<BindingInput>()
            .init_resource::<ActionRequests>()
            .init_resource::<ActionFocus>()
            .add_message::<ActionFinished>()
            .add_message::<BindingConflict>()
            .configure_sets(
                PreUpdate,
                (
                    ActionSystems::CollectInput,
                    ActionSystems::Shortcuts,
                    ActionSystems::Dispatch,
                )
                    .chain(),
            )
            .add_systems(
                PreUpdate,
                (
                    shortcuts.in_set(ActionSystems::Shortcuts),
                    dispatch.in_set(ActionSystems::Dispatch),
                ),
            );
        #[cfg(feature = "ui")]
        ui::install(app);
    }
}

pub trait ActionAppExt {
    /// Make `A` known: it can be bound, listed and handled. With settings, its
    /// keys become the setting `keys.<A::KEY>`.
    fn register_action<A: Action>(&mut self) -> &mut Self;
    /// Register `A` and set its global handler.
    fn add_action_handler<A: Action>(&mut self, handler: Handler) -> &mut Self;
}

impl ActionAppExt for App {
    fn register_action<A: Action>(&mut self) -> &mut Self {
        assert!(
            valid_key(A::KEY) && !A::LABEL.trim().is_empty(),
            "Action {:?} needs a dotted key and a label",
            A::KEY
        );
        let world = self.world_mut();
        let mut registry = world.get_resource_or_init::<ActionRegistry>();
        let id = ActionId::of::<A>();
        if let Some(existing) = registry.get(&id) {
            assert!(
                existing.type_id == TypeId::of::<A>(),
                "Two actions use the key {:?}",
                A::KEY
            );
            return self;
        }
        registry.0.push(Registered {
            id: id.clone(),
            label: A::LABEL,
            type_id: TypeId::of::<A>(),
            repeats: A::REPEATS,
            global: None,
            on_entity: |world, entity| {
                world
                    .get::<ActionHandler<A>>(entity)
                    .map(|component| component.handler)
            },
            #[cfg(feature = "persistence")]
            keys: keys::KeySettings::of::<A>(),
        });
        world
            .get_resource_or_init::<Keybindings>()
            .set(id, A::DEFAULT_KEYS.to_vec());
        #[cfg(feature = "persistence")]
        crate::settings::SettingsAppExt::register_setting::<Keybinding<A>>(self);
        self
    }

    fn add_action_handler<A: Action>(&mut self, handler: Handler) -> &mut Self {
        self.register_action::<A>();
        self.world_mut()
            .resource_mut::<ActionRegistry>()
            .set_global_handler::<A>(Some(handler));
        self
    }
}

/// Reading what actions would do: `world.actions()`.
#[repr(transparent)]
pub struct ActionsRef {
    world: World,
}

/// Changing actions' keys: `world.actions_mut()`. It reads too.
#[repr(transparent)]
pub struct Actions {
    world: World,
}

impl std::ops::Deref for Actions {
    type Target = ActionsRef;
    fn deref(&self) -> &ActionsRef {
        view(&self.world)
    }
}

fn view(world: &World) -> &ActionsRef {
    // SAFETY: ActionsRef is a repr(transparent) wrapper of World.
    unsafe { &*(world as *const World).cast::<ActionsRef>() }
}

/// `world.actions()` and `world.actions_mut()`.
pub trait ActionsWorldExt {
    fn actions(&self) -> &ActionsRef;
    fn actions_mut(&mut self) -> &mut Actions;
}

impl ActionsWorldExt for World {
    fn actions(&self) -> &ActionsRef {
        view(self)
    }
    fn actions_mut(&mut self) -> &mut Actions {
        // SAFETY: Actions is a repr(transparent) wrapper of World.
        unsafe { &mut *(self as *mut World).cast::<Actions>() }
    }
}

impl ActionsRef {
    /// The entity `context` stands for.
    pub fn entity(&self, context: InvocationContext) -> Option<Entity> {
        match context {
            InvocationContext::Focused => (self.world.resource::<ActionFocus>().0)(&self.world),
            InvocationContext::Global => None,
            InvocationContext::Entity(entity) => Some(entity),
        }
    }

    /// Whether `action` would run in `context` right now.
    pub fn available(&self, action: &ActionId, context: InvocationContext) -> bool {
        resolve(&self.world, action, self.entity(context)).is_ok()
    }

    /// The chord that runs `action` in `context`, for menu hints. Resolved for
    /// this platform.
    pub fn shortcut_for(&self, action: &ActionId, context: InvocationContext) -> Option<KeyChord> {
        let world = &self.world;
        let entity = self.entity(context);
        let runs =
            |chord| chord_target(world, chord, entity).is_ok_and(|a| a.as_ref() == Some(action));
        let keys = world.resource::<Keybindings>().keys(action);
        keys.iter()
            .copied()
            .find(|&chord| runs(chord))
            .map(KeyChord::resolved)
    }

    /// "Also bound to Find" when another action shares one of `action`'s keys.
    /// Not an error: which one runs depends on what's focused.
    pub fn shared_with(&self, action: &ActionId) -> Option<String> {
        let bindings = self.world.resource::<Keybindings>();
        let registry = self.world.resource::<ActionRegistry>();
        let mut others: Vec<_> = bindings
            .keys(action)
            .iter()
            .flat_map(|&chord| bindings.actions_for(chord))
            .filter(|other| *other != action)
            .filter_map(|other| registry.label(other))
            .collect();
        others.dedup();
        (!others.is_empty()).then(|| format!("Also bound to {}", others.join(", ")))
    }
}

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

/// The handler that would run `action` for `entity`.
fn resolve(world: &World, action: &ActionId, entity: Option<Entity>) -> Result<ActionFn, String> {
    let registry = world.resource::<ActionRegistry>();
    let registered = registry.get(action).ok_or("Unknown action")?;
    if entity.is_some_and(|entity| world.get_entity(entity).is_err()) {
        return Err("It's gone".into());
    }
    let handler = entity
        .and_then(|entity| (registered.on_entity)(world, entity))
        .or(registered.global)
        .ok_or("Nothing handles this here")?;
    if (handler.available)(world, entity) {
        Ok(handler.run)
    } else {
        Err("Not available now".into())
    }
}

/// Which action `chord` runs for `entity`: among the actions bound to it, the
/// one handled in the most specific scope. `Err` lists a conflict.
fn chord_target(
    world: &World,
    chord: KeyChord,
    entity: Option<Entity>,
) -> Result<Option<ActionId>, Vec<ActionId>> {
    let registry = world.resource::<ActionRegistry>();
    let bound: Vec<_> = world
        .resource::<Keybindings>()
        .actions_for(chord)
        .cloned()
        .collect();
    let on_entity: Vec<_> = entity.map_or_else(Vec::new, |entity| {
        bound
            .iter()
            .filter(|action| registry.entity_handler(world, action, entity).is_some())
            .cloned()
            .collect()
    });
    let candidates = if on_entity.is_empty() {
        bound
            .into_iter()
            .filter(|action| registry.has_global_handler(action))
            .collect()
    } else {
        on_entity
    };
    match <[_; 1]>::try_from(candidates) {
        Ok([action]) => Ok(Some(action)),
        Err(candidates) if candidates.is_empty() => Ok(None),
        Err(candidates) => Err(candidates),
    }
}

fn shortcuts(world: &mut World) {
    let presses = std::mem::take(&mut world.resource_mut::<BindingInput>().0);
    for press in presses {
        let entity = world.actions().entity(press.context);
        let action = match chord_target(world, press.chord, entity) {
            Ok(Some(action)) => action,
            Ok(None) => continue,
            Err(actions) => {
                world.write_message(BindingConflict {
                    chord: press.chord,
                    actions,
                });
                continue;
            }
        };
        let repeats = world
            .resource::<ActionRegistry>()
            .get(&action)
            .is_some_and(|registered| registered.repeats);
        if press.repeat && !repeats {
            continue;
        }
        let context = entity.map_or(InvocationContext::Global, InvocationContext::Entity);
        world
            .resource_mut::<ActionRequests>()
            .submit(ActionRequest {
                action,
                context,
                source: None,
            });
    }
}

fn dispatch(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<ActionRequests>().0);
    for request in pending {
        let invocation = ActionInvocation {
            entity: world.actions().entity(request.context),
            action: request.action,
            source: request.source,
        };
        let (ran, result) = match resolve(world, &invocation.action, invocation.entity) {
            Ok(run) => (true, run(world, invocation.clone())),
            Err(why) => (false, Err(why)),
        };
        world.write_message(ActionFinished {
            invocation,
            ran,
            result,
        });
    }
}
