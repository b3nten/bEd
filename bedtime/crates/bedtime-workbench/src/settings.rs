//! Settings: typed values resolved through layers, each layer kept in a store.
//!
//! Layers stack in the order the app adds them; later layers win. A layer
//! holds only what was set in it, so a missing value inherits from the layers
//! below, and finally from `Setting::default()`. Workbench knows nothing about
//! what the layers mean; an app might stack `GlobalStore` under `WorkbenchStore`
//! so per-workspace values override global ones.
//!
//! A setting's `apply` runs whenever its resolved value changes: after a `set`
//! or `clear`, when layers load at startup, and when a layer's file changes on
//! disk. Consumers (resources, components) are updated there.
//!
//! The unit of override is the setting. Anything that should be overridable per
//! entry is split into one setting per entry with a derived key.
use crate::persistence::{Store, StoreBackend, StoreId, StoreSystems, Stores};
use bevy::prelude::*;
use ron::value::RawValue;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    time::{Duration, Instant},
};

const ENTRY: &str = "settings";
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

pub trait Setting: Send + Sync + 'static {
    type Value: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static;
    /// "filetree.indent". A function so generic settings can derive it.
    fn key() -> Cow<'static, str>;
    fn default() -> Self::Value;
    /// Which layers may hold this setting. All of them by default.
    fn allowed_in(_store: StoreId) -> bool {
        true
    }
    /// Runs whenever the resolved value changes, including once at startup.
    fn apply(_world: &mut World, _value: &Self::Value) {}
}

/// Define a setting.
///
/// ```ignore
/// setting! {
///     /// File tree indentation, in points.
///     pub FileTreeIndent: f32 = 16.0, key = "filetree.indent",
///     apply = |world, indent| world.resource_mut::<FileTreeConfig>().indent = *indent,
/// }
/// setting! {
///     pub RestoreLastWorkspace: bool = true, key = "startup.restore", only_in = [GlobalStore],
/// }
/// ```
#[macro_export]
macro_rules! setting {
    (
        $(#[$meta:meta])* $vis:vis $name:ident: $ty:ty = $default:expr, key = $key:literal
        $(, only_in = [$($store:ty),* $(,)?])?
        $(, apply = $apply:expr)? $(,)?
    ) => {
        $(#[$meta])*
        $vis struct $name;
        impl $crate::settings::Setting for $name {
            type Value = $ty;
            fn key() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed($key)
            }
            fn default() -> $ty {
                $default
            }
            $(
                fn allowed_in(store: $crate::persistence::StoreId) -> bool {
                    [$($crate::persistence::StoreId::of::<$store>()),*].contains(&store)
                }
            )?
            $(
                fn apply(world: &mut ::bevy::prelude::World, value: &$ty) {
                    let apply: fn(&mut ::bevy::prelude::World, &$ty) = $apply;
                    apply(world, value)
                }
            )?
        }
    };
}

/// All layers, lowest precedence first, and the registered settings.
#[derive(Resource, Default)]
pub struct Settings {
    layers: Vec<Layer>,
    registered: Vec<Registered>,
}

struct Layer {
    store: StoreId,
    /// Each value's RON text, exactly as stored. (`ron::Value` would lose enum
    /// variant names, and values of unknown settings must survive untouched.)
    values: BTreeMap<String, Box<RawValue>>,
    /// Set since the last save.
    dirty: bool,
    /// The layer's file couldn't be read, so it's never written.
    blocked: Option<String>,
    /// The backend's version of the file as last read or written.
    version: Option<String>,
    /// The last save failure, reported once.
    save_error: Option<String>,
}

struct Registered {
    key: Cow<'static, str>,
    /// The resolved value, encoded, so changes can be noticed without the type.
    resolve: fn(&Settings) -> String,
    apply: fn(&mut World),
    /// What `apply` last saw.
    applied: Option<String>,
    /// Layers whose stored value for this setting doesn't decode.
    invalid: Vec<StoreId>,
    check: fn(&Settings) -> Vec<StoreId>,
}

impl Settings {
    /// The value from the highest layer that has a usable one, else the default.
    pub fn get<S: Setting>(&self) -> S::Value {
        self.resolve::<S>()
            .map(|(_, value)| value)
            .unwrap_or_else(S::default)
    }

    /// Which layer the value comes from, or `None` for the default.
    pub fn source<S: Setting>(&self) -> Option<StoreId> {
        self.resolve::<S>().map(|(store, _)| store)
    }

    /// The value set in one layer, if any.
    pub fn get_in<S: Setting>(&self, store: StoreId) -> Option<S::Value> {
        let layer = self.layers.iter().find(|layer| layer.store == store)?;
        decode::<S>(layer)
    }

    /// The layers, lowest precedence first.
    pub fn layers(&self) -> impl Iterator<Item = StoreId> + '_ {
        self.layers.iter().map(|layer| layer.store)
    }

    /// Why a layer isn't loaded or saved, or which of its values don't decode.
    pub fn error(&self, store: StoreId) -> Option<String> {
        let layer = self.layers.iter().find(|layer| layer.store == store)?;
        if let Some(blocked) = &layer.blocked {
            return Some(blocked.clone());
        }
        let invalid: Vec<_> = self
            .registered
            .iter()
            .filter(|setting| setting.invalid.contains(&store))
            .map(|setting| setting.key.as_ref())
            .collect();
        (!invalid.is_empty()).then(|| format!("Invalid values for: {}", invalid.join(", ")))
    }

    fn resolve<S: Setting>(&self) -> Option<(StoreId, S::Value)> {
        self.layers
            .iter()
            .rev()
            .filter(|layer| S::allowed_in(layer.store))
            .find_map(|layer| decode::<S>(layer).map(|value| (layer.store, value)))
    }

    fn layer_mut(&mut self, store: StoreId) -> Result<&mut Layer, String> {
        self.layers
            .iter_mut()
            .find(|layer| layer.store == store)
            .ok_or_else(|| format!("{} isn't a settings layer", store.name()))
    }
}

