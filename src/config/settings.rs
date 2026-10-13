//! bEd's namespaced preference draft. Core owns effective settings and persistence.
use bed_api::settings::{SettingsChange, SettingsRequest, SettingsRequestId, SettingsSnapshot};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io;
#[cfg(test)]
use std::path::Path;
use std::{fs, path::PathBuf, time::Duration};

pub const SETTINGS_FILE: &str = "settings.json";

pub fn bundled_default_viewers() -> Value {
    let defaults: Value =
        serde_json::from_str(include_str!("../../resources/config/settings.json"))
            .expect("bundled settings must be valid JSON");
    defaults["bed.documents"]["default_viewers"].clone()
}

pub use bed_plugin_effects::settings::EffectPreset;

#[cfg(test)]
pub(crate) fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub struct Settings {
    pub settings: Value,
    pub effect_preset: EffectPreset,
    pub sidebar_visible: bool,
    pub terminal_visible: bool,
    pub request_lsp_dashboard: bool,
    pub request_config_file: Option<PathBuf>,
    pub is_embedded: bool,
    pub config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub settings_path: PathBuf,
    defaults: Value,
    submitted: Value,
    outbox: VecDeque<SettingsRequest>,
    pending: VecDeque<(SettingsRequestId, Vec<(String, String)>)>,
    needs_apply: bool,
    pub error: Option<String>,
}

impl Settings {
    pub fn with_paths(config_dir: PathBuf, resources_root: PathBuf) -> io::Result<Self> {
        let settings: Value =
            serde_json::from_str(include_str!("../../resources/config/settings.json"))?;
        let submitted = settings.clone();
        let view = Self {
            settings,
            effect_preset: EffectPreset::Off,
            sidebar_visible: true,
            terminal_visible: true,
            request_lsp_dashboard: false,
            request_config_file: None,
            is_embedded: false,
            settings_path: config_dir.join(SETTINGS_FILE),
            config_dir,
            resources_root,
            defaults: submitted.clone(),
            submitted,
            outbox: VecDeque::new(),
            pending: VecDeque::new(),
            needs_apply: true,
            error: None,
        };
        Ok(view)
    }

    pub fn get_app_resources_path() -> PathBuf {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut candidates = vec![
            cwd.clone(),
            cwd.join("bed"),
            cwd.join("../bed"),
            cwd.join("../../bed"),
            cwd.join("bed/bed"),
        ];
        if let Ok(binary) = std::env::current_exe()
            && let Some(dir) = binary.parent()
        {
            #[cfg(target_os = "macos")]
            candidates.push(dir.join("../Resources"));
            candidates.extend([dir.to_owned(), dir.join(".."), dir.join("../share/Bed")]);
        }
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        candidates
            .into_iter()
            .find(|path| path.join("resources/config").is_dir())
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .unwrap_or(cwd)
    }

