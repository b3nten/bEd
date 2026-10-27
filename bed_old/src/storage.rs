//! Application-owned global preferences I/O. Workspace I/O lives in session.
use crate::{
    layouts::LayoutDefaults,
    settings::{GlobalPreferences, Settings, StartupPreferences},
};
use bedtime_ui::egui;
use bedtime_workbench::{ErrorDialog, request_error_dialog};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Resource, Clone)]
pub struct ConfigDirectory(pub PathBuf);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    settings: Settings,
    #[serde(default)]
    startup: StartupPreferences,
    layouts: [Option<bedtime_workbench::persistence::Snapshot>; 5],
    #[serde(default)]
    bindings: crate::keybindings::GlobalBindings,
}

impl Document {
    fn capture(world: &World) -> Self {
        Self {
            version: 1,
            settings: world.resource::<GlobalPreferences>().0.clone(),
            startup: world.resource::<StartupPreferences>().clone(),
            layouts: world.resource::<LayoutDefaults>().0.clone(),
            bindings: world
                .get_resource::<crate::keybindings::GlobalBindings>()
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Independent values changed locally win; untouched values follow disk.
    fn merge(baseline: &Self, live: &Self, disk: &Self) -> Self {
        let mut result = live.clone();
        result.bindings.0 =
            crate::keybindings::merge(&baseline.bindings.0, &live.bindings.0, &disk.bindings.0);
        result.settings =
            Settings::merge_loaded(&baseline.settings, &live.settings, &disk.settings);
        if live.startup.open_last_workspace == baseline.startup.open_last_workspace {
            result.startup.open_last_workspace = disk.startup.open_last_workspace;
        }
        if live.startup.last_workspace == baseline.startup.last_workspace {
            result.startup.last_workspace = disk.startup.last_workspace.clone();
        }
        for i in 0..5 {
            if live.layouts[i] == baseline.layouts[i] {
                result.layouts[i] = disk.layouts[i].clone();
            }
        }
        result
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported preferences version".into());
        }
        self.settings.validate()?;
        if let Some(path) = &self.startup.last_workspace {
            crate::persistence::validate_absolute_path(path)?;
        }
        crate::keybindings::validate(&self.bindings.0)?;
        let registry = preference_registry();
        for snapshot in self.layouts.iter().flatten() {
            let captured =
                bedtime_workbench::persistence::CapturedLayout::deserialize(snapshot, &registry)?;
            crate::files::validate_layout_default(&captured)?;
            crate::git::validate_layout_default(&captured)?;
        }
        Ok(())
    }
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let text = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err("Configuration exceeds 4 MiB; previous file preserved".into());
    }
    Ok(text.into_bytes())
}

pub fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Configuration is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Configuration exceeds 4 MiB".into());
    }
    Ok(Some(bytes))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPreferences {
    version: u32,
    values: bedtime_workbench::persistence::Snapshot,
}

fn register_types(app: &mut App) {
    crate::persistence::register(app);
    app.register_type::<GlobalPreferences>()
        .register_type::<StartupPreferences>()
        .register_type::<LayoutDefaults>()
        .register_type::<crate::keybindings::GlobalBindings>();
}

fn preference_registry() -> AppTypeRegistry {
    let mut app = App::new();
    register_types(&mut app);
    app.world().resource::<AppTypeRegistry>().clone()
}

fn encode_preferences(document: &Document) -> Result<Vec<u8>, String> {
    let mut world = World::new();
    world.insert_resource(preference_registry());
    world.insert_resource(GlobalPreferences(document.settings.clone()));
    world.insert_resource(document.startup.clone());
    world.insert_resource(LayoutDefaults(document.layouts.clone()));
    world.insert_resource(document.bindings.clone());
    let values = bedtime_workbench::Workbench::capture_world::<
        crate::persistence::ReflectGlobalPersist,
    >(&mut world)?
    .serialize(world.resource::<AppTypeRegistry>())?;
    encode(&StoredPreferences { version: 2, values })
}

