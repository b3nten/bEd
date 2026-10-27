//! Commands: named actions, the handlers that perform them, and their keys.
//!
//! A command is a marker type with a stable key, a label and default keys.
//! Handlers perform it: one global handler in `CommandRegistry`, and any
//! number of `CommandHandler<C>` components on panels. When a command runs
//! with a panel in context, that panel's handler wins over the global one.
//!
//! Which keys trigger which command is a plain table, `Keybindings`. Commands
//! may share a key: pressing it runs the one handled in the most specific
//! scope (the focused panel, then global). Two commands sharing a key and
//! handled in the same scope are a conflict, reported rather than guessed.
//!
//! With settings, each command's keys are a setting (`Keybinding<C>`) that
//! fills the table; without, the table holds each command's defaults.
#[cfg(feature = "persistence")]
use crate::persistence::StoreId;
use crate::{Area, Layout, PanelInstance};
use bevy::prelude::*;
use std::{any::TypeId, borrow::Cow, collections::VecDeque, fmt, ops::BitOr};

pub trait ContextCommand: Send + Sync + 'static {
    /// "document.save": stable, used in keybinding settings.
    const KEY: &'static str;
    const LABEL: &'static str;
    const DEFAULT_KEYS: &'static [KeyChord] = &[];
    /// Whether holding the keys runs the command repeatedly.
    const REPEATS: bool = false;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ContextCommandId(Cow<'static, str>);

impl ContextCommandId {
    pub fn of<C: ContextCommand>() -> Self {
        Self(Cow::Borrowed(C::KEY))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// On a menu item or button: the command it runs.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Invokes(pub ContextCommandId);

impl Invokes {
    pub fn of<C: ContextCommand>() -> Self {
        Self(ContextCommandId::of::<C>())
    }
}

/// What a handler is told: which command, and the panel it runs for, if any.
/// A global handler gets the panel too, e.g. to save the focused document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCommandInvocation {
    pub command: ContextCommandId,
    pub panel: Option<Entity>,
}

pub type ContextCommandResult = Result<(), String>;
pub type CommandFn = fn(&mut World, ContextCommandInvocation) -> ContextCommandResult;

/// A panel's handler for command `C`.
#[derive(Component)]
pub struct CommandHandler<C: ContextCommand> {
    /// A disabled handler still claims the command for its panel: the command
    /// is unavailable there rather than falling back to the global handler.
    pub enabled: bool,
    call: CommandFn,
    marker: std::marker::PhantomData<fn() -> C>,
}

impl<C: ContextCommand> CommandHandler<C> {
    pub fn new(call: CommandFn) -> Self {
        Self {
            enabled: true,
            call,
            marker: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "persistence")]
type SetKeysIn = fn(&mut World, StoreId, Option<Vec<KeyChord>>) -> Result<(), String>;

/// The registered commands and their global handlers.
#[derive(Resource, Default)]
pub struct CommandRegistry(Vec<Registered>);

struct Registered {
    id: ContextCommandId,
    label: &'static str,
    type_id: TypeId,
    repeats: bool,
    global: Option<CommandFn>,
    /// The panel's `CommandHandler<C>`, as (enabled, call).
    on_panel: fn(&World, Entity) -> Option<(bool, CommandFn)>,
    /// `Keybinding<C>` in one layer, for settings UIs that don't know `C`.
    #[cfg(feature = "persistence")]
    keys_in: fn(&World, StoreId) -> Option<Vec<KeyChord>>,
    #[cfg(feature = "persistence")]
    set_keys_in: SetKeysIn,
}

impl CommandRegistry {
    fn get(&self, command: &ContextCommandId) -> Option<&Registered> {
        self.0.iter().find(|registered| &registered.id == command)
    }

    /// Every registered command and its label, in registration order.
    pub fn commands(&self) -> impl Iterator<Item = (&ContextCommandId, &'static str)> {
        self.0
            .iter()
            .map(|registered| (&registered.id, registered.label))
    }

    pub fn label(&self, command: &ContextCommandId) -> Option<&'static str> {
        self.get(command).map(|registered| registered.label)
    }

    /// Set or replace (or with `None`, remove) `C`'s global handler.
    pub fn set_global_handler<C: ContextCommand>(&mut self, handler: Option<CommandFn>) {
        let registered = self
            .0
            .iter_mut()
            .find(|registered| registered.type_id == TypeId::of::<C>())
            .unwrap_or_else(|| panic!("Command {} isn't registered", C::KEY));
        registered.global = handler;
    }

    pub fn has_global_handler(&self, command: &ContextCommandId) -> bool {
        self.get(command).is_some_and(|r| r.global.is_some())
    }

    fn panel_handler(
        &self,
        world: &World,
        command: &ContextCommandId,
        panel: Entity,
    ) -> Option<(bool, CommandFn)> {
        self.get(command)
            .and_then(|registered| (registered.on_panel)(world, panel))
    }
}

/// Each command's keys. Dispatch, menus and the settings UI read this.
#[derive(Resource, Default)]
pub struct Keybindings(Vec<(ContextCommandId, Vec<KeyChord>)>);

impl Keybindings {
    pub fn keys(&self, command: &ContextCommandId) -> &[KeyChord] {
        self.0
            .iter()
            .find(|(id, _)| id == command)
            .map_or(&[], |(_, chords)| chords)
    }

    pub fn set(&mut self, command: ContextCommandId, chords: Vec<KeyChord>) {
        match self.0.iter_mut().find(|(id, _)| *id == command) {
            Some((_, existing)) => *existing = chords,
            None => self.0.push((command, chords)),
        }
    }

    /// The commands bound to `chord`.
    pub fn commands_for(&self, chord: KeyChord) -> impl Iterator<Item = &ContextCommandId> {
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

/// Where a command runs: for the focused panel, a given panel, or no panel.
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
    pub context: InvocationContext,
}

impl ContextCommandRequest {
    pub fn new<C: ContextCommand>() -> Self {
        Self {
            command: ContextCommandId::of::<C>(),
            context: InvocationContext::Focused,
        }
    }
}

/// Commands waiting to run. They run in order in `PreUpdate`; requests made by
/// a handler run in the next frame.
#[derive(Resource, Default)]
pub struct ContextCommandRequests(VecDeque<ContextCommandRequest>);

impl ContextCommandRequests {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn invoke<C: ContextCommand>(&mut self) {
        self.submit(ContextCommandRequest::new::<C>());
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
    /// Cmd on macOS, Ctrl elsewhere.
    pub const PRIMARY: Self = Self(16);
    pub const fn contains(self, rhs: Self) -> bool {
        self.0 & rhs.0 == rhs.0
    }
    pub const fn union(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
    /// With `PRIMARY` replaced by this platform's modifier.
    pub fn resolved(self) -> Self {
        let primary = if !self.contains(Self::PRIMARY) {
            0
        } else if cfg!(target_os = "macos") {
            Self::META.0
        } else {
            Self::CONTROL.0
        };
        Self((self.0 & !Self::PRIMARY.0) | primary)
    }
}

impl BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: KeyCode,
}

const MODIFIER_NAMES: [(Modifiers, &str); 5] = [
    (Modifiers::PRIMARY, "Primary"),
    (Modifiers::CONTROL, "Ctrl"),
    (Modifiers::META, "Meta"),
    (Modifiers::ALT, "Alt"),
    (Modifiers::SHIFT, "Shift"),
];

impl KeyChord {
    pub const fn new(modifiers: Modifiers, key: KeyCode) -> Self {
        Self { modifiers, key }
    }

    /// `Primary+Shift+Z`.
    pub const fn primary(key: KeyCode) -> Self {
        Self::new(Modifiers::PRIMARY, key)
    }

    pub fn resolved(self) -> Self {
        Self::new(self.modifiers.resolved(), self.key)
    }

    /// For people, on this platform: "Cmd+Shift+Z" on macOS.
    pub fn label(self) -> String {
        let modifiers = self.modifiers.resolved();
        let meta = if cfg!(target_os = "macos") {
            "Cmd+"
        } else {
            "Super+"
        };
        let mut label = String::new();
        for (flag, name) in [
            (Modifiers::CONTROL, "Ctrl+"),
            (Modifiers::META, meta),
            (Modifiers::ALT, "Alt+"),
            (Modifiers::SHIFT, "Shift+"),
        ] {
            if modifiers.contains(flag) {
                label.push_str(name);
            }
        }
        label.push_str(&key_name(self.key));
        label
    }
}

/// "S" for `KeyS`, "1" for `Digit1`, else the `KeyCode` variant's name.
fn key_name(key: KeyCode) -> String {
    let name = format!("{key:?}");
    name.strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .unwrap_or(&name)
        .to_owned()
}

/// The portable form kept in settings: "Primary+Shift+Z", "Ctrl+Enter".
impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in MODIFIER_NAMES {
            if self.modifiers.contains(flag) {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key))
    }
}

#[cfg(feature = "persistence")]
impl std::str::FromStr for KeyChord {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        use serde::{Deserialize, de::IntoDeserializer};
        let mut parts: Vec<_> = text.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|key| !key.is_empty());
        let key = key.ok_or_else(|| format!("{text:?} has no key"))?;
        let mut modifiers = Modifiers::default();
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "primary" => Modifiers::PRIMARY,
                "ctrl" | "control" => Modifiers::CONTROL,
                "meta" | "cmd" | "super" => Modifiers::META,
                "alt" | "option" => Modifiers::ALT,
                "shift" => Modifiers::SHIFT,
                _ => return Err(format!("{part:?} in {text:?} isn't a modifier")),
            };
            modifiers = modifiers | flag;
        }
        let name = match key.chars().next() {
            Some(c) if key.len() == 1 && c.is_ascii_alphabetic() => {
                format!("Key{}", c.to_ascii_uppercase())
            }
            Some(c) if key.len() == 1 && c.is_ascii_digit() => format!("Digit{c}"),
            _ => key.to_owned(),
        };
        let deserializer: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
            name.as_str().into_deserializer();
        let key = KeyCode::deserialize(deserializer)
            .map_err(|_| format!("{key:?} in {text:?} isn't a key"))?;
        Ok(Self::new(modifiers, key))
    }
}

