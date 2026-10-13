//! Settings controls and the legacy floating settings window.
use bed_config::theme::ThemeDraft;
use bed_config::{EffectPreset, Settings};
use bed_document_session::editor::Editor;
use bed_ui::color::{blend, ensure_contrast};
use bed_ui::icons::Icons;
use bed_ui::util::popup_style::tooltip_text;
use dear_imgui_rs::{
    Condition, Key, MouseButton, PopupQueryFlags, StyleColor, StyleVar, Ui, WindowFlags,
    WindowHoveredFlags,
};
use serde_json::{Value, json};
use std::ops::{Deref, DerefMut};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum SettingsCategory {
    #[default]
    General,
    Bedtime,
    Performance,
    ThemeEditor,
    Effects,
    Keybindings,
    Extensions,
}

impl SettingsCategory {
    const ALL: [(Self, &'static str); 7] = [
        (Self::General, "General"),
        (Self::Bedtime, "Bedtime"),
        (Self::Performance, "Performance"),
        (Self::ThemeEditor, "Theme Editor"),
        (Self::Effects, "Effects"),
        (Self::Keybindings, "Keybindings"),
        (Self::Extensions, "Extensions"),
    ];
}

/// Presentation state for a floating settings window, separate from saved configuration.
pub struct SettingsWindowState {
    pub show_settings_window: bool,
    pub embedded_window_pos: [f32; 2],
    pub embedded_window_size: [f32; 2],
    pub embedded_window_collapsed: bool,
    pub category: SettingsCategory,
    /// Runtime presentation supplied by the host, never persisted.
    pub main_font: Option<dear_imgui_rs::FontId>,
    pub font_warning: Option<String>,
    theme_editor: ThemeDraft,
    was_focused: bool,
    pub(crate) viewers: Vec<bed_editor_api::FileViewer>,
    new_viewer_extension: String,
    pub(crate) shortcut_actions: Vec<bed_keybindings::Action>,
    shortcut_filter: String,
    pub(crate) shortcut_groups: std::collections::BTreeMap<String, String>,
    pub(crate) shortcut_recording: Option<(String, Option<usize>)>,
    shortcut_recording_frame: usize,
}

impl Default for SettingsWindowState {
    fn default() -> Self {
        Self {
            show_settings_window: false,
            embedded_window_pos: [200.0, 200.0],
            embedded_window_size: [900.0, 600.0],
            embedded_window_collapsed: false,
            category: SettingsCategory::General,
            main_font: None,
            font_warning: None,
            theme_editor: ThemeDraft::default(),
            was_focused: false,
            viewers: Vec::new(),
            new_viewer_extension: String::new(),
            shortcut_actions: Vec::new(),
            shortcut_filter: String::new(),
            shortcut_groups: Default::default(),
            shortcut_recording: None,
            shortcut_recording_frame: 0,
        }
    }
}

struct SettingsView<'a> {
    service: &'a mut Settings,
    window: &'a mut SettingsWindowState,
}

impl<'a> SettingsView<'a> {
    fn new(service: &'a mut Settings, window: &'a mut SettingsWindowState) -> Self {
        Self { service, window }
    }
}

impl Deref for SettingsView<'_> {
    type Target = Settings;

    fn deref(&self) -> &Settings {
        self.service
    }
}

impl DerefMut for SettingsView<'_> {
    fn deref_mut(&mut self) -> &mut Settings {
        self.service
    }
}

/// Concrete settings UI extension; the service crate has no dependency on its panel.
pub trait SettingsUi {
    fn draw(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
    );

    fn draw_with_icons(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
    );

    fn draw_with_icons_and_extensions(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    );

    fn draw_tab_with_state(
        &mut self,
        ui: &Ui,
        state: &mut SettingsWindowState,
        editor: &mut Editor,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    );

    fn draw_tab(&mut self, ui: &Ui, editor: &mut Editor, icons: Option<&Icons>);

    fn draw_tab_with_extensions(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    );
}

impl SettingsUi for Settings {
    fn draw(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
    ) {
        SettingsView::new(self, window).draw(ui, editor, shaders_available);
    }

    fn draw_with_icons(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
    ) {
        SettingsView::new(self, window).draw_with_icons(ui, editor, shaders_available, icons);
    }

    fn draw_with_icons_and_extensions(
        &mut self,
        ui: &Ui,
        window: &mut SettingsWindowState,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        SettingsView::new(self, window).draw_with_icons_and_extensions(
            ui,
            editor,
            shaders_available,
            icons,
            extensions,
        );
    }

    fn draw_tab_with_state(
        &mut self,
        ui: &Ui,
        state: &mut SettingsWindowState,
        editor: &mut Editor,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        SettingsView::new(self, state).draw_tab_with_extensions(ui, editor, icons, extensions);
    }

    fn draw_tab(&mut self, ui: &Ui, editor: &mut Editor, icons: Option<&Icons>) {
        self.draw_tab_with_extensions(ui, editor, icons, &mut |_, _| false);
    }

    fn draw_tab_with_extensions(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        let mut state = SettingsWindowState {
            theme_editor: std::mem::take(&mut self.theme_draft),
            ..Default::default()
        };
        let id = ui.get_id("bed-settings-category").raw();
        let selected = ui.with_bound_context(|| unsafe {
            dear_imgui_rs::sys::ImGuiStorage_GetInt(dear_imgui_rs::sys::igGetStateStorage(), id, 0)
        });
        state.category = SettingsCategory::ALL
            .get(selected as usize)
            .map_or(SettingsCategory::General, |entry| entry.0);
        SettingsView::new(self, &mut state).draw_tab_with_extensions(ui, editor, icons, extensions);
        self.theme_draft = std::mem::take(&mut state.theme_editor);
        ui.with_bound_context(|| unsafe {
            dear_imgui_rs::sys::ImGuiStorage_SetInt(
                dear_imgui_rs::sys::igGetStateStorage(),
                id,
                state.category as i32,
            );
        });
    }
}

