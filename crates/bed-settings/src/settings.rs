//! Desktop profile lifecycle and appearance. Embedded consumers provide their
//! configuration explicitly through the reusable session and view APIs.
use crate::{font::Font, keybinds::KeybindsManager};
use bed_document_session::editor::Editor;
use bed_editing::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{Context, StyleColor};
use serde_json::{Value, json};
use std::io;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

pub const PRIMARY_PROFILE: &str = "bed.json";
const LEGACY_PRIMARY_PROFILE: &str = "ned.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EffectPreset {
    #[default]
    Sharp,
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

/// nlohmann's stream extraction reads one JSON value and permits trailing text.
/// This matters for the comments after upstream's keybinds object.
pub fn read_json(path: &Path) -> io::Result<Value> {
    let bytes = fs::read(path)?;
    serde_json::Deserializer::from_slice(&bytes)
        .into_iter::<Value>()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Empty JSON file"))?
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
pub fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    write_json_contents(fs::File::create(path)?, value)
}
fn write_json_contents(mut file: impl Write, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_string_pretty(value)?;
    // Reformat to upstream's four-space indentation without another direct crate.
    for line in bytes.lines() {
        let spaces = line.bytes().take_while(|c| *c == b' ').count();
        writeln!(file, "{}{}", " ".repeat(spaces * 2), &line[spaces..])?;
    }
    Ok(())
}

pub struct Settings {
    pub settings: Value,
    pub effect_preset: EffectPreset,
    pub keybinds: KeybindsManager,
    pub font: Font,
    pub sidebar_visible: bool,
    pub terminal_visible: bool,
    pub request_lsp_dashboard: bool,
    pub request_config_file: Option<PathBuf>,
    pub is_embedded: bool,
    pub config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub settings_path: PathBuf,
    disk_time: Option<SystemTime>,
    needs_apply: bool,
    pub error: Option<String>,
}