#[cfg(feature = "persistence")]
impl serde::Serialize for KeyChord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "persistence")]
impl<'de> serde::Deserialize<'de> for KeyChord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
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

/// A chord pressed where several commands bound to it are handled in the same
/// scope. None of them runs.
#[derive(Message, Clone, Debug)]
pub struct BindingConflict {
    pub chord: KeyChord,
    pub commands: Vec<ContextCommandId>,
}

pub trait CommandAppExt {
    /// Make `C` known: it can be bound, listed and handled. With settings, its
    /// keys become the setting `keys.<C::KEY>`.
    fn register_command<C: ContextCommand>(&mut self) -> &mut Self;
    /// Register `C` and set its global handler.
    fn add_command_handler<C: ContextCommand>(&mut self, handler: CommandFn) -> &mut Self;
}

impl CommandAppExt for App {
    fn register_command<C: ContextCommand>(&mut self) -> &mut Self {
        assert!(
            crate::valid_key(C::KEY) && !C::LABEL.trim().is_empty(),
            "Command {:?} needs a dotted key and a label",
            C::KEY
        );
        let world = self.world_mut();
        let mut registry = world.get_resource_or_init::<CommandRegistry>();
        let id = ContextCommandId::of::<C>();
        if let Some(existing) = registry.get(&id) {
            assert!(
                existing.type_id == TypeId::of::<C>(),
                "Two commands use the key {:?}",
                C::KEY
            );
            return self;
        }
        registry.0.push(Registered {
            id: id.clone(),
            label: C::LABEL,
            type_id: TypeId::of::<C>(),
            repeats: C::REPEATS,
            global: None,
            on_panel: |world, panel| {
                world
                    .get::<CommandHandler<C>>(panel)
                    .map(|handler| (handler.enabled, handler.call))
            },
            #[cfg(feature = "persistence")]
            keys_in: |world, store| {
                world
                    .resource::<crate::settings::Settings>()
                    .get_in::<Keybinding<C>>(store)
            },
            #[cfg(feature = "persistence")]
            set_keys_in: |world, store, chords| match chords {
                Some(chords) => crate::settings::set_setting::<Keybinding<C>>(world, store, chords),
                None => crate::settings::clear_setting::<Keybinding<C>>(world, store),
            },
        });
        world
            .get_resource_or_init::<Keybindings>()
            .set(id, C::DEFAULT_KEYS.to_vec());
        #[cfg(feature = "persistence")]
        crate::settings::SettingsAppExt::register_setting::<Keybinding<C>>(self);
        self
    }

