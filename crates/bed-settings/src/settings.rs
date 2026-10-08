//! Desktop preferences and appearance. Embedded consumers provide their
//! configuration explicitly through the reusable session and view APIs.
use crate::{
    font::Font,
    keybinds::KeybindsManager,
    theme::{BUILTIN_THEMES, DEFAULT_THEME, Theme, ThemeDraft},
};
use bed_document_session::editor::Editor;
use bed_editing::util::color::{blend, ensure_contrast};
use dear_imgui_rs::{Context, StyleColor};
use serde_json::{Value, json};
use std::io;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub const SETTINGS_FILE: &str = "settings.json";

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
    pub theme: Theme,
    pub theme_draft: ThemeDraft,
    disk_contents: Option<Vec<u8>>,
    theme_contents: Vec<u8>,
    needs_apply: bool,
    pub error: Option<String>,
    pub reload_error: Option<String>,
}

impl Settings {
    pub fn new() -> io::Result<Self> {
        Self::with_paths(Self::get_user_config_dir()?, Self::get_app_resources_path())
    }
    pub fn with_paths(config_dir: PathBuf, resources_root: PathBuf) -> io::Result<Self> {
        let keybinds = KeybindsManager::new(&config_dir, &resources_root);
        let mut this = Self {
            settings: json!({}),
            effect_preset: EffectPreset::Off,
            keybinds,
            font: Font::default(),
            sidebar_visible: true,
            terminal_visible: true,
            request_lsp_dashboard: false,
            request_config_file: None,
            is_embedded: false,
            config_dir,
            resources_root: resources_root.clone(),
            settings_path: PathBuf::new(),
            theme: Theme::load(&resources_root.join("resources/themes/tokyo.json"))?,
            theme_draft: ThemeDraft::default(),
            disk_contents: None,
            theme_contents: Vec::new(),
            needs_apply: true,
            error: None,
            reload_error: None,
        };
        this.load_settings()?;
        this.keybinds.load_keybinds()?;
        Ok(this)
    }
    pub fn get_user_config_dir() -> io::Result<PathBuf> {
        std::env::var_os("HOME")
            .filter(|s| !s.is_empty())
            .map(|home| PathBuf::from(home).join(".config/bed"))
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
    fn read_preferences(bytes: &[u8]) -> io::Result<Value> {
        let value: Value = serde_json::from_slice(bytes)?;
        if !value.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "settings.json must contain an object",
            ));
        }
        Ok(value)
    }
    pub fn load_settings(&mut self) -> io::Result<()> {
        fs::create_dir_all(self.config_dir.join("themes"))?;
        self.settings_path = self.config_dir.join(SETTINGS_FILE);
        for name in [SETTINGS_FILE, "keybinds.json", "lsp.json"] {
            let source = self.resources_root.join("resources/config").join(name);
            let destination = self.config_dir.join(name);
            let bytes = fs::read(source)?;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
            {
                Ok(mut file) => file.write_all(&bytes)?,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        self.settings = Self::read_preferences(&fs::read(
            self.resources_root.join("resources/config/settings.json"),
        )?)?;
        self.check_settings_file();
        self.needs_apply = true;
        Ok(())
    }
    pub fn save_settings(&mut self) -> io::Result<()> {
        // A broken external edit must be repaired by its author, never silently
        // replaced by the UI's last valid in-memory preferences.
        match fs::read(&self.settings_path) {
            Ok(bytes) => {
                Self::read_preferences(&bytes)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        crate::persistence::write_atomic(&self.settings_path, &self.settings)?;
        self.disk_contents = Some(fs::read(&self.settings_path)?);
        Ok(())
    }
    pub fn check_settings_file(&mut self) -> bool {
        let mut changed = false;
        self.reload_error = None;
        match fs::read(&self.settings_path) {
            Ok(bytes) if self.disk_contents.as_ref() != Some(&bytes) => {
                match Self::read_preferences(&bytes) {
                    Ok(value) => {
                        self.settings = value;
                        self.disk_contents = Some(bytes);
                        self.reload_error = None;
                        self.sync_effect_preset();
                        changed = true;
                    }
                    Err(e) => {
                        self.reload_error = Some(format!(
                            "{}: {e}. Repair the JSON to resume saving.",
                            self.settings_path.display()
                        ))
                    }
                }
            }
            Err(e) => self.reload_error = Some(format!("{}: {e}", self.settings_path.display())),
            _ => {}
        }
        match self.refresh_theme() {
            Ok(reloaded) => changed |= reloaded,
            Err(e) => self.reload_error = Some(e.to_string()),
        }
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
        let previous = self.settings["theme"].clone();
        self.settings["theme"] = json!(selection);
        if let Err(e) = self.save_settings() {
            self.settings["theme"] = previous;
            return Err(e);
        }
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
            crate::persistence::write_atomic(&path, &value)?;
            self.check_settings_file();
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
            match crate::persistence::write_atomic_new(&path, &value) {
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
                legacy.enabled = true;
                legacy.pulse_intensity = self.number("pulse_intensity", 0.05);
                legacy
            }
            EffectPreset::Custom => ShaderSettings::from_json(&self.settings),
        }
    }
    pub fn set_effect_preset(&mut self, preset: EffectPreset) -> io::Result<()> {
        self.settings["effect_preset"] = json!(preset.name());
        self.save_settings()?;
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
        self.sync_effect_preset();
        if let Err(e) = self.refresh_theme() {
            self.reload_error = Some(e.to_string());
        }
        let name = self.settings["font"]
            .as_str()
            .unwrap_or("Paper Mono")
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
        style.set_color(StyleColor::TextSelectedBg, self.theme.selection);
        style.set_color(StyleColor::ScrollbarBg, [0.0; 4]);
        if !self.is_embedded {
            let bg = self.background_color();
            let opacity = self.background_opacity();
            let panel = [bg[0], bg[1], bg[2], opacity];
            let tab_bar = self.theme.tab_bar;
            let chrome = [tab_bar[0], tab_bar[1], tab_bar[2], opacity];
            style.set_color(StyleColor::WindowBg, panel);
            // Children share their parent's single background fill.
            style.set_color(StyleColor::ChildBg, [0.0; 4]);
            for slot in [
                StyleColor::TitleBg,
                StyleColor::TitleBgActive,
                StyleColor::TitleBgCollapsed,
                StyleColor::MenuBarBg,
            ] {
                style.set_color(slot, chrome);
            }
            style.set_color(StyleColor::DockingEmptyBg, panel);
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
            for (slot, color) in bed_ui::util::popup_style::control_colors(
                self.theme.surface,
                text,
                self.accent_color(),
            ) {
                style.set_color(slot, color);
            }
            style.set_color(StyleColor::TextSelectedBg, self.theme.selection);
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
            self.highlight_colors(),
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
        Settings::with_paths(
            temp.0.clone(),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap()
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
mod desktop_tests {
    use super::*;
    use crate::test_support::TempDir;
    fn root() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }
    fn settings(dir: &TempDir) -> Settings {
        Settings::with_paths(dir.path("config"), root()).unwrap()
    }
    #[test]
    fn fresh_preferences_ignore_legacy_and_seed_only_application_config() {
        let dir = TempDir::new();
        fs::create_dir_all(dir.path("config")).unwrap();
        fs::write(
            dir.path("config/bed.json"),
            br#"{"fontSize":99,"settings_file":"amber.json"}"#,
        )
        .unwrap();
        fs::write(dir.path("config/ned.json"), b"legacy unchanged").unwrap();
        let current = settings(&dir);
        assert_eq!(current.settings_path, dir.path("config/settings.json"));
        assert_eq!(current.settings["font"], "Paper Mono");
        assert_eq!(current.font_size(), 20.0);
        assert_eq!(current.background_opacity(), 1.0);
        assert!(!current.shader_settings().enabled);
        assert_eq!(current.list_themes().len(), BUILTIN_THEMES.len());
        assert!(current.settings["themes"].is_null());
        assert!(!dir.path("config/tokyo.json").exists());
        assert!(!dir.path("config/effects.json").exists());
        assert_eq!(
            fs::read(dir.path("config/ned.json")).unwrap(),
            b"legacy unchanged"
        );
    }
    #[test]
    fn every_theme_switch_preserves_preferences_and_survives_restart() {
        let dir = TempDir::new();
        let mut current = settings(&dir);
        current.settings["fontSize"] = json!(27);
        current.settings["background_opacity"] = json!(0.5);
        current.settings["autosave"] = json!(false);
        current.settings["plugins"] = json!({"fixture":{"enabled":true}});
        current.set_effect_preset(EffectPreset::Sharp).unwrap();
        let mut expected = current.settings.clone();
        for id in BUILTIN_THEMES {
            current.select_theme(id).unwrap();
            expected["theme"] = json!(id);
            assert_eq!(current.settings, expected);
            assert!(current.shader_settings().enabled);
            assert!(current.theme.terminal.iter().all(|color| color[3] == 1.0));
            assert_eq!(current.theme.tab_bar, current.theme.background, "{id}");
            assert_eq!(settings(&dir).settings, expected);
        }
        current.set_effect_preset(EffectPreset::Off).unwrap();
        assert!(!settings(&dir).shader_settings().enabled);
    }
    #[test]
    fn custom_color_reload_is_isolated_and_invalid_edits_are_recoverable() {
        let dir = TempDir::new();
        let mut current = settings(&dir);
        let path = dir.path("config/themes/test.json");
        let mut value = read_json(&root().join("resources/themes/tokyo.json")).unwrap();
        value["name"] = json!("Custom");
        value["ui"]["tab_bar"] = json!("#123456");
        value["fontSize"] = json!(99);
        value["shader_toggle"] = json!(true);
        write_json(&path, &value).unwrap();
        current.select_theme("themes/test.json").unwrap();
        assert_eq!(current.font_size(), 20.0);
        assert!(!current.shader_settings().enabled);
        assert_eq!(
            current.theme.tab_bar[..3],
            [
                0x12 as f32 / 255.0,
                0x34 as f32 / 255.0,
                0x56 as f32 / 255.0
            ]
        );
        value["syntax"]["keyword"] = json!("#123456");
        write_json(&path, &value).unwrap();
        assert!(current.check_settings_file());
        let valid = current.highlight_colors();
        assert_eq!(valid.slots[2][0], 0x12 as f32 / 255.0);
        fs::write(&path, b"{ bad").unwrap();
        assert!(!current.check_settings_file());
        assert_eq!(current.highlight_colors(), valid);
        assert!(current.reload_error.is_some());
        assert!(current.select_theme("themes/test.json").is_err());
        assert!(current.select_theme("themes/../settings.json").is_err());
        write_json(&path, &value).unwrap();
        current.check_settings_file();
        assert_eq!(current.theme.name, "Custom");
        assert!(current.reload_error.is_none());
    }
    #[test]
    fn broken_settings_are_never_overwritten_and_valid_edits_reload() {
        let dir = TempDir::new();
        let mut current = settings(&dir);
        let original = current.settings.clone();
        fs::write(&current.settings_path, b"{ incomplete").unwrap();
        assert!(!current.check_settings_file());
        assert_eq!(current.settings, original);
        assert!(current.save_settings().is_err());
        assert_eq!(fs::read(&current.settings_path).unwrap(), b"{ incomplete");
        assert_eq!(settings(&dir).font_size(), 20.0);
        let mut repaired = original;
        repaired["fontSize"] = json!(25);
        repaired["effect_preset"] = json!("sharp");
        write_json(&current.settings_path, &repaired).unwrap();
        assert!(current.check_settings_file());
        assert_eq!(current.font_size(), 25.0);
        assert_eq!(current.effect_preset, EffectPreset::Sharp);
        current.save_settings().unwrap();
        assert_eq!(settings(&dir).font_size(), 25.0);
    }
    #[test]
    fn opacity_changes_background_slots_without_fading_ink_or_controls() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let dir = TempDir::new();
        let mut current = settings(&dir);
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        let mut original_text = None;
        for opacity in [0.0, 0.5, 1.0] {
            current.settings["background_opacity"] = json!(opacity);
            current.request_apply();
            current.apply(&mut context, &mut Editor::new()).unwrap();
            let style = context.style();
            assert_eq!(style.color(StyleColor::WindowBg)[3], opacity);
            assert_eq!(style.color(StyleColor::TitleBg)[3], opacity);
            assert_eq!(
                style.color(StyleColor::TitleBg),
                style.color(StyleColor::WindowBg)
            );
            assert_eq!(
                style.color(StyleColor::TitleBgActive),
                style.color(StyleColor::WindowBg)
            );
            assert_eq!(style.color(StyleColor::ChildBg)[3], 0.0);
            for slot in [
                StyleColor::Text,
                StyleColor::PopupBg,
                StyleColor::FrameBg,
                StyleColor::Button,
                StyleColor::CheckMark,
            ] {
                assert_eq!(style.color(slot)[3], 1.0, "{slot:?}");
            }
            let text = style.color(StyleColor::Text);
            if let Some(previous) = original_text {
                assert_eq!(text, previous);
            }
            original_text = Some(text);
        }
    }
    #[test]
    fn custom_theme_saves_are_unique_color_only_and_reload_selected_edits() {
        let dir = TempDir::new();
        let mut current = settings(&dir);
        let original = current.settings.clone();
        let builtin = current.theme_path("tokyo").unwrap();
        let builtin_bytes = fs::read(&builtin).unwrap();
        let mut value = current.theme.to_json();
        value["name"] = json!("  My / Night!  ");
        value["fontSize"] = json!(40);
        value["shader_toggle"] = json!(true);
        let first = current.save_custom_theme(None, &value).unwrap();
        let second = current.save_custom_theme(None, &value).unwrap();
        assert_eq!(first, "themes/my-night.json");
        assert_eq!(second, "themes/my-night-2.json");
        assert_eq!(current.settings, original);
        let saved = read_json(&current.theme_path(&first).unwrap()).unwrap();
        assert!(saved["fontSize"].is_null() && saved["shader_toggle"].is_null());
        assert_eq!(saved["name"], "My / Night!");
        assert!(current.list_themes().iter().any(|theme| theme.0 == first));
        for invalid in [
            "tokyo",
            "themes/../settings.json",
            "themes/nested/theme.json",
        ] {
            assert!(current.save_custom_theme(Some(invalid), &value).is_err());
        }
        current.select_theme(&first).unwrap();
        value["ui"]["background"] = json!("#123456");
        current.save_custom_theme(Some(&first), &value).unwrap();
        assert_eq!(
            current.theme.background,
            current.load_theme(&first).unwrap().background
        );
        assert_eq!(current.theme.tab_bar, current.theme.background);
        assert_eq!(current.font_size(), 20.0);
        assert!(!current.shader_settings().enabled);
        assert_eq!(fs::read(builtin).unwrap(), builtin_bytes);
        assert_eq!(settings(&dir).theme.background, current.theme.background);
        assert!(
            fs::read_dir(dir.path("config/themes"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
        let before = fs::read(current.theme_path(&first).unwrap()).unwrap();
        value["syntax"]["keyword"] = json!("not a color");
        assert!(current.save_custom_theme(Some(&first), &value).is_err());
        assert_eq!(
            fs::read(current.theme_path(&first).unwrap()).unwrap(),
            before
        );
    }

    #[test]
    fn pasted_themes_validate_without_discarding_the_previous_draft() {
        let dir = TempDir::new();
        let current = settings(&dir);
        let mut draft = ThemeDraft::default();
        draft.load(current.theme.clone(), None);
        draft.json_input = "{ incomplete".into();
        assert!(draft.import_json().is_err());
        assert_eq!(draft.palette.as_ref().unwrap().name, current.theme.name);
        for id in BUILTIN_THEMES {
            let theme = current.load_theme(id).unwrap();
            let value = theme.to_json();
            draft.json_input = serde_json::to_string(&value).unwrap();
            draft.import_json().unwrap();
            assert_eq!(draft.palette.as_ref().unwrap().to_json(), value);
            assert_eq!(draft.palette.as_ref().unwrap().syntax, theme.syntax);
            assert_eq!(draft.palette.as_ref().unwrap().terminal, theme.terminal);
            assert!(draft.destination.is_none() && draft.dirty);
        }
    }

    #[test]
    fn customizing_effects_keeps_theme_font_and_off_state() {
        let dir = TempDir::new();
        let mut current = settings(&dir);
        let theme = current.settings["theme"].clone();
        let font = current.settings["font"].clone();
        current.customize_shaders().unwrap();
        assert_eq!(current.effect_preset, EffectPreset::Custom);
        assert!(!current.shader_settings().enabled);
        assert_eq!(current.settings["theme"], theme);
        assert_eq!(current.settings["font"], font);
        assert_eq!(settings(&dir).effect_preset, EffectPreset::Custom);
    }
}
