//! Settings controls and the legacy floating settings window.
use bed_document_session::editor::Editor;
use bed_editing::util::color::{blend, ensure_contrast};
use bed_settings::{EffectPreset, PRIMARY_PROFILE, Settings, font::Font};
use bed_ui::icons::Icons;
use dear_imgui_rs::{
    Condition, Key, MouseButton, PopupQueryFlags, StyleColor, StyleVar, TreeNodeFlags, Ui,
    WindowFlags, WindowHoveredFlags,
};
use serde_json::{Value, json};
use std::{
    fs,
    ops::{Deref, DerefMut},
};

/// Presentation state for a floating settings window, separate from saved configuration.
pub struct SettingsWindowState {
    pub show_settings_window: bool,
    pub embedded_window_pos: [f32; 2],
    pub embedded_window_size: [f32; 2],
    pub embedded_window_collapsed: bool,
    was_focused: bool,
}
impl Default for SettingsWindowState {
    fn default() -> Self {
        Self {
            show_settings_window: false,
            embedded_window_pos: [200.0, 200.0],
            embedded_window_size: [900.0, 600.0],
            embedded_window_collapsed: false,
            was_focused: false,
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
        SettingsView::new(self, &mut SettingsWindowState::default())
            .draw_tab_with_extensions(ui, editor, icons, extensions);
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
        let background = (!self.is_embedded
            && self.settings["backgroundColor"]
                .as_array()
                .is_some_and(|colors| colors.len() >= 3))
        .then(|| {
            let bg = self.background_color();
            ui.push_style_color(
                StyleColor::ChildBg,
                blend(self.text_color(), [bg[0], bg[1], bg[2], 1.0], 0.025),
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
                    ui.text_colored(
                        ensure_contrast([1.0, 0.4, 0.3, 1.0], self.background_color(), 4.5),
                        error,
                    );
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
                if extensions(ui, &mut self.settings) {
                    self.persist_ui();
                }
            });
        drop(padding);
        drop(background);
        self.handle_window_input(ui, editor);
    }
    /// Controls only; Workbench owns this tab and its focus/close behavior.
    pub fn draw_tab_with_extensions(
        &mut self,
        ui: &Ui,
        editor: &mut Editor,
        icons: Option<&Icons>,
        extensions: &mut dyn FnMut(&Ui, &mut Value) -> bool,
    ) {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        if let Some(error) = &self.error {
            ui.text_colored(
                ensure_contrast([1.0, 0.4, 0.3, 1.0], self.background_color(), 4.5),
                error,
            );
        }
        self.draw_profile_selector(ui);
        self.draw_main_settings(ui);
        self.draw_mac_settings(ui);
        self.draw_syntax_colors(ui);
        self.draw_toggle_settings(ui, editor);
        self.draw_shader_settings(ui);
        self.draw_keybinds_settings(ui, editor.lsp_client().is_some());
        if extensions(ui, &mut self.settings) {
            self.persist_ui();
        }
        let _ = icons;
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
        ui.spacing();
        let mut autosave = self.autosave_enabled();
        if ui.checkbox("Autosave code files", &mut autosave) {
            self.settings["autosave"] = json!(autosave);
            self.persist_ui();
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
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
                "UI Animations",
                "ui_animations",
                "(Animate panels, tree branches, popovers and navigation jumps)",
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
                self.window.show_settings_window = false;
            } else {
                eprintln!("[Settings] Keybinds file not found: {}", keybinds.display());
            }
        }
        ui.same_line();
        ui.text_disabled("(Edit keyboard shortcuts)");
        if defaults.is_file() && !keybinds.exists() {
            ui.text_colored(
                ensure_contrast([1.0, 0.5, 0.0, 1.0], self.background_color(), 4.5),
                "Using default keybinds",
            );
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
                self.window.show_settings_window = false;
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
    use bed_settings::read_json;
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
    fn extension_settings_render_and_persist_in_tab_and_legacy_window() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut current = settings(&temp);
        let mut window_state = SettingsWindowState::default();
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
                current.draw_tab_with_extensions(ui, &mut editor, None, &mut |ui, value| {
                    tab_calls += 1;
                    ui.text("Image Viewer");
                    value["plugins"]["image"]["fit"] = json!(true);
                    true
                });
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
        settings.settings["themes"] =
            json!({"old": {"text": [0.1, 0.2, 0.3, 1.0], "variable": [0.4, 0.5, 0.6, 1.0]}});
        settings.settings["theme"] = json!("old");
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
        settings.settings["backgroundColor"] = json!(background);
        settings.settings["themes"]["default"]["text"] = json!(text);
        settings.settings["theme"] = json!("default");
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
            assert!(bed_editing::util::color::contrast_ratio(ink, background) >= 4.5);
            assert!(ink[0] < 0.5 && ink[1] < 0.5 && ink[2] < 0.5);
        }
        assert_eq!(context.style().color(StyleColor::Text), text);
    }

    #[cfg(target_os = "macos")]
    fn apply_pending(context: &mut Context, settings: &mut Settings) -> bool {
        let applied = settings.apply(context, &mut Editor::new()).unwrap();
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
        settings.settings["mac_background_opacity"] = json!(0.5);
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
        let opacity = settings.number("mac_background_opacity", 0.0);
        assert!(opacity > 0.7 && opacity < 0.8);
        assert!(apply_pending(&mut context, &mut settings));
        assert_eq!(
            read_json(&settings.settings_path).unwrap()["mac_background_opacity"],
            settings.settings["mac_background_opacity"]
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