    fn add_command_handler<C: ContextCommand>(&mut self, handler: CommandFn) -> &mut Self {
        self.register_command::<C>();
        self.world_mut()
            .resource_mut::<CommandRegistry>()
            .set_global_handler::<C>(Some(handler));
        self
    }
}

/// The setting holding command `C`'s keys: `"keys.document.save": ["Primary+S"]`.
/// A layer's list replaces lower layers' lists; an empty list unbinds.
#[cfg(feature = "persistence")]
pub struct Keybinding<C>(std::marker::PhantomData<C>);

#[cfg(feature = "persistence")]
impl<C: ContextCommand> crate::settings::Setting for Keybinding<C> {
    type Value = Vec<KeyChord>;
    fn key() -> Cow<'static, str> {
        format!("keys.{}", C::KEY).into()
    }
    fn default() -> Vec<KeyChord> {
        C::DEFAULT_KEYS.to_vec()
    }
    fn apply(world: &mut World, chords: &Vec<KeyChord>) {
        world
            .resource_mut::<Keybindings>()
            .set(ContextCommandId::of::<C>(), chords.clone());
    }
}

/// The keys `command` has in one settings layer, if that layer sets them.
#[cfg(feature = "persistence")]
pub fn keys_in(world: &World, command: &ContextCommandId, store: StoreId) -> Option<Vec<KeyChord>> {
    let registered = world.resource::<CommandRegistry>().get(command)?;
    (registered.keys_in)(world, store)
}

