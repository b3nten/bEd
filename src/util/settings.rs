//! Desktop profile lifecycle and appearance. Embedded consumers provide their
//! configuration explicitly through the reusable session and view APIs.
use crate::util::{font::Font, keybinds::KeybindsManager};
use bed_session::editor::Editor;
use dear_imgui_rs::{
    Condition, Context, Key, MouseButton, PopupQueryFlags, StyleColor, StyleVar, TreeNodeFlags, Ui,
    WindowFlags, WindowHoveredFlags,
};
use serde_json::{Value, json};
use std::io;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

const PRIMARY_PROFILE: &str = "bed.json";
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
    pub show_settings_window: bool,
    pub request_lsp_dashboard: bool,
    pub request_config_file: Option<PathBuf>,
    pub is_embedded: bool,
    pub embedded_window_pos: [f32; 2],
    pub embedded_window_size: [f32; 2],
    pub embedded_window_collapsed: bool,
    pub config_dir: PathBuf,
    pub resources_root: PathBuf,
    pub settings_path: PathBuf,
    disk_time: Option<SystemTime>,
    needs_apply: bool,
    was_focused: bool,
    pub error: Option<String>,
}

impl Settings {
    pub fn lsp_presentation(&self) -> bed_ui::presentation::LspPresentationOptions {
        bed_ui::presentation::LspPresentationOptions {
            background_color: self.background_color(),
            embedded: self.is_embedded,
            symbol_key: self.keybinds.get_action_key("lsp_symbol_info"),
            definition_key: self.keybinds.get_action_key("lsp_find_def"),
            references_key: self.keybinds.get_action_key("lsp_find_ref"),
        }
    }

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
            show_settings_window: false,
            request_lsp_dashboard: false,
            request_config_file: None,
            is_embedded: false,
            embedded_window_pos: [200.0, 200.0],
            embedded_window_size: [900.0, 600.0],
            embedded_window_collapsed: false,
            config_dir,
            resources_root,
            settings_path: PathBuf::new(),
            disk_time: None,
            needs_apply: true,
            was_focused: false,
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
        #[cfg(target_os = "windows")]
        if let Some(home) = std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()) {
            return Ok(PathBuf::from(home).join("bed/config"));
        }
        #[cfg(target_os = "windows")]
        if let (Some(drive), Some(path)) =
            (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH"))
        {
            let mut home = drive;
            home.push(path);
            return Ok(PathBuf::from(home).join("bed/config"));
        }
        std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .map(|home| PathBuf::from(home).join("bed/config"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Home directory is not set (USERPROFILE/HOME)",
                )
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
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
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
            #[cfg(windows)]
            if primary.exists() {
                use std::os::windows::ffi::OsStrExt;
                use windows_sys::Win32::Storage::FileSystem::{
                    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
                };
                let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
                let to: Vec<u16> = primary.as_os_str().encode_wide().chain(Some(0)).collect();
                // SAFETY: Both buffers are nul-terminated and live for the call.
                if unsafe {
                    MoveFileExW(
                        from.as_ptr(),
                        to.as_ptr(),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
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
    pub fn shader_settings(&self) -> bed_effects::shader_manager::ShaderSettings {
        use bed_effects::shader_manager::ShaderSettings;
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
        super::workspace_state::write_atomic(
            &self.config_dir.join("effects.json"),
            &json!({"version":1,"preset":preset.name()}),
        )?;
        self.effect_preset = preset;
        Ok(())
    }
    fn customize_shaders(&mut self) -> io::Result<()> {
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
        color(&self.settings["themes"][theme]["text"], [1.0; 4])
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
            [text[0] * 0.6, text[1] * 0.6, text[2] * 0.6, text[3]],
        );
        style.set_color(StyleColor::TextSelectedBg, [1.0, 0.1, 0.7, 0.3]);
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
            for (slot, alpha) in [
                (StyleColor::Button, 0.12),
                (StyleColor::ButtonHovered, 0.18),
                (StyleColor::ButtonActive, 0.30),
            ] {
                style.set_color(slot, [text[0], text[1], text[2], alpha]);
            }
            style.set_scrollbar_size(30.0 * style.font_scale_dpi().max(1.0));
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
    fn persist_ui(&mut self) {
        if let Err(e) = self.save_settings() {
            self.error = Some(e.to_string());
        }
    }
    pub fn draw(&mut self, ui: &Ui, editor: &mut Editor, shaders_available: bool) {
        self.draw_with_icons(ui, editor, shaders_available, None);
    }
    pub fn draw_with_icons(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&super::icons::Icons>,
    ) {
        if !self.show_settings_window {
            return;
        }
        if self.is_embedded {
            let mut window_open = true;
            ui.window("Settings")
                .opened(&mut window_open)
                .position(self.embedded_window_pos, Condition::FirstUseEver)
                .size(self.embedded_window_size, Condition::FirstUseEver)
                .flags(WindowFlags::NO_COLLAPSE)
                .build(|| {
                    self.embedded_window_pos = ui.window_pos();
                    self.embedded_window_size = ui.window_size();
                    editor.view.block_input = true;
                    self.draw_settings_content(ui, editor, false, icons);
                });
            // The source title-bar X changes visibility without the explicit
            // save/unblock performed by closeSettingsWindow (Escape/background).
            if !window_open {
                self.show_settings_window = false;
            }
            return;
        }
        let display = ui.io().display_size();
        let compact = display[0] < 1100.0 || self.font_size() > 40.0;
        let size = [
            display[0] * if compact { 0.90 } else { 0.75 },
            display[1] * if compact { 0.80 } else { 0.85 },
        ];
        let fs = ui.current_font_size();
        let alpha = ui.push_style_var(StyleVar::Alpha(1.0));
        let rounding = ui.push_style_var(StyleVar::WindowRounding(fs * 0.5));
        let border = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
        let scrollbar = ui.push_style_var(StyleVar::ScrollbarSize(fs * 0.7));
        let scrollbar_round = ui.push_style_var(StyleVar::ScrollbarRounding(fs * 0.5));
        let padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.75; 2]));
        let bg = self.background_color();
        let window_bg = ui.push_style_color(
            StyleColor::WindowBg,
            [bg[0] * 0.8, bg[1] * 0.8, bg[2] * 0.8, 1.0],
        );
        let frame_bg = ui.push_style_color(
            StyleColor::FrameBg,
            [bg[0] * 0.5, bg[1] * 0.5, bg[2] * 0.5, 1.0],
        );
        let scrollbar_bg = ui.push_style_color(
            StyleColor::ScrollbarBg,
            [bg[0] * 0.5, bg[1] * 0.5, bg[2] * 0.5, 0.0],
        );
        let popup_bg = ui.push_style_color(
            StyleColor::PopupBg,
            [bg[0] * 0.5, bg[1] * 0.5, bg[2] * 0.5, 1.0],
        );
        let border_color = ui.push_style_color(StyleColor::Border, [0.3, 0.3, 0.3, 1.0]);
        let grab = ui.push_style_color(StyleColor::ScrollbarGrab, [0.3, 0.3, 0.3, 1.0]);
        let grab_hover =
            ui.push_style_color(StyleColor::ScrollbarGrabHovered, [0.4, 0.4, 0.4, 1.0]);
        let grab_active =
            ui.push_style_color(StyleColor::ScrollbarGrabActive, [0.5, 0.5, 0.5, 1.0]);
        let font = self.font.main.map(|font| ui.push_font(font));
        // ned passes the internal Modal flag to Begin directly. The safe binding
        // only accepts public flags, so preserve that source behavior here; the
        // bound Ui owns the live context and the guard balances End on unwind.
        let visible = ui.with_bound_context(|| unsafe {
            use dear_imgui_rs::sys;
            sys::igSetNextWindowPos(
                [(display[0] - size[0]) * 0.5, (display[1] - size[1]) * 0.5].into(),
                sys::ImGuiCond_Always,
                [0.0; 2].into(),
            );
            sys::igSetNextWindowSize(size.into(), sys::ImGuiCond_Always);
            sys::igBegin(
                c"Settings".as_ptr(),
                std::ptr::null_mut(),
                sys::ImGuiWindowFlags_NoTitleBar
                    | sys::ImGuiWindowFlags_NoResize
                    | sys::ImGuiWindowFlags_NoMove
                    | sys::ImGuiWindowFlags_NoScrollbar
                    | sys::ImGuiWindowFlags_Modal,
            )
        });
        let window = NativeSettingsWindowGuard(ui);
        if visible {
            editor.view.block_input = true;
            self.draw_settings_content(ui, editor, shaders_available, icons);
        }
        drop(window);
        drop(font);
        drop(grab_active);
        drop(grab_hover);
        drop(grab);
        drop(border_color);
        drop(popup_bg);
        drop(scrollbar_bg);
        drop(frame_bg);
        drop(window_bg);
        drop(padding);
        drop(scrollbar_round);
        drop(scrollbar);
        drop(border);
        drop(rounding);
        drop(alpha);
    }
    fn close_settings_window(&mut self, editor: &mut Editor) {
        self.show_settings_window = false;
        self.persist_ui();
        editor.view.block_input = false;
    }
    fn draw_settings_content(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&super::icons::Icons>,
    ) {
        if !self.is_embedded {
            let padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
            self.draw_window_header(ui, editor, icons);
            drop(padding);
        }
        let background = (!self.is_embedded
            && self.settings["backgroundColor"]
                .as_array()
                .is_some_and(|colors| colors.len() >= 3))
        .then(|| {
            let bg = self.background_color();
            ui.push_style_color(
                StyleColor::ChildBg,
                [bg[0] * 0.8, bg[1] * 0.8, bg[2] * 0.8, 1.0],
            )
        });
        let fs = ui.current_font_size();
        let padding = ui.push_style_var(StyleVar::WindowPadding([fs * 0.75, fs * 0.25]));
        let lsp_available = editor.lsp_client().is_some();
        ui.child_window("SettingsContent")
            .size([0.0, ui.content_region_avail()[1]])
            .flags(WindowFlags::ALWAYS_VERTICAL_SCROLLBAR)
            .build(ui, || {
                if let Some(error) = &self.error {
                    ui.text_colored([1.0, 0.4, 0.3, 1.0], error);
                }
                self.draw_profile_selector(ui);
                self.draw_main_settings(ui);
                if !self.is_embedded {
                    self.draw_mac_settings(ui);
                }
                self.draw_syntax_colors(ui);
                self.draw_toggle_settings(ui, editor);
                if shaders_available && !self.is_embedded {
                    self.draw_shader_settings(ui);
                }
                self.draw_keybinds_settings(ui, lsp_available);
            });
        drop(padding);
        drop(background);
        self.handle_window_input(ui, editor);
    }
    /// Controls only; Workbench owns this tab and its focus/close behavior.
    pub fn draw_tab(&mut self, ui: &Ui, editor: &mut Editor, icons: Option<&super::icons::Icons>) {
        if let Some(error) = &self.error {
            ui.text_colored([1.0, 0.4, 0.3, 1.0], error);
        }
        self.draw_profile_selector(ui);
        self.draw_main_settings(ui);
        self.draw_mac_settings(ui);
        self.draw_syntax_colors(ui);
        self.draw_toggle_settings(ui, editor);
        self.draw_shader_settings(ui);
        self.draw_keybinds_settings(ui, editor.lsp_client().is_some());
        let _ = icons;
    }
    fn draw_window_header(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        icons: Option<&super::icons::Icons>,
    ) {
        let focused =
            ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::ROOT_AND_CHILD_WINDOWS);
        let hovered = ui.is_window_hovered_with_flags(WindowHoveredFlags::ROOT_AND_CHILD_WINDOWS);
        if self.was_focused && !focused && self.show_settings_window && !hovered {
            self.close_settings_window(editor);
        }
        self.was_focused = focused;
        ui.group(|| {
            ui.text("Settings");
            let side = ui.current_font_size();
            let x = ui.content_region_avail()[0] - side - ui.clone_style().frame_padding()[0] * 2.0;
            ui.same_line_with_pos(if x > 0.0 {
                x
            } else {
                ui.cursor_pos()[0] + 100.0
            });
            let cursor = ui.cursor_pos();
            if ui.invisible_button("##close-settings", [side; 2]) {
                self.close_settings_window(editor);
            }
            let hovered = ui.is_item_hovered();
            ui.set_cursor_pos(cursor);
            if let Some(texture) = icons.and_then(|icons| icons.get("close")) {
                ui.image_config(texture, [side; 2])
                    .tint_color([1.0, 1.0, 1.0, if hovered { 0.6 } else { 1.0 }])
                    .build();
            } else {
                // Hosts can delay their asset upload; keep the same hit rectangle
                // and draw the close glyph until they supply the source SVG texture.
                let pos = ui.cursor_screen_pos();
                let pad = side * 0.2;
                let ink = [1.0, 1.0, 1.0, if hovered { 0.6 } else { 1.0 }];
                let draw = ui.get_window_draw_list();
                draw.add_line(
                    [pos[0] + pad, pos[1] + pad],
                    [pos[0] + side - pad, pos[1] + side - pad],
                    ink,
                )
                .thickness(1.5)
                .build();
                draw.add_line(
                    [pos[0] + side - pad, pos[1] + pad],
                    [pos[0] + pad, pos[1] + side - pad],
                    ink,
                )
                .thickness(1.5)
                .build();
                ui.dummy([side; 2]);
            }
        });
        ui.separator();
    }
    fn handle_window_input(&mut self, ui: &Ui, editor: &mut Editor) {
        if ui.is_key_pressed(Key::Escape) {
            self.close_settings_window(editor);
            return;
        }
        if !ui.is_mouse_clicked(MouseButton::Left)
            || ui.is_any_item_hovered()
            || ui.is_window_hovered_with_flags(
                WindowHoveredFlags::ANY_WINDOW | WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_POPUP,
            )
            || ui.is_popup_open_with_flags("", PopupQueryFlags::ANY_POPUP_ID)
        {
            return;
        }
        let mouse = ui.io().mouse_pos();
        let pos = ui.window_pos();
        let size = ui.window_size();
        if mouse[0] < pos[0]
            || mouse[0] > pos[0] + size[0]
            || mouse[1] < pos[1]
            || mouse[1] > pos[1] + size[1]
        {
            self.close_settings_window(editor);
        }
    }
    fn draw_profile_selector(&mut self, ui: &Ui) {
        ui.spacing();
        let current = self
            .settings_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| PRIMARY_PROFILE.into());
        let mut profiles = self.list_profiles();
        if !current.is_empty() && !profiles.contains(&current) {
            profiles.insert(0, current.clone());
        }
        if let Some(_combo) = ui.begin_combo("##ActiveSettingsFileCombo", &current) {
            for name in profiles {
                let selected = name == current;
                if ui.selectable_config(&name).selected(selected).build()
                    && !selected
                    && let Err(error) = self.switch_to_profile(&name)
                {
                    self.error = Some(error.to_string());
                }
                if selected {
                    ui.set_item_default_focus();
                }
            }
        }
        ui.same_line();
        ui.text("Profile");
        ui.spacing();
    }
    fn draw_main_settings(&mut self, ui: &Ui) {
        let current = self.settings["font"]
            .as_str()
            .unwrap_or("SourceCodePro-Regular")
            .to_owned();
        if let Some(_combo) = ui.begin_combo("Font", display_font_name(&current)) {
            for name in Font::available_fonts(&self.resources_root) {
                let selected = name == current;
                if ui
                    .selectable_config(display_font_name(name))
                    .selected(selected)
                    .build()
                {
                    self.settings["font"] = json!(name);
                    self.request_apply();
                    self.persist_ui();
                }
                if selected {
                    ui.set_item_default_focus();
                }
            }
        }
        ui.spacing();
        let mut size = self.font_size();
        if ui
            .slider_config("Font Size", 4.0, 64.0)
            .try_display_format("%.0f")
            .expect("constant valid format")
            .build(&mut size)
        {
            self.settings["fontSize"] = json!(size);
            self.request_apply();
        }
        if ui.is_item_deactivated_after_edit() {
            self.persist_ui();
        }
        ui.spacing();
        let mut bg = self.settings["backgroundColor"]
            .as_array()
            .filter(|colors| colors.len() == 4)
            .map(|_| self.background_color())
            .unwrap_or([0.058, 0.194, 0.158, 1.0]);
        if ui.color_edit4("Background Color", &mut bg) {
            self.settings["backgroundColor"] = json!(bg);
            self.request_apply();
            self.persist_ui();
        }
    }
    fn draw_mac_settings(&mut self, ui: &Ui) {
        #[cfg(target_os = "macos")]
        {
            ui.spacing();
            ui.text("macOS Settings");
            ui.separator();
            ui.spacing();
            let mut opacity = self.number("mac_background_opacity", 0.5);
            if ui
                .slider_config("Background Opacity", 0.0, 1.0)
                .try_display_format("%.2f")
                .expect("constant valid format")
                .build(&mut opacity)
            {
                self.settings["mac_background_opacity"] = json!(opacity);
                self.request_apply();
                self.persist_ui();
            }
            let mut blur = self.bool("mac_blur_enabled", true);
            if ui.checkbox("Enable Background Blur", &mut blur) {
                self.settings["mac_blur_enabled"] = json!(blur);
                self.request_apply();
                self.persist_ui();
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ui;
    }
    fn draw_syntax_colors(&mut self, ui: &Ui) {
        let theme = self.settings["theme"]
            .as_str()
            .unwrap_or("default")
            .to_owned();
        if !self.settings["themes"][&theme].is_object() {
            ui.text(format!("Theme '{theme}' not found."));
            return;
        }
        for (key, fallback) in [
            ("text", "text"),
            ("keyword", "text"),
            ("string", "text"),
            ("number", "text"),
            ("comment", "text"),
            ("function", "text"),
            ("type", "text"),
            ("variable", "text"),
            ("parameter", "variable"),
            ("property", "variable"),
            ("constant", "number"),
            ("operator", "text"),
            ("punctuation", "text"),
            ("special", "keyword"),
        ] {
            let colors = &mut self.settings["themes"][&theme];
            if colors[key].as_array().is_none_or(|rgba| rgba.len() != 4) {
                colors[key] = if colors[fallback]
                    .as_array()
                    .is_some_and(|rgba| rgba.len() == 4)
                {
                    colors[fallback].clone()
                } else {
                    json!([0.75, 0.75, 0.75, 1.0])
                };
            }
        }
        ui.spacing();
        if !ui.collapsing_header("Syntax Colors", TreeNodeFlags::empty()) {
            return;
        }
        ui.spacing();
        for (label, key) in [
            ("Text", "text"),
            ("Keywords", "keyword"),
            ("Strings", "string"),
            ("Numbers", "number"),
            ("Comments", "comment"),
            ("Functions", "function"),
            ("Types", "type"),
            ("Identifier", "variable"),
            ("Parameter", "parameter"),
            ("Property / field", "property"),
            ("Constant", "constant"),
            ("Operator", "operator"),
            ("Punctuation", "punctuation"),
            ("Special / builtin", "special"),
        ] {
            let mut rgba = color(
                &self.settings["themes"][&theme][key],
                [0.75, 0.75, 0.75, 1.0],
            );
            ui.text(label);
            ui.same_line_with_pos(200.0);
            if ui.color_edit4(format!("##{key}"), &mut rgba) {
                self.settings["themes"][&theme][key] = json!(rgba);
                self.request_apply();
            }
            if ui.is_item_deactivated_after_edit() {
                self.settings["themes"][&theme][key] = json!(rgba);
                self.request_apply();
                self.persist_ui();
            }
        }
    }
    fn draw_toggle_settings(&mut self, ui: &Ui, editor: &mut Editor) {
        ui.spacing();
        ui.text("Toggle Settings");
        ui.separator();
        ui.spacing();
        for (label, key, help, spaced) in [
            (
                "File Explorer",
                "sidebar_visible",
                "(Show/hide file explorer sidebar)",
                true,
            ),
            (
                "Terminal",
                "terminal_visible",
                "(Show/hide bottom terminal panel)",
                true,
            ),
            (
                "Rainbow Mode",
                "rainbow",
                "(Rainbow cursor & line numbers)",
                false,
            ),
            (
                "Minimap",
                "minimap",
                "(Code overview strip on the right)",
                false,
            ),
            (
                "TreeSitter Mode",
                "treesitter",
                "(Syntax Highlighting)",
                false,
            ),
            (
                "Git Changed Lines",
                "git_changed_lines",
                "(Highlight changed lines in git)",
                false,
            ),
        ] {
            let mut value = self.bool(key, true);
            if ui.checkbox(label, &mut value) {
                match key {
                    "sidebar_visible" => {
                        if let Err(error) = self.toggle_sidebar() {
                            self.error = Some(error.to_string());
                        }
                    }
                    "terminal_visible" => {
                        if let Err(error) = self.toggle_terminal() {
                            self.error = Some(error.to_string());
                        }
                    }
                    _ => {
                        self.settings[key] = json!(value);
                        if key == "treesitter" {
                            self.request_apply();
                        }
                        if key == "git_changed_lines" {
                            editor.set_git_changed_lines(value);
                        }
                        self.persist_ui();
                    }
                }
            }
            ui.same_line();
            ui.text_disabled(help);
            if spaced {
                ui.spacing();
            }
        }
    }
    fn draw_shader_settings(&mut self, ui: &Ui) {
        ui.spacing();
        let mut selected = match self.effect_preset {
            EffectPreset::Sharp => 0,
            EffectPreset::Off => 1,
            EffectPreset::Legacy => 2,
            EffectPreset::Custom => 3,
        };
        if ui.combo_simple_string(
            "Effect preset",
            &mut selected,
            &["Sharp CRT", "Off", "Legacy", "Custom"],
        ) {
            let preset = [
                EffectPreset::Sharp,
                EffectPreset::Off,
                EffectPreset::Legacy,
                EffectPreset::Custom,
            ][selected];
            let result = if preset == EffectPreset::Custom {
                self.customize_shaders()
            } else {
                self.set_effect_preset(preset)
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
        }
        if self.effect_preset != EffectPreset::Custom {
            if ui.button("Customize effects…")
                && let Err(error) = self.customize_shaders()
            {
                self.error = Some(error.to_string());
            }
            return;
        }
        ui.text("Shaders");
        ui.separator();
        ui.spacing();
        let mut enabled = self.bool("shader_toggle", true);
        if ui.checkbox("Enable Shader Effects", &mut enabled) {
            self.settings["shader_toggle"] = json!(enabled);
            self.persist_ui();
        }
        ui.same_line();
        ui.text_disabled("(CRT & visual effects)");
        for (label, key, min, max, format, default) in [
            ("Scanline", "scanline_intensity", 0.0, 1.0, "%.02f", 0.20),
            ("Vignette", "vignet_intensity", 0.0, 1.0, "%.02f", 0.25),
            ("Bloom", "bloom_intensity", 0.0, 1.0, "%.02f", 0.75),
            ("Static", "static_intensity", 0.0, 0.5, "%.03f", 0.208),
            (
                "RGB Shift",
                "colorshift_intensity",
                0.0,
                10.0,
                "%.02f",
                0.90,
            ),
            ("Curvature", "curvature_intensity", 0.0, 0.5, "%.02f", 0.0),
            ("Burn-in", "burnin_intensity", 0.0, 0.999, "%.03f", 0.0),
            ("Jitter", "jitter_intensity", 0.0, 10.0, "%.02f", 2.81),
            ("Pulse", "pulse_intensity", 0.0, 0.1, "%.03f", 0.0),
            ("FPS Target", "fps_target", 20.0, 1000.0, "%.0f", 120.0),
        ] {
            let mut value = self.number(key, default);
            if ui
                .slider_config(label, min, max)
                .flags(dear_imgui_rs::SliderFlags::ALWAYS_CLAMP)
                .try_display_format(format)
                .expect("constant valid format")
                .build(&mut value)
            {
                self.settings[key] = json!(value);
            }
            if ui.is_item_deactivated_after_edit() {
                self.persist_ui();
            }
            ui.spacing();
        }
    }
    fn draw_keybinds_settings(&mut self, ui: &Ui, lsp_available: bool) {
        ui.spacing();
        ui.separator();
        ui.spacing();
        let keybinds = self.config_dir.join("keybinds.json");
        let defaults = self.config_dir.join("default-keybinds.json");
        if ui.button("Open Keybinds File") {
            if keybinds.is_file() {
                self.request_config_file = Some(keybinds.clone());
                self.show_settings_window = false;
            } else {
                eprintln!("[Settings] Keybinds file not found: {}", keybinds.display());
            }
        }
        ui.same_line();
        ui.text_disabled("(Edit keyboard shortcuts)");
        if defaults.is_file() && !keybinds.exists() {
            ui.text_colored([1.0, 0.5, 0.0, 1.0], "Using default keybinds");
            if ui.button("Restore Default Keybinds")
                && let Err(error) =
                    fs::copy(defaults, keybinds).and_then(|_| self.keybinds.load_keybinds())
            {
                self.error = Some(error.to_string());
            }
            ui.same_line();
            ui.text_disabled("(Reset to default configuration)");
        }
        if lsp_available {
            ui.spacing();
            ui.separator();
            ui.spacing();
            if ui.button("LSP Dashboard") {
                self.request_lsp_dashboard = true;
                self.show_settings_window = false;
            }
            ui.same_line();
            ui.text_disabled("(View LSP server status)");
        }
    }
}

struct NativeSettingsWindowGuard<'ui>(&'ui Ui);
impl Drop for NativeSettingsWindowGuard<'_> {
    fn drop(&mut self) {
        self.0
            .with_bound_context(|| unsafe { dear_imgui_rs::sys::igEnd() });
    }
}