fn decode<S: Setting>(layer: &Layer) -> Option<S::Value> {
    layer.values.get(S::key().as_ref())?.into_rust().ok()
}

fn encode<T: Serialize>(value: &T) -> Result<Box<RawValue>, String> {
    RawValue::from_rust(value).map_err(|e| e.to_string())
}

/// Set a setting in one layer.
pub fn set_setting<S: Setting>(
    world: &mut World,
    store: StoreId,
    value: S::Value,
) -> Result<(), String> {
    if !S::allowed_in(store) {
        return Err(format!("{} can't be set in {}", S::key(), store.name()));
    }
    let encoded = encode(&value)?;
    let mut settings = world.resource_mut::<Settings>();
    let layer = settings.layer_mut(store)?;
    layer.values.insert(S::key().into_owned(), encoded);
    layer.dirty = true;
    apply_changed(world);
    Ok(())
}

/// Remove a setting from one layer, so it inherits from the layers below.
pub fn clear_setting<S: Setting>(world: &mut World, store: StoreId) -> Result<(), String> {
    let mut settings = world.resource_mut::<Settings>();
    let layer = settings.layer_mut(store)?;
    if layer.values.remove(S::key().as_ref()).is_some() {
        layer.dirty = true;
        apply_changed(world);
    }
    Ok(())
}

/// Run `apply` for every registered setting whose resolved value changed.
fn apply_changed(world: &mut World) {
    let count = world.resource::<Settings>().registered.len();
    for index in 0..count {
        let settings = world.resource::<Settings>();
        let setting = &settings.registered[index];
        let resolved = (setting.resolve)(settings);
        if setting.applied.as_ref() == Some(&resolved) {
            continue;
        }
        let apply = setting.apply;
        world.resource_mut::<Settings>().registered[index].applied = Some(resolved);
        apply(world);
    }
}

