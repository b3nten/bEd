//! Settings layers: what each holds, loading, saving and reloading them.
use crate::settings::{Setting, SettingsError, SettingsWorldExt};
use bedtime_persistence::{Store, StoreBackend, StoreError, StoreErrorKind, StoreId, Stores};
use bevy::prelude::*;
use ron::value::RawValue;
use serde::Serialize;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// A layer's entry in its store: `settings.ron` in a `DirectoryStore`.
pub const SETTINGS_ENTRY: &str = "settings";
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Changing settings: `world.settings_mut()`. Reading is the [`Settings`]
/// resource (`world.settings()`).
#[repr(transparent)]
pub struct SettingsMut {
    world: World,
}

impl SettingsMut {
    pub(crate) fn of(world: &mut World) -> &mut Self {
        // SAFETY: SettingsMut is a repr(transparent) wrapper of World.
        unsafe { &mut *(world as *mut World).cast::<Self>() }
    }
}

/// All layers, lowest precedence first, and the registered settings.
#[derive(Resource, Default)]
pub struct Settings {
    layers: Vec<Layer>,
    registered: Vec<Registered>,
    /// Defaults the app set in place of settings' own (`set_setting_default`).
    defaults: Vec<(Cow<'static, str>, Box<dyn std::any::Any + Send + Sync>)>,
}

struct Layer {
    store: StoreId,
    /// What the layer is called in the settings panel ("Global").
    label: Cow<'static, str>,
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
            .unwrap_or_else(|| self.default_of::<S>())
    }

    /// Which layer the value comes from, or `None` for the default.
    pub fn source<S: Setting>(&self) -> Option<StoreId> {
        self.resolve::<S>().map(|(store, _)| store)
    }

    /// The value as `store` and the layers below it give it, ignoring layers
    /// above: what a settings editor shows while editing that layer.
    pub fn get_at<S: Setting>(&self, store: StoreId) -> S::Value {
        let below = self
            .layers
            .iter()
            .position(|layer| layer.store == store)
            .map_or(0, |index| index + 1);
        self.layers[..below]
            .iter()
            .rev()
            .filter(|layer| S::allowed_in(layer.store))
            .find_map(decode::<S>)
            .unwrap_or_else(|| self.default_of::<S>())
    }

    /// The layer above `store` whose different value is in use instead of
    /// `store`'s: changes in `store` won't show while it sets one. A layer
    /// above that sets the same value overrides nothing.
    pub fn overridden_above<S: Setting>(&self, store: StoreId) -> Option<StoreId> {
        let index = self.layers.iter().position(|layer| layer.store == store)?;
        let (source, value) = self.resolve::<S>()?;
        let above = self.layers[index + 1..]
            .iter()
            .any(|layer| layer.store == source);
        (above && value != self.get_at::<S>(store)).then_some(source)
    }

    /// Make `value` the app's default for the setting `key` (its value type).
    pub(crate) fn set_default_value(
        &mut self,
        key: Cow<'static, str>,
        value: Box<dyn std::any::Any + Send + Sync>,
    ) {
        self.defaults.retain(|(known, _)| *known != key);
        self.defaults.push((key, value));
    }

    /// The value with nothing set in any layer: the app's default for it
    /// (`set_setting_default`), else the setting's own.
    pub fn default_of<S: Setting>(&self) -> S::Value {
        let key = S::key();
        self.defaults
            .iter()
            .find(|(known, _)| *known == key)
            .and_then(|(_, value)| value.downcast_ref::<S::Value>())
            .cloned()
            .unwrap_or_else(S::default)
    }

