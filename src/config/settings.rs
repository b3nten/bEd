//! bEd's preference draft and theme-file view. Core owns effective settings and persistence.
use crate::config::{
    keybinds::KeybindingsDraft,
    theme::{BUILTIN_THEMES, DEFAULT_THEME, Theme, ThemeDraft},
};
use bed_api::settings::{SettingsChange, SettingsRequest, SettingsRequestId, SettingsSnapshot};
use bed_ui::color::ensure_contrast;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub const SETTINGS_FILE: &str = "settings.json";

pub fn bundled_default_viewers() -> Value {
    let defaults: Value =
        serde_json::from_str(include_str!("../../resources/config/settings.json"))
            .expect("bundled settings must be valid JSON");
    defaults["default_viewers"].clone()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EffectPreset {
    Sharp,
    #[default]
    Off,
    Legacy,
    Custom,
}

impl EffectPreset {
    fn name(self) -> &'static str {
        match self {
            Self::Sharp => "sharp",
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Custom => "custom",
        }
    }
}

#[cfg(test)]
pub(crate) fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub struct Settings {
    pub settings: Value,
    pub effect_preset: EffectPreset,
    pub keybinds: KeybindingsDraft,
    pub sidebar_visible: bool,
    pub terminal_visible: bool,
    pub request_lsp_dashboard: bool,
    pub request_config_file: Option<PathBuf>,
    pub is_embedded: bool,
    pub config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub settings_path: PathBuf,
    pub theme: Theme,
    pub theme_draft: ThemeDraft,
    defaults: Value,
    submitted: Value,
    outbox: VecDeque<SettingsRequest>,
    pending: VecDeque<(SettingsRequestId, Vec<(String, String)>)>,
    theme_contents: Vec<u8>,
    needs_apply: bool,
    pub error: Option<String>,
    pub reload_error: Option<String>,
}

impl Settings {
    pub fn with_paths(config_dir: PathBuf, resources_root: PathBuf) -> io::Result<Self> {
        let mut settings: Value =
            serde_json::from_str(include_str!("../../resources/config/settings.json"))?;
        settings
            .as_object_mut()
            .expect("Bundled settings are an object")
            .remove("default_viewers_version");
        settings["cursor_smear"] = json!(true);
        settings["pulse_intensity"] = json!(0.05);
        let submitted = draft_values(&settings, &bed_keybindings::Overrides::default());
        let mut view = Self {
            settings,
            effect_preset: EffectPreset::Off,
            keybinds: KeybindingsDraft::default(),
            sidebar_visible: true,
            terminal_visible: true,
            request_lsp_dashboard: false,
            request_config_file: None,
            is_embedded: false,
            settings_path: config_dir.join(SETTINGS_FILE),
            config_dir,
            resources_root,
            theme: Theme::from_json(&serde_json::from_str(include_str!(
                "../../resources/themes/tokyo.json"
            ))?)?,
            theme_draft: ThemeDraft::default(),
            defaults: submitted.clone(),
            submitted,
            outbox: VecDeque::new(),
            pending: VecDeque::new(),
            theme_contents: Vec::new(),
            needs_apply: true,
            error: None,
            reload_error: None,
        };
        view.refresh_theme()?;
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
        let current = draft_values(&self.settings, &self.keybinds.overrides);
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
        let mut current = draft_values(&self.settings, &self.keybinds.overrides);
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
        self.settings = flatten_values(&current);
        let bindings = serde_json::from_value(current["bed.workbench"]["keybindings"].clone())
            .expect("Live and draft keybindings were validated");
        if self.keybinds.overrides != bindings {
            self.needs_apply = true;
        }
        self.keybinds.overrides = bindings;
        self.needs_apply |= before != self.settings;
        if let Some(error) = &snapshot.error {
            self.error = Some(error.clone());
        }
        self.sync_effect_preset();
        self.refresh_theme_file();
    }

    pub fn refresh_theme_file(&mut self) -> bool {
        self.reload_error = None;
        let changed = match self.refresh_theme() {
            Ok(changed) => changed,
            Err(error) => {
                self.reload_error = Some(error.to_string());
                false
            }
        };
        self.needs_apply |= changed;
        changed
    }

    fn sync_effect_preset(&mut self) {
        self.effect_preset = match self.settings["effect_preset"].as_str() {
            Some("sharp") => EffectPreset::Sharp,
            Some("legacy") => EffectPreset::Legacy,
            Some("custom") => EffectPreset::Custom,
            _ => EffectPreset::Off,
        };
    }