impl Settings {
    pub fn new() -> io::Result<Self> {
        Self::with_paths(Self::get_user_config_dir()?, Self::get_app_resources_path())
    }
    pub fn with_paths(config_dir: PathBuf, resources_root: PathBuf) -> io::Result<Self> {
        let keybinds = KeybindsManager::new(&config_dir, &resources_root);
        let mut this = Self {
            settings: json!({}),
            effect_preset: EffectPreset::Sharp,
            keybinds,
            font: Font::default(),
            sidebar_visible: true,
            terminal_visible: true,
            request_lsp_dashboard: false,
            request_config_file: None,
            is_embedded: false,
            config_dir,
            resources_root,
            settings_path: PathBuf::new(),
            disk_time: None,
            needs_apply: true,
            error: None,
        };
        this.load_settings()?;
        if let Ok(effects) = read_json(&this.config_dir.join("effects.json")) {
            this.effect_preset = match effects["preset"].as_str() {
                Some("off") => EffectPreset::Off,
                Some("legacy") => EffectPreset::Legacy,
                Some("custom") => EffectPreset::Custom,
                _ => EffectPreset::Sharp,
            };
        }
        this.keybinds.load_keybinds()?;
        Ok(this)
    }
    pub fn get_user_config_dir() -> io::Result<PathBuf> {
        std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .map(|home| PathBuf::from(home).join("bed/config"))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Home directory is not set (HOME)")
            })
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
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        candidates
            .into_iter()
            .find(|path| {
                path.join("resources/fonts").is_dir() && path.join("resources/config").is_dir()
            })
            .map(|path| fs::canonicalize(&path).unwrap_or(path))
            .unwrap_or(cwd)
    }
    fn bundled_path(&self) -> PathBuf {
        self.resources_root
            .join("resources/config")
            .join(PRIMARY_PROFILE)
    }
    fn primary_path(&self) -> PathBuf {
        self.config_dir.join(PRIMARY_PROFILE)
    }
    fn selects_legacy_profile(&self, name: &str) -> bool {
        self.selects_profile(name, LEGACY_PRIMARY_PROFILE)
    }
    fn selects_profile(&self, name: &str, profile: &str) -> bool {
        name == profile
            || fs::canonicalize(self.config_dir.join(name))
                .ok()
                .zip(fs::canonicalize(self.config_dir.join(profile)).ok())
                .is_some_and(|(selected, expected)| selected == expected)
    }
    fn write_primary(&self, value: &Value) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let primary = self.primary_path();
        let temporary = self.config_dir.join(format!(
            ".bed-profile-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&temporary)?;
            write_json_contents(&file, value)?;
            if let Ok(metadata) = fs::metadata(&primary) {
                fs::set_permissions(&temporary, metadata.permissions())?;
            }
            file.sync_all()?;
            drop(file);
            // Replace the directory entry itself, so a legacy-targeting Bed
            // symlink or hard link never causes a write to the original file.
            fs::rename(&temporary, &primary)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    fn migrate_legacy_primary(&self) -> io::Result<()> {
        let primary = self.primary_path();
        if primary.try_exists()? {
            return Ok(());
        }
        let Ok(mut legacy) = read_json(&self.config_dir.join(LEGACY_PRIMARY_PROFILE)) else {
            return Ok(());
        };
        if !legacy.is_object() {
            return Ok(());
        }
        if legacy["settings_file"]
            .as_str()
            .is_none_or(|name| self.selects_legacy_profile(name))
        {
            legacy["settings_file"] = json!(PRIMARY_PROFILE);
        }
        // Never overwrite a Bed profile created by another running instance.
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(&primary) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(error),
        };
        let result = write_json_contents(file, &legacy);
        if result.is_err() {
            let _ = fs::remove_file(&primary);
        }
        result
    }
    fn legacy_profile(&self) -> io::Result<Value> {
        let mut profile = read_json(&self.config_dir.join(LEGACY_PRIMARY_PROFILE))?;
        if !profile.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Legacy settings are not an object",
            ));
        }
        profile["settings_file"] = json!(PRIMARY_PROFILE);
        Ok(profile)
    }
    fn touch_disk_time(&mut self) {
        self.disk_time = fs::metadata(&self.settings_path)
            .ok()
            .and_then(|m| m.modified().ok());
    }
    fn load_bundled(&mut self) -> io::Result<()> {
        self.settings_path = self.bundled_path();
        self.settings = read_json(&self.settings_path)?;
        if !self.settings.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bundled settings are not an object",
            ));
        }
        self.needs_apply = true;
        Ok(())
    }
    pub fn load_settings(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.config_dir)?;
        self.migrate_legacy_primary()?;
        for entry in fs::read_dir(self.resources_root.join("resources/config"))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let dest = self.config_dir.join(entry.file_name());
                if !dest.exists() {
                    fs::copy(entry.path(), dest)?;
                }
            }
        }
        let primary = self.primary_path();
        let Ok(mut pointer) = read_json(&primary) else {
            return self.load_bundled();
        };
        if !pointer.is_object() {
            return self.load_bundled();
        }
        if pointer["settings_file"].as_str().is_none() {
            pointer["settings_file"] = json!(PRIMARY_PROFILE);
            self.write_primary(&pointer)?;
        } else if self
            .selects_legacy_profile(pointer["settings_file"].as_str().expect("validated string"))
        {
            let Ok(profile) = self.legacy_profile() else {
                return self.load_bundled();
            };
            self.write_primary(&profile)?;
            pointer = profile;
        }
        self.settings_path = self
            .config_dir
            .join(pointer["settings_file"].as_str().expect("validated string"));
        let Ok(profile) = read_json(&self.settings_path) else {
            return self.load_bundled();
        };
        if !profile.is_object() {
            return self.load_bundled();
        }
        self.settings = profile;
        self.touch_disk_time();
        self.needs_apply = true;
        Ok(())
    }
    pub fn save_settings(&mut self) -> io::Result<()> {
        if self.settings_path == self.primary_path() {
            self.write_primary(&self.settings)?;
        } else {
            write_json(&self.settings_path, &self.settings)?;
        }
        self.touch_disk_time();
        Ok(())
    }
    pub fn check_settings_file(&mut self) -> bool {
        let modified = fs::metadata(&self.settings_path)
            .ok()
            .and_then(|m| m.modified().ok());
        if modified.is_none() || modified <= self.disk_time {
            return false;
        }
        if let Ok(settings) = read_json(&self.settings_path) {
            self.settings = settings;
            self.disk_time = modified;
            self.needs_apply = true;
            return true;
        }
        false
    }
    pub fn switch_to_profile(&mut self, name: &str) -> io::Result<()> {
        let legacy = self.selects_legacy_profile(name);
        let path = if legacy {
            self.primary_path()
        } else {
            self.config_dir.join(name)
        };
        let mut loaded = if legacy {
            self.legacy_profile()?
        } else {
            read_json(&path)?
        };
        let primary = self.primary_path();
        let mut pointer = if legacy {
            loaded.clone()
        } else {
            read_json(&primary)?
        };
        if !pointer.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Profile pointer is not an object",
            ));
        }
        let primary_selected = legacy || self.selects_profile(name, PRIMARY_PROFILE);
        pointer["settings_file"] = json!(if primary_selected {
            PRIMARY_PROFILE
        } else {
            name
        });
        self.write_primary(&pointer)?;
        if primary_selected {
            loaded = pointer;
        }
        self.settings = loaded;
        self.settings_path = if primary_selected { primary } else { path };
        self.touch_disk_time();
        self.needs_apply = true;
        Ok(())
    }
    pub fn list_profiles(&self) -> Vec<String> {
        let mut profiles = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.config_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_file()
                    && path.extension().is_some_and(|e| e == "json")
                    && ![
                        "keybinds.json",
                        "default-keybinds.json",
                        "lsp.json",
                        ".undo-redo-bed.json",
                        LEGACY_PRIMARY_PROFILE,
                    ]
                    .contains(&name.as_str())
                {
                    profiles.push(name);
                }
            }
        }
        profiles.sort();
        profiles
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
        self.number("fontSize", 20.0).max(1.0)
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
    pub fn shader_settings(&self) -> bed_effects_config::ShaderSettings {
        use bed_effects_config::ShaderSettings;
        match self.effect_preset {
            EffectPreset::Sharp => ShaderSettings::subtle(),
            EffectPreset::Off => ShaderSettings {
                enabled: false,
                ..ShaderSettings::subtle()
            },
            EffectPreset::Legacy => {
                let mut legacy = ShaderSettings::from_json(&self.settings);
                legacy.pulse_intensity = self.number("pulse_intensity", 0.05);
                legacy
            }
            EffectPreset::Custom => ShaderSettings::from_json(&self.settings),
        }
    }
    pub fn set_effect_preset(&mut self, preset: EffectPreset) -> io::Result<()> {
        crate::persistence::write_atomic(
            &self.config_dir.join("effects.json"),
            &json!({"version":1,"preset":preset.name()}),
        )?;
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
        self.save_settings()?;
        self.set_effect_preset(EffectPreset::Custom)
    }
    pub fn background_color(&self) -> [f32; 4] {
        color(
            &self.settings["backgroundColor"],
            [0.09061047, 0.09061049, 0.13687152, 1.0],
        )
    }
    pub fn text_color(&self) -> [f32; 4] {
        let theme = self.settings["theme"].as_str().unwrap_or("default");
        ensure_contrast(
            color(&self.settings["themes"][theme]["text"], [1.0; 4]),
            self.background_color(),
            4.5,
        )
    }
    pub fn accent_color(&self) -> [f32; 4] {
        let theme = self.settings["theme"].as_str().unwrap_or("default");
        ensure_contrast(
            color(
                &self.settings["themes"][theme]["function"],
                self.text_color(),
            ),
            self.background_color(),
            3.0,
        )
    }
    pub fn rainbow(&self) -> bool {
        self.bool("rainbow", true)
    }
    pub fn request_apply(&mut self) {
        self.needs_apply = true;
    }
    pub fn apply(&mut self, context: &mut Context, editor: &mut Editor) -> io::Result<bool> {
        if !self.needs_apply {
            return Ok(false);
        }
        self.needs_apply = false;
        if !self.settings.is_object() {
            self.load_bundled()?;
            self.needs_apply = false;
        }
        let name = self.settings["font"]
            .as_str()
            .unwrap_or("SourceCodePro-Regular")
            .to_owned();
        self.font.set_font(&name, self.font_size());
        // Embedding shares the host's atlas. Append our own fonts and leave the
        // host's default size intact; Workbench scopes its main-font selection.
        let host_font_size = context.style().font_size_base();
        let loaded = self
            .font
            .load(context, &self.resources_root, !self.is_embedded);
        if self.is_embedded {
            context.style_mut().set_font_size_base(host_font_size);
        }
        loaded.map_err(io::Error::other)?;
        let style = context.style_mut();
        let text = self.text_color();
        style.set_color(StyleColor::Text, text);
        style.set_color(
            StyleColor::TextDisabled,
            ensure_contrast(
                blend(text, self.background_color(), 0.60),
                self.background_color(),
                3.0,
            ),
        );
        style.set_color(
            StyleColor::TextSelectedBg,
            blend(self.accent_color(), self.background_color(), 0.20),
        );
        style.set_color(StyleColor::ScrollbarBg, [0.0; 4]);
        if !self.is_embedded {
            let bg = self.background_color();
            let opaque = [bg[0], bg[1], bg[2], 1.0];
            style.set_color(StyleColor::WindowBg, bg);
            for slot in [
                StyleColor::ChildBg,
                StyleColor::TitleBg,
                StyleColor::TitleBgActive,
                StyleColor::TitleBgCollapsed,
                StyleColor::MenuBarBg,
                StyleColor::DockingEmptyBg,
            ] {
                style.set_color(slot, opaque);
            }
            for slot in [
                StyleColor::Tab,
                StyleColor::TabSelected,
                StyleColor::TabDimmed,
                StyleColor::TabDimmedSelected,
                StyleColor::TabSelectedOverline,
                StyleColor::TabDimmedSelectedOverline,
                StyleColor::ScrollbarGrab,
                StyleColor::ScrollbarGrabHovered,
                StyleColor::ScrollbarGrabActive,
            ] {
                style.set_color(slot, [0.0; 4]);
            }
            style.set_color(StyleColor::TabHovered, [text[0], text[1], text[2], 0.12]);
            for (slot, color) in
                bed_ui::util::popup_style::control_colors(bg, text, self.accent_color())
            {
                style.set_color(slot, color);
            }
            let fs = self.font_size();
            style.set_frame_rounding(fs * 0.25);
            style.set_frame_border_size(1.0);
            style.set_frame_padding([fs * 0.55, fs * 0.30]);
            style.set_grab_rounding(fs * 0.2);
            style.set_grab_min_size(fs * 0.5);
            style.set_popup_rounding(bed_ui::util::popup_style::POPUP_ROUNDING);
            style.set_popup_border_size(bed_ui::util::popup_style::POPUP_BORDER_SIZE);
            style.set_window_rounding(fs * 0.4);
            style.set_scrollbar_rounding(fs * 0.3);
            style.set_scrollbar_size(fs * 0.55);
            style.set_tab_bar_border_size(0.0);
            style.set_disabled_alpha(0.75);
        }
        self.sidebar_visible = self.bool("sidebar_visible", true);
        self.terminal_visible = self.bool("terminal_visible", true);
        editor.highlight.enabled = self.bool("treesitter", true);
        editor.set_git_changed_lines(self.bool("git_changed_lines", true));
        editor.highlight.force_color_update(
            bed_highlight::tree_sitter::ThemeColors::from_settings(&self.settings),
            &editor.state,
            &mut editor.ops,
        );
        editor.refresh_highlighting();
        Ok(true)
    }
    pub fn toggle_sidebar(&mut self) -> io::Result<()> {
        self.sidebar_visible = !self.sidebar_visible;
        self.settings["sidebar_visible"] = json!(self.sidebar_visible);
        self.save_settings()
    }
    pub fn toggle_terminal(&mut self) -> io::Result<()> {
        self.terminal_visible = !self.terminal_visible;
        self.settings["terminal_visible"] = json!(self.terminal_visible);
        self.save_settings()
    }
}