fn display_font_name(name: &str) -> String {
    if name == "System Default" || !name.contains('.') {
        return name.to_owned();
    }
    name.rsplit_once('.').unwrap().0.replace('-', " ")
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
        Settings::with_paths(temp.0.clone(), PathBuf::from(env!("CARGO_MANIFEST_DIR"))).unwrap()
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

    struct NativeSettingsWindow {
        pos: [f32; 2],
        size: [f32; 2],
        flags: i32,
        has_close: bool,
        title_height: f32,
        colors: Vec<u32>,
        close_icon: Option<NativeIcon>,
    }
    struct NativeIcon {
        min: [f32; 2],
        max: [f32; 2],
        colors: Vec<u32>,
    }
    fn settings_context() -> Context {
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }
    fn draw_settings(
        context: &mut Context,
        settings: &mut Settings,
        editor: &mut Editor,
    ) -> NativeSettingsWindow {
        draw_settings_with_icons(context, settings, editor, None)
    }
    fn draw_settings_with_icons(
        context: &mut Context,
        settings: &mut Settings,
        editor: &mut Editor,
        icons: Option<&crate::util::icons::Icons>,
    ) -> NativeSettingsWindow {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        context.prepare_frame(FramePrepareOptions::new([1800.0, 1200.0], 1.0 / 60.0));
        let ui = context.frame();
        let counts = ui.with_bound_context(|| unsafe {
            let ctx = &*sys::igGetCurrentContext();
            (
                ctx.ColorStack.Size,
                ctx.StyleVarStack.Size,
                ctx.CurrentWindowStack.Size,
            )
        });
        settings.draw_with_icons(ui, editor, true, icons);
        let window = ui.with_bound_context(|| unsafe {
            let ctx = &*sys::igGetCurrentContext();
            assert_eq!(
                counts,
                (
                    ctx.ColorStack.Size,
                    ctx.StyleVarStack.Size,
                    ctx.CurrentWindowStack.Size
                )
            );
            let window = sys::igFindWindowByName(c"Settings".as_ptr())
                .as_ref()
                .unwrap();
            let draw = &*window.DrawList;
            let vertices =
                std::slice::from_raw_parts(draw.VtxBuffer.Data, draw.VtxBuffer.Size as usize);
            let commands =
                std::slice::from_raw_parts(draw.CmdBuffer.Data, draw.CmdBuffer.Size as usize);
            let close_icon = commands
                .iter()
                .find(|command| command.TexRef._TexID == 99)
                .map(|command| {
                    let indices = std::slice::from_raw_parts(
                        draw.IdxBuffer.Data.add(command.IdxOffset as usize),
                        command.ElemCount as usize,
                    );
                    let mut min = [f32::INFINITY; 2];
                    let mut max = [f32::NEG_INFINITY; 2];
                    let mut colors = Vec::new();
                    for index in indices {
                        let vertex = &vertices[*index as usize + command.VtxOffset as usize];
                        min[0] = min[0].min(vertex.pos.x);
                        min[1] = min[1].min(vertex.pos.y);
                        max[0] = max[0].max(vertex.pos.x);
                        max[1] = max[1].max(vertex.pos.y);
                        colors.push(vertex.col);
                    }
                    NativeIcon { min, max, colors }
                });
            NativeSettingsWindow {
                pos: [window.Pos.x, window.Pos.y],
                size: [window.Size.x, window.Size.y],
                flags: window.Flags,
                has_close: window.HasCloseButton,
                title_height: window.TitleBarHeight,
                colors: vertices.iter().map(|vertex| vertex.col).collect(),
                close_icon,
            }
        });
        drop(context.render_legacy());
        window
    }
    #[test]
    fn native_embedded_settings_preserves_host_palette_and_remembers_moved_geometry() {
        use dear_imgui_rs::sys;
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.is_embedded = true;
        settings.show_settings_window = true;
        let mut context = settings_context();
        let window_bg = [0.25, 0.5, 0.75, 1.0];
        let frame_bg = [0.33, 0.44, 0.55, 0.66];
        let child_bg = [0.12, 0.23, 0.34, 0.45];
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, window_bg);
        context.style_mut().set_color(StyleColor::FrameBg, frame_bg);
        context.style_mut().set_color(StyleColor::ChildBg, child_bg);
        let mut editor = Editor::new();
        let first = draw_settings(&mut context, &mut settings, &mut editor);
        assert_eq!(first.pos, [200.0, 200.0]);
        assert_eq!(first.size, [900.0, 600.0]);
        assert_ne!(first.flags & sys::ImGuiWindowFlags_NoCollapse, 0);
        assert_eq!(
            first.flags
                & (sys::ImGuiWindowFlags_NoMove
                    | sys::ImGuiWindowFlags_NoResize
                    | sys::ImGuiWindowFlags_NoTitleBar),
            0
        );
        assert!(first.has_close && first.title_height > 0.0);
        let second = draw_settings(&mut context, &mut settings, &mut editor);
        let host_color = unsafe { sys::igColorConvertFloat4ToU32(window_bg.into()) };
        assert!(second.colors.contains(&host_color));
        for (slot, rgba) in [
            (StyleColor::WindowBg, window_bg),
            (StyleColor::FrameBg, frame_bg),
            (StyleColor::ChildBg, child_bg),
        ] {
            assert_eq!(context.style().color(slot), rgba);
        }
        // Genuine native title-bar dragging must survive the next Begin call.
        let title = [first.pos[0] + 80.0, first.pos[1] + first.title_height * 0.5];
        context.io_mut().add_mouse_pos_event(title);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings(&mut context, &mut settings, &mut editor);
        context
            .io_mut()
            .add_mouse_pos_event([title[0] + 80.0, title[1] + 55.0]);
        let moved = draw_settings(&mut context, &mut settings, &mut editor);
        assert_eq!(moved.pos, [280.0, 255.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings(&mut context, &mut settings, &mut editor);
        context.binding().with_bound_context(|| unsafe {
            sys::igSetWindowSize_Str(
                c"Settings".as_ptr(),
                [640.0, 420.0].into(),
                sys::ImGuiCond_Always,
            );
        });
        let resized = draw_settings(&mut context, &mut settings, &mut editor);
        assert_eq!(resized.pos, [280.0, 255.0]);
        assert_eq!(resized.size, [640.0, 420.0]);
        assert_eq!(settings.embedded_window_pos, resized.pos);
        assert_eq!(settings.embedded_window_size, resized.size);
        assert!(!settings.embedded_window_collapsed);
        settings.show_settings_window = false;
        settings.draw(context.frame(), &mut editor, false);
        drop(context.render_legacy());
        settings.show_settings_window = true;
        let reopened = draw_settings(&mut context, &mut settings, &mut editor);
        assert_eq!(reopened.pos, resized.pos);
        assert_eq!(reopened.size, resized.size);
    }

    #[test]
    fn native_embedded_settings_close_button_escape_and_background_input_match_source() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.is_embedded = true;
        settings.show_settings_window = true;
        let mut editor = Editor::new();
        let mut context = settings_context();
        draw_settings(&mut context, &mut settings, &mut editor);
        let window = draw_settings(&mut context, &mut settings, &mut editor);
        settings.settings["test_close"] = json!("unsaved before title close");
        let font_size = context.style().font_size_base();
        let padding = context.style().frame_padding();
        let close = [
            window.pos[0] + window.size[0] - padding[0] - font_size * 0.5,
            window.pos[1] + padding[1] + font_size * 0.5,
        ];
        context.io_mut().add_mouse_pos_event(close);
        draw_settings(&mut context, &mut settings, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings(&mut context, &mut settings, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings(&mut context, &mut settings, &mut editor);
        assert!(!settings.show_settings_window);
        assert!(editor.view.block_input);
        assert!(read_json(&settings.settings_path).unwrap()["test_close"].is_null());
        settings.show_settings_window = true;
        context.io_mut().add_mouse_pos_event([0.0, 0.0]);
        draw_settings(&mut context, &mut settings, &mut editor);
        context.io_mut().add_key_event(Key::Escape, true);
        draw_settings(&mut context, &mut settings, &mut editor);
        assert!(!settings.show_settings_window);
        assert!(!editor.view.block_input);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["test_close"],
            "unsaved before title close"
        );
        context.io_mut().add_key_event(Key::Escape, false);
        settings.show_settings_window = true;
        draw_settings(&mut context, &mut settings, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings(&mut context, &mut settings, &mut editor);
        assert!(!settings.show_settings_window);
        assert!(!editor.view.block_input);
    }

    #[test]
    fn native_standalone_settings_keeps_centered_fixed_window_geometry() {
        use dear_imgui_rs::sys;
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.show_settings_window = true;
        let mut context = settings_context();
        let mut editor = Editor::new();
        let window = draw_settings(&mut context, &mut settings, &mut editor);
        assert_eq!(window.size, [1350.0, 1020.0]);
        assert_eq!(window.pos, [225.0, 90.0]);
        assert_eq!(
            window.flags
                & (sys::ImGuiWindowFlags_NoMove
                    | sys::ImGuiWindowFlags_NoResize
                    | sys::ImGuiWindowFlags_NoTitleBar),
            sys::ImGuiWindowFlags_NoMove
                | sys::ImGuiWindowFlags_NoResize
                | sys::ImGuiWindowFlags_NoTitleBar
        );
        assert!(!window.has_close);
        assert_ne!(window.flags & sys::ImGuiWindowFlags_Modal, 0);
    }

    #[test]
    fn native_standalone_header_uses_uploaded_icon_and_saves_on_close() {
        use crate::util::icons::Icons;
        use dear_imgui_rs::{TextureId, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.show_settings_window = true;
        settings.settings["themes"] =
            json!({"old": {"text": [0.1, 0.2, 0.3, 1.0], "variable": [0.4, 0.5, 0.6, 1.0]}});
        settings.settings["theme"] = json!("old");
        let mut context = settings_context();
        let mut editor = Editor::new();
        let mut icons = Icons::default();
        icons.set_texture("close", TextureId::new(99));
        draw_settings_with_icons(&mut context, &mut settings, &mut editor, Some(&icons));
        let window =
            draw_settings_with_icons(&mut context, &mut settings, &mut editor, Some(&icons));
        let icon = window.close_icon.unwrap();
        let side = context.style().font_size_base();
        assert_eq!(
            [icon.max[0] - icon.min[0], icon.max[1] - icon.min[1]],
            [side; 2]
        );
        let colors = &settings.settings["themes"]["old"];
        // Upstream upgrades old eight-slot themes even with the picker collapsed.
        assert_eq!(colors["parameter"], colors["variable"]);
        assert_eq!(colors["property"], colors["variable"]);
        assert_eq!(colors["constant"], colors["number"]);
        assert_eq!(colors.as_object().unwrap().len(), 14);
        context.io_mut().add_mouse_pos_event([
            (icon.min[0] + icon.max[0]) * 0.5,
            (icon.min[1] + icon.max[1]) * 0.5,
        ]);
        let hovered =
            draw_settings_with_icons(&mut context, &mut settings, &mut editor, Some(&icons));
        let tint = unsafe { sys::igColorConvertFloat4ToU32([1.0, 1.0, 1.0, 0.6].into()) };
        assert!(
            hovered
                .close_icon
                .unwrap()
                .colors
                .iter()
                .all(|color| *color == tint)
        );
        settings.settings["test_close"] = json!("header persists");
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings_with_icons(&mut context, &mut settings, &mut editor, Some(&icons));
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings_with_icons(&mut context, &mut settings, &mut editor, Some(&icons));
        assert!(!settings.show_settings_window);
        assert!(!editor.view.block_input);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["test_close"],
            "header persists"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_mac_opacity_and_blur_inputs_persist_and_request_apply() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.settings["mac_background_opacity"] = json!(0.5);
        settings.settings["mac_blur_enabled"] = json!(true);
        settings.save_settings().unwrap();
        settings.needs_apply = false;
        let mut context = settings_context();
        let draw = |context: &mut Context, settings: &mut Settings| {
            context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut slider = [0.0; 2];
            let mut checkbox = [0.0; 2];
            let mut active = 0;
            let mut slider_id = 0;
            ui.window("mac-controls")
                .position([100.0, 100.0], Condition::Always)
                .size([600.0, 300.0], Condition::Always)
                .build(|| {
                    let width = ui.calc_item_width();
                    settings.draw_mac_settings(ui);
                    let (min, _) = ui.item_rect();
                    let height = ui.frame_height();
                    checkbox = [min[0] + height * 0.5, min[1] + height * 0.5];
                    slider = [
                        min[0] + width * 0.75,
                        min[1] - ui.clone_style().item_spacing()[1] - height * 0.5,
                    ];
                    ui.with_bound_context(|| unsafe {
                        slider_id = sys::igGetID_Str(c"Background Opacity".as_ptr());
                        active = (*sys::igGetCurrentContext()).ActiveId;
                    });
                });
            drop(context.render_legacy());
            (slider, checkbox, active, slider_id)
        };
        draw(&mut context, &mut settings);
        let (slider, checkbox, _, _) = draw(&mut context, &mut settings);
        context.io_mut().add_mouse_pos_event(slider);
        draw(&mut context, &mut settings);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let (_, _, active, slider_id) = draw(&mut context, &mut settings);
        assert_eq!(active, slider_id, "mouse hit the actual opacity slider");
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw(&mut context, &mut settings);
        let opacity = settings.number("mac_background_opacity", 0.0);
        assert!(opacity > 0.7 && opacity < 0.8);
        assert!(settings.needs_apply);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["mac_background_opacity"],
            settings.settings["mac_background_opacity"]
        );
        settings.needs_apply = false;
        context.io_mut().add_mouse_pos_event(checkbox);
        draw(&mut context, &mut settings);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut settings);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw(&mut context, &mut settings);
        assert!(!settings.bool("mac_blur_enabled", true));
        assert!(settings.needs_apply);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["mac_blur_enabled"],
            false
        );
    }
}