    pub fn theme_path(&self, selection: &str) -> io::Result<PathBuf> {
        if BUILTIN_THEMES.contains(&selection) {
            return Ok(self
                .resources_root
                .join("resources/themes")
                .join(format!("{selection}.json")));
        }
        let path = Path::new(selection);
        if selection.starts_with("themes/")
            && path.extension().is_some_and(|e| e == "json")
            && path
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Ok(self.config_dir.join(path));
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Unknown theme '{selection}'; choose a built-in ID or themes/<name>.json"),
        ))
    }

    fn refresh_theme(&mut self) -> io::Result<bool> {
        let selection = self.settings["theme"].as_str().unwrap_or(DEFAULT_THEME);
        let path = self.theme_path(selection)?;
        let bytes = fs::read(&path)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        if bytes == self.theme_contents {
            return Ok(false);
        }
        let theme = Theme::from_json(&serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })?)
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        self.theme = theme;
        self.theme_contents = bytes;
        Ok(true)
    }

    pub fn select_theme(&mut self, selection: &str) -> io::Result<()> {
        // Validate before changing the selection or writing any preferences.
        Theme::load(&self.theme_path(selection)?)?;
        self.settings["theme"] = json!(selection);
        self.request_save();
        self.refresh_theme()?;
        self.request_apply();
        Ok(())
    }

    pub fn list_themes(&self) -> Vec<(String, String)> {
        let mut themes: Vec<_> = BUILTIN_THEMES
            .iter()
            .filter_map(|id| {
                Theme::load(&self.theme_path(id).ok()?)
                    .ok()
                    .map(|t| (id.to_string(), t.name))
            })
            .collect();
        if let Ok(entries) = fs::read_dir(self.config_dir.join("themes")) {
            let mut custom: Vec<_> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if !path.is_file() || path.extension().is_none_or(|e| e != "json") {
                        return None;
                    }
                    let label = Theme::load(&path).map_or_else(
                        |_| format!("{} (invalid JSON)", entry.file_name().to_string_lossy()),
                        |t| t.name,
                    );
                    Some((
                        format!("themes/{}", entry.file_name().to_string_lossy()),
                        format!("{label} (custom)"),
                    ))
                })
                .collect();
            custom.sort();
            themes.extend(custom);
        }
        themes
    }

    pub fn load_theme(&self, selection: &str) -> io::Result<Theme> {
        Theme::load(&self.theme_path(selection)?)
    }

    /// Save a validated color-only theme. New drafts receive a unique filename;
    /// an existing custom selection is the only destination that can be updated.
    pub fn save_custom_theme(
        &mut self,
        destination: Option<&str>,
        value: &Value,
    ) -> io::Result<String> {
        let theme = Theme::from_json(value)?;
        let value = theme.to_json();
        if let Some(selection) = destination {
            if !selection.starts_with("themes/") || Path::new(selection).components().count() != 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only custom themes can be edited",
                ));
            }
            let path = self.theme_path(selection)?;
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Custom theme no longer exists; clone it to save a new copy",
                ));
            }
            crate::config::persistence::write_atomic(&path, &value)?;
            self.refresh_theme_file();
            return Ok(selection.to_owned());
        }
        let mut slug = String::new();
        for c in theme.name.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
            if slug.len() >= 80 {
                break;
            }
        }
        let slug = slug.trim_end_matches('-');
        let slug = if slug.is_empty() {
            "custom-theme"
        } else {
            slug
        };
        for index in 1.. {
            let selection = if index == 1 {
                format!("themes/{slug}.json")
            } else {
                format!("themes/{slug}-{index}.json")
            };
            let path = self.theme_path(&selection)?;
            match crate::config::persistence::write_atomic_new(&path, &value) {
                Ok(()) => return Ok(selection),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    pub fn background_opacity(&self) -> f32 {
        self.number("background_opacity", 1.0).clamp(0.0, 1.0)
    }

    /// Opaque theme color used for contrast and transient surfaces. Background
    /// opacity is applied to rendered backgrounds, never to foreground ink.
    pub fn background_color(&self) -> [f32; 4] {
        self.theme.background
    }

    /// Opaque base color beneath workspace panels. Rendering applies the same
    /// background opacity as panel surfaces.
    pub fn window_background_color(&self) -> [f32; 4] {
        self.theme.window_background
    }

    pub fn text_color(&self) -> [f32; 4] {
        ensure_contrast(self.theme.foreground, self.theme.background, 4.5)
    }

    pub fn accent_color(&self) -> [f32; 4] {
        ensure_contrast(self.theme.accent, self.theme.background, 3.0)
    }

    pub fn highlight_colors(&self) -> bed_highlight::tree_sitter::ThemeColors {
        self.theme.syntax.clone()
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.settings[key].as_bool().unwrap_or(default)
    }

    pub fn number(&self, key: &str, default: f32) -> f32 {
        self.settings[key]
            .as_f64()
            .filter(|v| v.is_finite())
            .map_or(default, |v| v as f32)
    }

    pub fn font_size(&self) -> f32 {
        self.number("fontSize", 20.0).clamp(4.0, 64.0)
    }

    pub fn autosave_enabled(&self) -> bool {
        self.bool("autosave", true)
    }

    pub fn autosave_delay(&self) -> Duration {
        Duration::from_millis(
            self.number("autosave_delay_ms", 1000.0)
                .clamp(100.0, 60_000.0) as u64,
        )
    }

    pub fn lsp_autocomplete_enabled(&self) -> bool {
        self.bool("lsp_autocomplete", true)
    }

    pub fn lsp_completion_delay(&self) -> Duration {
        Duration::from_millis(
            self.number("lsp_completion_delay_ms", 150.0)
                .clamp(0.0, 5_000.0) as u64,
        )
    }

    pub fn lsp_format_on_save_enabled(&self) -> bool {
        self.bool("lsp_format_on_save", true)
    }

    pub fn shader_settings(&self) -> bed_plugin_effects::settings::ShaderSettings {
        use bed_plugin_effects::settings::ShaderSettings;
        match self.effect_preset {
            EffectPreset::Sharp => ShaderSettings::subtle(),
            EffectPreset::Off => ShaderSettings {
                enabled: false,
                ..ShaderSettings::subtle()
            },
            EffectPreset::Legacy => {
                let mut legacy = ShaderSettings::from_json(&self.settings);
                legacy.enabled = true;
                legacy.pulse_intensity = self.number("pulse_intensity", 0.05);
                legacy
            }
            EffectPreset::Custom => ShaderSettings::from_json(&self.settings),
        }
    }

    pub fn set_effect_preset(&mut self, preset: EffectPreset) -> io::Result<()> {
        self.settings["effect_preset"] = json!(preset.name());
        self.request_save();
        self.effect_preset = preset;
        Ok(())
    }

    pub fn customize_shaders(&mut self) -> io::Result<()> {
        let current = self.shader_settings();
        self.settings["shader_toggle"] = json!(current.enabled);
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
            self.settings[key] = json!(value);
        }
        self.settings["effect_preset"] = json!(EffectPreset::Custom.name());
        self.effect_preset = EffectPreset::Custom;
        self.request_save();
        Ok(())
    }

    pub fn rainbow(&self) -> bool {
        self.bool("rainbow", true)
    }

    pub fn request_apply(&mut self) {
        self.needs_apply = true;
    }

    /// Consume draft appearance changes; preference I/O belongs to core.
    pub fn refresh(&mut self) -> bool {
        self.refresh_theme_file();
        if !self.needs_apply {
            return false;
        }
        self.needs_apply = false;
        self.sync_effect_preset();
        self.sidebar_visible = self.bool("sidebar_visible", true);
        self.terminal_visible = self.bool("terminal_visible", true);
        true
    }

    /// Pure projection; the UI host owns atlas handles and applies these values.
    pub fn appearance(&self) -> bed_ui::appearance::UiTheme {
        use bed_ui::appearance::{UiFontOptions, UiPalette, UiTheme};
        UiTheme {
            fonts: UiFontOptions {
                family: self.settings["font"]
                    .as_str()
                    .unwrap_or("Paper Mono")
                    .into(),
                size: self.font_size(),
            },
            palette: UiPalette {
                background: self.background_color(),
                window_background: self.window_background_color(),
                tab_bar: self.theme.tab_bar,
                surface: self.theme.surface,
                text: self.text_color(),
                accent: self.accent_color(),
                selection: self.theme.selection,
            },
            opacity: self.background_opacity(),
            motion: bed_ui::appearance::UiMotion {
                enabled: self.bool("ui_animations", true),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn toggle_sidebar(&mut self) -> io::Result<()> {
        self.sidebar_visible = !self.sidebar_visible;
        self.settings["sidebar_visible"] = json!(self.sidebar_visible);
        self.request_save();
        Ok(())
    }

    pub fn toggle_terminal(&mut self) -> io::Result<()> {
        self.terminal_visible = !self.terminal_visible;
        self.settings["terminal_visible"] = json!(self.terminal_visible);
        self.request_save();
        Ok(())
    }
}

pub(crate) fn draft_values(flat: &Value, keybindings: &bed_keybindings::Overrides) -> Value {
    let mut values = json!({});
    if let Some(plugins) = flat["plugins"].as_object() {
        for (owner, namespace) in plugins {
            values[owner] = namespace.clone();
        }
    }
    for (key, value) in flat.as_object().expect("Settings draft is an object") {
        if key == "plugins" {
            continue;
        }
        values[crate::config::settings_owner(key)][key] = value.clone();
    }
    values["bed.workbench"]["keybindings"] =
        serde_json::to_value(keybindings).expect("Keybindings serialize");
    values
}

fn flatten_values(values: &Value) -> Value {
    let mut flat = json!({"plugins":{}});
    for (owner, namespace) in values.as_object().expect("Canonical draft namespaces") {
        for (key, value) in namespace.as_object().expect("Canonical draft namespace") {
            if owner == "bed.workbench" && key == "keybindings" {
                continue;
            }
            if crate::config::settings_owner(key) == owner {
                flat[key] = value.clone();
            } else {
                flat["plugins"][owner][key] = value.clone();
            }
        }
    }
    flat
}
