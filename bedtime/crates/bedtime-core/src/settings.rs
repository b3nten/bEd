//! Global preference transactions. This owner alone publishes values and writes disk.
use bedtime_api::settings::{
    SettingDefinition, SettingsChange, SettingsRegistry, SettingsRequest, SettingsResults,
    SettingsSnapshot,
};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// A headless core keeps preferences in memory. The launcher supplies a disk path.
#[derive(Resource)]
pub struct SettingsStore {
    path: Option<PathBuf>,
    overrides: Value,
    disk_contents: Option<Vec<u8>>,
    registry_revision: u64,
    revision: u64,
}
impl Default for SettingsStore {
    fn default() -> Self {
        Self {
            path: None,
            overrides: json!({}),
            disk_contents: None,
            registry_revision: 0,
            revision: 0,
        }
    }
}

pub fn install_settings(world: &mut World) {
    world.init_resource::<SettingsRegistry>();
    world.init_resource::<SettingsStore>();
    world.init_resource::<SettingsSnapshot>();
    world.init_resource::<SettingsResults>();
    for (owner, definitions) in [
        (
            "bedtime.workbench",
            vec![
                SettingDefinition::new(
                    "keybindings",
                    "Keybindings",
                    serde_json::to_value(SettingsSnapshot::default().keybindings())
                        .expect("Keybindings serialize"),
                )
                .validated_by(bedtime_api::settings::validate_keybindings),
                SettingDefinition::new("tab_bar_add_button", "Show add panel button", false),
                SettingDefinition::new("tab_icons", "Show tab icons", true),
            ],
        ),
        (
            "bedtime.ui",
            vec![
                SettingDefinition::new("font", "Font family", "Paper Mono"),
                SettingDefinition::new("fontSize", "Font size", 20.0).validated_by(font_size),
                SettingDefinition::new("background_opacity", "Background opacity", 1.0)
                    .validated_by(opacity),
                SettingDefinition::new("ui_animations", "Animate interface", true),
            ],
        ),
        (
            "bedtime.documents",
            vec![
                SettingDefinition::new("default_viewers", "Default file viewers", json!({}))
                    .validated_by(file_viewers),
                SettingDefinition::new("autosave", "Autosave", true),
                SettingDefinition::new("autosave_delay_ms", "Autosave delay (ms)", 1000.0)
                    .validated_by(autosave_delay),
            ],
        ),
    ] {
        let missing = definitions
            .into_iter()
            .filter(|definition| {
                world
                    .resource::<SettingsRegistry>()
                    .definition(owner, definition.key)
                    .is_none()
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bedtime_api::settings::register(world, owner, missing)
                .expect("Core defaults are valid");
        }
    }
    crate::settings_view::register(world);
}

fn font_size(value: &Value) -> Result<(), String> {
    if value
        .as_f64()
        .is_some_and(|value| (4.0..=64.0).contains(&value))
    {
        Ok(())
    } else {
        Err("Font size must be between 4 and 64".into())
    }
}
fn opacity(value: &Value) -> Result<(), String> {
    if value
        .as_f64()
        .is_some_and(|value| (0.0..=1.0).contains(&value))
    {
        Ok(())
    } else {
        Err("Opacity must be between 0 and 1".into())
    }
}
fn autosave_delay(value: &Value) -> Result<(), String> {
    let milliseconds = value.as_f64().ok_or("Autosave delay must be a number")?;
    std::time::Duration::try_from_secs_f64(milliseconds / 1000.0)
        .map(|_| ())
        .map_err(|_| "Autosave delay must be a nonnegative supported duration".into())
}
fn file_viewers(value: &Value) -> Result<(), String> {
    if value.as_object().is_some_and(|viewers| {
        viewers.iter().all(|(extension, viewer)| {
            !extension.is_empty()
                && viewer
                    .as_str()
                    .is_some_and(bedtime_api::workbench::valid_id)
        })
    }) {
        Ok(())
    } else {
        Err("Default viewers must map nonempty extensions to viewer identifiers".into())
    }
}

/// Select the global settings file before the application starts. No workspace is needed.
pub fn initialize_settings(world: &mut World, path: Option<PathBuf>) -> io::Result<()> {
    install_settings(world);
    world.resource_mut::<SettingsStore>().path = path;
    reload_settings(world).map_err(io::Error::other)
}

/// Application defaults replace registered defaults, never the user's overrides.
pub fn set_application_defaults(
    world: &mut World,
    owner: &str,
    defaults: impl IntoIterator<Item = (String, Value)>,
) -> io::Result<()> {
    world
        .resource_mut::<SettingsRegistry>()
        .set_defaults(owner, &defaults.into_iter().collect::<Vec<_>>())
        .map_err(io::Error::other)
}

/// Publish the full registry after all feature and application registrations are installed.
pub fn finalize_settings(world: &mut World) -> io::Result<()> {
    reload_settings(world).map_err(io::Error::other)?;
    world.resource_mut::<SettingsRegistry>().seal();
    Ok(())
}

/// Update the document service policy without changing its context, history, or transports.
pub fn configure_document_settings(world: &mut World) -> io::Result<()> {
    let Some(session) = world.get_non_send::<bedtime_documents::DocumentSession>() else {
        return Ok(());
    };
    let mut options = session.options().clone();
    let settings = world.resource::<SettingsSnapshot>();
    let enabled = settings
        .get("bedtime.documents", "autosave")
        .and_then(Value::as_bool)
        .ok_or_else(|| io::Error::other("Document preferences have not been initialized"))?;
    let milliseconds = settings
        .get("bedtime.documents", "autosave_delay_ms")
        .and_then(Value::as_f64)
        .expect("Registered autosave delay was validated");
    options.autosave = enabled.then(|| {
        std::time::Duration::try_from_secs_f64(milliseconds / 1000.0)
            .expect("Registered autosave delay was validated")
    });
    if &options != session.options() {
        world
            .non_send_mut::<bedtime_documents::DocumentSession>()
            .configure(options)?;
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn read_disk(path: &Path) -> io::Result<(Value, Option<Vec<u8>>)> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((json!({}), None)),
        Err(error) => return Err(error),
    };
    let value: Value = serde_json::from_slice(&bytes)?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Settings file must be an object"))?;
    if object.len() != 2 || value["version"].as_u64() != Some(1) || !value["overrides"].is_object()
    {
        return Err(invalid(
            "Unsupported settings format; expected version 1 and namespaced overrides",
        ));
    }
    Ok((value["overrides"].clone(), Some(bytes)))
}

fn effective(registry: &SettingsRegistry, overrides: &Value) -> Result<Value, String> {
    let overrides = overrides
        .as_object()
        .ok_or("Settings overrides must be an object")?;
    for (owner, namespace) in overrides {
        if !bedtime_api::workbench::valid_id(owner) {
            return Err(format!("Invalid settings owner: {owner}"));
        }
        let namespace = namespace
            .as_object()
            .ok_or_else(|| format!("Settings namespace {owner} must be an object"))?;
        for (key, value) in namespace {
            if !bedtime_api::workbench::valid_id(key) {
                return Err(format!("Invalid setting key: {owner}.{key}"));
            }
            if let Some(definition) = registry.definition(owner, key) {
                definition
                    .validate_value(value)
                    .map_err(|error| format!("{owner}.{key}: {error}"))?;
            }
        }
    }
    let mut values = overrides.clone();
    for (owner, definitions) in registry.owners() {
        let namespace = values
            .entry(owner)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("Namespace validated");
        for definition in definitions {
            namespace
                .entry(definition.key)
                .or_insert_with(|| definition.default.clone());
        }
    }
    Ok(Value::Object(values))
}

fn publish(world: &mut World, values: Value, error: Option<String>) {
    let registry_revision = world.resource::<SettingsRegistry>().revision();
    let changed = world.resource::<SettingsSnapshot>().settings != values;
    let revision = {
        let mut store = world.resource_mut::<SettingsStore>();
        store.registry_revision = registry_revision;
        if changed {
            store.revision = store
                .revision
                .checked_add(1)
                .expect("Settings revision exhausted");
        }
        store.revision
    };
    let default_viewers = values["bedtime.documents"]["default_viewers"]
        .as_object()
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));
    world.insert_resource(SettingsSnapshot {
        settings: values,
        default_viewers,
        revision,
        error,
    });
}