#[cfg(test)]
mod effect_preset_tests {
    use super::*;
    use crate::files::test_support::TempDir;
    #[test]
    fn sharp_is_clear_without_rewriting_custom_profile_values() {
        let dir = TempDir::new();
        let mut settings = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        settings.settings["bloom_intensity"] = json!(0.7);
        settings.settings["burnin_intensity"] = json!(0.93);
        settings.settings["font"] = json!("SourceCodePro-Regular");
        let original = settings.settings.clone();
        assert_eq!(
            settings.shader_settings(),
            bed_effects::shader_manager::ShaderSettings::subtle()
        );
        assert_eq!(settings.settings, original);
        settings.set_effect_preset(EffectPreset::Legacy).unwrap();
        assert_eq!(settings.shader_settings().bloom_intensity, 0.7);
        assert_eq!(settings.settings, original);
        settings.set_effect_preset(EffectPreset::Off).unwrap();
        assert!(!settings.shader_settings().enabled);
        let loaded = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        assert_eq!(loaded.effect_preset, EffectPreset::Off);
    }
    #[test]
    fn customizing_copies_visible_effects_preserving_theme_and_fonts() {
        let dir = TempDir::new();
        let mut settings = Settings::with_paths(
            dir.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
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
            bed_effects::shader_manager::ShaderSettings::subtle()
        );
    }
}
