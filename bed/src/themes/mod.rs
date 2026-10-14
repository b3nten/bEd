//! bEd's theme catalog, color-file editing and ordered runtime projection.
pub(crate) mod editor;
mod file;
mod files;
mod persistence;
#[cfg(test)]
pub(crate) mod tests;
pub use file::{DEFAULT_THEME, ThemeDraft, ThemeFile};
pub use files::ThemeFiles;

use bedtime_api::settings::{
    SettingsRequest, SettingsRequestId, SettingsResults, SettingsSnapshot,
};
use bevy::prelude::*;
use std::{collections::HashMap, path::PathBuf};

/// The last valid file remains active when an external edit cannot be loaded.
#[derive(Resource, Clone)]
pub struct ActiveTheme {
    pub selection: String,
    pub file: ThemeFile,
    pub error: Option<String>,
    contents: Vec<u8>,
}
impl Default for ActiveTheme {
    fn default() -> Self {
        Self {
            selection: DEFAULT_THEME.into(),
            file: ThemeFile::bundled_default(),
            error: None,
            contents: Vec::new(),
        }
    }
}
impl ActiveTheme {
    pub fn runtime_palette(&self) -> bedtime_ui::appearance::UiPalette {
        self.file.runtime_palette()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeSaveId(u64);
#[derive(Message, Clone)]
pub struct SaveTheme {
    pub id: ThemeSaveId,
    destination: Option<String>,
    file: ThemeFile,
    apply: bool,
}
impl SaveTheme {
    pub fn new(destination: Option<String>, file: ThemeFile, apply: bool) -> Self {
        Self {
            id: ThemeSaveId(bedtime_api::features::next_request_sequence()),
            destination,
            file,
            apply,
        }
    }
}

pub struct ThemeSaved {
    pub selection: String,
    pub apply_error: Option<String>,
}
#[derive(Resource, Default)]
pub struct ThemeResults(HashMap<ThemeSaveId, Result<ThemeSaved, String>>);
impl ThemeResults {
    pub fn take(&mut self, id: ThemeSaveId) -> Option<Result<ThemeSaved, String>> {
        self.0.remove(&id)
    }
}

#[derive(Resource, Default)]
struct PendingApplies(Vec<(SettingsRequestId, ThemeSaveId, String)>);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeSystems {
    Configure,
    Save,
}

pub struct ThemePlugin {
    resources_root: PathBuf,
}
impl ThemePlugin {
    pub fn new(resources_root: PathBuf) -> Self {
        Self { resources_root }
    }
}
impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        install_theme_services(app.world_mut(), self.resources_root.clone());
        app.add_message::<SaveTheme>()
            .add_message::<SettingsRequest>()
            .add_systems(
                PreUpdate,
                (finish_applies, configure_themes)
                    .chain()
                    .in_set(ThemeSystems::Configure)
                    .in_set(bedtime_desktop::DesktopSystems::Configure)
                    .after(bedtime_core::CoreConfigurationSystems::Settings)
                    .before(bedtime_core::CoreSystems::Services),
            )
            .add_systems(
                PreUpdate,
                save_themes
                    .in_set(ThemeSystems::Save)
                    .in_set(bedtime_desktop::DesktopSystems::AfterUi)
                    .after(bedtime_core::CoreSystems::Requests),
            );
    }
}

/// Headless bEd fixtures explicitly install the same app-owned theme resources.
/// Constructing them reads no files and writes no directories.
pub fn install_theme_services(world: &mut World, resources_root: PathBuf) {
    if !world.contains_resource::<ThemeFiles>() {
        let config_dir = world
            .resource::<bedtime_api::launch::ApplicationPaths>()
            .config_dir
            .clone();
        bedtime_core::settings::install_settings(world);
        bedtime_api::settings::register(
            world,
            "bed.application",
            vec![
                bedtime_api::settings::SettingDefinition::new("theme", "Theme", DEFAULT_THEME)
                    .validated_by(validate_selection),
            ],
        )
        .expect("bEd theme preferences install once");
        world.insert_resource(ThemeFiles::new(config_dir, resources_root));
    }
    world.init_resource::<ActiveTheme>();
    world.init_resource::<ThemeResults>();
    world.init_resource::<PendingApplies>();
    if !world.contains_resource::<Messages<SaveTheme>>() {
        bevy::ecs::message::MessageRegistry::register_message::<SaveTheme>(world);
    }
    if !world.contains_resource::<Messages<SettingsRequest>>() {
        bevy::ecs::message::MessageRegistry::register_message::<SettingsRequest>(world);
    }
}