fn report_error(world: &mut World, error: String) {
    world.resource_mut::<SettingsSnapshot>().error = Some(error);
}

/// Refresh disk and newly registered defaults without overwriting a malformed file.
pub fn reload_settings(world: &mut World) -> Result<(), String> {
    install_settings(world);
    let path = world.resource::<SettingsStore>().path.clone();
    let result = match path {
        Some(path) => read_disk(&path).map_err(|error| format!("{}: {error}", path.display())),
        None => Ok((world.resource::<SettingsStore>().overrides.clone(), None)),
    };
    let (overrides, bytes) = match result {
        Ok(value) => value,
        Err(error) => {
            let previous = world.resource::<SettingsStore>().overrides.clone();
            if let Ok(values) = effective(world.resource::<SettingsRegistry>(), &previous) {
                publish(world, values, Some(error.clone()));
            } else {
                report_error(world, error.clone());
            }
            return Err(error);
        }
    };
    let values = match effective(world.resource::<SettingsRegistry>(), &overrides) {
        Ok(values) => values,
        Err(error) => {
            let previous = world.resource::<SettingsStore>().overrides.clone();
            if let Ok(values) = effective(world.resource::<SettingsRegistry>(), &previous) {
                publish(world, values, Some(error.clone()));
            } else {
                report_error(world, error.clone());
            }
            return Err(error);
        }
    };
    {
        let mut store = world.resource_mut::<SettingsStore>();
        store.overrides = overrides;
        store.disk_contents = bytes;
    }
    publish(world, values, None);
    Ok(())
}