/// Set `command`'s keys in one settings layer, or with `None` clear them there.
#[cfg(feature = "persistence")]
pub fn set_keys_in(
    world: &mut World,
    command: &ContextCommandId,
    store: StoreId,
    chords: Option<Vec<KeyChord>>,
) -> Result<(), String> {
    let set = world
        .resource::<CommandRegistry>()
        .get(command)
        .ok_or("Unknown command")?
        .set_keys_in;
    set(world, store, chords)
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<CommandRegistry>()
        .init_resource::<Keybindings>()
        .init_resource::<BindingInput>()
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
            (
                shortcuts.in_set(ContextCommandSystems::Shortcuts),
                dispatch.in_set(ContextCommandSystems::Dispatch),
            ),
        );
}

pub(crate) fn focused_panel(world: &World) -> Option<Entity> {
    let layout = crate::active_layout(world)?;
    let area = world.get::<Layout>(layout)?.focused_area()?;
    world.get::<Area>(area)?.selected_panel()
}

fn context_panel(world: &World, context: InvocationContext) -> Option<Entity> {
    match context {
        InvocationContext::Focused => focused_panel(world),
        InvocationContext::Global => None,
        InvocationContext::Panel(panel) => Some(panel),
    }
}

/// The handler that would run `command` for `panel`.
fn resolve(
    world: &World,
    command: &ContextCommandId,
    panel: Option<Entity>,
) -> Result<CommandFn, String> {
    let registry = world.resource::<CommandRegistry>();
    let registered = registry.get(command).ok_or("Unknown command")?;
    if let Some(panel) = panel {
        if world.get::<PanelInstance>(panel).is_none() {
            return Err("The panel is gone".into());
        }
        if let Some((enabled, call)) = (registered.on_panel)(world, panel) {
            return if enabled {
                Ok(call)
            } else {
                Err("Disabled in this panel".into())
            };
        }
    }
    registered
        .global
        .ok_or_else(|| "Nothing handles this here".into())
}