fn read(path: &Path) -> Result<Option<Document>, String> {
    read_bytes(path)?
        .map(|bytes| {
            let stored: StoredPreferences = ron::de::from_bytes(&bytes)
                .map_err(|e| format!("Invalid preferences; original file preserved: {e}"))?;
            if stored.version != 2 {
                return Err("Unsupported preferences version; original file preserved".into());
            }
            let prepared = bedtime_workbench::persistence::CapturedWorld::<
                crate::persistence::ReflectGlobalPersist,
            >::deserialize(&stored.values, &preference_registry())?;
            if prepared.has_entities() {
                return Err("Global preferences contain entities".into());
            }
            let document = Document {
                version: 1,
                settings: prepared
                    .resource::<GlobalPreferences>()
                    .ok_or("Missing global settings")?
                    .0,
                startup: prepared
                    .resource::<StartupPreferences>()
                    .ok_or("Missing startup settings")?,
                layouts: prepared
                    .resource::<LayoutDefaults>()
                    .ok_or("Missing layout defaults")?
                    .0,
                bindings: prepared
                    .resource::<crate::keybindings::GlobalBindings>()
                    .ok_or("Missing global bindings")?,
            };
            document.validate()?;
            Ok(document)
        })
        .transpose()
}

struct PreferenceFile {
    path: PathBuf,
    baseline: Document,
    desired: Document,
    saved: Option<Document>,
    // Once admitted, policy survives later persistence/refresh failures.
    policy_ready: bool,
    job: Option<JoinHandle<Result<Option<Document>, String>>>,
    writing: Option<Document>,
    changed_at: Instant,
    error: Option<String>,
    reported: bool,
    checked_at: Instant,
}

impl PreferenceFile {
    fn new(path: PathBuf, initial: Document) -> Self {
        let source = path.clone();
        Self {
            path,
            baseline: initial.clone(),
            desired: initial,
            saved: None,
            policy_ready: false,
            job: Some(std::thread::spawn(move || read(&source))),
            writing: None,
            changed_at: Instant::now(),
            error: None,
            reported: false,
            checked_at: Instant::now(),
        }
    }