    /// Capture one complete save intention now; admission preserves this sequence.
    pub fn request_save(&mut self) {
        let current = self.settings.clone();
        let mut changes = Vec::new();
        for (owner, namespace) in current.as_object().expect("Draft namespaces") {
            for (key, value) in namespace.as_object().expect("Draft namespace fields") {
                if self.submitted[owner][key] != *value {
                    changes.push(SettingsChange::set(owner, key, value.clone()));
                }
            }
        }
        for (owner, namespace) in self.submitted.as_object().expect("Submitted namespaces") {
            for key in namespace
                .as_object()
                .expect("Submitted namespace fields")
                .keys()
            {
                if current[owner].get(key).is_none() {
                    changes.push(SettingsChange::reset(owner, key));
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        let request = SettingsRequest::batch(changes).tracked();
        let keys = request
            .changes
            .iter()
            .map(|change| {
                let (owner, key) = change.owner_key();
                (owner.to_owned(), key.to_owned())
            })
            .collect();
        self.pending
            .push_back((request.receipt.expect("Tracked request"), keys));
        self.outbox.push_back(request);
        self.submitted = current;
        self.request_apply();
    }

    pub fn take_requests(&mut self) -> Vec<SettingsRequest> {
        self.outbox.drain(..).collect()
    }

    pub(crate) fn application_defaults(&self) -> Value {
        self.defaults.clone()
    }

    pub(crate) fn request_ids(&self) -> Vec<SettingsRequestId> {
        self.pending.iter().map(|(id, _)| *id).collect()
    }

    pub(crate) fn synchronize(
        &mut self,
        snapshot: &SettingsSnapshot,
        results: Vec<(SettingsRequestId, Result<u64, String>)>,
    ) {
        if !results.is_empty() {
            self.error = None;
        }
        for (id, result) in results {
            self.pending.retain(|(pending, _)| *pending != id);
            if let Err(error) = result {
                self.error = Some(error);
            }
        }
        let mut current = self.settings.clone();
        for (owner, namespace) in snapshot
            .settings
            .as_object()
            .expect("Published settings namespaces")
        {
            for (key, value) in namespace.as_object().expect("Published settings namespace") {
                let pending = self.pending.iter().any(|(_, keys)| {
                    keys.iter().any(|(pending_owner, pending_key)| {
                        pending_owner == owner && pending_key == key
                    })
                });
                let unsaved = current[owner][key] != self.submitted[owner][key];
                if !pending && !unsaved {
                    current[owner][key] = value.clone();
                    self.submitted[owner][key] = value.clone();
                }
            }
        }
        let before = self.settings.clone();
        self.settings = current;
        self.needs_apply |= before != self.settings;
        if let Some(error) = &snapshot.error {
            self.error = Some(error.clone());
        }
        self.sync_effect_preset();
    }

    fn sync_effect_preset(&mut self) {
        self.effect_preset = EffectPreset::from_preferences(&self.settings["bed.effects"]);
    }

    pub fn keybindings(&self) -> bed_keybindings::Overrides {
        serde_json::from_value(self.settings["bed.workbench"]["keybindings"].clone())
            .expect("The keybinding draft is validated before it is installed")
    }

    pub fn set_keybindings(&mut self, overrides: bed_keybindings::Overrides) {
        self.settings["bed.workbench"]["keybindings"] =
            serde_json::to_value(overrides).expect("Keybindings serialize");
    }

    pub fn background_opacity(&self) -> f32 {
        self.number("bed.ui", "background_opacity", 1.0)
            .clamp(0.0, 1.0)
    }

    pub fn bool(&self, owner: &str, key: &str, default: bool) -> bool {
        self.settings[owner][key].as_bool().unwrap_or(default)
    }

    pub fn number(&self, owner: &str, key: &str, default: f32) -> f32 {
        self.settings[owner][key]
            .as_f64()
            .filter(|v| v.is_finite())
            .map_or(default, |v| v as f32)
    }

    pub fn font_size(&self) -> f32 {
        self.number("bed.ui", "fontSize", 20.0).clamp(4.0, 64.0)
    }

    pub fn autosave_enabled(&self) -> bool {
        self.bool("bed.documents", "autosave", true)
    }

    pub fn autosave_delay(&self) -> Duration {
        Duration::from_millis(
            self.number("bed.documents", "autosave_delay_ms", 1000.0)
                .clamp(100.0, 60_000.0) as u64,
        )
    }

    pub fn lsp_autocomplete_enabled(&self) -> bool {
        self.bool("bed.editor", "lsp_autocomplete", true)
    }

    pub fn lsp_completion_delay(&self) -> Duration {
        Duration::from_millis(
            self.number("bed.editor", "lsp_completion_delay_ms", 150.0)
                .clamp(0.0, 5_000.0) as u64,
        )
    }

    pub fn lsp_format_on_save_enabled(&self) -> bool {
        self.bool("bed.editor", "lsp_format_on_save", true)
    }

    pub fn shader_settings(&self) -> bed_plugin_effects::settings::ShaderSettings {
        bed_plugin_effects::settings::ShaderSettings::from_preferences(
            &self.settings["bed.effects"],
        )
    }

    pub fn set_effect_preset(&mut self, preset: EffectPreset) -> io::Result<()> {
        self.settings["bed.effects"]["effect_preset"] = json!(preset.name());
        self.request_save();
        self.effect_preset = preset;
        Ok(())
    }

    pub fn customize_shaders(&mut self) -> io::Result<()> {
        let current = self.shader_settings();
        self.settings["bed.effects"]["shader_toggle"] = json!(current.enabled);
        for (key, value) in [
            ("scanline_intensity", current.scanline_intensity),
            ("vignet_intensity", current.vignet_intensity),
            ("bloom_intensity", current.bloom_intensity),
            ("static_intensity", current.static_intensity),
            ("colorshift_intensity", current.colorshift_intensity),
            ("jitter_intensity", current.jitter_intensity),
            ("curvature_intensity", current.curvature_intensity),
            ("pixelation_intensity", current.pixelation_intensity),
            ("pixel_width", current.pixel_width),
            ("burnin_intensity", current.burnin_intensity),
            ("pulse_intensity", current.pulse_intensity),
        ] {
            self.settings["bed.effects"][key] = json!(value);
        }
        self.settings["bed.effects"]["effect_preset"] = json!(EffectPreset::Custom.name());
        self.effect_preset = EffectPreset::Custom;
        self.request_save();
        Ok(())
    }

    pub fn rainbow(&self) -> bool {
        self.bool("bed.editor", "rainbow", true)
    }

    pub fn request_apply(&mut self) {
        self.needs_apply = true;
    }

    /// Consume draft appearance changes; preference I/O belongs to core.
    pub fn refresh(&mut self) -> bool {
        if !self.needs_apply {
            return false;
        }
        self.needs_apply = false;
        self.sync_effect_preset();
        self.sidebar_visible = self.bool("bed.application", "sidebar_visible", true);
        self.terminal_visible = self.bool("bed.application", "terminal_visible", true);
        true
    }

    /// Pure projection; the UI host owns atlas handles and applies these values.
    pub fn appearance(
        &self,
        palette: &bed_ui::appearance::UiPalette,
    ) -> bed_ui::appearance::UiTheme {
        use bed_ui::appearance::{UiFontOptions, UiTheme};
        UiTheme {
            fonts: UiFontOptions {
                family: self.settings["bed.ui"]["font"]
                    .as_str()
                    .unwrap_or("Paper Mono")
                    .into(),
                size: self.font_size(),
            },
            palette: palette.clone(),
            opacity: self.background_opacity(),
            motion: bed_ui::appearance::UiMotion {
                enabled: self.bool("bed.ui", "ui_animations", true),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn toggle_sidebar(&mut self) -> io::Result<()> {
        self.sidebar_visible = !self.sidebar_visible;
        self.settings["bed.application"]["sidebar_visible"] = json!(self.sidebar_visible);
        self.request_save();
        Ok(())
    }

    pub fn toggle_terminal(&mut self) -> io::Result<()> {
        self.terminal_visible = !self.terminal_visible;
        self.settings["bed.application"]["terminal_visible"] = json!(self.terminal_visible);
        self.request_save();
        Ok(())
    }
}