/// Load and publish one complete palette before document and panel preparation.
pub fn configure_themes(world: &mut World) {
    let files = world.resource::<ThemeFiles>().clone();
    let selection = world
        .resource::<SettingsSnapshot>()
        .get("bed.application", "theme")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(DEFAULT_THEME)
        .to_owned();
    let result = (|| -> std::io::Result<Option<(ThemeFile, Vec<u8>)>> {
        let path = files.theme_path(&selection)?;
        let bytes = std::fs::read(&path).map_err(|error| {
            std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        let active = world.resource::<ActiveTheme>();
        if active.selection == selection && active.contents == bytes {
            return Ok(None);
        }
        let value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        let file = ThemeFile::from_json(&value)?;
        Ok(Some((file, bytes)))
    })();
    let mut active = world.resource_mut::<ActiveTheme>();
    match result {
        Ok(Some((file, contents))) => {
            active.selection = selection;
            active.file = file;
            active.contents = contents;
            active.error = None;
        }
        Ok(None) => active.error = None,
        Err(error) => active.error = Some(error.to_string()),
    }
    let file = active.file.clone();
    let palette = file.runtime_palette();
    if let Some(mut theme) = world.get_resource_mut::<bedtime_desktop::DesktopTheme>() {
        theme.0.palette = palette.clone();
    }
    if world
        .get_resource::<bedtime_core::SyntaxAppearance>()
        .is_none_or(|appearance| appearance.0 != file.syntax)
    {
        world.insert_resource(bedtime_core::SyntaxAppearance(file.syntax));
    }
    world.insert_resource(bedtime_api::presentation::TerminalPalette(file.terminal));
}

fn save_themes(
    mut requests: MessageReader<SaveTheme>,
    files: Res<ThemeFiles>,
    mut results: ResMut<ThemeResults>,
    mut pending: ResMut<PendingApplies>,
    mut settings: MessageWriter<SettingsRequest>,
) {
    for request in requests.read() {
        match files.save_custom_theme(request.destination.as_deref(), &request.file.to_json()) {
            Ok(selection) if request.apply => {
                let apply =
                    SettingsRequest::new("bed.application", "theme", selection.clone()).tracked();
                pending.0.push((
                    apply.receipt.expect("Tracked theme selection"),
                    request.id,
                    selection,
                ));
                settings.write(apply);
            }
            Ok(selection) => {
                results.0.insert(
                    request.id,
                    Ok(ThemeSaved {
                        selection,
                        apply_error: None,
                    }),
                );
            }
            Err(error) => {
                results.0.insert(request.id, Err(error.to_string()));
            }
        }
    }
}

fn finish_applies(
    mut pending: ResMut<PendingApplies>,
    mut settings: ResMut<SettingsResults>,
    mut results: ResMut<ThemeResults>,
) {
    pending.0.retain(|(receipt, id, selection)| {
        let Some(result) = settings.take(*receipt) else {
            return true;
        };
        results.0.insert(
            *id,
            Ok(ThemeSaved {
                selection: selection.clone(),
                apply_error: result
                    .err()
                    .map(|error| format!("Theme saved, but could not apply it: {error}")),
            }),
        );
        false
    });
}

fn validate_selection(value: &serde_json::Value) -> Result<(), String> {
    let selection = value.as_str().expect("Theme selection type was validated");
    ThemeFiles::new(PathBuf::new(), PathBuf::new())
        .theme_path(selection)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn process_theme_requests(app: &mut App) {
    let id = app.world_mut().register_system(save_themes);
    app.world_mut().run_system(id).unwrap();
    app.world_mut().unregister_system(id).unwrap();
    app.world_mut()
        .resource_mut::<Messages<SaveTheme>>()
        .clear();
}