/// Runtime polling uses file bytes as the change detector; invalid edits retain valid values.
pub fn refresh_settings(world: &mut World) {
    let registry_changed = world.resource::<SettingsStore>().registry_revision
        != world.resource::<SettingsRegistry>().revision();
    let disk_changed = world
        .resource::<SettingsStore>()
        .path
        .as_ref()
        .is_some_and(|path| match fs::read(path) {
            Ok(bytes) => world.resource::<SettingsStore>().disk_contents.as_ref() != Some(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                world.resource::<SettingsStore>().disk_contents.is_some()
            }
            Err(_) => true,
        });
    if registry_changed || disk_changed {
        let _ = reload_settings(world);
    }
}

fn apply_changes(
    registry: &SettingsRegistry,
    mut overrides: Value,
    changes: &[SettingsChange],
) -> Result<Value, String> {
    for change in changes {
        let (owner, key) = change.owner_key();
        let definition = registry
            .definition(owner, key)
            .ok_or_else(|| format!("Setting is not registered: {owner}.{key}"))?;
        if let SettingsChange::Set { value, .. } = change {
            definition.validate_value(value)?;
        }
    }
    let root = overrides
        .as_object_mut()
        .expect("Stored overrides validated");
    for change in changes {
        match change {
            SettingsChange::Set { owner, key, value } => {
                let namespace = root
                    .entry(owner.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("Stored namespace validated");
                namespace.insert(key.clone(), value.clone());
            }
            SettingsChange::Reset { owner, key } => {
                if let Some(namespace) = root.get_mut(owner).and_then(Value::as_object_mut) {
                    namespace.remove(key);
                    if namespace.is_empty() {
                        root.remove(owner);
                    }
                }
            }
        }
    }
    Ok(overrides)
}

fn lock_file(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Settings file needs a parent directory"))?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn write_atomic(path: &Path, overrides: &Value) -> io::Result<Vec<u8>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut bytes = serde_json::to_vec_pretty(&json!({"version":1,"overrides":overrides}))?;
    bytes.push(b'\n');
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|()| bytes)
}

