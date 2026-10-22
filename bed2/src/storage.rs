//! Application-owned global preferences I/O. Workspace I/O lives in session.
use crate::{
    layouts::{LayoutDefaults, LayoutTemplate},
    settings::{GlobalPreferences, Settings, StartupPreferences},
};
use bedtime_ui2::egui;
use bedtime_workbench2::{ErrorDialog, request_error_dialog};
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
    layouts: [LayoutTemplate; 5],
}
impl Document {
    fn capture(world: &World) -> Self {
        Self {
            version: 1,
            settings: world.resource::<GlobalPreferences>().0.clone(),
            startup: world.resource::<StartupPreferences>().clone(),
            layouts: world.resource::<LayoutDefaults>().0.clone(),
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported preferences version".into());
        }
        self.settings.validate()?;
        for template in &self.layouts {
            template.validate()?;
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
fn read(path: &Path) -> Result<Option<Document>, String> {
    read_bytes(path)?
        .map(|bytes| {
            let document: Document = ron::de::from_bytes(&bytes).map_err(|e| e.to_string())?;
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
    loaded: bool,
    job: Option<JoinHandle<Result<Option<Document>, String>>>,
    writing: Option<Document>,
    changed_at: Instant,
    error: Option<String>,
    reported: bool,
}
impl PreferenceFile {
    fn new(path: PathBuf, initial: Document) -> Self {
        let source = path.clone();
        Self {
            path,
            baseline: initial.clone(),
            desired: initial,
            saved: None,
            loaded: false,
            job: Some(std::thread::spawn(move || read(&source))),
            writing: None,
            changed_at: Instant::now(),
            error: None,
            reported: false,
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
                    Ok(_) => self.saved = Some(written),
                    Err(e) => self.error = Some(e),
                }
            } else {
                self.loaded = true;
                match result {
                    Ok(Some(disk)) => {
                        let mut merged = self.desired.clone();
                        merged.settings = Settings::merge_loaded(
                            &self.baseline.settings,
                            &merged.settings,
                            &disk.settings,
                        );
                        if merged.startup.open_last_workspace
                            == self.baseline.startup.open_last_workspace
                        {
                            merged.startup.open_last_workspace = disk.startup.open_last_workspace;
                        }
                        if merged.startup.last_workspace == self.baseline.startup.last_workspace {
                            merged.startup.last_workspace = disk.startup.last_workspace.clone();
                        }
                        for i in 0..5 {
                            if merged.layouts[i] == self.baseline.layouts[i] {
                                merged.layouts[i] = disk.layouts[i].clone();
                            }
                        }
                        self.saved = Some(disk);
                        self.desired = merged.clone();
                        loaded = Some(merged);
                    }
                    Ok(None) => self.saved = Some(self.baseline.clone()),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(error) = &self.error {
            return Err(format!("{}: {error}", self.path.display()));
        }
        if self.loaded
            && self.job.is_none()
            && self.saved.as_ref() != Some(&self.desired)
            && (flush || self.changed_at.elapsed() >= Duration::from_millis(250))
        {
            self.desired.validate()?;
            let bytes = encode(&self.desired)?;
            let path = self.path.clone();
            self.writing = Some(self.desired.clone());
            self.job = Some(std::thread::spawn(move || {
                // Never silently replace newly introduced malformed/newer data.
                read(&path)?;
                atomic_write(&path, &bytes).map(|_| None)
            }));
        }
        Ok(loaded)
    }
    fn retry(&mut self) {
        if self.job.is_some() {
            return;
        }
        let baseline = self.saved.clone().unwrap_or_else(|| self.baseline.clone());
        let mut replacement = Self::new(self.path.clone(), baseline);
        replacement.desired = self.desired.clone();
        *self = replacement;
    }
    fn finished(&self) -> bool {
        self.loaded
            && self.error.is_none()
            && self.job.is_none()
            && self.saved.as_ref() == Some(&self.desired)
    }
}
#[derive(Resource, Default)]
struct PreferencesFile(Option<PreferenceFile>);
pub fn install(app: &mut App) {
    app.init_resource::<PreferencesFile>()
        .init_resource::<LayoutDefaults>()
        .init_resource::<StartupPreferences>();
}
pub fn configure(app: &mut App) {
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(directory) = directory {
        configure_directory(app, directory.join("bed2"));
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
        .is_none_or(|file| file.loaded && file.saved.is_some() && file.error.is_none())
}
pub fn ready(world: &World, workspace: Entity) -> bool {
    crate::files::root_ready(world, workspace)
        && global_ready(world)
        && crate::session::preferences_ready(world, workspace)
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
        ".bed2-{}-{}.tmp",
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
    fn host() -> (App, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed2-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = App::new();
        app.add_plugins(bedtime_workbench2::WorkbenchCorePlugin)
            .init_resource::<GlobalPreferences>();
        install(&mut app);
        (app, root)
    }
    fn settle(app: &mut App) -> Result<(), String> {
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
        atomic_write(&path, &encode(&disk).unwrap()).unwrap();
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
        assert!(!root.join("settings2.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn older_preferences_default_to_not_reopening_workspaces() {
        let (app, root) = host();
        #[derive(Serialize)]
        struct OlderPreferences {
            version: u32,
            settings: Settings,
            layouts: [LayoutTemplate; 5],
        }
        let value = Document::capture(app.world());
        let bytes = encode(&OlderPreferences {
            version: value.version,
            settings: value.settings,
            layouts: value.layouts,
        })
        .unwrap();
        let document: Document = ron::de::from_bytes(&bytes).unwrap();
        document.validate().unwrap();
        assert!(!document.startup.open_last_workspace);
        assert_eq!(document.startup.last_workspace, None);
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
        atomic_write(&path, &encode(&repaired).unwrap()).unwrap();
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
        let workspace = bedtime_workbench2::Workspaces::new(app.world_mut())
            .open("Temporary")
            .entity();
        app.world_mut().entity_mut(workspace).insert((
            crate::files::WorkspaceRoot(root.join("home")),
            crate::settings::WorkspacePreferences(crate::settings::SettingsOverrides {
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
        let mut future = Document::capture(app.world());
        future.version = 2;
        atomic_write(&path, &encode(&future).unwrap()).unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