impl SettingsView<'_> {
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
        icons: Option<&Icons>,
    ) {
        self.draw_with_icons_and_extensions(ui, editor, shaders_available, icons, &mut |_, _| {
            false
        });
    }

    pub fn draw_with_icons_and_extensions(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        if !self.window.show_settings_window {
            return;
        }
        if self.is_embedded {
            let mut window_open = true;
            ui.window("Settings")
                .opened(&mut window_open)
                .position(self.window.embedded_window_pos, Condition::FirstUseEver)
                .size(self.window.embedded_window_size, Condition::FirstUseEver)
                .flags(WindowFlags::NO_COLLAPSE)
                .build(|| {
                    self.window.embedded_window_pos = ui.window_pos();
                    self.window.embedded_window_size = ui.window_size();
                    editor.view.block_input = true;
                    self.draw_settings_content(ui, editor, false, icons, extensions);
                });
            // The source title-bar X changes visibility without the explicit
            // save/unblock performed by closeSettingsWindow (Escape/background).
            if !window_open {
                self.window.show_settings_window = false;
            }
            return;
        }
        let display = ui.io().display_size();
        let compact = display[0] < 1100.0 || ui.current_font_size() > 40.0;
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
        let surface = blend(self.text_color(), [bg[0], bg[1], bg[2], 1.0], 0.025);
        let window_bg = ui.push_style_color(StyleColor::WindowBg, surface);
        let frame_bg = ui.push_style_color(
            StyleColor::FrameBg,
            blend(self.text_color(), surface, 0.045),
        );
        let scrollbar_bg = ui.push_style_color(StyleColor::ScrollbarBg, [0.0; 4]);
        let popup_bg = ui.push_style_color(StyleColor::PopupBg, surface);
        let border_color =
            ui.push_style_color(StyleColor::Border, blend(self.text_color(), surface, 0.30));
        let grab = ui.push_style_color(StyleColor::ScrollbarGrab, [0.0; 4]);
        let grab_hover = ui.push_style_color(
            StyleColor::ScrollbarGrabHovered,
            blend(self.text_color(), surface, 0.50),
        );
        let grab_active = ui.push_style_color(
            StyleColor::ScrollbarGrabActive,
            blend(self.text_color(), surface, 0.65),
        );
        let font = self.window.main_font.map(|font| ui.push_font(font));
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
            self.draw_settings_content(ui, editor, shaders_available, icons, extensions);
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
        self.window.show_settings_window = false;
        self.persist_ui();
        editor.view.block_input = false;
    }

    fn draw_settings_content(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        shaders_available: bool,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        if !self.is_embedded {
            let padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
            self.draw_window_header(ui, editor, icons);
            drop(padding);
        }
        self.draw_categories(ui, editor, shaders_available, extensions);
        self.handle_window_input(ui, editor);
    }

    /// The docked panel owns this state, independently of other settings views.
    pub fn draw_tab_with_extensions(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        _: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        self.draw_categories(ui, editor, !self.is_embedded, extensions);
    }

    fn draw_categories(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        shaders_available: bool,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        if let Some(error) = &self.reload_error {
            ui.text_wrapped(error);
        }
        if let Some(error) = &self.error {
            ui.text_wrapped(error);
        }
        if let Some(warning) = &self.window.font_warning {
            ui.text_wrapped(warning);
        }
        let width = (ui.current_font_size() * 9.0).min(ui.content_region_avail()[0] * 0.35);
        // Parent panel IDs distinguish settings instances; a separate child ID
        // per category lets ImGui retain each category's scroll position.
        ui.child_window("SettingsCategories")
            .size([width, 0.0])
            .build(ui, || {
                for (category, label) in SettingsCategory::ALL {
                    if ui
                        .selectable_config(label)
                        .selected(self.window.category == category)
                        .build()
                    {
                        self.window.category = category;
                    }
                }
            });
        ui.same_line();
        let category = self.window.category;
        ui.child_window(format!("SettingsContent{category:?}"))
            .size([0.0, 0.0])
            .build(ui, || match category {
                SettingsCategory::General => {
                    ui.text("General");
                    ui.separator();
                    ui.text_disabled("New windows without a workspace open the Startup layout.");
                    ui.text_disabled("Window → Set Current Layout saves the current arrangement.");
                    if ui.button("Open Settings JSON") {
                        self.request_config_file = Some(self.settings_path.clone());
                    }
                    ui.dummy([0.0, ui.current_font_size()]);
                    ui.text("Appearance");
                    ui.separator();
                    self.draw_theme_selector(ui);
                    self.draw_main_settings(ui);
                    if !self.is_embedded {
                        self.draw_mac_settings(ui);
                    }
                    self.draw_boolean(ui, "UI Animations", "ui_animations", true);
                    self.draw_boolean(ui, "Show tab bar add button", "tab_bar_add_button", false);
                    self.draw_boolean(ui, "Rainbow cursor and line numbers", "rainbow", true);
                    ui.dummy([0.0, ui.current_font_size()]);
                    ui.text("Editor");
                    ui.separator();
                    self.draw_autosave_settings(ui);
                    self.draw_boolean(ui, "Minimap", "minimap", true);
                    self.draw_boolean(ui, "Word Wrap", "word_wrap", true);
                    self.draw_boolean(ui, "Syntax highlighting", "treesitter", true);
                    self.draw_boolean(ui, "Git changed lines", "git_changed_lines", true);
                    self.draw_lsp_settings(ui);
                    if editor.lsp_client().is_some() && ui.button("LSP Dashboard") {
                        self.request_lsp_dashboard = true;
                    }
                    ui.dummy([0.0, ui.current_font_size()]);
                    self.draw_default_viewers(ui);
                }
                SettingsCategory::Bedtime => {
                    ui.text("Bedtime");
                    ui.separator();
                    self.draw_boolean(ui, "bEdtime when idle", "bedtime_enabled", true);
                    let mut delay = self.number("bedtime_delay_seconds", 60.0);
                    if ui.slider("bEdtime delay (seconds)", 10.0, 1800.0, &mut delay) {
                        self.settings["bedtime_delay_seconds"] = json!(delay);
                    }
                    if ui.is_item_deactivated_after_edit() {
                        self.persist_ui();
                    }
                    ui.text_disabled("Move the mouse or press a key to wake the editor.");
                }
                SettingsCategory::Performance => {
                    ui.text("Performance");
                    ui.separator();
                    self.draw_boolean(ui, "Limit frame rate", "fps_toggle", true);
                    for (label, key, default) in [
                        ("Frame rate", "fps_target", 57.0),
                        ("Unfocused frame rate", "fps_target_unfocused", 30.0),
                    ] {
                        let mut value = self.number(key, default);
                        if ui.slider(label, 10.0, 240.0, &mut value) {
                            self.settings[key] = json!(value);
                        }
                        if ui.is_item_deactivated_after_edit() {
                            self.persist_ui();
                        }
                    }
                }
                SettingsCategory::ThemeEditor => {
                    crate::theme_editor::draw(ui, &mut self.window.theme_editor, self.service);
                }
                SettingsCategory::Effects => {
                    ui.text("Cursor");
                    ui.separator();
                    self.draw_boolean(ui, "Cursor Smear", "cursor_smear", true);
                    ui.dummy([0.0, ui.current_font_size()]);
                    if shaders_available {
                        self.draw_shader_settings(ui);
                    } else {
                        ui.text_disabled("Effects are provided by the desktop host.");
                    }
                }
                SettingsCategory::Keybindings => self.draw_keybinds_settings(ui),
                SettingsCategory::Extensions => {
                    if extensions(ui, &mut self.settings) {
                        self.persist_ui();
                    }
                }
            });
    }

    fn draw_default_viewers(&mut self, ui: &Ui) {
        ui.text("Default viewers");
        ui.separator();
        ui.text_wrapped("Text files open in the code editor. Binary files use the first matching viewer, or Hex Editor. These associations override that choice for new opens.");
        if !self.settings["default_viewers"].is_object() {
            self.settings["default_viewers"] = json!({});
        }
        let mut extensions: Vec<String> = self.settings["default_viewers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        extensions.sort();
        let mut changed = false;
        for extension in extensions {
            let _id = ui.push_id(&extension);
            let selected = self.settings["default_viewers"][&extension]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let label = self
                .window
                .viewers
                .iter()
                .find(|v| v.id == selected || v.aliases.contains(&selected.as_str()))
                .map(|v| v.label.to_owned())
                .unwrap_or_else(|| format!("Unavailable: {selected}"));
            ui.text(format!(".{extension}"));
            ui.same_line();
            if let Some(_combo) = ui.begin_combo("##viewer", &label) {
                for viewer in &self.window.viewers {
                    if (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                        && ui
                            .selectable_config(viewer.label)
                            .selected(viewer.id == selected)
                            .build()
                    {
                        self.service.settings["default_viewers"][&extension] = json!(viewer.id);
                        changed = true;
                    }
                }
            }
            ui.same_line();
            if ui.button("Remove") {
                self.settings["default_viewers"]
                    .as_object_mut()
                    .unwrap()
                    .remove(&extension);
                changed = true;
            }
        }
        ui.input_text("Extension", &mut self.window.new_viewer_extension)
            .build();
        let extension = self
            .window
            .new_viewer_extension
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let valid = !extension.is_empty()
            && extension
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
        if let Some(_combo) = ui.begin_combo("Add association", "Choose viewer…") {
            for viewer in &self.window.viewers {
                if valid
                    && (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                    && ui.selectable(viewer.label)
                {
                    self.service.settings["default_viewers"][&extension] = json!(viewer.id);
                    self.window.new_viewer_extension.clear();
                    changed = true;
                }
            }
        }
        if ui.button("Reset to defaults") {
            self.settings["default_viewers"] = bed_config::settings::bundled_default_viewers();
            changed = true;
        }
        if changed {
            self.persist_ui();
        }
    }

    fn draw_boolean(&mut self, ui: &Ui, label: &str, key: &str, default: bool) {
        let mut value = self.bool(key, default);
        if ui.checkbox(label, &mut value) {
            self.settings[key] = json!(value);
            self.request_apply();
            self.persist_ui();
        }
    }

    fn draw_window_header(&mut self, ui: &Ui, editor: &mut Editor, icons: Option<&Icons>) {
        let focused =
            ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::ROOT_AND_CHILD_WINDOWS);
        let hovered = ui.is_window_hovered_with_flags(WindowHoveredFlags::ROOT_AND_CHILD_WINDOWS);
        if self.window.was_focused && !focused && self.window.show_settings_window && !hovered {
            self.close_settings_window(editor);
        }
        self.window.was_focused = focused;
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
            let mut ink = ui.style_color(StyleColor::Text);
            ink[3] *= if hovered { 0.6 } else { 1.0 };
            let ink = ensure_contrast(ink, ui.style_color(StyleColor::WindowBg), 3.0);
            ui.set_cursor_pos(cursor);
            if let Some(texture) = icons.and_then(|icons| icons.get("close")) {
                ui.image_config(texture, [side; 2]).tint_color(ink).build();
            } else {
                // Hosts can delay their asset upload; keep the same hit rectangle
                // and draw the close glyph until they supply the source SVG texture.
                let pos = ui.cursor_screen_pos();
                let pad = side * 0.2;
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

    fn draw_theme_selector(&mut self, ui: &Ui) {
        let current = self.settings["theme"]
            .as_str()
            .unwrap_or("tokyo")
            .to_owned();
        if let Some(_combo) = ui.begin_combo("Theme", self.theme.name.clone()) {
            for (id, name) in self.list_themes() {
                let _id = ui.push_id(&id);
                let selected = id == current;
                if ui.selectable_config(&name).selected(selected).build()
                    && !selected
                    && let Err(error) = self.select_theme(&id)
                {
                    self.error = Some(error.to_string());
                }
                if selected {
                    ui.set_item_default_focus();
                }
            }
        }
        if current.starts_with("themes/") && ui.button("Open Theme JSON") {
            match self.theme_path(&current) {
                Ok(path) => self.request_config_file = Some(path),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        ui.text_wrapped("Custom themes: copy a built-in theme JSON into the config directory's themes folder, then select it here.");
        ui.spacing();
    }

    fn draw_main_settings(&mut self, ui: &Ui) {
        let current = self.settings["font"]
            .as_str()
            .unwrap_or("Paper Mono")
            .to_owned();
        if let Some(_combo) = ui.begin_combo("Font", &current) {
            for name in bed_ui::font::available_fonts() {
                let selected = name == current;
                if ui.selectable_config(&name).selected(selected).build() {
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
        let size = self.font_size();
        if let Some(_combo) = ui.begin_combo("Font Size", format!("{size:.0}")) {
            for value in 10..=40 {
                let selected = size == value as f32;
                if ui
                    .selectable_config(value.to_string())
                    .selected(selected)
                    .build()
                    && !selected
                {
                    self.settings["fontSize"] = json!(value);
                    self.request_apply();
                    self.persist_ui();
                }
                if selected {
                    ui.set_item_default_focus();
                }
            }
        }
        ui.spacing();
    }

    fn draw_autosave_settings(&mut self, ui: &Ui) {
        let mut autosave = self.autosave_enabled();
        if ui.checkbox("Autosave code files", &mut autosave) {
            self.settings["autosave"] = json!(autosave);
            self.persist_ui();
        }
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                "Save files after typing stops. New files require a path before autosave can run.",
            );
        }
        {
            let _disabled = ui.begin_disabled_with_cond(!autosave);
            let mut delay = self.autosave_delay().as_millis() as i32;
            if ui
                .slider_config("Autosave delay", 100, 60_000)
                .flags(
                    dear_imgui_rs::SliderFlags::ALWAYS_CLAMP
                        | dear_imgui_rs::SliderFlags::LOGARITHMIC,
                )
                .try_display_format("%d ms")
                .expect("constant valid format")
                .build(&mut delay)
            {
                self.settings["autosave_delay_ms"] = json!(delay);
            }
            if ui.is_item_deactivated_after_edit() {
                self.persist_ui();
            }
        }
    }

    fn draw_lsp_settings(&mut self, ui: &Ui) {
        self.draw_boolean(ui, "Automatic LSP completions", "lsp_autocomplete", true);
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                "Show completions while typing. The completion shortcut also works with automatic completions disabled.",
            );
        }
        {
            let _disabled = ui.begin_disabled_with_cond(!self.lsp_autocomplete_enabled());
            let mut delay = self.lsp_completion_delay().as_millis() as i32;
            if ui
                .slider_config("Completion delay", 0, 5_000)
                .flags(dear_imgui_rs::SliderFlags::ALWAYS_CLAMP)
                .try_display_format("%d ms")
                .expect("constant valid format")
                .build(&mut delay)
            {
                self.settings["lsp_completion_delay_ms"] = json!(delay);
                self.request_apply();
            }
            if ui.is_item_deactivated_after_edit() {
                self.persist_ui();
            }
        }
        self.draw_boolean(ui, "Format on explicit save", "lsp_format_on_save", true);
        if ui.is_item_hovered() {
            tooltip_text(
                ui,
                "Use the language server formatter before saving. Background autosave does not format files.",
            );
        }
    }

    fn draw_mac_settings(&mut self, ui: &Ui) {
        ui.spacing();
        let mut opacity = self.background_opacity();
        if ui
            .slider_config("Background Opacity", 0.0, 1.0)
            .try_display_format("%.2f")
            .expect("constant valid format")
            .build(&mut opacity)
        {
            self.settings["background_opacity"] = json!(opacity);
            self.request_apply();
        }
        if ui.is_item_deactivated_after_edit() {
            self.persist_ui();
        }
        #[cfg(target_os = "macos")]
        self.draw_boolean(ui, "Enable Background Blur", "mac_blur_enabled", true);
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
        let mut enabled = self.bool("shader_toggle", false);
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

    fn draw_keybinds_settings(&mut self, ui: &Ui) {
        use bed_keybindings::{Bindings, CollisionKind, Platform, Stroke};
        let size = ui.current_font_size();
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([size * 0.65, size * 0.55]));
        let _padding = ui.push_style_var(StyleVar::FramePadding([size * 0.6, size * 0.3]));
        ui.dummy([0.0, size * 0.5]);
        ui.text_wrapped("Common shortcuts include application commands and actions handled by the focused panel or widget.");
        ui.input_text(
            "Search actions or shortcuts",
            &mut self.window.shortcut_filter,
        )
        .build();
        if let Some(error) = self.keybinds.reload_error.clone() {
            ui.text_wrapped(error);
        }
        if ui.button("Open Keybindings File") {
            self.request_config_file = Some(self.keybinds.path().to_owned());
        }
        let actions = self.window.shortcut_actions.clone();
        let bindings = Bindings::new(
            actions.clone(),
            &self.keybinds.overrides,
            Platform::current(),
        );
        let collisions = bindings.collisions();
        let filter = self.window.shortcut_filter.to_lowercase();
        let mut edit: Option<(bed_keybindings::Action, Option<usize>, Stroke)> = None;
        if let Some((id, index)) = self.window.shortcut_recording.clone() {
            ui.separator();
            let action = actions.iter().find(|a| a.id == id);
            let position = ui.cursor_screen_pos();
            let pulse = (ui.time() as f32 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            ui.get_window_draw_list()
                .add_circle(
                    [position[0] + size * 0.35, position[1] + size * 0.5],
                    size * (0.22 + pulse * 0.05),
                    [0.95, 0.2, 0.22, 0.45 + pulse * 0.55],
                )
                .filled(true)
                .build();
            ui.dummy([size * 0.7, size]);
            ui.same_line();
            ui.text(format!(
                "Recording: {}",
                action.map_or(id.as_str(), |a| a.label.as_str())
            ));
            ui.text_disabled("Press a combination. Escape cancels.");
            let cancel =
                ui.button("Cancel recording") || ui.is_key_pressed_with_repeat(Key::Escape, false);
            ui.same_line();
            let escape = ui.button("Assign Escape");
            if cancel {
                self.window.shortcut_recording = None;
            } else if let Some(action) = action {
                let captured = if escape {
                    Some("Escape".parse().unwrap())
                } else if ui.frame_count() > self.window.shortcut_recording_frame {
                    bed_keybindings::record(ui)
                } else {
                    None
                };
                if let Some(stroke) = captured {
                    edit = Some((action.clone(), index, stroke));
                    self.window.shortcut_recording = None;
                }
            }
            ui.separator();
        }
        let mut groups = actions
            .iter()
            .map(|a| match &a.scope {
                bed_keybindings::Scope::Global | bed_keybindings::Scope::Focused => {
                    "Common".to_owned()
                }
                bed_keybindings::Scope::Panel(id) => id.clone(),
            })
            .collect::<Vec<_>>();
        groups.sort();
        groups.dedup();
        if let Some(index) = groups.iter().position(|g| g == "Common") {
            let common = groups.remove(index);
            groups.insert(0, common);
        }
        for group in groups {
            let header = if group == bed_keybindings::EDITOR_SCOPE {
                "Editor".to_owned()
            } else {
                self.window
                    .shortcut_groups
                    .get(&group)
                    .cloned()
                    .unwrap_or_else(|| group.clone())
            };
            let matching = actions
                .iter()
                .filter(|a| match &a.scope {
                    bed_keybindings::Scope::Global | bed_keybindings::Scope::Focused => {
                        group == "Common"
                    }
                    bed_keybindings::Scope::Panel(id) => id == &group,
                })
                .filter(|action| {
                    format!(
                        "{} {} {} {}",
                        action.id,
                        action.label,
                        action.owner,
                        bindings
                            .for_action(&action.id)
                            .iter()
                            .map(|b| b.shortcut.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .to_lowercase()
                    .contains(&filter)
                })
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            if !filter.is_empty() {
                ui.set_next_item_open(true);
            }
            ui.dummy([0.0, size * 0.35]);
            if !ui.collapsing_header(&header, dear_imgui_rs::TreeNodeFlags::empty()) {
                continue;
            }
            ui.indent_by(size * 0.8);
            for action in matching {
                ui.dummy([0.0, size * 0.45]);
                let _id = ui.push_id(&action.id);
                ui.text(&action.label);
                ui.same_line();
                ui.text_disabled(
                    if self.keybinds.overrides.actions.contains_key(&action.id) {
                        "Custom"
                    } else {
                        "Default"
                    },
                );
                let shortcuts = self
                    .keybinds
                    .overrides
                    .actions
                    .get(&action.id)
                    .map(|v| v.iter().map(|a| a.shortcut).collect::<Vec<_>>())
                    .unwrap_or_else(|| action.defaults.clone());
                if shortcuts.is_empty() {
                    ui.text_disabled("Unbound");
                }
                let mut remove = None;
                for (index, stroke) in shortcuts.iter().enumerate() {
                    let _index = ui.push_id(index as i32);
                    if ui.button(stroke.label(Platform::current())) {
                        self.window.shortcut_recording = Some((action.id.clone(), Some(index)));
                        self.window.shortcut_recording_frame = ui.frame_count();
                    }
                    ui.same_line();
                    if ui.small_button("Remove") {
                        remove = Some(index);
                    }
                }
                if let Some(index) = remove {
                    if let Err(error) = self.keybinds.remove(action, index) {
                        self.error = Some(error.to_string());
                    }
                }
                if ui.small_button("Add binding") {
                    self.window.shortcut_recording = Some((action.id.clone(), None));
                    self.window.shortcut_recording_frame = ui.frame_count();
                }
                ui.same_line();
                if ui.small_button("Clear") {
                    if let Err(error) = self.keybinds.clear(&action.id) {
                        self.error = Some(error.to_string());
                    }
                }
                ui.same_line();
                if ui.small_button("Restore defaults") {
                    if let Err(error) = self.keybinds.restore(&action.id) {
                        self.error = Some(error.to_string());
                    }
                }
                if self.keybinds.overrides.actions.contains_key(&action.id) {
                    ui.text_disabled(format!(
                        "Defaults: {}",
                        if action.defaults.is_empty() {
                            "Unbound".into()
                        } else {
                            action
                                .defaults
                                .iter()
                                .map(|s| s.label(Platform::current()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ));
                }
                for warning in collisions.iter().filter(|c| {
                    c.kind != CollisionKind::Reuse
                        && (c.first == action.id || c.second == action.id)
                }) {
                    let other = if warning.first == action.id {
                        &warning.second
                    } else {
                        &warning.first
                    };
                    let other = actions
                        .iter()
                        .find(|a| &a.id == other)
                        .map_or(other.as_str(), |a| a.label.as_str());
                    let _font =
                        ui.push_font_with_size(None, ui.clone_style().font_size_base() * 0.85);
                    let _color = ui.push_style_color(StyleColor::Text, [0.78, 0.57, 0.34, 1.0]);
                    ui.text_wrapped(format!(
                        "{} is also assigned to {}.",
                        warning.shortcut, other
                    ));
                }
                ui.dummy([0.0, size * 0.3]);
                ui.separator();
            }
            ui.unindent_by(size * 0.8);
        }
        if actions.is_empty() {
            ui.text_disabled("No shortcut actions registered by this host.");
        }
        if let Some((action, index, stroke)) = edit {
            if let Err(error) = self.keybinds.assign(&action, index, stroke) {
                self.error = Some(error.to_string());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bed_config::read_json;
    use dear_imgui_rs::Context;
    use std::path::PathBuf;
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
    fn lsp_completion_and_explicit_save_controls_persist_their_preferences() {
        use dear_imgui_rs::FramePrepareOptions;
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut current = settings(&temp);
        let mut state = SettingsWindowState::default();
        let mut context = settings_context();
        let draw =
            |context: &mut Context, current: &mut Settings, state: &mut SettingsWindowState| {
                context.prepare_frame(FramePrepareOptions::new([600.0, 320.0], 1.0 / 60.0));
                let ui = context.frame();
                let mut targets = [[0.0; 2]; 2];
                ui.window("LSP preferences")
                    .position([0.0; 2], Condition::Always)
                    .size([600.0, 320.0], Condition::Always)
                    .build(|| {
                        let start = ui.cursor_screen_pos();
                        targets[0] = [
                            start[0] + ui.frame_height() * 0.5,
                            start[1] + ui.frame_height() * 0.5,
                        ];
                        SettingsView::new(current, state).draw_lsp_settings(ui);
                        let minimum = ui.item_rect_min();
                        let maximum = ui.item_rect_max();
                        targets[1] = [
                            (minimum[0] + maximum[0]) * 0.5,
                            (minimum[1] + maximum[1]) * 0.5,
                        ];
                    });
                drop(context.render_legacy());
                targets
            };
        draw(&mut context, &mut current, &mut state);
        let targets = draw(&mut context, &mut current, &mut state);
        for target in targets {
            context.io_mut().add_mouse_pos_event(target);
            draw(&mut context, &mut current, &mut state);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            draw(&mut context, &mut current, &mut state);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            draw(&mut context, &mut current, &mut state);
        }
        assert!(!current.lsp_autocomplete_enabled());
        assert!(!current.lsp_format_on_save_enabled());
        let restarted = settings(&temp);
        assert!(!restarted.lsp_autocomplete_enabled());
        assert!(!restarted.lsp_format_on_save_enabled());
        assert_eq!(
            restarted.lsp_completion_delay(),
            std::time::Duration::from_millis(150)
        );
    }

    #[test]
    fn effects_cursor_smear_defaults_on_and_persists_when_shaders_are_unavailable() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut current = settings(&temp);
        let mut state = SettingsWindowState {
            category: SettingsCategory::Effects,
            ..Default::default()
        };
        let mut editor = Editor::new();
        let mut context = settings_context();
        assert!(current.bool("cursor_smear", true));
        current.refresh();
        let draw = |context: &mut Context,
                    current: &mut Settings,
                    state: &mut SettingsWindowState,
                    editor: &mut Editor| {
            context.prepare_frame(FramePrepareOptions::new([900.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut checkbox = [0.0; 2];
            ui.window("Cursor smear settings")
                .position([0.0; 2], Condition::Always)
                .size([900.0, 600.0], Condition::Always)
                .build(|| {
                    SettingsView::new(current, state).draw_categories(
                        ui,
                        editor,
                        false,
                        &mut |_, _| false,
                    );
                    checkbox = ui.with_bound_context(|| unsafe {
                        let native = &*sys::igGetCurrentContext();
                        let window = (0..native.Windows.Size)
                            .map(|index| &**native.Windows.Data.add(index as usize))
                            .find(|window| {
                                std::ffi::CStr::from_ptr(window.Name)
                                    .to_string_lossy()
                                    .contains("SettingsContentEffects")
                            })
                            .expect("Effects category rendered without shader support");
                        [
                            window.DC.CursorStartPos.x + ui.frame_height() * 0.5,
                            window.DC.CursorStartPos.y
                                + ui.text_line_height()
                                + native.Style.ItemSpacing.y * 2.0
                                + ui.frame_height() * 0.5,
                        ]
                    });
                });
            drop(context.render_legacy());
            checkbox
        };
        draw(&mut context, &mut current, &mut state, &mut editor);
        let checkbox = draw(&mut context, &mut current, &mut state, &mut editor);
        context.io_mut().add_mouse_pos_event(checkbox);
        draw(&mut context, &mut current, &mut state, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut current, &mut state, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw(&mut context, &mut current, &mut state, &mut editor);
        assert!(
            !current.bool("cursor_smear", true),
            "Effects checkbox turns the smear off"
        );
        assert!(
            current.refresh(),
            "toggle requests an immediate presentation refresh"
        );
        assert_eq!(
            read_json(&current.settings_path).unwrap()["cursor_smear"],
            false
        );
        assert!(
            !settings(&temp).bool("cursor_smear", true),
            "off state survives restarting"
        );
    }

    #[test]
    fn recording_alternatives_persists_and_escape_cancels_without_changing_bindings() {
        use bed_keybindings::{Bindings, Platform};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let action = bed_keybindings::common_actions()
            .into_iter()
            .find(|action| action.id == bed_keybindings::RESET_ZOOM)
            .unwrap();
        let mut state = SettingsWindowState::default();
        state.shortcut_actions = vec![action.clone()];
        state.shortcut_recording = Some((action.id.clone(), None));
        let mut context = settings_context();
        let draw =
            |context: &mut Context, settings: &mut Settings, state: &mut SettingsWindowState| {
                context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [1000.0, 800.0],
                    1.0 / 60.0,
                ));
                let ui = context.frame();
                ui.window("Shortcut settings")
                    .size([900.0, 700.0], Condition::Always)
                    .build(|| {
                        SettingsView::new(settings, state).draw_keybinds_settings(ui);
                    });
                drop(context.render_legacy());
            };
        context.io_mut().add_key_event(Key::ModCtrl, true);
        context.io_mut().add_key_event(Key::ModAlt, true);
        context.io_mut().add_key_event(Key::F12, true);
        draw(&mut context, &mut settings, &mut state);
        assert!(state.shortcut_recording.is_none());
        let model = Bindings::new(
            vec![action.clone()],
            &settings.keybinds.overrides,
            Platform::current(),
        );
        assert_eq!(model.for_action(&action.id).len(), 2);
        assert_eq!(
            model.for_action(&action.id)[1].shortcut.to_string(),
            "Ctrl+Alt+F12"
        );
        let disk = std::fs::read(settings.keybinds.path()).unwrap();
        assert_eq!(
            bed_keybindings::Overrides::parse(&disk).unwrap(),
            settings.keybinds.overrides
        );
        context.io_mut().add_key_event(Key::F12, false);
        context.io_mut().add_key_event(Key::ModCtrl, false);
        context.io_mut().add_key_event(Key::ModAlt, false);
        draw(&mut context, &mut settings, &mut state);
        state.shortcut_recording = Some((action.id.clone(), Some(0)));
        context.io_mut().add_key_event(Key::Escape, true);
        draw(&mut context, &mut settings, &mut state);
        assert!(state.shortcut_recording.is_none());
        assert_eq!(std::fs::read(settings.keybinds.path()).unwrap(), disk);
        settings.keybinds.clear(&action.id).unwrap();
        assert!(
            Bindings::new(
                vec![action.clone()],
                &settings.keybinds.overrides,
                Platform::current()
            )
            .for_action(&action.id)
            .is_empty()
        );
        settings.keybinds.restore(&action.id).unwrap();
        assert_eq!(
            Bindings::new(
                vec![action.clone()],
                &settings.keybinds.overrides,
                Platform::current()
            )
            .for_action(&action.id)
            .len(),
            1
        );
    }

    #[test]
    fn default_viewer_reset_control_persists_bundled_associations() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.settings["default_viewers"] = json!({"svg":"missing.viewer"});
        let mut state = SettingsWindowState::default();
        let mut context = settings_context();
        let draw =
            |context: &mut Context, settings: &mut Settings, state: &mut SettingsWindowState| {
                context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [1000.0, 800.0],
                    1.0 / 60.0,
                ));
                let ui = context.frame();
                let mut button = [0.0; 2];
                ui.window("Default viewer settings")
                    .position([0.0; 2], Condition::Always)
                    .size([900.0, 700.0], Condition::Always)
                    .build(|| {
                        SettingsView::new(settings, state).draw_default_viewers(ui);
                        let (min, max) = ui.item_rect();
                        button = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                    });
                drop(context.render_legacy());
                button
            };
        draw(&mut context, &mut settings, &mut state);
        let button = draw(&mut context, &mut settings, &mut state);
        context.io_mut().add_mouse_pos_event(button);
        draw(&mut context, &mut settings, &mut state);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut settings, &mut state);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw(&mut context, &mut settings, &mut state);
        assert_eq!(
            settings.settings["default_viewers"],
            bed_config::settings::bundled_default_viewers()
        );
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["default_viewers"],
            settings.settings["default_viewers"]
        );
    }

    #[test]
    fn theme_editor_requires_a_choice_and_keeps_the_draft_when_going_back() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut draft = ThemeDraft::default();
        let mut context = settings_context();
        let draw = |context: &mut Context,
                    settings: &mut Settings,
                    draft: &mut ThemeDraft,
                    activate: Option<&str>| {
            context.prepare_frame(FramePrepareOptions::new([1000.0, 1200.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut last_item = 0;
            ui.window("Theme choices")
                .position([0.0; 2], Condition::Always)
                .size([1000.0, 1200.0], Condition::Always)
                .build(|| {
                    crate::theme_editor::draw(ui, draft, settings);
                    ui.with_bound_context(|| unsafe {
                        last_item = (*sys::igGetCurrentContext()).LastItemData.ID;
                        if let Some(label) = activate {
                            sys::igActivateItemByID(ui.get_id(label).raw());
                        }
                    });
                });
            drop(context.render_legacy());
            last_item
        };
        draw(&mut context, &mut settings, &mut draft, None);
        let chooser_item = draw(&mut context, &mut settings, &mut draft, None);
        assert!(!draft.editing);
        assert!(draft.palette.is_none());

        draw(&mut context, &mut settings, &mut draft, Some("Clone Theme"));
        draw(&mut context, &mut settings, &mut draft, None);
        assert!(draft.editing);
        assert!(draft.destination.is_none());
        assert!(draft.dirty);
        assert_eq!(draft.palette.as_ref().unwrap().name, "Tokyo Night Copy");
        let editor_item = draw(&mut context, &mut settings, &mut draft, None);
        assert_ne!(chooser_item, editor_item);
        draft.palette.as_mut().unwrap().name = "My unsaved colors".into();

        draw(
            &mut context,
            &mut settings,
            &mut draft,
            Some("Back to Themes"),
        );
        draw(&mut context, &mut settings, &mut draft, None);
        assert!(!draft.editing);
        assert!(draft.dirty);
        assert_eq!(
            chooser_item,
            draw(&mut context, &mut settings, &mut draft, None)
        );
        draw(
            &mut context,
            &mut settings,
            &mut draft,
            Some("Continue editing My unsaved colors"),
        );
        draw(&mut context, &mut settings, &mut draft, None);
        assert!(draft.editing);
        assert_eq!(draft.palette.as_ref().unwrap().name, "My unsaved colors");
        assert_eq!(settings.settings["theme"], "tokyo");
        assert_eq!(fs::read_dir(temp.0.join("themes")).unwrap().count(), 0);
    }

    #[test]
    fn theme_editor_saves_and_applies_an_independent_view_draft() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let original_font = settings.settings["font"].clone();
        let mut states = [
            SettingsWindowState::default(),
            SettingsWindowState::default(),
        ];
        for (index, state) in states.iter_mut().enumerate() {
            let mut theme = settings.theme.clone();
            theme.name = format!("Custom view {index}");
            state.theme_editor.load(theme, None);
        }
        states[0].theme_editor.palette.as_mut().unwrap().background = [0.2, 0.3, 0.4, 1.0];
        let mut context = settings_context();
        let draw =
            |context: &mut Context, settings: &mut Settings, state: &mut SettingsWindowState| {
                context.prepare_frame(FramePrepareOptions::new([1000.0, 2000.0], 1.0 / 60.0));
                let ui = context.frame();
                let mut button = [0.0; 2];
                ui.window("Theme editing")
                    .position([0.0; 2], Condition::Always)
                    .size([1000.0, 2000.0], Condition::Always)
                    .build(|| {
                        for label in ["Syntax Colors", "Terminal Colors"] {
                            let id = ui.get_id(label).raw();
                            ui.with_bound_context(|| unsafe {
                                sys::ImGuiStorage_SetInt(sys::igGetStateStorage(), id, 1);
                            });
                        }
                        crate::theme_editor::draw(ui, &mut state.theme_editor, settings);
                        let (min, max) = ui.item_rect();
                        button = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                        ui.with_bound_context(|| unsafe {
                            assert_eq!(
                                (*sys::igGetCurrentContext()).LastItemData.ID,
                                sys::igGetID_Str(c"Save & Apply".as_ptr())
                            );
                        });
                    });
                drop(context.render_legacy());
                button
            };
        draw(&mut context, &mut settings, &mut states[0]);
        let button = draw(&mut context, &mut settings, &mut states[0]);
        context.io_mut().add_mouse_pos_event(button);
        draw(&mut context, &mut settings, &mut states[0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw(&mut context, &mut settings, &mut states[0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw(&mut context, &mut settings, &mut states[0]);
        assert_eq!(settings.settings["theme"], "themes/custom-view-0.json");
        assert_eq!(
            read_json(&temp.0.join("themes/custom-view-0.json")).unwrap()["ui"]["background"],
            "#334d66"
        );
        assert_eq!(settings.settings["font"], original_font);
        assert!(!settings.shader_settings().enabled);
        assert!(states[0].theme_editor.destination.is_some());
        assert!(states[1].theme_editor.destination.is_none());
        assert_eq!(
            states[1].theme_editor.palette.as_ref().unwrap().name,
            "Custom view 1"
        );
    }

    #[test]
    fn category_clicks_stay_with_their_settings_view() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut states = [
            SettingsWindowState::default(),
            SettingsWindowState::default(),
        ];
        states[1].category = SettingsCategory::Extensions;
        let mut editor = Editor::new();
        let mut context = settings_context();
        let mut extension_calls = [0; 2];
        let mut frame = |context: &mut Context, states: &mut [SettingsWindowState; 2]| {
            context.prepare_frame(FramePrepareOptions::new([1800.0, 1000.0], 1.0 / 60.0));
            let ui = context.frame();
            for (index, state) in states.iter_mut().enumerate() {
                ui.window(["Settings one", "Settings two"][index])
                    .position([10.0 + index as f32 * 800.0, 10.0], Condition::Always)
                    .size([760.0, 800.0], Condition::Always)
                    .build(|| {
                        settings.draw_tab_with_state(ui, state, &mut editor, None, &mut |ui, _| {
                            extension_calls[index] += 1;
                            ui.text("Extension preferences");
                            false
                        });
                    });
            }
            let bedtime = ui.with_bound_context(|| unsafe {
                let native = &*sys::igGetCurrentContext();
                (0..native.Windows.Size as usize)
                    .find_map(|index| {
                        let window = &**native.Windows.Data.add(index);
                        let name = std::ffi::CStr::from_ptr(window.Name).to_string_lossy();
                        (name.starts_with("Settings one/") && name.contains("SettingsCategories"))
                            .then(|| {
                                let height = window.FontRefSize;
                                [
                                    window.DC.CursorStartPos.x + height,
                                    window.DC.CursorStartPos.y
                                        + height
                                        + native.Style.ItemSpacing.y
                                        + height * 0.5,
                                ]
                            })
                    })
                    .expect("first view has a category sidebar")
            });
            drop(context.render_legacy());
            bedtime
        };
        frame(&mut context, &mut states);
        let bedtime = frame(&mut context, &mut states);
        context.io_mut().add_mouse_pos_event(bedtime);
        frame(&mut context, &mut states);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut states);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut states);
        frame(&mut context, &mut states);
        assert_eq!(states[0].category, SettingsCategory::Bedtime);
        assert_eq!(states[1].category, SettingsCategory::Extensions);
        assert_eq!(extension_calls[0], 0);
        assert_eq!(extension_calls[1], 6);
    }

    #[test]
    fn extension_settings_render_and_persist_in_tab_and_legacy_window() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut current = settings(&temp);
        let mut window_state = SettingsWindowState {
            category: SettingsCategory::Extensions,
            ..Default::default()
        };
        current.settings["plugins"]["unavailable"] = json!({"retained": true});
        let mut editor = Editor::new();
        let mut context = settings_context();
        context.io_mut().set_display_size([1800.0, 1200.0]);
        context.io_mut().set_delta_time(1.0 / 60.0);
        let mut tab_calls = 0;
        let ui = context.frame();
        ui.window("Settings tab")
            .size([1000.0, 1000.0], Condition::Always)
            .build(|| {
                current.draw_tab_with_state(
                    ui,
                    &mut window_state,
                    &mut editor,
                    None,
                    &mut |ui, value| {
                        tab_calls += 1;
                        ui.text("Image Viewer");
                        value["plugins"]["image"]["fit"] = json!(true);
                        true
                    },
                );
            });
        drop(context.render_legacy());
        assert_eq!(tab_calls, 1);
        assert_eq!(
            read_json(&current.settings_path).unwrap()["plugins"]["image"]["fit"],
            true
        );
        window_state.show_settings_window = true;
        let mut legacy_calls = 0;
        let ui = context.frame();
        current.draw_with_icons_and_extensions(
            ui,
            &mut window_state,
            &mut editor,
            true,
            None,
            &mut |ui, value| {
                legacy_calls += 1;
                ui.text("Image Viewer");
                value["plugins"]["image"]["zoom"] = json!(2.0);
                true
            },
        );
        drop(context.render_legacy());
        assert_eq!(legacy_calls, 1);
        let restarted = settings(&temp);
        assert_eq!(restarted.settings["plugins"]["image"]["fit"], true);
        assert_eq!(restarted.settings["plugins"]["image"]["zoom"], 2.0);
        assert_eq!(
            restarted.settings["plugins"]["unavailable"]["retained"],
            true
        );
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
        window_state: &mut SettingsWindowState,
        editor: &mut Editor,
    ) -> NativeSettingsWindow {
        draw_settings_with_icons(context, settings, window_state, editor, None)
    }

    fn draw_settings_with_icons(
        context: &mut Context,
        settings: &mut Settings,
        window_state: &mut SettingsWindowState,
        editor: &mut Editor,
        icons: Option<&bed_ui::icons::Icons>,
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
        settings.draw_with_icons(ui, window_state, editor, true, icons);
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
        let mut window_state = SettingsWindowState {
            show_settings_window: true,
            ..Default::default()
        };
        settings.is_embedded = true;
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
        let first = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
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
        let second = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
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
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context
            .io_mut()
            .add_mouse_pos_event([title[0] + 80.0, title[1] + 55.0]);
        let moved = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert_eq!(moved.pos, [280.0, 255.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context.binding().with_bound_context(|| unsafe {
            sys::igSetWindowSize_Str(
                c"Settings".as_ptr(),
                [640.0, 420.0].into(),
                sys::ImGuiCond_Always,
            );
        });
        let resized = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert_eq!(resized.pos, [280.0, 255.0]);
        assert_eq!(resized.size, [640.0, 420.0]);
        assert_eq!(window_state.embedded_window_pos, resized.pos);
        assert_eq!(window_state.embedded_window_size, resized.size);
        assert!(!window_state.embedded_window_collapsed);
        window_state.show_settings_window = false;
        settings.draw(context.frame(), &mut window_state, &mut editor, false);
        drop(context.render_legacy());
        window_state.show_settings_window = true;
        let reopened = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert_eq!(reopened.pos, resized.pos);
        assert_eq!(reopened.size, resized.size);
    }

    #[test]
    fn native_embedded_settings_close_button_escape_and_background_input_match_source() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut window_state = SettingsWindowState {
            show_settings_window: true,
            ..Default::default()
        };
        settings.is_embedded = true;
        let mut editor = Editor::new();
        let mut context = settings_context();
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        let window = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        settings.settings["test_close"] = json!("unsaved before title close");
        let font_size = context.style().font_size_base();
        let padding = context.style().frame_padding();
        let close = [
            window.pos[0] + window.size[0] - padding[0] - font_size * 0.5,
            window.pos[1] + padding[1] + font_size * 0.5,
        ];
        context.io_mut().add_mouse_pos_event(close);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert!(!window_state.show_settings_window);
        assert!(editor.view.block_input);
        assert!(read_json(&settings.settings_path).unwrap()["test_close"].is_null());
        window_state.show_settings_window = true;
        context.io_mut().add_mouse_pos_event([0.0, 0.0]);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context.io_mut().add_key_event(Key::Escape, true);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert!(!window_state.show_settings_window);
        assert!(!editor.view.block_input);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["test_close"],
            "unsaved before title close"
        );
        context.io_mut().add_key_event(Key::Escape, false);
        window_state.show_settings_window = true;
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
        assert!(!window_state.show_settings_window);
        assert!(!editor.view.block_input);
    }

    #[test]
    fn native_standalone_settings_keeps_centered_fixed_window_geometry() {
        use dear_imgui_rs::sys;
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut window_state = SettingsWindowState {
            show_settings_window: true,
            ..Default::default()
        };
        let mut context = settings_context();
        let mut editor = Editor::new();
        let window = draw_settings(&mut context, &mut settings, &mut window_state, &mut editor);
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
        use bed_ui::icons::Icons;
        use dear_imgui_rs::{TextureId, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut window_state = SettingsWindowState {
            show_settings_window: true,
            ..Default::default()
        };
        let mut context = settings_context();
        let mut editor = Editor::new();
        let mut icons = Icons::default();
        icons.set_texture("close", TextureId::new(99));
        draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        let window = draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        let icon = window.close_icon.unwrap();
        let side = context.style().font_size_base();
        assert_eq!(
            [icon.max[0] - icon.min[0], icon.max[1] - icon.min[1]],
            [side; 2]
        );
        context.io_mut().add_mouse_pos_event([
            (icon.min[0] + icon.max[0]) * 0.5,
            (icon.min[1] + icon.max[1]) * 0.5,
        ]);
        let hovered = draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
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
        draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        assert!(!window_state.show_settings_window);
        assert!(!editor.view.block_input);
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["test_close"],
            "header persists"
        );
    }

    #[test]
    fn light_settings_header_tints_the_close_icon_with_readable_theme_ink() {
        use bed_ui::icons::Icons;
        use dear_imgui_rs::TextureId;
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        let mut window_state = SettingsWindowState {
            show_settings_window: true,
            ..Default::default()
        };
        let background = [0.97, 0.94, 0.89, 1.0];
        let text = [0.12, 0.18, 0.23, 1.0];
        settings.theme.background = background;
        settings.theme.foreground = text;
        let mut context = settings_context();
        context
            .style_mut()
            .set_color(StyleColor::WindowBg, background);
        context.style_mut().set_color(StyleColor::Text, text);
        let mut icons = Icons::default();
        icons.set_texture("close", TextureId::new(99));
        let mut editor = Editor::new();
        draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        let header = draw_settings_with_icons(
            &mut context,
            &mut settings,
            &mut window_state,
            &mut editor,
            Some(&icons),
        );
        for rgba in header.close_icon.unwrap().colors {
            let ink = [
                (rgba & 0xff) as f32 / 255.0,
                ((rgba >> 8) & 0xff) as f32 / 255.0,
                ((rgba >> 16) & 0xff) as f32 / 255.0,
                1.0,
            ];
            assert!(bed_ui::color::contrast_ratio(ink, background) >= 4.5);
            assert!(ink[0] < 0.5 && ink[1] < 0.5 && ink[2] < 0.5);
        }
        assert_eq!(context.style().color(StyleColor::Text), text);
    }

    #[cfg(target_os = "macos")]
    fn apply_pending(context: &mut Context, settings: &mut Settings) -> bool {
        let applied = settings.refresh();
        if applied {
            bed_ui::appearance::apply_style(context, &settings.appearance(), settings.is_embedded);
        }
        if applied {
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
        }
        applied
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_mac_opacity_and_blur_inputs_persist_and_request_apply() {
        use dear_imgui_rs::{FramePrepareOptions, sys};
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut settings = settings(&temp);
        settings.settings["background_opacity"] = json!(0.5);
        settings.settings["mac_blur_enabled"] = json!(true);
        settings.save_settings().unwrap();
        let mut context = settings_context();
        assert!(apply_pending(&mut context, &mut settings));
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
                    SettingsView::new(settings, &mut SettingsWindowState::default())
                        .draw_mac_settings(ui);
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
        let opacity = settings.number("background_opacity", 0.0);
        assert!(opacity > 0.7 && opacity < 0.8);
        assert!(apply_pending(&mut context, &mut settings));
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["background_opacity"],
            settings.settings["background_opacity"]
        );
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
        assert!(apply_pending(&mut context, &mut settings));
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["mac_blur_enabled"],
            false
        );
    }
}

#[cfg(test)]
use std::fs;