    fn tick(&mut self, desired: Document, flush: bool) -> Result<Option<Document>, String> {
        if desired != self.desired {
            self.desired = desired;
            self.changed_at = Instant::now();
        }
        let mut loaded = None;
        if self.job.as_ref().is_some_and(|job| job.is_finished()) {
            let result = self
                .job
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err("Preferences worker panicked".into()));
            if let Some(written) = self.writing.take() {
                match result {
                    Ok(Some(disk)) => {
                        self.desired = Document::merge(&written, &self.desired, &disk);
                        self.saved = Some(disk);
                        loaded = Some(self.desired.clone());
                    }
                    Ok(None) => unreachable!("Preference writes return their committed snapshot"),
                    Err(e) => self.error = Some(e),
                }
            } else {
                match result {
                    Ok(Some(disk)) => {
                        let merged = Document::merge(&self.baseline, &self.desired, &disk);
                        self.saved = Some(disk);
                        self.policy_ready = true;
                        self.desired = merged.clone();
                        loaded = Some(merged);
                    }
                    Ok(None) => {
                        self.saved = Some(self.baseline.clone());
                        self.policy_ready = true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(error) = &self.error {
            return Err(format!("{}: {error}", self.path.display()));
        }
        if self.policy_ready
            && self.job.is_none()
            && self.saved.as_ref() != Some(&self.desired)
            && (flush || self.changed_at.elapsed() >= Duration::from_millis(250))
        {
            self.desired.validate()?;
            let candidate = self.desired.clone();
            let baseline = self.saved.clone().unwrap_or_else(|| self.baseline.clone());
            let path = self.path.clone();
            self.writing = Some(candidate.clone());
            self.job = Some(std::thread::spawn(move || {
                // Serialize read/merge/replace across instances. Atomic replacement
                // alone cannot prevent a valid but stale writer losing other edits.
                let parent = path.parent().ok_or("Preferences have no parent")?;
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                let lock = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(parent.join("preferences.lock"))
                    .map_err(|e| e.to_string())?;
                lock.lock().map_err(|e| e.to_string())?;
                let disk = read(&path)?.unwrap_or_else(|| baseline.clone());
                let merged = Document::merge(&baseline, &candidate, &disk);
                merged.validate()?;
                atomic_write(&path, &encode_preferences(&merged)?)?;
                Ok(Some(merged))
            }));
            self.checked_at = Instant::now();
        } else if self.policy_ready
            && self.job.is_none()
            && !flush
            && self.checked_at.elapsed() >= Duration::from_secs(1)
        {
            self.baseline = self.saved.clone().unwrap_or_else(|| self.baseline.clone());
            let source = self.path.clone();
            self.job = Some(std::thread::spawn(move || read(&source)));
            self.checked_at = Instant::now();
        }
        Ok(loaded)
    }

    fn retry(&mut self) {
        if self.job.is_some() {
            return;
        }
        self.baseline = self.saved.clone().unwrap_or_else(|| self.baseline.clone());
        let source = self.path.clone();
        self.job = Some(std::thread::spawn(move || read(&source)));
        self.error = None;
        self.reported = false;
    }

    fn finished(&self) -> bool {
        self.policy_ready
            && self.error.is_none()
            && self.job.is_none()
            && self.saved.as_ref() == Some(&self.desired)
    }
}

#[derive(Resource, Default)]
struct PreferencesFile(Option<PreferenceFile>);
pub fn install(app: &mut App) {
    register_types(app);
    app.init_resource::<crate::keybindings::GlobalBindings>();
    app.init_resource::<PreferencesFile>()
        .init_resource::<LayoutDefaults>()
        .init_resource::<StartupPreferences>();
}

pub fn configure(app: &mut App) {
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(directory) = directory {
        configure_directory(app, directory.join("bed"));
    } else {
        request_error_dialog(
            app.world_mut(),
            ErrorDialog {
                title: "Settings".into(),
                message:
                    "No user configuration directory is available; settings will stay in memory."
                        .into(),
                details: None,
            },
        );
    }
}

pub fn configure_directory(app: &mut App, directory: PathBuf) {
    let initial = Document::capture(app.world());
    app.insert_resource(PreferencesFile(Some(PreferenceFile::new(
        directory.join("preferences.bed"),
        initial,
    ))))
    .insert_resource(ConfigDirectory(directory));
}

pub fn global_ready(world: &World) -> bool {
    world
        .get_resource::<PreferencesFile>()
        .and_then(|f| f.0.as_ref())
        .is_none_or(|file| file.policy_ready)
}

pub fn ready(world: &World) -> bool {
    crate::files::root_ready(world)
        && global_ready(world)
        && crate::session::preferences_ready(world)
}

pub fn update(world: &mut World) {
    let _ = drive(world, false);
}

pub fn flush(world: &mut World) -> Result<bool, String> {
    drive(world, true)
}

fn drive(world: &mut World, flush: bool) -> Result<bool, String> {
    if !world.contains_resource::<PreferencesFile>() {
        return Ok(true);
    }
    world.resource_scope(|world, mut state: Mut<PreferencesFile>| {
        let Some(file) = &mut state.0 else { return Ok(true); };
        let result = file.tick(Document::capture(world), flush);
        match result {
            Ok(Some(ref loaded)) => {
                world.resource_mut::<GlobalPreferences>().0 = loaded.settings.clone();
                world.resource_mut::<LayoutDefaults>().0 = loaded.layouts.clone();
                *world.resource_mut::<StartupPreferences>() = loaded.startup.clone();
                *world.resource_mut::<crate::keybindings::GlobalBindings>() = loaded.bindings.clone();
            }
            Err(ref error) if !file.reported => {
                request_error_dialog(world, ErrorDialog { title: "Settings".into(), message: "Preferences couldn't be read or saved. Repair the file or access problem, then retry in Settings.".into(), details: Some(error.clone()) });
                file.reported = true;
            }
            _ => {}
        }
        result.map(|_| file.finished())
    })
}

pub fn retry_failed(world: &mut World) {
    if let Some(mut state) = world.get_resource_mut::<PreferencesFile>()
        && let Some(file) = &mut state.0
        && file.error.is_some()
    {
        file.retry();
    }
}

pub fn draw_errors(ui: &mut egui::Ui, world: &mut World) {
    let error = world
        .get_resource::<PreferencesFile>()
        .and_then(|s| s.0.as_ref())
        .and_then(|f| f.error.clone());
    if error.is_some() && ui.button("Retry preferences storage").clicked() {
        retry_failed(world);
    }
}

pub fn workspace_path(world: &World, root: &Path) -> Option<PathBuf> {
    let digest = blake3::hash(root.as_os_str().as_encoded_bytes());
    world
        .get_resource::<ConfigDirectory>()
        .map(|config| config.0.join("workspaces").join(format!("{digest}.bed")))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    if bytes.len() > MAX_BYTES {
        return Err("Configuration exceeds 4 MiB; previous file preserved".into());
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or("Configuration destination has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(
        ".bed-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(temporary);
        return Err(format!("{}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(super) fn host() -> (App, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = App::new();
        app.add_plugins(bedtime_workbench::WorkbenchCorePlugin)
            .init_resource::<GlobalPreferences>();
        install(&mut app);
        (app, root)
    }

    pub(super) fn settle(app: &mut App) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if flush(app.world_mut())? {
                return Ok(());
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn separate_instances_merge_independent_values_and_refresh_without_rewriting() {
        let (mut first, root) = host();
        let (mut second, unused) = host();
        configure_directory(&mut first, root.clone());
        configure_directory(&mut second, root.clone());
        settle(&mut first).unwrap();
        settle(&mut second).unwrap();
        first
            .world_mut()
            .resource_mut::<GlobalPreferences>()
            .0
            .font_size = 24.0;
        second
            .world_mut()
            .resource_mut::<GlobalPreferences>()
            .0
            .panel_gap = 13.0;
        // Both instances read the same baseline before either writes.
        settle(&mut first).unwrap();
        settle(&mut second).unwrap();
        let disk = read(&root.join("preferences.bed")).unwrap().unwrap();
        assert_eq!(disk.settings.font_size, 24.0);
        assert_eq!(disk.settings.panel_gap, 13.0);
        assert_eq!(
            second.world().resource::<GlobalPreferences>().0.font_size,
            24.0
        );
        let bytes = std::fs::read(root.join("preferences.bed")).unwrap();
        first
            .world_mut()
            .resource_mut::<PreferencesFile>()
            .0
            .as_mut()
            .unwrap()
            .checked_at = Instant::now() - Duration::from_secs(2);
        update(first.world_mut());
        let deadline = Instant::now() + Duration::from_secs(5);
        while first.world().resource::<GlobalPreferences>().0.panel_gap != 13.0 {
            update(first.world_mut());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(std::fs::read(root.join("preferences.bed")).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(unused).unwrap();
    }

    #[test]
    fn global_ron_roundtrip_merges_late_values_and_does_not_write_on_startup() {
        let (mut app, root) = host();
        let mut disk = Document::capture(app.world());
        disk.startup.open_last_workspace = true;
        disk.startup.last_workspace = Some(root.join("project"));
        disk.settings.line_numbers = false;
        disk.settings.panel_gap = 14.0;
        disk.settings.effects_enabled = true;
        disk.settings.curvature_intensity = 0.6;
        disk.settings.burn_in_intensity = 0.8;
        let path = root.join("preferences.bed");
        atomic_write(&path, &encode_preferences(&disk).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        configure_directory(&mut app, root.clone());
        app.world_mut()
            .resource_mut::<GlobalPreferences>()
            .0
            .panel_gap = 11.0;
        settle(&mut app).unwrap();
        let stored = read(&path).unwrap().unwrap();
        assert!(stored.startup.open_last_workspace);
        assert_eq!(stored.startup.last_workspace, Some(root.join("project")));
        assert!(!stored.settings.line_numbers);
        assert_eq!(stored.settings.panel_gap, 11.0);
        assert!(stored.settings.effects_enabled);
        assert_eq!(stored.settings.curvature_intensity, 0.6);
        assert_eq!(stored.settings.burn_in_intensity, 0.8);
        assert_ne!(std::fs::read(&path).unwrap(), before);
        let bytes = std::fs::read(&path).unwrap();
        configure_directory(&mut app, root.clone());
        settle(&mut app).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(
            app.world()
                .resource::<GlobalPreferences>()
                .0
                .effects_enabled
        );
        assert_eq!(
            app.world()
                .resource::<GlobalPreferences>()
                .0
                .curvature_intensity,
            0.6
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_read_preserves_bytes_and_retry_merges_repaired_values() {
        let (mut app, root) = host();
        let path = root.join("preferences.bed");
        std::fs::write(&path, "broken").unwrap();
        configure_directory(&mut app, root.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while flush(app.world_mut()).is_ok() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        app.world_mut()
            .resource_mut::<GlobalPreferences>()
            .0
            .panel_gap = 13.0;
        assert!(flush(app.world_mut()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        let mut repaired = Document::capture(app.world());
        repaired.settings.panel_gap = 20.0;
        repaired.settings.line_numbers = false;
        atomic_write(&path, &encode_preferences(&repaired).unwrap()).unwrap();
        retry_failed(app.world_mut());
        settle(&mut app).unwrap();
        assert_eq!(
            app.world().resource::<GlobalPreferences>().0.panel_gap,
            13.0
        );
        assert!(!app.world().resource::<GlobalPreferences>().0.line_numbers);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn temporary_workspace_settings_never_persist_but_global_changes_do() {
        let (mut app, root) = host();
        app.add_plugins(crate::session::SessionPlugin);
        configure_directory(&mut app, root.join("config"));
        let workspace = crate::test_layout(app.world_mut()).entity();
        app.world_mut().entity_mut(workspace).insert((
            crate::files::ProjectRoot(root.join("home")),
            crate::settings::ProjectPreferences(crate::settings::SettingsOverrides {
                panel_gap: Some(25.0),
                ..default()
            }),
        ));
        app.world_mut()
            .resource_mut::<GlobalPreferences>()
            .0
            .panel_gap = 9.0;
        settle(&mut app).unwrap();
        assert!(crate::session::flush(app.world_mut()).unwrap());
        assert!(root.join("config/preferences.bed").exists());
        assert!(!root.join("config/workspaces").exists());
        assert!(!root.join("home/.bed").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_write_and_unsupported_version_preserve_existing_data() {
        let (app, root) = host();
        let path = root.join("preferences.bed");
        atomic_write(&path, b"original").unwrap();
        assert!(atomic_write(&path, &vec![b'x'; MAX_BYTES + 1]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let bytes = encode_preferences(&Document::capture(app.world())).unwrap();
        let mut future: StoredPreferences = ron::de::from_bytes(&bytes).unwrap();
        future.version += 1;
        let bytes = encode(&future).unwrap();
        atomic_write(&path, &bytes).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn previous_application_type_paths_are_rejected_without_overwriting() {
        let (mut app, root) = host();
        let path = root.join("preferences.bed");
        let current =
            String::from_utf8(encode_preferences(&Document::capture(app.world())).unwrap())
                .unwrap();
        let previous = current.replace("bed::", "bed2::");
        assert_ne!(
            current, previous,
            "fixture must exercise the former namespace"
        );
        atomic_write(&path, previous.as_bytes()).unwrap();
        configure_directory(&mut app, root.clone());
        assert!(settle(&mut app).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), previous.as_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_value_preferences_are_rejected_without_overwriting() {
        let (mut app, root) = host();
        let path = root.join("preferences.bed");
        let bytes = encode(&Document::capture(app.world())).unwrap();
        atomic_write(&path, &bytes).unwrap();
        configure_directory(&mut app, root.clone());
        assert!(settle(&mut app).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }
}