    /// The value `store` would have without its own: from the layers below
    /// it, or the default.
    pub fn inherited<S: Setting>(&self, store: StoreId) -> S::Value {
        let below = self
            .layers
            .iter()
            .position(|layer| layer.store == store)
            .unwrap_or(0);
        self.layers[..below]
            .iter()
            .rev()
            .filter(|layer| S::allowed_in(layer.store))
            .find_map(decode::<S>)
            .unwrap_or_else(|| self.default_of::<S>())
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

    /// What a layer is called in the settings panel.
    pub fn label(&self, store: StoreId) -> Option<&str> {
        let layer = self.layers.iter().find(|layer| layer.store == store)?;
        Some(&layer.label)
    }

    /// The layer below `store`, if any.
    pub fn below(&self, store: StoreId) -> Option<StoreId> {
        let index = self.layers.iter().position(|layer| layer.store == store)?;
        index.checked_sub(1).map(|below| self.layers[below].store)
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

    /// A new, empty top layer for `store`, labelled after it.
    fn push_layer(&mut self, store: StoreId) {
        let name = store.name().rsplit("::").next().unwrap_or(store.name());
        self.layers.push(Layer {
            store,
            label: name.strip_suffix("Store").unwrap_or(name).to_owned().into(),
            values: BTreeMap::new(),
            dirty: false,
            blocked: None,
            version: None,
            save_error: None,
        });
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

impl SettingsMut {
    /// Set a setting in one layer.
    pub fn set<S: Setting>(&mut self, store: StoreId, value: S::Value) -> Result<(), String> {
        let world = &mut self.world;
        if !S::allowed_in(store) {
            return Err(format!("{} can't be set in {}", S::key(), store.name()));
        }
        // Setting what the layer would inherit anyway stores nothing, so it keeps
        // following the layers below (a workspace set back to the global value
        // follows later global changes).
        if value == world.resource::<Settings>().inherited::<S>(store) {
            return world.settings_mut().clear::<S>(store);
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
    pub fn clear<S: Setting>(&mut self, store: StoreId) -> Result<(), String> {
        let world = &mut self.world;
        let mut settings = world.resource_mut::<Settings>();
        let layer = settings.layer_mut(store)?;
        if layer.values.remove(S::key().as_ref()).is_some() {
            layer.dirty = true;
            apply_changed(world);
        }
        Ok(())
    }
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
        world.write_message(SettingsError(format!(
            "Some settings can't be used, so their defaults apply: {}",
            found.join(", ")
        )));
    }
}

pub trait SettingsAppExt {
    /// Add a settings layer kept in store `S`. Later layers take precedence.
    /// Without a backend for `S`, the layer lives in memory only.
    fn add_settings_layer<S: Store>(&mut self) -> &mut Self;
    /// What layer `S` is called in the settings panel. By default its store's
    /// name without "Store" (`GlobalStore` is "Global").
    fn label_settings_layer<S: Store>(&mut self, label: impl Into<Cow<'static, str>>) -> &mut Self;
    fn register_setting<S: Setting>(&mut self) -> &mut Self;
    /// Use `value` as setting `S`'s default, in place of the one it was
    /// declared with: an app's own default for a plugin's setting. Before or
    /// after the setting is registered; the latest call wins.
    fn set_setting_default<S: Setting>(&mut self, value: S::Value) -> &mut Self;
}

impl SettingsAppExt for App {
    fn set_setting_default<S: Setting>(&mut self, value: S::Value) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<Settings>()
            .set_default_value(S::key(), Box::new(value));
        self
    }

    fn add_settings_layer<S: Store>(&mut self) -> &mut Self {
        let store = StoreId::of::<S>();
        let mut settings = self.world_mut().get_resource_or_init::<Settings>();
        assert!(
            settings.layers.iter().all(|layer| layer.store != store),
            "{} was added as a settings layer twice",
            store.name()
        );
        settings.push_layer(store);
        self
    }

    fn label_settings_layer<S: Store>(&mut self, label: impl Into<Cow<'static, str>>) -> &mut Self {
        let mut settings = self.world_mut().get_resource_or_init::<Settings>();
        let layer = settings
            .layer_mut(StoreId::of::<S>())
            .expect("Add the settings layer before labelling it");
        layer.label = label.into();
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
        #[cfg(feature = "ui")]
        crate::settings::ui::register::<S>(self);
        self
    }
}

/// One setting's value as stored in `backend`'s layer, read directly, e.g. to
/// decide something before the app is built. `None` when it isn't set there or
/// can't be read.
pub fn read_setting<S: Setting>(backend: &dyn StoreBackend) -> Option<S::Value> {
    let bytes = backend.read(SETTINGS_ENTRY).ok()??;
    let text = std::str::from_utf8(&bytes).ok()?;
    let values: BTreeMap<String, Box<RawValue>> = ron::from_str(text).ok()?;
    values.get(S::key().as_ref())?.into_rust().ok()
}

/// A layer's values and the version of the file they came from.
type LayerFile = (BTreeMap<String, Box<RawValue>>, Option<String>);

/// Read one layer's file. `None` means it doesn't exist yet.
fn read_layer(world: &World, store: StoreId) -> Result<Option<LayerFile>, String> {
    let Some(backend) = world
        .get_resource::<Stores>()
        .and_then(|stores| stores.backend(store))
    else {
        return Ok(None);
    };
    let version = backend.version(SETTINGS_ENTRY)?;
    let Some(bytes) = backend.read(SETTINGS_ENTRY)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let values = ron::from_str(text).map_err(|e| e.to_string())?;
    Ok(Some((values, version)))
}

pub(crate) fn load_layers(world: &mut World) {
    let stores: Vec<_> = world.resource::<Settings>().layers().collect();
    for store in stores {
        read_into_layer(world, store);
    }
    check_values(world);
    apply_changed(world);
}

/// Point the top layer at `store` while the app runs (a workspace switch):
/// the layer that was there for it is dropped (save it first, with
/// `save_layer`), and with `label` a new one is read from the store's
/// current backend. Settings that changed are applied.
pub(crate) fn replace_layer(world: &mut World, store: StoreId, label: Option<String>) {
    let mut settings = world.resource_mut::<Settings>();
    settings.layers.retain(|layer| layer.store != store);
    if let Some(label) = label {
        settings.push_layer(store);
        settings.layer_mut(store).unwrap().label = label.into();
        read_into_layer(world, store);
    }
    check_values(world);
    apply_changed(world);
}

/// Write layer `store` now if it changed.
pub(crate) fn save_layer(world: &mut World, store: StoreId) {
    let dirty = world
        .resource::<Settings>()
        .layers
        .iter()
        .any(|layer| layer.store == store && layer.dirty && layer.blocked.is_none());
    if dirty {
        write_layer(world, store);
    }
}

fn read_into_layer(world: &mut World, store: StoreId) {
    {
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
                world.write_message(StoreError {
                    store,
                    entry: SETTINGS_ENTRY.into(),
                    kind: StoreErrorKind::Load,
                    error,
                });
            }
        }
    }
}

/// A layer whose file changed on disk (edited by hand, or written by another
/// instance) is reloaded; last writer wins.
pub(crate) fn reload_changed_layers(world: &mut World, mut last: Local<Option<Instant>>) {
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
        let Some(backend) = world
            .get_resource::<Stores>()
            .and_then(|stores| stores.backend(store))
        else {
            continue;
        };
        let Ok(version) = backend.version(SETTINGS_ENTRY) else {
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
                    world.write_message(StoreError {
                        store,
                        entry: SETTINGS_ENTRY.into(),
                        kind: StoreErrorKind::Load,
                        error,
                    });
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
pub(crate) fn save_layers(world: &mut World, mut last: Local<Option<Instant>>) {
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
        write_layer(world, store);
    }
}

/// Write layer `store` through its backend (none: just mark it saved).
fn write_layer(world: &mut World, store: StoreId) {
    let Some(backend) = world
        .get_resource::<Stores>()
        .and_then(|stores| stores.backend(store))
    else {
        world
            .resource_mut::<Settings>()
            .layer_mut(store)
            .unwrap()
            .dirty = false;
        return;
    };
    let settings = world.resource::<Settings>();
    let layer = settings.layers.iter().find(|l| l.store == store).unwrap();
    let result = ron::ser::to_string_pretty(&layer.values, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())
        .and_then(|text| backend.write(SETTINGS_ENTRY, text.as_bytes()))
        .and_then(|()| backend.version(SETTINGS_ENTRY));
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
                return;
            }
            layer.save_error = Some(error.clone());
            world.write_message(StoreError {
                store,
                entry: SETTINGS_ENTRY.into(),
                kind: StoreErrorKind::Save,
                error,
            });
        }
    }
}