/// Apply one complete ordered transaction; disk validation and I/O precede publication.
pub fn apply_settings_request(world: &mut World, request: &SettingsRequest) -> Result<u64, String> {
    install_settings(world);
    let result: Result<u64, String> = (|| {
        let path = world.resource::<SettingsStore>().path.clone();
        let registry = world.resource::<SettingsRegistry>().clone();
        let (overrides, bytes) = if let Some(path) = path {
            let _lock = lock_file(&path).map_err(|error| error.to_string())?;
            let (latest, _) =
                read_disk(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            effective(&registry, &latest)?;
            let overrides = apply_changes(&registry, latest, &request.changes)?;
            let _ = effective(&registry, &overrides)?;
            let bytes = write_atomic(&path, &overrides).map_err(|error| error.to_string())?;
            (overrides, Some(bytes))
        } else {
            (
                apply_changes(
                    &registry,
                    world.resource::<SettingsStore>().overrides.clone(),
                    &request.changes,
                )?,
                None,
            )
        };
        let values = effective(&registry, &overrides)?;
        {
            let mut store = world.resource_mut::<SettingsStore>();
            store.overrides = overrides;
            store.disk_contents = bytes;
        }
        publish(world, values, None);
        Ok(world.resource::<SettingsSnapshot>().revision)
    })();
    if let Err(error) = &result {
        report_error(world, error.clone());
    }
    if let Some(id) = request.receipt {
        world
            .resource_mut::<SettingsResults>()
            .0
            .insert(id, result.clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::temp_dir;

    fn owner(path: &Path) -> World {
        let mut world = World::new();
        install_settings(&mut world);
        bedtime_api::settings::register(
            &mut world,
            "fixture",
            vec![
                SettingDefinition::new("enabled", "Enabled", true),
                SettingDefinition::new("count", "Count", 3),
            ],
        )
        .unwrap();
        initialize_settings(&mut world, Some(path.to_owned())).unwrap();
        world
    }

    #[test]
    fn batch_is_atomic_registered_and_reset_restores_default() {
        let dir = temp_dir::TempDir::new();
        let path = dir.path("settings.json");
        let mut world = owner(&path);
        let request = SettingsRequest::batch(vec![
            SettingsChange::set("fixture", "enabled", false),
            SettingsChange::set("fixture", "count", "wrong type"),
        ])
        .tracked();
        assert!(apply_settings_request(&mut world, &request).is_err());
        assert_eq!(
            world
                .resource::<SettingsSnapshot>()
                .get("fixture", "enabled"),
            Some(&json!(true))
        );
        assert!(!path.exists());
        assert!(
            world
                .resource_mut::<SettingsResults>()
                .take(request.receipt.unwrap())
                .unwrap()
                .is_err()
        );
        apply_settings_request(
            &mut world,
            &SettingsRequest::new("fixture", "enabled", false),
        )
        .unwrap();
        assert_eq!(
            owner(&path)
                .resource::<SettingsSnapshot>()
                .get("fixture", "enabled"),
            Some(&json!(false))
        );
        apply_settings_request(&mut world, &SettingsRequest::reset("fixture", "enabled")).unwrap();
        assert_eq!(
            world
                .resource::<SettingsSnapshot>()
                .get("fixture", "enabled"),
            Some(&json!(true))
        );
        assert!(
            apply_settings_request(&mut world, &SettingsRequest::new("fixture", "unknown", 1))
                .is_err()
        );
    }

    #[test]
    fn independent_windows_merge_changes_and_keep_absent_plugin_overrides() {
        let dir = temp_dir::TempDir::new();
        let path = dir.path("settings.json");
        fs::write(
            &path,
            br#"{"version":1,"overrides":{"absent.plugin":{"draft":{"future":1}}}}"#,
        )
        .unwrap();
        let mut first = owner(&path);
        let mut second = owner(&path);
        apply_settings_request(
            &mut first,
            &SettingsRequest::new("fixture", "enabled", false),
        )
        .unwrap();
        apply_settings_request(&mut second, &SettingsRequest::new("fixture", "count", 8)).unwrap();
        reload_settings(&mut first).unwrap();
        let snapshot = first.resource::<SettingsSnapshot>();
        assert_eq!(snapshot.get("fixture", "enabled"), Some(&json!(false)));
        assert_eq!(snapshot.get("fixture", "count"), Some(&json!(8)));
        assert_eq!(
            snapshot.get("absent.plugin", "draft"),
            Some(&json!({"future":1}))
        );
    }

    #[test]
    fn simultaneous_writers_commit_both_keys_under_one_disk_lock() {
        let dir = temp_dir::TempDir::new();
        let path = dir.path("settings.json");
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for change in [
                SettingsChange::set("fixture", "enabled", false),
                SettingsChange::set("fixture", "count", 19),
            ] {
                let path = &path;
                let start = &start;
                scope.spawn(move || {
                    let mut world = owner(path);
                    start.wait();
                    apply_settings_request(&mut world, &SettingsRequest::batch(vec![change]))
                        .unwrap();
                });
            }
        });
        let world = owner(&path);
        assert_eq!(
            world
                .resource::<SettingsSnapshot>()
                .get("fixture", "enabled"),
            Some(&json!(false))
        );
        assert_eq!(
            world.resource::<SettingsSnapshot>().get("fixture", "count"),
            Some(&json!(19))
        );
    }

    #[test]
    fn malformed_startup_and_reload_never_replace_the_file_or_live_values() {
        let dir = temp_dir::TempDir::new();
        let path = dir.path("settings.json");
        let mut world = owner(&path);
        apply_settings_request(&mut world, &SettingsRequest::new("fixture", "count", 7)).unwrap();
        fs::write(&path, "{broken").unwrap();
        assert!(reload_settings(&mut world).is_err());
        assert_eq!(
            world.resource::<SettingsSnapshot>().get("fixture", "count"),
            Some(&json!(7))
        );
        assert!(
            apply_settings_request(&mut world, &SettingsRequest::new("fixture", "count", 9))
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "{broken");
        let mut restarted = World::new();
        install_settings(&mut restarted);
        bedtime_api::settings::register(
            &mut restarted,
            "fixture",
            vec![SettingDefinition::new("count", "Count", 3)],
        )
        .unwrap();
        assert!(initialize_settings(&mut restarted, Some(path.clone())).is_err());
        assert_eq!(
            restarted
                .resource::<SettingsSnapshot>()
                .get("fixture", "count"),
            Some(&json!(3))
        );
        assert!(restarted.resource::<SettingsSnapshot>().error.is_some());
        fs::write(
            &path,
            br#"{"version":1,"overrides":{"fixture":{"count":11}}}"#,
        )
        .unwrap();
        reload_settings(&mut world).unwrap();
        assert_eq!(
            world.resource::<SettingsSnapshot>().get("fixture", "count"),
            Some(&json!(11))
        );
    }

    #[test]
    fn keybindings_are_validated_and_persisted_with_global_preferences() {
        let dir = temp_dir::TempDir::new();
        let path = dir.path("settings.json");
        let mut world = owner(&path);
        assert!(
            apply_settings_request(
                &mut world,
                &SettingsRequest::new(
                    "bedtime.workbench",
                    "keybindings",
                    json!({"version":99,"actions":{}})
                )
            )
            .is_err()
        );
        let overrides =
            json!({"version":1,"actions":{"fixture.run":[{"shortcut":"Primary+R","order":1}]}});
        apply_settings_request(
            &mut world,
            &SettingsRequest::new("bedtime.workbench", "keybindings", overrides),
        )
        .unwrap();
        assert_eq!(
            owner(&path)
                .resource::<SettingsSnapshot>()
                .keybindings()
                .actions
                .len(),
            1
        );
        let contents: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(contents["overrides"]["bedtime.workbench"]["keybindings"].is_object());
    }

    #[test]
    fn document_preferences_change_autosave_without_replacing_the_session() {
        let dir = temp_dir::TempDir::new();
        let mut world = owner(&dir.path("settings.json"));
        let session =
            bedtime_documents::DocumentSession::with_options(bedtime_documents::SessionOptions {
                project_root: Some(dir.path("")),
                ..Default::default()
            })
            .unwrap();
        let identity = session.session_id();
        let root = session.options().project_root.clone();
        world.insert_non_send(session);
        configure_document_settings(&mut world).unwrap();
        assert_eq!(
            world
                .non_send::<bedtime_documents::DocumentSession>()
                .options()
                .autosave,
            Some(std::time::Duration::from_secs(1))
        );
        apply_settings_request(
            &mut world,
            &SettingsRequest::new("bedtime.documents", "autosave", false),
        )
        .unwrap();
        configure_document_settings(&mut world).unwrap();
        let session = world.non_send::<bedtime_documents::DocumentSession>();
        assert_eq!(session.session_id(), identity);
        assert_eq!(session.options().project_root, root);
        assert_eq!(session.options().autosave, None);
        assert!(
            apply_settings_request(
                &mut world,
                &SettingsRequest::new("bedtime.documents", "autosave_delay_ms", 1e100)
            )
            .is_err()
        );
    }
}