/// Note values that don't decode, and report newly found ones.
fn check_values(world: &mut World) {
    let mut found = Vec::new();
    let mut settings = world.resource_mut::<Settings>();
    for index in 0..settings.registered.len() {
        let invalid = (settings.registered[index].check)(&settings);
        let setting = &mut settings.registered[index];
        for &store in &invalid {
            if !setting.invalid.contains(&store) {
                found.push(format!("{} in {}", setting.key, store.name()));
            }
        }
        setting.invalid = invalid;
    }
    if !found.is_empty() {
        crate::closing::report_error(
            world,
            "Invalid settings",
            "Some settings have values that can't be used; their defaults apply instead.",
            found.join("\n"),
        );
    }
}

pub trait SettingsAppExt {
    /// Add a settings layer kept in store `S`. Later layers take precedence.
    /// Without a backend for `S`, the layer lives in memory only.
    fn add_settings_layer<S: Store>(&mut self) -> &mut Self;
    fn register_setting<S: Setting>(&mut self) -> &mut Self;
}

impl SettingsAppExt for App {
    fn add_settings_layer<S: Store>(&mut self) -> &mut Self {
        let mut settings = self.world_mut().get_resource_or_init::<Settings>();
        let store = StoreId::of::<S>();
        assert!(
            settings.layers.iter().all(|layer| layer.store != store),
            "{} was added as a settings layer twice",
            store.name()
        );
        settings.layers.push(Layer {
            store,
            values: BTreeMap::new(),
            dirty: false,
            blocked: None,
            version: None,
            save_error: None,
        });
        self
    }