/// Whether `command` would run in `context` right now.
pub fn command_available(
    world: &World,
    command: &ContextCommandId,
    context: InvocationContext,
) -> bool {
    resolve(world, command, context_panel(world, context)).is_ok()
}

/// Which command `chord` runs for `panel`: among the commands bound to it,
/// the one handled in the most specific scope. `Err` lists a conflict.
fn chord_target(
    world: &World,
    chord: KeyChord,
    panel: Option<Entity>,
) -> Result<Option<ContextCommandId>, Vec<ContextCommandId>> {
    let registry = world.resource::<CommandRegistry>();
    let bound: Vec<_> = world
        .resource::<Keybindings>()
        .commands_for(chord)
        .cloned()
        .collect();
    let in_panel: Vec<_> = panel.map_or_else(Vec::new, |panel| {
        bound
            .iter()
            .filter(|command| registry.panel_handler(world, command, panel).is_some())
            .cloned()
            .collect()
    });
    let candidates = if in_panel.is_empty() {
        bound
            .into_iter()
            .filter(|command| registry.has_global_handler(command))
            .collect()
    } else {
        in_panel
    };
    match <[_; 1]>::try_from(candidates) {
        Ok([command]) => Ok(Some(command)),
        Err(candidates) if candidates.is_empty() => Ok(None),
        Err(candidates) => Err(candidates),
    }
}

/// The chord that runs `command` in `context`, for menu hints. Resolved for
/// this platform.
pub fn shortcut_for(
    world: &World,
    command: &ContextCommandId,
    context: InvocationContext,
) -> Option<KeyChord> {
    let panel = context_panel(world, context);
    let runs = |chord| chord_target(world, chord, panel).is_ok_and(|c| c.as_ref() == Some(command));
    let keys = world.resource::<Keybindings>().keys(command);
    keys.iter()
        .copied()
        .find(|&chord| runs(chord))
        .map(KeyChord::resolved)
}

fn shortcuts(world: &mut World) {
    let presses = std::mem::take(&mut world.resource_mut::<BindingInput>().0);
    for press in presses {
        let panel = context_panel(world, press.context);
        let command = match chord_target(world, press.chord, panel) {
            Ok(Some(command)) => command,
            Ok(None) => continue,
            Err(commands) => {
                world.write_message(BindingConflict {
                    chord: press.chord,
                    commands,
                });
                continue;
            }
        };
        let repeats = world
            .resource::<CommandRegistry>()
            .get(&command)
            .is_some_and(|registered| registered.repeats);
        if press.repeat && !repeats {
            continue;
        }
        let context = panel.map_or(InvocationContext::Global, InvocationContext::Panel);
        world
            .resource_mut::<ContextCommandRequests>()
            .submit(ContextCommandRequest { command, context });
    }
}

fn dispatch(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<ContextCommandRequests>().0);
    for request in pending {
        let invocation = ContextCommandInvocation {
            panel: context_panel(world, request.context),
            command: request.command,
        };
        let result = resolve(world, &invocation.command, invocation.panel)
            .and_then(|call| call(world, invocation.clone()));
        world.write_message(ContextCommandFinished { invocation, result });
    }
}

#[cfg(test)]
mod tests;