fn color(value: &Value, default: [f32; 4]) -> [f32; 4] {
    let Some(array) = value.as_array().filter(|a| a.len() >= 3) else {
        return default;
    };
    let mut result = default;
    for (i, v) in array.iter().take(4).enumerate() {
        let Some(number) = v.as_f64().filter(|v| v.is_finite()) else {
            return default;
        };
        result[i] = number as f32;
    }
    if array.len() == 3 {
        result[3] = 1.0;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::Key;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-settings-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn settings(temp: &TempDir) -> Settings {
        Settings::with_paths(
            temp.0.clone(),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap()
    }
    #[test]
    fn autosave_defaults_limits_and_profile_changes_persist() {
        let temp = TempDir::new();
        let mut current = settings(&temp);
        assert!(current.autosave_enabled());
        assert_eq!(current.autosave_delay(), Duration::from_millis(1000));
        for (configured, expected) in [(-10, 100), (0, 100), (2500, 2500), (90_000, 60_000)] {
            current.settings["autosave_delay_ms"] = json!(configured);
            assert_eq!(current.autosave_delay(), Duration::from_millis(expected));
        }
        current.settings["autosave_delay_ms"] = json!("invalid");
        assert_eq!(current.autosave_delay(), Duration::from_millis(1000));
        current.settings["autosave"] = json!(false);
        current.settings["autosave_delay_ms"] = json!(2500);
        current.save_settings().unwrap();
        let restarted = settings(&temp);
        assert!(!restarted.autosave_enabled());
        assert_eq!(restarted.autosave_delay(), Duration::from_millis(2500));
    }

    #[test]
    fn seeds_missing_defaults_only_and_follows_primary_profile_pointer() {
        let temp = TempDir::new();
        let mut first = settings(&temp);
        assert_eq!(first.settings_path.file_name().unwrap(), "tokyo.json");
        assert_eq!(
            read_json(&temp.0.join("bed.json")).unwrap()["settings_file"],
            "tokyo.json"
        );
        assert!(!temp.0.join("ned.json").exists());
        assert_eq!(
            first.keybinds.get_action_key("toggle_file_finder"),
            Some(Key::P)
        );
        first.settings["fontSize"] = json!(31);
        first.save_settings().unwrap();
        fs::remove_file(temp.0.join("amber.json")).unwrap();
        let second = settings(&temp);
        assert_eq!(second.font_size(), 31.0);
        assert!(temp.0.join("amber.json").exists());
    }

    #[test]
    fn profile_switch_persists_pointer_and_preserves_source_formats() {
        let temp = TempDir::new();
        let mut first = settings(&temp);
        first.switch_to_profile("amber.json").unwrap();
        assert_eq!(
            read_json(&temp.0.join("bed.json")).unwrap()["settings_file"],
            "amber.json"
        );
        let second = settings(&temp);
        assert_eq!(second.settings_path.file_name().unwrap(), "amber.json");
        assert!(second.list_profiles().contains(&"amber.json".to_owned()));
        assert!(!second.list_profiles().contains(&"keybinds.json".to_owned()));
        assert!(!second.list_profiles().contains(&"lsp.json".to_owned()));
    }

    #[test]
    fn switching_back_to_bed_then_saving_keeps_bed_selected_after_restart() {
        for name in ["bed.json", "./bed.json"] {
            let temp = TempDir::new();
            let mut current = settings(&temp);
            current.switch_to_profile("amber.json").unwrap();
            current.switch_to_profile(name).unwrap();
            assert_eq!(current.settings["settings_file"], "bed.json");
            assert_eq!(current.settings_path, temp.0.join("bed.json"));
            current.settings["fontSize"] = json!(34);
            current.save_settings().unwrap();
            let restarted = settings(&temp);
            assert_eq!(restarted.settings_path, temp.0.join("bed.json"));
            assert_eq!(restarted.font_size(), 34.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn bed_primary_links_to_legacy_are_detached_without_modifying_legacy_source() {
        for symbolic in [false, true] {
            let temp = TempDir::new();
            let legacy = b"{\"settings_file\":\"bed.json\",\"fontSize\":39}\n";
            fs::write(temp.0.join("ned.json"), legacy).unwrap();
            if symbolic {
                std::os::unix::fs::symlink("ned.json", temp.0.join("bed.json")).unwrap();
            } else {
                fs::hard_link(temp.0.join("ned.json"), temp.0.join("bed.json")).unwrap();
            }
            let mut migrated = settings(&temp);
            assert_eq!(migrated.settings_path, temp.0.join("bed.json"));
            assert_eq!(migrated.font_size(), 39.0);
            migrated.settings["fontSize"] = json!(40);
            migrated.save_settings().unwrap();
            assert_eq!(settings(&temp).font_size(), 40.0);
            assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
            assert!(
                !fs::symlink_metadata(temp.0.join("bed.json"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn absent_pointer_uses_primary_and_bad_active_profile_uses_bundled_default() {
        let temp = TempDir::new();
        let mut first = settings(&temp);
        write_json(&temp.0.join("bed.json"), &json!({"fontSize":24})).unwrap();
        first.load_settings().unwrap();
        assert_eq!(first.font_size(), 24.0);
        assert_eq!(
            read_json(&temp.0.join("bed.json")).unwrap()["settings_file"],
            "bed.json"
        );
        fs::write(
            temp.0.join("bed.json"),
            b"{\"settings_file\":\"missing.json\"}",
        )
        .unwrap();
        first.load_settings().unwrap();
        assert_eq!(first.settings_path, first.bundled_path());
    }

    #[test]
    fn legacy_default_profiles_migrate_without_changing_the_original_file() {
        for pointer in [None, Some("ned.json"), Some("./ned.json")] {
            let temp = TempDir::new();
            let mut legacy = json!({"fontSize":27, "custom":{"retained":[1,2,3]}});
            if let Some(pointer) = pointer {
                legacy["settings_file"] = json!(pointer);
            }
            let original = format!("{}\n// retain legacy comments\n", legacy);
            fs::write(temp.0.join("ned.json"), original.as_bytes()).unwrap();
            let mut migrated = settings(&temp);
            assert_eq!(migrated.settings_path, temp.0.join("bed.json"));
            assert_eq!(migrated.font_size(), 27.0);
            assert_eq!(migrated.settings["custom"]["retained"], json!([1, 2, 3]));
            assert_eq!(migrated.settings["settings_file"], "bed.json");
            migrated.settings["fontSize"] = json!(32);
            migrated.save_settings().unwrap();
            assert_eq!(settings(&temp).font_size(), 32.0);
            assert_eq!(
                fs::read(temp.0.join("ned.json")).unwrap(),
                original.as_bytes()
            );
            assert!(!migrated.list_profiles().contains(&"ned.json".to_owned()));
        }
    }

    #[test]
    fn legacy_custom_profile_selection_and_settings_survive_migration_and_reload() {
        let temp = TempDir::new();
        let legacy = b"{\"settings_file\":\"my-theme.json\",\"fontSize\":29,\"custom\":true}\n";
        let custom = b"{\"fontSize\":35,\"theme\":\"my-own-theme\",\"unknown\":{\"keep\":true}}\n";
        fs::write(temp.0.join("ned.json"), legacy).unwrap();
        fs::write(temp.0.join("my-theme.json"), custom).unwrap();
        let first = settings(&temp);
        assert_eq!(first.settings_path, temp.0.join("my-theme.json"));
        assert_eq!(first.font_size(), 35.0);
        assert_eq!(first.settings["unknown"]["keep"], true);
        assert_eq!(
            read_json(&temp.0.join("bed.json")).unwrap(),
            serde_json::from_slice::<Value>(legacy).unwrap()
        );
        assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
        assert_eq!(fs::read(temp.0.join("my-theme.json")).unwrap(), custom);
        // Once imported, the Bed pointer owns future selection changes.
        write_json(
            &temp.0.join("ned.json"),
            &json!({"settings_file":"tokyo.json"}),
        )
        .unwrap();
        let mut second = settings(&temp);
        assert_eq!(second.settings_path, temp.0.join("my-theme.json"));
        second.switch_to_profile("amber.json").unwrap();
        assert_eq!(settings(&temp).settings_path, temp.0.join("amber.json"));
        assert_eq!(
            read_json(&temp.0.join("ned.json")).unwrap()["settings_file"],
            "tokyo.json"
        );
    }

    #[test]
    fn existing_bed_profile_takes_priority_over_legacy_settings() {
        let temp = TempDir::new();
        let bed = b"{\"settings_file\":\"bed.json\",\"fontSize\":26}\n";
        let legacy = b"{\"settings_file\":\"ned.json\",\"fontSize\":41}\n";
        fs::write(temp.0.join("bed.json"), bed).unwrap();
        fs::write(temp.0.join("ned.json"), legacy).unwrap();
        assert_eq!(settings(&temp).font_size(), 26.0);
        assert_eq!(fs::read(temp.0.join("bed.json")).unwrap(), bed);
        assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
    }

    #[test]
    fn bed_pointer_selecting_legacy_profile_imports_it_before_future_saves() {
        let temp = TempDir::new();
        let legacy = b"{\"settings_file\":\"tokyo.json\",\"fontSize\":37,\"custom\":true}\n";
        fs::write(temp.0.join("ned.json"), legacy).unwrap();
        write_json(
            &temp.0.join("bed.json"),
            &json!({"settings_file":"./ned.json"}),
        )
        .unwrap();
        let mut migrated = settings(&temp);
        assert_eq!(migrated.settings_path, temp.0.join("bed.json"));
        assert_eq!(migrated.font_size(), 37.0);
        migrated.settings["fontSize"] = json!(38);
        migrated.save_settings().unwrap();
        migrated.switch_to_profile("ned.json").unwrap();
        assert_eq!(migrated.settings_path, temp.0.join("bed.json"));
        assert_eq!(migrated.font_size(), 37.0);
        assert_eq!(migrated.settings["settings_file"], "bed.json");
        assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
    }

    #[test]
    fn invalid_legacy_files_seed_bed_defaults_without_overwriting_legacy_data() {
        for legacy in [b"{ invalid".as_slice(), b"[]"] {
            let temp = TempDir::new();
            fs::write(temp.0.join("ned.json"), legacy).unwrap();
            assert_eq!(settings(&temp).settings_path, temp.0.join("tokyo.json"));
            assert!(temp.0.join("bed.json").is_file());
            assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
        }
    }

    #[test]
    fn missing_or_bad_legacy_selected_profile_keeps_pointer_and_uses_bed_fallback() {
        for bad_profile in [None, Some(b"[]".as_slice()), Some(b"{ invalid".as_slice())] {
            let temp = TempDir::new();
            let legacy = b"{\"settings_file\":\"my-theme.json\",\"fontSize\":23}\n";
            fs::write(temp.0.join("ned.json"), legacy).unwrap();
            if let Some(bytes) = bad_profile {
                fs::write(temp.0.join("my-theme.json"), bytes).unwrap();
            }
            let migrated = settings(&temp);
            assert_eq!(migrated.settings_path, migrated.bundled_path());
            assert_eq!(migrated.settings_path.file_name().unwrap(), "bed.json");
            assert_eq!(
                read_json(&temp.0.join("bed.json")).unwrap()["settings_file"],
                "my-theme.json"
            );
            assert_eq!(fs::read(temp.0.join("ned.json")).unwrap(), legacy);
        }
    }

    #[test]
    fn profile_picker_preserves_user_dot_profiles_and_excludes_bed_history() {
        let temp = TempDir::new();
        let current = settings(&temp);
        write_json(&temp.0.join(".my-theme.json"), &json!({"fontSize":28})).unwrap();
        write_json(&temp.0.join(".undo-redo-bed.json"), &json!({})).unwrap();
        assert!(
            current
                .list_profiles()
                .contains(&".my-theme.json".to_owned())
        );
        assert!(
            !current
                .list_profiles()
                .contains(&".undo-redo-bed.json".to_owned())
        );
    }

    #[test]
    fn json_stream_accepts_trailing_comments_but_rejects_comments_inside_object() {
        let temp = TempDir::new();
        let path = temp.0.join("json");
        fs::write(&path, b"{\"toggle\":\"p\"}\n// help text").unwrap();
        assert_eq!(read_json(&path).unwrap()["toggle"], "p");
        fs::write(&path, b"{ // invalid\n \"toggle\":\"p\"}").unwrap();
        assert!(read_json(&path).is_err());
    }

    #[test]
    fn malformed_reload_retains_last_valid_profile_and_keybinds() {
        let temp = TempDir::new();
        let mut first = settings(&temp);
        let previous = first.settings.clone();
        first.disk_time = Some(SystemTime::UNIX_EPOCH);
        fs::write(&first.settings_path, b"{ invalid").unwrap();
        assert!(!first.check_settings_file());
        assert_eq!(first.settings, previous);
        fs::write(
            temp.0.join("keybinds.json"),
            b"{\"toggle_file_finder\":\"F2\"}",
        )
        .unwrap();
        assert!(first.keybinds.check_keybinds_file().unwrap());
        assert_eq!(
            first.keybinds.get_action_key("toggle_file_finder"),
            Some(Key::F2)
        );
    }

    #[test]
    fn embedded_apply_appends_fonts_and_preserves_host_size_and_backgrounds() {
        use dear_imgui_rs::{Condition, FontSource, FramePrepareOptions};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.is_embedded = true;
        settings.settings["fontSize"] = json!(21.0);
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        context.style_mut().set_font_size_base(27.0);
        let window_bg = [0.11, 0.22, 0.33, 0.88];
        let child_bg = [0.44, 0.55, 0.66, 0.77];
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, window_bg);
        context.style_mut().set_color(StyleColor::ChildBg, child_bg);
        let mut editor = Editor::new();
        assert!(settings.apply(&mut context, &mut editor).unwrap());
        let first = settings.font.main.unwrap();
        assert_eq!(first.reference_size(), Some(21.0));
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        assert_eq!(context.style().color(StyleColor::WindowBg), window_bg);
        assert_eq!(context.style().color(StyleColor::ChildBg), child_bg);
        let fonts_before = unsafe { (*context.font_atlas().raw()).Fonts.Size };
        settings.settings["font"] = json!("JetBrainsMonoNL-Regular");
        settings.settings["fontSize"] = json!(31.0);
        settings.request_apply();
        assert!(settings.apply(&mut context, &mut editor).unwrap());
        assert_ne!(settings.font.main.unwrap(), first);
        assert_eq!(settings.font.main.unwrap().reference_size(), Some(31.0));
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        assert_eq!(context.style().color(StyleColor::WindowBg), window_bg);
        assert_eq!(context.style().color(StyleColor::ChildBg), child_bg);
        assert!(unsafe { (*context.font_atlas().raw()).Fonts.Size } > fonts_before);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let font = ui.push_font(host);
        assert_eq!(ui.current_font_size(), 13.0);
        ui.window("preserved host")
            .position([10.0, 10.0], Condition::Always)
            .size([400.0, 200.0], Condition::Always)
            .build(|| ui.text("host font remains valid after reload"));
        drop(font);
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
    }

    #[test]
    fn rejected_embedded_font_loader_preserves_host_sources_and_size() {
        use dear_imgui_rs::{FontLoader, FontSource};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.is_embedded = true;
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .set_font_loader(FontLoader::stb_truetype())
            .unwrap();
        let host = context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(13.0)]);
        context.style_mut().set_font_size_base(27.0);
        assert!(settings.apply(&mut context, &mut Editor::new()).is_err());
        assert_eq!(host.reference_size(), Some(13.0));
        assert_eq!(context.style().font_size_base(), 27.0);
        let atlas = context.font_atlas().raw();
        unsafe {
            assert_eq!((*atlas).Fonts.Size, 1);
            assert_eq!((*atlas).Sources.Size, 1);
        }
    }
}

#[cfg(test)]
mod effect_preset_tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn sharp_is_clear_without_rewriting_custom_profile_values() {
        let dir = TempDir::new();
        let mut settings = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
        settings.settings["bloom_intensity"] = json!(0.7);
        settings.settings["burnin_intensity"] = json!(0.93);
        settings.settings["font"] = json!("SourceCodePro-Regular");
        let original = settings.settings.clone();
        assert_eq!(
            settings.shader_settings(),
            bed_effects_config::ShaderSettings::subtle()
        );
        assert_eq!(settings.settings, original);
        settings.set_effect_preset(EffectPreset::Legacy).unwrap();
        assert_eq!(settings.shader_settings().bloom_intensity, 0.7);
        assert_eq!(settings.settings, original);
        settings.set_effect_preset(EffectPreset::Off).unwrap();
        assert!(!settings.shader_settings().enabled);
        let loaded = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
        assert_eq!(loaded.effect_preset, EffectPreset::Off);
    }
    #[test]
    fn customizing_copies_visible_effects_preserving_theme_and_fonts() {
        let dir = TempDir::new();
        let mut settings = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
        let font = settings.settings["font"].clone();
        let themes = settings.settings["themes"].clone();
        settings.settings["pixelation_intensity"] = json!(0.6);
        settings.settings["pixel_width"] = json!(17.0);
        settings.customize_shaders().unwrap();
        assert_eq!(settings.effect_preset, EffectPreset::Custom);
        assert_eq!(settings.settings["font"], font);
        assert_eq!(settings.settings["themes"], themes);
        assert_eq!(
            settings.shader_settings(),
            bed_effects_config::ShaderSettings::subtle()
        );
    }
}