    fn register_setting<S: Setting>(&mut self) -> &mut Self {
        let mut settings = self.world_mut().get_resource_or_init::<Settings>();
        assert!(
            settings.registered.iter().all(|r| r.key != S::key()),
            "Setting {} was registered twice",
            S::key()
        );
        settings.registered.push(Registered {
            key: S::key(),
            resolve: |settings| ron::to_string(&settings.get::<S>()).unwrap_or_default(),
            apply: |world| {
                let value = world.resource::<Settings>().get::<S>();
                S::apply(world, &value);
            },
            applied: None,
            invalid: Vec::new(),
            check: |settings| {
                settings
                    .layers
                    .iter()
                    .filter(|layer| {
                        layer.values.contains_key(S::key().as_ref())
                            && (!S::allowed_in(layer.store) || decode::<S>(layer).is_none())
                    })
                    .map(|layer| layer.store)
                    .collect()
            },
        });
        self
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Settings>()
        .add_systems(PreStartup, load_layers.in_set(StoreSystems::LoadSettings))
        .add_systems(
            Last,
            (reload_changed_layers, save_layers).before(crate::SessionSystems::Close),
        );
}

/// One setting's value as stored in `backend`'s layer, read directly, e.g. to
/// decide something before the app is built. `None` when it isn't set there or
/// can't be read.
pub fn read_setting<S: Setting>(backend: &dyn StoreBackend) -> Option<S::Value> {
    let bytes = backend.read(ENTRY).ok()??;
    let text = std::str::from_utf8(&bytes).ok()?;
    let values: BTreeMap<String, Box<RawValue>> = ron::from_str(text).ok()?;
    values.get(S::key().as_ref())?.into_rust().ok()
}

/// Read one layer's file. `None` means it doesn't exist yet.
fn read_layer(
    world: &World,
    store: StoreId,
) -> Result<Option<(BTreeMap<String, Box<RawValue>>, Option<String>)>, String> {
    let Some(backend) = world.resource::<Stores>().backend(store) else {
        return Ok(None);
    };
    let version = backend.version(ENTRY)?;
    let Some(bytes) = backend.read(ENTRY)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let values = ron::from_str(text).map_err(|e| e.to_string())?;
    Ok(Some((values, version)))
}

fn load_layers(world: &mut World) {
    let stores: Vec<_> = world.resource::<Settings>().layers().collect();
    for store in stores {
        let result = read_layer(world, store);
        let mut settings = world.resource_mut::<Settings>();
        let layer = settings.layer_mut(store).unwrap();
        match result {
            Ok(Some((values, version))) => {
                layer.values = values;
                layer.version = version;
            }
            Ok(None) => {}
            Err(error) => {
                layer.blocked = Some(error.clone());
                crate::closing::report_error(
                    world,
                    "Couldn't load settings",
                    "Settings couldn't be read, so they won't be saved over.",
                    format!("{}: {error}", store.name()),
                );
            }
        }
    }
    check_values(world);
    apply_changed(world);
}

/// A layer whose file changed on disk (edited by hand, or written by another
/// instance) is reloaded; last writer wins.
fn reload_changed_layers(world: &mut World, mut last: Local<Option<Instant>>) {
    if last.is_some_and(|at| at.elapsed() < CHECK_INTERVAL) {
        return;
    }
    *last = Some(Instant::now());
    let candidates: Vec<_> = world
        .resource::<Settings>()
        .layers
        .iter()
        .filter(|layer| !layer.dirty)
        .map(|layer| (layer.store, layer.version.clone()))
        .collect();
    let mut changed = false;
    for (store, known) in candidates {
        let Some(backend) = world.resource::<Stores>().backend(store) else {
            continue;
        };
        let Ok(version) = backend.version(ENTRY) else {
            continue;
        };
        if version.is_none() || version == known {
            continue;
        }
        let result = read_layer(world, store);
        let mut settings = world.resource_mut::<Settings>();
        let layer = settings.layer_mut(store).unwrap();
        match result {
            Ok(loaded) => {
                let (values, version) = loaded.unwrap_or_default();
                layer.values = values;
                layer.version = version;
                layer.blocked = None;
                changed = true;
            }
            Err(error) => {
                // Remember this version so the same bad file isn't re-read every second.
                layer.version = version;
                if layer.blocked.is_none() {
                    layer.blocked = Some(error.clone());
                    crate::closing::report_error(
                        world,
                        "Couldn't load settings",
                        "Settings changed on disk but couldn't be read; they won't be saved over.",
                        format!("{}: {error}", store.name()),
                    );
                }
            }
        }
    }
    if changed {
        check_values(world);
        apply_changed(world);
    }
}

/// Dirty layers are written about once a second, and on exit.
fn save_layers(world: &mut World, mut last: Local<Option<Instant>>) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !exiting && last.is_some_and(|at| at.elapsed() < CHECK_INTERVAL) {
        return;
    }
    *last = Some(Instant::now());
    let dirty: Vec<_> = world
        .resource::<Settings>()
        .layers
        .iter()
        .filter(|layer| layer.dirty && layer.blocked.is_none())
        .map(|layer| layer.store)
        .collect();
    for store in dirty {
        let Some(backend) = world.resource::<Stores>().backend(store) else {
            world
                .resource_mut::<Settings>()
                .layer_mut(store)
                .unwrap()
                .dirty = false;
            continue;
        };
        let settings = world.resource::<Settings>();
        let layer = settings.layers.iter().find(|l| l.store == store).unwrap();
        let result = ron::ser::to_string_pretty(&layer.values, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
            .and_then(|text| backend.write(ENTRY, text.as_bytes()))
            .and_then(|()| backend.version(ENTRY));
        match result {
            Ok(version) => {
                let mut settings = world.resource_mut::<Settings>();
                let layer = settings.layer_mut(store).unwrap();
                layer.dirty = false;
                layer.version = version;
                layer.save_error = None;
            }
            Err(error) => {
                // Stays dirty, so the next save retries. Report each failure once.
                let mut settings = world.resource_mut::<Settings>();
                let layer = settings.layer_mut(store).unwrap();
                if layer.save_error.as_ref() == Some(&error) {
                    continue;
                }
                layer.save_error = Some(error.clone());
                crate::closing::report_error(
                    world,
                    "Couldn't save settings",
                    "Settings couldn't be saved. Saving will keep retrying.",
                    format!("{}: {error}", store.name()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
